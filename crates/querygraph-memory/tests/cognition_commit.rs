#![cfg(feature = "turso")]

mod support;

use support::job_key;

use std::sync::{Arc, Barrier};

use grust_core::prelude::{GraphStore, Start, Traversal, Value};
use querygraph_memory::TursoMemoryStore;
use querygraph_memory::cognition::CognitionJobStatus;
use typesec_memory::{
    CognitionCommitStatus, CognitionCommitStore, CognitionIdempotencyKey, IndexMutation,
    MemoryContent, MemoryDraft, MemoryError, MemoryId, MemoryStore, Provenance,
};

use support::cognition_vault::CognitionFixture;
use support::{config, digest, record};

#[tokio::test]
async fn commit_atomically_writes_memory_outbox_audit_outcome_and_terminal_job() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store =
        TursoMemoryStore::open_with_config(config(&dir, "cognition_atomic")).expect("open store");
    let source = record("source", "never copy this plaintext", None);
    let fixture = CognitionFixture::customize(store, source.clone(), "job", |proposal| {
        proposal.drafts.push(MemoryDraft::new(
            source.kind,
            MemoryContent::text("sensitive derived proposal text"),
            Provenance::Operator,
        ));
    });

    let outcome = fixture.apply().expect("apply cognition");
    let store = fixture.store();
    assert_eq!(outcome.status, CognitionCommitStatus::Applied);
    assert!(outcome.backend_commit_hash.starts_with("sha256:"));
    assert_eq!(outcome.audit.operation_id, "job");
    assert!(outcome.audit.affected_ids.contains(&source.id));
    assert_eq!(outcome.audit.affected_ids.len(), 2);
    assert!(
        store
            .get(&source.id)
            .expect("read source")
            .expect("source exists")
            .invalid_at
            .is_some()
    );
    assert_eq!(
        store
            .cognition_job(&job_key("job"))
            .expect("load job")
            .expect("job exists")
            .status,
        CognitionJobStatus::Completed
    );

    for (label, expected) in [
        ("CognitionProposal", 0),
        ("CognitionJob", 1),
        ("CognitionIndexOutbox", 2),
        ("CognitionAudit", 1),
        ("CognitionOutcome", 1),
    ] {
        let nodes = store
            .graph()
            .traverse(Traversal {
                start: Start::NodesByLabel(label.into()),
                steps: Vec::new(),
                limit: None,
            })
            .await
            .expect("read cognition nodes");
        assert_eq!(nodes.len(), expected, "unexpected {label} count");
        let encoded = serde_json::to_string(&nodes).expect("serialize cognition nodes");
        assert!(!encoded.contains("never copy this plaintext"));
        assert!(!encoded.contains("sensitive derived proposal text"));
        assert!(!encoded.contains("marciana.test"));
    }
}

#[test]
fn replay_after_reopen_returns_original_receipt_time_and_audit() {
    let dir = tempfile::tempdir().expect("temporary database");
    let config = config(&dir, "cognition_replay");
    let source = record("source", "source text", None);
    let (first, proposal) = {
        let store = TursoMemoryStore::open_with_config(config.clone()).expect("open store");
        let fixture = CognitionFixture::new(store, source, "job");
        let outcome = fixture.apply().expect("first apply");
        (outcome, fixture.proposal.clone())
    };

    let reopened = CognitionFixture::resume(
        TursoMemoryStore::open_with_config(config).expect("reopen store"),
        proposal,
    );
    let replay = reopened.apply().expect("recover committed cognition");
    assert_eq!(replay.status, CognitionCommitStatus::AlreadyApplied);
    assert_eq!(replay.backend_commit_hash, first.backend_commit_hash);
    assert_eq!(replay.prior_version, first.prior_version);
    assert_eq!(replay.resulting_version, first.resulting_version);
    assert_eq!(replay.affected_ids, first.affected_ids);
    assert_eq!(replay.committed_at, first.committed_at);
    assert_eq!(replay.audit, first.audit);
}

#[test]
fn expired_worker_lease_commits_only_through_fresh_vault_authority() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_expired_proposal"))
        .expect("open store");
    let staged_at = chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
        .expect("past timestamp")
        .with_timezone(&chrono::Utc);
    let fixture = CognitionFixture::staged_at(
        store,
        record("source", "source text", None),
        "job",
        staged_at,
    );
    let staged = fixture
        .store()
        .cognition_job(&job_key("job"))
        .expect("read staged job")
        .expect("staged job exists");
    assert!(
        staged
            .lease
            .as_ref()
            .expect("proposal staging lease")
            .expires_at
            < chrono::Utc::now()
    );

    let outcome = fixture
        .apply()
        .expect("fresh TypeSec vault preparation authorizes commit");
    assert_eq!(outcome.status, CognitionCommitStatus::Applied);
}

#[test]
fn concurrent_identical_apply_recovers_byte_stable_committed_evidence() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_concurrent"))
        .expect("open store");
    let source = record("source", "source text", None);
    let fixture = Arc::new(CognitionFixture::new(store, source, "job"));
    let barrier = Arc::new(Barrier::new(2));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let fixture = Arc::clone(&fixture);
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            fixture.apply().expect("concurrent idempotent apply")
        }));
    }
    let left = threads.remove(0).join().expect("left worker");
    let right = threads.remove(0).join().expect("right worker");
    assert_eq!(left.backend_commit_hash, right.backend_commit_hash);
    assert_eq!(left.prior_version, right.prior_version);
    assert_eq!(left.resulting_version, right.resulting_version);
    assert_eq!(left.affected_ids, right.affected_ids);
    assert_eq!(left.committed_at, right.committed_at);
    assert_eq!(left.audit, right.audit);
}

