//! Strict decoding and validation of untrusted Sail result rows.

use std::collections::{BTreeSet, HashMap};
use std::io::Cursor;

use arrow::array::{Array, StringArray};
use arrow::ipc::reader::StreamReader;
use typesec_memory::{ConsolidationPlan, RecalledMemory};

use super::SailCognitionSession;
use crate::analytics::planning::{deduplicate, normalize_text, reconcile};
use crate::cognition::CognitionError;
use crate::cognition::budget;

mod preflight;

pub(super) async fn query_pairs(
    store: &dyn SailCognitionSession,
    sql: &str,
) -> Result<Vec<(String, String)>, CognitionError> {
    let chunks = store
        .query_arrow_ipc_bounded(sql, budget::MAX_RESULT_CHUNKS, budget::MAX_ARROW_BYTES)
        .await
        .map_err(super::sail_query_error)?;
    decode_pair_chunks(chunks)
}

fn decode_pair_chunks(chunks: Vec<Vec<u8>>) -> Result<Vec<(String, String)>, CognitionError> {
    budget::check_result_chunks(chunks.len())?;
    let arrow_bytes = chunks
        .iter()
        .try_fold(0usize, |total, chunk| total.checked_add(chunk.len()));
    budget::check_arrow_bytes(arrow_bytes.unwrap_or(usize::MAX))?;
    preflight::validate_pair_streams(&chunks)?;

    let mut rows = Vec::new();
    for chunk in chunks {
        let reader = StreamReader::try_new(Cursor::new(chunk), None)
            .map_err(|_| sail_error("Sail returned invalid Arrow IPC"))?;
        for batch in reader {
            let batch = batch.map_err(|_| sail_error("Sail returned an invalid Arrow batch"))?;
            let row_count = rows.len().saturating_add(batch.num_rows());
            budget::check_result_rows(row_count)?;
            if batch.num_columns() != 2 {
                return Err(sail_error("Sail result must contain exactly two columns"));
            }
            let left = batch
                .column(0)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| sail_error("first result column is not UTF-8"))?;
            let right = batch
                .column(1)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| sail_error("second result column is not UTF-8"))?;
            for row in 0..batch.num_rows() {
                if left.is_null(row) || right.is_null(row) {
                    return Err(sail_error("Sail result contains a null pair value"));
                }
                let pair = (left.value(row), right.value(row));
                if pair.0.trim().is_empty() || pair.1.trim().is_empty() {
                    return Err(sail_error("Sail result contains an empty pair value"));
                }
                rows.push((pair.0.to_owned(), pair.1.to_owned()));
            }
        }
    }
    Ok(rows)
}

pub(super) fn dedup_plan(
    memories: &[RecalledMemory],
    rows: Vec<(String, String)>,
) -> Result<(ConsolidationPlan, usize), CognitionError> {
    let by_id = memories_by_id(memories)?;
    let planning = deduplicate(memories);
    let mut seen_ids = BTreeSet::new();
    let mut actual_rows = BTreeSet::new();
    for (key, id) in rows {
        if key.trim().is_empty() {
            return Err(sail_error("dedup result contains an empty group key"));
        }
        let memory = by_id
            .get(id.as_str())
            .ok_or_else(|| sail_error("dedup result references an unknown source id"))?;
        if normalize_text(&memory.content.text) != key {
            return Err(sail_error("dedup result key does not match its source"));
        }
        if !seen_ids.insert(id.clone()) {
            return Err(sail_error("dedup result repeats a source id"));
        }
        actual_rows.insert((key.clone(), id));
    }
    if actual_rows != planning.rows {
        return Err(sail_error(
            "dedup result does not exactly cover authorized duplicate rows",
        ));
    }

    Ok((planning.plan, planning.group_count))
}

pub(super) fn reconcile_plan(
    memories: &[RecalledMemory],
    rows: Vec<(String, String)>,
) -> Result<(ConsolidationPlan, usize), CognitionError> {
    let by_id = memories_by_id(memories)?;
    let planning = reconcile(memories);
    let mut pairs = BTreeSet::new();
    for (newer_id, older_id) in rows {
        if !pairs.insert((newer_id.clone(), older_id.clone())) {
            return Err(sail_error("reconcile result repeats a source pair"));
        }
        if newer_id == older_id {
            return Err(sail_error("reconcile result contains a self-pair"));
        }
        if !by_id.contains_key(newer_id.as_str()) {
            return Err(sail_error(
                "reconcile result references an unknown newer source id",
            ));
        }
        if !by_id.contains_key(older_id.as_str()) {
            return Err(sail_error(
                "reconcile result references an unknown older source id",
            ));
        }
    }
    if pairs != planning.pairs {
        return Err(sail_error(
            "reconcile result does not exactly cover authorized contradiction pairs",
        ));
    }

    Ok((planning.plan, planning.invalidated_count))
}

fn memories_by_id(
    memories: &[RecalledMemory],
) -> Result<HashMap<&str, &RecalledMemory>, CognitionError> {
    let by_id: HashMap<_, _> = memories
        .iter()
        .map(|memory| (memory.id.as_str(), memory))
        .collect();
    if by_id.len() != memories.len() {
        return Err(sail_error("authorized cognition input repeats a source id"));
    }
    Ok(by_id)
}

fn sail_error(message: &'static str) -> CognitionError {
    CognitionError::Sail(message)
}

#[cfg(test)]
mod tests;
