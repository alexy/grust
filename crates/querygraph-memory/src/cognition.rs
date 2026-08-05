//! QueryGraph-native cognition over governed LakeCat snapshots and Sail.
//!
//! LakeCat proves the source, Sail performs bounded batch work, and this
//! module emits an inert TypeSec proposal. Only `MemoryVault` applies it.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use typesec_memory::{CognitionProposal, ConsolidationPlan, Label, MemoryId, RecalledMemory};

#[cfg(feature = "sail")]
mod sail;
#[cfg(feature = "sail")]
pub use sail::LiveSailCognitionExecutor;

use crate::analytics::{contradiction_plan, dedup_plan};

/// Hash-bound proof of the LakeCat snapshot and governed Sail scan used by a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GovernedLakeCatSnapshot {
    /// LakeCat catalog URI or id.
    pub catalog: String,
    /// Iceberg namespace.
    pub namespace: String,
    /// Iceberg table.
    pub table: String,
    /// Immutable Iceberg snapshot id.
    pub snapshot_id: i64,
    /// Digest of the opaque LakeCat/Sail plan-task token.
    pub plan_task_digest: String,
    /// Cryptographically verified TypeDID subject.
    pub subject: String,
    /// Purpose bound into authorization and scan planning.
    pub purpose: String,
    /// Projection LakeCat allowed after policy narrowing.
    pub effective_projection: Vec<String>,
    /// Digest of LakeCat's authorization receipt.
    pub authorization_receipt_digest: String,
}

impl GovernedLakeCatSnapshot {
    /// Validate and return a stable proof digest.
    pub fn digest(&self) -> Result<String, CognitionError> {
        for (name, value) in [
            ("catalog", self.catalog.as_str()),
            ("namespace", self.namespace.as_str()),
            ("table", self.table.as_str()),
            ("planTaskDigest", self.plan_task_digest.as_str()),
            ("subject", self.subject.as_str()),
            ("purpose", self.purpose.as_str()),
            (
                "authorizationReceiptDigest",
                self.authorization_receipt_digest.as_str(),
            ),
        ] {
            if value.trim().is_empty() {
                return Err(CognitionError::InvalidSnapshot(name.to_owned()));
            }
        }
        if self.effective_projection.is_empty()
            || self
                .effective_projection
                .iter()
                .any(|field| field.trim().is_empty())
        {
            return Err(CognitionError::InvalidSnapshot(
                "effectiveProjection".to_owned(),
            ));
        }
        let bytes = serde_json::to_vec(self)
            .map_err(|error| CognitionError::Serialization(error.to_string()))?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

/// Native cognition operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CognitionOperation {
    /// Supersede exact duplicates.
    Deduplicate,
    /// Detect contradictions and propose invalidating obsolete assertions.
    Reconcile,
}

impl CognitionOperation {
    fn name(self) -> &'static str {
        match self {
            Self::Deduplicate => "marciana.deduplicate",
            Self::Reconcile => "marciana.reconcile",
        }
    }
}

/// Authorized inputs to a cognition engine.
pub struct CognitionRequest<'a> {
    /// Idempotent durable job id.
    pub job_id: &'a str,
    /// Governed source proof.
    pub source: &'a GovernedLakeCatSnapshot,
    /// Memories already revealed by TypeSec for this subject and purpose.
    pub memories: &'a [RecalledMemory],
    /// Requested operation.
    pub operation: CognitionOperation,
}

/// Cognition failure.
#[derive(Debug, thiserror::Error)]
pub enum CognitionError {
    /// A governed proof field was absent.
    #[error("invalid governed LakeCat snapshot field: {0}")]
    InvalidSnapshot(String),
    /// No durable job identity was supplied.
    #[error("cognition job id must not be empty")]
    MissingJobId,
    /// Proof serialization failed.
    #[error("cognition proof serialization failed: {0}")]
    Serialization(String),
    /// Sail failed.
    #[error("Sail cognition failed: {0}")]
    Sail(String),
}

/// Engine producing inert proposals, never direct writes.
#[async_trait::async_trait]
pub trait CognitionEngine: Send + Sync {
    /// Produce a proposal from an already-authorized request.
    async fn propose(
        &self,
        request: CognitionRequest<'_>,
    ) -> Result<CognitionProposal, CognitionError>;
}

/// Deterministic conformance oracle for Sail implementations.
#[derive(Debug, Default, Clone, Copy)]
pub struct ReferenceCognitionEngine;

