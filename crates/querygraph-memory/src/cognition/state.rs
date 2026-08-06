//! Durable, plaintext-free scheduler state for cognition work.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub(super) const JOB_SCHEMA_VERSION: u32 = 2;

/// Scheduler lifecycle persisted in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CognitionJobStatus {
    /// Ready for a worker.
    Pending,
    /// Exclusively owned until the lease expires.
    Leased,
    /// A worker failed and the bounded retry budget is not exhausted.
    Failed,
    /// A canonical proposal digest was durably staged for governed application.
    ProposalReady,
    /// The memory transaction committed.
    Completed,
    /// Explicitly cancelled or out of attempts.
    Cancelled,
}

impl CognitionJobStatus {
    pub(crate) fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

/// Hashed lease state stored with a [`CognitionJob`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitionLeaseState {
    /// Domain-separated worker identity digest.
    pub owner_digest: String,
    /// Digest of the bearer token returned to the worker.
    pub token_digest: String,
    /// Lease start used for audit-safe scheduling diagnostics.
    pub acquired_at: DateTime<Utc>,
    /// Exclusive ownership deadline.
    pub expires_at: DateTime<Utc>,
}

/// Durable cognition scheduler record.
///
/// The raw space id, authority subject and purpose, job id, worker identity,
/// bearer token, and failure text are never serialized. Callers retain those
/// values and use their scoped digests to address this record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitionJob {
    /// Exact persisted schema understood by this adapter.
    pub(crate) schema_version: u32,
    /// Domain-separated digest of the external space, authority scope, and job ids.
    pub job_digest: String,
    /// Domain-separated digest of the scheduler/tenant that submitted it.
    pub owner_digest: String,
    /// Exact canonical digest of the immutable, verified TypeDID request envelope.
    pub typedid_request_digest: String,
    /// Current lifecycle state.
    pub status: CognitionJobStatus,
    /// Number of leases issued, including expired leases.
    pub attempts: u32,
    /// Hard retry ceiling.
    pub max_attempts: u32,
    /// Optimistic-CAS revision.
    pub revision: u64,
    /// Current lease, if any. Only digests are stored.
    pub lease: Option<CognitionLeaseState>,
    /// Canonical proposal digest after staging.
    pub proposal_digest: Option<String>,
    /// Canonical digest of the exact TypeSec-prepared terminal decision.
    pub completion_digest: Option<String>,
    /// Digest of the most recent failure, never its plaintext.
    pub last_error_digest: Option<String>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Caller-supplied logical timestamp for the latest transition.
    ///
    /// A completed job uses the TypeSec audit's `prepared_at`. The authoritative
    /// backend commit time exists only in the committed outcome and receipt.
    pub transitioned_at: DateTime<Utc>,
}

/// Bearer lease returned to a cognition worker.
///
/// `token` is never persisted directly and must be presented for renewal,
/// proposal staging, or failure.
pub struct CognitionLease {
    /// Digest-safe durable job identity.
    job_digest: String,
    /// Bearer token.
    token: String,
    /// One-based attempt number.
    attempt: u32,
    /// Exclusive ownership deadline.
    expires_at: DateTime<Utc>,
}

impl CognitionLease {
    pub(super) fn new(
        job_digest: String,
        token: String,
        attempt: u32,
        expires_at: DateTime<Utc>,
    ) -> Self {
        Self {
            job_digest,
            token,
            attempt,
            expires_at,
        }
    }

    /// Borrow the digest-safe job identity.
    pub fn job_digest(&self) -> &str {
        &self.job_digest
    }

    /// Borrow the bearer token for the next scheduler operation.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Return the one-based delivery attempt.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Return the exclusive ownership deadline.
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }
}

/// Durable scheduler operation failure.
#[derive(Debug, thiserror::Error)]
pub enum CognitionStateError {
    /// A caller supplied an empty or invalid argument.
    #[error("invalid cognition job input: {0}")]
    Invalid(String),
    /// No matching job exists.
    #[error("cognition job was not found")]
    NotFound,
    /// The same scoped external job identity was reused for different immutable input.
    #[error("cognition job digest collision")]
    DigestCollision,
    /// An optimistic state transition lost a concurrent exact-node CAS.
    #[error("cognition job changed concurrently")]
    ConcurrentModification,
    /// Another worker holds an unexpired lease.
    #[error("cognition job has an active lease")]
    LeaseHeld,
    /// The supplied token is absent, stale, or expired.
    #[error("cognition lease is stale")]
    StaleLease,
    /// The job is already terminal.
    #[error("cognition job is terminal")]
    Terminal,
    /// A valid credential was presented for a transition the current state forbids.
    #[error("invalid cognition job transition: {0}")]
    InvalidTransition(String),
    /// The bounded attempt budget is exhausted.
    #[error("cognition job exhausted its attempt budget")]
    AttemptsExhausted,
    /// Proposal canonicalization failed.
    #[error("cognition proposal serialization failed: {0}")]
    Serialization(String),
    /// The authoritative graph rejected or could not persist a transition.
    #[error("cognition state backend failed: {0}")]
    Backend(String),
}
