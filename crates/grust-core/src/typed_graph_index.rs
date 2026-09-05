//! Snapshot-owned, typed adjacency for compact read execution.

use std::{collections::HashMap, io, sync::Arc};

use crate::{Graph, GrustError, Label, NodeId, Result};

/// One adjacency entry. Parallel edges remain distinct through `edge`.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TypedNeighbor {
    pub vertex: u32,
    pub edge: u32,
}

#[derive(Debug)]
struct Csr {
    offsets: CsrOffsets,
    neighbors: Vec<TypedNeighbor>,
}

#[derive(Debug)]
enum CsrOffsets {
    Dense(Vec<u32>),
    Sparse {
        sources: Vec<u32>,
        offsets: Vec<u32>,
    },
}

impl Csr {
    fn build(vertices: usize, edges: &[(u32, u32, u32)], reverse: bool) -> Self {
        // Every nonempty dense type pays at most 4E + 1 <= 5E offsets.
        // Sparse types must never allocate or initialize a V-sized scratch array.
        if edges.len() >= vertices.div_ceil(4)
            && let Some(offset_count) = vertices.checked_add(1)
        {
            Self::build_dense(offset_count, edges, reverse)
        } else {
            Self::build_sparse(edges, reverse)
        }
    }

    fn build_dense(offset_count: usize, edges: &[(u32, u32, u32)], reverse: bool) -> Self {
        // Bucket edges by source in linear time, then sort only each vertex's
        // neighbors. Avoid sorting the complete edge relation in both directions.
        let mut offsets = vec![0; offset_count];
        for &(from, to, _) in edges {
            offsets[if reverse { to } else { from } as usize] += 1;
        }
        let mut end = 0;
        for offset in &mut offsets {
            end += *offset;
            *offset = end;
        }
        let mut neighbors = vec![TypedNeighbor { vertex: 0, edge: 0 }; edges.len()];
        // Exclusive ends double as descending insertion cursors. Once filled,
        // they are exactly the CSR starts, with the final sentinel untouched.
        for &(from, to, edge) in edges {
            let (source, vertex) = if reverse { (to, from) } else { (from, to) };
            offsets[source as usize] -= 1;
            neighbors[offsets[source as usize] as usize] = TypedNeighbor { vertex, edge };
        }
        for bounds in offsets.windows(2) {
            neighbors[bounds[0] as usize..bounds[1] as usize].sort_unstable();
        }
        Self {
            offsets: CsrOffsets::Dense(offsets),
            neighbors,
        }
    }

    fn build_sparse(edges: &[(u32, u32, u32)], reverse: bool) -> Self {
        let mut ordered: Vec<_> = edges
            .iter()
            .map(|&(from, to, edge)| {
                if reverse {
                    (to, from, edge)
                } else {
                    (from, to, edge)
                }
            })
            .collect();
        ordered.sort_unstable();
        let mut sources = Vec::new();
        let mut offsets = Vec::new();
        let mut neighbors = Vec::with_capacity(edges.len());
        for (source, vertex, edge) in ordered {
            if sources.last() != Some(&source) {
                sources.push(source);
                offsets.push(neighbors.len() as u32);
            }
            neighbors.push(TypedNeighbor { vertex, edge });
        }
        offsets.push(neighbors.len() as u32);
        Self {
            offsets: CsrOffsets::Sparse { sources, offsets },
            neighbors,
        }
    }

    fn at(&self, vertex: u32) -> &[TypedNeighbor] {
        let (offsets, slot) = match &self.offsets {
            CsrOffsets::Dense(offsets) => (offsets, vertex as usize),
            CsrOffsets::Sparse { sources, offsets } => {
                let Ok(slot) = sources.binary_search(&vertex) else {
                    return &[];
                };
                (offsets, slot)
            }
        };
        // Never compute vertex + 1: u32::MAX must also be safe on 32-bit hosts.
        let Some(bounds) = offsets.get(slot..).and_then(|tail| tail.get(..2)) else {
            return &[];
        };
        &self.neighbors[bounds[0] as usize..bounds[1] as usize]
    }

    #[inline]
    fn sparse_sources(&self) -> Option<&[u32]> {
        match &self.offsets {
            CsrOffsets::Dense(_) => None,
            CsrOffsets::Sparse { sources, .. } => Some(sources),
        }
    }
}

#[derive(Debug)]
struct TypedAdjacency {
    forward: Csr,
    reverse: Csr,
}

