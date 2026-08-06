//! Cross-field validation for vault-prepared commits.
//!
//! These checks enforce identities and relationships the storage adapter can
//! prove without reimplementing TypeSec's policy and proposal compiler.
//! TypeSec remains authoritative for deriving operation content, labels,
//! lineage, and retention from a proposal.

use std::collections::{BTreeMap, BTreeSet};

use typesec_memory::{
    CognitionAuditEvidence, CognitionCommitError, CognitionEffect, IndexMutation,
    MAX_COGNITION_MUTATIONS, MAX_COGNITION_SOURCE_COUNT, MemoryId, PreparedCognitionCommit,
    StoreBatchOp, StoreError,
};

use super::bounds::is_canonical_text;
use super::graph::{authority_scope_matches, is_sha256, validate_idempotency_key};
use super::outbox::MAX_COGNITION_OUTBOX_ENTRIES;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExpectedIndexMutation {
    Upsert,
    Remove,
}

pub(super) fn validate_prepared_commit(
    commit: &PreparedCognitionCommit,
) -> Result<(), CognitionCommitError> {
    validate_collection_sizes(
        commit.effect(),
        commit.source_preconditions().len(),
        commit.operations().len(),
        commit.index_outbox().len(),
        commit.audit().affected_ids.len(),
    )?;
    super::commit_outcome::validate_affected_ids(commit.effect(), &commit.audit().affected_ids)?;
    validate_identity(commit)?;
    validate_output_entities(commit.operations())?;
    validate_digests(commit)?;
    let source_ids = unique_source_ids(commit)?;
    let expected = expected_mutations(commit, &source_ids)?;
    validate_outbox(commit, &expected)?;
    let expected_affected = expected.keys().cloned().collect::<Vec<_>>();
    if commit.audit().affected_ids != expected_affected {
        return Err(invalid("audit affected IDs do not match operations"));
    }
    Ok(())
}

fn validate_identity(commit: &PreparedCognitionCommit) -> Result<(), CognitionCommitError> {
    validate_idempotency_key(commit.idempotency_key()).map_err(invalid)?;
    if commit.proposal_digest().trim().is_empty() {
        return Err(invalid("incomplete prepared cognition commit"));
    }
    if commit.idempotency_key().space_id() != commit.audit().space_id {
        return Err(invalid("idempotency space does not match audit space"));
    }
    if commit.idempotency_key().job_id() != commit.audit().operation_id {
        return Err(invalid("idempotency job does not match audit operation"));
    }
    if !authority_scope_matches(
        commit.idempotency_key(),
        &commit.audit().subject,
        &commit.audit().purpose,
    ) {
        return Err(invalid(
            "idempotency authority scope does not match audit authority",
        ));
    }
    if commit.proposal_digest() != commit.audit().proposal_digest {
        return Err(invalid("proposal digest does not match audit evidence"));
    }
    if !is_canonical_text(&commit.audit().subject)
        || !is_canonical_text(&commit.audit().purpose)
        || !is_canonical_text(&commit.audit().policy_decision_id)
        || commit.audit().schema_version != CognitionAuditEvidence::SCHEMA_VERSION
        || commit.audit().authority_revalidated_at > commit.audit().prepared_at
    {
        return Err(invalid("audit authority identity is incomplete"));
    }
    Ok(())
}

fn validate_collection_sizes(
    effect: CognitionEffect,
    source_count: usize,
    operation_count: usize,
    outbox_count: usize,
    affected_count: usize,
) -> Result<(), CognitionCommitError> {
    if source_count == 0 || source_count > MAX_COGNITION_SOURCE_COUNT {
        return Err(invalid("cognition source count exceeds its fixed limit"));
    }
    if operation_count > MAX_COGNITION_MUTATIONS {
        return Err(invalid("cognition operation count exceeds its fixed limit"));
    }
    if outbox_count > MAX_COGNITION_OUTBOX_ENTRIES || affected_count > MAX_COGNITION_MUTATIONS {
        return Err(invalid(
            "cognition mutation evidence exceeds its fixed limit",
        ));
    }
    match effect {
        CognitionEffect::Mutated
            if operation_count == 0 || outbox_count == 0 || affected_count == 0 =>
        {
            return Err(invalid(
                "mutating cognition commit has incomplete mutation evidence",
            ));
        }
        CognitionEffect::NoChange
            if operation_count != 0 || outbox_count != 0 || affected_count != 0 =>
        {
            return Err(invalid(
                "no-change cognition commit contains mutation evidence",
            ));
        }
        CognitionEffect::Mutated | CognitionEffect::NoChange => {}
    }
    Ok(())
}

