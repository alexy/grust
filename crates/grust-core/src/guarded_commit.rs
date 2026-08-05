//! Durable, idempotent graph commit contracts.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{GraphMutation, GraphMutationStore, Node, NodeId, Result};

/// A database-state precondition for an atomic graph commit.
///
/// Exact-node expectations provide an optimistic concurrency guard over the
/// node's identity, label, and complete property map. Backends must evaluate
/// every expectation and apply every mutation in the same transaction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GraphExpectation {
    /// The node must not exist when the transaction is evaluated.
    Absent(NodeId),
    /// The stored node must exactly equal this previously read value.
    Exact(Node),
}

impl GraphExpectation {
    /// The node identity guarded by this expectation.
    pub fn node_id(&self) -> &NodeId {
        match self {
            Self::Absent(id) => id,
            Self::Exact(node) => &node.id,
        }
    }
}

/// An idempotent, optimistic-concurrency-guarded graph mutation batch.
///
/// `request_digest` is the caller's canonical digest of every security and
/// mutation input. Reusing an idempotency key with a different digest must
/// fail; replaying the same key and digest returns the original commit receipt
/// without evaluating guards or applying mutations again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuardedGraphCommit {
    pub idempotency_key: String,
    pub request_digest: String,
    pub expectations: Vec<GraphExpectation>,
    pub mutations: Vec<GraphMutation>,
}

impl GuardedGraphCommit {
    /// Build a guarded commit with no expectations yet.
    pub fn new(
        idempotency_key: impl Into<String>,
        request_digest: impl Into<String>,
        mutations: Vec<GraphMutation>,
    ) -> Self {
        Self {
            idempotency_key: idempotency_key.into(),
            request_digest: request_digest.into(),
            expectations: Vec::new(),
            mutations,
        }
    }

    /// Attach the database-state expectations evaluated before mutation.
    #[must_use]
    pub fn with_expectations(mut self, expectations: Vec<GraphExpectation>) -> Self {
        self.expectations = expectations;
        self
    }
}

/// Durable identity of a successfully committed guarded transaction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GraphCommitReceipt {
    /// Backend-issued identity written in the same transaction as the graph.
    pub commit_id: String,
    /// Backend clock timestamp persisted alongside `commit_id`.
    pub committed_at: String,
    /// Whether this call observed an already-committed idempotent request.
    pub replayed: bool,
}

/// Atomic graph commits with durable idempotency and optimistic guards.
///
/// This capability is deliberately separate from [`GraphMutationStore`]: a
/// backend must not advertise guarded commits unless it can evaluate all
/// expectations, apply all mutations, and persist the returned commit identity
/// in one durable transaction.
#[async_trait]
pub trait GraphCommitStore: GraphMutationStore {
    /// Apply `commit` once or return the receipt from an identical prior call.
    ///
    /// Implementations must reject empty idempotency keys and request digests,
    /// reject key reuse with a different request digest, and leave no mutation
    /// or idempotency record behind when an expectation or mutation fails.
    async fn commit_guarded(&self, commit: &GuardedGraphCommit) -> Result<GraphCommitReceipt>;
}
