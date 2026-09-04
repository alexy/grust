//! Live Spark Connect implementation of the governed cognition executor.

use std::sync::Arc;

use grust_sail::SailGraphStore;

use super::{
    CognitionError, CognitionOperation, CognitionRequest, SailCognitionExecutor,
    SailCognitionExecutorError, SailCognitionOutput,
};

mod output;
mod request;
mod worker;

use crate::cognition::budget;

/// Executes bounded cognition analysis on a live Sail Spark Connect service.
///
/// Staged normalized and contradiction keys are content-derived and are not
/// anonymization. The Sail endpoint must therefore be inside the processing
/// boundary authorized for the request's protected input.
pub struct LiveSailCognitionExecutor {
    store: Arc<dyn SailCognitionSession>,
}

impl LiveSailCognitionExecutor {
    /// Use an established Sail session. The session owns the temporary Arrow views.
    pub fn new(store: Arc<SailGraphStore>) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
trait SailCognitionSession: Send + Sync {
    async fn stage_arrow_ipc_view(
        &self,
        name: &str,
        ipc_stream: &[u8],
    ) -> grust_core::prelude::Result<()>;

    /// Return result data bounded before the transport retains excessive chunks.
    async fn query_arrow_ipc_bounded(
        &self,
        sql: &str,
        max_chunks: usize,
        max_bytes: usize,
    ) -> grust_core::prelude::Result<Vec<Vec<u8>>>;

    async fn drop_arrow_ipc_view(&self, name: &str) -> grust_core::prelude::Result<()>;
}

#[async_trait::async_trait]
impl SailCognitionSession for SailGraphStore {
    async fn stage_arrow_ipc_view(
        &self,
        name: &str,
        ipc_stream: &[u8],
    ) -> grust_core::prelude::Result<()> {
        SailGraphStore::stage_arrow_ipc_view(self, name, ipc_stream).await
    }

    async fn query_arrow_ipc_bounded(
        &self,
        sql: &str,
        max_chunks: usize,
        max_bytes: usize,
    ) -> grust_core::prelude::Result<Vec<Vec<u8>>> {
        SailGraphStore::query_arrow_ipc_bounded(self, sql, max_chunks, max_bytes).await
    }

    async fn drop_arrow_ipc_view(&self, name: &str) -> grust_core::prelude::Result<()> {
        SailGraphStore::drop_arrow_ipc_view(self, name).await
    }
}

#[async_trait::async_trait]
impl SailCognitionExecutor for LiveSailCognitionExecutor {
    async fn execute(
        &self,
        request: &CognitionRequest<'_>,
    ) -> Result<SailCognitionOutput, SailCognitionExecutorError> {
        super::engine_validation::validate_request(request).map_err(sanitize_executor_error)?;

        let view = job_view();
        let ipc = request::encode(request.input.memories()).map_err(sanitize_executor_error)?;
        let work = OwnedCognitionWork {
            operation: request.operation,
            memories: request::owned_planning_memories(request.input.memories()),
            snapshot_id: request.source.snapshot_id,
        };
        worker::run(
            Arc::clone(&self.store),
            view,
            move |store, view| async move {
                store
                    .stage_arrow_ipc_view(&view, &ipc)
                    .await
                    .map_err(sail_stage_error)?;
                execute_staged(store.as_ref(), &work, &view).await
            },
        )
        .await
        .map_err(sanitize_executor_error)
    }
}

struct OwnedCognitionWork {
    operation: CognitionOperation,
    memories: Vec<typesec_memory::RecalledMemory>,
    snapshot_id: i64,
}

async fn execute_staged(
    store: &dyn SailCognitionSession,
    work: &OwnedCognitionWork,
    view: &str,
) -> Result<SailCognitionOutput, CognitionError> {
    let (plan, count) = match work.operation {
        CognitionOperation::Deduplicate => {
            let sql = format!(
                "SELECT normalized, id FROM {view} WHERE normalized IN \
                 (SELECT normalized FROM {view} GROUP BY normalized HAVING COUNT(*) > 1) \
                 ORDER BY normalized, valid_from, id LIMIT {}",
                budget::MAX_RESULT_ROWS + 1
            );
            let rows = output::query_pairs(store, &sql).await?;
            output::dedup_plan(&work.memories, rows)?
        }
        CognitionOperation::Reconcile => {
            let sql = format!(
                "SELECT newer.id, older.id FROM {view} newer JOIN {view} older \
                 ON newer.prefix = older.prefix AND newer.tail <> older.tail \
                 AND (newer.valid_from > older.valid_from OR \
                      (newer.valid_from = older.valid_from AND newer.id > older.id)) \
                 WHERE newer.prefix <> '' ORDER BY newer.id, older.id LIMIT {}",
                budget::MAX_RESULT_ROWS + 1
            );
            let rows = output::query_pairs(store, &sql).await?;
            output::reconcile_plan(&work.memories, rows)?
        }
    };

    Ok(SailCognitionOutput {
        plan,
        evidence: vec![format!(
            "sail operation={} candidates={count} snapshot={}",
            work.operation.as_str(),
            work.snapshot_id
        )],
    })
}

fn job_view() -> String {
    format!("marciana_{}", uuid::Uuid::new_v4().simple())
}

fn sail_stage_error(_: grust_core::prelude::GrustError) -> CognitionError {
    CognitionError::Sail("Sail stage failed")
}

fn sail_query_error(_: grust_core::prelude::GrustError) -> CognitionError {
    CognitionError::Sail("Sail query failed")
}

fn sail_cleanup_error(_: grust_core::prelude::GrustError) -> &'static str {
    "Sail cleanup failed"
}

fn sanitize_executor_error(error: CognitionError) -> SailCognitionExecutorError {
    match error {
        CognitionError::InvalidSnapshot(_)
        | CognitionError::InvalidJobId
        | CognitionError::BindingMismatch(_)
        | CognitionError::InvalidAlgorithm
        | CognitionError::ProjectionDenied => SailCognitionExecutorError::RequestRejected,
        CognitionError::ResourceBudgetExceeded(_) => {
            SailCognitionExecutorError::ResourceBudgetExceeded
        }
        CognitionError::SailCleanup(_) => SailCognitionExecutorError::CleanupFailed,
        CognitionError::SailCleanupAfterFailure { .. } => {
            SailCognitionExecutorError::ExecutionAndCleanupFailed
        }
        CognitionError::InvalidExecutorOutput
        | CognitionError::Serialization(_)
        | CognitionError::Sail(_) => SailCognitionExecutorError::ExecutionFailed,
    }
}

#[cfg(test)]
mod tests;
