//! Durable cognition outcome recovery and consistency validation.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use grust_core::prelude::{GraphCommitReceipt, GraphCommitStore, Node, RfcDate};
use typesec_memory::{
    CognitionAuditEvidence, CognitionCommitError, CognitionCommitOutcome, CognitionCommitStatus,
    CognitionEffect, CognitionIdempotencyKey, MAX_COGNITION_MUTATIONS, MAX_COGNITION_SOURCE_BYTES,
};

use super::CognitionJobStatus;
use super::bounds::is_canonical_text;
use super::commit_envelope::{completed_job_digest, envelope_digest, resulting_version};
use super::commit_support::{AUDIT_DOMAIN, json_commit_digest, state_store_error, store_error};
use super::graph::{
    AUDIT_LABEL, DurableOutcome, OUTCOME_LABEL, OUTCOME_SCHEMA_VERSION, audit_node_id,
    commit_ledger_key, is_sha256, outcome_node_id, payload_json, tagged_digest,
    validate_idempotency_key,
};
use crate::GraphStoreMemoryStore;

const OUTBOX_NODE_ID_BYTES: usize = "cog-outbox:".len() + 64;
const MAX_OUTBOX_MANIFEST_BYTES: usize =
    super::outbox::MAX_COGNITION_OUTBOX_ENTRIES * OUTBOX_NODE_ID_BYTES;

pub(super) fn recover<G: GraphCommitStore>(
    store: &GraphStoreMemoryStore<G>,
    key: &CognitionIdempotencyKey,
    proposal_digest: &str,
) -> Result<Option<CognitionCommitOutcome>, CognitionCommitError> {
    validate_idempotency_key(key).map_err(store_error)?;
    if !is_sha256(proposal_digest) {
        return Err(store_error("invalid cognition proposal digest"));
    }
    let outcome: Option<DurableOutcome> = store
        .run_commit(store.graph().get_node(&outcome_node_id(key)))?
        .map(|node| decode_durable_outcome(&node))
        .transpose()?;
    let Some(outcome) = outcome else {
        return Ok(None);
    };
    validate_durable_outcome(key, &outcome)?;
    if outcome.proposal_digest != proposal_digest {
        return Err(CognitionCommitError::IdempotencyConflict);
    }

    let audit_node = store
        .run_commit(store.graph().get_node(&audit_node_id(key)))?
        .ok_or_else(|| store_error("committed cognition outcome has no audit evidence"))?;
    if audit_node.id.as_str() != outcome.audit_node_id {
        return Err(store_error(
            "cognition outcome points to another audit node",
        ));
    }
    let audit: CognitionAuditEvidence = decode_audit(&audit_node)?;
    if json_commit_digest(AUDIT_DOMAIN, &audit)? != outcome.audit_digest {
        return Err(store_error("committed cognition audit evidence changed"));
    }
    validate_recovered_audit(key, &outcome, &audit)?;

    let (_, job) = store
        .load_job_node(key)
        .map_err(state_store_error)?
        .ok_or_else(|| store_error("committed cognition outcome has no durable job"))?;
    validate_recovered_job(&job, &outcome, &audit)?;
    if completed_job_digest(&job)? != outcome.completed_job_digest
        || envelope_digest(&outcome)? != outcome.envelope_digest
    {
        return Err(store_error(
            "committed cognition envelope does not match durable evidence",
        ));
    }

    let receipt = store
        .run_commit(
            store
                .graph()
                .recover_guarded_commit(&commit_ledger_key(key), &outcome.envelope_digest),
        )?
        .ok_or_else(|| {
            store_error("committed cognition outcome has no matching commit ledger entry")
        })?;
    if !receipt.replayed {
        return Err(store_error(
            "recovered cognition receipt was not marked as historical",
        ));
    }
    // The guarded receipt and envelope prove that the ordered outbox manifest
    // and all of its nodes were created atomically. Recovery intentionally does
    // not reread every subsequently mutable outbox node: doing so on every
    // claim would make draining a bounded manifest quadratic. Claim and ack are
    // the current-state integrity checkpoints; they direct-load and validate a
    // manifest entry before any delivery transition is accepted.
    Ok(Some(map_outcome(
        &receipt,
        &outcome,
        audit,
        CognitionCommitStatus::AlreadyApplied,
    )?))
}

fn validate_recovered_job(
    job: &super::CognitionJob,
    outcome: &DurableOutcome,
    audit: &CognitionAuditEvidence,
) -> Result<(), CognitionCommitError> {
    if job.status != CognitionJobStatus::Completed
        || job.typedid_request_digest != audit.typedid_request_digest
        || job.proposal_digest.as_deref() != Some(outcome.proposal_digest.as_str())
        || job.completion_digest.as_deref() != Some(outcome.prepared_digest.as_str())
        || job.transitioned_at != audit.prepared_at
    {
        return Err(store_error(
            "committed cognition job does not match its durable outcome and TypeDID request",
        ));
    }
    Ok(())
}

