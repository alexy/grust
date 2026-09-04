use super::*;
use crate::cognition::bounds::{
    MAX_COGNITION_IDENTITY_BYTES, MAX_COGNITION_PROJECTION_BYTES, MAX_COGNITION_PROJECTION_FIELDS,
};

fn snapshot() -> GovernedLakeCatSnapshot {
    GovernedLakeCatSnapshot {
        catalog: "lakecat://prod".into(),
        namespace: "research".into(),
        table: "findings".into(),
        snapshot_id: 42,
        governed_scan_digest: digest("scan"),
        snapshot_digest: digest("snapshot"),
        plan_task_digest: digest("plan"),
        subject: "did:key:researcher".into(),
        purpose: "research".into(),
        effective_projection: vec!["id".into(), "finding".into(), "valid_from".into()],
        authorization_receipt_digest: digest("receipt"),
    }
}

fn digest(value: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
}

#[test]
fn snapshot_identity_byte_limit_is_inclusive_and_canonical() {
    let mut source = snapshot();
    source.catalog = "x".repeat(MAX_COGNITION_IDENTITY_BYTES);
    source.digest().expect("inclusive identity byte limit");

    source.catalog.push('x');
    assert!(matches!(
        source.digest(),
        Err(CognitionError::InvalidSnapshot("catalog"))
    ));
    source.catalog = "lakecat://prod\nsecret".into();
    assert!(matches!(
        source.digest(),
        Err(CognitionError::InvalidSnapshot("catalog"))
    ));
    source.catalog = " lakecat://prod".into();
    assert!(matches!(
        source.digest(),
        Err(CognitionError::InvalidSnapshot("catalog"))
    ));
}

#[test]
fn projection_count_limit_is_inclusive_and_duplicates_fail_closed() {
    let mut source = snapshot();
    source.effective_projection = (0..MAX_COGNITION_PROJECTION_FIELDS)
        .map(|index| format!("field-{index}"))
        .collect();
    source.digest().expect("inclusive projection count limit");

    source.effective_projection.push("one-too-many".into());
    assert!(matches!(
        source.digest(),
        Err(CognitionError::InvalidSnapshot("effective projection"))
    ));
    source.effective_projection = vec!["duplicate".into(), "duplicate".into()];
    assert!(matches!(
        source.digest(),
        Err(CognitionError::InvalidSnapshot("effective projection"))
    ));
}

#[test]
fn projection_aggregate_byte_limit_is_inclusive() {
    assert_eq!(
        MAX_COGNITION_PROJECTION_BYTES % MAX_COGNITION_IDENTITY_BYTES,
        0,
        "fixture assumes an exact number of maximum-size fields"
    );
    let field_count = MAX_COGNITION_PROJECTION_BYTES / MAX_COGNITION_IDENTITY_BYTES;
    let mut source = snapshot();
    source.effective_projection = (0..field_count)
        .map(|index| {
            let prefix = format!("{index:04}");
            format!(
                "{prefix}{}",
                "x".repeat(MAX_COGNITION_IDENTITY_BYTES - prefix.len())
            )
        })
        .collect();
    source.digest().expect("inclusive projection byte limit");

    source.effective_projection.push("x".into());
    assert!(matches!(
        source.digest(),
        Err(CognitionError::InvalidSnapshot("effective projection"))
    ));
}

#[test]
fn projection_and_field_mapping_reject_control_text_and_overlong_fields() {
    let mut source = snapshot();
    source.effective_projection = vec!["id".into(), "finding\nsecret".into()];
    assert!(matches!(
        source.digest(),
        Err(CognitionError::InvalidSnapshot("effective projection"))
    ));

    let exact = "x".repeat(MAX_COGNITION_IDENTITY_BYTES);
    let projection = vec![exact.clone(), "text".into(), "valid_from".into()];
    let mut mapping = CognitionFieldMapping {
        id: exact,
        text: "text".into(),
        valid_from: "valid_from".into(),
    };
    mapping
        .validate(&projection)
        .expect("inclusive field mapping byte limit");
    mapping.id.push('x');
    assert!(matches!(
        mapping.validate(&projection),
        Err(CognitionError::InvalidSnapshot("cognition field mapping"))
    ));
    mapping.id = "id\nsecret".into();
    assert!(matches!(
        mapping.validate(&projection),
        Err(CognitionError::InvalidSnapshot("cognition field mapping"))
    ));
}

#[test]
fn field_mapping_duplicates_and_denials_have_fixed_errors() {
    let projection = vec!["id".into(), "text".into(), "valid_from".into()];
    let duplicate = CognitionFieldMapping {
        id: "id".into(),
        text: "id".into(),
        valid_from: "valid_from".into(),
    };
    assert!(matches!(
        duplicate.validate(&projection),
        Err(CognitionError::InvalidSnapshot(
            "duplicate cognition field mapping"
        ))
    ));

    let denied = CognitionFieldMapping {
        id: "id".into(),
        text: "private-source-field".into(),
        valid_from: "valid_from".into(),
    }
    .validate(&projection)
    .expect_err("unapproved field must fail");
    assert!(matches!(&denied, CognitionError::ProjectionDenied));
    assert_eq!(
        denied.to_string(),
        "cognition field is not in the authorized projection"
    );
    assert!(!denied.to_string().contains("private-source-field"));
}
