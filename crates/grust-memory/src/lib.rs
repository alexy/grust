use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use async_trait::async_trait;
use grust_core::prelude::*;

#[derive(Clone, Debug, Default)]
pub struct MemoryGraphStore {
    inner: Arc<RwLock<MemoryGraph>>,
}

#[derive(Clone, Debug, Default)]
struct MemoryGraph {
    nodes: BTreeMap<NodeId, Node>,
    edges: BTreeMap<(NodeId, Label, NodeId), Edge>,
    schema: Option<GraphSchema>,
}

impl MemoryGraphStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn graph(&self) -> Graph {
        let inner = self.inner.read().expect("memory graph lock poisoned");
        Graph {
            nodes: inner.nodes.values().cloned().collect(),
            edges: inner.edges.values().cloned().collect(),
        }
    }

    fn node_matches(node: &Node, label: Option<&Label>, props: &Props) -> bool {
        label.is_none_or(|label| &node.label == label)
            && props.iter().all(|(key, value)| {
                if key == "id" {
                    value.as_str().is_some_and(|id| node.id.as_str() == id)
                } else {
                    node.props.get(key) == Some(value)
                }
            })
    }

    fn matching_node_ids(inner: &MemoryGraph, label: Option<&Label>, props: &Props) -> Vec<NodeId> {
        inner
            .nodes
            .values()
            .filter(|node| Self::node_matches(node, label, props))
            .map(|node| node.id.clone())
            .collect()
    }

    fn relationship_matches(
        inner: &MemoryGraph,
        edge: &Edge,
        relationship: &GraphRelationshipMatch,
    ) -> bool {
        if edge.label != relationship.label {
            return false;
        }
        if relationship
            .id
            .as_ref()
            .is_some_and(|id| edge.id.as_ref() != Some(id))
        {
            return false;
        }
        let Some(from) = inner.nodes.get(&edge.from) else {
            return false;
        };
        let Some(to) = inner.nodes.get(&edge.to) else {
            return false;
        };
        Self::node_matches(
            from,
            relationship.from.label.as_ref(),
            &relationship.from.props,
        ) && Self::node_matches(to, relationship.to.label.as_ref(), &relationship.to.props)
    }

    fn matching_edges(inner: &MemoryGraph, relationship: &GraphRelationshipMatch) -> Vec<Edge> {
        inner
            .edges
            .values()
            .filter(|edge| Self::relationship_matches(inner, edge, relationship))
            .cloned()
            .collect()
    }
}

