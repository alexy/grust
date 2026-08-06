#![cfg(feature = "turso")]

mod support;

use support::job_key;

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use grust_core::prelude::{
    Edge, EdgeQuery, Graph, GraphAdminStore, GraphCommitReceipt, GraphCommitStore,
    GraphExpectation, GraphMutation, GraphMutationAtomicity, GraphMutationStore, GraphStore,
    GuardedGraphCommit, LoadReport, Node, NodeId, PutOutcome, Result as GraphResult, Start,
    Traversal, Value,
};
use grust_turso::TursoGraphStore;
use querygraph_memory::cognition::CognitionJobStatus;
use querygraph_memory::{GraphStoreMemoryStore, TursoMemoryStore};
use typesec_memory::{
    CognitionCommitError, CognitionCommitStatus, ConsolidationPlan, ConsolidationStep,
    MemoryContent, MemoryDraft, MemoryError, MemoryId, MemoryStore, Provenance,
};

use support::cognition_vault::CognitionFixture;
use support::{at, config, record};

#[tokio::test]
async fn identical_commit_after_initial_recovery_wins_over_its_own_stale_source() {
    let dir = tempfile::tempdir().expect("temporary database");
    let config = config(&dir, "cognition_source_recovery_race");
    let graph = TursoGraphStore::connect(config.clone())
        .await
        .expect("connect graph");
    graph.bootstrap().await.expect("bootstrap graph");
    let fixture = Arc::new(CognitionFixture::new(
        GraphStoreMemoryStore::new(RacingStore::new(graph)),
        record("source", "source text", None),
        "job",
    ));
    let peer = CognitionFixture::resume(
        TursoMemoryStore::open_with_config(config).expect("open peer store"),
        fixture.proposal.clone(),
    );
    fixture
        .store()
        .graph()
        .arm_source_read(NodeId::from("rec:source"), 2);

    let delayed = {
        let fixture = Arc::clone(&fixture);
        std::thread::spawn(move || fixture.apply())
    };
    fixture.store().graph().wait_for_blocked_source_read();
    let winner = peer.apply();
    fixture.store().graph().release_source_read();

    let winner = winner.expect("peer commits while source read is delayed");
    let recovered = delayed
        .join()
        .expect("delayed worker")
        .expect("delayed worker recovers the identical commit");
    assert_eq!(winner.status, CognitionCommitStatus::Applied);
    assert_eq!(recovered.status, CognitionCommitStatus::AlreadyApplied);
    let mut recovered_as_applied = recovered;
    recovered_as_applied.status = CognitionCommitStatus::Applied;
    assert_eq!(recovered_as_applied, winner);
}

#[tokio::test]
async fn exact_source_guard_rolls_back_every_cognition_side_effect_on_race() {
    let dir = tempfile::tempdir().expect("temporary database");
    let graph = TursoGraphStore::connect(config(&dir, "cognition_source_race"))
        .await
        .expect("connect graph");
    graph.bootstrap().await.expect("bootstrap graph");
    let source = record("source", "source text", None);
    let fixture = CognitionFixture::new(
        GraphStoreMemoryStore::new(RacingStore::new(graph)),
        source.clone(),
        "job",
    );

    let mut raced = fixture
        .store()
        .graph()
        .get_node(&NodeId::from("rec:source"))
        .await
        .expect("read source node")
        .expect("source exists");
    let Value::Json(record) = raced.props.get_mut("record").expect("record payload") else {
        panic!("record payload is JSON")
    };
    record["invalid_at"] = serde_json::to_value(at(9)).expect("race timestamp");
    fixture.store().graph().arm_source(raced);

    assert!(matches!(
        fixture.apply(),
        Err(MemoryError::CognitionCommit(CognitionCommitError::StaleSource(id)))
            if id == source.id
    ));
    let raced_source = fixture
        .store()
        .get(&source.id)
        .expect("read raced source")
        .expect("source remains");
    assert_eq!(raced_source.invalid_at, Some(at(9)));
    assert_eq!(
        fixture
            .store()
            .cognition_job(&job_key("job"))
            .expect("read job")
            .expect("job exists")
            .status,
        CognitionJobStatus::ProposalReady
    );
    for label in ["CognitionIndexOutbox", "CognitionAudit", "CognitionOutcome"] {
        assert!(
            fixture
                .store()
                .graph()
                .traverse(Traversal {
                    start: Start::NodesByLabel(label.into()),
                    steps: Vec::new(),
                    limit: None,
                })
                .await
                .expect("read graph")
                .is_empty(),
            "{label} must roll back"
        );
    }
}

