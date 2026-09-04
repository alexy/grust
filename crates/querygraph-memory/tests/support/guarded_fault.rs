use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use grust_core::prelude::{
    Edge, EdgeQuery, Graph, GraphCommitReceipt, GraphCommitStore, GraphMutation,
    GraphMutationAtomicity, GraphMutationStore, GraphStore, GrustError, GuardedGraphCommit,
    LoadReport, Node, NodeId, PutOutcome, Result as GraphResult, Traversal,
};
use grust_turso::TursoGraphStore;

#[derive(Clone)]
pub enum GuardedFault {
    Pass,
    Replay { prefix: String },
    BackendError { prefix: String, secret: String },
    CommitThenResponseLoss { prefix: String, secret: String },
    ReceiptTimestamp { prefix: String, value: String },
    HideRecovery { prefix: String },
}

pub struct FaultControl {
    state: Arc<Mutex<FaultState>>,
}

struct FaultState {
    fault: GuardedFault,
    commit_calls: usize,
    recovery_calls: usize,
}

pub struct FaultingStore {
    inner: TursoGraphStore,
    state: Arc<Mutex<FaultState>>,
}

impl FaultingStore {
    pub fn new(inner: TursoGraphStore) -> (Self, FaultControl) {
        let state = Arc::new(Mutex::new(FaultState {
            fault: GuardedFault::Pass,
            commit_calls: 0,
            recovery_calls: 0,
        }));
        (
            Self {
                inner,
                state: Arc::clone(&state),
            },
            FaultControl { state },
        )
    }
}

impl FaultControl {
    pub fn set(&self, fault: GuardedFault) {
        self.state.lock().expect("fault state lock").fault = fault;
    }

    pub fn commit_calls(&self) -> usize {
        self.state.lock().expect("fault state lock").commit_calls
    }

    pub fn recovery_calls(&self) -> usize {
        self.state.lock().expect("fault state lock").recovery_calls
    }
}

#[async_trait]
impl GraphStore for FaultingStore {
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
impl GraphMutationStore for FaultingStore {
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
impl GraphCommitStore for FaultingStore {
    async fn commit_guarded(&self, commit: &GuardedGraphCommit) -> GraphResult<GraphCommitReceipt> {
        let fault = {
            let mut state = self.state.lock().expect("fault state lock");
            state.commit_calls += 1;
            state.fault.clone()
        };
        match fault {
            GuardedFault::Replay { prefix } if commit.idempotency_key.starts_with(&prefix) => {
                Ok(GraphCommitReceipt {
                    commit_id: "fault:replayed".into(),
                    committed_at: "2026-08-05T12:00:00.000Z".into(),
                    replayed: true,
                })
            }
            GuardedFault::BackendError { prefix, secret }
                if commit.idempotency_key.starts_with(&prefix) =>
            {
                Err(GrustError::Backend(secret))
            }
            GuardedFault::CommitThenResponseLoss { prefix, secret }
                if commit.idempotency_key.starts_with(&prefix) =>
            {
                self.inner.commit_guarded(commit).await?;
                Err(GrustError::Backend(secret))
            }
            GuardedFault::ReceiptTimestamp { prefix, value }
                if commit.idempotency_key.starts_with(&prefix) =>
            {
                let mut receipt = self.inner.commit_guarded(commit).await?;
                receipt.committed_at = value;
                Ok(receipt)
            }
            _ => self.inner.commit_guarded(commit).await,
        }
    }

    async fn recover_guarded_commit(
        &self,
        idempotency_key: &str,
        request_digest: &str,
    ) -> GraphResult<Option<GraphCommitReceipt>> {
        let fault = {
            let mut state = self.state.lock().expect("fault state lock");
            state.recovery_calls += 1;
            state.fault.clone()
        };
        match fault {
            GuardedFault::HideRecovery { prefix } if idempotency_key.starts_with(&prefix) => {
                Ok(None)
            }
            GuardedFault::BackendError { prefix, secret }
                if idempotency_key.starts_with(&prefix) =>
            {
                Err(GrustError::Backend(secret))
            }
            GuardedFault::ReceiptTimestamp { prefix, value }
                if idempotency_key.starts_with(&prefix) =>
            {
                let mut receipt = self
                    .inner
                    .recover_guarded_commit(idempotency_key, request_digest)
                    .await?;
                if let Some(receipt) = &mut receipt {
                    receipt.committed_at = value;
                }
                Ok(receipt)
            }
            _ => {
                self.inner
                    .recover_guarded_commit(idempotency_key, request_digest)
                    .await
            }
        }
    }
}
