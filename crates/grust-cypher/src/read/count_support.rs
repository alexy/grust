//! Weighted simple support shared by exact count factorization plans.
//!
//! Physical parallel and reciprocal relationship slots are coalesced into one
//! weighted undirected endpoint pair. Callers choose whether self-loop weights
//! are retained separately. Degree-ranked orientation and sorted forward-list
//! intersections enumerate every distinct-vertex support triangle once in
//! O(M^(3/2)) intersection work and O(V + M) auxiliary storage, where M is the
//! number of distinct non-loop endpoint pairs and V is the active domain size.
//! Constructing the explicit rank
//! order costs O(V log V), and sorting all forward rows costs O(M log M). The
//! rank uses simple support degree, not weighted degree; edge multiplicity is
//! payload only.

use super::*;
use grust_core::TypedGraphIndex;
use std::{cmp::Ordering, mem::size_of};

#[derive(Clone, Copy, Debug)]
pub(super) struct WeightedEdge {
    pub(super) a: u32,
    pub(super) b: u32,
    pub(super) multiplicity: u32,
}

#[derive(Clone, Copy, Debug)]
struct ForwardEdge {
    vertex: u32,
    multiplicity: u32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct WeightedTriangle {
    pub(super) x: usize,
    pub(super) y: usize,
    pub(super) z: usize,
    pub(super) xy: u128,
    pub(super) xz: u128,
    pub(super) yz: u128,
}

pub(super) struct WeightedSupport {
    edges: Vec<WeightedEdge>,
}

pub(super) struct OrientedSupport {
    forward: Vec<Vec<ForwardEdge>>,
    rank_to_ordinal: Vec<u32>,
}

fn support_error(message: impl Into<String>) -> GrustError {
    gql_execution(format!(
        "count support invariant failed: {}",
        message.into()
    ))
}

fn allocation_bytes<T>(items: usize, context: &str) -> Result<usize> {
    items.checked_mul(size_of::<T>()).ok_or_else(|| {
        gql_execution(format!(
            "count support allocation overflowed while {context}"
        ))
    })
}

fn reserved_vec<T>(items: usize, context: &str) -> Result<Vec<T>> {
    read_budget::charge_intermediate_bytes(allocation_bytes::<T>(items, context)?, context)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(items)
        .map_err(|_| gql_execution(format!("count support allocation failed while {context}")))?;
    Ok(values)
}

/// A padded accounting model for the infallible library sort. Checkpoints on
/// both sides bound the only non-cooperative part of support construction.
fn sort_work(items: usize) -> usize {
    if items < 2 {
        return items;
    }
    let levels = items.checked_ilog2().map_or(1, |level| level as usize + 1);
    items.saturating_mul(levels).saturating_mul(4)
}

fn add_weight(left: u128, right: u128) -> Result<u128> {
    left.checked_add(right)
        .ok_or_else(|| gql_execution("count support edge multiplicity exceeds u128"))
}

/// The typed index globally admits at most `u32::MAX` physical edge slots, so
/// any one coalesced endpoint group fits u32. Keep the conversion checked at
/// the storage boundary so that invariant cannot silently drift.
fn compact_multiplicity(multiplicity: u128) -> Result<u32> {
    u32::try_from(multiplicity)
        .map_err(|_| gql_execution("count support multiplicity exceeds the index edge bound"))
}

/// Merge the two sorted CSR slices for one undirected endpoint. Incoming
/// self-loop entries duplicate their outgoing slots and are scanned but not
/// counted twice. Every raw physical slot and grouped endpoint is charged.
fn undirected_groups(
    index: &TypedGraphIndex,
    center: u32,
    relationship: &str,
    mut visit: impl FnMut(u32, u128) -> Result<()>,
) -> Result<()> {
    let outgoing = index.outgoing(center, relationship);
    let incoming = index.incoming(center, relationship);
    let (mut out, mut inc) = (0, 0);
    while out < outgoing.len() || inc < incoming.len() {
        let vertex = match (outgoing.get(out), incoming.get(inc)) {
            (Some(a), Some(b)) => a.vertex.min(b.vertex),
            (Some(a), None) => a.vertex,
            (None, Some(b)) => b.vertex,
            (None, None) => unreachable!(),
        };
        let mut multiplicity = 0u128;
        while outgoing.get(out).is_some_and(|next| next.vertex == vertex) {
            read_budget::charge_candidate_work(1, "scanning weighted support edges")?;
            multiplicity = add_weight(multiplicity, 1)?;
            out += 1;
        }
        while incoming.get(inc).is_some_and(|next| next.vertex == vertex) {
            read_budget::charge_candidate_work(1, "scanning weighted support edges")?;
            if vertex != center {
                multiplicity = add_weight(multiplicity, 1)?;
            }
            inc += 1;
        }
        read_budget::charge_candidate_work(1, "grouping weighted support endpoints")?;
        visit(vertex, multiplicity)?;
    }
    Ok(())
}

fn validate_domain(index: &TypedGraphIndex, vertices: &[u32], vertex_slot: &[u32]) -> Result<()> {
    if vertex_slot.len() != index.graph().nodes.len() {
        return Err(support_error("inverse-map length differs from graph"));
    }
    if vertices.len() > index.graph().nodes.len() {
        return Err(support_error("active domain exceeds graph cardinality"));
    }
    for (ordinal, &vertex) in vertices.iter().enumerate() {
        read_budget::charge_candidate_work(1, "validating weighted support domain")?;
        if vertex_slot.get(vertex as usize).copied() != Some(ordinal as u32) {
            return Err(support_error("inverse map and active vertices disagree"));
        }
    }
    read_budget::checkpoint()
}

fn mapped_slot(neighbor: u32, vertices: &[u32], vertex_slot: &[u32]) -> Result<Option<usize>> {
    let slot = vertex_slot[neighbor as usize];
    if slot == u32::MAX {
        return Ok(None);
    }
    let ordinal = slot as usize;
    if vertices.get(ordinal).copied() != Some(neighbor) {
        return Err(support_error(
            "inverse map contains a foreign active vertex",
        ));
    }
    Ok(Some(ordinal))
}

fn support_size(
    index: &TypedGraphIndex,
    relationship: &str,
    vertices: &[u32],
    vertex_slot: &[u32],
    mut loops: Option<&mut [u128]>,
) -> Result<usize> {
    let mut edge_count = 0usize;
    for (ordinal, &vertex) in vertices.iter().enumerate() {
        // Empty typed adjacency still consumes one unit and checks the deadline.
        read_budget::charge_candidate_work(1, "sizing weighted support vertices")?;
        undirected_groups(index, vertex, relationship, |neighbor, multiplicity| {
            let Some(other) = mapped_slot(neighbor, vertices, vertex_slot)? else {
                return Ok(());
            };
            if other == ordinal {
                if let Some(counts) = loops.as_deref_mut() {
                    counts[ordinal] = multiplicity;
                }
            } else if ordinal < other {
                edge_count = edge_count
                    .checked_add(1)
                    .ok_or_else(|| support_error("edge count overflowed"))?;
            }
            Ok(())
        })?;
    }
    Ok(edge_count)
}

fn fill_support(
    index: &TypedGraphIndex,
    relationship: &str,
    vertices: &[u32],
    vertex_slot: &[u32],
    edges: &mut Vec<WeightedEdge>,
) -> Result<()> {
    for (ordinal, &vertex) in vertices.iter().enumerate() {
        read_budget::charge_candidate_work(1, "filling weighted support vertices")?;
        undirected_groups(index, vertex, relationship, |neighbor, multiplicity| {
            let Some(other) = mapped_slot(neighbor, vertices, vertex_slot)? else {
                return Ok(());
            };
            if ordinal < other {
                if edges.len() == edges.capacity() {
                    return Err(support_error("edge fill exceeded proven capacity"));
                }
                edges.push(WeightedEdge {
                    a: ordinal as u32,
                    b: other as u32,
                    multiplicity: compact_multiplicity(multiplicity)?,
                });
            }
            Ok(())
        })?;
    }
    Ok(())
}

fn build_support(
    index: &TypedGraphIndex,
    relationship: &str,
    vertices: &[u32],
    vertex_slot: &[u32],
    loops: Option<&mut [u128]>,
) -> Result<WeightedSupport> {
    validate_domain(index, vertices, vertex_slot)?;
    let edge_count = support_size(index, relationship, vertices, vertex_slot, loops)?;
    let mut edges = reserved_vec(edge_count, "allocating weighted support edges")?;
    fill_support(index, relationship, vertices, vertex_slot, &mut edges)?;
    if edges.len() != edge_count {
        return Err(support_error("edge set changed between sizing and fill"));
    }
    Ok(WeightedSupport { edges })
}

impl WeightedSupport {
    /// Build only the non-loop support. Loops are scanned for truthful work
    /// accounting, but no loop-sized scratch allocation is made.
    pub(super) fn build(
        index: &TypedGraphIndex,
        relationship: &str,
        vertices: &[u32],
        vertex_slot: &[u32],
    ) -> Result<Self> {
        build_support(index, relationship, vertices, vertex_slot, None)
    }

