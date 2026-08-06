use std::sync::Arc;

use arrow::array::{ArrayRef, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use chrono::{TimeZone, Timelike, Utc};
use typesec_memory::{
    ConsolidationStep, Label, MemoryContent, MemoryId, MemoryKind, Provenance, RecalledMemory,
};

use super::{decode_pair_chunks, dedup_plan, reconcile_plan};
use crate::cognition::{CognitionError, budget};

#[test]
fn pair_decoder_rejects_a_non_pair_schema_without_panicking() {
    let chunk = ipc(
        vec![Field::new("only", DataType::Utf8, false)],
        vec![Arc::new(StringArray::from(vec!["value"]))],
    );
    assert!(matches!(
        decode_pair_chunks(vec![chunk]),
        Err(CognitionError::Sail(message)) if message.contains("exactly two columns")
    ));
}

#[test]
fn pair_decoder_rejects_null_rows_instead_of_dropping_them() {
    let chunk = ipc(
        vec![
            Field::new("left", DataType::Utf8, true),
            Field::new("right", DataType::Utf8, true),
        ],
        vec![
            Arc::new(StringArray::from(vec![Some("left"), None])),
            Arc::new(StringArray::from(vec![Some("right"), Some("right")])),
        ],
    );
    assert!(matches!(
        decode_pair_chunks(vec![chunk]),
        Err(CognitionError::Sail(message)) if message.contains("null pair")
    ));
}

#[test]
fn pair_decoder_rejects_excess_chunks_before_arrow_decoding() {
    let error = decode_pair_chunks(vec![Vec::new(); budget::MAX_RESULT_CHUNKS + 1])
        .expect_err("chunk budget");
    assert!(matches!(
        error,
        CognitionError::ResourceBudgetExceeded("Sail result chunks")
    ));
}

#[test]
fn dedup_rows_are_source_bound_and_form_real_groups() {
    let memories = vec![
        memory("older", "Same   finding", 0),
        memory("newer", "same finding", 1),
    ];
    let (plan, count) = dedup_plan(
        &memories,
        vec![
            ("same finding".into(), "newer".into()),
            ("same finding".into(), "older".into()),
        ],
    )
    .expect("valid duplicate group");
    assert_eq!(count, 1);
    let [
        ConsolidationStep::Supersede {
            superseded,
            replacement,
        },
    ] = plan.steps.as_slice()
    else {
        panic!("one supersede step expected")
    };
    assert_eq!(
        superseded,
        &[
            MemoryId::from_string("older"),
            MemoryId::from_string("newer")
        ]
    );
    let replacement = serde_json::to_value(replacement).expect("serialize replacement");
    assert_eq!(replacement["content"]["text"], "Same   finding");

    for malformed in [
        vec![("same finding".into(), "unknown".into())],
        vec![("same finding".into(), "older".into())],
        vec![
            ("same finding".into(), "older".into()),
            ("same finding".into(), "older".into()),
        ],
        vec![
            ("wrong key".into(), "older".into()),
            ("wrong key".into(), "newer".into()),
        ],
    ] {
        assert!(dedup_plan(&memories, malformed).is_err());
    }
    assert!(
        dedup_plan(&memories, Vec::new()).is_err(),
        "empty output cannot omit an authorized duplicate group"
    );

    let three = vec![
        memory("first", "same finding", 0),
        memory("second", "same finding", 1),
        memory("third", "same finding", 2),
    ];
    assert!(
        dedup_plan(
            &three,
            vec![
                ("same finding".into(), "first".into()),
                ("same finding".into(), "second".into()),
            ],
        )
        .is_err(),
        "partial output cannot omit one authorized duplicate row"
    );
}

#[test]
fn reconcile_rows_must_match_source_ids_and_precedence_semantics() {
    let memories = vec![
        memory("older", "Alice lives in Rome", 0),
        memory("newer", "Alice lives in Paris", 1),
    ];
    let (plan, count) = reconcile_plan(&memories, vec![("newer".into(), "older".into())])
        .expect("valid contradiction pair");
    assert_eq!(count, 1);
    assert!(matches!(
        plan.steps.as_slice(),
        [ConsolidationStep::Invalidate { ids }]
            if ids == &[MemoryId::from_string("older")]
    ));

    for malformed in [
        vec![("newer".into(), "unknown".into())],
        vec![("older".into(), "newer".into())],
        vec![("older".into(), "older".into())],
        vec![
            ("newer".into(), "older".into()),
            ("newer".into(), "older".into()),
        ],
    ] {
        assert!(reconcile_plan(&memories, malformed).is_err());
    }
    assert!(
        reconcile_plan(&memories, Vec::new()).is_err(),
        "empty output cannot omit an authorized contradiction pair"
    );
}

#[test]
fn reconcile_uses_sail_microsecond_precision_before_the_id_tiebreak() {
    let base = Utc
        .with_ymd_and_hms(2026, 8, 5, 12, 0, 0)
        .single()
        .expect("fixture time");
    let actually_newer = memory_at(
        "a-source",
        "Alice lives in Paris",
        base.with_nanosecond(900).expect("fixture nanos"),
    );
    let actually_older = memory_at(
        "z-source",
        "Alice lives in Rome",
        base.with_nanosecond(100).expect("fixture nanos"),
    );
    let (plan, count) = reconcile_plan(
        &[actually_newer, actually_older],
        vec![("z-source".into(), "a-source".into())],
    )
    .expect("SQL and local precedence agree");
    assert_eq!(count, 1);
    assert!(matches!(
        plan.steps.as_slice(),
        [ConsolidationStep::Invalidate { ids }]
            if ids == &[MemoryId::from_string("a-source")]
    ));
}

#[test]
fn output_validation_errors_never_echo_source_or_attacker_values() {
    let memories = vec![
        memory("protected-source-id", "Protected source plaintext", 0),
        memory("other-source-id", "Protected source plaintext", 1),
    ];
    let errors = [
        dedup_plan(
            &memories,
            vec![(
                "attacker-controlled-key".into(),
                "attacker-controlled-id".into(),
            )],
        )
        .expect_err("unknown id"),
        dedup_plan(
            &memories,
            vec![
                (
                    "attacker-controlled-key".into(),
                    "protected-source-id".into(),
                ),
                ("attacker-controlled-key".into(), "other-source-id".into()),
            ],
        )
        .expect_err("wrong group key"),
        reconcile_plan(
            &memories,
            vec![(
                "attacker-controlled-id".into(),
                "protected-source-id".into(),
            )],
        )
        .expect_err("unknown reconcile id"),
        dedup_plan(&memories, Vec::new()).expect_err("omitted duplicate group"),
    ];
    for error in errors {
        let message = error.to_string();
        for secret in [
            "Protected source plaintext",
            "protected-source-id",
            "other-source-id",
            "attacker-controlled-key",
            "attacker-controlled-id",
        ] {
            assert!(!message.contains(secret), "error exposed {secret:?}");
        }
    }
}

fn memory(id: &str, text: &str, second: u32) -> RecalledMemory {
    memory_at(
        id,
        text,
        Utc.with_ymd_and_hms(2026, 8, 5, 12, 0, second)
            .single()
            .expect("fixture time"),
    )
}

fn memory_at(id: &str, text: &str, valid_from: chrono::DateTime<Utc>) -> RecalledMemory {
    RecalledMemory {
        id: MemoryId::from_string(id),
        kind: MemoryKind::Semantic,
        label: Label::Internal,
        content: MemoryContent::text(text),
        entities: Vec::new(),
        provenance: Provenance::Operator,
        valid_from,
    }
}

fn ipc(fields: Vec<Field>, columns: Vec<ArrayRef>) -> Vec<u8> {
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(Arc::clone(&schema), columns).expect("record batch");
    let mut data = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut data, &schema).expect("IPC writer");
        writer.write(&batch).expect("write IPC batch");
        writer.finish().expect("finish IPC stream");
    }
    data
}
