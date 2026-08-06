//! Graph encoding shared by cognition state and authoritative commits.

use std::collections::BTreeMap;

use grust_core::prelude::{Node, NodeId, Value};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use typesec_memory::{
    CognitionAuditEvidence, CognitionEffect, CognitionIdempotencyKey, IndexMutation,
};

use super::outbox::CognitionOutboxRecord;
use super::{CognitionJob, CognitionStateError};

pub(super) const JOB_LABEL: &str = "CognitionJob";
pub(super) const OUTBOX_LABEL: &str = "CognitionIndexOutbox";
pub(super) const AUDIT_LABEL: &str = "CognitionAudit";
pub(super) const OUTCOME_LABEL: &str = "CognitionOutcome";
pub(super) const OUTCOME_SCHEMA_VERSION: u32 = 3;

const PAYLOAD: &str = "payload";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DurableOutcome {
    pub schema_version: u32,
    pub effect: CognitionEffect,
    pub proposal_digest: String,
    pub prepared_digest: String,
    pub prior_version: String,
    pub resulting_version: String,
    pub audit_node_id: String,
    pub audit_digest: String,
    pub completed_job_digest: String,
    pub outbox_node_ids: Vec<String>,
    pub envelope_digest: String,
}

pub(super) fn tagged_digest(domain: &str, values: &[&[u8]]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain.as_bytes());
    digest.update([0]);
    for value in values {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value);
    }
    format!("sha256:{:x}", digest.finalize())
}

pub(super) fn is_sha256(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn validate_idempotency_key(key: &CognitionIdempotencyKey) -> Result<(), &'static str> {
    key.validate()
        .map_err(|_| "invalid cognition idempotency key")
}

pub(super) fn json_digest<T: Serialize + ?Sized>(
    domain: &str,
    value: &T,
) -> Result<String, CognitionStateError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| CognitionStateError::Serialization("canonical JSON encoding failed".into()))?;
    Ok(tagged_digest(domain, &[&bytes]))
}

pub(super) fn job_digest(key: &CognitionIdempotencyKey) -> String {
    tagged_digest(
        "querygraph.cognition.job.v1",
        &[
            key.space_id().as_bytes(),
            key.authority_scope_digest().as_bytes(),
            key.job_id().as_bytes(),
        ],
    )
}

pub(super) fn owner_digest(owner: &str) -> String {
    tagged_digest("querygraph.cognition.owner.v1", &[owner.as_bytes()])
}

pub(super) fn token_digest(token: &str) -> String {
    tagged_digest("querygraph.cognition.lease-token.v1", &[token.as_bytes()])
}

pub(super) fn error_digest(error: &str) -> String {
    tagged_digest("querygraph.cognition.failure.v1", &[error.as_bytes()])
}

pub(super) fn job_node_id(key: &CognitionIdempotencyKey) -> NodeId {
    NodeId::from(format!("cog-job:{}", digest_suffix(&job_digest(key))).as_str())
}

pub(super) fn outcome_node_id(key: &CognitionIdempotencyKey) -> NodeId {
    let digest = commit_key_digest(key);
    NodeId::from(format!("cog-outcome:{}", digest_suffix(&digest)).as_str())
}

pub(super) fn audit_node_id(key: &CognitionIdempotencyKey) -> NodeId {
    let digest = commit_key_digest(key);
    NodeId::from(format!("cog-audit:{}", digest_suffix(&digest)).as_str())
}

pub(super) fn commit_ledger_key(key: &CognitionIdempotencyKey) -> String {
    format!(
        "cognition-commit:{}",
        digest_suffix(&commit_key_digest(key))
    )
}

pub(super) fn transition_ledger_key(job: &CognitionJob) -> String {
    format!(
        "cognition-state:{}:{}",
        digest_suffix(&job.job_digest),
        job.revision
    )
}

pub(super) fn commit_key_digest(key: &CognitionIdempotencyKey) -> String {
    tagged_digest(
        "querygraph.cognition.commit-key.v1",
        &[
            key.space_id().as_bytes(),
            key.authority_scope_digest().as_bytes(),
            key.job_id().as_bytes(),
        ],
    )
}

