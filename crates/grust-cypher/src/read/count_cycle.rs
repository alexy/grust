//! Count a proven directed creator/reply cycle closed by one undirected edge.
//! R(c,p), H(c,u), H(p,v), K(u,v); conflicting literal string constraints
//! prove c != p, so the two H edges cannot share a physical identity.

use super::count_predicate::{node_matches, props_match};
use super::*;
use grust_core::{TypedGraphIndex, TypedNeighbor};

mod plan;
use plan::Cycle;

pub(super) fn supports(query: &Query) -> Result<bool> {
    Ok(plan::plan(query)?.is_some())
}

fn overflow() -> GrustError {
    gql_execution("cycle count arithmetic exceeds u128")
}

fn add(left: u128, right: u128) -> Result<u128> {
    left.checked_add(right).ok_or_else(overflow)
}

fn multiply(left: u128, right: u128) -> Result<u128> {
    left.checked_mul(right).ok_or_else(overflow)
}

/// Borrowed sorted target groups; parallel/reciprocal edges retain multiplicity.
/// For undirected K, merge outgoing and incoming slices, skipping the second
/// appearance of each self-loop. No per-neighborhood buffers are allocated.
struct Groups<'a> {
    center: u32,
    outgoing: &'a [TypedNeighbor],
    incoming: &'a [TypedNeighbor],
    out: usize,
    inc: usize,
}

impl<'a> Groups<'a> {
    fn new(index: &'a TypedGraphIndex, center: u32, kind: &str, undirected: bool) -> Self {
        Self {
            center,
            outgoing: index.outgoing(center, kind),
            incoming: if undirected {
                index.incoming(center, kind)
            } else {
                &[]
            },
            out: 0,
            inc: 0,
        }
    }

    fn next(
        &mut self,
        graph: &Graph,
        rel: &RelationshipPattern,
        masks: &[u8],
        bit: u8,
        params: &CypherParameters,
    ) -> Result<Option<(u32, u128)>> {
        let vertex = match (self.outgoing.get(self.out), self.incoming.get(self.inc)) {
            (Some(a), Some(b)) => a.vertex.min(b.vertex),
            (Some(a), None) => a.vertex,
            (None, Some(b)) => b.vertex,
            (None, None) => return Ok(None),
        };
        read_budget::charge_candidate_work(1, "grouping cycle adjacency targets")?;
        let mut count = 0;
        while self
            .outgoing
            .get(self.out)
            .is_some_and(|edge| edge.vertex == vertex)
        {
            read_budget::charge_candidate_work(1, "scanning cycle adjacency edges")?;
            if masks[vertex as usize] & bit != 0
                && props_match(
                    &graph.edges[self.outgoing[self.out].edge as usize].props,
                    rel.properties.as_ref(),
                    params,
                )?
            {
                count = add(count, 1)?;
            }
            self.out += 1;
        }
        while self
            .incoming
            .get(self.inc)
            .is_some_and(|edge| edge.vertex == vertex)
        {
            read_budget::charge_candidate_work(1, "scanning cycle adjacency edges")?;
            if vertex != self.center
                && masks[vertex as usize] & bit != 0
                && props_match(
                    &graph.edges[self.incoming[self.inc].edge as usize].props,
                    rel.properties.as_ref(),
                    params,
                )?
            {
                count = add(count, 1)?;
            }
            self.inc += 1;
        }
        Ok(Some((vertex, count)))
    }
}

fn intersect(
    index: &TypedGraphIndex,
    cycle: &Cycle<'_>,
    masks: &[u8],
    post: u32,
    person: u32,
    params: &CypherParameters,
) -> Result<u128> {
    read_budget::charge_candidate_work(1, "starting cycle adjacency intersection")?;
    let creator = cycle.creators[1];
    let mut creators = Groups::new(index, post, &creator.types[0], false);
    let mut knows = Groups::new(index, person, &cycle.knows.types[0], true);
    let depth = |len: usize| len.checked_ilog2().map_or(0, |n| n as usize + 1);
    let probe_cost = creators
        .outgoing
        .len()
        .saturating_mul(depth(knows.outgoing.len()) + depth(knows.incoming.len()) + 1);
    let merge_cost = creators
        .outgoing
        .len()
        .saturating_add(knows.outgoing.len())
        .saturating_add(knows.incoming.len());
    // Size-based estimate, not a functional-creator assumption: grouping still
    // preserves arbitrary creator multiplicity, and dense lists retain merging.
    if probe_cost < merge_cost {
        let mut count = 0;
        while let Some((target, multiplicity)) =
            creators.next(index.graph(), creator, masks, 8, params)?
        {
            read_budget::charge_candidate_work(1, "visiting cycle probe targets")?;
            if multiplicity == 0 {
                continue;
            }
            let outgoing = probe(index.graph(), knows.outgoing, target, cycle.knows, params)?;
            let incoming = if target == person {
                0
            } else {
                probe(index.graph(), knows.incoming, target, cycle.knows, params)?
            };
            count = add(count, multiply(multiplicity, add(outgoing, incoming)?)?)?;
        }
        return Ok(count);
    }
    let mut left = creators.next(index.graph(), creator, masks, 8, params)?;
    let mut right = knows.next(index.graph(), cycle.knows, masks, 8, params)?;
    let mut count = 0;
    while let (Some((a, ac)), Some((b, bc))) = (left, right) {
        read_budget::charge_candidate_work(1, "intersecting cycle adjacency targets")?;
        if a == b {
            count = add(count, multiply(ac, bc)?)?;
        }
        if a <= b {
            left = creators.next(index.graph(), creator, masks, 8, params)?;
        }
        if b <= a {
            right = knows.next(index.graph(), cycle.knows, masks, 8, params)?;
        }
    }
    Ok(count)
}

