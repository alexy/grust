//! Proof and exact support-triangle subtraction for anti-wedges.

use super::*;
use crate::read::count_support::WeightedSupport;

const A: u8 = 1;
const B: u8 = 2;
const C: u8 = 4;
const ACTIVE_DOMAIN_CHUNK_SIZE: usize = 256;

pub(super) fn supports(
    optional: &ast::MatchClause,
    with: &ast::WithClause,
    nodes: &[&NodePattern; 4],
    relationship_type: &str,
) -> bool {
    let [path] = optional.patterns.as_slice() else {
        return false;
    };
    let [segment] = path.segments.as_slice() else {
        return false;
    };
    let relation = &segment.relationship;
    let (Some(a), Some(c), Some(d), Some(k)) = (
        &nodes[0].variable,
        &nodes[2].variable,
        &nodes[3].variable,
        &relation.variable,
    ) else {
        return false;
    };
    if !optional.optional
        || optional.where_clause.is_some()
        || path.variable.is_some()
        || path.shortest.is_some()
        || relation.direction != ast::Direction::Undirected
        || relation.length.is_some()
        || relation.properties.is_some()
        || relation.types.as_slice() != [relationship_type]
        || nodes.iter().any(|node| node.variable.as_ref() == Some(k))
    {
        return false;
    }
    for node in [&path.start, &segment.node] {
        if !node.labels.is_empty() || node.properties.is_some() {
            return false;
        }
    }
    if !matches!((&path.start.variable, &segment.node.variable), (Some(left), Some(right))
        if (left == a && right == c) || (left == c && right == a))
    {
        return false;
    }
    let p = &with.projection;
    if p.star
        || p.distinct
        || !p.order_by.is_empty()
        || p.skip.is_some()
        || p.limit.is_some()
        || p.items.len() != 4
    {
        return false;
    }
    let names = [a, c, d, k];
    let mut seen = 0u8;
    for item in &p.items {
        let Expr::Variable(name) = &item.expr else {
            return false;
        };
        let Some(slot) = names.iter().position(|expected| *expected == name) else {
            return false;
        };
        if item.alias.is_some() || seen & (1 << slot) != 0 {
            return false;
        }
        seen |= 1 << slot;
    }
    let Some(Expr::Binary {
        op: BinaryOp::And,
        lhs,
        rhs,
    }) = &with.where_clause
    else {
        return false;
    };
    let null_k = |expr: &Expr| {
        matches!(expr,
        Expr::IsNull { operand, negated: false }
        if matches!(operand.as_ref(), Expr::Variable(name) if name == k))
    };
    (null_k(lhs) && inequality(rhs, a, c)) || (null_k(rhs) && inequality(lhs, a, c))
}

fn active_domain(masks: &[u8]) -> Result<(Vec<u32>, Vec<u32>)> {
    let mut active = 0usize;
    // Each mask costs one work unit. Precharging small, fixed-cost chunks
    // bounds deadline-check spacing without a TLS lookup for every byte.
    for chunk in masks.chunks(ACTIVE_DOMAIN_CHUNK_SIZE) {
        read_budget::charge_candidate_work(chunk.len(), "sizing anti-wedge active vertices")?;
        for &mask in chunk {
            if mask & (A | B | C) != 0 {
                active = active.checked_add(1).ok_or_else(arithmetic_overflow)?;
            }
        }
    }
    read_budget::checkpoint()?;
    let mut vertices = reserved_vec(active, "allocating anti-wedge active vertices")?;
    read_budget::charge_candidate_work(masks.len(), "initializing anti-wedge inverse map")?;
    let mut vertex_slot = reserved_vec(masks.len(), "allocating anti-wedge inverse map")?;
    vertex_slot.resize(masks.len(), u32::MAX);
    for (chunk_index, chunk) in masks.chunks(ACTIVE_DOMAIN_CHUNK_SIZE).enumerate() {
        read_budget::charge_candidate_work(chunk.len(), "indexing anti-wedge active vertices")?;
        for (offset, &mask) in chunk.iter().enumerate() {
            if mask & (A | B | C) == 0 {
                continue;
            }
            if vertices.len() == vertices.capacity() {
                return Err(gql_execution(
                    "anti-wedge active vertices exceeded their proven capacity",
                ));
            }
            let vertex = chunk_index * ACTIVE_DOMAIN_CHUNK_SIZE + offset;
            vertex_slot[vertex] = vertices.len() as u32;
            vertices.push(vertex as u32);
        }
    }
    read_budget::checkpoint()?;
    if vertices.len() != active {
        return Err(gql_execution(
            "anti-wedge active vertex set changed between sizing and fill",
        ));
    }
    Ok((vertices, vertex_slot))
}

fn add_total(total: &mut u128, term: u128) -> Result<()> {
    *total = total.checked_add(term).ok_or_else(arithmetic_overflow)?;
    Ok(())
}

