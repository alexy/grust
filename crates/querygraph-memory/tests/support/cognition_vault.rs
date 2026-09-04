use std::sync::Arc;

use grust_core::prelude::GraphCommitStore;
use querygraph_memory::GraphStoreMemoryStore;
use typesec_core::policy::{MintOptions, RequestContext, mint_capability_for_id};
use typesec_core::{CanRead, CanWrite, Capability, Permission, Resource};
use typesec_memory::{
    CognitionAuthorityError, CognitionAuthorityEvidence, CognitionAuthorityVerifier,
    CognitionBinding, CognitionCommitOutcome, CognitionEffect, CognitionProposal,
    ConsolidationPlan, ConsolidationStep, GovernedSourceScope, Label, MemoryError, MemorySpace,
    MemoryStore, MemoryVault, StoredRecord,
};

use super::{at, digest, stage_at};

const SUBJECT: &str = "did:key:alice";
const PURPOSE: &str = "research";
const POLICY: &str = r#"
roles:
  - name: cognition-worker
    permissions: [read, write]
    resources: ["memory/user:alice/**"]
assignments:
  - subject: "did:key:alice"
    roles: [cognition-worker]
"#;

pub struct CognitionFixture<G: GraphCommitStore> {
    vault: MemoryVault<GraphStoreMemoryStore<G>>,
    space: MemorySpace,
    write: Capability<CanWrite, MemorySpace>,
    context: RequestContext,
    pub proposal: CognitionProposal,
}

struct ConfiguredVault<G: GraphCommitStore> {
    vault: MemoryVault<GraphStoreMemoryStore<G>>,
    space: MemorySpace,
    context: RequestContext,
    read: Capability<CanRead, MemorySpace>,
    write: Capability<CanWrite, MemorySpace>,
}

impl<G: GraphCommitStore> CognitionFixture<G> {
    pub fn new(store: GraphStoreMemoryStore<G>, source: StoredRecord, job_id: &str) -> Self {
        Self::customize(store, source, job_id, |_| {})
    }

    pub fn no_change(store: GraphStoreMemoryStore<G>, source: StoredRecord, job_id: &str) -> Self {
        Self::customize(store, source, job_id, |proposal| {
            proposal.effect = CognitionEffect::NoChange;
            proposal.drafts.clear();
            proposal.plan = ConsolidationPlan::new();
        })
    }

    pub fn staged_at(
        store: GraphStoreMemoryStore<G>,
        source: StoredRecord,
        job_id: &str,
        staged_at: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        Self::customize_staged_at(store, source, job_id, staged_at, |_| {})
    }

    pub fn customize(
        store: GraphStoreMemoryStore<G>,
        source: StoredRecord,
        job_id: &str,
        customize: impl FnOnce(&mut CognitionProposal),
    ) -> Self {
        Self::customize_staged_at(store, source, job_id, at(1), customize)
    }

    fn customize_staged_at(
        store: GraphStoreMemoryStore<G>,
        source: StoredRecord,
        job_id: &str,
        staged_at: chrono::DateTime<chrono::Utc>,
        customize: impl FnOnce(&mut CognitionProposal),
    ) -> Self {
        let ConfiguredVault {
            vault,
            space,
            context,
            read,
            write,
        } = configured_vault(store);
        vault.store().put(source.clone()).expect("persist source");
        let governed_source_scope = source.governed_source_scope().cloned();
        let source_ids = std::slice::from_ref(&source.id);
        let manifest = match governed_source_scope.as_ref() {
            Some(scope) => {
                vault.governed_cognition_source_manifest(&space, &read, source_ids, &context, scope)
            }
            None => vault.cognition_source_manifest(&space, &read, source_ids, &context),
        }
        .expect("source manifest");
        let binding = binding(&space, manifest.digest, governed_source_scope);
        let mut proposal = CognitionProposal::new(
            job_id,
            binding.snapshot_digest.clone(),
            binding.source_manifest_digest.clone(),
            "marciana.test",
            "1",
            vec![source.id.clone()],
            Label::Internal,
        )
        .with_plan(
            ConsolidationPlan::new().then(ConsolidationStep::Invalidate {
                ids: vec![source.id.clone()],
            }),
        )
        .with_binding(binding.clone());
        customize(&mut proposal);
        stage_at(vault.store(), source, &proposal, staged_at);
        let vault =
            vault.with_cognition_authority(Arc::new(EchoAuthority::for_proposal(&proposal)));
        Self {
            vault,
            space,
            write,
            context,
            proposal,
        }
    }

