//! Leased, reopen-safe delivery of ID-only cognition index mutations.

use chrono::{DateTime, Duration, Utc};
use grust_core::prelude::{
    GraphCommitStore, GraphExpectation, GraphMutation, GrustError, GuardedGraphCommit, Node, NodeId,
};
use serde::{Deserialize, Serialize};
/// Maximum ID-only repair entries one cognition transaction may create.
pub use typesec_memory::MAX_COGNITION_MUTATIONS as MAX_COGNITION_OUTBOX_ENTRIES;
use typesec_memory::{CognitionCommitStore, CognitionIdempotencyKey, IndexMutation};

use super::backend::state_backend_error;
use super::bounds::is_canonical_text;
use super::graph::{
    DurableOutcome, OUTBOX_LABEL, OUTCOME_LABEL, OUTCOME_SCHEMA_VERSION, commit_key_digest,
    decode_versioned_payload, json_digest, outbox_node_id, outcome_node_id, owner_digest,
    validate_idempotency_key,
};
use super::lease::{lease_expiry, validate_bearer_token};
use super::{CognitionLeaseState, CognitionStateError};
use crate::GraphStoreMemoryStore;

const TRANSITION_DOMAIN: &str = "querygraph.cognition.outbox-transition.v1";
const OUTBOX_SCHEMA_VERSION: u32 = 1;

