//! Exact two-hop undirected wedge counts with distinct outer endpoints.
//!
//! For each center b and next vertex c, count
//! `(degree_a(b) - multiplicity_a(b,c)) * multiplicity(b,c) * leaf_count(c)`.
//! The inequality a != c proves the two T edges cannot share an identity;
//! the final U edge has a different type. Graph vertices may otherwise alias.
//! The index validates unique node IDs, so slot inequality agrees with the
//! reference executor's node inequality for these directly bound variables.
//! The anti-join variant subtracts forbidden endpoint closures by enumerating
//! the weighted simple-support triangles once, rather than probing an
//! adjacency for every materialized wedge.

use super::*;
use grust_core::TypedGraphIndex;
use std::mem::size_of;

mod anti;
mod base;
mod groups;
mod role_masks;

use groups::groups;

struct Wedge<'a> {
    nodes: [&'a NodePattern; 4],
    repeated_type: &'a str,
    leaf_type: &'a str,
    projection: &'a Projection,
    anti: bool,
}

fn scalar_bound(expr: Option<&Expr>) -> Option<u64> {
    match expr {
        None => Some(0),
        Some(Expr::Integer(value)) if *value >= 0 => Some(*value as u64),
        _ => None,
    }
}

/// Structural eligibility is shared by public classification and execution.
/// No property maps are accepted, even empty maps: labels are the only filters.
fn plan(query: &Query) -> Result<Option<Wedge<'_>>> {
    if query.parts.len() != 1 || query.parts[0].union.is_some() {
        return Ok(None);
    }
    let (m, ret, anti) = match &query.parts[0].query.clauses[..] {
        [Clause::Match(m), Clause::Return(ret)] => (m, ret, None),
        [
            Clause::Match(m),
            Clause::Match(optional),
            Clause::With(with),
            Clause::Return(ret),
        ] => (m, ret, Some((optional, with))),
        _ => return Ok(None),
    };
    let [path] = &m.patterns[..] else {
        return Ok(None);
    };
    let [first, second, third] = &path.segments[..] else {
        return Ok(None);
    };
    if m.optional || path.variable.is_some() || path.shortest.is_some() {
        return Ok(None);
    }
    read_budget::charge_candidate_work(4, "proving count wedge")?;
    let nodes = [&path.start, &first.node, &second.node, &third.node];
    for (i, node) in nodes.iter().enumerate() {
        if node.properties.is_some()
            || node.variable.as_ref().is_some_and(|name| {
                nodes[..i]
                    .iter()
                    .any(|earlier| earlier.variable.as_ref() == Some(name))
            })
        {
            return Ok(None);
        }
    }
    let (Some(a), Some(c)) = (&nodes[0].variable, &nodes[2].variable) else {
        return Ok(None);
    };
    if anti.is_none()
        && !m
            .where_clause
            .as_ref()
            .is_some_and(|expr| inequality(expr, a, c))
    {
        return Ok(None);
    }
    if anti.is_some() && m.where_clause.is_some() {
        return Ok(None);
    }
    let relationships = [
        &first.relationship,
        &second.relationship,
        &third.relationship,
    ];
    for relationship in relationships {
        if relationship.variable.is_some()
            || relationship.length.is_some()
            || relationship.properties.is_some()
            || relationship.types.len() != 1
        {
            return Ok(None);
        }
    }
    if relationships[0].direction != ast::Direction::Undirected
        || relationships[1].direction != ast::Direction::Undirected
        || relationships[2].direction != ast::Direction::Outgoing
        || relationships[0].types[0] != relationships[1].types[0]
        || relationships[0].types[0] == relationships[2].types[0]
    {
        return Ok(None);
    }
    if let Some((optional, with)) = anti {
        read_budget::charge_candidate_work(8, "proving count wedge anti-join")?;
        if !anti::supports(optional, with, &nodes, &relationships[0].types[0]) {
            return Ok(None);
        }
    }
    let p = &ret.projection;
    if p.star
        || p.distinct
        || p.items.len() != 1
        || !p.order_by.is_empty()
        || scalar_bound(p.skip.as_ref()).is_none()
        || scalar_bound(p.limit.as_ref()).is_none()
        || !matches!(&p.items[0].expr,
            Expr::Function { name, distinct: false, star: true, args }
            if name.eq_ignore_ascii_case("count") && args.is_empty())
    {
        return Ok(None);
    }
    Ok(Some(Wedge {
        nodes,
        repeated_type: &relationships[0].types[0],
        leaf_type: &relationships[2].types[0],
        projection: p,
        anti: anti.is_some(),
    }))
}