#[test]
fn committed_key_rejects_a_different_proposal_digest() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_idempotency"))
        .expect("open store");
    let source = record("source", "source text", None);
    let fixture = CognitionFixture::new(store, source, "job");
    let binding = fixture.proposal.binding.as_ref().expect("proposal binding");
    let key = CognitionIdempotencyKey::for_authority(
        &binding.space_id,
        &binding.subject,
        &binding.purpose,
        &fixture.proposal.job_id,
    )
    .expect("canonical cognition key");
    fixture.apply().expect("first apply succeeds");

    assert!(matches!(
        fixture
            .store()
            .recover_cognition(&key, &digest("different proposal")),
        Err(typesec_memory::CognitionCommitError::IdempotencyConflict)
    ));
}

#[tokio::test]
async fn outbox_payload_is_id_only() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store =
        TursoMemoryStore::open_with_config(config(&dir, "cognition_outbox")).expect("open store");
    let source = record("source", "private source text", None);
    let fixture = CognitionFixture::new(store, source.clone(), "job");
    fixture.apply().expect("apply cognition");
    let nodes = fixture
        .store()
        .graph()
        .traverse(Traversal {
            start: Start::NodesByLabel("CognitionIndexOutbox".into()),
            steps: Vec::new(),
            limit: None,
        })
        .await
        .expect("read outbox");
    let encoded = serde_json::to_string(&nodes).expect("serialize outbox");
    assert!(encoded.contains(source.id.as_str()));
    assert!(!encoded.contains("private source text"));
    assert_eq!(
        serde_json::to_string(&IndexMutation::Remove(MemoryId::from_string("source")))
            .expect("serialize expected mutation"),
        r#"{"Remove":"source"}"#
    );
}

#[tokio::test]
async fn recovery_rejects_corrupt_durable_outcome_digests() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_outcome_integrity"))
        .expect("open store");
    let fixture = CognitionFixture::new(store, record("source", "source text", None), "job");
    fixture.apply().expect("apply cognition");
    let mut nodes = fixture
        .store()
        .graph()
        .traverse(Traversal {
            start: Start::NodesByLabel("CognitionOutcome".into()),
            steps: Vec::new(),
            limit: None,
        })
        .await
        .expect("read outcome nodes");
    let mut node = nodes.pop().expect("outcome node");
    let Value::Json(payload) = node.props.get_mut("payload").expect("outcome payload") else {
        panic!("outcome payload is JSON")
    };
    payload["resultingVersion"] = serde_json::json!("not-a-canonical-digest");
    fixture
        .store()
        .graph()
        .put_node(&node)
        .await
        .expect("tamper outcome fixture");

    assert!(matches!(
        fixture.apply(),
        Err(MemoryError::CognitionCommit(
            typesec_memory::CognitionCommitError::Store(_)
        ))
    ));
}

#[tokio::test]
async fn commit_rejects_a_staged_job_bound_to_another_typedid_request() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_request_commit"))
        .expect("open store");
    let source = record("source", "source text", None);
    let fixture = CognitionFixture::new(store, source.clone(), "job");
    replace_job_typedid_request(fixture.store(), digest("substituted TypeDID request")).await;

    assert!(matches!(
        fixture.apply(),
        Err(MemoryError::CognitionCommit(
            typesec_memory::CognitionCommitError::Store(_)
        ))
    ));
    assert_eq!(
        fixture
            .store()
            .get(&source.id)
            .expect("read source")
            .expect("source exists")
            .invalid_at,
        None
    );
    assert_eq!(
        fixture
            .store()
            .cognition_job(&job_key("job"))
            .expect("read job")
            .expect("job exists")
            .status,
        CognitionJobStatus::ProposalReady
    );
}

#[tokio::test]
async fn recovery_rejects_a_completed_job_bound_to_another_typedid_request() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_request_recovery"))
        .expect("open store");
    let fixture = CognitionFixture::new(store, record("source", "source text", None), "job");
    fixture.apply().expect("apply cognition");
    let proposal_digest = fixture
        .proposal
        .canonical_digest()
        .expect("proposal digest");
    replace_job_typedid_request(fixture.store(), digest("substituted TypeDID request")).await;

    assert!(matches!(
        fixture
            .store()
            .recover_cognition(&job_key("job"), &proposal_digest),
        Err(typesec_memory::CognitionCommitError::Store(_))
    ));
}

async fn replace_job_typedid_request(store: &TursoMemoryStore, digest: String) {
    let mut nodes = store
        .graph()
        .traverse(Traversal {
            start: Start::NodesByLabel("CognitionJob".into()),
            steps: Vec::new(),
            limit: None,
        })
        .await
        .expect("read job nodes");
    let mut node = nodes.pop().expect("job node");
    let Value::Json(payload) = node.props.get_mut("payload").expect("job payload") else {
        panic!("job payload is JSON")
    };
    payload["typedidRequestDigest"] = serde_json::json!(digest);
    store
        .graph()
        .put_node(&node)
        .await
        .expect("tamper job fixture");
}
