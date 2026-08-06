mod support;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use querygraph_memory::cognition::{
    CognitionEngine, CognitionEngineProfile, CognitionError, CognitionFieldMapping,
    CognitionOperation, CognitionRequest, GovernedLakeCatSnapshot,
    MAX_COGNITION_AUTHORIZED_INPUT_BYTES, MAX_COGNITION_IDENTITY_BYTES, ReferenceCognitionEngine,
    SailCognitionEngine, SailCognitionExecutor, SailCognitionExecutorError, SailCognitionOutput,
};
use typesec_memory::{
    CognitionBinding, CognitionEffect, ConsolidationPlan, ConsolidationStep, GovernedSourceScope,
    Label, MAX_COGNITION_EVIDENCE_ITEMS, MemoryId, StoredRecord,
};

use support::cognition_input::{authorized_input, governed_authorized_input_for};
use support::{digest, governed_record, record};

fn source() -> GovernedLakeCatSnapshot {
    GovernedLakeCatSnapshot {
        catalog: "lakecat://prod".into(),
        namespace: "research".into(),
        table: "findings".into(),
        snapshot_id: 42,
        governed_scan_digest: digest("scan"),
        snapshot_digest: digest("snapshot"),
        plan_task_digest: digest("plan"),
        subject: "did:key:researcher".into(),
        purpose: "research".into(),
        effective_projection: vec!["id".into(), "finding".into(), "valid_from".into()],
        authorization_receipt_digest: digest("receipt"),
    }
}

fn binding(
    source: &GovernedLakeCatSnapshot,
    input: &typesec_memory::AuthorizedCognitionInput,
) -> CognitionBinding {
    CognitionBinding {
        space_id: "memory/user:alice/semantic".into(),
        subject: source.subject.clone(),
        purpose: source.purpose.clone(),
        governed_source_scope: input.governed_source_scope().cloned(),
        governed_scan_digest: source.governed_scan_digest.clone(),
        snapshot_digest: source.snapshot_digest.clone(),
        plan_task_digest: source.plan_task_digest.clone(),
        authorization_receipt_digest: source.authorization_receipt_digest.clone(),
        effective_projection: source.effective_projection.clone(),
        source_manifest_digest: input.manifest().digest.clone(),
        typedid_request_digest: digest("typedid"),
    }
}

fn mapping(text: &str) -> CognitionFieldMapping {
    CognitionFieldMapping {
        id: "id".into(),
        text: text.into(),
        valid_from: "valid_from".into(),
    }
}

fn labeled_record(id: &str, text: &str, label: Label) -> StoredRecord {
    let mut encoded = serde_json::to_value(record(id, text, None)).expect("serialize record");
    encoded["label"] = serde_json::json!(label.name());
    serde_json::from_value(encoded).expect("deserialize labeled record")
}

struct CountingSailExecutor {
    calls: Arc<AtomicUsize>,
}

struct AdversarialErrorExecutor {
    dynamic_backend_text: String,
}

struct AdversarialOutputExecutor {
    plan: ConsolidationPlan,
    evidence: Vec<String>,
}

#[async_trait::async_trait]
impl SailCognitionExecutor for AdversarialErrorExecutor {
    async fn execute(
        &self,
        _request: &CognitionRequest<'_>,
    ) -> Result<SailCognitionOutput, SailCognitionExecutorError> {
        let _dynamic_text_stays_private = &self.dynamic_backend_text;
        Err(SailCognitionExecutorError::ExecutionFailed)
    }
}

#[async_trait::async_trait]
impl SailCognitionExecutor for AdversarialOutputExecutor {
    async fn execute(
        &self,
        _request: &CognitionRequest<'_>,
    ) -> Result<SailCognitionOutput, SailCognitionExecutorError> {
        Ok(SailCognitionOutput {
            plan: self.plan.clone(),
            evidence: self.evidence.clone(),
        })
    }
}

#[async_trait::async_trait]
impl SailCognitionExecutor for CountingSailExecutor {
    async fn execute(
        &self,
        _request: &CognitionRequest<'_>,
    ) -> Result<SailCognitionOutput, SailCognitionExecutorError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(SailCognitionOutput {
            plan: ConsolidationPlan::new(),
            evidence: Vec::new(),
        })
    }
}

