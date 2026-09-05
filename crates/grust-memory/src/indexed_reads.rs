//! Reads served from the cached typed snapshot.
//!
//! Loading a graph into an empty store builds its `TypedGraphIndex`; until the
//! next write, `traverse` and endpoint-anchored `get_edges` walk that index's
//! `u32` slot adjacency instead of the string-keyed edge maps. Every write
//! invalidates the snapshot and reads return to the maps, so a workload that
//! interleaves point writes and reads never rebuilds an index inside a read;
//! `MemoryGraphStore::indexed_snapshot` rebuilds it on request.
//!
//! Results are the same nodes and edges in the same order as the map walk:
//! vertex slots follow node-id order and relationship types are visited
//! sorted, which is the `(label, neighbour id)` order of the edge maps. The
//! one difference is `Direction::Both`, which lists outgoing neighbours before
//! incoming ones instead of interleaving them by the far endpoint's id.

use super::*;

impl MemoryGraphStore {
    /// The cached snapshot when a load or an `indexed_snapshot()` call built
    /// one and no write has invalidated it since. Never builds one itself.
    ///
    /// Callers hold the graph read lock, so the snapshot they get is the
    /// store's current contents: `write_inner` clears the cache under the
    /// write lock before it mutates anything.
    pub(super) fn cached_index(&self) -> Option<Arc<TypedGraphIndex>> {
        self.index_cache
            .lock()
            .expect("memory index cache lock poisoned")
            .clone()
    }

    /// Build the snapshot after a load when the store was empty before it.
    ///
    /// A fresh load is the one moment a full index build is proportional to
    /// the work just done; incremental loads into a populated store leave the
    /// maps in charge rather than pay O(V + E) per batch. A graph the index
    /// cannot represent (dangling edges, more than `u32::MAX` slots) simply
    /// keeps the map path, which accepts both.
    pub(super) fn warm_index_after_load(&self, was_empty: bool) {
        if was_empty {
            self.indexed_snapshot().ok();
        }
    }
}

/// `traversal` over `index`, as `GraphStore::traverse` would answer it.
pub(super) fn traverse_indexed(index: &TypedGraphIndex, traversal: &Traversal) -> Vec<Node> {
    let graph = index.graph();
    traverse_slots(index, traversal)
        .into_iter()
        .map(|vertex| graph.nodes[vertex as usize].clone())
        .collect()
}

/// `traversal` over `index`, as `GraphStore::traverse_ids` would answer it:
/// the same vertices, cloning only their ids.
pub(super) fn traverse_ids_indexed(index: &TypedGraphIndex, traversal: &Traversal) -> Vec<NodeId> {
    let graph = index.graph();
    traverse_slots(index, traversal)
        .into_iter()
        .map(|vertex| graph.nodes[vertex as usize].id.clone())
        .collect()
}

/// The vertex slots `traversal` reaches over `index`, in result order.
fn traverse_slots(index: &TypedGraphIndex, traversal: &Traversal) -> Vec<u32> {
    let graph = index.graph();
    let mut current: Vec<u32> = match &traversal.start {
        Start::Node(id) => index.vertex_index(id.as_str()).into_iter().collect(),
        Start::NodesByLabel(label) => index.vertices_with_label(label.as_str()).to_vec(),
        Start::NodesByProperty { label, key, value } => index
            .vertices_with_label(label.as_str())
            .iter()
            .copied()
            .filter(|&vertex| graph.nodes[vertex as usize].props.get(key) == Some(value))
            .collect(),
    };
    for step in &traversal.steps {
        let mut next = Vec::new();
        let types = step_types(index, step.edge.as_ref());
        for &vertex in &current {
            for relationship in &types {
                let view = index.adjacency(relationship);
                let accept = |target: u32, next: &mut Vec<u32>| {
                    let wanted = step
                        .node
                        .as_ref()
                        .is_none_or(|label| graph.nodes[target as usize].label == *label);
                    if wanted {
                        next.push(target);
                    }
                };
                match step.direction {
                    Direction::Out => view
                        .outgoing(vertex)
                        .iter()
                        .for_each(|n| accept(n.vertex, &mut next)),
                    Direction::In => view
                        .incoming(vertex)
                        .iter()
                        .for_each(|n| accept(n.vertex, &mut next)),
                    Direction::Both => {
                        view.outgoing(vertex)
                            .iter()
                            .for_each(|n| accept(n.vertex, &mut next));
                        // A self-loop is one edge, already listed as outgoing.
                        view.incoming(vertex)
                            .iter()
                            .filter(|n| n.vertex != vertex)
                            .for_each(|n| accept(n.vertex, &mut next));
                    }
                }
            }
        }
        current = next;
    }
    if let Some(limit) = traversal.limit {
        current.truncate(limit as usize);
    }
    current
}

/// Edges matching `query` over `index`, or `None` when the query anchors on
/// neither endpoint and only a full scan can answer it.
pub(super) fn edges_indexed(index: &TypedGraphIndex, query: &EdgeQuery) -> Option<Vec<Edge>> {
    let graph = index.graph();
    let types = step_types(index, query.label.as_ref());
    let matches = |edge: &Edge| {
        query.from.as_ref().is_none_or(|from| from == &edge.from)
            && query.to.as_ref().is_none_or(|to| to == &edge.to)
    };
    let neighbours = |vertex: u32, reverse: bool| {
        types
            .iter()
            .flat_map(move |relationship| {
                let view = index.adjacency(relationship);
                if reverse {
                    view.incoming(vertex)
                } else {
                    view.outgoing(vertex)
                }
            })
            .map(|n| &graph.edges[n.edge as usize])
            .filter(|edge| matches(edge))
            .cloned()
            .collect::<Vec<_>>()
    };
    let (anchor, reverse) = match (&query.from, &query.to) {
        (Some(from), _) => (from, false),
        (None, Some(to)) => (to, true),
        (None, None) => return None,
    };
    Some(match index.vertex_index(anchor.as_str()) {
        Some(vertex) => neighbours(vertex, reverse),
        None => Vec::new(),
    })
}

/// The relationship types a step or query visits: the one it names, or every
/// type in the index in sorted order.
fn step_types<'a>(index: &'a TypedGraphIndex, label: Option<&'a Label>) -> Vec<&'a str> {
    match label {
        Some(label) => vec![label.as_str()],
        None => index.relationship_types().map(Label::as_str).collect(),
    }
}

#[cfg(test)]
#[path = "indexed_reads_tests.rs"]
mod tests;