fn inequality(expr: &Expr, a: &str, c: &str) -> bool {
    matches!(expr, Expr::Binary { op: BinaryOp::Ne, lhs, rhs }
        if matches!((lhs.as_ref(), rhs.as_ref()), (Expr::Variable(left), Expr::Variable(right))
            if (left == a && right == c) || (left == c && right == a)))
}

pub(super) fn supports(query: &Query) -> Result<bool> {
    Ok(plan(query)?.is_some())
}

fn arithmetic_overflow() -> GrustError {
    gql_execution("count wedge arithmetic exceeds u128")
}

fn allocation_bytes<T>(items: usize, context: &str) -> Result<usize> {
    items
        .checked_mul(size_of::<T>())
        .ok_or_else(|| gql_execution(format!("count wedge allocation overflowed while {context}")))
}

fn reserved_vec<T>(items: usize, context: &str) -> Result<Vec<T>> {
    read_budget::charge_intermediate_bytes(allocation_bytes::<T>(items, context)?, context)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(items)
        .map_err(|_| gql_execution(format!("count wedge allocation failed while {context}")))?;
    Ok(values)
}

fn weighted_count(degree: u128, excluded: u128, multiplicity: u128, leaves: u64) -> Result<u128> {
    let allowed = degree
        .checked_sub(excluded)
        .ok_or_else(|| gql_execution("count wedge exclusion exceeds exact degree"))?;
    allowed
        .checked_mul(multiplicity)
        .and_then(|value| value.checked_mul(u128::from(leaves)))
        .ok_or_else(arithmetic_overflow)
}

pub(super) fn try_execute(
    index: &TypedGraphIndex,
    query: &Query,
) -> Result<Option<CypherResultTable>> {
    let Some(wedge) = plan(query)? else {
        return Ok(None);
    };
    let graph = index.graph();
    let roles = role_masks::prepare(index, &wedge)?;
    let masks = roles.masks();
    read_budget::charge_candidate_work(graph.nodes.len(), "initializing count wedge leaf counts")?;
    let mut leaves = reserved_vec(graph.nodes.len(), "allocating count wedge leaf counts")?;
    leaves.resize(graph.nodes.len(), 0u64);
    for vertex in roles.c_candidates() {
        read_budget::charge_candidate_work(1, "counting wedge leaves")?;
        if masks[vertex] & 4 == 0 {
            continue;
        }
        let leaf_count = &mut leaves[vertex];
        for neighbor in index.outgoing(vertex as u32, wedge.leaf_type) {
            read_budget::charge_candidate_work(1, "scanning count wedge leaf edges")?;
            if masks[neighbor.vertex as usize] & 8 != 0 {
                // A validated TypedGraphIndex has at most u32::MAX edges.
                *leaf_count += 1;
            }
        }
    }
    let count = if wedge.anti {
        anti::count(index, wedge.repeated_type, masks, &leaves)?
    } else {
        base::count(
            index,
            wedge.repeated_type,
            masks,
            &leaves,
            roles.b_candidates(),
        )?
    };
    scalar_table(count, wedge.projection).map(Some)
}

fn scalar_table(count: u128, p: &Projection) -> Result<CypherResultTable> {
    // No capping before subtraction. In fact the index's u32 edge bound makes
    // the entire three-edge count <= 4 * E^3 < 2^98, well within u128.
    let count = i64::try_from(count).map_err(|_| gql_execution("count result exceeds int64"))?;
    let suppressed =
        scalar_bound(p.skip.as_ref()).unwrap() > 0 || p.limit == Some(Expr::Integer(0));
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

#[cfg(test)]
mod anti_tests;

#[cfg(test)]
mod anti_triangle_tests;