#[async_trait::async_trait]
impl CognitionEngine for ReferenceCognitionEngine {
    async fn propose(
        &self,
        request: CognitionRequest<'_>,
    ) -> Result<CognitionProposal, CognitionError> {
        let (plan, evidence) = match request.operation {
            CognitionOperation::Deduplicate => (
                dedup_plan(request.memories),
                vec!["reference exact normalized-text grouping".to_owned()],
            ),
            CognitionOperation::Reconcile => {
                let (found, plan) = contradiction_plan(request.memories);
                (
                    plan,
                    vec![format!("reference contradictions={}", found.len())],
                )
            }
        };
        build_proposal(request, plan, evidence, "reference", "1")
    }
}

/// Output from a Sail batch executor.
pub struct SailCognitionOutput {
    /// Proposed mutations.
    pub plan: ConsolidationPlan,
    /// Audit-safe evidence with no source plaintext.
    pub evidence: Vec<String>,
    /// Sail plan/model version.
    pub executor_version: String,
}

/// Narrow seam implemented with `grust-sail` and Spark Connect.
#[async_trait::async_trait]
pub trait SailCognitionExecutor: Send + Sync {
    /// Execute only the LakeCat-authorized projection.
    async fn execute(
        &self,
        request: &CognitionRequest<'_>,
    ) -> Result<SailCognitionOutput, CognitionError>;
}

/// Converts Sail results into TypeSec proposals.
pub struct SailCognitionEngine<E> {
    executor: E,
}

impl<E> SailCognitionEngine<E> {
    /// Wrap a Sail executor.
    pub fn new(executor: E) -> Self {
        Self { executor }
    }
}

#[async_trait::async_trait]
impl<E: SailCognitionExecutor> CognitionEngine for SailCognitionEngine<E> {
    async fn propose(
        &self,
        request: CognitionRequest<'_>,
    ) -> Result<CognitionProposal, CognitionError> {
        let output = self.executor.execute(&request).await?;
        build_proposal(
            request,
            output.plan,
            output.evidence,
            "sail",
            &output.executor_version,
        )
    }
}

fn build_proposal(
    request: CognitionRequest<'_>,
    plan: ConsolidationPlan,
    evidence: Vec<String>,
    executor: &str,
    version: &str,
) -> Result<CognitionProposal, CognitionError> {
    if request.job_id.trim().is_empty() {
        return Err(CognitionError::MissingJobId);
    }
    let snapshot = request.source.digest()?;
    let source_ids: Vec<MemoryId> = request.memories.iter().map(|m| m.id.clone()).collect();
    let mut digest = Sha256::new();
    digest.update(snapshot.as_bytes());
    for memory in request.memories {
        digest.update([0]);
        digest.update(memory.id.as_str().as_bytes());
        digest.update([0]);
        digest.update(memory.label.name().as_bytes());
    }
    let joined = request
        .memories
        .iter()
        .fold(Label::Public, |label, memory| label.join(memory.label));
    let mut proposal = CognitionProposal::new(
        request.job_id,
        snapshot,
        format!("sha256:{:x}", digest.finalize()),
        format!("{}.{}", request.operation.name(), executor),
        version,
        source_ids,
        joined,
    )
    .with_plan(plan);
    proposal.evidence = evidence;
    Ok(proposal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use typesec_memory::{MemoryContent, MemoryKind, Provenance};

    fn source() -> GovernedLakeCatSnapshot {
        GovernedLakeCatSnapshot {
            catalog: "lakecat://prod".into(),
            namespace: "research".into(),
            table: "findings".into(),
            snapshot_id: 42,
            plan_task_digest: "sha256:plan".into(),
            subject: "did:key:researcher".into(),
            purpose: "research".into(),
            effective_projection: vec!["finding".into()],
            authorization_receipt_digest: "sha256:receipt".into(),
        }
    }

    fn memory(id: &str, text: &str, label: Label) -> RecalledMemory {
        RecalledMemory {
            id: MemoryId::from_string(id),
            kind: MemoryKind::Semantic,
            label,
            content: MemoryContent::text(text),
            entities: vec![],
            provenance: Provenance::Operator,
            valid_from: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        }
    }

    #[tokio::test]
    async fn proposal_binds_governed_proof_and_joins_labels() {
        let source = source();
        let memories = vec![
            memory("m1", "Alice likes espresso", Label::Internal),
            memory("m2", "alice likes espresso", Label::Sensitive),
        ];
        let proposal = ReferenceCognitionEngine
            .propose(CognitionRequest {
                job_id: "job-42",
                source: &source,
                memories: &memories,
                operation: CognitionOperation::Deduplicate,
            })
            .await
            .unwrap();
        assert_eq!(proposal.joined_label, Label::Sensitive);
        assert_eq!(proposal.plan.steps.len(), 1);
        assert!(proposal.input_snapshot.starts_with("sha256:"));
    }

    #[test]
    fn empty_projection_fails_closed() {
        let mut source = source();
        source.effective_projection.clear();
        assert!(matches!(
            source.digest(),
            Err(CognitionError::InvalidSnapshot(_))
        ));
    }
}