/// Maximum entries one scoped claim call may return.
pub const MAX_COGNITION_OUTBOX_CLAIM: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum OutboxStatus {
    Pending,
    Leased,
    Acked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CognitionOutboxRecord {
    pub schema_version: u32,
    pub commit_digest: String,
    pub ordinal: u64,
    pub mutation: IndexMutation,
    pub status: OutboxStatus,
    pub revision: u64,
    pub attempts: u32,
    pub lease: Option<CognitionLeaseState>,
    pub acked_token_digest: Option<String>,
}

impl CognitionOutboxRecord {
    pub(super) fn pending(commit_digest: String, ordinal: u64, mutation: IndexMutation) -> Self {
        Self {
            schema_version: OUTBOX_SCHEMA_VERSION,
            commit_digest,
            ordinal,
            mutation,
            status: OutboxStatus::Pending,
            revision: 0,
            attempts: 0,
            lease: None,
            acked_token_digest: None,
        }
    }
}

/// Exclusive ID-only index repair work returned to one worker.
pub struct CognitionOutboxClaim {
    /// Digest-safe graph identity used for acknowledgement.
    entry_id: String,
    /// Rehydrate this record id inside the authorized vault; no plaintext is carried.
    mutation: IndexMutation,
    /// Bearer token required for acknowledgement.
    token: String,
    /// Delivery attempt number.
    attempt: u32,
    /// Exclusive ownership deadline.
    expires_at: DateTime<Utc>,
}

impl CognitionOutboxClaim {
    /// Borrow the digest-safe entry identity used for acknowledgement.
    pub fn entry_id(&self) -> &str {
        &self.entry_id
    }

    /// Borrow the ID-only semantic-index mutation.
    pub fn mutation(&self) -> &IndexMutation {
        &self.mutation
    }

    /// Borrow the bearer token required for acknowledgement.
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

impl<G: GraphCommitStore> GraphStoreMemoryStore<G> {
    /// Claim up to the public limit from one committed job's bounded manifest.
    ///
    /// Job scoping direct-loads only the immutable outbox identities bound into
    /// that commit envelope; acknowledged history is never globally scanned.
    /// Claims are capped at one hour so abandoned work remains recoverable.
    /// Transient backend errors are returned for the worker to retry; they are
    /// never mistaken for an empty queue.
    ///
    /// `owner` is an audit identity, not authentication. The enclosing trusted
    /// worker-pool boundary must authorize claims and protect the scoped key.
    pub fn claim_cognition_outbox(
        &self,
        key: &CognitionIdempotencyKey,
        owner: &str,
        now: DateTime<Utc>,
        ttl: Duration,
        limit: usize,
    ) -> Result<Vec<CognitionOutboxClaim>, CognitionStateError> {
        validate_idempotency_key(key)
            .map_err(|message| CognitionStateError::Invalid(message.into()))?;
        if !is_canonical_text(owner) || limit == 0 || limit > MAX_COGNITION_OUTBOX_CLAIM {
            return Err(CognitionStateError::Invalid(
                "outbox owner and bounded claim limit are invalid".into(),
            ));
        }
        let expires_at = lease_expiry(now, ttl)?;
        let node = self
            .run_graph(self.graph.get_node(&outcome_node_id(key)))?
            .ok_or(CognitionStateError::NotFound)?;
        let outcome: DurableOutcome = decode_versioned_payload(
            &node,
            OUTCOME_LABEL,
            OUTCOME_SCHEMA_VERSION,
            "unsupported persisted cognition outcome schema version",
        )?;
        super::commit_outcome::validate_durable_outcome(key, &outcome)
            .map_err(|_| corrupt_outbox("durable outcome manifest is invalid"))?;
        self.recover_cognition(key, &outcome.proposal_digest)
            .map_err(|_| corrupt_outbox("durable outcome manifest is not committed"))?
            .ok_or_else(|| corrupt_outbox("durable outcome manifest is missing"))?;

        let mut claims = Vec::new();
        let commit_digest = commit_key_digest(key);
        for (ordinal, entry_id) in outcome.outbox_node_ids.iter().enumerate() {
            if claims.len() == limit {
                break;
            }
            let id = NodeId::from(entry_id.as_str());
            let node = self
                .run_graph(self.graph.get_node(&id))?
                .ok_or_else(|| corrupt_outbox("durable manifest entry is missing"))?;
            let mut record = decode_outbox(&node)?;
            let ordinal = u64::try_from(ordinal)
                .map_err(|_| corrupt_outbox("manifest ordinal exceeds u64"))?;
            if record.commit_digest != commit_digest || record.ordinal != ordinal {
                return Err(corrupt_outbox(
                    "entry identity does not match its commit manifest",
                ));
            }
            if record.status == OutboxStatus::Acked {
                continue;
            }
            if record
                .lease
                .as_ref()
                .is_some_and(|lease| now < lease.acquired_at)
            {
                return Err(CognitionStateError::InvalidTransition(
                    "outbox claim timestamp predates its active lease".into(),
                ));
            }
            if record
                .lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at > now)
            {
                continue;
            }
            record.attempts = record.attempts.checked_add(1).ok_or_else(|| {
                CognitionStateError::InvalidTransition("outbox attempt counter overflow".into())
            })?;
            advance_outbox_revision(&mut record)?;
            record.status = OutboxStatus::Leased;
            let token = super::lease::new_lease_token();
            record.lease = Some(CognitionLeaseState {
                owner_digest: owner_digest(owner),
                token_digest: super::graph::token_digest(&token),
                acquired_at: now,
                expires_at,
            });
            if !self.try_outbox_transition(&node, &record)? {
                continue;
            }
            claims.push(CognitionOutboxClaim {
                entry_id: node.id.as_str().to_owned(),
                mutation: record.mutation,
                token,
                attempt: record.attempts,
                expires_at,
            });
        }
        Ok(claims)
    }

    /// Acknowledge a delivered repair. Identical retries are harmless.
    ///
    /// The unexpired claim token is the sole bearer credential for this state
    /// transition; the enclosing service must keep it confidential.
    pub fn ack_cognition_outbox(
        &self,
        entry_id: &str,
        token: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, CognitionStateError> {
        if !is_canonical_outbox_node_id(entry_id) {
            return Err(CognitionStateError::Invalid(
                "outbox entry id is not canonical".into(),
            ));
        }
        validate_bearer_token(token)?;
        let id = NodeId::from(entry_id);
        let node = self
            .run_graph(self.graph.get_node(&id))?
            .ok_or(CognitionStateError::NotFound)?;
        let mut record = decode_outbox(&node)?;
        let supplied = super::graph::token_digest(token);
        if record.status == OutboxStatus::Acked {
            return if record.acked_token_digest.as_deref() == Some(supplied.as_str()) {
                Ok(false)
            } else {
                Err(CognitionStateError::StaleLease)
            };
        }
        let lease = record
            .lease
            .as_ref()
            .ok_or(CognitionStateError::StaleLease)?;
        if now < lease.acquired_at {
            return Err(CognitionStateError::InvalidTransition(
                "outbox acknowledgement predates its lease".into(),
            ));
        }
        if lease.expires_at <= now || lease.token_digest != supplied {
            return Err(CognitionStateError::StaleLease);
        }
        record.status = OutboxStatus::Acked;
        advance_outbox_revision(&mut record)?;
        record.lease = None;
        record.acked_token_digest = Some(supplied);
        if self.try_outbox_transition(&node, &record)? {
            Ok(true)
        } else {
            let current = self
                .run_graph(self.graph.get_node(&id))?
                .ok_or(CognitionStateError::NotFound)?;
            let current = decode_outbox(&current)?;
            if current.status == OutboxStatus::Acked
                && current.acked_token_digest == record.acked_token_digest
            {
                Ok(false)
            } else {
                Err(CognitionStateError::ConcurrentModification)
            }
        }
    }

    fn try_outbox_transition(
        &self,
        expected: &grust_core::prelude::Node,
        next: &CognitionOutboxRecord,
    ) -> Result<bool, CognitionStateError> {
        let node = super::graph::payload_node(OUTBOX_LABEL, expected.id.clone(), next)?;
        let request = json_digest(TRANSITION_DOMAIN, next)?;
        let key = format!("cognition-outbox:{}:{}", expected.id, next.revision);
        let guarded =
            GuardedGraphCommit::new(key, request, vec![GraphMutation::UpsertNode(node.clone())])
                .with_expectations(vec![GraphExpectation::Exact(expected.clone())]);
        match self.bridge.run(self.graph.commit_guarded(&guarded)) {
            Ok(receipt) => {
                if receipt.replayed {
                    self.verify_replayed_node(&node)?;
                }
                Ok(true)
            }
            Err(
                GrustError::GraphExpectationFailed(_) | GrustError::GraphIdempotencyConflict(_),
            ) => Ok(false),
            Err(_) => Err(state_backend_error()),
        }
    }
}

fn advance_outbox_revision(record: &mut CognitionOutboxRecord) -> Result<(), CognitionStateError> {
    record.revision = record
        .revision
        .checked_add(1)
        .ok_or_else(|| CognitionStateError::InvalidTransition("outbox revision overflow".into()))?;
    Ok(())
}

fn decode_outbox(node: &Node) -> Result<CognitionOutboxRecord, CognitionStateError> {
    let record = decode_versioned_payload(
        node,
        OUTBOX_LABEL,
        OUTBOX_SCHEMA_VERSION,
        "unsupported persisted cognition outbox schema version",
    )?;
    validate_persisted_outbox(node, &record)?;
    Ok(record)
}

fn validate_persisted_outbox(
    node: &Node,
    record: &CognitionOutboxRecord,
) -> Result<(), CognitionStateError> {
    if record.schema_version != OUTBOX_SCHEMA_VERSION {
        return Err(corrupt_outbox(
            "unsupported persisted outbox schema version",
        ));
    }
    if !super::graph::is_sha256(&record.commit_digest) || !canonical_mutation_id(&record.mutation) {
        return Err(corrupt_outbox("immutable entry identity is invalid"));
    }
    let expected_id = outbox_node_id(&record.commit_digest, record.ordinal, &record.mutation)?;
    if node.id != expected_id {
        return Err(corrupt_outbox(
            "node id does not match immutable entry identity",
        ));
    }
    let (expects_lease, expects_ack) = match record.status {
        OutboxStatus::Pending => (false, false),
        OutboxStatus::Leased => (true, false),
        OutboxStatus::Acked => (false, true),
    };
    if expects_lease != record.lease.is_some() || expects_ack != record.acked_token_digest.is_some()
    {
        return Err(corrupt_outbox("status and credential state disagree"));
    }
    if (record.status == OutboxStatus::Pending) != (record.attempts == 0) {
        return Err(corrupt_outbox("status and attempt count disagree"));
    }
    if record
        .acked_token_digest
        .as_deref()
        .is_some_and(|digest| !super::graph::is_sha256(digest))
    {
        return Err(corrupt_outbox(
            "acknowledgement digest is not canonical SHA-256",
        ));
    }
    if let Some(lease) = &record.lease {
        if !super::graph::is_sha256(&lease.owner_digest)
            || !super::graph::is_sha256(&lease.token_digest)
        {
            return Err(corrupt_outbox(
                "lease identity digest is not canonical SHA-256",
            ));
        }
        if lease.acquired_at >= lease.expires_at || record.attempts == 0 {
            return Err(corrupt_outbox("lease timestamps or attempt are invalid"));
        }
    }
    Ok(())
}

fn canonical_mutation_id(mutation: &IndexMutation) -> bool {
    let id = match mutation {
        IndexMutation::Upsert(id) | IndexMutation::Remove(id) => id.as_str(),
    };
    is_canonical_text(id)
}

pub(super) fn is_canonical_outbox_node_id(value: &str) -> bool {
    value.strip_prefix("cog-outbox:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn corrupt_outbox(message: &str) -> CognitionStateError {
    CognitionStateError::Backend(format!("invalid persisted cognition outbox: {message}"))
}
