//! Shared error and digest plumbing for authoritative cognition commits.

use grust_core::prelude::{GraphCommitStore, GrustError};
use typesec_memory::{CognitionCommitError, StoreError};

use super::CognitionStateError;
use super::graph::json_digest;
use crate::GraphStoreMemoryStore;

pub(super) const AUDIT_DOMAIN: &str = "querygraph.cognition.audit.v3";

impl<G: GraphCommitStore> GraphStoreMemoryStore<G> {
    pub(super) fn run_commit<T: Send>(
        &self,
        future: impl std::future::Future<Output = grust_core::prelude::Result<T>> + Send,
    ) -> Result<T, CognitionCommitError> {
        self.bridge.run(future).map_err(map_graph_error)
    }
}

pub(super) fn json_commit_digest<T: serde::Serialize + ?Sized>(
    domain: &str,
    value: &T,
) -> Result<String, CognitionCommitError> {
    json_digest(domain, value).map_err(state_store_error)
}

pub(super) fn map_graph_error(error: GrustError) -> CognitionCommitError {
    match error {
        GrustError::GraphIdempotencyConflict(_) => CognitionCommitError::IdempotencyConflict,
        _ => store_error("cognition commit backend operation failed"),
    }
}

pub(super) fn state_store_error(_: CognitionStateError) -> CognitionCommitError {
    store_error("cognition durable state validation failed")
}

pub(super) fn store_error(message: impl Into<String>) -> CognitionCommitError {
    CognitionCommitError::Store(StoreError::Backend(message.into()))
}
