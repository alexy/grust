//! Exact source loading and record mutation construction for cognition commits.

use std::collections::BTreeMap;

use grust_core::prelude::{GraphCommitStore, GraphExpectation, GraphMutation, Node, NodeId};
use typesec_memory::{
    CognitionCommitError, CognitionSourcePrecondition, MemoryId, StoreBatchOp, StoredRecord,
};

use super::commit_support::store_error;
use crate::{GraphStoreMemoryStore, decode_record, encode_record, record_node_id};

pub(super) struct ExactSource {
    pub(super) node: Node,
    record: StoredRecord,
}

pub(super) struct CommitRecordChanges {
    pub(super) mutations: Vec<GraphMutation>,
    pub(super) shared_node_expectations: Vec<GraphExpectation>,
}

impl<G: GraphCommitStore> GraphStoreMemoryStore<G> {
    pub(super) fn load_exact_sources(
        &self,
        preconditions: &[CognitionSourcePrecondition],
        space_id: &str,
    ) -> Result<BTreeMap<MemoryId, ExactSource>, CognitionCommitError> {
        let mut sources = BTreeMap::new();
        for precondition in preconditions {
            let node = self
                .run_commit(self.graph.get_node(&record_node_id(&precondition.id)))?
                .ok_or_else(|| CognitionCommitError::StaleSource(precondition.id.clone()))?;
            let record = decode_record(&node)
                .map_err(|_| store_error("cognition source record is invalid"))?;
            if record.space_id != space_id {
                return Err(store_error("cognition source belongs to another space"));
            }
            let current = CognitionSourcePrecondition::for_record(&record)
                .map_err(|_| store_error("cognition source digest failed"))?;
            if &current != precondition {
                return Err(CognitionCommitError::StaleSource(precondition.id.clone()));
            }
            if sources
                .insert(precondition.id.clone(), ExactSource { node, record })
                .is_some()
            {
                return Err(store_error("duplicate cognition source precondition"));
            }
        }
        Ok(sources)
    }

    pub(super) fn commit_record_changes(
        &self,
        operations: &[StoreBatchOp],
        sources: &BTreeMap<MemoryId, ExactSource>,
    ) -> Result<CommitRecordChanges, CognitionCommitError> {
        let mut mutations = Vec::new();
        let mut shared_nodes = BTreeMap::<NodeId, Node>::new();
        for operation in operations {
            match operation {
                StoreBatchOp::Put(record) => {
                    let record_mutations = Self::record_mutations(record)
                        .map_err(|_| store_error("cognition output encoding failed"))?;
                    collect_shared_entity_nodes(&record_mutations, &mut shared_nodes)?;
                    mutations.extend(record_mutations);
                }
                StoreBatchOp::Invalidate { id, at } => {
                    let mut record = sources
                        .get(id)
                        .ok_or_else(|| store_error("invalidation target is not a guarded source"))?
                        .record
                        .clone();
                    record.invalid_at = Some(*at);
                    mutations
                        .push(GraphMutation::UpsertNode(encode_record(&record).map_err(
                            |_| store_error("cognition source encoding failed"),
                        )?));
                }
            }
        }
        let shared_node_expectations = self.shared_node_expectations(shared_nodes)?;
        Ok(CommitRecordChanges {
            mutations,
            shared_node_expectations,
        })
    }

    pub(super) fn find_stale_source(
        &self,
        preconditions: &[CognitionSourcePrecondition],
    ) -> Result<Option<MemoryId>, CognitionCommitError> {
        for expected in preconditions {
            let node = self.run_commit(self.graph.get_node(&record_node_id(&expected.id)))?;
            let Some(node) = node else {
                return Ok(Some(expected.id.clone()));
            };
            let record = decode_record(&node)
                .map_err(|_| store_error("cognition source record is invalid"))?;
            let actual = CognitionSourcePrecondition::for_record(&record)
                .map_err(|_| store_error("cognition source digest failed"))?;
            if &actual != expected {
                return Ok(Some(expected.id.clone()));
            }
        }
        Ok(None)
    }

    fn shared_node_expectations(
        &self,
        shared_nodes: BTreeMap<NodeId, Node>,
    ) -> Result<Vec<GraphExpectation>, CognitionCommitError> {
        let mut expectations = Vec::with_capacity(shared_nodes.len());
        for desired in shared_nodes.into_values() {
            match self.run_commit(self.graph.get_node(&desired.id))? {
                None => expectations.push(GraphExpectation::Absent(desired.id)),
                Some(actual) if actual == desired => {
                    expectations.push(GraphExpectation::Exact(actual));
                }
                Some(_) => {
                    return Err(store_error(
                        "cognition entity identity conflicts with durable graph",
                    ));
                }
            }
        }
        Ok(expectations)
    }
}

fn collect_shared_entity_nodes(
    mutations: &[GraphMutation],
    shared_nodes: &mut BTreeMap<NodeId, Node>,
) -> Result<(), CognitionCommitError> {
    for mutation in mutations {
        let GraphMutation::UpsertNode(node) = mutation else {
            continue;
        };
        if node.label.as_str() != crate::ENTITY_LABEL {
            continue;
        }
        if shared_nodes
            .insert(node.id.clone(), node.clone())
            .is_some_and(|existing| existing != *node)
        {
            return Err(store_error(
                "cognition outputs disagree on shared entity identity",
            ));
        }
    }
    Ok(())
}