#[test]
fn native_host_profiles_are_exact_and_available_before_execution() {
    let calls = Arc::new(AtomicUsize::new(0));
    let _sail = SailCognitionEngine::new(CountingSailExecutor {
        calls: Arc::clone(&calls),
    });

    for (operation, operation_name) in [
        (CognitionOperation::Deduplicate, "marciana.deduplicate"),
        (CognitionOperation::Reconcile, "marciana.reconcile"),
    ] {
        let reference = CognitionEngineProfile::reference(operation);
        assert_eq!(reference.algorithm(), format!("{operation_name}.reference"));
        assert_eq!(reference.algorithm_version(), "2");
        assert!(reference.matches(reference.algorithm(), "2"));
        assert!(!reference.matches(reference.algorithm(), "1"));

        let distributed = CognitionEngineProfile::sail(operation);
        assert_eq!(distributed.algorithm(), format!("{operation_name}.sail"));
        assert_eq!(distributed.algorithm_version(), "2");
        assert!(!distributed.matches(distributed.algorithm(), "0.12.0"));
        assert!(!distributed.matches(distributed.algorithm(), "forged-version"));
    }
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn reference_and_sail_proposals_agree_on_typed_effect() {
    for operation in [
        CognitionOperation::Deduplicate,
        CognitionOperation::Reconcile,
    ] {
        assert_effect_parity(
            vec![record("only", "one unique assertion", None)],
            operation,
            CognitionEffect::NoChange,
        )
        .await;
    }
    assert_effect_parity(
        vec![
            record("left", "Alice likes espresso", None),
            record("right", " alice  likes  espresso ", None),
        ],
        CognitionOperation::Deduplicate,
        CognitionEffect::Mutated,
    )
    .await;
    assert_effect_parity(
        vec![
            record("older", "project status closed", None),
            record("newer", "project status open", None),
        ],
        CognitionOperation::Reconcile,
        CognitionEffect::Mutated,
    )
    .await;
}

#[tokio::test]
async fn proposal_is_born_with_canonical_binding_manifest_and_label_join() {
    let source = source();
    let input = authorized_input(vec![
        labeled_record("m1", "Alice likes espresso", Label::Internal),
        labeled_record("m2", "alice likes espresso", Label::Sensitive),
    ]);
    let binding = binding(&source, &input);
    let mapping = mapping("finding");
    let proposal = ReferenceCognitionEngine
        .propose(CognitionRequest {
            job_id: "job-42",
            source: &source,
            binding: &binding,
            input: &input,
            field_mapping: &mapping,
            operation: CognitionOperation::Deduplicate,
        })
        .await
        .expect("bound proposal");
    assert_eq!(proposal.joined_label, Label::Sensitive);
    assert_eq!(proposal.plan.steps.len(), 1);
    assert_eq!(proposal.input_snapshot, binding.snapshot_digest);
    assert_eq!(proposal.source_digest, input.manifest().digest);
    assert_eq!(
        proposal.schema_version,
        typesec_memory::CognitionProposal::SCHEMA_VERSION
    );
    assert_eq!(proposal.binding.as_ref(), Some(&binding));
    assert!(
        proposal
            .binding
            .as_ref()
            .expect("proposal remains bound")
            .governed_source_scope
            .is_none()
    );
}

#[tokio::test]
async fn governed_scope_is_preserved_in_the_born_bound_proposal() {
    let source = source();
    let scope = GovernedSourceScope::from_digest(digest("governed-source"))
        .expect("canonical governed source scope");
    let input = governed_authorized_input_for(
        vec![
            governed_record("m1", "Alice likes espresso", None, &scope),
            governed_record("m2", "alice likes espresso", None, &scope),
        ],
        "research",
        &scope,
    );
    let binding = binding(&source, &input);
    let mapping = mapping("finding");
    let proposal = ReferenceCognitionEngine
        .propose(CognitionRequest {
            job_id: "job-governed",
            source: &source,
            binding: &binding,
            input: &input,
            field_mapping: &mapping,
            operation: CognitionOperation::Deduplicate,
        })
        .await
        .expect("governed bound proposal");

    assert_eq!(input.governed_source_scope(), Some(&scope));
    assert_eq!(
        proposal.schema_version,
        typesec_memory::CognitionProposal::SCHEMA_VERSION
    );
    assert_eq!(
        proposal
            .binding
            .as_ref()
            .and_then(|binding| binding.governed_source_scope.as_ref()),
        Some(&scope)
    );
}

#[tokio::test]
async fn governed_scope_mismatch_fails_before_planning() {
    let source = source();
    let mapping = mapping("finding");
    let scope = GovernedSourceScope::from_digest(digest("governed-source"))
        .expect("canonical governed source scope");

    let local_input = authorized_input(vec![record("local", "local source", None)]);
    let mut scoped_binding = binding(&source, &local_input);
    scoped_binding.governed_source_scope = Some(scope.clone());
    assert!(matches!(
        ReferenceCognitionEngine
            .propose(CognitionRequest {
                job_id: "job-local-scope-mismatch",
                source: &source,
                binding: &scoped_binding,
                input: &local_input,
                field_mapping: &mapping,
                operation: CognitionOperation::Deduplicate,
            })
            .await,
        Err(CognitionError::BindingMismatch("governed source scope"))
    ));

    let governed_input = governed_authorized_input_for(
        vec![governed_record("governed", "governed source", None, &scope)],
        "research",
        &scope,
    );
    let mut local_binding = binding(&source, &governed_input);
    local_binding.governed_source_scope = None;
    assert!(matches!(
        ReferenceCognitionEngine
            .propose(CognitionRequest {
                job_id: "job-governed-scope-mismatch",
                source: &source,
                binding: &local_binding,
                input: &governed_input,
                field_mapping: &mapping,
                operation: CognitionOperation::Deduplicate,
            })
            .await,
        Err(CognitionError::BindingMismatch("governed source scope"))
    ));
}

#[tokio::test]
async fn noncanonical_job_ids_never_reach_the_sail_executor() {
    let source = source();
    let input = authorized_input(vec![record("m1", "Alice likes espresso", None)]);
    let binding = binding(&source, &input);
    let mapping = mapping("finding");
    let calls = Arc::new(AtomicUsize::new(0));
    let engine = SailCognitionEngine::new(CountingSailExecutor {
        calls: Arc::clone(&calls),
    });

    let exact = "j".repeat(MAX_COGNITION_IDENTITY_BYTES);
    engine
        .propose(CognitionRequest {
            job_id: &exact,
            source: &source,
            binding: &binding,
            input: &input,
            field_mapping: &mapping,
            operation: CognitionOperation::Deduplicate,
        })
        .await
        .expect("inclusive job identity limit");
    assert_eq!(calls.load(Ordering::Relaxed), 1);

    for invalid in [
        "job\nprotected".to_owned(),
        "j".repeat(MAX_COGNITION_IDENTITY_BYTES + 1),
    ] {
        assert!(matches!(
            engine
                .propose(CognitionRequest {
                    job_id: &invalid,
                    source: &source,
                    binding: &binding,
                    input: &input,
                    field_mapping: &mapping,
                    operation: CognitionOperation::Deduplicate,
                })
                .await,
            Err(CognitionError::InvalidJobId)
        ));
    }
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn external_executor_cannot_inject_dynamic_backend_text() {
    const SECRET: &str = "dynamic backend response with protected source text";
    let source = source();
    let input = authorized_input(vec![record("m1", "Alice likes espresso", None)]);
    let binding = binding(&source, &input);
    let mapping = mapping("finding");
    let error = SailCognitionEngine::new(AdversarialErrorExecutor {
        dynamic_backend_text: SECRET.into(),
    })
    .propose(CognitionRequest {
        job_id: "job-executor-error",
        source: &source,
        binding: &binding,
        input: &input,
        field_mapping: &mapping,
        operation: CognitionOperation::Deduplicate,
    })
    .await
    .expect_err("external executor fails");

    assert_eq!(
        error.to_string(),
        "Sail cognition failed: Sail executor failed"
    );
    assert!(!error.to_string().contains(SECRET));
}

#[tokio::test]
async fn executor_out_of_source_mutation_is_rejected_without_echoing_text() {
    const SOURCE_SECRET: &str = "private source assertion from the vault";
    const ATTACKER_TARGET: &str = "attacker-controlled/out-of-source-target";
    let plan = ConsolidationPlan::new().then(ConsolidationStep::Invalidate {
        ids: vec![MemoryId::from_string(ATTACKER_TARGET)],
    });

    let error = reject_executor_output(plan, Vec::new(), SOURCE_SECRET).await;

    assert!(matches!(&error, CognitionError::InvalidExecutorOutput));
    assert_eq!(
        error.to_string(),
        "cognition executor returned invalid proposal output"
    );
    assert!(!error.to_string().contains(SOURCE_SECRET));
    assert!(!error.to_string().contains(ATTACKER_TARGET));
}

#[tokio::test]
async fn executor_over_budget_evidence_is_rejected_without_echoing_text() {
    const SOURCE_SECRET: &str = "private source text for evidence attack";
    const ATTACKER_EVIDENCE: &str = "attacker-controlled evidence with source-like text";
    let evidence = vec![ATTACKER_EVIDENCE.to_owned(); MAX_COGNITION_EVIDENCE_ITEMS + 1];

    let error = reject_executor_output(ConsolidationPlan::new(), evidence, SOURCE_SECRET).await;

    assert!(matches!(&error, CognitionError::InvalidExecutorOutput));
    assert_eq!(
        error.to_string(),
        "cognition executor returned invalid proposal output"
    );
    assert!(!error.to_string().contains(SOURCE_SECRET));
    assert!(!error.to_string().contains(ATTACKER_EVIDENCE));
}

async fn reject_executor_output(
    plan: ConsolidationPlan,
    evidence: Vec<String>,
    source_text: &str,
) -> CognitionError {
    let source = source();
    let input = authorized_input(vec![record("m1", source_text, None)]);
    let binding = binding(&source, &input);
    let mapping = mapping("finding");
    SailCognitionEngine::new(AdversarialOutputExecutor { plan, evidence })
        .propose(CognitionRequest {
            job_id: "job-adversarial-output",
            source: &source,
            binding: &binding,
            input: &input,
            field_mapping: &mapping,
            operation: CognitionOperation::Deduplicate,
        })
        .await
        .expect_err("executor output is not a canonical TypeSec proposal")
}

#[tokio::test]
async fn reference_engine_accepts_large_input_prebounded_by_type_sec() {
    const ID: &str = "budget-source";
    // TypeSec also bounds the complete serialized authorization envelope, so
    // an opaque AuthorizedCognitionInput cannot reach Grust's independent
    // ID+text ceiling. The exact 4 MiB/+1 engine budget is tested at its shared
    // CognitionSourceBudget seam; this exercises a large valid opaque input.
    let text = "x".repeat(MAX_COGNITION_AUTHORIZED_INPUT_BYTES / 2 - ID.len());
    let source = source();
    let input = authorized_input(vec![record(ID, &text, None)]);
    let binding = binding(&source, &input);
    let mapping = mapping("finding");
    ReferenceCognitionEngine
        .propose(CognitionRequest {
            job_id: "job-byte-budget",
            source: &source,
            binding: &binding,
            input: &input,
            field_mapping: &mapping,
            operation: CognitionOperation::Deduplicate,
        })
        .await
        .expect("large bounded authorized input");
}

#[tokio::test]
async fn unauthorized_staged_field_fails_before_planning() {
    let source = source();
    let input = authorized_input(vec![record("m1", "Alice likes espresso", None)]);
    let binding = binding(&source, &input);
    let mapping = mapping("unapproved_private_text");
    assert!(matches!(
        ReferenceCognitionEngine
            .propose(CognitionRequest {
                job_id: "job-42",
                source: &source,
                binding: &binding,
                input: &input,
                field_mapping: &mapping,
                operation: CognitionOperation::Deduplicate,
            })
            .await,
        Err(CognitionError::ProjectionDenied)
    ));
}

#[tokio::test]
async fn one_column_cannot_supply_multiple_staged_semantics() {
    let source = source();
    let input = authorized_input(vec![record("m1", "Alice likes espresso", None)]);
    let binding = binding(&source, &input);
    let mapping = CognitionFieldMapping {
        id: "finding".into(),
        text: "finding".into(),
        valid_from: "finding".into(),
    };
    assert!(matches!(
        ReferenceCognitionEngine
            .propose(CognitionRequest {
                job_id: "job-42",
                source: &source,
                binding: &binding,
                input: &input,
                field_mapping: &mapping,
                operation: CognitionOperation::Deduplicate,
            })
            .await,
        Err(CognitionError::InvalidSnapshot(field)) if field.contains("duplicate")
    ));
}

#[test]
fn malformed_snapshot_and_empty_projection_fail_closed() {
    let mut source = source();
    source.effective_projection.clear();
    assert!(matches!(
        source.digest(),
        Err(CognitionError::InvalidSnapshot(_))
    ));
    source.effective_projection.push("finding".into());
    source.snapshot_digest = "snapshot 42".into();
    assert!(matches!(
        source.digest(),
        Err(CognitionError::InvalidSnapshot(_))
    ));
}

async fn assert_effect_parity(
    records: Vec<StoredRecord>,
    operation: CognitionOperation,
    expected: CognitionEffect,
) {
    let source = source();
    let input = authorized_input(records);
    let binding = binding(&source, &input);
    let mapping = mapping("finding");
    let reference = ReferenceCognitionEngine
        .propose(CognitionRequest {
            job_id: "effect-parity",
            source: &source,
            binding: &binding,
            input: &input,
            field_mapping: &mapping,
            operation,
        })
        .await
        .expect("reference proposal");
    let sail = SailCognitionEngine::new(AdversarialOutputExecutor {
        plan: reference.plan.clone(),
        evidence: Vec::new(),
    })
    .propose(CognitionRequest {
        job_id: "effect-parity",
        source: &source,
        binding: &binding,
        input: &input,
        field_mapping: &mapping,
        operation,
    })
    .await
    .expect("Sail proposal");

    assert_eq!(reference.effect, expected);
    assert_eq!(sail.effect, expected);
    assert_eq!(
        serde_json::to_value(&reference.plan).expect("serialize reference plan"),
        serde_json::to_value(&sail.plan).expect("serialize Sail plan")
    );
    assert!(reference.drafts.is_empty());
    assert!(sail.drafts.is_empty());
    assert_eq!(
        reference.plan.steps.is_empty(),
        expected == CognitionEffect::NoChange
    );
}