#[tokio::test]
async fn deterministic_output_collision_cannot_overwrite_or_partially_commit() {
    let dir = tempfile::tempdir().expect("temporary database");
    let graph = TursoGraphStore::connect(config(&dir, "cognition_output_collision"))
        .await
        .expect("connect graph");
    graph.bootstrap().await.expect("bootstrap graph");
    let source = record("source", "source text", None);
    let fixture = CognitionFixture::customize(
        GraphStoreMemoryStore::new(RacingStore::new(graph)),
        source.clone(),
        "job",
        |proposal| {
            proposal.plan = ConsolidationPlan::new().then(ConsolidationStep::Supersede {
                superseded: vec![source.id.clone()],
                replacement: MemoryDraft::new(
                    source.kind,
                    MemoryContent::text("attempted overwrite"),
                    Provenance::Operator,
                ),
            });
        },
    );
    let collided_id = fixture.store().graph().arm_output_collision();

    assert!(fixture.apply().is_err());
    assert_eq!(
        fixture
            .store()
            .get(&source.id)
            .expect("read source")
            .expect("source remains")
            .invalid_at,
        None
    );
    let target_id = MemoryId::from_string(
        collided_id
            .lock()
            .expect("collision id lock")
            .clone()
            .expect("collision target id"),
    );
    let target = fixture
        .store()
        .get(&target_id)
        .expect("read target")
        .expect("target remains");
    let encoded = serde_json::to_string(&target).expect("serialize target");
    assert!(encoded.contains("original target text"));
    assert!(!encoded.contains("attempted overwrite"));
    assert_eq!(
        fixture
            .store()
            .cognition_job(&job_key("job"))
            .expect("read job")
            .expect("job exists")
            .status,
        CognitionJobStatus::ProposalReady
    );
    for label in ["CognitionIndexOutbox", "CognitionAudit", "CognitionOutcome"] {
        assert!(
            fixture
                .store()
                .graph()
                .traverse(Traversal {
                    start: Start::NodesByLabel(label.into()),
                    steps: Vec::new(),
                    limit: None,
                })
                .await
                .expect("read graph")
                .is_empty()
        );
    }
}

struct RacingStore {
    inner: TursoGraphStore,
    race: Mutex<Option<Race>>,
    source_reads: SourceReadGate,
}

enum Race {
    Source(Node),
    Output(Arc<Mutex<Option<String>>>),
}

impl RacingStore {
    fn new(inner: TursoGraphStore) -> Self {
        Self {
            inner,
            race: Mutex::new(None),
            source_reads: SourceReadGate::default(),
        }
    }

    fn arm_source_read(&self, target: NodeId, ordinal: usize) {
        self.source_reads.arm(target, ordinal);
    }

    fn wait_for_blocked_source_read(&self) {
        self.source_reads.wait_until_blocked();
    }

    fn release_source_read(&self) {
        self.source_reads.release();
    }

    fn arm_source(&self, replacement: Node) {
        *self.race.lock().expect("race lock") = Some(Race::Source(replacement));
    }

    fn arm_output_collision(&self) -> Arc<Mutex<Option<String>>> {
        let output = Arc::new(Mutex::new(None));
        *self.race.lock().expect("race lock") = Some(Race::Output(Arc::clone(&output)));
        output
    }
}

#[derive(Default)]
struct SourceReadGate {
    state: Mutex<SourceReadState>,
    changed: Condvar,
}

#[derive(Default)]
struct SourceReadState {
    target: Option<NodeId>,
    remaining: usize,
    blocked: bool,
    released: bool,
}

impl SourceReadGate {
    fn arm(&self, target: NodeId, ordinal: usize) {
        assert!(ordinal > 0, "source read ordinal is one-based");
        *self.state.lock().expect("source read gate") = SourceReadState {
            target: Some(target),
            remaining: ordinal,
            blocked: false,
            released: false,
        };
    }

