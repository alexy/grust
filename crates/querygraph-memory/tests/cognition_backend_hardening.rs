#![cfg(feature = "turso")]

mod support;

use chrono::Duration;
use grust_core::prelude::GraphAdminStore;
use grust_turso::TursoGraphStore;
use querygraph_memory::GraphStoreMemoryStore;
use querygraph_memory::cognition::CognitionStateError;
use typesec_memory::{CognitionCommitError, CognitionCommitStore};

use support::cognition_vault::CognitionFixture;
use support::guarded_fault::{FaultControl, FaultingStore, GuardedFault};
use support::{at, config, digest, job_key, record};

const BACKEND_SECRET: &str = "protected-row-text/backend-secret";

#[tokio::test]
async fn replayed_submission_and_transition_require_the_exact_durable_job() {
    let dir = tempfile::tempdir().expect("temporary database");
    let (store, control) = faulting_store(&dir, "cognition_replay_job").await;
    control.set(GuardedFault::Replay {
        prefix: "cognition-state:".into(),
    });
    assert!(matches!(
        store.submit_cognition_job(&job_key("missing"), "owner", &digest("request"), 2, at(0)),
        Err(CognitionStateError::Backend(_))
    ));
    assert!(
        store
            .cognition_job(&job_key("missing"))
            .expect("read missing job")
            .is_none()
    );

    control.set(GuardedFault::Pass);
    store
        .submit_cognition_job(&job_key("job"), "owner", &digest("request"), 2, at(0))
        .expect("submit real job");
    control.set(GuardedFault::Replay {
        prefix: "cognition-state:".into(),
    });
    assert!(matches!(
        store.acquire_cognition_lease(&job_key("job"), "worker", at(1), Duration::minutes(1)),
        Err(CognitionStateError::Backend(_))
    ));
}

#[tokio::test]
async fn replayed_outbox_claim_requires_the_exact_durable_lease() {
    let dir = tempfile::tempdir().expect("temporary database");
    let (store, control) = faulting_store(&dir, "cognition_replay_outbox").await;
    let fixture = CognitionFixture::new(store, record("source", "source text", None), "job");
    fixture.apply().expect("seed committed outbox");
    control.set(GuardedFault::Replay {
        prefix: "cognition-outbox:".into(),
    });

    assert!(matches!(
        fixture.store().claim_cognition_outbox(
            &job_key("job"),
            "indexer",
            chrono::Utc::now(),
            Duration::minutes(1),
            1,
        ),
        Err(CognitionStateError::Backend(_))
    ));
}

#[tokio::test]
async fn generic_backend_errors_never_echo_protected_values() {
    let scheduler_dir = tempfile::tempdir().expect("scheduler database");
    let (scheduler, scheduler_control) =
        faulting_store(&scheduler_dir, "cognition_secret_scheduler").await;
    scheduler_control.set(backend_fault("cognition-state:"));
    let scheduler_error = scheduler
        .submit_cognition_job(&job_key("job"), "owner", &digest("request"), 2, at(0))
        .expect_err("scheduler backend fault");
    assert!(!scheduler_error.to_string().contains(BACKEND_SECRET));

    let commit_dir = tempfile::tempdir().expect("commit database");
    let (commit_store, commit_control) =
        faulting_store(&commit_dir, "cognition_secret_commit").await;
    let fixture = CognitionFixture::new(
        commit_store,
        record("source", "protected source text", None),
        "job",
    );
    commit_control.set(backend_fault("cognition-commit:"));
    let commit_error = fixture.apply().expect_err("commit backend fault");
    assert!(!commit_error.to_string().contains(BACKEND_SECRET));

    commit_control.set(GuardedFault::Pass);
    fixture.apply().expect("commit for outbox fault");
    commit_control.set(backend_fault("cognition-outbox:"));
    let outbox_error = fixture.store().claim_cognition_outbox(
        &job_key("job"),
        "indexer",
        chrono::Utc::now(),
        Duration::minutes(1),
        1,
    );
    let Err(outbox_error) = outbox_error else {
        panic!("outbox backend fault must fail")
    };
    assert!(!outbox_error.to_string().contains(BACKEND_SECRET));
}

#[tokio::test]
async fn repeated_missing_ledger_recovery_never_creates_a_receipt() {
    let dir = tempfile::tempdir().expect("temporary database");
    let (store, control) = faulting_store(&dir, "cognition_missing_ledger").await;
    let fixture = CognitionFixture::new(store, record("source", "source text", None), "job");
    fixture.apply().expect("seed durable outcome");
    let proposal_digest = fixture
        .proposal
        .canonical_digest()
        .expect("proposal digest");
    let commits_before = control.commit_calls();
    let recovery_before = control.recovery_calls();
    control.set(GuardedFault::HideRecovery {
        prefix: "cognition-commit:".into(),
    });

    for _ in 0..2 {
        assert!(matches!(
            fixture
                .store()
                .recover_cognition(&job_key("job"), &proposal_digest),
            Err(CognitionCommitError::Store(_))
        ));
    }
    assert_eq!(control.commit_calls(), commits_before);
    assert_eq!(control.recovery_calls(), recovery_before + 2);
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

fn backend_fault(prefix: &str) -> GuardedFault {
    GuardedFault::BackendError {
        prefix: prefix.into(),
        secret: BACKEND_SECRET.into(),
    }
}
