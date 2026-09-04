#![allow(dead_code)]

pub mod cognition_input;
#[cfg(feature = "turso")]
pub mod cognition_vault;
#[cfg(feature = "turso")]
pub mod guarded_fault;

use chrono::{DateTime, TimeZone, Utc};
#[cfg(feature = "turso")]
use grust_core::prelude::GraphCommitStore;
#[cfg(feature = "turso")]
use querygraph_memory::GraphStoreMemoryStore;
#[cfg(feature = "turso")]
use querygraph_memory::turso::TursoConfig;
use sha2::{Digest, Sha256};
#[cfg(feature = "turso")]
use tempfile::TempDir;
#[cfg(feature = "turso")]
use typesec_memory::MemoryStore;
use typesec_memory::{
    CognitionBinding, CognitionIdempotencyKey, CognitionProposal, CognitionSourcePrecondition,
    ConsolidationPlan, ConsolidationStep, GovernedSourceScope, Label, StoredRecord,
};

#[cfg(feature = "turso")]
pub fn config(dir: &TempDir, prefix: &str) -> TursoConfig {
    TursoConfig {
        path: dir.path().join("memory.db").to_string_lossy().into_owned(),
        table_prefix: prefix.to_owned(),
        batch_size: 32,
        ..TursoConfig::default()
    }
}

pub fn at(second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 5, 12, 0, second)
        .single()
        .expect("valid fixture timestamp")
}

pub fn digest(value: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
}

pub fn job_key(job_id: &str) -> CognitionIdempotencyKey {
    job_key_for_authority(
        "memory/user:alice/semantic",
        "did:key:alice",
        "research",
        job_id,
    )
}

pub fn job_key_for_authority(
    space_id: &str,
    subject: &str,
    purpose: &str,
    job_id: &str,
) -> CognitionIdempotencyKey {
    CognitionIdempotencyKey::for_authority(space_id, subject, purpose, job_id)
        .expect("canonical cognition idempotency fixture")
}

pub fn record(id: &str, text: &str, invalid_at: Option<DateTime<Utc>>) -> StoredRecord {
    record_with_scope(id, text, invalid_at, None)
}

pub fn governed_record(
    id: &str,
    text: &str,
    invalid_at: Option<DateTime<Utc>>,
    scope: &GovernedSourceScope,
) -> StoredRecord {
    record_with_scope(id, text, invalid_at, Some(scope))
}

fn record_with_scope(
    id: &str,
    text: &str,
    invalid_at: Option<DateTime<Utc>>,
    scope: Option<&GovernedSourceScope>,
) -> StoredRecord {
    let mut encoded = serde_json::json!({
        "id": id,
        "space_id": "memory/user:alice/semantic",
        "kind": "semantic",
        "label": "internal",
        "quarantined": false,
        "entities": [],
        "provenance": { "source": "operator" },
        "observed_at": at(0),
        "valid_from": at(0),
        "invalid_at": invalid_at,
        "expires_at": null,
        "purposes": ["research"],
        "content": { "text": text }
    });
    if let Some(scope) = scope {
        encoded
            .as_object_mut()
            .expect("record fixture is an object")
            .insert(
                "governed_source_scope".into(),
                serde_json::to_value(scope).expect("serialize governed source scope"),
            );
    }
    serde_json::from_value(encoded).expect("stored record fixture")
}

pub fn proposal(job_id: &str, source: &StoredRecord) -> CognitionProposal {
    let source_precondition = CognitionSourcePrecondition::for_record(source).expect("source hash");
    let binding = CognitionBinding {
        space_id: source.space_id.clone(),
        subject: "did:key:alice".into(),
        purpose: "research".into(),
        governed_source_scope: source.governed_source_scope().cloned(),
        governed_scan_digest: digest("scan"),
        snapshot_digest: digest("snapshot"),
        plan_task_digest: digest("plan"),
        authorization_receipt_digest: digest("authorization"),
        effective_projection: vec!["finding".into()],
        source_manifest_digest: digest("manifest"),
        typedid_request_digest: digest("request"),
    };
    CognitionProposal::new(
        job_id,
        binding.snapshot_digest.clone(),
        binding.source_manifest_digest.clone(),
        "marciana.test",
        "1",
        vec![source.id.clone()],
        Label::Internal,
    )
    .with_plan(
        ConsolidationPlan::new().then(ConsolidationStep::Invalidate {
            ids: vec![source_precondition.id],
        }),
    )
    .with_binding(binding)
}

#[cfg(feature = "turso")]
pub fn stage_at<G: GraphCommitStore>(
    store: &GraphStoreMemoryStore<G>,
    source: StoredRecord,
    proposal: &CognitionProposal,
    now: DateTime<Utc>,
) {
    let key = job_key(&proposal.job_id);
    store.put(source).expect("persist source");
    let typedid_request_digest = proposal
        .binding
        .as_ref()
        .expect("staged proposal has a binding")
        .typedid_request_digest
        .clone();
    store
        .submit_cognition_job(&key, "scheduler", &typedid_request_digest, 3, now)
        .expect("submit job");
    let lease = store
        .acquire_cognition_lease(&key, "worker", now, chrono::Duration::minutes(5))
        .expect("acquire lease");
    store
        .persist_cognition_proposal(&key, lease.token(), proposal, now)
        .expect("stage proposal");
}
