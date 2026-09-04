#![cfg(feature = "turso")]

mod support;

use std::sync::{Arc, Barrier};

use chrono::{Duration, Utc};
use grust_core::prelude::{GraphAdminStore, GraphCommitStore, GraphStore, Start, Traversal, Value};
use grust_turso::TursoGraphStore;
use querygraph_memory::cognition::CognitionJobStatus;
use querygraph_memory::{GraphStoreMemoryStore, TursoMemoryStore};
use typesec_memory::{
    CognitionAuditEvidence, CognitionCommitError, CognitionCommitOutcome, CognitionCommitStatus,
    CognitionEffect, CognitionSourcePrecondition, MemoryError, MemoryStore,
};

use support::cognition_vault::CognitionFixture;
use support::guarded_fault::{FaultControl, FaultingStore, GuardedFault};
use support::{config, job_key, record};

#[tokio::test]
async fn no_change_commits_decision_without_memory_or_outbox_mutation() {
    let dir = tempfile::tempdir().expect("temporary database");
    let source = record("source", "source text", None);
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_no_change"))
        .expect("open store");
    let expected_source =
        CognitionSourcePrecondition::for_record(&source).expect("source precondition");
    let fixture = CognitionFixture::no_change(store, source.clone(), "job");

    let outcome = fixture.apply().expect("commit no-change decision");

    assert_eq!(outcome.status, CognitionCommitStatus::Applied);
    assert_eq!(outcome.effect, CognitionEffect::NoChange);
    assert_eq!(outcome.audit.effect, CognitionEffect::NoChange);
    assert!(outcome.affected_ids.is_empty());
    assert_eq!(outcome.prior_version, outcome.resulting_version);
    let stored_source = fixture
        .store()
        .get(&source.id)
        .expect("read source after no-change")
        .expect("source remains");
    assert_eq!(
        CognitionSourcePrecondition::for_record(&stored_source).expect("stored precondition"),
        expected_source
    );
    assert_no_change_graph(fixture.store()).await;
    assert!(
        fixture
            .store()
            .claim_cognition_outbox(
                &job_key("job"),
                "worker",
                Utc::now(),
                Duration::minutes(1),
                1,
            )
            .expect("claim empty no-change outbox")
            .is_empty()
    );

    let outcome_node = only_node(fixture.store(), "CognitionOutcome").await;
    assert_eq!(payload(&outcome_node)["schemaVersion"], 3);
    assert_eq!(payload(&outcome_node)["effect"], "no_change");
    let audit_node = only_node(fixture.store(), "CognitionAudit").await;
    assert_eq!(
        payload(&audit_node)["schemaVersion"],
        CognitionAuditEvidence::SCHEMA_VERSION
    );
    assert_eq!(payload(&audit_node)["effect"], "no_change");
    let job_node = only_node(fixture.store(), "CognitionJob").await;
    assert_eq!(
        payload(&job_node)["completionDigest"],
        payload(&outcome_node)["preparedDigest"]
    );
    assert_ne!(
        payload(&job_node)["completionDigest"],
        payload(&outcome_node)["resultingVersion"]
    );
}

#[test]
fn concurrent_identical_no_change_recovers_byte_stable_evidence() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_no_change_race"))
        .expect("open store");
    let fixture = Arc::new(CognitionFixture::no_change(
        store,
        record("source", "source text", None),
        "job",
    ));
    let barrier = Arc::new(Barrier::new(2));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let fixture = Arc::clone(&fixture);
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            fixture.apply().expect("concurrent no-change apply")
        }));
    }

    let left = threads.remove(0).join().expect("left worker");
    let right = threads.remove(0).join().expect("right worker");
    assert_same_commit(&left, &right);
    assert!(matches!(
        (left.status, right.status),
        (
            CognitionCommitStatus::Applied,
            CognitionCommitStatus::AlreadyApplied
        ) | (
            CognitionCommitStatus::AlreadyApplied,
            CognitionCommitStatus::Applied
        )
    ));
}

#[tokio::test]
async fn no_change_retry_and_reopen_return_the_original_decision() {
    let dir = tempfile::tempdir().expect("temporary database");
    let config = config(&dir, "cognition_no_change_reopen");
    let (first, retry, proposal) = {
        let store = TursoMemoryStore::open_with_config(config.clone()).expect("open store");
        let fixture =
            CognitionFixture::no_change(store, record("source", "source text", None), "job");
        let first = fixture.apply().expect("commit no-change decision");
        let retry = fixture.apply().expect("retry no-change decision");
        assert_eq!(retry.status, CognitionCommitStatus::AlreadyApplied);
        assert_same_commit(&first, &retry);
        (first, retry, fixture.proposal.clone())
    };

    let reopened = CognitionFixture::resume(
        TursoMemoryStore::open_with_config(config).expect("reopen store"),
        proposal,
    );
    let recovered = reopened.apply().expect("recover no-change after reopen");
    assert_eq!(recovered.status, CognitionCommitStatus::AlreadyApplied);
    assert_same_commit(&first, &recovered);
    assert_eq!(recovered, retry);
    assert_no_change_graph(reopened.store()).await;
}

