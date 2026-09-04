//! Durable leased scheduler and proposal persistence over guarded graph CAS.
//!
//! These are storage-level primitives for an authenticated scheduler and
//! trusted worker pool; canonical owner strings and scoped job keys are stable
//! identities, not credentials. A worker may intentionally differ from the
//! submitter so expired work can be transferred. After acquisition, the
//! unpersisted lease token is the sole bearer credential for worker state
//! changes. Marciana's composition boundary must authenticate access to these
//! methods and must never treat knowledge of a job key as authorization.

use chrono::{DateTime, Duration, Utc};
use grust_core::prelude::GraphCommitStore;
use typesec_memory::{CognitionIdempotencyKey, CognitionProposal};

use super::bounds::{is_bounded_failure, is_canonical_text};
use super::graph::{
    authority_scope_matches, error_digest, is_sha256, job_digest, owner_digest, token_digest,
    validate_idempotency_key,
};
use super::invariants::{advance_attempt, advance_job, validate_transition_time};
use super::lease::{
    cancel_exhausted, lease_expiry, new_lease_token, validate_bearer_token, validate_lease,
};
use super::{
    CognitionJob, CognitionJobStatus, CognitionLease, CognitionLeaseState, CognitionStateError,
};
use crate::GraphStoreMemoryStore;

impl<G: GraphCommitStore> GraphStoreMemoryStore<G> {
    /// Submit a durable job for a verified TypeDID request envelope, or recover
    /// the identical existing submission.
    ///
    /// `owner` identifies the authenticated submitting scheduler. This storage
    /// method records its digest but does not authenticate the caller itself.
    pub fn submit_cognition_job(
        &self,
        key: &CognitionIdempotencyKey,
        owner: &str,
        typedid_request_digest: &str,
        max_attempts: u32,
        now: DateTime<Utc>,
    ) -> Result<CognitionJob, CognitionStateError> {
        validate_submission(key, owner, typedid_request_digest, max_attempts)?;
        if let Some((_, existing)) = self.load_job_node(key)? {
            if existing.typedid_request_digest != typedid_request_digest
                || existing.owner_digest != owner_digest(owner)
                || existing.max_attempts != max_attempts
            {
                return Err(CognitionStateError::DigestCollision);
            }
            return Ok(existing);
        }

        let job = CognitionJob {
            schema_version: super::state::JOB_SCHEMA_VERSION,
            job_digest: job_digest(key),
            owner_digest: owner_digest(owner),
            typedid_request_digest: typedid_request_digest.to_owned(),
            status: CognitionJobStatus::Pending,
            attempts: 0,
            max_attempts,
            revision: 0,
            lease: None,
            proposal_digest: None,
            completion_digest: None,
            last_error_digest: None,
            created_at: now,
            transitioned_at: now,
        };
        match self.create_job(key, &job) {
            Ok(()) => Ok(job),
            Err(CognitionStateError::ConcurrentModification) => {
                self.recover_submission(key, owner, typedid_request_digest, max_attempts)
            }
            Err(error) => Err(error),
        }
    }

    /// Load digest-safe durable job state.
    ///
    /// The scoped key is an address, not read authorization; the enclosing
    /// scheduler must authenticate and authorize access to operational state.
    pub fn cognition_job(
        &self,
        key: &CognitionIdempotencyKey,
    ) -> Result<Option<CognitionJob>, CognitionStateError> {
        validate_job_key(key)?;
        Ok(self.load_job_node(key)?.map(|(_, job)| job))
    }

    fn recover_submission(
        &self,
        key: &CognitionIdempotencyKey,
        owner: &str,
        typedid_request_digest: &str,
        max_attempts: u32,
    ) -> Result<CognitionJob, CognitionStateError> {
        let (_, existing) = self.load_job_node(key)?.ok_or_else(|| {
            CognitionStateError::Backend("job CAS conflicted without a job".into())
        })?;
        if existing.typedid_request_digest == typedid_request_digest
            && existing.owner_digest == owner_digest(owner)
            && existing.max_attempts == max_attempts
        {
            Ok(existing)
        } else {
            Err(CognitionStateError::DigestCollision)
        }
    }

