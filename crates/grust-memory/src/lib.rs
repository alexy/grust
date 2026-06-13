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

#[cfg(test)]
mod tests;
