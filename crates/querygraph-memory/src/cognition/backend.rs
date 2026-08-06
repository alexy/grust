//! Guarded graph CAS primitives shared by scheduler and commit paths.

use chrono::{DateTime, Utc};
use grust_core::prelude::{
    GraphCommitStore, GraphExpectation, GraphMutation, GrustError, GuardedGraphCommit, Node,
};
use typesec_memory::CognitionIdempotencyKey;

use super::graph::{JOB_LABEL, decode_versioned_payload, encode_job, job_node_id, json_digest};
use super::invariants::{advance_job, validate_persisted_job};
use super::{CognitionJob, CognitionJobStatus, CognitionStateError};
use crate::GraphStoreMemoryStore;

const TRANSITION_DOMAIN: &str = "querygraph.cognition.state-transition.v1";
const BACKEND_FAILURE: &str = "cognition graph backend operation failed";

impl<G: GraphCommitStore> GraphStoreMemoryStore<G> {
    pub(crate) fn load_job_node(
        &self,
        key: &CognitionIdempotencyKey,
    ) -> Result<Option<(Node, CognitionJob)>, CognitionStateError> {
        let expected_id = job_node_id(key);
        let node = self.run_graph(self.graph.get_node(&expected_id))?;
        node.map(|node| {
            if node.id != expected_id {
                return Err(CognitionStateError::Backend(
                    "cognition backend returned a job under the wrong node id".into(),
                ));
            }
            let job = decode_versioned_payload(
                &node,
                JOB_LABEL,
                super::state::JOB_SCHEMA_VERSION,
                "unsupported persisted cognition job schema version",
            )?;
            validate_persisted_job(key, &job)?;
            Ok((node, job))
        })
        .transpose()
    }

    pub(crate) fn create_job(
        &self,
        key: &CognitionIdempotencyKey,
        job: &CognitionJob,
    ) -> Result<(), CognitionStateError> {
        self.persist_job(key, job, vec![GraphExpectation::Absent(job_node_id(key))])
    }

    pub(crate) fn transition_job(
        &self,
        key: &CognitionIdempotencyKey,
        expected: Node,
        job: &CognitionJob,
    ) -> Result<(), CognitionStateError> {
        self.persist_job(key, job, vec![GraphExpectation::Exact(expected)])
    }

    fn persist_job(
        &self,
        key: &CognitionIdempotencyKey,
        job: &CognitionJob,
        expectations: Vec<GraphExpectation>,
    ) -> Result<(), CognitionStateError> {
        let desired = encode_job(key, job)?;
        let request = json_digest(TRANSITION_DOMAIN, job)?;
        let guarded = GuardedGraphCommit::new(
            super::graph::transition_ledger_key(job),
            request,
            vec![GraphMutation::UpsertNode(desired.clone())],
        )
        .with_expectations(expectations);
        let receipt = match self.bridge.run(self.graph.commit_guarded(&guarded)) {
            Ok(receipt) => receipt,
            Err(
                GrustError::GraphExpectationFailed(_) | GrustError::GraphIdempotencyConflict(_),
            ) => return Err(CognitionStateError::ConcurrentModification),
            Err(_) => return Err(state_backend_error()),
        };
        if receipt.replayed {
            self.verify_replayed_node(&desired)?;
        }
        Ok(())
    }

    pub(crate) fn verify_replayed_node(&self, desired: &Node) -> Result<(), CognitionStateError> {
        let actual = self.run_graph(self.graph.get_node(&desired.id))?;
        if actual.as_ref() != Some(desired) {
            return Err(CognitionStateError::Backend(
                "replayed cognition transition lacks its exact durable node".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn run_graph<T: Send>(
        &self,
        future: impl std::future::Future<Output = grust_core::prelude::Result<T>> + Send,
    ) -> Result<T, CognitionStateError> {
        self.bridge.run(future).map_err(|_| state_backend_error())
    }
}

pub(super) fn mark_job_completed(
    job: &mut CognitionJob,
    completion_digest: &str,
    now: DateTime<Utc>,
) -> Result<(), CognitionStateError> {
    advance_job(job, now)?;
    job.status = CognitionJobStatus::Completed;
    job.completion_digest = Some(completion_digest.to_owned());
    job.lease = None;
    Ok(())
}

pub(super) fn state_backend_error() -> CognitionStateError {
    CognitionStateError::Backend(BACKEND_FAILURE.into())
}