#[async_trait]
impl GraphStore for MemoryGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        let mut inner = self.inner.write().expect("memory graph lock poisoned");
        inner.schema = Some(schema.clone());
        Ok(())
    }

    async fn put_node(&self, node: &Node) -> Result<PutOutcome> {
        let mut inner = self.inner.write().expect("memory graph lock poisoned");
        if let Some(schema) = &inner.schema {
            schema.validate_node(node)?;
        }
        let previous = inner.nodes.insert(node.id.clone(), node.clone());
        Ok(match previous {
            Some(_) => PutOutcome::Updated,
            None => PutOutcome::Inserted,
        })
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        let mut inner = self.inner.write().expect("memory graph lock poisoned");
        if let Some(schema) = &inner.schema {
            schema.validate_edge_with(edge, |id| inner.nodes.get(id).map(|node| &node.label))?;
        }
        let previous = inner.edges.insert(
            (edge.from.clone(), edge.label.clone(), edge.to.clone()),
            edge.clone(),
        );
        Ok(match previous {
            Some(_) => PutOutcome::Updated,
            None => PutOutcome::Inserted,
        })
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        let mut inner = self.inner.write().expect("memory graph lock poisoned");
        if let Some(schema) = &inner.schema {
            schema.validate_graph(graph)?;
        }
        let mut report = LoadReport::default();
        for node in &graph.nodes {
            inner.nodes.insert(node.id.clone(), node.clone());
            report.nodes += 1;
        }
        for edge in &graph.edges {
            inner.edges.insert(
                (edge.from.clone(), edge.label.clone(), edge.to.clone()),
                edge.clone(),
            );
            report.edges += 1;
        }
        Ok(report)
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        let inner = self.inner.read().expect("memory graph lock poisoned");
        Ok(inner.nodes.get(id).cloned())
    }

    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>> {
        let inner = self.inner.read().expect("memory graph lock poisoned");
        Ok(ids
            .iter()
            .filter_map(|id| inner.nodes.get(id).cloned())
            .collect())
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        let inner = self.inner.read().expect("memory graph lock poisoned");
        Ok(inner
            .edges
            .values()
            .filter(|edge| {
                query.from.as_ref().is_none_or(|from| from == &edge.from)
                    && query.to.as_ref().is_none_or(|to| to == &edge.to)
                    && query
                        .label
                        .as_ref()
                        .is_none_or(|label| label == &edge.label)
            })
            .cloned()
            .collect())
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        let inner = self.inner.read().expect("memory graph lock poisoned");
        let mut current = match traversal.start {
            Start::Node(id) => inner
                .nodes
                .get(&id)
                .cloned()
                .into_iter()
                .collect::<Vec<_>>(),
            Start::NodesByLabel(label) => inner
                .nodes
                .values()
                .filter(|node| node.label == label)
                .cloned()
                .collect(),
            Start::NodesByProperty { label, key, value } => inner
                .nodes
                .values()
                .filter(|node| node.label == label && node.props.get(&key) == Some(&value))
                .cloned()
                .collect(),
        };

        for step in traversal.steps {
            let mut next = Vec::new();
            for node in &current {
                for edge in inner.edges.values() {
                    let label_matches = step.edge.as_ref().is_none_or(|label| label == &edge.label);
                    let out_matches = matches!(step.direction, Direction::Out | Direction::Both)
                        && edge.from == node.id;
                    let in_matches = matches!(step.direction, Direction::In | Direction::Both)
                        && edge.to == node.id;

                    if !label_matches || (!out_matches && !in_matches) {
                        continue;
                    }

                    let target_id = if out_matches { &edge.to } else { &edge.from };
                    if let Some(target) = inner.nodes.get(target_id)
                        && step
                            .node
                            .as_ref()
                            .is_none_or(|label| label == &target.label)
                    {
                        next.push(target.clone());
                    }
                }
            }
            current = next;
        }

        if let Some(limit) = traversal.limit {
            current.truncate(limit as usize);
        }
        Ok(current)
    }
}

#[async_trait]
impl GraphMutationStore for MemoryGraphStore {
    async fn delete_node(&self, id: &NodeId) -> Result<()> {
        let mut inner = self.inner.write().expect("memory graph lock poisoned");
        inner.nodes.remove(id);
        inner
            .edges
            .retain(|(from, _, to), _| from != id && to != id);
        Ok(())
    }

    async fn delete_edge(&self, from: &NodeId, label: &Label, to: &NodeId) -> Result<()> {
        let mut inner = self.inner.write().expect("memory graph lock poisoned");
        inner
            .edges
            .remove(&(from.clone(), label.clone(), to.clone()));
        Ok(())
    }
}

