//! Live Spark Connect implementation of the governed cognition executor.

use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::sync::Arc;

use arrow::array::{Array, Int64Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use grust_sail::SailGraphStore;
use sha2::{Digest, Sha256};
use typesec_memory::{
    ConsolidationPlan, ConsolidationStep, MemoryContent, MemoryDraft, MemoryId, Provenance,
};

use super::{
    CognitionError, CognitionOperation, CognitionRequest, SailCognitionExecutor,
    SailCognitionOutput,
};

/// Executes bounded cognition analysis on a live Sail Spark Connect service.
pub struct LiveSailCognitionExecutor {
    store: Arc<SailGraphStore>,
}

impl LiveSailCognitionExecutor {
    /// Use an established Sail session. The session owns the temporary Arrow views.
    pub fn new(store: Arc<SailGraphStore>) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl SailCognitionExecutor for LiveSailCognitionExecutor {
    async fn execute(
        &self,
        request: &CognitionRequest<'_>,
    ) -> Result<SailCognitionOutput, CognitionError> {
        request.source.digest()?;
        if request.job_id.trim().is_empty() {
            return Err(CognitionError::MissingJobId);
        }

        let view = job_view(request.job_id);
        let ipc = request_batch(request.memories)?;
        self.store
            .stage_arrow_ipc_view(&view, &ipc)
            .await
            .map_err(|error| CognitionError::Sail(error.to_string()))?;

        let (plan, count) = match request.operation {
            CognitionOperation::Deduplicate => {
                let sql = format!(
                    "SELECT normalized, id FROM {view} WHERE normalized IN \
                     (SELECT normalized FROM {view} GROUP BY normalized HAVING COUNT(*) > 1) \
                     ORDER BY normalized, valid_from, id"
                );
                let rows = query_pairs(&self.store, &sql).await?;
                dedup_plan(request, rows)
            }
            CognitionOperation::Reconcile => {
                let sql = format!(
                    "SELECT newer.id, older.id FROM {view} newer JOIN {view} older \
                     ON newer.prefix = older.prefix AND newer.tail <> older.tail \
                     AND (newer.valid_from > older.valid_from OR \
                          (newer.valid_from = older.valid_from AND newer.id > older.id)) \
                     WHERE newer.prefix <> '' ORDER BY newer.id, older.id"
                );
                let rows = query_pairs(&self.store, &sql).await?;
                reconcile_plan(rows)
            }
        };

        Ok(SailCognitionOutput {
            plan,
            evidence: vec![format!(
                "sail operation={} candidates={count} snapshot={}",
                request.operation.name(),
                request.source.snapshot_id
            )],
            executor_version: env!("CARGO_PKG_VERSION").to_owned(),
        })
    }
}

fn job_view(job_id: &str) -> String {
    let digest = Sha256::digest(job_id.as_bytes());
    format!("marciana_{}", &format!("{digest:x}")[..24])
}

fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn assertion_parts(text: &str) -> (String, String) {
    let normalized = normalize(text);
    let Some((prefix, tail)) = normalized.rsplit_once(' ') else {
        return (String::new(), String::new());
    };
    (prefix.to_owned(), tail.to_owned())
}

fn request_batch(memories: &[typesec_memory::RecalledMemory]) -> Result<Vec<u8>, CognitionError> {
    let ids = StringArray::from_iter_values(memories.iter().map(|m| m.id.as_str()));
    let texts = StringArray::from_iter_values(memories.iter().map(|m| m.content.text.as_str()));
    let normalized =
        StringArray::from_iter_values(memories.iter().map(|m| normalize(&m.content.text)));
    let parts: Vec<_> = memories
        .iter()
        .map(|m| assertion_parts(&m.content.text))
        .collect();
    let prefixes = StringArray::from_iter_values(parts.iter().map(|p| p.0.as_str()));
    let tails = StringArray::from_iter_values(parts.iter().map(|p| p.1.as_str()));
    let valid =
        Int64Array::from_iter_values(memories.iter().map(|m| m.valid_from.timestamp_micros()));
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("text", DataType::Utf8, false),
        Field::new("normalized", DataType::Utf8, false),
        Field::new("prefix", DataType::Utf8, false),
        Field::new("tail", DataType::Utf8, false),
        Field::new("valid_from", DataType::Int64, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(ids),
            Arc::new(texts),
            Arc::new(normalized),
            Arc::new(prefixes),
            Arc::new(tails),
            Arc::new(valid),
        ],
    )
    .map_err(|error| CognitionError::Serialization(error.to_string()))?;
    let mut data = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut data, &schema)
            .map_err(|error| CognitionError::Serialization(error.to_string()))?;
        writer
            .write(&batch)
            .map_err(|error| CognitionError::Serialization(error.to_string()))?;
        writer
            .finish()
            .map_err(|error| CognitionError::Serialization(error.to_string()))?;
    }
    Ok(data)
}

