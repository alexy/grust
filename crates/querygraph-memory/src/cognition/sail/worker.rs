//! Owned stage/query/cleanup worker that survives caller cancellation.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use super::{SailCognitionSession, sail_cleanup_error};
use crate::cognition::CognitionError;

const OPERATION_DEADLINE: Duration = Duration::from_secs(10 * 60);
const ABORT_DEADLINE: Duration = Duration::from_secs(5);
const CLEANUP_DEADLINE: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
struct WorkerDeadlines {
    operation: Duration,
    abort: Duration,
    cleanup: Duration,
}

const DEFAULT_DEADLINES: WorkerDeadlines = WorkerDeadlines {
    operation: OPERATION_DEADLINE,
    abort: ABORT_DEADLINE,
    cleanup: CLEANUP_DEADLINE,
};

pub(super) async fn run<T, Operation, Work>(
    store: Arc<dyn SailCognitionSession>,
    view: String,
    operation: Operation,
) -> Result<T, CognitionError>
where
    T: Send + 'static,
    Operation: FnOnce(Arc<dyn SailCognitionSession>, String) -> Work + Send + 'static,
    Work: Future<Output = Result<T, CognitionError>> + Send + 'static,
{
    run_with_deadlines(store, view, operation, DEFAULT_DEADLINES).await
}

async fn run_with_deadlines<T, Operation, Work>(
    store: Arc<dyn SailCognitionSession>,
    view: String,
    operation: Operation,
    deadlines: WorkerDeadlines,
) -> Result<T, CognitionError>
where
    T: Send + 'static,
    Operation: FnOnce(Arc<dyn SailCognitionSession>, String) -> Work + Send + 'static,
    Work: Future<Output = Result<T, CognitionError>> + Send + 'static,
{
    let worker = tokio::spawn(async move {
        let operation_store = Arc::clone(&store);
        let operation_view = view.clone();
        let mut operation_task =
            tokio::spawn(async move { operation(operation_store, operation_view).await });
        let result = match tokio::time::timeout(deadlines.operation, &mut operation_task).await {
            Ok(joined) => joined.map_err(join_error).and_then(|result| result),
            Err(_) => {
                operation_task.abort();
                let _ = tokio::time::timeout(deadlines.abort, &mut operation_task).await;
                Err(CognitionError::Sail("Sail operation timed out"))
            }
        };
        let cleanup =
            match tokio::time::timeout(deadlines.cleanup, store.drop_arrow_ipc_view(&view)).await {
                Ok(cleanup) => cleanup.map_err(sail_cleanup_error),
                Err(_) => Err("Sail cleanup timed out"),
            };
        combine(result, cleanup)
    });
    worker.await.map_err(join_error)?
}

fn join_error(_: tokio::task::JoinError) -> CognitionError {
    CognitionError::Sail("Sail worker failed")
}

#[cfg(test)]
mod tests;

fn combine<T>(
    result: Result<T, CognitionError>,
    cleanup: Result<(), &'static str>,
) -> Result<T, CognitionError> {
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(cleanup)) => Err(CognitionError::SailCleanup(cleanup)),
        (Err(primary), Ok(())) => Err(primary),
        (Err(primary), Err(cleanup)) => Err(CognitionError::SailCleanupAfterFailure {
            primary: Box::new(primary),
            cleanup,
        }),
    }
}
