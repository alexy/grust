#![cfg(feature = "turso")]

mod support;

use grust_core::prelude::{GraphStore, Node, Start, Traversal, Value};
use querygraph_memory::TursoMemoryStore;
use querygraph_memory::cognition::CognitionJob;
use serde::Serialize;
use sha2::{Digest, Sha256};
use typesec_memory::{
    CognitionAuditEvidence, CognitionCommitError, CognitionEffect, MemoryError, StoreError,
};

use support::cognition_vault::CognitionFixture;
use support::{config, digest, record};

#[tokio::test]
async fn unsupported_outcome_schema_fails_closed() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_outcome_schema"))
        .expect("open store");
    let fixture = CognitionFixture::new(store, record("source", "source text", None), "job");
    fixture.apply().expect("apply cognition");
    let mut outcome = only_node(fixture.store(), "CognitionOutcome").await;
    payload_mut(&mut outcome)["schemaVersion"] = serde_json::json!(4);
    fixture
        .store()
        .graph()
        .put_node(&outcome)
        .await
        .expect("tamper outcome schema");

    assert!(matches!(
        fixture.apply(),
        Err(MemoryError::CognitionCommit(CognitionCommitError::Store(_)))
    ));
}

#[tokio::test]
async fn checked_in_legacy_outcome_and_audit_schemas_fail_precisely() {
    assert_legacy_payload_rejected(
        "legacy_outcome_v2",
        "CognitionOutcome",
        serde_json::from_str(include_str!("fixtures/cognition_outcome_v2_legacy.json"))
            .expect("legacy outcome fixture"),
        "unsupported persisted cognition outcome schema version",
    )
    .await;
    for (prefix, fixture) in [
        (
            "legacy_audit_v1",
            include_str!("fixtures/cognition_audit_v1_legacy.json"),
        ),
        (
            "legacy_audit_v2",
            include_str!("fixtures/cognition_audit_v2_legacy.json"),
        ),
    ] {
        assert_legacy_payload_rejected(
            prefix,
            "CognitionAudit",
            serde_json::from_str(fixture).expect("legacy audit fixture"),
            "unsupported persisted cognition audit schema version",
        )
        .await;
    }
    let mut unversioned_audit: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/cognition_audit_v2_legacy.json"))
            .expect("legacy audit fixture");
    unversioned_audit
        .as_object_mut()
        .expect("legacy audit object")
        .remove("schemaVersion");
    assert_legacy_payload_rejected(
        "legacy_audit_unversioned",
        "CognitionAudit",
        unversioned_audit,
        "unsupported persisted cognition audit schema version",
    )
    .await;
}

