use super::*;

pub(super) struct Cycle<'a> {
    pub nodes: Vec<Vec<&'a NodePattern>>,
    pub roles: [usize; 4], // c, p, u, v
    pub reply: &'a RelationshipPattern,
    pub creators: [&'a RelationshipPattern; 2],
    pub knows: &'a RelationshipPattern,
    pub projection: &'a Projection,
}

struct Atom<'a> {
    from: usize,
    to: usize,
    rel: &'a RelationshipPattern,
    type_slot: usize,
    undirected: bool,
}

fn strings_equal(left: &str, right: &str, context: &'static str) -> Result<bool> {
    if left.len() != right.len() {
        return Ok(false);
    }
    read_budget::charge_candidate_work(left.len().saturating_add(1), context)?;
    Ok(left == right)
}

pub(super) fn scalar_bound(expr: Option<&Expr>) -> Option<u64> {
    match expr {
        None => Some(0),
        Some(Expr::Integer(value)) if *value >= 0 => Some(*value as u64),
        _ => None,
    }
}

fn literals(properties: Option<&MapLiteral>) -> Result<bool> {
    if let Some(map) = properties {
        for (_, expr) in &map.entries {
            read_budget::charge_candidate_work(1, "proving cycle literal predicates")?;
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

fn node<'a>(
    pattern: &'a NodePattern,
    names: &mut Vec<&'a str>,
    nodes: &mut Vec<Vec<&'a NodePattern>>,
) -> Result<Option<usize>> {
    read_budget::charge_candidate_work(1, "proving cycle node mentions")?;
    let Some(name) = pattern.variable.as_deref() else {
        return Ok(None);
    };
    if !literals(pattern.properties.as_ref())? {
        return Ok(None);
    }
    let mut slot = None;
    for (index, &existing) in names.iter().enumerate() {
        if strings_equal(existing, name, "comparing cycle variable names")? {
            slot = Some(index);
            break;
        }
    }
    let slot = if let Some(slot) = slot {
        slot
    } else {
        if names.len() == 4 {
            return Ok(None);
        }
        names.push(name);
        nodes.push(Vec::new());
        names.len() - 1
    };
    nodes[slot].push(pattern);
    Ok(Some(slot))
}

/// A string cannot satisfy two distinct string equalities on the same key.
/// Retain every conjunct, including duplicate map keys and separate mentions.
/// JSON serialization inequality is NOT a safe numeric disjointness proof:
/// reference comparisons coerce int/float/decimal through f64 in mixed cases.
fn disjoint(left: &[&NodePattern], right: &[&NodePattern]) -> Result<bool> {
    for a in left
        .iter()
        .filter_map(|node| node.properties.as_ref())
        .flat_map(|map| &map.entries)
    {
        for b in right
            .iter()
            .filter_map(|node| node.properties.as_ref())
            .flat_map(|map| &map.entries)
        {
            read_budget::charge_candidate_work(1, "proving cycle source disjointness")?;
            if strings_equal(&a.0, &b.0, "comparing cycle property keys")?
                && let (Expr::String(x), Expr::String(y)) = (&a.1, &b.1)
                && !strings_equal(x, y, "comparing cycle disjointness literals")?
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(super) fn plan(query: &Query) -> Result<Option<Cycle<'_>>> {
    read_budget::checkpoint()?;
    if query.parts.len() != 1 || query.parts[0].union.is_some() {
        return Ok(None);
    }
    let [Clause::Match(m), Clause::Return(ret)] = &query.parts[0].query.clauses[..] else {
        return Ok(None);
    };
    if m.optional || m.where_clause.is_some() || m.patterns.is_empty() || m.patterns.len() > 4 {
        return Ok(None);
    }
    let mut edges = 0usize;
    for path in &m.patterns {
        if path.variable.is_some()
            || path.shortest.is_some()
            || path.segments.is_empty()
            || path.segments.len() > 4
        {
            return Ok(None);
        }
        edges += path.segments.len();
    }
    if edges != 4 {
        return Ok(None);
    }
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
    read_budget::charge_intermediate_bytes(
        (edges + m.patterns.len()) * 512,
        "planning count cycle",
    )?;
    let (mut names, mut nodes) = (Vec::new(), Vec::new());
    let mut atoms: Vec<Atom<'_>> = Vec::new();
    for path in &m.patterns {
        let Some(mut from) = node(&path.start, &mut names, &mut nodes)? else {
            return Ok(None);
        };
        for segment in &path.segments {
            read_budget::charge_candidate_work(1, "proving cycle relationship atoms")?;
            let rel = &segment.relationship;
            if rel.variable.is_some()
                || rel.length.is_some()
                || rel.types.len() != 1
                || !literals(rel.properties.as_ref())?
            {
                return Ok(None);
            }
            let Some(to) = node(&segment.node, &mut names, &mut nodes)? else {
                return Ok(None);
            };
            let (source, target) = if rel.direction == ast::Direction::Incoming {
                (to, from)
            } else {
                (from, to)
            };
            // Intern at most four borrowed types once. All later topology
            // comparisons use integer slots, not repeated uncharged strings.
            let mut type_slot = atoms.len();
            for atom in &atoms {
                if strings_equal(
                    &atom.rel.types[0],
                    &rel.types[0],
                    "comparing cycle relationship types",
                )? {
                    type_slot = atom.type_slot;
                    break;
                }
            }
            atoms.push(Atom {
                from: source,
                to: target,
                rel,
                type_slot,
                undirected: rel.direction == ast::Direction::Undirected,
            });
            from = to;
        }
    }
    if nodes.len() != 4 || atoms.iter().filter(|a| a.undirected).count() != 1 {
        return Ok(None);
    }
    let knows = atoms.iter().find(|a| a.undirected).unwrap();
    let Some(reply) = atoms.iter().find(|a| {
        !a.undirected && atoms.iter().filter(|b| b.type_slot == a.type_slot).count() == 1
    }) else {
        return Ok(None);
    };
    let creator = |source| {
        atoms
            .iter()
            .find(|a| !a.undirected && a.from == source && a.type_slot != reply.type_slot)
    };
    let (Some(left), Some(right)) = (creator(reply.from), creator(reply.to)) else {
        return Ok(None);
    };
    let roles = [reply.from, reply.to, left.to, right.to];
    if roles
        .iter()
        .enumerate()
        .any(|(i, slot)| roles[..i].contains(slot))
        || left.type_slot != right.type_slot
        || left.type_slot == knows.type_slot
        || !((knows.from == left.to && knows.to == right.to)
            || (knows.to == left.to && knows.from == right.to))
        || !disjoint(&nodes[reply.from], &nodes[reply.to])?
    {
        return Ok(None);
    }
    Ok(Some(Cycle {
        nodes,
        roles,
        reply: reply.rel,
        creators: [left.rel, right.rel],
        knows: knows.rel,
        projection: p,
    }))
}
