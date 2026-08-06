//! Lease token generation, expiry, and validation.

use chrono::{DateTime, Duration, Utc};

use super::CognitionStateError;
use super::bounds::is_canonical_bearer_token;
use super::graph::token_digest;
use super::invariants::advance_job;
use super::{CognitionJob, CognitionJobStatus};

const MAX_LEASE_TTL_SECONDS: i64 = 60 * 60;

pub(super) fn new_lease_token() -> String {
    format!("lease:{}", uuid::Uuid::new_v4())
}

pub(super) fn validate_lease(
    job: &CognitionJob,
    token: &str,
    now: DateTime<Utc>,
) -> Result<(), CognitionStateError> {
    validate_bearer_token(token)?;
    if job.status.is_terminal() {
        return Err(CognitionStateError::Terminal);
    }
    if !matches!(
        job.status,
        CognitionJobStatus::Leased | CognitionJobStatus::ProposalReady
    ) {
        return Err(CognitionStateError::InvalidTransition(format!(
            "status {:?} cannot use a worker lease",
            job.status
        )));
    }
    let lease = job.lease.as_ref().ok_or(CognitionStateError::StaleLease)?;
    if lease.expires_at <= now || lease.token_digest != token_digest(token) {
        return Err(CognitionStateError::StaleLease);
    }
    Ok(())
}

pub(super) fn validate_bearer_token(token: &str) -> Result<(), CognitionStateError> {
    if !is_canonical_bearer_token(token) {
        return Err(CognitionStateError::Invalid(
            "lease token is not bounded canonical text".into(),
        ));
    }
    Ok(())
}

pub(super) fn cancel_exhausted(
    job: &mut CognitionJob,
    now: DateTime<Utc>,
) -> Result<(), CognitionStateError> {
    advance_job(job, now)?;
    job.status = CognitionJobStatus::Cancelled;
    job.lease = None;
    Ok(())
}

pub(super) fn lease_expiry(
    now: DateTime<Utc>,
    ttl: Duration,
) -> Result<DateTime<Utc>, CognitionStateError> {
    if ttl <= Duration::zero() {
        return Err(CognitionStateError::Invalid(
            "lease duration must be positive".into(),
        ));
    }
    if ttl > Duration::seconds(MAX_LEASE_TTL_SECONDS) {
        return Err(CognitionStateError::Invalid(
            "lease duration must not exceed one hour; renew long-running work".into(),
        ));
    }
    now.checked_add_signed(ttl)
        .ok_or_else(|| CognitionStateError::Invalid("lease expiry overflows".into()))
}
