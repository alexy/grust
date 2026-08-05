#![cfg(feature = "sail")]

use std::env;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use grust_sail::{SailConfig, SailGraphStore};
use querygraph_memory::cognition::{
    CognitionEngine, CognitionOperation, CognitionRequest, GovernedLakeCatSnapshot,
    LiveSailCognitionExecutor, ReferenceCognitionEngine, SailCognitionEngine,
};
use typesec_memory::{
    CognitionProposal, ConsolidationPlan, ConsolidationStep, Label, MemoryContent, MemoryId,
    MemoryKind, Provenance, RecalledMemory,
};

fn governed_source() -> GovernedLakeCatSnapshot {
    GovernedLakeCatSnapshot {
        catalog: "lakecat://private-production".into(),
        namespace: "tenant_secret".into(),
        table: "private_findings".into(),
        snapshot_id: 9_001,
        plan_task_digest: "sha256:opaque-plan-secret".into(),
        subject: "did:key:private-researcher".into(),
        purpose: "purpose/private-research".into(),
        effective_projection: vec!["finding".into()],
        authorization_receipt_digest: "sha256:opaque-authorization-secret".into(),
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
    assert_eq!(left.job_id, right.job_id);
    assert_eq!(left.input_snapshot, right.input_snapshot);
    assert_eq!(left.source_digest, right.source_digest);
    assert_eq!(left.algorithm, right.algorithm);
    assert_eq!(left.algorithm_version, right.algorithm_version);
    assert_eq!(left.source_ids, right.source_ids);
    assert_eq!(left.joined_label, right.joined_label);
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

#[tokio::test]
#[ignore = "requires a live Sail Spark Connect endpoint (run scripts/integration-test.sh --backend sail)"]
async fn live_sail_cognition_matches_reference_and_keeps_evidence_secret() {
    let mut config = SailConfig::default();
    config.endpoint = sail_endpoint();
    config.user_id = "marciana-live-test".into();
    let store = Arc::new(
        SailGraphStore::connect(config)
            .await
            .expect("connect to the live Sail endpoint"),
    );
    let live = SailCognitionEngine::new(LiveSailCognitionExecutor::new(store));
    let reference = ReferenceCognitionEngine;
    let source = governed_source();
    let memories = memories();

    for (operation, job_id) in [
        (CognitionOperation::Deduplicate, "tenant-secret/deduplicate"),
        (CognitionOperation::Reconcile, "tenant-secret/reconcile"),
    ] {
        let expected = reference
            .propose(CognitionRequest {
                job_id,
                source: &source,
                memories: &memories,
                operation,
            })
            .await
            .expect("reference cognition proposal");
        let actual = live
            .propose(CognitionRequest {
                job_id,
                source: &source,
                memories: &memories,
                operation,
            })
            .await
            .expect("live Sail cognition proposal");
        let repeated = live
            .propose(CognitionRequest {
                job_id,
                source: &source,
                memories: &memories,
                operation,
            })
            .await
            .expect("idempotent live Sail cognition proposal");

        assert_eq!(
            plan_fingerprint(&actual.plan),
            plan_fingerprint(&expected.plan),
            "live Sail plan must match the reference oracle"
        );
        assert_eq!(actual.joined_label, Label::Secret);
        assert_stable_proposal(&actual, &repeated);
        assert_evidence_is_secret_safe(&actual, &source, &memories);
    }
}
