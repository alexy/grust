use std::io::Write;

use arrow::ipc::reader::StreamReader;
use chrono::{TimeZone, Utc};
use typesec_memory::{
    Label, MAX_COGNITION_SOURCE_BYTES, MemoryContent, MemoryId, MemoryKind, Provenance,
    RecalledMemory,
};

use super::{BoundedBuffer, encode, owned_planning_memories};
use crate::cognition::CognitionError;

#[test]
fn staged_schema_contains_only_fields_used_by_sail_planning() {
    let encoded = encode(&[memory("source", "Protected raw text", 0)]).expect("encode request");
    let reader = StreamReader::try_new(encoded.as_slice(), None).expect("decode request");
    let schema = reader.schema();
    let fields: Vec<_> = schema
        .fields()
        .iter()
        .map(|field| field.name().to_owned())
        .collect();
    assert_eq!(fields, ["id", "normalized", "prefix", "tail", "valid_from"]);
    assert!(!fields.iter().any(|field| field == "text"));
}

#[test]
fn bounded_writer_accepts_its_limit_and_rejects_the_next_byte() {
    let mut output = BoundedBuffer::new(4);
    output.write_all(b"1234").expect("inclusive boundary");
    assert!(output.write_all(b"5").is_err());
    assert!(output.exceeded);
    assert_eq!(output.bytes, b"1234");
}

#[test]
fn sail_encoder_enforces_the_shared_authorized_input_byte_boundary() {
    const ID: &str = "source";
    let exact_text = "x".repeat(MAX_COGNITION_SOURCE_BYTES - ID.len());
    encode(&[memory(ID, &exact_text, 0)]).expect("inclusive raw input boundary");

    let over_text = format!("{exact_text}x");
    let error = encode(&[memory(ID, &over_text, 0)]).expect_err("raw input over budget");
    assert!(matches!(
        error,
        CognitionError::ResourceBudgetExceeded("authorized source input")
    ));
}

#[test]
fn detached_planning_input_drops_fields_the_worker_does_not_use() {
    let mut source = memory("source", "Protected raw text", 0);
    source
        .entities
        .push(typesec_memory::EntityRef::new("Alice", "person"));
    source
        .content
        .attributes
        .insert("large".into(), serde_json::json!([1, 2, 3]));
    let owned = owned_planning_memories(&[source]);
    assert!(owned[0].entities.is_empty());
    assert!(owned[0].content.attributes.is_empty());
    assert_eq!(owned[0].content.text, "Protected raw text");
}

fn memory(id: &str, text: &str, second: u32) -> RecalledMemory {
    RecalledMemory {
        id: MemoryId::from_string(id),
        kind: MemoryKind::Semantic,
        label: Label::Internal,
        content: MemoryContent::text(text),
        entities: Vec::new(),
        provenance: Provenance::Operator,
        valid_from: Utc
            .with_ymd_and_hms(2026, 8, 5, 12, 0, second)
            .single()
            .expect("fixture time"),
    }
}
