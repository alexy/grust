//! Validation and checked state advancement for durable cognition jobs.

use chrono::{DateTime, Utc};
use typesec_memory::CognitionIdempotencyKey;

use super::graph::{is_sha256, job_digest};
use super::state::JOB_SCHEMA_VERSION;
use super::{CognitionJob, CognitionJobStatus, CognitionStateError};

pub(super) fn validate_persisted_job(
    key: &CognitionIdempotencyKey,
    job: &CognitionJob,
) -> Result<(), CognitionStateError> {
    if job.schema_version != JOB_SCHEMA_VERSION {
        return Err(corrupt("unsupported persisted job schema version"));
    }
    if job.job_digest != job_digest(key) {
        return Err(corrupt(
            "job digest does not match its addressed scoped job identity",
        ));
    }
    for (name, digest) in [
        ("job", Some(job.job_digest.as_str())),
        ("owner", Some(job.owner_digest.as_str())),
        ("TypeDID request", Some(job.typedid_request_digest.as_str())),
        ("proposal", job.proposal_digest.as_deref()),
        ("completion", job.completion_digest.as_deref()),
        ("last error", job.last_error_digest.as_deref()),
    ] {
        if digest.is_some_and(|value| !is_sha256(value)) {
            return Err(corrupt(format!("{name} digest is not canonical SHA-256")));
        }
    }
    if job.max_attempts == 0 || job.attempts > job.max_attempts {
        return Err(corrupt("attempt counters violate the configured budget"));
    }
    if job.created_at > job.transitioned_at {
        return Err(corrupt("transition timestamp predates creation"));
    }

    let expects_lease = matches!(
        job.status,
        CognitionJobStatus::Leased | CognitionJobStatus::ProposalReady
    );
    if expects_lease != job.lease.is_some() {
        return Err(corrupt("job status and lease presence disagree"));
    }
    if expects_lease && job.attempts == 0 {
        return Err(corrupt("leased job has no issued attempt"));
    }
    if let Some(lease) = &job.lease {
        if !is_sha256(&lease.owner_digest) || !is_sha256(&lease.token_digest) {
            return Err(corrupt("lease identity digest is not canonical SHA-256"));
        }
        if lease.acquired_at > job.transitioned_at
            || lease.acquired_at >= lease.expires_at
            || job.transitioned_at >= lease.expires_at
        {
            return Err(corrupt("lease timestamps are inconsistent"));
        }
    }
    if job.status == CognitionJobStatus::ProposalReady && job.proposal_digest.is_none() {
        return Err(corrupt("proposal-ready job has no proposal digest"));
    }
    if (job.status == CognitionJobStatus::Completed) != job.completion_digest.is_some() {
        return Err(corrupt("completion status and digest disagree"));
    }
    Ok(())
}

pub(super) fn validate_transition_time(
    job: &CognitionJob,
    now: DateTime<Utc>,
) -> Result<(), CognitionStateError> {
    if now < job.transitioned_at {
        return Err(CognitionStateError::InvalidTransition(
            "transition timestamp predates durable job state".into(),
        ));
    }
    Ok(())
}

pub(super) fn advance_job(
    job: &mut CognitionJob,
    now: DateTime<Utc>,
) -> Result<(), CognitionStateError> {
    validate_transition_time(job, now)?;
    job.revision = job
        .revision
        .checked_add(1)
        .ok_or_else(|| CognitionStateError::InvalidTransition("job revision overflow".into()))?;
    job.transitioned_at = now;
    Ok(())
}

pub(super) fn advance_attempt(job: &mut CognitionJob) -> Result<(), CognitionStateError> {
    job.attempts = job.attempts.checked_add(1).ok_or_else(|| {
        CognitionStateError::InvalidTransition("job attempt counter overflow".into())
    })?;
    Ok(())
}

fn corrupt(message: impl Into<String>) -> CognitionStateError {
    CognitionStateError::Backend(format!(
        "invalid persisted cognition job: {}",
        message.into()
    ))
}