    /// Build non-loop support and retain one coalesced self-loop multiplicity
    /// for each active vertex.
    pub(super) fn build_with_loops(
        index: &TypedGraphIndex,
        relationship: &str,
        vertices: &[u32],
        vertex_slot: &[u32],
    ) -> Result<(Self, Vec<u128>)> {
        read_budget::charge_candidate_work(vertices.len(), "initializing support loop counts")?;
        let mut loops = reserved_vec(vertices.len(), "allocating support loop counts")?;
        loops.resize(vertices.len(), 0);
        let support = build_support(index, relationship, vertices, vertex_slot, Some(&mut loops))?;
        Ok((support, loops))
    }

    pub(super) fn edges(&self) -> &[WeightedEdge] {
        &self.edges
    }

    /// Orient by `(simple support degree, stable graph vertex slot)`. A
    /// degree-d vertex has at most min(d, 2M/d) later neighbors, which bounds
    /// forward intersections by O(M^(3/2)). Materializing the explicit rank
    /// order itself is a separately charged O(V log V) sort.
    pub(super) fn orient(&self, vertices: &[u32]) -> Result<OrientedSupport> {
        u32::try_from(vertices.len())
            .map_err(|_| support_error("orientation domain exceeds u32 capacity"))?;
        read_budget::charge_candidate_work(vertices.len(), "initializing support degrees")?;
        let mut degrees = reserved_vec(vertices.len(), "allocating support degrees")?;
        degrees.resize(vertices.len(), 0usize);
        for edge in &self.edges {
            read_budget::charge_candidate_work(2, "counting weighted support degrees")?;
            for slot in [edge.a as usize, edge.b as usize] {
                degrees[slot] = degrees[slot]
                    .checked_add(1)
                    .ok_or_else(|| support_error("degree overflowed"))?;
            }
        }

        read_budget::charge_candidate_work(vertices.len(), "initializing support rank order")?;
        let mut rank_to_ordinal =
            reserved_vec(vertices.len(), "allocating support rank-to-ordinal map")?;
        for ordinal in 0..vertices.len() {
            rank_to_ordinal.push(ordinal as u32);
        }
        read_budget::checkpoint()?;
        read_budget::charge_candidate_work(
            sort_work(rank_to_ordinal.len()),
            "sorting support rank order",
        )?;
        rank_to_ordinal.sort_unstable_by_key(|&ordinal| {
            let ordinal = ordinal as usize;
            (degrees[ordinal], vertices[ordinal])
        });
        read_budget::checkpoint()?;

        read_budget::charge_candidate_work(vertices.len(), "initializing support ordinal ranks")?;
        let mut ordinal_to_rank =
            reserved_vec(vertices.len(), "allocating support ordinal-to-rank map")?;
        ordinal_to_rank.resize(vertices.len(), u32::MAX);
        for (rank, &ordinal) in rank_to_ordinal.iter().enumerate() {
            read_budget::charge_candidate_work(1, "indexing support ranks")?;
            ordinal_to_rank[ordinal as usize] = rank as u32;
        }
        drop(degrees);

        read_budget::charge_candidate_work(vertices.len(), "initializing forward degrees")?;
        let mut outdegrees = reserved_vec(vertices.len(), "allocating forward support degrees")?;
        outdegrees.resize(vertices.len(), 0usize);
        for edge in &self.edges {
            read_budget::charge_candidate_work(1, "sizing forward support adjacency")?;
            let (from, _) = ranked(edge, &ordinal_to_rank);
            outdegrees[from] = outdegrees[from]
                .checked_add(1)
                .ok_or_else(|| support_error("forward degree overflowed"))?;
        }

        let mut forward = reserved_vec(
            vertices.len(),
            "allocating forward support adjacency headers",
        )?;
        for &degree in &outdegrees {
            read_budget::charge_candidate_work(1, "initializing forward support adjacency")?;
            forward.push(reserved_vec(
                degree,
                "allocating forward support adjacency slots",
            )?);
        }
        drop(outdegrees);
        for edge in &self.edges {
            read_budget::charge_candidate_work(1, "filling forward support adjacency")?;
            let (from, to) = ranked(edge, &ordinal_to_rank);
            if forward[from].len() == forward[from].capacity() {
                return Err(support_error("forward adjacency exceeded proven capacity"));
            }
            forward[from].push(ForwardEdge {
                vertex: to as u32,
                multiplicity: edge.multiplicity,
            });
        }
        drop(ordinal_to_rank);
        for neighbors in &mut forward {
            read_budget::charge_candidate_work(1, "visiting forward support adjacency")?;
            read_budget::checkpoint()?;
            read_budget::charge_candidate_work(
                sort_work(neighbors.len()),
                "sorting forward support adjacency",
            )?;
            neighbors.sort_unstable_by_key(|neighbor| neighbor.vertex);
            read_budget::checkpoint()?;
        }
        Ok(OrientedSupport {
            forward,
            rank_to_ordinal,
        })
    }
}

fn ranked(edge: &WeightedEdge, ordinal_to_rank: &[u32]) -> (usize, usize) {
    let a = ordinal_to_rank[edge.a as usize] as usize;
    let b = ordinal_to_rank[edge.b as usize] as usize;
    if a < b { (a, b) } else { (b, a) }
}

impl OrientedSupport {
    /// Visit each distinct-vertex support triangle exactly once. `xy`, `xz`,
    /// and `yz` are the physical-slot multiplicities on its three endpoint
    /// pairs; callers decide whether each weight participates in their count.
    pub(super) fn visit_triangles(
        &self,
        mut visit: impl FnMut(WeightedTriangle) -> Result<()>,
    ) -> Result<()> {
        read_budget::checkpoint()?;
        for (x, outgoing) in self.forward.iter().enumerate() {
            read_budget::charge_candidate_work(1, "visiting support triangle vertices")?;
            for (xy_index, xy) in outgoing.iter().enumerate() {
                read_budget::charge_candidate_work(1, "visiting forward support edges")?;
                let y = xy.vertex as usize;
                // Targets are ranks in ascending order. All entries before and
                // including xy are <= y, while every forward neighbor of y is
                // > y, so only the strict suffix can close this triangle.
                let (mut left, mut right) = (xy_index + 1, 0);
                while left < outgoing.len() && right < self.forward[y].len() {
                    read_budget::charge_candidate_work(1, "intersecting forward support")?;
                    match outgoing[left].vertex.cmp(&self.forward[y][right].vertex) {
                        Ordering::Less => left += 1,
                        Ordering::Greater => right += 1,
                        Ordering::Equal => {
                            read_budget::charge_candidate_work(
                                1,
                                "visiting weighted support triangle",
                            )?;
                            visit(WeightedTriangle {
                                x: self.rank_to_ordinal[x] as usize,
                                y: self.rank_to_ordinal[y] as usize,
                                z: self.rank_to_ordinal[outgoing[left].vertex as usize] as usize,
                                xy: u128::from(xy.multiplicity),
                                xz: u128::from(outgoing[left].multiplicity),
                                yz: u128::from(self.forward[y][right].multiplicity),
                            })?;
                            left += 1;
                            right += 1;
                        }
                    }
                }
            }
        }
        read_budget::checkpoint()
    }
}

#[cfg(test)]
mod compact_tests {
    use super::*;

    #[test]
    fn stored_weights_are_compact_and_conversion_is_checked() {
        assert_eq!(size_of::<WeightedEdge>(), 12);
        assert_eq!(size_of::<ForwardEdge>(), 8);
        assert_eq!(
            compact_multiplicity(u128::from(u32::MAX)).unwrap(),
            u32::MAX
        );
        assert!(compact_multiplicity(u128::from(u32::MAX) + 1).is_err());
    }
}

#[cfg(test)]
#[path = "count_support_profile.rs"]
mod profile;

#[cfg(test)]
#[path = "count_support_tests.rs"]
mod tests;