pub(super) fn map_outcome(
    receipt: &GraphCommitReceipt,
    durable: &DurableOutcome,
    audit: CognitionAuditEvidence,
    status: CognitionCommitStatus,
) -> Result<CognitionCommitOutcome, CognitionCommitError> {
    if durable.audit_node_id.trim().is_empty() {
        return Err(store_error("cognition outcome has no audit node identity"));
    }
    if durable.effect != audit.effect {
        return Err(store_error(
            "cognition outcome effect does not match audit evidence",
        ));
    }
    let committed_at = validated_commit_time(receipt, audit.prepared_at)?;
    Ok(CognitionCommitOutcome {
        status,
        effect: durable.effect,
        backend_commit_hash: tagged_digest(
            "querygraph.cognition.backend-commit.v1",
            &[receipt.commit_id.as_bytes()],
        ),
        prior_version: durable.prior_version.clone(),
        resulting_version: durable.resulting_version.clone(),
        affected_ids: audit.affected_ids.clone(),
        committed_at,
        audit,
    })
}

fn validated_commit_time(
    receipt: &GraphCommitReceipt,
    prepared_at: DateTime<Utc>,
) -> Result<DateTime<Utc>, CognitionCommitError> {
    let canonical = RfcDate::parse(receipt.committed_at.clone())
        .map_err(|_| store_error("cognition backend commit timestamp is not canonical RFC 3339"))?;
    let committed_at = DateTime::parse_from_rfc3339(canonical.as_str())
        .map_err(|_| store_error("cognition backend commit timestamp is not canonical RFC 3339"))?
        .with_timezone(&Utc);
    if committed_at < prepared_at {
        return Err(store_error(
            "cognition backend commit timestamp predates preparation",
        ));
    }
    Ok(committed_at)
}

pub(super) fn validate_durable_outcome(
    key: &CognitionIdempotencyKey,
    outcome: &DurableOutcome,
) -> Result<(), CognitionCommitError> {
    if outcome.schema_version != OUTCOME_SCHEMA_VERSION {
        return Err(store_error(
            "unsupported persisted cognition outcome schema version",
        ));
    }
    for (name, digest) in [
        ("proposal", outcome.proposal_digest.as_str()),
        ("prepared commit", outcome.prepared_digest.as_str()),
        ("prior version", outcome.prior_version.as_str()),
        ("resulting version", outcome.resulting_version.as_str()),
        ("audit", outcome.audit_digest.as_str()),
        ("completed job", outcome.completed_job_digest.as_str()),
        ("commit envelope", outcome.envelope_digest.as_str()),
    ] {
        if !is_sha256(digest) {
            return Err(store_error(format!(
                "committed cognition {name} digest is invalid"
            )));
        }
    }
    let version_shape_is_invalid = match outcome.effect {
        CognitionEffect::Mutated => outcome.prior_version == outcome.resulting_version,
        CognitionEffect::NoChange => outcome.prior_version != outcome.resulting_version,
    };
    if version_shape_is_invalid
        || outcome.resulting_version
            != resulting_version(
                outcome.effect,
                &outcome.prior_version,
                &outcome.prepared_digest,
            )
        || outcome.audit_node_id != audit_node_id(key).as_str()
    {
        return Err(store_error(
            "committed cognition outcome fields are inconsistent",
        ));
    }
    validate_outbox_manifest(outcome.effect, &outcome.outbox_node_ids)?;
    Ok(())
}

fn validate_outbox_manifest(
    effect: CognitionEffect,
    outbox_node_ids: &[String],
) -> Result<(), CognitionCommitError> {
    let shape_is_invalid = match effect {
        CognitionEffect::Mutated => outbox_node_ids.is_empty(),
        CognitionEffect::NoChange => !outbox_node_ids.is_empty(),
    };
    if shape_is_invalid
        || outbox_node_ids.len() > super::outbox::MAX_COGNITION_OUTBOX_ENTRIES
        || !within_byte_budget(
            outbox_node_ids.iter().map(String::as_str),
            MAX_OUTBOX_MANIFEST_BYTES,
        )
        || outbox_node_ids
            .iter()
            .any(|id| !super::outbox::is_canonical_outbox_node_id(id))
    {
        return Err(store_error(
            "committed cognition outbox manifest is invalid",
        ));
    }
    let unique: BTreeSet<_> = outbox_node_ids.iter().collect();
    if unique.len() != outbox_node_ids.len() {
        return Err(store_error(
            "committed cognition outbox manifest is invalid",
        ));
    }
    Ok(())
}