#[tokio::test]
async fn no_change_response_loss_recovers_without_recommit_or_outbox() {
    let dir = tempfile::tempdir().expect("temporary database");
    let prefix = "cognition_no_change_response_loss";
    let (retry, proposal) = {
        let (store, control) = faulting_store(&dir, prefix).await;
        let fixture =
            CognitionFixture::no_change(store, record("source", "source text", None), "job");
        control.set(GuardedFault::CommitThenResponseLoss {
            prefix: "cognition-commit:".into(),
            secret: "protected backend response".into(),
        });
        fixture
            .apply()
            .expect_err("successful no-change response is lost");
        let calls_after_commit = control.commit_calls();

        control.set(GuardedFault::Pass);
        let retry = fixture
            .apply()
            .expect("recover no-change after response loss");
        assert_eq!(retry.status, CognitionCommitStatus::AlreadyApplied);
        assert_eq!(retry.effect, CognitionEffect::NoChange);
        assert_eq!(control.commit_calls(), calls_after_commit);
        assert_no_change_graph(fixture.store()).await;
        (retry, fixture.proposal.clone())
    };

    let (store, control) = faulting_store(&dir, prefix).await;
    let reopened = CognitionFixture::resume(store, proposal);
    let recovered = reopened.apply().expect("recover reopened no-change");
    assert_eq!(recovered, retry);
    assert_eq!(control.commit_calls(), 0);
    assert_no_change_graph(reopened.store()).await;
}

#[tokio::test]
async fn no_change_effect_and_outbox_tampering_fail_closed() {
    for (prefix, field, value) in [
        (
            "cognition_no_change_effect_tamper",
            "effect",
            serde_json::json!("mutated"),
        ),
        (
            "cognition_no_change_outbox_tamper",
            "outboxNodeIds",
            serde_json::json!([format!("cog-outbox:{}", "a".repeat(64))]),
        ),
    ] {
        let dir = tempfile::tempdir().expect("temporary database");
        let store = TursoMemoryStore::open_with_config(config(&dir, prefix)).expect("open store");
        let fixture =
            CognitionFixture::no_change(store, record("source", "source text", None), "job");
        fixture.apply().expect("commit no-change decision");
        let mut outcome = only_node(fixture.store(), "CognitionOutcome").await;
        payload_mut(&mut outcome)[field] = value;
        fixture
            .store()
            .graph()
            .put_node(&outcome)
            .await
            .expect("tamper no-change outcome");

        assert!(matches!(
            fixture.apply(),
            Err(MemoryError::CognitionCommit(CognitionCommitError::Store(_)))
        ));
    }
}

async fn assert_no_change_graph<G: GraphCommitStore>(store: &GraphStoreMemoryStore<G>) {
    assert_eq!(
        store
            .cognition_job(&job_key("job"))
            .expect("read job")
            .expect("job exists")
            .status,
        CognitionJobStatus::Completed
    );
    for (label, expected) in [
        ("MemoryRecord", 1),
        ("CognitionJob", 1),
        ("CognitionIndexOutbox", 0),
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
            .expect("read cognition graph");
        assert_eq!(nodes.len(), expected, "unexpected {label} count");
    }
}

fn assert_same_commit(left: &CognitionCommitOutcome, right: &CognitionCommitOutcome) {
    let mut left = left.clone();
    let mut right = right.clone();
    left.status = CognitionCommitStatus::Applied;
    right.status = CognitionCommitStatus::Applied;
    assert_eq!(left, right);
    assert_eq!(
        serde_json::to_vec(&left).expect("serialize left outcome"),
        serde_json::to_vec(&right).expect("serialize right outcome")
    );
}

async fn only_node<G: GraphCommitStore>(
    store: &GraphStoreMemoryStore<G>,
    label: &str,
) -> grust_core::prelude::Node {
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

fn payload(node: &grust_core::prelude::Node) -> &serde_json::Value {
    let Some(Value::Json(payload)) = node.props.get("payload") else {
        panic!("cognition payload is JSON")
    };
    payload
}

fn payload_mut(node: &mut grust_core::prelude::Node) -> &mut serde_json::Value {
    let Some(Value::Json(payload)) = node.props.get_mut("payload") else {
        panic!("cognition payload is JSON")
    };
    payload
}

async fn faulting_store(
    dir: &tempfile::TempDir,
    prefix: &str,
) -> (GraphStoreMemoryStore<FaultingStore>, FaultControl) {
    let graph = TursoGraphStore::connect(config(dir, prefix))
        .await
        .expect("connect graph");
    graph.bootstrap().await.expect("bootstrap graph");
    let (graph, control) = FaultingStore::new(graph);
    (GraphStoreMemoryStore::new(graph), control)
}