    /// Acquire an exclusive lease with a fresh, unpersisted bearer token.
    ///
    /// `owner` identifies the authenticated worker and may deliberately differ
    /// from the submitting scheduler so expired work can move between workers.
    /// The enclosing service must authorize acquisition; the scoped key is not
    /// a credential.
    ///
    /// A lease is capped at one hour; long-running work must renew it.
    pub fn acquire_cognition_lease(
        &self,
        key: &CognitionIdempotencyKey,
        owner: &str,
        now: DateTime<Utc>,
        ttl: Duration,
    ) -> Result<CognitionLease, CognitionStateError> {
        validate_identity(key, owner)?;
        let expires_at = lease_expiry(now, ttl)?;
        let (node, mut job) = self
            .load_job_node(key)?
            .ok_or(CognitionStateError::NotFound)?;
        validate_transition_time(&job, now)?;
        if job.status.is_terminal() {
            return Err(CognitionStateError::Terminal);
        }
        if job
            .lease
            .as_ref()
            .is_some_and(|lease| lease.expires_at > now)
        {
            return Err(CognitionStateError::LeaseHeld);
        }
        if job.attempts >= job.max_attempts {
            cancel_exhausted(&mut job, now)?;
            self.transition_job(key, node, &job)?;
            return Err(CognitionStateError::AttemptsExhausted);
        }

        advance_attempt(&mut job)?;
        advance_job(&mut job, now)?;
        job.status = CognitionJobStatus::Leased;
        let token = new_lease_token();
        job.lease = Some(CognitionLeaseState {
            owner_digest: owner_digest(owner),
            token_digest: token_digest(&token),
            acquired_at: now,
            expires_at,
        });
        self.transition_job(key, node, &job)?;
        Ok(CognitionLease::new(
            job.job_digest.clone(),
            token,
            job.attempts,
            expires_at,
        ))
    }

    /// Extend an active lease without changing its bearer token, for at most
    /// one hour from `now`.
    ///
    /// Possession of the unexpired token is the worker credential; owner text
    /// is intentionally not accepted again or used as substitute authority.
    pub fn renew_cognition_lease(
        &self,
        key: &CognitionIdempotencyKey,
        token: &str,
        now: DateTime<Utc>,
        ttl: Duration,
    ) -> Result<CognitionLease, CognitionStateError> {
        validate_job_key(key)?;
        validate_bearer_token(token)?;
        let expires_at = lease_expiry(now, ttl)?;
        let (node, mut job) = self
            .load_job_node(key)?
            .ok_or(CognitionStateError::NotFound)?;
        validate_transition_time(&job, now)?;
        validate_lease(&job, token, now)?;
        let current_expiry = job
            .lease
            .as_ref()
            .ok_or(CognitionStateError::StaleLease)?
            .expires_at;
        if expires_at <= current_expiry {
            return Err(CognitionStateError::InvalidTransition(
                "lease renewal must extend the current expiry".into(),
            ));
        }
        let lease = job.lease.as_mut().ok_or(CognitionStateError::StaleLease)?;
        lease.expires_at = expires_at;
        advance_job(&mut job, now)?;
        self.transition_job(key, node, &job)?;
        Ok(CognitionLease::new(
            job.job_digest.clone(),
            token.to_owned(),
            job.attempts,
            expires_at,
        ))
    }

