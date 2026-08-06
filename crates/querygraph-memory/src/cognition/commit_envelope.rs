//! Canonical binding between a prepared TypeSec commit and durable graph evidence.

use serde::Serialize;
use typesec_memory::{CognitionCommitError, CognitionEffect};

use super::CognitionJob;
use super::commit_support::json_commit_digest;
use super::graph::{DurableOutcome, tagged_digest};

const RESULT_DOMAIN: &str = "querygraph.cognition.resulting-version.v1";
const COMPLETED_JOB_DOMAIN: &str = "querygraph.cognition.completed-job.v1";
const ENVELOPE_DOMAIN: &str = "querygraph.cognition.commit-envelope.v3";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitEnvelope<'a> {
    effect: CognitionEffect,
    proposal_digest: &'a str,
    prepared_digest: &'a str,
    prior_version: &'a str,
    resulting_version: &'a str,
    audit_node_id: &'a str,
    audit_digest: &'a str,
    completed_job_digest: &'a str,
    outbox_node_ids: &'a [String],
}

pub(super) fn resulting_version(
    effect: CognitionEffect,
    prior_version: &str,
    prepared_digest: &str,
) -> String {
    match effect {
        CognitionEffect::Mutated => tagged_digest(RESULT_DOMAIN, &[prepared_digest.as_bytes()]),
        CognitionEffect::NoChange => prior_version.to_owned(),
    }
}

pub(super) fn completed_job_digest(job: &CognitionJob) -> Result<String, CognitionCommitError> {
    json_commit_digest(COMPLETED_JOB_DOMAIN, job)
}

pub(super) fn envelope_digest(outcome: &DurableOutcome) -> Result<String, CognitionCommitError> {
    json_commit_digest(
        ENVELOPE_DOMAIN,
        &CommitEnvelope {
            effect: outcome.effect,
            proposal_digest: &outcome.proposal_digest,
            prepared_digest: &outcome.prepared_digest,
            prior_version: &outcome.prior_version,
            resulting_version: &outcome.resulting_version,
            audit_node_id: &outcome.audit_node_id,
            audit_digest: &outcome.audit_digest,
            completed_job_digest: &outcome.completed_job_digest,
            outbox_node_ids: &outcome.outbox_node_ids,
        },
    )
}
