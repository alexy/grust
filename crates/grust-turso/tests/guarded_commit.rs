use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, SecondsFormat, Timelike, Utc};
use grust_core::prelude::{
    Edge, GraphAdminStore, GraphCommitStore, GraphExpectation, GraphMutation, GraphStore,
    GrustError, GuardedGraphCommit, Node, NodeId, Value,
};
use grust_turso::{TursoConfig, TursoGraphStore};

fn node(id: &str, value: &str) -> Node {
    Node::new(
        "GuardedTest",
        id,
        BTreeMap::from([("value".to_string(), Value::String(value.to_string()))]),
    )
}

async fn open(path: &Path, prefix: &str) -> TursoGraphStore {
    let store = TursoGraphStore::connect(TursoConfig {
        path: path.to_string_lossy().into_owned(),
        table_prefix: prefix.to_string(),
        ..TursoConfig::default()
    })
    .await
    .expect("open Turso store");
    store.bootstrap().await.expect("bootstrap Turso store");
    store
}

#[tokio::test]
async fn guarded_commit_replays_and_rejects_key_digest_collision() {
    let dir = tempfile::tempdir().expect("temporary database directory");
    let store = open(&dir.path().join("replay.db"), "guarded_replay").await;
    let request = GuardedGraphCommit::new(
        "job-1/apply",
        "sha256:request-1",
        vec![GraphMutation::UpsertNode(node("n1", "first"))],
    )
    .with_expectations(vec![GraphExpectation::Absent(NodeId::from("n1"))]);

    let first = store
        .commit_guarded(&request)
        .await
        .expect("first guarded commit");
    assert!(!first.replayed);
    assert!(first.commit_id.starts_with("turso:"));
    assert_eq!(
        store
            .get_node(&NodeId::from("n1"))
            .await
            .expect("read committed node"),
        Some(node("n1", "first"))
    );

    let replay = store
        .commit_guarded(&request)
        .await
        .expect("idempotent replay");
    assert!(replay.replayed);
    assert_eq!(replay.commit_id, first.commit_id);
    assert_eq!(replay.committed_at, first.committed_at);

    let collision = GuardedGraphCommit::new(
        "job-1/apply",
        "sha256:different-request",
        vec![GraphMutation::UpsertNode(node("n1", "wrong"))],
    );
    assert!(matches!(
        store.commit_guarded(&collision).await,
        Err(GrustError::GraphIdempotencyConflict(key)) if key == "job-1/apply"
    ));
    assert_eq!(
        store
            .get_node(&NodeId::from("n1"))
            .await
            .expect("read node after collision"),
        Some(node("n1", "first"))
    );
}

#[tokio::test]
async fn guarded_commit_receipt_preserves_submillisecond_ordering() {
    let dir = tempfile::tempdir().expect("temporary database directory");
    let store = open(&dir.path().join("precision.db"), "guarded_precision").await;
    let prepared_at = Utc::now();
    let receipt = store
        .commit_guarded(&GuardedGraphCommit::new(
            "job-precision",
            "sha256:precision",
            vec![GraphMutation::UpsertNode(node("precision", "value"))],
        ))
        .await
        .expect("guarded commit with precise receipt");
    let committed_at = DateTime::parse_from_rfc3339(&receipt.committed_at)
        .expect("canonical receipt timestamp")
        .with_timezone(&Utc);

    assert!(committed_at >= prepared_at);
    assert_eq!(
        receipt.committed_at,
        committed_at.to_rfc3339_opts(SecondsFormat::Nanos, true)
    );
    assert_eq!(
        receipt
            .committed_at
            .split_once('.')
            .and_then(|(_, suffix)| suffix.strip_suffix('Z'))
            .map(str::len),
        Some(9)
    );
    if committed_at.nanosecond() % 1_000_000 != 0 {
        let same_millisecond_preparation = committed_at - chrono::TimeDelta::nanoseconds(1);
        assert_eq!(
            same_millisecond_preparation.timestamp_millis(),
            committed_at.timestamp_millis()
        );
        assert!(committed_at >= same_millisecond_preparation);
    }
}

