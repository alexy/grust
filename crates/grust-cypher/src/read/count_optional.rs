//! Independent OPTIONAL leaves multiply mandatory assignment weights by
//! max(1, matching physical edges). Each clause pads once, after all its own
//! anchor, relationship and leaf predicates. Optional bindings are never used
//! later, and globally disjoint types prove relationship independence.

use super::*;

pub(super) struct Leaf<'a> {
    anchor: &'a NodePattern,
    leaf: &'a NodePattern,
    relationship: &'a RelationshipPattern,
    direction: ast::Direction,
}

/// The caller precharges planner storage for every pattern/projection occurrence.
/// Mandatory variables stay in the forest even when a bag-preserving WITH drops
/// them: projecting a binding away must not discard its incoming multiplicity.
pub(super) fn plan_suffix<'a>(
    mut clauses: &'a [Clause],
    variables: &HashMap<&'a str, usize>,
    types: &mut HashSet<&'a str>,
    slots: usize,
) -> Result<Option<Vec<Vec<Leaf<'a>>>>> {
    let mut retained = None;
    if let Some(Clause::With(w)) = clauses.first() {
        let p = &w.projection;
        if w.where_clause.is_some()
            || p.star
            || p.distinct
            || p.items.is_empty()
            || !p.order_by.is_empty()
            || p.skip.is_some()
            || p.limit.is_some()
        {
            return Ok(None);
        }
        let mut names = HashSet::new();
        for item in &p.items {
            read_budget::charge_candidate_work(1, "planning count WITH bindings")?;
            let Expr::Variable(name) = &item.expr else {
                return Ok(None);
            };
            if item.alias.is_some()
                || !variables.contains_key(name.as_str())
                || !names.insert(name.as_str())
            {
                return Ok(None);
            }
        }
        retained = Some(names);
        clauses = &clauses[1..];
        if clauses.is_empty() {
            return Ok(None);
        }
    }
    let mut leaves: Vec<Vec<Leaf<'a>>> = (0..slots).map(|_| Vec::new()).collect();
    if clauses.is_empty() {
        return Ok(Some(leaves));
    }
    // Keep every earlier name, including those dropped through WITH. Rebinding
    // one as an optional leaf, or using an earlier nullable leaf, is not proven.
    let mut used: HashSet<&str> = variables.keys().copied().collect();
    for clause in clauses {
        read_budget::charge_candidate_work(1, "planning optional count leaf")?;
        let Clause::Match(m) = clause else {
            return Ok(None);
        };
        if !m.optional || m.where_clause.is_some() || m.patterns.len() != 1 {
            return Ok(None);
        }
        let path = &m.patterns[0];
        if path.variable.is_some() || path.shortest.is_some() || path.segments.len() != 1 {
            return Ok(None);
        }
        let segment = &path.segments[0];
        let rel = &segment.relationship;
        if rel.variable.is_some()
            || rel.length.is_some()
            || rel.types.len() != 1
            || !types.insert(rel.types[0].as_str())
            || !literal_properties(rel.properties.as_ref())
            || !literal_properties(path.start.properties.as_ref())
            || !literal_properties(segment.node.properties.as_ref())
        {
            return Ok(None);
        }
        let anchor_slot = |node: &NodePattern| {
            let name = node.variable.as_deref()?;
            if retained.as_ref().is_some_and(|names| !names.contains(name)) {
                return None;
            }
            variables.get(name).copied()
        };
        let (slot, anchor, leaf, direction) =
            match (anchor_slot(&path.start), anchor_slot(&segment.node)) {
                (Some(slot), None) => (slot, &path.start, &segment.node, rel.direction),
                (None, Some(slot)) => {
                    let direction = match rel.direction {
                        ast::Direction::Outgoing => ast::Direction::Incoming,
                        ast::Direction::Incoming => ast::Direction::Outgoing,
                        ast::Direction::Undirected => ast::Direction::Undirected,
                    };
                    (slot, &segment.node, &path.start, direction)
                }
                _ => return Ok(None),
            };
        if leaf
            .variable
            .as_deref()
            .is_some_and(|name| !used.insert(name))
        {
            return Ok(None);
        }
        leaves[slot].push(Leaf {
            anchor,
            leaf,
            relationship: rel,
            direction,
        });
    }
    Ok(Some(leaves))
}

/// No row products or per-leaf vertex arrays: each borrowed typed adjacency is
/// visited once per eligible anchor. Sparse typed lookup adds log(active slots).
/// Work is charged even for zero-degree anchors; shared predicate evaluation
/// accounts literal copies. Budget failures propagate, never retry unbounded.
pub(super) fn apply(
    index: &TypedGraphIndex,
    leaves: &[Leaf<'_>],
    weights: &mut [u64],
    params: &CypherParameters,
) -> Result<()> {
    let graph = index.graph();
    for leaf in leaves {
        for (vertex, weight) in weights.iter_mut().enumerate() {
            read_budget::charge_candidate_work(1, "combining optional count leaves")?;
            if *weight == 0 || !node_matches(&graph.nodes[vertex], leaf.anchor, params)? {
                // A failed OPTIONAL anchor predicate pads; it must not filter
                // out the mandatory assignment or contribute a zero factor.
                continue;
            }
            let mut degree = 0;
            for reverse in [false, true] {
                if (reverse && leaf.direction == ast::Direction::Outgoing)
                    || (!reverse && leaf.direction == ast::Direction::Incoming)
                {
                    continue;
                }
                let rel = leaf.relationship;
                let neighbors = if reverse {
                    index.incoming(vertex as u32, &rel.types[0])
                } else {
                    index.outgoing(vertex as u32, &rel.types[0])
                };
                for neighbor in neighbors {
                    read_budget::charge_candidate_work(1, "scanning optional count edges")?;
                    if reverse
                        && leaf.direction == ast::Direction::Undirected
                        && neighbor.vertex as usize == vertex
                    {
                        continue;
                    }
                    if props_match(
                        &graph.edges[neighbor.edge as usize].props,
                        rel.properties.as_ref(),
                        params,
                    )? && node_matches(
                        &graph.nodes[neighbor.vertex as usize],
                        leaf.leaf,
                        params,
                    )? {
                        degree = add(degree, 1);
                    }
                }
            }
            *weight = multiply(*weight, degree.max(1));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "count_optional_tests.rs"]
mod tests;
