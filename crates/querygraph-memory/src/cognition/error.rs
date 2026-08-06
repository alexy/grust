//! Fixed, non-sensitive cognition failure categories.

/// Cognition failure.
#[derive(Debug, thiserror::Error)]
pub enum CognitionError {
    /// A governed proof field was absent.
    #[error("invalid governed LakeCat snapshot field: {0}")]
    InvalidSnapshot(&'static str),
    /// The durable job identity was not bounded canonical text.
    #[error("cognition job id is not canonical")]
    InvalidJobId,
    /// Proof serialization failed under a fixed, non-sensitive category.
    #[error("cognition proof serialization failed: {0}")]
    Serialization(&'static str),
    /// A TypeSec binding or source manifest did not match the governed request.
    #[error("cognition authority binding mismatch: {0}")]
    BindingMismatch(&'static str),
    /// An engine profile did not identify a native algorithm and version.
    #[error("cognition algorithm identity is not native")]
    InvalidAlgorithm,
    /// An executor returned a plan or evidence that cannot form a canonical TypeSec proposal.
    #[error("cognition executor returned invalid proposal output")]
    InvalidExecutorOutput,
    /// Sail would stage a field omitted by the authorized projection.
    #[error("cognition field is not in the authorized projection")]
    ProjectionDenied,
    /// Sail failed.
    #[error("Sail cognition failed: {0}")]
    Sail(&'static str),
    /// Bounded cognition input or output exceeded a fixed local limit.
    #[error("cognition resource budget exceeded: {0}")]
    ResourceBudgetExceeded(&'static str),
    /// Sail completed the work but could not remove its protected temp view.
    #[error("Sail cognition temp-view cleanup failed: {0}")]
    SailCleanup(&'static str),
    /// Sail work failed and cleanup independently failed afterward.
    #[error("{primary}; Sail cognition temp-view cleanup also failed: {cleanup}")]
    SailCleanupAfterFailure {
        /// Original execution failure, preserved for typed inspection.
        #[source]
        primary: Box<CognitionError>,
        /// Cleanup failure, which never contains staged row data.
        cleanup: &'static str,
    },
}

/// Text-free failures accepted from a Sail cognition executor.
///
/// Executor and backend strings cannot cross this boundary. Implementations
/// select only a closed failure category; the engine converts it to a fixed
/// public [`CognitionError`].
///
/// ```compile_fail
/// # use querygraph_memory::cognition::SailCognitionExecutorError;
/// fn expose_backend_text(message: String) -> SailCognitionExecutorError {
///     message.into()
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SailCognitionExecutorError {
    /// The governed request was rejected before execution.
    #[error("Sail cognition executor rejected the governed request")]
    RequestRejected,
    /// The executor exceeded a fixed local resource budget.
    #[error("Sail cognition executor exceeded a resource budget")]
    ResourceBudgetExceeded,
    /// Staging, querying, decoding, validation, or worker execution failed.
    #[error("Sail cognition executor failed")]
    ExecutionFailed,
    /// Execution completed, but protected temporary state could not be removed.
    #[error("Sail cognition executor cleanup failed")]
    CleanupFailed,
    /// Execution and its subsequent cleanup failed independently.
    #[error("Sail cognition executor and cleanup failed")]
    ExecutionAndCleanupFailed,
}

pub(super) fn cognition_executor_error(error: SailCognitionExecutorError) -> CognitionError {
    match error {
        SailCognitionExecutorError::RequestRejected => {
            CognitionError::Sail("Sail executor rejected the governed request")
        }
        SailCognitionExecutorError::ResourceBudgetExceeded => {
            CognitionError::ResourceBudgetExceeded("Sail executor")
        }
        SailCognitionExecutorError::ExecutionFailed => CognitionError::Sail("Sail executor failed"),
        SailCognitionExecutorError::CleanupFailed => {
            CognitionError::SailCleanup("Sail executor cleanup failed")
        }
        SailCognitionExecutorError::ExecutionAndCleanupFailed => {
            CognitionError::SailCleanupAfterFailure {
                primary: Box::new(CognitionError::Sail("Sail executor failed")),
                cleanup: "Sail executor cleanup failed",
            }
        }
    }
}
