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
    async fn put_node(&self, node: &Node) -> Result<NodeId> {
        let mut inner = self.inner.write().expect("memory graph lock poisoned");
        inner.nodes.insert(node.id.clone(), node.clone());
        Ok(node.id.clone())
    }

    async fn put_edge(&self, edge: &Edge) -> Result<Option<EdgeId>> {
        let mut inner = self.inner.write().expect("memory graph lock poisoned");
        inner.edges.insert(
            (edge.from.clone(), edge.label.clone(), edge.to.clone()),
            edge.clone(),
        );
        Ok(edge.id.clone())
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        let inner = self.inner.read().expect("memory graph lock poisoned");
        Ok(inner.nodes.get(id).cloned())
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
                    if let Some(target) = inner.nodes.get(target_id) {
                        if step
                            .node
                            .as_ref()
                            .is_none_or(|label| label == &target.label)
                        {
                            next.push(target.clone());
                        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_graph_and_traverses_one_step() {
        let mut builder = GraphBuilder::new();
        let talk = builder.node("Talk", "talk-1").finish();
        let person = builder.node("Person", "person-1").finish();
        builder.edge("PRESENTED_BY", &talk, &person).finish();
        let graph = builder.build();

        let store = MemoryGraphStore::new();
        futures_executor::block_on(store.put_graph(&graph)).unwrap();
        let speakers = futures_executor::block_on(
            store.traverse(
                Traversal::from_node("talk-1")
                    .out("PRESENTED_BY")
                    .to("Person"),
            ),
        )
        .unwrap();

        assert_eq!(speakers.len(), 1);
        assert_eq!(speakers[0].id, NodeId::from("person-1"));
    }
}