/// Borrowed adjacency for one relationship type in an immutable index.
///
/// Obtain a view with [`TypedGraphIndex::adjacency`] to resolve the type once.
/// Copying the view neither allocates nor clones the graph or its `Arc`.
/// Row access performs no type hashing: dense rows use O(1) offset lookup,
/// while sparse rows use O(log(active sources)) lookup. An absent type has
/// empty rows, as do invalid vertex slots, including `u32::MAX`.
///
/// Slices retain physical edge multiplicity and are sorted by the other vertex
/// slot, then edge slot. A self-loop appears once in each direction; callers
/// combining both directions must avoid double-counting it when appropriate.
/// Sparse-source accessors borrow the CSR's sorted, unique nonempty row slots;
/// they return `None` for dense storage and `Some(&[])` for an absent type.
/// Views do not execute queries or exempt callers from read budgets.
///
/// The view and its slices borrow the index, not the relationship string, and
/// cannot outlive the index even if another owner retains the graph snapshot:
///
/// ```compile_fail
/// use grust_core::{Graph, TypedGraphIndex};
/// use std::sync::Arc;
/// let snapshot = Arc::new(Graph::default());
/// let view = {
///     let index = TypedGraphIndex::new(snapshot.clone()).unwrap();
///     index.adjacency("T")
/// };
/// assert!(view.outgoing(0).is_empty());
/// ```
#[derive(Clone, Copy, Debug)]
pub struct TypedAdjacencyView<'index> {
    adjacency: Option<&'index TypedAdjacency>,
}

impl<'index> TypedAdjacencyView<'index> {
    /// Outgoing neighbors, borrowed for the lifetime of the indexed snapshot.
    #[inline]
    pub fn outgoing(&self, vertex: u32) -> &'index [TypedNeighbor] {
        self.adjacency
            .map(|adjacency| adjacency.forward.at(vertex))
            .unwrap_or(&[])
    }

    /// Incoming neighbors, borrowed for the lifetime of the indexed snapshot.
    #[inline]
    pub fn incoming(&self, vertex: u32) -> &'index [TypedNeighbor] {
        self.adjacency
            .map(|adjacency| adjacency.reverse.at(vertex))
            .unwrap_or(&[])
    }

    /// Sorted, unique outgoing slots with nonempty rows when storage is sparse.
    ///
    /// Dense storage returns `None`, because enumerating only its nonempty rows
    /// would require a scan. An absent relationship type returns `Some(&[])`.
    #[inline]
    pub fn sparse_outgoing_sources(&self) -> Option<&'index [u32]> {
        match self.adjacency {
            Some(adjacency) => adjacency.forward.sparse_sources(),
            None => Some(&[]),
        }
    }

    /// Sorted, unique incoming slots with nonempty rows when storage is sparse.
    ///
    /// Dense storage returns `None`, because enumerating only its nonempty rows
    /// would require a scan. An absent relationship type returns `Some(&[])`.
    #[inline]
    pub fn sparse_incoming_sources(&self) -> Option<&'index [u32]> {
        match self.adjacency {
            Some(adjacency) => adjacency.reverse.sparse_sources(),
            None => Some(&[]),
        }
    }
}

/// A reusable index that owns an immutable graph snapshot through an `Arc`.
///
/// Vertex and edge slots refer to this snapshot's vector positions, not to
/// property values. Holding the snapshot prevents mutation from invalidating
/// the index; `Arc::make_mut` on another owner creates a separate graph.
/// Construction validates node identities and endpoints and preserves edge
/// multiplicity. It does not execute queries or exempt them from read budgets.
///
/// Auxiliary slot storage is O(V + E), not O(V times relationship types): a
/// nonempty type uses dense offsets only when it has at least ceil(V / 4)
/// edges, otherwise it stores sorted active sources and sparse offsets. Dense
/// source lookup is O(1); sparse lookup is O(log(active sources)). Each returned
/// neighbor slice is sorted by destination slot and then edge slot.
///
/// Construction sorts neighbors (worst-case O(V + E log E) structural work)
/// and traverses the serialized graph once to cache its exact compact JSON
/// byte length, without allocating a graph-sized encoding buffer. That cached
/// length measures the graph, not this index's allocations. Variable-length
/// identifiers, labels, and property data additionally contribute their byte
/// costs; the graph snapshot remains owned by the index.
#[derive(Debug)]
pub struct TypedGraphIndex {
    graph: Arc<Graph>,
    serialized_graph_bytes: usize,
    vertex_by_id: HashMap<NodeId, u32>,
    vertices_by_label: HashMap<Label, Vec<u32>>,
    adjacency: HashMap<Label, TypedAdjacency>,
}