#[tokio::test]
async fn guarded_commit_recovery_is_read_only_and_stable() {
    let dir = tempfile::tempdir().expect("temporary database directory");
    let store = open(&dir.path().join("recovery.db"), "guarded_recovery").await;

    for _ in 0..2 {
        assert_eq!(
            store
                .recover_guarded_commit("job-recovery", "sha256:recovery")
                .await
                .expect("look up absent guarded commit"),
            None
        );
    }

    let request = GuardedGraphCommit::new(
        "job-recovery",
        "sha256:recovery",
        vec![GraphMutation::UpsertNode(node("recovered", "value"))],
    );
    let first = store
        .commit_guarded(&request)
        .await
        .expect("commit after absent recovery lookups");
    assert!(
        !first.replayed,
        "absent recovery lookups must not create a commit receipt"
    );

    let mut expected = first.clone();
    expected.replayed = true;
    for _ in 0..2 {
        assert_eq!(
            store
                .recover_guarded_commit("job-recovery", "sha256:recovery")
                .await
                .expect("recover committed receipt"),
            Some(expected.clone())
        );
    }

    assert!(matches!(
        store
            .recover_guarded_commit("job-recovery", "sha256:different")
            .await,
        Err(GrustError::GraphIdempotencyConflict(key)) if key == "job-recovery"
    ));
}

#[tokio::test]
async fn guarded_commit_recovery_rejects_empty_identity_inputs() {
    let dir = tempfile::tempdir().expect("temporary database directory");
    let store = open(
        &dir.path().join("recovery-validation.db"),
        "guarded_recovery_validation",
    )
    .await;

    assert!(matches!(
        store
            .recover_guarded_commit("  ", "sha256:request")
            .await,
        Err(GrustError::Schema(message))
            if message == "guarded commit idempotency key must not be empty"
    ));
    assert!(matches!(
        store.recover_guarded_commit("job-validation", "\t").await,
        Err(GrustError::Schema(message))
            if message == "guarded commit request digest must not be empty"
    ));
}

#[tokio::test]
async fn absent_and_exact_expectations_fail_without_partial_writes() {
    let dir = tempfile::tempdir().expect("temporary database directory");
    let store = open(&dir.path().join("expectations.db"), "guarded_expect").await;
    store
        .put_node(&node("source", "v1"))
        .await
        .expect("seed source node");
    let expected = store
        .get_node(&NodeId::from("source"))
        .await
        .expect("read source")
        .expect("source exists");

    let absent_failure = GuardedGraphCommit::new(
        "job-absent",
        "sha256:absent",
        vec![GraphMutation::UpsertNode(node(
            "side-effect",
            "must-not-land",
        ))],
    )
    .with_expectations(vec![GraphExpectation::Absent(NodeId::from("source"))]);
    assert!(matches!(
        store.commit_guarded(&absent_failure).await,
        Err(GrustError::GraphExpectationFailed(_))
    ));
    assert!(
        store
            .get_node(&NodeId::from("side-effect"))
            .await
            .expect("read side effect")
            .is_none()
    );

    store
        .put_node(&node("source", "v2"))
        .await
        .expect("concurrent source update");
    let exact_failure = GuardedGraphCommit::new(
        "job-exact",
        "sha256:exact",
        vec![GraphMutation::UpsertNode(node(
            "other-effect",
            "must-not-land",
        ))],
    )
    .with_expectations(vec![GraphExpectation::Exact(expected)]);
    assert!(matches!(
        store.commit_guarded(&exact_failure).await,
        Err(GrustError::GraphExpectationFailed(_))
    ));
    assert!(
        store
            .get_node(&NodeId::from("other-effect"))
            .await
            .expect("read other side effect")
            .is_none()
    );
}