fn base_direction(
    masks: &[u8],
    leaves: &[u64],
    degree: u128,
    center: usize,
    endpoint: usize,
    multiplicity: u128,
) -> Result<u128> {
    read_budget::charge_candidate_work(1, "placing anti-wedge support edge")?;
    if masks[center] & B == 0 || masks[endpoint] & C == 0 || leaves[endpoint] == 0 {
        return Ok(0);
    }
    let equality = if masks[endpoint] & A != 0 {
        multiplicity
    } else {
        0
    };
    weighted_count(degree, equality, multiplicity, leaves[endpoint])
}

fn triangle_placement(
    masks: &[u8],
    leaves: &[u64],
    a: usize,
    b: usize,
    c: usize,
    ab: u128,
    bc: u128,
) -> Result<u128> {
    read_budget::charge_candidate_work(1, "placing anti-wedge support triangle")?;
    if masks[a] & A == 0 || masks[b] & B == 0 || masks[c] & C == 0 || leaves[c] == 0 {
        return Ok(0);
    }
    ab.checked_mul(bc)
        .and_then(|value| value.checked_mul(u128::from(leaves[c])))
        .ok_or_else(arithmetic_overflow)
}

/// Count the equality-adjusted wedge base and the forbidden distinct-vertex
/// support triangles separately. A closure edge is existence evidence only;
/// its multiplicity never enters the excluded term.
fn count_parts(
    index: &TypedGraphIndex,
    relationship: &str,
    masks: &[u8],
    leaves: &[u64],
) -> Result<(u128, u128)> {
    let graph_len = index.graph().nodes.len();
    if masks.len() != graph_len || leaves.len() != graph_len {
        return Err(gql_execution(
            "anti-wedge role masks and leaves must cover the indexed graph",
        ));
    }
    let (vertices, vertex_slot) = active_domain(masks)?;
    let support = WeightedSupport::build(index, relationship, &vertices, &vertex_slot)?;
    drop(vertex_slot);

    read_budget::charge_candidate_work(vertices.len(), "initializing anti-wedge A degrees")?;
    let mut a_degree = reserved_vec(vertices.len(), "allocating anti-wedge A degrees")?;
    a_degree.resize(vertices.len(), 0u128);
    for edge in support.edges() {
        read_budget::charge_candidate_work(2, "counting anti-wedge A degrees")?;
        let (x, y) = (edge.a as usize, edge.b as usize);
        if masks[vertices[y] as usize] & A != 0 {
            a_degree[x] = a_degree[x]
                .checked_add(u128::from(edge.multiplicity))
                .ok_or_else(arithmetic_overflow)?;
        }
        if masks[vertices[x] as usize] & A != 0 {
            a_degree[y] = a_degree[y]
                .checked_add(u128::from(edge.multiplicity))
                .ok_or_else(arithmetic_overflow)?;
        }
    }

    let mut base = 0u128;
    for edge in support.edges() {
        let (x, y) = (edge.a as usize, edge.b as usize);
        let (vx, vy) = (vertices[x] as usize, vertices[y] as usize);
        let multiplicity = u128::from(edge.multiplicity);
        let forward = base_direction(masks, leaves, a_degree[x], vx, vy, multiplicity)?;
        add_total(&mut base, forward)?;
        let reverse = base_direction(masks, leaves, a_degree[y], vy, vx, multiplicity)?;
        add_total(&mut base, reverse)?;
    }
    drop(a_degree);

    let oriented = support.orient(&vertices)?;
    let mut excluded = 0u128;
    oriented.visit_triangles(|triangle| {
        let (x, y, z) = (
            vertices[triangle.x] as usize,
            vertices[triangle.y] as usize,
            vertices[triangle.z] as usize,
        );
        for term in [
            triangle_placement(masks, leaves, y, x, z, triangle.xy, triangle.xz)?,
            triangle_placement(masks, leaves, z, x, y, triangle.xz, triangle.xy)?,
            triangle_placement(masks, leaves, x, y, z, triangle.xy, triangle.yz)?,
            triangle_placement(masks, leaves, z, y, x, triangle.yz, triangle.xy)?,
            triangle_placement(masks, leaves, x, z, y, triangle.xz, triangle.yz)?,
            triangle_placement(masks, leaves, y, z, x, triangle.yz, triangle.xz)?,
        ] {
            add_total(&mut excluded, term)?;
        }
        Ok(())
    })?;
    Ok((base, excluded))
}

pub(super) fn count(
    index: &TypedGraphIndex,
    relationship: &str,
    masks: &[u8],
    leaves: &[u64],
) -> Result<u128> {
    let (base, excluded) = count_parts(index, relationship, masks, leaves)?;
    base.checked_sub(excluded)
        .ok_or_else(|| gql_execution("anti-wedge triangle exclusion exceeds exact base"))
}

#[cfg(test)]
mod active_domain_tests;
#[cfg(test)]
mod budget_tests;
#[cfg(test)]
mod count_tests;
