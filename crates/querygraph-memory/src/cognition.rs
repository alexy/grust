//! QueryGraph-native cognition over governed LakeCat snapshots and Sail.
//!
//! LakeCat proves the source, Sail performs bounded batch work, and this
//! module emits an inert TypeSec proposal. Only `MemoryVault` applies it.

mod backend;
mod bounds;
mod budget;
mod commit;
mod commit_envelope;
mod commit_outcome;
mod commit_sources;
mod commit_support;
mod commit_validation;
mod engine;
mod engine_validation;
mod error;
mod graph;
mod invariants;
mod lease;
mod operation;
mod outbox;
mod profile;
#[cfg(feature = "sail")]
mod sail;
mod snapshot;
mod state;
mod store;

pub use bounds::{
    MAX_COGNITION_BEARER_TOKEN_BYTES, MAX_COGNITION_FAILURE_BYTES, MAX_COGNITION_IDENTITY_BYTES,
    MAX_COGNITION_PROJECTION_BYTES, MAX_COGNITION_PROJECTION_FIELDS,
};
pub use engine::{
    CognitionEngine, CognitionRequest, ReferenceCognitionEngine, SailCognitionEngine,
    SailCognitionExecutor, SailCognitionOutput,
};
pub use error::{CognitionError, SailCognitionExecutorError};
pub use operation::{
    CognitionOperation, DEDUPLICATE_ALGORITHM_SPEC_VERSION, ParseCognitionOperationError,
    RECONCILE_ALGORITHM_SPEC_VERSION,
};
pub use outbox::{CognitionOutboxClaim, MAX_COGNITION_OUTBOX_CLAIM, MAX_COGNITION_OUTBOX_ENTRIES};
pub use profile::CognitionEngineProfile;
#[cfg(feature = "sail")]
pub use sail::LiveSailCognitionExecutor;
pub use snapshot::{CognitionFieldMapping, GovernedLakeCatSnapshot};
pub use state::{
    CognitionJob, CognitionJobStatus, CognitionLease, CognitionLeaseState, CognitionStateError,
};
pub use typesec_memory::{
    MAX_COGNITION_SOURCE_BYTES as MAX_COGNITION_AUTHORIZED_INPUT_BYTES, MAX_COGNITION_SOURCE_BYTES,
    MAX_COGNITION_SOURCE_COUNT,
};