#[tokio::test]
async fn coordinated_durable_evidence_tampering_cannot_bypass_the_ledger() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_envelope_ledger"))
        .expect("open store");
    let fixture = CognitionFixture::new(store, record("source", "source text", None), "job");
    fixture.apply().expect("apply cognition");

    let mut audit_node = only_node(fixture.store(), "CognitionAudit").await;
    payload_mut(&mut audit_node)["policyDecisionId"] =
        serde_json::json!("policy-decision:coordinated-tamper");
    let audit: CognitionAuditEvidence =
        serde_json::from_value(payload(&audit_node).clone()).expect("decode tampered audit");
    let audit_digest = tagged_json("querygraph.cognition.audit.v3", &audit);

    let mut job_node = only_node(fixture.store(), "CognitionJob").await;
    payload_mut(&mut job_node)["lastErrorDigest"] = serde_json::json!(digest("tampered job"));
    let job: CognitionJob =
        serde_json::from_value(payload(&job_node).clone()).expect("decode tampered job");
    let completed_job_digest = tagged_json("querygraph.cognition.completed-job.v1", &job);

    let mut outcome_node = only_node(fixture.store(), "CognitionOutcome").await;
    let outcome = payload_mut(&mut outcome_node);
    outcome["auditDigest"] = serde_json::json!(audit_digest);
    outcome["completedJobDigest"] = serde_json::json!(completed_job_digest);
    let outbox_node_ids: Vec<String> =
        serde_json::from_value(outcome["outboxNodeIds"].clone()).expect("outbox manifest");
    let effect: CognitionEffect =
        serde_json::from_value(outcome["effect"].clone()).expect("outcome effect");
    let envelope_digest = {
        let envelope = TestEnvelope {
            effect,
            proposal_digest: text(outcome, "proposalDigest"),
            prepared_digest: text(outcome, "preparedDigest"),
            prior_version: text(outcome, "priorVersion"),
            resulting_version: text(outcome, "resultingVersion"),
            audit_node_id: text(outcome, "auditNodeId"),
            audit_digest: text(outcome, "auditDigest"),
            completed_job_digest: text(outcome, "completedJobDigest"),
            outbox_node_ids: &outbox_node_ids,
        };
        tagged_json("querygraph.cognition.commit-envelope.v3", &envelope)
    };
    outcome["envelopeDigest"] = serde_json::json!(envelope_digest);

    for node in [&audit_node, &job_node, &outcome_node] {
        fixture
            .store()
            .graph()
            .put_node(node)
            .await
            .expect("persist coordinated tamper");
    }
    assert!(matches!(
        fixture.apply(),
        Err(MemoryError::CognitionCommit(
            CognitionCommitError::IdempotencyConflict
        ))
    ));
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TestEnvelope<'a> {
    effect: CognitionEffect,
    proposal_digest: &'a str,
    prepared_digest: &'a str,
    prior_version: &'a str,
    resulting_version: &'a str,
    audit_node_id: &'a str,
    audit_digest: &'a str,
    completed_job_digest: &'a str,
    outbox_node_ids: &'a [String],
}

async fn only_node(store: &TursoMemoryStore, label: &str) -> Node {
    let mut nodes = store
        .graph()
        .traverse(Traversal {
            start: Start::NodesByLabel(label.into()),
            steps: Vec::new(),
            limit: None,
        })
        .await
        .expect("read durable cognition node");
    assert_eq!(nodes.len(), 1, "expected one {label} node");
    nodes.pop().expect("durable cognition node")
}

fn payload(node: &Node) -> &serde_json::Value {
    let Some(Value::Json(payload)) = node.props.get("payload") else {
        panic!("cognition payload is JSON")
    };
    payload
}

fn payload_mut(node: &mut Node) -> &mut serde_json::Value {
    let Some(Value::Json(payload)) = node.props.get_mut("payload") else {
        panic!("cognition payload is JSON")
    };
    payload
}

fn text<'a>(value: &'a serde_json::Value, field: &str) -> &'a str {
    value[field].as_str().expect("string envelope field")
}

fn tagged_json(domain: &str, value: &impl Serialize) -> String {
    let bytes = serde_json::to_vec(value).expect("canonical fixture JSON");
    let mut digest = Sha256::new();
    digest.update(domain.as_bytes());
    digest.update([0]);
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
    format!("sha256:{:x}", digest.finalize())
}

async fn assert_legacy_payload_rejected(
    prefix: &str,
    label: &str,
    legacy_payload: serde_json::Value,
    expected: &str,
) {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, prefix)).expect("open store");
    let fixture = CognitionFixture::new(store, record("source", "source text", None), "job");
    fixture.apply().expect("seed current cognition commit");
    let mut node = only_node(fixture.store(), label).await;
    *payload_mut(&mut node) = legacy_payload;
    fixture
        .store()
        .graph()
        .put_node(&node)
        .await
        .expect("install checked-in legacy payload");

    let error = fixture
        .apply()
        .expect_err("legacy durable schema must fail closed");
    assert!(matches!(
        error,
        MemoryError::CognitionCommit(CognitionCommitError::Store(StoreError::Backend(message)))
            if message == expected
    ));
}
