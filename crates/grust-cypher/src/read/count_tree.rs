//! Exact nonnegative COUNT algebra for proven fixed-length pattern forests,
//! optionally followed by independent, null-padded single-edge leaves.
//! No query-name shortcuts, mutation indexes, or match-row materialization.

use super::count_predicate::{node_matches, props_match};
use super::*;
use grust_core::TypedGraphIndex;
use std::collections::HashSet;

mod mandatory_adjacency;
#[path = "count_optional.rs"]
mod optional;

// Capping preserves every representable result, including zero annihilation
// after an enormous subtree. Only a nonrepresentable final result is an error.
const CAP: u64 = i64::MAX as u64 + 1;

fn add(a: u64, b: u64) -> u64 {
    (u128::from(a) + u128::from(b)).min(u128::from(CAP)) as u64
}

fn multiply(a: u64, b: u64) -> u64 {
    (u128::from(a) * u128::from(b)).min(u128::from(CAP)) as u64
}

struct Atom<'a> {
    from: usize,
    to: usize,
    relationship: &'a RelationshipPattern,
}

struct Forest<'a> {
    nodes: Vec<Vec<&'a NodePattern>>,
    atoms: Vec<Atom<'a>>,
    adjacency: Vec<Vec<(usize, usize)>>,
    optionals: Vec<Vec<optional::Leaf<'a>>>,
    projection: &'a Projection,
}

/// Eligibility and the traversal used by execution are established together.
/// A caller cannot observe a factorized plan before acyclicity is proved.
struct ProvenForest<'a> {
    forest: Forest<'a>,
    parent: Vec<Option<(usize, usize)>>,
    order: Vec<usize>,
    roots: Vec<usize>,
}

fn literal_properties(properties: Option<&MapLiteral>) -> bool {
    properties.is_none_or(|map| {
        map.entries.iter().all(|(_, expr)| {
            matches!(
                expr,
                Expr::Null | Expr::Boolean(_) | Expr::Integer(_) | Expr::Float(_) | Expr::String(_)
            )
        })
    })
}

fn scalar_bound(expr: Option<&Expr>) -> Option<u64> {
    match expr {
        None => Some(0),
        Some(Expr::Integer(value)) if *value >= 0 => Some(*value as u64),
        _ => None,
    }
}

fn plan(query: &Query) -> Result<Option<ProvenForest<'_>>> {
    if query.parts.len() != 1 || query.parts[0].union.is_some() {
        return Ok(None);
    }
    let clauses = &query.parts[0].query.clauses;
    let Some(Clause::Return(ret)) = clauses.last() else {
        return Ok(None);
    };
    let p = &ret.projection;
    if p.star
        || p.distinct
        || p.items.len() != 1
        || !p.order_by.is_empty()
        || scalar_bound(p.skip.as_ref()).is_none()
        || scalar_bound(p.limit.as_ref()).is_none()
    {
        return Ok(None);
    }
    if !matches!(&p.items[0].expr, Expr::Function { name, distinct: false, star: true, args }
        if name.eq_ignore_ascii_case("count") && args.is_empty())
    {
        return Ok(None);
    }
    // Charge before allocating planner maps/vectors. Patterns are borrowed;
    // strings, property maps and AST expressions are not copied.
    let mut occurrences = 0usize;
    let body = &clauses[..clauses.len() - 1];
    for clause in body {
        read_budget::checkpoint()?;
        match clause {
            Clause::Match(m) if m.where_clause.is_none() => {
                for path in &m.patterns {
                    occurrences = occurrences.saturating_add(path.segments.len().saturating_add(1));
                }
            }
            Clause::With(w) if w.where_clause.is_none() => {
                occurrences = occurrences.saturating_add(w.projection.items.len());
            }
            _ => return Ok(None),
        }
    }
    read_budget::charge_intermediate_bytes(
        occurrences.saturating_mul(512),
        "planning count forest",
    )?;
    let mut forest = Forest {
        nodes: Vec::new(),
        atoms: Vec::new(),
        adjacency: Vec::new(),
        optionals: Vec::new(),
        projection: p,
    };
    let mut variables: HashMap<&str, usize> = HashMap::new();
    let mut types = HashSet::new();
    fn node<'a>(
        p: &'a NodePattern,
        variables: &mut HashMap<&'a str, usize>,
        nodes: &mut Vec<Vec<&'a NodePattern>>,
    ) -> usize {
        if let Some(name) = p.variable.as_deref() {
            if let Some(&slot) = variables.get(name) {
                nodes[slot].push(p);
                return slot;
            }
            variables.insert(name, nodes.len());
        }
        nodes.push(vec![p]);
        nodes.len() - 1
    }
    let mandatory_len = body
        .iter()
        .take_while(|clause| matches!(clause, Clause::Match(m) if !m.optional))
        .count();
    for clause in &body[..mandatory_len] {
        let Clause::Match(m) = clause else {
            unreachable!()
        };
        for path in &m.patterns {
            read_budget::charge_candidate_work(1, "planning count pattern")?;
            if path.variable.is_some()
                || path.shortest.is_some()
                || !literal_properties(path.start.properties.as_ref())
            {
                return Ok(None);
            }
            let mut from = node(&path.start, &mut variables, &mut forest.nodes);
            for segment in &path.segments {
                read_budget::charge_candidate_work(1, "planning count relationship")?;
                let rel = &segment.relationship;
                if rel.variable.is_some()
                    || rel.length.is_some()
                    || rel.types.len() != 1
                    || !types.insert(rel.types[0].as_str())
                    || !literal_properties(rel.properties.as_ref())
                    || !literal_properties(segment.node.properties.as_ref())
                {
                    return Ok(None);
                }
                let to = node(&segment.node, &mut variables, &mut forest.nodes);
                forest.atoms.push(Atom {
                    from,
                    to,
                    relationship: rel,
                });
                from = to;
            }
        }
    }
    let Some(optionals) = optional::plan_suffix(
        &body[mandatory_len..],
        &variables,
        &mut types,
        forest.nodes.len(),
    )?
    else {
        return Ok(None);
    };
    forest.optionals = optionals;
    forest.adjacency = vec![Vec::new(); forest.nodes.len()];
    for (edge, atom) in forest.atoms.iter().enumerate() {
        forest.adjacency[atom.from].push((atom.to, edge));
        forest.adjacency[atom.to].push((atom.from, edge));
    }
    prove_forest(forest)
}