/// Find just one sorted physical target group, charging every binary-search
/// comparison and every qualifying-group edge visit. No key/property copies.
fn probe(
    graph: &Graph,
    neighbors: &[TypedNeighbor],
    target: u32,
    rel: &RelationshipPattern,
    params: &CypherParameters,
) -> Result<u128> {
    let (mut low, mut high) = (0, neighbors.len());
    while low < high {
        read_budget::charge_candidate_work(1, "probing cycle adjacency targets")?;
        let middle = low + (high - low) / 2;
        if neighbors[middle].vertex < target {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    let mut count = 0;
    while neighbors.get(low).is_some_and(|edge| edge.vertex == target) {
        read_budget::charge_candidate_work(1, "scanning cycle probe edges")?;
        if props_match(
            &graph.edges[neighbors[low].edge as usize].props,
            rel.properties.as_ref(),
            params,
        )? {
            count = add(count, 1)?;
        }
        low += 1;
    }
    Ok(count)
}

/// Only O(V) role masks and constant cursor state beyond the borrowed plan.
/// Grouped reply/creator edges avoid duplicate work for parallel edges. Work
/// still depends on creator/K neighborhoods per distinct reply endpoint pair;
/// no functional creator constraint or linear-time complexity is assumed.
pub(super) fn try_execute(
    index: &TypedGraphIndex,
    query: &Query,
    params: &CypherParameters,
) -> Result<Option<CypherResultTable>> {
    let Some(cycle) = plan::plan(query)? else {
        return Ok(None);
    };
    let graph = index.graph();
    read_budget::charge_intermediate_bytes(graph.nodes.len(), "allocating cycle role masks")?;
    read_budget::charge_candidate_work(graph.nodes.len(), "initializing cycle role masks")?;
    let mut masks = vec![0u8; graph.nodes.len()];
    for (role, &slot) in cycle.roles.iter().enumerate() {
        // A required label can seed candidates even when its first mention
        // was bare. Every mention remains a conjunct; unlabeled roles scan V.
        let label = cycle.nodes[slot].iter().find_map(|p| p.labels.first());
        let candidates = if let Some(label) = label {
            // Hashing/comparing a borrowed label costs bytes even on a miss.
            read_budget::charge_candidate_work(
                label.len().saturating_mul(2).saturating_add(1),
                "looking up cycle candidate labels",
            )?;
            Some(index.vertices_with_label(label))
        } else {
            None
        };
        for candidate in 0..candidates.map_or(graph.nodes.len(), |vertices| vertices.len()) {
            let vertex = candidates.map_or(candidate, |vertices| vertices[candidate] as usize);
            let mut matches = true;
            for pattern in &cycle.nodes[slot] {
                read_budget::charge_candidate_work(1, "filtering cycle node mentions")?;
                if !node_matches(&graph.nodes[vertex], pattern, params)? {
                    matches = false;
                    break;
                }
            }
            if matches {
                masks[vertex] |= 1 << role;
            }
        }
    }
    let mut count = 0;
    for (comment, mask) in masks.iter().enumerate() {
        read_budget::charge_candidate_work(1, "visiting cycle reply sources")?;
        if mask & 1 == 0 {
            continue;
        }
        let mut replies = Groups::new(index, comment as u32, &cycle.reply.types[0], false);
        while let Some((post, reply_count)) = replies.next(graph, cycle.reply, &masks, 2, params)? {
            if reply_count == 0 {
                continue;
            }
            read_budget::charge_candidate_work(1, "visiting cycle reply pairs")?;
            let creator = cycle.creators[0];
            let mut creators = Groups::new(index, comment as u32, &creator.types[0], false);
            while let Some((person, creator_count)) =
                creators.next(graph, creator, &masks, 4, params)?
            {
                if creator_count == 0 {
                    continue;
                }
                let matches = intersect(index, &cycle, &masks, post, person, params)?;
                count = add(
                    count,
                    multiply(multiply(reply_count, creator_count)?, matches)?,
                )?;
            }
        }
    }
    scalar_table(count, cycle.projection).map(Some)
}

fn scalar_table(count: u128, p: &Projection) -> Result<CypherResultTable> {
    let count = i64::try_from(count).map_err(|_| gql_execution("count result exceeds int64"))?;
    let suppressed =
        plan::scalar_bound(p.skip.as_ref()).unwrap() > 0 || p.limit == Some(Expr::Integer(0));
    read_budget::charge_intermediate_bytes(
        128usize.saturating_add(p.items[0].alias.as_ref().map_or(4, String::len)),
        "shaping scalar count result",
    )?;
    Ok(CypherResultTable {
        columns: vec![p.items[0].alias.clone().unwrap_or_else(|| "expr".into())],
        rows: if suppressed {
            Vec::new()
        } else {
            vec![vec![Value::Int(count)]]
        },
    })
}

#[cfg(test)]
mod tests;