#[async_trait]
impl CypherMutationExecutor for MemoryGraphStore {
    async fn execute_cypher_mutation_plan(
        &self,
        plan: &GraphMutationPlan,
    ) -> Result<GraphMutationReport> {
        let mut report = plan.report();
        for operation in &plan.operations {
            match operation {
                GraphMutationPlanOp::PatchMatchingNodes {
                    label,
                    props,
                    patch,
                    ..
                } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let ids = Self::matching_node_ids(&inner, label.as_ref(), props);
                    report.matched_rows += ids.len();
                    report.node_patches += ids.len();
                    report.changed_nodes += ids.len();

                    let mut patched = Vec::with_capacity(ids.len());
                    for id in &ids {
                        if let Some(node) = inner.nodes.get(id) {
                            let mut node = node.clone();
                            for (key, value) in patch {
                                node.props.insert(key.clone(), value.clone());
                            }
                            if let Some(schema) = &inner.schema {
                                schema.validate_node(&node)?;
                            }
                            patched.push(node);
                        }
                    }
                    for node in patched {
                        inner.nodes.insert(node.id.clone(), node);
                    }
                }
                GraphMutationPlanOp::RemoveMatchingNodeProps {
                    label, props, keys, ..
                } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let ids = Self::matching_node_ids(&inner, label.as_ref(), props);
                    report.matched_rows += ids.len();
                    report.node_property_removes += ids.len();
                    report.changed_nodes += ids.len();

                    let mut updated = Vec::with_capacity(ids.len());
                    for id in &ids {
                        if let Some(node) = inner.nodes.get(id) {
                            let mut node = node.clone();
                            for key in keys {
                                node.props.remove(key);
                            }
                            if let Some(schema) = &inner.schema {
                                schema.validate_node(&node)?;
                            }
                            updated.push(node);
                        }
                    }
                    for node in updated {
                        inner.nodes.insert(node.id.clone(), node);
                    }
                }
                GraphMutationPlanOp::DeleteMatchingNodes { label, props, .. } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let ids = Self::matching_node_ids(&inner, label.as_ref(), props);
                    let incident_edges = inner
                        .edges
                        .keys()
                        .filter(|(from, _, to)| ids.iter().any(|id| id == from || id == to))
                        .count();

                    report.matched_rows += ids.len();
                    report.node_deletes += ids.len();
                    report.changed_nodes += ids.len();
                    report.edge_deletes += incident_edges;
                    report.changed_edges += incident_edges;

                    for id in &ids {
                        inner.nodes.remove(id);
                    }
                    inner
                        .edges
                        .retain(|(from, _, to), _| !ids.iter().any(|id| id == from || id == to));
                }
                GraphMutationPlanOp::PatchMatchingEdges {
                    relationship,
                    patch,
                    ..
                } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let edges = Self::matching_edges(&inner, relationship);
                    report.matched_rows += edges.len();
                    report.edge_patches += edges.len();
                    report.changed_edges += edges.len();

                    let mut patched = Vec::with_capacity(edges.len());
                    for mut edge in edges {
                        for (key, value) in patch {
                            edge.props.insert(key.clone(), value.clone());
                        }
                        if let Some(schema) = &inner.schema {
                            schema.validate_edge_with(&edge, |id| {
                                inner.nodes.get(id).map(|node| &node.label)
                            })?;
                        }
                        patched.push(edge);
                    }
                    for edge in patched {
                        inner.edges.insert(
                            (edge.from.clone(), edge.label.clone(), edge.to.clone()),
                            edge,
                        );
                    }
                }
                GraphMutationPlanOp::RemoveMatchingEdgeProps {
                    relationship, keys, ..
                } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let edges = Self::matching_edges(&inner, relationship);
                    report.matched_rows += edges.len();
                    report.edge_property_removes += edges.len();
                    report.changed_edges += edges.len();

                    let mut updated = Vec::with_capacity(edges.len());
                    for mut edge in edges {
                        for key in keys {
                            edge.props.remove(key);
                        }
                        if let Some(schema) = &inner.schema {
                            schema.validate_edge_with(&edge, |id| {
                                inner.nodes.get(id).map(|node| &node.label)
                            })?;
                        }
                        updated.push(edge);
                    }
                    for edge in updated {
                        inner.edges.insert(
                            (edge.from.clone(), edge.label.clone(), edge.to.clone()),
                            edge,
                        );
                    }
                }
                GraphMutationPlanOp::DeleteMatchingEdges { relationship, .. } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let edges = Self::matching_edges(&inner, relationship);
                    report.matched_rows += edges.len();
                    report.edge_deletes += edges.len();
                    report.changed_edges += edges.len();
                    for edge in edges {
                        inner.edges.remove(&(
                            edge.from.clone(),
                            edge.label.clone(),
                            edge.to.clone(),
                        ));
                    }
                }
                _ => {
                    let mutation = GraphMutation::from(operation.clone());
                    self.apply_mutations(std::slice::from_ref(&mutation))
                        .await?;
                }
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests;