fn validate_recovered_audit(
    key: &CognitionIdempotencyKey,
    outcome: &DurableOutcome,
    audit: &CognitionAuditEvidence,
) -> Result<(), CognitionCommitError> {
    if audit.operation_id != key.job_id()
        || audit.space_id != key.space_id()
        || !super::graph::authority_scope_matches(key, &audit.subject, &audit.purpose)
        || audit.proposal_digest != outcome.proposal_digest
        || audit.effect != outcome.effect
        || audit.schema_version != CognitionAuditEvidence::SCHEMA_VERSION
        || audit.authority_revalidated_at > audit.prepared_at
        || audit.governed_scan_digest == audit.snapshot_digest
        || audit.subject.trim().is_empty()
        || audit.purpose.trim().is_empty()
        || audit.policy_decision_id.trim().is_empty()
    {
        return Err(store_error(
            "committed cognition audit identity is inconsistent",
        ));
    }
    if audit
        .governed_source_scope
        .as_ref()
        .is_some_and(|scope| !is_sha256(scope.as_str()))
    {
        return Err(store_error(
            "committed cognition audit source scope is invalid",
        ));
    }
    for digest in [
        audit.proposal_digest.as_str(),
        audit.binding_digest.as_str(),
        audit.source_manifest_digest.as_str(),
        audit.typedid_request_digest.as_str(),
        audit.governed_scan_digest.as_str(),
        audit.snapshot_digest.as_str(),
        audit.authorization_receipt_digest.as_str(),
        audit.evidence_digest.as_str(),
    ] {
        if !is_sha256(digest) {
            return Err(store_error(
                "committed cognition audit contains an invalid digest",
            ));
        }
    }
    validate_affected_ids(outcome.effect, &audit.affected_ids)?;
    if audit.affected_ids.len() != outcome.outbox_node_ids.len() {
        return Err(store_error(
            "committed cognition mutation evidence is inconsistent",
        ));
    }
    Ok(())
}

pub(super) fn validate_affected_ids(
    effect: CognitionEffect,
    affected_ids: &[typesec_memory::MemoryId],
) -> Result<(), CognitionCommitError> {
    let shape_is_invalid = match effect {
        CognitionEffect::Mutated => affected_ids.is_empty(),
        CognitionEffect::NoChange => !affected_ids.is_empty(),
    };
    if shape_is_invalid
        || affected_ids.len() > MAX_COGNITION_MUTATIONS
        || !within_byte_budget(
            affected_ids.iter().map(|id| id.as_str()),
            MAX_COGNITION_SOURCE_BYTES,
        )
        || affected_ids
            .iter()
            .any(|id| !is_canonical_text(id.as_str()))
    {
        return Err(store_error(
            "committed cognition audit affected ids are invalid",
        ));
    }
    if affected_ids.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(store_error(
            "committed cognition audit affected ids are invalid",
        ));
    }
    Ok(())
}

fn within_byte_budget<'a>(mut values: impl Iterator<Item = &'a str>, limit: usize) -> bool {
    values
        .try_fold(0usize, |total, value| total.checked_add(value.len()))
        .is_some_and(|total| total <= limit)
}

fn decode_versioned_commit_payload<T: serde::de::DeserializeOwned>(
    node: &Node,
    label: &str,
    expected_version: u32,
    unsupported_schema: &'static str,
    invalid_payload: &'static str,
) -> Result<T, CognitionCommitError> {
    let payload = payload_json(node, label).map_err(state_store_error)?;
    if payload
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(expected_version))
    {
        return Err(store_error(unsupported_schema));
    }
    serde_json::from_value(payload.clone()).map_err(|_| store_error(invalid_payload))
}

fn decode_durable_outcome(node: &Node) -> Result<DurableOutcome, CognitionCommitError> {
    decode_versioned_commit_payload(
        node,
        OUTCOME_LABEL,
        OUTCOME_SCHEMA_VERSION,
        "unsupported persisted cognition outcome schema version",
        "persisted cognition outcome payload is invalid",
    )
}

fn decode_audit(node: &Node) -> Result<CognitionAuditEvidence, CognitionCommitError> {
    decode_versioned_commit_payload(
        node,
        AUDIT_LABEL,
        CognitionAuditEvidence::SCHEMA_VERSION,
        "unsupported persisted cognition audit schema version",
        "persisted cognition audit payload is invalid",
    )
}

#[cfg(test)]
mod tests;
