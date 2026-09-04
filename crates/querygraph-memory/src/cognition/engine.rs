//! Cognition engine contracts and proposal construction.

use typesec_memory::{
    AuthorizedCognitionInput, CognitionBinding, CognitionEffect, CognitionProposal,
    ConsolidationPlan, Label, MemoryId,
};

use super::engine_validation::validate_request;
use super::error::{CognitionError, SailCognitionExecutorError, cognition_executor_error};
use super::operation::CognitionOperation;
use super::profile::CognitionEngineProfile;
use super::snapshot::{CognitionFieldMapping, GovernedLakeCatSnapshot};
use crate::analytics::planning::{deduplicate, reconcile};

/// Authorized inputs to a cognition engine.
pub struct CognitionRequest<'a> {
    /// Idempotent durable job id.
    pub job_id: &'a str,
    /// Governed source proof.
    pub source: &'a GovernedLakeCatSnapshot,
    /// Canonical TypeSec authority binding attached without post-hoc rewriting.
    pub binding: &'a CognitionBinding,
    /// TypeSec-authorized memories and manifest from the same record revisions.
    pub input: &'a AuthorizedCognitionInput,
    /// Mapping used by governed ingestion to derive these authorized memories.
    ///
    /// The mapping is checked against LakeCat's narrowed projection. Native
    /// Sail analysis stages its own bounded, derived schema rather than raw
    /// LakeCat rows or caller-selected column names.
    pub field_mapping: &'a CognitionFieldMapping,
    /// Requested operation.
    pub operation: CognitionOperation,
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
        validate_request(&request)?;
        let profile = CognitionEngineProfile::reference(request.operation);
        validate_profile(request.operation, profile)?;
        let (plan, evidence) = match request.operation {
            CognitionOperation::Deduplicate => {
                let planning = deduplicate(request.input.memories());
                (
                    planning.plan,
                    vec!["reference exact normalized-text grouping".to_owned()],
                )
            }
            CognitionOperation::Reconcile => {
                let planning = reconcile(request.input.memories());
                (
                    planning.plan,
                    vec![format!("reference contradictions={}", planning.pairs.len())],
                )
            }
        };
        build_proposal(request, plan, evidence, profile)
    }
}

/// Output from a Sail batch executor.
pub struct SailCognitionOutput {
    /// Proposed mutations.
    pub plan: ConsolidationPlan,
    /// Audit-safe evidence with no source plaintext.
    pub evidence: Vec<String>,
}

/// Narrow seam implemented with `grust-sail` and Spark Connect.
#[async_trait::async_trait]
pub trait SailCognitionExecutor: Send + Sync {
    /// Analyze only the TypeSec-authorized memories derived from the governed
    /// LakeCat projection.
    async fn execute(
        &self,
        request: &CognitionRequest<'_>,
    ) -> Result<SailCognitionOutput, SailCognitionExecutorError>;
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
        validate_request(&request)?;
        let profile = CognitionEngineProfile::sail(request.operation);
        validate_profile(request.operation, profile)?;
        let output = self
            .executor
            .execute(&request)
            .await
            .map_err(cognition_executor_error)?;
        build_proposal(request, output.plan, output.evidence, profile)
    }
}

fn build_proposal(
    request: CognitionRequest<'_>,
    plan: ConsolidationPlan,
    evidence: Vec<String>,
    profile: CognitionEngineProfile,
) -> Result<CognitionProposal, CognitionError> {
    let source_ids: Vec<MemoryId> = request
        .input
        .manifest()
        .sources
        .iter()
        .map(|source| source.id.clone())
        .collect();
    let joined = request
        .input
        .memories()
        .iter()
        .fold(Label::Public, |label, memory| label.join(memory.label));
    let effect = effect_for_plan(&plan);
    let mut proposal = CognitionProposal::new(
        request.job_id,
        request.binding.snapshot_digest.clone(),
        request.input.manifest().digest.clone(),
        profile.algorithm(),
        profile.algorithm_version(),
        source_ids,
        joined,
    )
    .with_plan(plan)
    .with_effect(effect)
    .with_binding(request.binding.clone());
    proposal.evidence = evidence;
    proposal
        .canonical_digest()
        .map_err(|_| CognitionError::InvalidExecutorOutput)?;
    Ok(proposal)
}

fn effect_for_plan(plan: &ConsolidationPlan) -> CognitionEffect {
    if plan.steps.is_empty() {
        CognitionEffect::NoChange
    } else {
        CognitionEffect::Mutated
    }
}

fn validate_profile(
    operation: CognitionOperation,
    profile: CognitionEngineProfile,
) -> Result<(), CognitionError> {
    if operation.is_native_algorithm(profile.algorithm(), profile.algorithm_version()) {
        Ok(())
    } else {
        Err(CognitionError::InvalidAlgorithm)
    }
}