    fn maybe_block(&self, id: &NodeId) {
        let mut state = self.state.lock().expect("source read gate");
        if state.target.as_ref() != Some(id) || state.remaining == 0 {
            return;
        }
        state.remaining -= 1;
        if state.remaining != 0 {
            return;
        }
        state.blocked = true;
        self.changed.notify_all();
        while !state.released {
            state = self.changed.wait(state).expect("source read gate");
        }
    }

    fn wait_until_blocked(&self) {
        let state = self.state.lock().expect("source read gate");
        let (state, _) = self
            .changed
            .wait_timeout_while(state, Duration::from_secs(10), |state| !state.blocked)
            .expect("source read gate");
        assert!(state.blocked, "timed out waiting for delayed source read");
    }

    fn release(&self) {
        self.state.lock().expect("source read gate").released = true;
        self.changed.notify_all();
    }
}

#[async_trait]
impl GraphStore for RacingStore {
    async fn put_node(&self, node: &Node) -> GraphResult<PutOutcome> {
        self.inner.put_node(node).await
    }

    async fn put_edge(&self, edge: &Edge) -> GraphResult<PutOutcome> {
        self.inner.put_edge(edge).await
    }

    async fn put_graph(&self, graph: &Graph) -> GraphResult<LoadReport> {
        self.inner.put_graph(graph).await
    }

    async fn get_node(&self, id: &NodeId) -> GraphResult<Option<Node>> {
        self.source_reads.maybe_block(id);
        self.inner.get_node(id).await
    }

    async fn get_edges(&self, query: EdgeQuery) -> GraphResult<Vec<Edge>> {
        self.inner.get_edges(query).await
    }

    async fn traverse(&self, traversal: Traversal) -> GraphResult<Vec<Node>> {
        self.inner.traverse(traversal).await
    }
}

#[async_trait]
impl GraphMutationStore for RacingStore {
    fn mutation_atomicity(&self) -> GraphMutationAtomicity {
        self.inner.mutation_atomicity()
    }

    async fn delete_node(&self, id: &NodeId) -> GraphResult<()> {
        self.inner.delete_node(id).await
    }

    async fn delete_edge(
        &self,
        from: &NodeId,
        label: &grust_core::prelude::Label,
        to: &NodeId,
    ) -> GraphResult<()> {
        self.inner.delete_edge(from, label, to).await
    }

    async fn apply_mutations(&self, mutations: &[GraphMutation]) -> GraphResult<()> {
        self.inner.apply_mutations(mutations).await
    }
}

#[async_trait]
impl GraphCommitStore for RacingStore {
    async fn commit_guarded(&self, commit: &GuardedGraphCommit) -> GraphResult<GraphCommitReceipt> {
        if commit.idempotency_key.starts_with("cognition-commit:") {
            let race = self.race.lock().expect("race lock").take();
            match race {
                Some(Race::Source(replacement)) => {
                    self.inner.put_node(&replacement).await?;
                }
                Some(Race::Output(output)) => {
                    let mut node = absent_output_node(commit).expect("prepared output node");
                    let Value::Json(record) = node.props.get_mut("record").expect("record payload")
                    else {
                        panic!("record payload is JSON")
                    };
                    record["content"]["text"] = serde_json::json!("original target text");
                    *output.lock().expect("collision id lock") =
                        record["id"].as_str().map(ToOwned::to_owned);
                    self.inner.put_node(&node).await?;
                }
                None => {}
            }
        }
        self.inner.commit_guarded(commit).await
    }

    async fn recover_guarded_commit(
        &self,
        idempotency_key: &str,
        request_digest: &str,
    ) -> GraphResult<Option<GraphCommitReceipt>> {
        self.inner
            .recover_guarded_commit(idempotency_key, request_digest)
            .await
    }
}

fn absent_output_node(commit: &GuardedGraphCommit) -> Option<Node> {
    commit.mutations.iter().find_map(|mutation| {
        let GraphMutation::UpsertNode(node) = mutation else {
            return None;
        };
        commit
            .expectations
            .iter()
            .any(
                |expectation| matches!(expectation, GraphExpectation::Absent(id) if id == &node.id),
            )
            .then(|| node.clone())
    })
}