#[tokio::test]
async fn mutation_failure_rolls_back_graph_and_commit_ledger() {
    let dir = tempfile::tempdir().expect("temporary database directory");
    let store = open(&dir.path().join("rollback.db"), "guarded_rollback").await;
    let request = GuardedGraphCommit::new(
        "job-rollback",
        "sha256:rollback",
        vec![
            GraphMutation::UpsertNode(node("partial", "must-roll-back")),
            GraphMutation::UpsertEdge(Edge::new(
                "MISSING_ENDPOINTS",
                "missing-from",
                "missing-to",
                BTreeMap::new(),
            )),
        ],
    );

    assert!(store.commit_guarded(&request).await.is_err());
    assert!(
        store
            .get_node(&NodeId::from("partial"))
            .await
            .expect("read rolled-back node")
            .is_none()
    );
    assert!(
        store.commit_guarded(&request).await.is_err(),
        "a failed transaction must not leave an idempotent success receipt"
    );
}

#[tokio::test]
async fn commit_receipt_survives_close_and_reopen() {
    let dir = tempfile::tempdir().expect("temporary database directory");
    let path = dir.path().join("persistent.db");
    let request = GuardedGraphCommit::new(
        "job-persistent",
        "sha256:persistent",
        vec![GraphMutation::UpsertNode(node("persistent", "value"))],
    );
    let first = {
        let store = open(&path, "guarded_persistent").await;
        store
            .commit_guarded(&request)
            .await
            .expect("commit before close")
    };

    let reopened = open(&path, "guarded_persistent").await;
    let replay = reopened
        .commit_guarded(&request)
        .await
        .expect("replay after reopen");
    assert!(replay.replayed);
    assert_eq!(replay.commit_id, first.commit_id);
    assert_eq!(replay.committed_at, first.committed_at);
}

#[tokio::test]
async fn malformed_commit_receipt_fails_closed_after_reopen() {
    let dir = tempfile::tempdir().expect("temporary database directory");
    let path = dir.path().join("malformed-receipt.db");
    let request = GuardedGraphCommit::new(
        "job-malformed",
        "sha256:malformed",
        vec![GraphMutation::UpsertNode(node("malformed", "value"))],
    );
    {
        let store = open(&path, "guarded_malformed").await;
        store
            .commit_guarded(&request)
            .await
            .expect("commit before receipt tampering");
    }
    let raw = "protected malformed timestamp";
    rusqlite::Connection::open(&path)
        .expect("open raw fixture database")
        .execute(
            "UPDATE guarded_malformed_commits SET committed_at = ?1 \
             WHERE idempotency_key = ?2",
            (raw, "job-malformed"),
        )
        .expect("tamper receipt timestamp");

    let reopened = open(&path, "guarded_malformed").await;
    let error = reopened
        .recover_guarded_commit("job-malformed", "sha256:malformed")
        .await
        .expect_err("malformed durable receipt must fail closed");
    assert_eq!(
        error.to_string(),
        "backend error: Turso commit ledger timestamp is invalid"
    );
    assert!(!error.to_string().contains(raw));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_same_request_commits_once() {
    let dir = tempfile::tempdir().expect("temporary database directory");
    let path = dir.path().join("concurrent.db");
    let first_store = open(&path, "guarded_concurrent").await;
    let second_store = open(&path, "guarded_concurrent").await;
    let request = GuardedGraphCommit::new(
        "job-concurrent",
        "sha256:concurrent",
        vec![GraphMutation::UpsertNode(node("winner", "one"))],
    )
    .with_expectations(vec![GraphExpectation::Absent(NodeId::from("winner"))]);
    let other_request = request.clone();

    let (first, second) = tokio::join!(
        first_store.commit_guarded(&request),
        second_store.commit_guarded(&other_request)
    );
    let first = first.expect("first concurrent request");
    let second = second.expect("second concurrent request");
    assert_eq!(first.commit_id, second.commit_id);
    assert_ne!(first.replayed, second.replayed);
}
