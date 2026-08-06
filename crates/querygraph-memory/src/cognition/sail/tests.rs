use super::*;

use std::sync::Arc;
use std::sync::Mutex;

use grust_core::prelude::GrustError;
use tokio::sync::Notify;

use crate::analytics::planning::assertion_parts;

const BACKEND_SECRET: &str = "protected staged plaintext from backend";

#[test]
fn concurrent_view_names_are_random_unique_and_safe() {
    let first = job_view();
    let second = job_view();
    assert_ne!(first, second);
    for view in [first, second] {
        assert!(view.starts_with("marciana_"));
        assert_eq!(view.len(), 41);
        assert!(
            view.bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        );
    }
}

#[test]
fn assertion_split_matches_reference_heuristic() {
    assert_eq!(
        assertion_parts("Alice lives in Rome"),
        ("alice lives in".into(), "rome".into())
    );
    assert_eq!(assertion_parts("singleton"), (String::new(), String::new()));
}

#[derive(Default)]
struct MockSailSession {
    events: Mutex<Vec<&'static str>>,
    query_budgets: Mutex<Vec<(usize, usize)>>,
    fail_stage: bool,
    fail_query: bool,
    fail_cleanup: bool,
    block_query: bool,
    query_started: Option<Arc<Notify>>,
    query_release: Option<Arc<Notify>>,
    cleanup_finished: Option<Arc<Notify>>,
}

impl MockSailSession {
    fn events(&self) -> Vec<&'static str> {
        self.events.lock().expect("mock event lock").clone()
    }

    fn record(&self, event: &'static str) {
        self.events.lock().expect("mock event lock").push(event);
    }

    fn query_budgets(&self) -> Vec<(usize, usize)> {
        self.query_budgets.lock().expect("mock budget lock").clone()
    }
}

#[async_trait::async_trait]
impl SailCognitionSession for MockSailSession {
    async fn stage_arrow_ipc_view(
        &self,
        _name: &str,
        _ipc_stream: &[u8],
    ) -> grust_core::prelude::Result<()> {
        self.record("stage");
        if self.fail_stage {
            Err(GrustError::Backend(format!(
                "stage failed: {BACKEND_SECRET}"
            )))
        } else {
            Ok(())
        }
    }

    async fn query_arrow_ipc_bounded(
        &self,
        _sql: &str,
        max_chunks: usize,
        max_bytes: usize,
    ) -> grust_core::prelude::Result<Vec<Vec<u8>>> {
        self.record("query");
        self.query_budgets
            .lock()
            .expect("mock budget lock")
            .push((max_chunks, max_bytes));
        if let Some(started) = &self.query_started {
            started.notify_one();
        }
        if self.block_query {
            if let Some(release) = &self.query_release {
                release.notified().await;
            } else {
                std::future::pending::<()>().await;
            }
        }
        if self.fail_query {
            Err(GrustError::Backend(format!(
                "query failed: {BACKEND_SECRET}"
            )))
        } else {
            Ok(Vec::new())
        }
    }

    async fn drop_arrow_ipc_view(&self, _name: &str) -> grust_core::prelude::Result<()> {
        self.record("cleanup");
        if let Some(finished) = &self.cleanup_finished {
            finished.notify_one();
        }
        if self.fail_cleanup {
            Err(GrustError::Backend(format!(
                "cleanup failed: {BACKEND_SECRET}"
            )))
        } else {
            Ok(())
        }
    }
}

async fn mock_execution(store: Arc<dyn SailCognitionSession>) -> Result<(), CognitionError> {
    store
        .stage_arrow_ipc_view("marciana_safe", &[])
        .await
        .map_err(sail_stage_error)?;
    store
        .query_arrow_ipc_bounded(
            "SELECT 1",
            budget::MAX_RESULT_CHUNKS,
            budget::MAX_ARROW_BYTES,
        )
        .await
        .map_err(sail_query_error)?;
    Ok(())
}

async fn run_mock(store: Arc<MockSailSession>) -> Result<(), CognitionError> {
    let session: Arc<dyn SailCognitionSession> = store;
    worker::run(session, "marciana_safe".into(), |store, _view| {
        mock_execution(store)
    })
    .await
}

