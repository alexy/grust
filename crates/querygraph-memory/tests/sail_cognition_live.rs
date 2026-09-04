#![cfg(feature = "sail")]

mod support;

use std::env;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use grust_sail::{SailConfig, SailGraphStore};
use querygraph_memory::cognition::{
    CognitionEngine, CognitionFieldMapping, CognitionOperation, CognitionRequest,
    GovernedLakeCatSnapshot, LiveSailCognitionExecutor, ReferenceCognitionEngine,
    SailCognitionEngine,
};
use typesec_memory::{
    CognitionBinding, CognitionProposal, ConsolidationPlan, ConsolidationStep, GovernedSourceScope,
    Label, MemoryContent, MemoryId, MemoryKind, Provenance, RecalledMemory, StoredRecord,
};

use support::cognition_input::governed_authorized_input_for;

const LIVE_MEMORY_SPACE_ID: &str = "memory/user:alice/semantic";

fn digest(value: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
}

fn governed_source() -> GovernedLakeCatSnapshot {
    GovernedLakeCatSnapshot {
        catalog: "lakecat://private-production".into(),
        namespace: "tenant_secret".into(),
        table: "private_findings".into(),
        snapshot_id: 9_001,
        governed_scan_digest: digest("opaque-scan-secret"),
        snapshot_digest: digest("opaque-snapshot-secret"),
        plan_task_digest: digest("opaque-plan-secret"),
        subject: "did:key:private-researcher".into(),
        purpose: "purpose/private-research".into(),
        effective_projection: vec!["id".into(), "finding".into(), "valid_from".into()],
        authorization_receipt_digest: digest("opaque-authorization-secret"),
    }
}

fn memory(id: &str, text: &str, year: i32, label: Label) -> RecalledMemory {
    RecalledMemory {
        id: MemoryId::from_string(id),
        kind: MemoryKind::Semantic,
        label,
        content: MemoryContent::text(text),
        entities: vec![],
        provenance: Provenance::Operator,
        valid_from: Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).unwrap(),
    }
}

fn memories() -> Vec<RecalledMemory> {
    vec![
        memory(
            "duplicate-old",
            "Private tenant prefers espresso",
            2023,
            Label::Internal,
        ),
        memory(
            "duplicate-new",
            "private   tenant prefers ESPRESSO",
            2024,
            Label::Sensitive,
        ),
        memory(
            "assertion-old",
            "Private tenant lives in Rome",
            2023,
            Label::Public,
        ),
        memory(
            "assertion-new",
            "Private tenant lives in Venice",
            2025,
            Label::Secret,
        ),
    ]
}

fn stored(memory: &RecalledMemory, governed_source_scope: &GovernedSourceScope) -> StoredRecord {
    serde_json::from_value(serde_json::json!({
        "id": memory.id,
        "space_id": LIVE_MEMORY_SPACE_ID,
        "kind": memory.kind,
        "label": memory.label,
        "quarantined": false,
        "entities": memory.entities,
        "provenance": memory.provenance,
        "governed_source_scope": governed_source_scope,
        "observed_at": memory.valid_from,
        "valid_from": memory.valid_from,
        "invalid_at": null,
        "expires_at": null,
        "purposes": ["purpose/private-research"],
        "content": memory.content,
    }))
    .expect("stored live-test source")
}

fn sail_endpoint() -> String {
    env::var("SAIL_ENDPOINT").unwrap_or_else(|_| {
        let host = env::var("SAIL_HOST").unwrap_or_else(|_| "127.0.0.1".into());
        let port = env::var("SAIL_PORT").unwrap_or_else(|_| "50051".into());
        format!("http://{host}:{port}")
    })
}

fn plan_fingerprint(plan: &ConsolidationPlan) -> Vec<(String, Vec<String>)> {
    plan.steps
        .iter()
        .map(|step| match step {
            ConsolidationStep::Supersede { superseded, .. } => (
                "supersede".into(),
                superseded.iter().map(|id| id.as_str().to_owned()).collect(),
            ),
            ConsolidationStep::Invalidate { ids } => (
                "invalidate".into(),
                ids.iter().map(|id| id.as_str().to_owned()).collect(),
            ),
        })
        .collect()
}

fn assert_stable_proposal(left: &CognitionProposal, right: &CognitionProposal) {
    assert_eq!(left.schema_version, right.schema_version);
    assert_eq!(left.effect, right.effect);
    assert_eq!(left.job_id, right.job_id);
    assert_eq!(left.input_snapshot, right.input_snapshot);
    assert_eq!(left.source_digest, right.source_digest);
    assert_eq!(left.algorithm, right.algorithm);
    assert_eq!(left.algorithm_version, right.algorithm_version);
    assert_eq!(left.source_ids, right.source_ids);
    assert_eq!(left.joined_label, right.joined_label);
    assert_eq!(left.binding, right.binding);
    assert_eq!(left.evidence, right.evidence);
    assert_eq!(plan_fingerprint(&left.plan), plan_fingerprint(&right.plan));
}