fn validate_output_entities(operations: &[StoreBatchOp]) -> Result<(), CognitionCommitError> {
    let mut entity_count = 0usize;
    let mut shared_kinds = BTreeMap::<&str, &str>::new();
    for record in operations.iter().filter_map(|operation| match operation {
        StoreBatchOp::Put(record) => Some(record),
        StoreBatchOp::Invalidate { .. } => None,
    }) {
        entity_count = entity_count
            .checked_add(record.entities.len())
            .ok_or_else(|| invalid("cognition output entity count exceeds its fixed limit"))?;
        if entity_count > MAX_COGNITION_MUTATIONS {
            return Err(invalid(
                "cognition output entity count exceeds its fixed limit",
            ));
        }

        let mut record_names = BTreeSet::new();
        for entity in &record.entities {
            if !is_canonical_text(&entity.name) || !is_canonical_text(&entity.kind) {
                return Err(invalid("cognition output entity identity is not canonical"));
            }
            if !record_names.insert(entity.name.as_str()) {
                return Err(invalid("cognition output repeats an entity name"));
            }
            if shared_kinds
                .insert(entity.name.as_str(), entity.kind.as_str())
                .is_some_and(|kind| kind != entity.kind.as_str())
            {
                return Err(invalid(
                    "cognition outputs disagree on a shared entity kind",
                ));
            }
        }
    }
    Ok(())
}

fn validate_digests(commit: &PreparedCognitionCommit) -> Result<(), CognitionCommitError> {
    let audit = commit.audit();
    for (name, digest) in [
        ("proposal", commit.proposal_digest()),
        ("binding", audit.binding_digest.as_str()),
        ("source manifest", audit.source_manifest_digest.as_str()),
        ("TypeDID request", audit.typedid_request_digest.as_str()),
        ("governed scan", audit.governed_scan_digest.as_str()),
        ("snapshot", audit.snapshot_digest.as_str()),
        (
            "authorization receipt",
            audit.authorization_receipt_digest.as_str(),
        ),
        ("worker evidence", audit.evidence_digest.as_str()),
    ] {
        if !is_sha256(digest) {
            return Err(invalid(format!("{name} digest is not canonical SHA-256")));
        }
    }
    if audit.governed_scan_digest == audit.snapshot_digest {
        return Err(invalid(
            "governed scan and snapshot digests must be distinct",
        ));
    }
    if commit
        .source_preconditions()
        .iter()
        .any(|source| !is_sha256(&source.record_digest))
    {
        return Err(invalid(
            "source precondition digest is not canonical SHA-256",
        ));
    }
    Ok(())
}

fn unique_source_ids(
    commit: &PreparedCognitionCommit,
) -> Result<BTreeSet<MemoryId>, CognitionCommitError> {
    let sources: BTreeSet<_> = commit
        .source_preconditions()
        .iter()
        .map(|source| source.id.clone())
        .collect();
    if sources.iter().any(|id| !is_canonical_text(id.as_str())) {
        return Err(invalid("cognition source ID is not canonical text"));
    }
    if sources.len() != commit.source_preconditions().len() {
        return Err(invalid("duplicate cognition source precondition"));
    }
    Ok(sources)
}

fn expected_mutations(
    commit: &PreparedCognitionCommit,
    source_ids: &BTreeSet<MemoryId>,
) -> Result<BTreeMap<MemoryId, ExpectedIndexMutation>, CognitionCommitError> {
    let mut expected = BTreeMap::new();
    for operation in commit.operations() {
        let (id, index) = match operation {
            StoreBatchOp::Put(record) => {
                if record.space_id != commit.idempotency_key().space_id() {
                    return Err(invalid("cognition output belongs to another space"));
                }
                (record.id.clone(), ExpectedIndexMutation::Upsert)
            }
            StoreBatchOp::Invalidate { id, .. } => {
                if !source_ids.contains(id) {
                    return Err(invalid("invalidation target is not a guarded source"));
                }
                (id.clone(), ExpectedIndexMutation::Remove)
            }
        };
        if !is_canonical_text(id.as_str()) {
            return Err(invalid(
                "cognition operation record ID is not canonical text",
            ));
        }
        if expected.insert(id, index).is_some() {
            return Err(invalid("record has more than one cognition operation"));
        }
    }
    Ok(expected)
}

fn validate_outbox(
    commit: &PreparedCognitionCommit,
    expected: &BTreeMap<MemoryId, ExpectedIndexMutation>,
) -> Result<(), CognitionCommitError> {
    let mut actual = BTreeMap::new();
    for mutation in commit.index_outbox() {
        let (id, kind) = match mutation {
            IndexMutation::Upsert(id) => (id.clone(), ExpectedIndexMutation::Upsert),
            IndexMutation::Remove(id) => (id.clone(), ExpectedIndexMutation::Remove),
        };
        if actual.insert(id, kind).is_some() {
            return Err(invalid("duplicate cognition index outbox ID"));
        }
    }
    if &actual != expected {
        return Err(invalid("index outbox does not match cognition operations"));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> CognitionCommitError {
    CognitionCommitError::Store(StoreError::Backend(message.into()))
}

#[cfg(test)]
mod tests;
