#![cfg(feature = "turso")]

mod support;

use grust_core::prelude::{GraphAdminStore, GraphCommitStore, Start, Traversal, Value};
use grust_turso::TursoGraphStore;
use querygraph_memory::cognition::CognitionJobStatus;
use querygraph_memory::{GraphStoreMemoryStore, TursoMemoryStore};
use typesec_memory::{
    CognitionCommitOutcome, CognitionCommitStatus, GovernedSourceScope, MemoryId, MemoryStore,
};

use support::cognition_vault::CognitionFixture;
use support::guarded_fault::{FaultControl, FaultingStore, GuardedFault};
use support::{config, digest, governed_record, job_key, record};

const LOST_RESPONSE_SECRET: &str = "protected commit response";

#[tokio::test]
async fn commit_then_response_loss_recovers_once_on_retry_and_reopen() {
    let dir = tempfile::tempdir().expect("temporary database");
    let prefix = "cognition_lost_response";
    let (retry, proposal) = {
        let (store, control) = faulting_store(&dir, prefix).await;
        let fixture = CognitionFixture::new(store, record("source", "source text", None), "job");
        control.set(GuardedFault::CommitThenResponseLoss {
            prefix: "cognition-commit:".into(),
            secret: LOST_RESPONSE_SECRET.into(),
        });

        let error = fixture
            .apply()
            .expect_err("backend loses the successful commit response");
        assert!(!error.to_string().contains(LOST_RESPONSE_SECRET));
        let calls_after_commit = control.commit_calls();

        control.set(GuardedFault::Pass);
        let retry = fixture.apply().expect("retry recovers committed outcome");
        assert_eq!(retry.status, CognitionCommitStatus::AlreadyApplied);
        assert!(retry.audit.governed_source_scope.is_none());
        assert_eq!(control.commit_calls(), calls_after_commit);
        assert_single_durable_commit(fixture.store(), &MemoryId::from_string("source")).await;
        (retry, fixture.proposal.clone())
    };

    let (store, reopen_control) = faulting_store(&dir, prefix).await;
    let reopened = CognitionFixture::resume(store, proposal);
    let recovered = reopened.apply().expect("reopen recovers committed outcome");
    assert_eq!(recovered.status, CognitionCommitStatus::AlreadyApplied);
    assert_eq!(recovered, retry);
    assert_eq!(reopen_control.commit_calls(), 0);
    assert_single_durable_commit(reopened.store(), &MemoryId::from_string("source")).await;
}

#[tokio::test]
async fn governed_scope_survives_atomic_commit_and_reopen() {
    let dir = tempfile::tempdir().expect("temporary database");
    let prefix = "cognition_governed_scope";
    let scope = GovernedSourceScope::from_digest(digest("governed-source"))
        .expect("canonical governed source scope");
    let (first, proposal) = {
        let (store, _) = faulting_store(&dir, prefix).await;
        let source = governed_record("source", "source text", None, &scope);
        let fixture = CognitionFixture::new(store, source, "job");
        assert_eq!(
            fixture.proposal.schema_version,
            typesec_memory::CognitionProposal::SCHEMA_VERSION
        );
        let first = fixture.apply().expect("commit governed cognition");
        assert_eq!(first.status, CognitionCommitStatus::Applied);
        assert_eq!(first.audit.governed_source_scope.as_ref(), Some(&scope));
        assert_single_durable_commit(fixture.store(), &MemoryId::from_string("source")).await;
        (first, fixture.proposal.clone())
    };

    let (store, _) = faulting_store(&dir, prefix).await;
    let reopened = CognitionFixture::resume(store, proposal);
    let recovered = reopened
        .apply()
        .expect("recover governed cognition after reopen");
    assert_eq!(recovered.status, CognitionCommitStatus::AlreadyApplied);
    assert_eq!(recovered.audit.governed_source_scope.as_ref(), Some(&scope));
    assert_same_commit(&first, &recovered);
    assert_single_durable_commit(reopened.store(), &MemoryId::from_string("source")).await;
}

#[tokio::test]
async fn current_wire_shape_matches_checked_in_golden_after_reopen() {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/cognition_wire_current.json"))
            .expect("current cognition wire golden");
    let dir = tempfile::tempdir().expect("temporary database");
    let config = config(&dir, "cognition_current_wire_golden");
    let proposal = {
        let store = TursoMemoryStore::open_with_config(config.clone()).expect("open store");
        let fixture = CognitionFixture::new(store, record("source", "source text", None), "job");
        fixture.apply().expect("commit current cognition wire");
        fixture.proposal.clone()
    };

    let reopened = CognitionFixture::resume(
        TursoMemoryStore::open_with_config(config).expect("reopen store"),
        proposal,
    );
    let recovered = reopened.apply().expect("recover current cognition wire");
    assert_eq!(recovered.status, CognitionCommitStatus::AlreadyApplied);
    assert_eq!(
        u64::from(reopened.proposal.schema_version),
        golden["proposalSchemaVersion"]
            .as_u64()
            .expect("golden proposal schema version")
    );

    let audit = only_payload(reopened.store(), "CognitionAudit").await;
    let job = only_payload(reopened.store(), "CognitionJob").await;
    let outcome = only_payload(reopened.store(), "CognitionOutcome").await;
    assert_wire_shape(&audit, &golden, "auditSchemaVersion", "auditFields");
    assert_wire_shape(&job, &golden, "jobSchemaVersion", "jobFields");
    assert_wire_shape(&outcome, &golden, "outcomeSchemaVersion", "outcomeFields");
    assert_eq!(job["completionDigest"], outcome["preparedDigest"]);
    assert_ne!(job["completionDigest"], outcome["resultingVersion"]);
}

