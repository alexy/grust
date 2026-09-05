//! Exact directed two-source counts: `(a)<-[:R]-(m)<-[:U]-(c)-[:R]->(b)`.
//! Direct node inequality proves the two R edges have different identities,
//! even when m and c are the same physical vertex. An optional bare c-R-a
//! anti-edge excludes a by existence, independent of b's filters/multiplicity.

use super::count_predicate::{node_matches, props_match};
use super::*;
use grust_core::TypedGraphIndex;

struct Tags<'a> {
    nodes: [&'a NodePattern; 4],
    relationships: [&'a RelationshipPattern; 3],
    anti: bool,
    projection: &'a Projection,
}

fn literal_properties(properties: Option<&MapLiteral>) -> Result<bool> {
    if let Some(map) = properties {
        for (_, expr) in &map.entries {
            read_budget::charge_candidate_work(1, "proving tag literal predicates")?;
            if !matches!(
                expr,
                Expr::Null | Expr::Boolean(_) | Expr::Integer(_) | Expr::Float(_) | Expr::String(_)
            ) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn scalar_bound(expr: Option<&Expr>) -> Option<u64> {
    match expr {
        None => Some(0),
        Some(Expr::Integer(value)) if *value >= 0 => Some(*value as u64),
        _ => None,
    }
}

fn inequality(expr: &Expr, first: &str, last: &str) -> bool {
    matches!(expr, Expr::Binary { op: BinaryOp::Ne, lhs, rhs }
        if matches!((lhs.as_ref(), rhs.as_ref()), (Expr::Variable(a), Expr::Variable(b))
            if (a == first && b == last) || (a == last && b == first)))
}

fn anti_suffix(optional: &MatchClause, with: &WithClause, names: [&str; 4], kind: &str) -> bool {
    let [path] = &optional.patterns[..] else {
        return false;
    };
    let [segment] = &path.segments[..] else {
        return false;
    };
    let rel = &segment.relationship;
    let Some(h) = rel.variable.as_deref() else {
        return false;
    };
    if !optional.optional
        || optional.where_clause.is_some()
        || path.variable.is_some()
        || path.shortest.is_some()
        || path.start.variable.as_deref() != Some(names[2])
        || segment.node.variable.as_deref() != Some(names[0])
        || !path.start.labels.is_empty()
        || !segment.node.labels.is_empty()
        || path.start.properties.is_some()
        || segment.node.properties.is_some()
        || names.contains(&h)
        || rel.types.len() != 1
        || rel.types[0] != kind
        || rel.direction != ast::Direction::Outgoing
        || rel.length.is_some()
        || rel.properties.is_some()
    {
        return false;
    }
    let p = &with.projection;
    if p.star
        || p.distinct
        || !p.order_by.is_empty()
        || p.skip.is_some()
        || p.limit.is_some()
        || p.items.len() != 3
    {
        return false;
    }
    // Projecting away m/c retains the incoming bag. No DISTINCT, expressions,
    // aliases or duplicated names can change that bag or the NULL predicate.
    if ![names[0], names[3], h].iter().all(|name| {
        p.items
            .iter()
            .filter(|item| {
                item.alias.is_none()
                    && matches!(&item.expr, Expr::Variable(value) if value.as_str() == *name)
            })
            .count()
            == 1
    }) {
        return false;
    }
    let Some(Expr::Binary {
        op: BinaryOp::And,
        lhs,
        rhs,
    }) = &with.where_clause
    else {
        return false;
    };
    let null_test = |expr: &Expr| {
        matches!(expr,
        Expr::IsNull { operand, negated: false }
        if matches!(operand.as_ref(), Expr::Variable(name) if name == h))
    };
    (null_test(lhs) && inequality(rhs, names[0], names[3]))
        || (null_test(rhs) && inequality(lhs, names[0], names[3]))
}

/// The complete structural proof is shared by classification and execution.
/// Four logical names must differ; physical vertices may coincide freely.
fn plan(query: &Query) -> Result<Option<Tags<'_>>> {
    read_budget::checkpoint()?;
    if query.parts.len() != 1 || query.parts[0].union.is_some() {
        return Ok(None);
    }
    let (m, suffix, ret) = match &query.parts[0].query.clauses[..] {
        [Clause::Match(m), Clause::Return(ret)] => (m, None, ret),
        [
            Clause::Match(m),
            Clause::Match(optional),
            Clause::With(w),
            Clause::Return(ret),
        ] => (m, Some((optional, w)), ret),
        _ => return Ok(None),
    };
    let [path] = &m.patterns[..] else {
        return Ok(None);
    };
    let [first, middle, last] = &path.segments[..] else {
        return Ok(None);
    };
    if m.optional || path.variable.is_some() || path.shortest.is_some() {
        return Ok(None);
    }
    read_budget::charge_candidate_work(7, "proving directed tag count")?;
    let nodes = [&path.start, &first.node, &middle.node, &last.node];
    let [Some(a), Some(message), Some(comment), Some(b)] =
        nodes.map(|node| node.variable.as_deref())
    else {
        return Ok(None);
    };
    let names = [a, message, comment, b];
    for (i, node) in nodes.iter().enumerate() {
        if names[..i].contains(&names[i]) || !literal_properties(node.properties.as_ref())? {
            return Ok(None);
        }
    }
    let relationships = [
        &first.relationship,
        &middle.relationship,
        &last.relationship,
    ];
    for rel in relationships {
        if rel.variable.is_some()
            || rel.length.is_some()
            || rel.types.len() != 1
            || !literal_properties(rel.properties.as_ref())?
        {
            return Ok(None);
        }
    }
    if relationships[0].direction != ast::Direction::Incoming
        || relationships[1].direction != ast::Direction::Incoming
        || relationships[2].direction != ast::Direction::Outgoing
        || relationships[0].types[0] != relationships[2].types[0]
        || relationships[0].types[0] == relationships[1].types[0]
    {
        return Ok(None);
    }
    let anti = if let Some((optional, with)) = suffix {
        read_budget::charge_candidate_work(5, "proving tag anti-join scope")?;
        if m.where_clause.is_some()
            || !anti_suffix(optional, with, names, &relationships[0].types[0])
        {
            return Ok(None);
        }
        true
    } else {
        if !m
            .where_clause
            .as_ref()
            .is_some_and(|expr| inequality(expr, a, b))
        {
            return Ok(None);
        }
        false
    };
    let p = &ret.projection;
    if p.star
        || p.distinct
        || p.items.len() != 1
        || !p.order_by.is_empty()
        || scalar_bound(p.skip.as_ref()).is_none()
        || scalar_bound(p.limit.as_ref()).is_none()
        || !matches!(&p.items[0].expr, Expr::Function { name, distinct: false, star: true, args }
            if name.eq_ignore_ascii_case("count") && args.is_empty())
    {
        return Ok(None);
    }
    Ok(Some(Tags {
        nodes,
        relationships,
        anti,
        projection: p,
    }))
}

pub(super) fn supports(query: &Query) -> Result<bool> {
    Ok(plan(query)?.is_some())
}

fn overflow() -> GrustError {
    gql_execution("directed tag count arithmetic exceeds u128")
}

fn contribution(
    left: u128,
    right: u128,
    excluded: u128,
    bridges: u128,
    anti: bool,
) -> Result<u128> {
    let base = if anti {
        left
    } else {
        left.checked_mul(right).ok_or_else(overflow)?
    };
    let allowed = base
        .checked_sub(excluded)
        .ok_or_else(|| gql_execution("tag exclusion exceeds exact cardinality"))?;
    let pairs = if anti {
        allowed.checked_mul(right).ok_or_else(overflow)?
    } else {
        allowed
    };
    pairs.checked_mul(bridges).ok_or_else(overflow)
}

/// Merge sorted raw target groups. For anti mode the right group is an
/// existence witness, NOT a filtered t2 group or an edge multiplicity factor.
fn exclusion(
    index: &TypedGraphIndex,
    tags: &Tags<'_>,
    masks: &[u8],
    m: u32,
    c: u32,
    params: &CypherParameters,
) -> Result<u128> {
    let kind = &tags.relationships[0].types[0];
    let (left, right) = (index.outgoing(m, kind), index.outgoing(c, kind));
    let (mut i, mut j, mut excluded) = (0, 0, 0u128);
    while i < left.len() && j < right.len() {
        read_budget::charge_candidate_work(1, "intersecting tag target groups")?;
        let (a, b) = (left[i].vertex, right[j].vertex);
        if a < b {
            i += 1;
            continue;
        }
        if b < a {
            j += 1;
            continue;
        }
        let (mut left_count, mut right_count) = (0u128, 0u128);
        while i < left.len() && left[i].vertex == a {
            read_budget::charge_candidate_work(1, "scanning tag intersection edges")?;
            if masks[a as usize] & 1 != 0
                && props_match(
                    &index.graph().edges[left[i].edge as usize].props,
                    tags.relationships[0].properties.as_ref(),
                    params,
                )?
            {
                left_count += 1;
            }
            i += 1;
        }
        while j < right.len() && right[j].vertex == a {
            read_budget::charge_candidate_work(1, "scanning tag intersection edges")?;
            if !tags.anti
                && masks[a as usize] & 8 != 0
                && props_match(
                    &index.graph().edges[right[j].edge as usize].props,
                    tags.relationships[2].properties.as_ref(),
                    params,
                )?
            {
                right_count += 1;
            }
            j += 1;
        }
        let term = if tags.anti {
            left_count
        } else {
            left_count.checked_mul(right_count).ok_or_else(overflow)?
        };
        excluded = excluded.checked_add(term).ok_or_else(overflow)?;
    }
    Ok(excluded)
}

/// O(V + E) auxiliary storage; no match rows. Precompute role masks/degrees,
/// then intersect once per distinct qualifying U endpoint pair, not per
/// parallel U edge. Work includes all zero-degree source visits and allocations.
pub(super) fn try_execute(
    index: &TypedGraphIndex,
    query: &Query,
    params: &CypherParameters,
) -> Result<Option<CypherResultTable>> {
    let Some(tags) = plan(query)? else {
        return Ok(None);
    };
    let graph = index.graph();
    read_budget::charge_intermediate_bytes(graph.nodes.len(), "allocating tag node masks")?;
    read_budget::charge_candidate_work(graph.nodes.len(), "initializing tag node masks")?;
    let mut masks = vec![0u8; graph.nodes.len()];
    for (node, mask) in graph.nodes.iter().zip(&mut masks) {
        read_budget::charge_candidate_work(4, "filtering tag vertices")?;
        for (role, pattern) in tags.nodes.iter().enumerate() {
            if node_matches(node, pattern, params)? {
                *mask |= 1 << role;
            }
        }
    }
    read_budget::charge_intermediate_bytes(
        graph.nodes.len().saturating_mul(16),
        "allocating tag degrees",
    )?;
    read_budget::charge_candidate_work(graph.nodes.len(), "initializing tag degrees")?;
    let mut degrees = vec![[0u64; 2]; graph.nodes.len()];
    for (vertex, degree) in degrees.iter_mut().enumerate() {
        read_budget::charge_candidate_work(1, "counting tag source degrees")?;
        for (side, source_bit, target_bit, rel) in [
            (0, 2, 1, tags.relationships[0]),
            (1, 4, 8, tags.relationships[2]),
        ] {
            if masks[vertex] & source_bit == 0 {
                continue;
            }
            for neighbor in index.outgoing(vertex as u32, &rel.types[0]) {
                read_budget::charge_candidate_work(1, "scanning filtered tag edges")?;
                if masks[neighbor.vertex as usize] & target_bit != 0
                    && props_match(
                        &graph.edges[neighbor.edge as usize].props,
                        rel.properties.as_ref(),
                        params,
                    )?
                {
                    // Every degree is bounded by the validated u32 edge count.
                    degree[side] += 1;
                }
            }
        }
    }
    let mut count = 0u128;
    for (c, mask) in masks.iter().enumerate() {
        read_budget::charge_candidate_work(1, "visiting tag bridge sources")?;
        if mask & 4 == 0 {
            continue;
        }
        let bridge = tags.relationships[1];
        let neighbors = index.outgoing(c as u32, &bridge.types[0]);
        let mut pos = 0;
        while pos < neighbors.len() {
            let m = neighbors[pos].vertex;
            let mut multiplicity = 0u128;
            while pos < neighbors.len() && neighbors[pos].vertex == m {
                read_budget::charge_candidate_work(1, "scanning tag bridge edges")?;
                if masks[m as usize] & 2 != 0
                    && props_match(
                        &graph.edges[neighbors[pos].edge as usize].props,
                        bridge.properties.as_ref(),
                        params,
                    )?
                {
                    multiplicity += 1;
                }
                pos += 1;
            }
            read_budget::charge_candidate_work(1, "grouping tag bridge endpoints")?;
            let (left, right) = (degrees[m as usize][0], degrees[c][1]);
            if multiplicity == 0 || left == 0 || right == 0 {
                continue;
            }
            let excluded = exclusion(index, &tags, &masks, m, c as u32, params)?;
            let term = contribution(
                u128::from(left),
                u128::from(right),
                excluded,
                multiplicity,
                tags.anti,
            )?;
            count = count.checked_add(term).ok_or_else(overflow)?;
        }
    }
    scalar_table(count, tags.projection).map(Some)
}

fn scalar_table(count: u128, p: &Projection) -> Result<CypherResultTable> {
    // Three directed physical edges give at most E^3 < 2^96 matches under
    // the index's u32 edge bound. Never cap before either exact subtraction.
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