fn assert_evidence_is_secret_safe(
    proposal: &CognitionProposal,
    source: &GovernedLakeCatSnapshot,
    memories: &[RecalledMemory],
) {
    let evidence = proposal.evidence.join("\n");
    for forbidden in [
        source.catalog.as_str(),
        source.namespace.as_str(),
        source.table.as_str(),
        source.plan_task_digest.as_str(),
        source.subject.as_str(),
        source.purpose.as_str(),
        source.authorization_receipt_digest.as_str(),
        proposal.job_id.as_str(),
    ] {
        assert!(
            !evidence.contains(forbidden),
            "audit evidence exposed governed input {forbidden:?}: {evidence}"
        );
    }
    for memory in memories {
        assert!(
            !evidence.contains(&memory.content.text),
            "audit evidence exposed source plaintext: {evidence}"
        );
    }
}

async fn assert_live_operation_matches_reference(operation: CognitionOperation, job_id: &str) {
    let config = SailConfig {
        endpoint: sail_endpoint(),
        user_id: "marciana-live-test".into(),
        ..SailConfig::default()
    };
    let store = Arc::new(
        SailGraphStore::connect(config)
            .await
            .expect("connect to the live Sail endpoint"),
    );
    let live = SailCognitionEngine::new(LiveSailCognitionExecutor::new(store));
    let reference = ReferenceCognitionEngine;
    let source = governed_source();
    let memories = memories();
    let governed_source_scope = GovernedSourceScope::from_digest(digest("lakecat-source-scope"))
        .expect("canonical governed source scope");
    let input = governed_authorized_input_for(
        memories
            .iter()
            .map(|memory| stored(memory, &governed_source_scope))
            .collect(),
        "purpose/private-research",
        &governed_source_scope,
    );
    let binding = CognitionBinding {
        space_id: LIVE_MEMORY_SPACE_ID.into(),
        subject: source.subject.clone(),
        purpose: source.purpose.clone(),
        governed_source_scope: input.governed_source_scope().cloned(),
        governed_scan_digest: source.governed_scan_digest.clone(),
        snapshot_digest: source.snapshot_digest.clone(),
        plan_task_digest: source.plan_task_digest.clone(),
        authorization_receipt_digest: source.authorization_receipt_digest.clone(),
        effective_projection: source.effective_projection.clone(),
        source_manifest_digest: input.manifest().digest.clone(),
        typedid_request_digest: digest("typedid-request"),
    };
    let field_mapping = CognitionFieldMapping {
        id: "id".into(),
        text: "finding".into(),
        valid_from: "valid_from".into(),
    };

    let expected = reference
        .propose(CognitionRequest {
            job_id,
            source: &source,
            binding: &binding,
            input: &input,
            field_mapping: &field_mapping,
            operation,
        })
        .await
        .expect("reference cognition proposal");
    let actual = live
        .propose(CognitionRequest {
            job_id,
            source: &source,
            binding: &binding,
            input: &input,
            field_mapping: &field_mapping,
            operation,
        })
        .await
        .expect("live Sail cognition proposal");
    let repeated = live
        .propose(CognitionRequest {
            job_id,
            source: &source,
            binding: &binding,
            input: &input,
            field_mapping: &field_mapping,
            operation,
        })
        .await
        .expect("idempotent live Sail cognition proposal");

    assert_eq!(
        plan_fingerprint(&actual.plan),
        plan_fingerprint(&expected.plan),
        "live Sail plan must match the reference oracle"
    );
    assert_eq!(actual.effect, expected.effect);
    assert_eq!(actual.joined_label, Label::Secret);
    for proposal in [&expected, &actual, &repeated] {
        assert_eq!(proposal.schema_version, CognitionProposal::SCHEMA_VERSION);
        assert_eq!(
            proposal
                .binding
                .as_ref()
                .and_then(|binding| binding.governed_source_scope.as_ref()),
            Some(&governed_source_scope)
        );
    }
    assert_stable_proposal(&actual, &repeated);
    assert_evidence_is_secret_safe(&actual, &source, &memories);
}

#[tokio::test]
#[ignore = "requires a live Sail Spark Connect endpoint (run scripts/integration-test.sh --backend sail)"]
async fn live_sail_deduplication_matches_reference_and_keeps_evidence_secret() {
    assert_live_operation_matches_reference(
        CognitionOperation::Deduplicate,
        "tenant-secret/deduplicate",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires a live Sail Spark Connect endpoint (run scripts/integration-test.sh --backend sail)"]
async fn live_sail_reconciliation_matches_reference_and_keeps_evidence_secret() {
    assert_live_operation_matches_reference(
        CognitionOperation::Reconcile,
        "tenant-secret/reconcile",
    )
    .await;
}