    /// Persist only a proposal's canonical identity and mark its job ready.
    ///
    /// Proposal drafts and evidence stay transient; cognition metadata never
    /// serializes their potentially protected content. The lease governs this
    /// worker staging step. Once staged, expiry does not authorize or forbid a
    /// mutation: only a freshly vault-authorized opaque TypeSec prepared commit
    /// can apply the proposal.
    ///
    /// This transition is bearer-authorized by the active lease token.
    pub fn persist_cognition_proposal(
        &self,
        key: &CognitionIdempotencyKey,
        token: &str,
        proposal: &CognitionProposal,
        now: DateTime<Utc>,
    ) -> Result<String, CognitionStateError> {
        validate_job_key(key)?;
        validate_bearer_token(token)?;
        let Some(binding) = proposal.binding.as_ref() else {
            return Err(CognitionStateError::Invalid(
                "proposal identity does not match scheduler job".into(),
            ));
        };
        if proposal.job_id != key.job_id()
            || binding.space_id != key.space_id()
            || !authority_scope_matches(key, &binding.subject, &binding.purpose)
        {
            return Err(CognitionStateError::Invalid(
                "proposal identity does not match scheduler job".into(),
            ));
        }
        let digest = proposal
            .canonical_digest()
            .map_err(|_| CognitionStateError::Serialization("proposal digest failed".into()))?;
        let (job_node, mut job) = self
            .load_job_node(key)?
            .ok_or(CognitionStateError::NotFound)?;
        validate_transition_time(&job, now)?;
        validate_lease(&job, token, now)?;
        if job.typedid_request_digest != binding.typedid_request_digest {
            return Err(CognitionStateError::DigestCollision);
        }

        if let Some(stored_digest) = job.proposal_digest.as_deref() {
            if stored_digest != digest {
                return Err(CognitionStateError::DigestCollision);
            }
            if job.status == CognitionJobStatus::ProposalReady {
                return Ok(digest);
            }
        }

        job.status = CognitionJobStatus::ProposalReady;
        job.proposal_digest = Some(digest.clone());
        advance_job(&mut job, now)?;
        self.transition_job(key, job_node, &job)?;
        Ok(digest)
    }

    /// Record a worker failure without persisting its plaintext.
    ///
    /// This transition is bearer-authorized by the active lease token.
    pub fn fail_cognition_job(
        &self,
        key: &CognitionIdempotencyKey,
        token: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<CognitionJob, CognitionStateError> {
        validate_job_key(key)?;
        validate_bearer_token(token)?;
        if !is_bounded_failure(error) {
            return Err(CognitionStateError::Invalid(
                "failure input exceeds its fixed bound or is empty".into(),
            ));
        }
        let (node, mut job) = self
            .load_job_node(key)?
            .ok_or(CognitionStateError::NotFound)?;
        validate_transition_time(&job, now)?;
        validate_lease(&job, token, now)?;
        job.status = if job.attempts >= job.max_attempts {
            CognitionJobStatus::Cancelled
        } else {
            CognitionJobStatus::Failed
        };
        job.last_error_digest = Some(error_digest(error));
        job.lease = None;
        advance_job(&mut job, now)?;
        self.transition_job(key, node, &job)?;
        Ok(job)
    }

    /// Cancel a non-terminal job using the submitting owner's identity.
    ///
    /// Matching the stored digest prevents cross-owner mistakes but does not
    /// authenticate raw owner text; the enclosing scheduler must authenticate
    /// and authorize cancellation before calling this storage primitive.
    pub fn cancel_cognition_job(
        &self,
        key: &CognitionIdempotencyKey,
        owner: &str,
        now: DateTime<Utc>,
    ) -> Result<CognitionJob, CognitionStateError> {
        validate_identity(key, owner)?;
        let (node, mut job) = self
            .load_job_node(key)?
            .ok_or(CognitionStateError::NotFound)?;
        validate_transition_time(&job, now)?;
        if job.owner_digest != owner_digest(owner) {
            return Err(CognitionStateError::StaleLease);
        }
        if job.status.is_terminal() {
            return Err(CognitionStateError::Terminal);
        }
        job.status = CognitionJobStatus::Cancelled;
        job.lease = None;
        advance_job(&mut job, now)?;
        self.transition_job(key, node, &job)?;
        Ok(job)
    }
}

fn validate_identity(
    key: &CognitionIdempotencyKey,
    owner: &str,
) -> Result<(), CognitionStateError> {
    validate_job_key(key)?;
    if !is_canonical_text(owner) {
        return Err(CognitionStateError::Invalid(
            "owner must be canonical text".into(),
        ));
    }
    Ok(())
}

fn validate_job_key(key: &CognitionIdempotencyKey) -> Result<(), CognitionStateError> {
    validate_idempotency_key(key).map_err(|message| CognitionStateError::Invalid(message.into()))
}

fn validate_submission(
    key: &CognitionIdempotencyKey,
    owner: &str,
    typedid_request_digest: &str,
    max_attempts: u32,
) -> Result<(), CognitionStateError> {
    validate_identity(key, owner)?;
    if !is_sha256(typedid_request_digest) || max_attempts == 0 {
        return Err(CognitionStateError::Invalid(
            "TypeDID request digest must be canonical SHA-256 and max attempts must be positive"
                .into(),
        ));
    }
    Ok(())
}
