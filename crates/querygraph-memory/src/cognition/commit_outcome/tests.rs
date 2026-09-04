use chrono::{DateTime, TimeDelta, Utc};
use grust_core::prelude::{GraphCommitReceipt, Value};
use typesec_memory::{
    CognitionAuditEvidence, CognitionCommitStatus, CognitionEffect, CognitionIdempotencyKey,
    GovernedSourceScope, MAX_COGNITION_IDENTITY_BYTES, MAX_COGNITION_MUTATIONS,
    MAX_COGNITION_SOURCE_BYTES, MemoryId,
};

use super::{
    DurableOutcome, MAX_OUTBOX_MANIFEST_BYTES, decode_audit, map_outcome, validate_affected_ids,
    validate_outbox_manifest, validate_recovered_audit,
};
use crate::cognition::graph::encode_audit;

#[test]
fn canonical_backend_commit_time_is_preserved_exactly() {
    let prepared_at = timestamp("2026-08-05T12:00:00.123456789Z");
    let committed_at = timestamp("2026-08-05T12:00:00.124Z");
    let outcome = map_outcome(
        &receipt("2026-08-05T14:00:00.124+02:00"),
        &durable(),
        audit(prepared_at),
        CognitionCommitStatus::Applied,
    )
    .expect("canonical backend time");

    assert_eq!(outcome.committed_at, committed_at);
    assert_ne!(outcome.committed_at, outcome.audit.prepared_at);
}

#[test]
fn regressive_backend_commit_time_fails_closed() {
    for committed_at in ["2026-08-05T12:00:00.123Z", "2026-08-05T12:00:00.122456789Z"] {
        assert_receipt_rejected(
            committed_at,
            "store backend error: cognition backend commit timestamp predates preparation",
        );
    }
}

#[test]
fn malformed_backend_commit_time_fails_closed_without_leaking_input() {
    for committed_at in [
        "not-a-backend-timestamp",
        "2026-08-05 12:00:00.124Z",
        "2026-08-05T12:00:00.124z",
    ] {
        assert_receipt_rejected(
            committed_at,
            "store backend error: cognition backend commit timestamp is not canonical RFC 3339",
        );
    }
}

#[test]
fn local_and_governed_audit_scopes_round_trip_losslessly() {
    let key = cognition_key();
    for expected_scope in [None, Some(governed_scope('9'))] {
        let mut expected = audit(timestamp("2026-08-05T12:00:00Z"));
        expected.governed_source_scope = expected_scope.clone();
        let node = encode_audit(&key, &expected).expect("encode durable audit");
        let Some(Value::Json(payload)) = node.props.get("payload") else {
            panic!("durable audit has a JSON payload");
        };
        assert_eq!(
            payload
                .get("governedSourceScope")
                .and_then(serde_json::Value::as_str),
            expected_scope.as_ref().map(GovernedSourceScope::as_str)
        );

        let recovered = decode_audit(&node).expect("decode durable audit");
        assert_eq!(recovered, expected);
        validate_recovered_audit(&key, &durable(), &recovered)
            .expect("validate recovered audit scope");
    }
}

#[test]
fn malformed_persisted_governed_scope_fails_closed() {
    let key = cognition_key();
    let mut node = encode_audit(&key, &audit(timestamp("2026-08-05T12:00:00Z")))
        .expect("encode durable audit");
    let Some(Value::Json(payload)) = node.props.get_mut("payload") else {
        panic!("durable audit has a JSON payload");
    };
    payload
        .as_object_mut()
        .expect("audit payload is an object")
        .insert(
            "governedSourceScope".into(),
            serde_json::json!("sha256:not-canonical"),
        );

    assert!(decode_audit(&node).is_err());
}

#[test]
fn recovered_audit_rejects_schema_snapshot_and_phase_time_drift() {
    let key = cognition_key();
    for mutate in [
        (|audit: &mut CognitionAuditEvidence| audit.schema_version += 1)
            as fn(&mut CognitionAuditEvidence),
        |audit| audit.snapshot_digest = audit.governed_scan_digest.clone(),
        |audit| audit.snapshot_digest = "sha256:not-canonical".into(),
        |audit| audit.authority_revalidated_at = audit.prepared_at + TimeDelta::nanoseconds(1),
    ] {
        let mut evidence = audit(timestamp("2026-08-05T12:00:00Z"));
        mutate(&mut evidence);
        assert!(validate_recovered_audit(&key, &durable(), &evidence).is_err());
    }
}

#[test]
fn durable_vector_counts_are_rejected_before_uniqueness_work() {
    let outbox_id = format!("cog-outbox:{}", "8".repeat(64));
    let outbox = vec![outbox_id; super::super::outbox::MAX_COGNITION_OUTBOX_ENTRIES + 1];
    assert!(validate_outbox_manifest(CognitionEffect::Mutated, &outbox).is_err());

    let affected = vec![MemoryId::from_string("source"); MAX_COGNITION_MUTATIONS + 1];
    assert!(validate_affected_ids(CognitionEffect::Mutated, &affected).is_err());
}