async fn query_pairs(
    store: &SailGraphStore,
    sql: &str,
) -> Result<Vec<(String, String)>, CognitionError> {
    let chunks = store
        .query_arrow_ipc(sql)
        .await
        .map_err(|error| CognitionError::Sail(error.to_string()))?;
    let mut rows = Vec::new();
    for chunk in chunks {
        let reader = StreamReader::try_new(Cursor::new(chunk), None)
            .map_err(|error| CognitionError::Sail(format!("invalid Arrow result: {error}")))?;
        for batch in reader {
            let batch = batch
                .map_err(|error| CognitionError::Sail(format!("invalid Arrow batch: {error}")))?;
            let left = batch
                .column(0)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| CognitionError::Sail("first result column is not UTF-8".into()))?;
            let right = batch
                .column(1)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| CognitionError::Sail("second result column is not UTF-8".into()))?;
            for row in 0..batch.num_rows() {
                if !left.is_null(row) && !right.is_null(row) {
                    rows.push((left.value(row).to_owned(), right.value(row).to_owned()));
                }
            }
        }
    }
    Ok(rows)
}

fn dedup_plan(
    request: &CognitionRequest<'_>,
    rows: Vec<(String, String)>,
) -> (ConsolidationPlan, usize) {
    let by_id: HashMap<_, _> = request
        .memories
        .iter()
        .map(|m| (m.id.as_str(), m))
        .collect();
    let mut groups: BTreeMap<String, Vec<MemoryId>> = BTreeMap::new();
    for (key, id) in rows {
        groups
            .entry(key)
            .or_default()
            .push(MemoryId::from_string(id));
    }
    let count = groups.len();
    let mut plan = ConsolidationPlan::new();
    for ids in groups.into_values() {
        let Some(first) = ids.first().and_then(|id| by_id.get(id.as_str())) else {
            continue;
        };
        let oldest = ids
            .iter()
            .filter_map(|id| by_id.get(id.as_str()))
            .map(|m| m.valid_from)
            .min()
            .unwrap_or(first.valid_from);
        let replacement = MemoryDraft::new(
            first.kind,
            MemoryContent::text(first.content.text.clone()),
            Provenance::Operator,
        )
        .valid_from(oldest);
        plan = plan.then(ConsolidationStep::Supersede {
            superseded: ids,
            replacement,
        });
    }
    (plan, count)
}

fn reconcile_plan(rows: Vec<(String, String)>) -> (ConsolidationPlan, usize) {
    let mut older = rows.into_iter().map(|(_, id)| id).collect::<Vec<_>>();
    older.sort();
    older.dedup();
    let count = older.len();
    let plan = older
        .into_iter()
        .fold(ConsolidationPlan::new(), |plan, id| {
            plan.then(ConsolidationStep::Invalidate {
                ids: vec![MemoryId::from_string(id)],
            })
        });
    (plan, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_names_do_not_expose_job_ids() {
        let view = job_view("tenant-secret/job-42");
        assert!(view.starts_with("marciana_"));
        assert!(!view.contains("tenant"));
        assert_eq!(view.len(), 33);
    }

    #[test]
    fn assertion_split_matches_reference_heuristic() {
        assert_eq!(
            assertion_parts("Alice lives in Rome"),
            ("alice lives in".into(), "rome".into())
        );
        assert_eq!(assertion_parts("singleton"), (String::new(), String::new()));
    }
}
