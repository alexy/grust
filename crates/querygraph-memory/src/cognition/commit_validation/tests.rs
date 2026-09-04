use super::*;
use typesec_memory::{EntityRef, StoredRecord};

#[test]
fn collection_limits_are_inclusive_and_reject_the_next_value() {
    validate_collection_sizes(
        CognitionEffect::Mutated,
        MAX_COGNITION_SOURCE_COUNT,
        MAX_COGNITION_MUTATIONS,
        MAX_COGNITION_OUTBOX_ENTRIES,
        MAX_COGNITION_MUTATIONS,
    )
    .expect("shared limits are inclusive");

    for counts in [
        (MAX_COGNITION_SOURCE_COUNT + 1, 1, 1, 1),
        (1, MAX_COGNITION_MUTATIONS + 1, 1, 1),
        (1, 1, MAX_COGNITION_OUTBOX_ENTRIES + 1, 1),
        (1, 1, 1, MAX_COGNITION_MUTATIONS + 1),
    ] {
        assert!(
            validate_collection_sizes(
                CognitionEffect::Mutated,
                counts.0,
                counts.1,
                counts.2,
                counts.3,
            )
            .is_err()
        );
    }
}

#[test]
fn effect_requires_its_exact_mutation_shape() {
    validate_collection_sizes(CognitionEffect::NoChange, 1, 0, 0, 0)
        .expect("no-change has no mutation evidence");

    for counts in [(0, 1, 1), (1, 0, 1), (1, 1, 0)] {
        assert!(
            validate_collection_sizes(CognitionEffect::Mutated, 1, counts.0, counts.1, counts.2)
                .is_err()
        );
    }
    for counts in [(1, 0, 0), (0, 1, 0), (0, 0, 1)] {
        assert!(
            validate_collection_sizes(CognitionEffect::NoChange, 1, counts.0, counts.1, counts.2)
                .is_err()
        );
    }
}

#[test]
fn aggregate_output_entity_limit_is_inclusive() {
    let exact = entities(MAX_COGNITION_MUTATIONS);
    validate_output_entities(&[put("output", exact.clone())])
        .expect("aggregate entity limit is inclusive");

    let mut over = exact;
    over.push(EntityRef::new("one-too-many", "person"));
    assert!(validate_output_entities(&[put("output", over)]).is_err());
}

#[test]
fn entity_names_and_kinds_must_be_bounded_canonical_text() {
    for entity in [
        EntityRef::new("Alice\nprotected", "person"),
        EntityRef::new("Alice", "person\nprotected"),
        EntityRef::new(
            "x".repeat(crate::cognition::MAX_COGNITION_IDENTITY_BYTES + 1),
            "person",
        ),
        EntityRef::new(
            "Alice",
            "x".repeat(crate::cognition::MAX_COGNITION_IDENTITY_BYTES + 1),
        ),
    ] {
        assert!(validate_output_entities(&[put("output", vec![entity])]).is_err());
    }
}

#[test]
fn duplicate_and_conflicting_entity_identities_fail_closed() {
    assert!(
        validate_output_entities(&[put(
            "output",
            vec![
                EntityRef::new("Alice", "person"),
                EntityRef::new("Alice", "person"),
            ],
        )])
        .is_err()
    );
    assert!(
        validate_output_entities(&[
            put("left", vec![EntityRef::new("Alice", "person")]),
            put("right", vec![EntityRef::new("Alice", "organization")]),
        ])
        .is_err()
    );
    validate_output_entities(&[
        put("left", vec![EntityRef::new("Alice", "person")]),
        put("right", vec![EntityRef::new("Alice", "person")]),
    ])
    .expect("consistent shared entities are allowed across outputs");
}

fn entities(count: usize) -> Vec<EntityRef> {
    (0..count)
        .map(|index| EntityRef::new(format!("entity-{index}"), "person"))
        .collect()
}

fn put(id: &str, entities: Vec<EntityRef>) -> StoreBatchOp {
    let mut record = record(id);
    record.entities = entities;
    StoreBatchOp::Put(Box::new(record))
}

fn record(id: &str) -> StoredRecord {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "space_id": "memory/user:alice/semantic",
        "kind": "semantic",
        "label": "internal",
        "quarantined": false,
        "entities": [],
        "provenance": { "source": "operator" },
        "observed_at": "2026-08-05T12:00:00Z",
        "valid_from": "2026-08-05T12:00:00Z",
        "invalid_at": null,
        "expires_at": null,
        "purposes": ["research"],
        "content": { "text": "derived" }
    }))
    .expect("stored record fixture")
}
