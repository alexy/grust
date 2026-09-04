//! Bounded Arrow staging for governed cognition inputs.

use std::io::{self, Write};
use std::sync::Arc;

use arrow::array::{Int64Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::StreamWriter;
use typesec_memory::{MemoryContent, Provenance, RecalledMemory};

use crate::analytics::planning::{normalize_text, normalized_assertion_parts};
use crate::cognition::{CognitionError, budget};

pub(super) fn encode(memories: &[RecalledMemory]) -> Result<Vec<u8>, CognitionError> {
    budget::check_authorized_input(memories)?;

    let normalized: Vec<_> = memories
        .iter()
        .map(|memory| normalize_text(&memory.content.text))
        .collect();
    let parts: Vec<_> = normalized
        .iter()
        .map(|text| normalized_assertion_parts(text))
        .collect();
    check_string_bytes(memories, &normalized, &parts)?;

    let schema = schema();
    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![
            Arc::new(StringArray::from_iter_values(
                memories.iter().map(|memory| memory.id.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                normalized.iter().map(String::as_str),
            )),
            Arc::new(StringArray::from_iter_values(
                parts.iter().map(|part| part.0.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                parts.iter().map(|part| part.1.as_str()),
            )),
            Arc::new(Int64Array::from_iter_values(
                memories
                    .iter()
                    .map(|memory| memory.valid_from.timestamp_micros()),
            )),
        ],
    )
    .map_err(|_| serialization_error("invalid governed Arrow request"))?;
    encode_batch(&schema, &batch)
}

pub(super) fn owned_planning_memories(memories: &[RecalledMemory]) -> Vec<RecalledMemory> {
    memories
        .iter()
        .map(|memory| RecalledMemory {
            id: memory.id.clone(),
            kind: memory.kind,
            label: memory.label,
            content: MemoryContent::text(memory.content.text.clone()),
            entities: Vec::new(),
            provenance: Provenance::Operator,
            valid_from: memory.valid_from,
        })
        .collect()
}

fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("normalized", DataType::Utf8, false),
        Field::new("prefix", DataType::Utf8, false),
        Field::new("tail", DataType::Utf8, false),
        Field::new("valid_from", DataType::Int64, false),
    ]))
}

fn check_string_bytes(
    memories: &[RecalledMemory],
    normalized: &[String],
    parts: &[(String, String)],
) -> Result<(), CognitionError> {
    let bytes = memories
        .iter()
        .map(|memory| memory.id.as_str().len())
        .chain(normalized.iter().map(String::len))
        .chain(parts.iter().flat_map(|part| [part.0.len(), part.1.len()]))
        .try_fold(0usize, usize::checked_add)
        .unwrap_or(usize::MAX);
    budget::check_arrow_bytes(bytes)
}

fn encode_batch(schema: &Schema, batch: &RecordBatch) -> Result<Vec<u8>, CognitionError> {
    let mut output = BoundedBuffer::new(budget::MAX_ARROW_BYTES);
    let encoded = {
        let mut writer = StreamWriter::try_new(&mut output, schema)
            .map_err(|_| serialization_error("Arrow request encoding failed"))?;
        writer
            .write(batch)
            .and_then(|()| writer.finish())
            .map_err(|_| serialization_error("Arrow request encoding failed"))
    };
    if output.exceeded {
        return Err(CognitionError::ResourceBudgetExceeded("Arrow bytes"));
    }
    encoded?;
    Ok(output.bytes)
}

fn serialization_error(message: &'static str) -> CognitionError {
    CognitionError::Serialization(message)
}

struct BoundedBuffer {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl BoundedBuffer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            exceeded: false,
        }
    }
}

impl Write for BoundedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let remaining = self.limit.saturating_sub(self.bytes.len());
        if bytes.len() > remaining {
            self.exceeded = true;
            return Err(io::Error::other("Arrow byte budget exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