#[tokio::test]
async fn temp_view_cleanup_runs_after_success() {
    let store = Arc::new(MockSailSession::default());
    run_mock(Arc::clone(&store))
        .await
        .expect("successful work and cleanup");
    assert_eq!(store.events(), vec!["stage", "query", "cleanup"]);
    assert_eq!(
        store.query_budgets(),
        vec![(budget::MAX_RESULT_CHUNKS, budget::MAX_ARROW_BYTES)]
    );
}

#[tokio::test]
async fn temp_view_cleanup_runs_after_staging_failure() {
    let store = Arc::new(MockSailSession {
        fail_stage: true,
        ..MockSailSession::default()
    });
    assert!(matches!(
        run_mock(Arc::clone(&store)).await,
        Err(CognitionError::Sail(message)) if message.contains("stage failed")
    ));
    assert_eq!(store.events(), vec!["stage", "cleanup"]);
}

#[tokio::test]
async fn cleanup_failure_preserves_the_primary_failure() {
    let store = Arc::new(MockSailSession {
        fail_query: true,
        fail_cleanup: true,
        ..MockSailSession::default()
    });
    let error = run_mock(Arc::clone(&store))
        .await
        .expect_err("query and cleanup fail");
    let CognitionError::SailCleanupAfterFailure { primary, cleanup } = error else {
        panic!("primary failure was not preserved")
    };
    assert!(matches!(
        *primary,
        CognitionError::Sail(message) if message.contains("query failed")
    ));
    assert!(cleanup.contains("cleanup failed"));
    assert_eq!(store.events(), vec!["stage", "query", "cleanup"]);
}

#[tokio::test]
async fn cleanup_failure_turns_success_into_an_error() {
    let store = Arc::new(MockSailSession {
        fail_cleanup: true,
        ..MockSailSession::default()
    });
    assert!(matches!(
        run_mock(Arc::clone(&store)).await,
        Err(CognitionError::SailCleanup(message)) if message.contains("cleanup failed")
    ));
    assert_eq!(store.events(), vec!["stage", "query", "cleanup"]);
}

#[tokio::test]
async fn adapter_failures_never_echo_backend_values() {
    let stage = Arc::new(MockSailSession {
        fail_stage: true,
        ..MockSailSession::default()
    });
    let stage_error = run_mock(Arc::clone(&stage)).await.expect_err("stage fails");
    assert!(!stage_error.to_string().contains(BACKEND_SECRET));

    let query_and_cleanup = Arc::new(MockSailSession {
        fail_query: true,
        fail_cleanup: true,
        ..MockSailSession::default()
    });
    let dual_error = run_mock(query_and_cleanup)
        .await
        .expect_err("query and cleanup fail");
    assert!(!dual_error.to_string().contains(BACKEND_SECRET));
}

#[tokio::test]
async fn outer_abort_still_drops_the_staged_view() {
    let query_started = Arc::new(Notify::new());
    let query_release = Arc::new(Notify::new());
    let cleanup_finished = Arc::new(Notify::new());
    let store = Arc::new(MockSailSession {
        block_query: true,
        query_started: Some(Arc::clone(&query_started)),
        query_release: Some(Arc::clone(&query_release)),
        cleanup_finished: Some(Arc::clone(&cleanup_finished)),
        ..MockSailSession::default()
    });
    let task = tokio::spawn(run_mock(Arc::clone(&store)));
    tokio::time::timeout(std::time::Duration::from_secs(1), query_started.notified())
        .await
        .expect("query started");

    task.abort();
    let _cancelled = task.await;
    assert_eq!(store.events(), vec!["stage", "query"]);
    query_release.notify_one();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        cleanup_finished.notified(),
    )
    .await
    .expect("cleanup completed after outer abort");
    assert_eq!(store.events(), vec!["stage", "query", "cleanup"]);
}

#[tokio::test]
async fn worker_join_failure_is_fixed_and_still_cleans_up() {
    let store = Arc::new(MockSailSession::default());
    let session: Arc<dyn SailCognitionSession> = store.clone();
    let error = worker::run(session, "marciana_safe".into(), |_store, _view| {
        panicking_operation()
    })
    .await
    .expect_err("worker panics");
    assert!(matches!(
        error,
        CognitionError::Sail(message) if message == "Sail worker failed"
    ));
    assert_eq!(store.events(), vec!["cleanup"]);
}

async fn panicking_operation() -> Result<(), CognitionError> {
    panic!("operation panic")
}