impl TypedGraphIndex {
    pub fn new(graph: Arc<Graph>) -> Result<Self> {
        u32::try_from(graph.nodes.len()).map_err(|_| {
            GrustError::Schema("typed graph index exceeds u32 vertex capacity".into())
        })?;
        u32::try_from(graph.edges.len()).map_err(|_| {
            GrustError::Schema("typed graph index exceeds u32 edge capacity".into())
        })?;
        let mut vertex_by_id = HashMap::with_capacity(graph.nodes.len());
        let mut vertices_by_label: HashMap<Label, Vec<u32>> = HashMap::new();
        for (vertex, node) in graph.nodes.iter().enumerate() {
            let vertex = vertex as u32;
            if vertex_by_id.insert(node.id.clone(), vertex).is_some() {
                return Err(GrustError::Schema(format!(
                    "duplicate vertex id '{}'",
                    node.id
                )));
            }
            vertices_by_label
                .entry(node.label.clone())
                .or_default()
                .push(vertex);
        }
        let mut typed_edges: HashMap<Label, Vec<(u32, u32, u32)>> = HashMap::new();
        for (edge_index, edge) in graph.edges.iter().enumerate() {
            let from = *vertex_by_id.get(&edge.from).ok_or_else(|| {
                GrustError::Schema(format!(
                    "edge source '{}' is not present in vertices",
                    edge.from
                ))
            })?;
            let to = *vertex_by_id.get(&edge.to).ok_or_else(|| {
                GrustError::Schema(format!(
                    "edge destination '{}' is not present in vertices",
                    edge.to
                ))
            })?;
            typed_edges
                .entry(edge.label.clone())
                .or_default()
                .push((from, to, edge_index as u32));
        }
        let mut adjacency = HashMap::with_capacity(typed_edges.len());
        for (label, edges) in typed_edges {
            let forward = Csr::build(graph.nodes.len(), &edges, false);
            let reverse = Csr::build(graph.nodes.len(), &edges, true);
            adjacency.insert(label, TypedAdjacency { forward, reverse });
        }
        let mut size = SerializedSize(0);
        serde_json::to_writer(&mut size, graph.as_ref()).map_err(|error| {
            GrustError::Serialization(format!("failed to measure indexed graph: {error}"))
        })?;
        Ok(Self {
            graph,
            serialized_graph_bytes: size.0,
            vertex_by_id,
            vertices_by_label,
            adjacency,
        })
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// Exact compact JSON byte length of this immutable graph snapshot.
    /// Computed once without a graph-sized encoding buffer, for bounded readers.
    pub fn serialized_graph_bytes(&self) -> usize {
        self.serialized_graph_bytes
    }

    pub fn vertex_index(&self, id: &str) -> Option<u32> {
        self.vertex_by_id.get(id).copied()
    }

    pub fn vertices_with_label(&self, label: &str) -> &[u32] {
        self.vertices_by_label
            .get(label)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Resolve a relationship type once without allocating or copying its name.
    ///
    /// The returned view borrows this index, not `relationship`. Missing types
    /// return an empty view with the same vertex-slot behavior as existing
    /// string-based adjacency methods.
    pub fn adjacency(&self, relationship: &str) -> TypedAdjacencyView<'_> {
        TypedAdjacencyView {
            adjacency: self.adjacency.get(relationship),
        }
    }

    pub fn outgoing(&self, vertex: u32, relationship: &str) -> &[TypedNeighbor] {
        self.adjacency(relationship).outgoing(vertex)
    }

    pub fn incoming(&self, vertex: u32, relationship: &str) -> &[TypedNeighbor] {
        self.adjacency(relationship).incoming(vertex)
    }

    pub fn has_relationship(&self, from: u32, to: u32, relationship: &str) -> bool {
        self.outgoing(from, relationship)
            .binary_search_by_key(&to, |n| n.vertex)
            .is_ok()
    }
}

struct SerializedSize(usize);

impl io::Write for SerializedSize {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("serialized graph size exceeds usize"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "typed_graph_index_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "typed_adjacency_view_tests.rs"]
mod view_tests;

#[cfg(test)]
#[path = "typed_adjacency_sparse_sources_tests.rs"]
mod sparse_source_tests;
