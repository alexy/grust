use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tokio::sync::Notify;

use super::*;

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct DeadlineSession {
    hang_cleanup: bool,
    cleanup_calls: AtomicUsize,
    cleanup_started: Notify,
    cleanup_finished: Notify,
    cleanup_dropped: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl SailCognitionSession for DeadlineSession {
    async fn stage_arrow_ipc_view(
        &self,
        _name: &str,
        _ipc_stream: &[u8],
    ) -> grust_core::prelude::Result<()> {
        Ok(())
    }

    async fn query_arrow_ipc_bounded(
        &self,
        _sql: &str,
        _max_chunks: usize,
        _max_bytes: usize,
    ) -> grust_core::prelude::Result<Vec<Vec<u8>>> {
        Ok(Vec::new())
    }

    async fn drop_arrow_ipc_view(&self, _name: &str) -> grust_core::prelude::Result<()> {
        self.cleanup_calls.fetch_add(1, Ordering::SeqCst);
        self.cleanup_started.notify_one();
        if self.hang_cleanup {
            let _drop = DropFlag(Arc::clone(&self.cleanup_dropped));
            std::future::pending().await
        } else {
            self.cleanup_finished.notify_one();
            Ok(())
        }
    }
}

fn short_deadlines() -> WorkerDeadlines {
    WorkerDeadlines {
        operation: Duration::from_secs(10),
        abort: Duration::from_secs(1),
        cleanup: Duration::from_secs(2),
    }
}

#[tokio::test(start_paused = true)]
async fn timed_out_operation_is_aborted_before_cleanup() {
    let store = Arc::new(DeadlineSession::default());
    let operation_started = Arc::new(Notify::new());
    let operation_dropped = Arc::new(AtomicBool::new(false));
    let session: Arc<dyn SailCognitionSession> = store.clone();
    let started = Arc::clone(&operation_started);
    let dropped = Arc::clone(&operation_dropped);
    let task = tokio::spawn(run_with_deadlines(
        session,
        "bounded_view".into(),
        move |_store, _view| async move {
            let _drop = DropFlag(dropped);
            started.notify_one();
            std::future::pending::<Result<(), CognitionError>>().await
        },
        short_deadlines(),
    ));
    operation_started.notified().await;

    tokio::time::advance(short_deadlines().operation).await;
    let error = task
        .await
        .expect("deadline worker joins")
        .expect_err("operation deadline must fail");
    assert!(matches!(
        error,
        CognitionError::Sail("Sail operation timed out")
    ));
    assert!(operation_dropped.load(Ordering::SeqCst));
    assert_eq!(store.cleanup_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn hung_cleanup_is_cancelled_at_its_own_deadline() {
    let store = Arc::new(DeadlineSession {
        hang_cleanup: true,
        ..DeadlineSession::default()
    });
    let session: Arc<dyn SailCognitionSession> = store.clone();
    let task = tokio::spawn(run_with_deadlines(
        session,
        "bounded_view".into(),
        |_store, _view| async { Ok::<_, CognitionError>(()) },
        short_deadlines(),
    ));
    store.cleanup_started.notified().await;

    tokio::time::advance(short_deadlines().cleanup).await;
    let error = task
        .await
        .expect("deadline worker joins")
        .expect_err("cleanup deadline must fail");
    assert!(matches!(
        error,
        CognitionError::SailCleanup("Sail cleanup timed out")
    ));
    assert!(store.cleanup_dropped.load(Ordering::SeqCst));
    assert_eq!(store.cleanup_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn caller_cancellation_cannot_leave_a_hung_operation_forever() {
    let store = Arc::new(DeadlineSession::default());
    let operation_started = Arc::new(Notify::new());
    let operation_dropped = Arc::new(AtomicBool::new(false));
    let session: Arc<dyn SailCognitionSession> = store.clone();
    let started = Arc::clone(&operation_started);
    let dropped = Arc::clone(&operation_dropped);
    let caller = tokio::spawn(run_with_deadlines(
        session,
        "bounded_view".into(),
        move |_store, _view| async move {
            let _drop = DropFlag(dropped);
            started.notify_one();
            std::future::pending::<Result<(), CognitionError>>().await
        },
        short_deadlines(),
    ));
    operation_started.notified().await;
    caller.abort();
    assert!(
        caller
            .await
            .expect_err("caller is cancelled")
            .is_cancelled()
    );

    tokio::time::advance(short_deadlines().operation).await;
    store.cleanup_finished.notified().await;
    assert!(operation_dropped.load(Ordering::SeqCst));
    assert_eq!(store.cleanup_calls.load(Ordering::SeqCst), 1);
}