pub(super) fn authority_scope_matches(
    key: &CognitionIdempotencyKey,
    subject: &str,
    purpose: &str,
) -> bool {
    CognitionIdempotencyKey::for_authority(key.space_id(), subject, purpose, key.job_id())
        .is_ok_and(|expected| expected.authority_scope_digest() == key.authority_scope_digest())
}

fn digest_suffix(digest: &str) -> &str {
    digest.strip_prefix("sha256:").unwrap_or(digest)
}

pub(super) fn payload_node<T: Serialize>(
    label: &str,
    id: NodeId,
    value: &T,
) -> Result<Node, CognitionStateError> {
    let payload = serde_json::to_value(value)
        .map_err(|_| CognitionStateError::Serialization("payload encoding failed".into()))?;
    Ok(Node::new(
        label,
        id,
        BTreeMap::from([(PAYLOAD.to_owned(), Value::Json(payload))]),
    ))
}

pub(super) fn decode_versioned_payload<T: DeserializeOwned>(
    node: &Node,
    expected_label: &str,
    expected_version: u32,
    unsupported_schema: &'static str,
) -> Result<T, CognitionStateError> {
    let payload = payload_json(node, expected_label)?;
    if payload
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(expected_version))
    {
        return Err(CognitionStateError::Backend(unsupported_schema.into()));
    }
    serde_json::from_value(payload.clone())
        .map_err(|_| CognitionStateError::Serialization("persisted payload is invalid".into()))
}

pub(super) fn payload_json<'a>(
    node: &'a Node,
    expected_label: &str,
) -> Result<&'a serde_json::Value, CognitionStateError> {
    if node.label.as_str() != expected_label {
        return Err(CognitionStateError::Backend(
            "persisted cognition node has an unexpected label".into(),
        ));
    }
    let Value::Json(payload) = node.props.get(PAYLOAD).ok_or_else(|| {
        CognitionStateError::Backend("persisted cognition node has no payload".into())
    })?
    else {
        return Err(CognitionStateError::Backend(
            "persisted cognition payload is not JSON".into(),
        ));
    };
    Ok(payload)
}

pub(super) fn encode_job(
    key: &CognitionIdempotencyKey,
    job: &CognitionJob,
) -> Result<Node, CognitionStateError> {
    payload_node(JOB_LABEL, job_node_id(key), job)
}

pub(super) fn encode_outbox(
    key: &CognitionIdempotencyKey,
    ordinal: usize,
    mutation: &IndexMutation,
) -> Result<Node, CognitionStateError> {
    let ordinal = u64::try_from(ordinal)
        .map_err(|_| CognitionStateError::Serialization("outbox ordinal exceeds u64".into()))?;
    let commit_digest = commit_key_digest(key);
    let id = outbox_node_id(&commit_digest, ordinal, mutation)?;
    payload_node(
        OUTBOX_LABEL,
        id,
        &CognitionOutboxRecord::pending(commit_digest, ordinal, mutation.clone()),
    )
}

pub(super) fn outbox_node_id(
    commit_digest: &str,
    ordinal: u64,
    mutation: &IndexMutation,
) -> Result<NodeId, CognitionStateError> {
    let encoded = serde_json::to_vec(mutation)
        .map_err(|_| CognitionStateError::Serialization("outbox encoding failed".into()))?;
    let ordinal_bytes = ordinal.to_be_bytes();
    let digest = tagged_digest(
        "querygraph.cognition.outbox.v1",
        &[commit_digest.as_bytes(), &ordinal_bytes, &encoded],
    );
    Ok(NodeId::from(
        format!("cog-outbox:{}", digest_suffix(&digest)).as_str(),
    ))
}

pub(super) fn encode_audit(
    key: &CognitionIdempotencyKey,
    audit: &CognitionAuditEvidence,
) -> Result<Node, CognitionStateError> {
    payload_node(AUDIT_LABEL, audit_node_id(key), audit)
}

pub(super) fn encode_outcome(
    key: &CognitionIdempotencyKey,
    outcome: &DurableOutcome,
) -> Result<Node, CognitionStateError> {
    payload_node(OUTCOME_LABEL, outcome_node_id(key), outcome)
}

#[cfg(test)]
mod tests;
