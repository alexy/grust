#![cfg(feature = "turso")]

mod support;

use support::job_key;

use querygraph_memory::TursoMemoryStore;
use querygraph_memory::cognition::CognitionJobStatus;
use typesec_memory::{
    CognitionApplyError, CognitionCommitError, ConsolidationPlan, ConsolidationStep, MemoryError,
    MemoryId, MemoryStore,
};

use support::cognition_vault::CognitionFixture;
use support::{config, record};

#[test]
fn vault_rejects_an_unguarded_plan_before_the_authoritative_commit() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_validation"))
        .expect("open store");
    let source = record("source", "source text", None);
    let mut fixture = CognitionFixture::new(store, source.clone(), "job");
    fixture.proposal.plan = ConsolidationPlan::new().then(ConsolidationStep::Invalidate {
        ids: vec![MemoryId::from_string("not-a-source")],
    });

    assert!(matches!(
        fixture.apply(),
        Err(MemoryError::Cognition(CognitionApplyError::InvalidPlan(_)))
    ));
    assert_uncommitted(&fixture, &source.id);
}

#[test]
fn vault_rejects_a_binding_subject_that_does_not_own_the_capability() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_binding_subject"))
        .expect("open store");
    let source = record("source", "source text", None);
    let mut fixture = CognitionFixture::new(store, source.clone(), "job");
    fixture
        .proposal
        .binding
        .as_mut()
        .expect("proposal binding")
        .subject = "did:key:mallory".into();

    assert!(matches!(
        fixture.apply(),
        Err(MemoryError::Cognition(
            CognitionApplyError::BindingMismatch("subject")
        ))
    ));
    assert_uncommitted(&fixture, &source.id);
}

#[test]
fn prepared_time_cannot_move_the_staged_job_clock_backward() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_monotonic_clock"))
        .expect("open store");
    let source = record("source", "source text", None);
    let fixture = CognitionFixture::staged_at(
        store,
        source.clone(),
        "job",
        chrono::DateTime::parse_from_rfc3339("2100-01-01T00:00:00Z")
            .expect("future timestamp")
            .with_timezone(&chrono::Utc),
    );

    let error = fixture.apply().expect_err("backward job clock is rejected");
    assert!(matches!(
        error,
        MemoryError::CognitionCommit(CognitionCommitError::Store(_))
    ));
    assert_uncommitted(&fixture, &source.id);
}

fn assert_uncommitted<G: grust_core::prelude::GraphCommitStore>(
    fixture: &CognitionFixture<G>,
    source_id: &MemoryId,
) {
    assert_eq!(
        fixture
            .store()
            .get(source_id)
            .expect("read source")
            .expect("source exists")
            .invalid_at,
        None
    );
    assert_eq!(
        fixture
            .store()
            .cognition_job(&job_key("job"))
            .expect("read job")
            .expect("job exists")
            .status,
        CognitionJobStatus::ProposalReady
    );
}