#[test]
fn durable_vector_bytes_are_rejected_before_uniqueness_work() {
    let outbox = vec!["x".repeat(MAX_OUTBOX_MANIFEST_BYTES + 1)];
    assert!(validate_outbox_manifest(CognitionEffect::Mutated, &outbox).is_err());

    let id_count = MAX_COGNITION_SOURCE_BYTES / MAX_COGNITION_IDENTITY_BYTES + 1;
    let suffix = "x".repeat(MAX_COGNITION_IDENTITY_BYTES - 4);
    let affected = (0..id_count)
        .map(|index| MemoryId::from_string(format!("{index:04}{suffix}")))
        .collect::<Vec<_>>();
    assert!(affected.len() <= MAX_COGNITION_MUTATIONS);
    assert!(validate_affected_ids(CognitionEffect::Mutated, &affected).is_err());
}

#[test]
fn durable_vectors_match_the_declared_effect_exactly() {
    let outbox = vec![format!("cog-outbox:{}", "8".repeat(64))];
    let affected = vec![MemoryId::from_string("source")];

    validate_outbox_manifest(CognitionEffect::NoChange, &[]).expect("no-change outbox is empty");
    validate_affected_ids(CognitionEffect::NoChange, &[])
        .expect("no-change affected IDs are empty");
    assert!(validate_outbox_manifest(CognitionEffect::Mutated, &[]).is_err());
    assert!(validate_affected_ids(CognitionEffect::Mutated, &[]).is_err());
    assert!(validate_outbox_manifest(CognitionEffect::NoChange, &outbox).is_err());
    assert!(validate_affected_ids(CognitionEffect::NoChange, &affected).is_err());
}

#[test]
fn affected_ids_must_retain_typesec_canonical_order() {
    let first = MemoryId::from_string("first");
    let second = MemoryId::from_string("second");
    validate_affected_ids(CognitionEffect::Mutated, &[first.clone(), second.clone()])
        .expect("strictly ordered affected IDs");
    assert!(validate_affected_ids(CognitionEffect::Mutated, &[second, first.clone()]).is_err());
    assert!(validate_affected_ids(CognitionEffect::Mutated, &[first.clone(), first]).is_err());
}

fn assert_receipt_rejected(committed_at: &str, expected: &str) {
    let prepared_at = timestamp("2026-08-05T12:00:00.123456789Z");
    let error = map_outcome(
        &receipt(committed_at),
        &durable(),
        audit(prepared_at),
        CognitionCommitStatus::Applied,
    )
    .expect_err("untrustworthy backend time must fail closed");

    assert_eq!(error.to_string(), expected);
    assert!(!error.to_string().contains(committed_at));
}

fn timestamp(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("valid test timestamp")
        .with_timezone(&Utc)
}

fn receipt(committed_at: &str) -> GraphCommitReceipt {
    GraphCommitReceipt {
        commit_id: "commit:receipt-clock".into(),
        committed_at: committed_at.into(),
        replayed: false,
    }
}

fn durable() -> DurableOutcome {
    DurableOutcome {
        schema_version: super::OUTCOME_SCHEMA_VERSION,
        effect: CognitionEffect::Mutated,
        proposal_digest: digest('a'),
        prepared_digest: digest('5'),
        prior_version: digest('b'),
        resulting_version: digest('c'),
        audit_node_id: "cog-audit:test".into(),
        audit_digest: digest('d'),
        completed_job_digest: digest('6'),
        outbox_node_ids: vec![format!("cog-outbox:{}", "8".repeat(64))],
        envelope_digest: digest('7'),
    }
}

fn audit(prepared_at: DateTime<Utc>) -> CognitionAuditEvidence {
    CognitionAuditEvidence {
        schema_version: CognitionAuditEvidence::SCHEMA_VERSION,
        effect: CognitionEffect::Mutated,
        operation_id: "job".into(),
        subject: "did:key:alice".into(),
        space_id: "memory/user:alice/semantic".into(),
        purpose: "research".into(),
        governed_source_scope: None,
        proposal_digest: digest('a'),
        binding_digest: digest('e'),
        source_manifest_digest: digest('f'),
        typedid_request_digest: digest('1'),
        governed_scan_digest: digest('2'),
        snapshot_digest: digest('9'),
        authorization_receipt_digest: digest('3'),
        policy_decision_id: "policy:decision".into(),
        evidence_digest: digest('4'),
        affected_ids: vec![MemoryId::from_string("source")],
        authority_revalidated_at: prepared_at - TimeDelta::seconds(1),
        prepared_at,
    }
}

fn cognition_key() -> CognitionIdempotencyKey {
    CognitionIdempotencyKey::for_authority(
        "memory/user:alice/semantic",
        "did:key:alice",
        "research",
        "job",
    )
    .expect("canonical cognition key")
}

fn governed_scope(byte: char) -> GovernedSourceScope {
    GovernedSourceScope::from_digest(digest(byte)).expect("canonical governed source scope")
}

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}