    pub fn resume(store: GraphStoreMemoryStore<G>, proposal: CognitionProposal) -> Self {
        let ConfiguredVault {
            vault,
            space,
            context,
            read: _,
            write,
        } = configured_vault(store);
        let vault =
            vault.with_cognition_authority(Arc::new(EchoAuthority::for_proposal(&proposal)));
        Self {
            vault,
            space,
            write,
            context,
            proposal,
        }
    }

    pub fn store(&self) -> &GraphStoreMemoryStore<G> {
        self.vault.store()
    }

    pub fn apply(&self) -> Result<CognitionCommitOutcome, MemoryError> {
        self.vault
            .apply_cognition(&self.space, &self.write, &self.proposal, &self.context)
    }
}

fn configured_vault<G: GraphCommitStore>(store: GraphStoreMemoryStore<G>) -> ConfiguredVault<G> {
    let policy = Arc::new(typesec_rbac::RbacEngine::from_yaml(POLICY).expect("fixture policy"));
    let space = MemorySpace::new("user:alice", "semantic");
    let context = RequestContext::new().with_purpose(PURPOSE);
    let read = capability::<CanRead>(&policy, &space, &context);
    let write = capability::<CanWrite>(&policy, &space, &context);
    ConfiguredVault {
        vault: MemoryVault::new(store).with_policy(policy),
        space,
        context,
        read,
        write,
    }
}

fn capability<P: Permission>(
    policy: &typesec_rbac::RbacEngine,
    space: &MemorySpace,
    context: &RequestContext,
) -> Capability<P, MemorySpace> {
    mint_capability_for_id(
        policy,
        SUBJECT,
        space.resource_id(),
        &MintOptions {
            context: context.clone(),
            ..MintOptions::default()
        },
    )
    .expect("fixture capability")
}

fn binding(
    space: &MemorySpace,
    source_manifest_digest: String,
    governed_source_scope: Option<GovernedSourceScope>,
) -> CognitionBinding {
    CognitionBinding {
        space_id: space.resource_id().to_owned(),
        subject: SUBJECT.into(),
        purpose: PURPOSE.into(),
        governed_source_scope,
        governed_scan_digest: digest("scan"),
        snapshot_digest: digest("snapshot"),
        plan_task_digest: digest("plan"),
        authorization_receipt_digest: digest("authorization"),
        effective_projection: vec!["finding".into()],
        source_manifest_digest,
        typedid_request_digest: digest("typedid"),
    }
}

struct EchoAuthority {
    job_id: String,
    algorithm: String,
    algorithm_version: String,
}

impl EchoAuthority {
    fn for_proposal(proposal: &CognitionProposal) -> Self {
        Self {
            job_id: proposal.job_id.clone(),
            algorithm: proposal.algorithm.clone(),
            algorithm_version: proposal.algorithm_version.clone(),
        }
    }
}

impl CognitionAuthorityVerifier for EchoAuthority {
    fn revalidate(
        &self,
        binding: &CognitionBinding,
        _context: &RequestContext,
    ) -> Result<CognitionAuthorityEvidence, CognitionAuthorityError> {
        Ok(CognitionAuthorityEvidence {
            space_id: binding.space_id.clone(),
            subject: binding.subject.clone(),
            purpose: binding.purpose.clone(),
            governed_source_scope: binding.governed_source_scope.clone(),
            job_id: self.job_id.clone(),
            algorithm: self.algorithm.clone(),
            algorithm_version: self.algorithm_version.clone(),
            governed_scan_digest: binding.governed_scan_digest.clone(),
            snapshot_digest: binding.snapshot_digest.clone(),
            plan_task_digest: binding.plan_task_digest.clone(),
            authorization_receipt_digest: binding.authorization_receipt_digest.clone(),
            effective_projection: binding.effective_projection.clone(),
            typedid_request_digest: binding.typedid_request_digest.clone(),
            policy_decision_id: "policy-decision:cognition-test".into(),
        })
    }
}