fn prove_forest(forest: Forest<'_>) -> Result<Option<ProvenForest<'_>>> {
    let mut parent = vec![None; forest.nodes.len()];
    let mut seen = vec![false; forest.nodes.len()];
    let mut order = Vec::new();
    let mut roots = Vec::new();
    // Iterative traversal proves acyclicity, including duplicate links and
    // same-slot loops. Repeated variable references for star centers are valid.
    for root in 0..forest.nodes.len() {
        if seen[root] {
            continue;
        }
        roots.push(root);
        seen[root] = true;
        let start = order.len();
        order.push(root);
        let mut cursor = start;
        while cursor < order.len() {
            read_budget::checkpoint()?;
            let slot = order[cursor];
            cursor += 1;
            for &(next, edge) in &forest.adjacency[slot] {
                if parent[slot] == Some((next, edge)) {
                    continue;
                }
                if seen[next] {
                    return Ok(None);
                }
                seen[next] = true;
                parent[next] = Some((slot, edge));
                order.push(next);
            }
        }
    }
    Ok(Some(ProvenForest {
        forest,
        parent,
        order,
        roots,
    }))
}

pub(super) fn supports(query: &Query) -> Result<bool> {
    Ok(plan(query)?.is_some())
}

pub(super) fn try_execute(
    index: &TypedGraphIndex,
    query: &Query,
    params: &CypherParameters,
) -> Result<Option<CypherResultTable>> {
    let Some(ProvenForest {
        forest,
        parent,
        order,
        roots,
    }) = plan(query)?
    else {
        return Ok(None);
    };
    let graph = index.graph();
    let mut weights: Vec<Option<Vec<u64>>> = (0..forest.nodes.len()).map(|_| None).collect();
    for &slot in order.iter().rev() {
        read_budget::charge_intermediate_bytes(
            graph.nodes.len().saturating_mul(8),
            "allocating count weights",
        )?;
        read_budget::charge_candidate_work(graph.nodes.len(), "initializing count weights")?;
        let mut values = vec![0; graph.nodes.len()];
        let label = forest.nodes[slot].iter().find_map(|p| p.labels.first());
        let candidates = if let Some(label) = label {
            read_budget::charge_candidate_work(
                label.len().saturating_mul(2).saturating_add(1),
                "looking up count forest candidate labels",
            )?;
            Some(index.vertices_with_label(label))
        } else {
            None
        };
        let candidate_count = candidates.map_or(graph.nodes.len(), |v| v.len());
        let prefilter = if candidate_count == 0 {
            None
        } else {
            mandatory_adjacency::prepare(index, &forest, slot)?
        };
        let candidates = match &prefilter {
            Some(prefilter) => prefilter.narrow_candidates(candidates, graph.nodes.len())?,
            None => candidates,
        };
        let candidate_count = candidates.map_or(graph.nodes.len(), |v| v.len());
        for candidate in 0..candidate_count {
            read_budget::charge_candidate_work(1, "filtering count vertices")?;
            let vertex = candidates.map_or(candidate, |v| v[candidate] as usize);
            if let Some(prefilter) = &prefilter
                && !prefilter.accepts(vertex as u32)?
            {
                continue;
            }
            let mut matches = true;
            for pattern in &forest.nodes[slot] {
                if !node_matches(&graph.nodes[vertex], pattern, params)? {
                    matches = false;
                    break;
                }
            }
            values[vertex] = u64::from(matches);
        }
        optional::apply(index, &forest.optionals[slot], &mut values, params)?;
        for &(child, edge) in &forest.adjacency[slot] {
            if parent[child] != Some((slot, edge)) {
                continue;
            }
            let child_values = weights[child].take().expect("postorder child weights");
            let atom = &forest.atoms[edge];
            let rel = atom.relationship;
            let direction = if slot == atom.from {
                rel.direction
            } else {
                match rel.direction {
                    ast::Direction::Outgoing => ast::Direction::Incoming,
                    ast::Direction::Incoming => ast::Direction::Outgoing,
                    ast::Direction::Undirected => ast::Direction::Undirected,
                }
            };
            // Only seed candidates can have nonzero weights. Optional
            // padding and earlier branch products never revive a zero, so
            // reuse the borrowed slice without another lookup or allocation.
            for candidate in 0..candidates.map_or(values.len(), |v| v.len()) {
                read_budget::charge_candidate_work(1, "combining count branches")?;
                let vertex = candidates.map_or(candidate, |v| v[candidate] as usize);
                let weight = &mut values[vertex];
                if *weight == 0 {
                    continue;
                }
                let mut sum = 0;
                for reverse in [false, true] {
                    if (reverse && direction == ast::Direction::Outgoing)
                        || (!reverse && direction == ast::Direction::Incoming)
                    {
                        continue;
                    }
                    let neighbors = if reverse {
                        index.incoming(vertex as u32, &rel.types[0])
                    } else {
                        index.outgoing(vertex as u32, &rel.types[0])
                    };
                    // Every physical slot is consumed on success, including
                    // skipped incoming loops. Borrowed chunks keep exact total
                    // work; a tight budget may refuse a whole chunk before its
                    // partially affordable prefix. Predicate charges stay local.
                    for chunk in neighbors.chunks(256) {
                        read_budget::charge_candidate_work(
                            chunk.len(),
                            "scanning typed count edges",
                        )?;
                        for neighbor in chunk {
                            if reverse
                                && direction == ast::Direction::Undirected
                                && neighbor.vertex as usize == vertex
                            {
                                continue;
                            }
                            if props_match(
                                &graph.edges[neighbor.edge as usize].props,
                                rel.properties.as_ref(),
                                params,
                            )? {
                                sum = add(sum, child_values[neighbor.vertex as usize]);
                            }
                        }
                    }
                }
                *weight = multiply(*weight, sum);
            }
        }
        weights[slot] = Some(values);
    }
    let mut count = 1;
    for root in roots {
        let values = weights[root].take().expect("root weights");
        read_budget::charge_candidate_work(values.len(), "summing count roots")?;
        count = multiply(count, values.into_iter().fold(0, add));
    }
    let count = i64::try_from(count).map_err(|_| gql_execution("count result exceeds int64"))?;
    let p = forest.projection;
    let suppressed =
        scalar_bound(p.skip.as_ref()).unwrap() > 0 || p.limit == Some(Expr::Integer(0));
    read_budget::charge_intermediate_bytes(
        128usize.saturating_add(p.items[0].alias.as_ref().map_or(4, String::len)),
        "shaping scalar count result",
    )?;
    Ok(Some(CypherResultTable {
        columns: vec![p.items[0].alias.clone().unwrap_or_else(|| "expr".into())],
        rows: if suppressed {
            Vec::new()
        } else {
            vec![vec![Value::Int(count)]]
        },
    }))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "count_tree/label_budget_tests.rs"]
mod label_budget_tests;

#[cfg(test)]
#[path = "count_tree/branch_candidate_tests.rs"]
mod branch_candidate_tests;

#[cfg(test)]
#[path = "count_tree/edge_scan_budget_tests.rs"]
mod edge_scan_budget_tests;