#[tokio::test]
async fn regressive_and_invalid_receipt_times_fail_closed_after_commit_and_reopen() {
    let dir = tempfile::tempdir().expect("temporary database");
    for (case, receipt_time, expected) in [
        (
            "regressive",
            "2000-01-01T00:00:00Z",
            "cognition backend commit timestamp predates preparation",
        ),
        (
            "invalid",
            "not-a-backend-timestamp",
            "cognition backend commit timestamp is not canonical RFC 3339",
        ),
    ] {
        let prefix = format!("cognition_receipt_{case}");
        let proposal = {
            let (store, control) = faulting_store(&dir, &prefix).await;
            control.set(receipt_timestamp_fault(receipt_time));
            let fixture =
                CognitionFixture::new(store, record("source", "source text", None), "job");
            let error = fixture
                .apply()
                .expect_err("untrustworthy initial backend time must fail closed");
            assert!(error.to_string().contains(expected));
            assert!(!error.to_string().contains(receipt_time));
            assert_single_durable_commit(fixture.store(), &MemoryId::from_string("source")).await;
            fixture.proposal.clone()
        };

        let (store, control) = faulting_store(&dir, &prefix).await;
        control.set(receipt_timestamp_fault(receipt_time));
        let reopened = CognitionFixture::resume(store, proposal);
        let error = reopened
            .apply()
            .expect_err("untrustworthy recovered backend time must fail closed");
        assert!(error.to_string().contains(expected));
        assert!(!error.to_string().contains(receipt_time));
        assert_eq!(control.commit_calls(), 0);
        assert_single_durable_commit(reopened.store(), &MemoryId::from_string("source")).await;
    }
}

async fn assert_single_durable_commit<G: GraphCommitStore>(
    store: &GraphStoreMemoryStore<G>,
    source_id: &MemoryId,
) {
    let source = store
        .get(source_id)
        .expect("read source")
        .expect("source exists");
    assert!(source.invalid_at.is_some());
    assert_eq!(
        store
            .cognition_job(&job_key("job"))
            .expect("read cognition job")
            .expect("cognition job exists")
            .status,
        CognitionJobStatus::Completed
    );
    for label in [
        "MemoryRecord",
        "CognitionJob",
        "CognitionIndexOutbox",
        "CognitionAudit",
        "CognitionOutcome",
    ] {
        let nodes = store
            .graph()
            .traverse(Traversal {
                start: Start::NodesByLabel(label.into()),
                steps: Vec::new(),
                limit: None,
            })
            .await
            .expect("read durable commit nodes");
        assert_eq!(nodes.len(), 1, "unexpected {label} count");
    }
}

fn assert_same_commit(first: &CognitionCommitOutcome, recovered: &CognitionCommitOutcome) {
    let mut recovered = recovered.clone();
    recovered.status = CognitionCommitStatus::Applied;
    assert_eq!(&recovered, first);
    assert_eq!(
        serde_json::to_vec(&recovered).expect("serialize recovered outcome"),
        serde_json::to_vec(first).expect("serialize applied outcome")
    );
}

async fn faulting_store(
    dir: &tempfile::TempDir,
    prefix: &str,
) -> (GraphStoreMemoryStore<FaultingStore>, FaultControl) {
    let graph = TursoGraphStore::connect(config(dir, prefix))
        .await
        .expect("connect graph");
    graph.bootstrap().await.expect("bootstrap graph");
    let (graph, control) = FaultingStore::new(graph);
    (GraphStoreMemoryStore::new(graph), control)
}

fn receipt_timestamp_fault(value: &str) -> GuardedFault {
    GuardedFault::ReceiptTimestamp {
        prefix: "cognition-commit:".into(),
        value: value.into(),
    }
}

async fn only_payload<G: GraphCommitStore>(
    store: &GraphStoreMemoryStore<G>,
    label: &str,
) -> serde_json::Value {
    let mut nodes = store
        .graph()
        .traverse(Traversal {
            start: Start::NodesByLabel(label.into()),
            steps: Vec::new(),
            limit: None,
        })
        .await
        .expect("read current cognition wire");
    assert_eq!(nodes.len(), 1, "expected one {label} node");
    let node = nodes.pop().expect("current cognition node");
    let Some(Value::Json(payload)) = node.props.get("payload") else {
        panic!("current cognition node has JSON payload")
    };
    payload.clone()
}

fn assert_wire_shape(
    payload: &serde_json::Value,
    golden: &serde_json::Value,
    version_key: &str,
    fields_key: &str,
) {
    assert_eq!(payload["schemaVersion"], golden[version_key]);
    let mut fields = payload
        .as_object()
        .expect("cognition payload object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    fields.sort();
    let expected: Vec<String> =
        serde_json::from_value(golden[fields_key].clone()).expect("golden field list");
    assert_eq!(fields, expected);
}
