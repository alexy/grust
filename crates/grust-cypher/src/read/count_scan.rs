//! Row-free scalar scans, zero-hop identity paths and bounded range cardinality.
//! Every arm has one guaranteed-nonnull binding source and one scalar COUNT.

use super::count_predicate::{node_matches, props_match};
use super::*;
use grust_core::TypedGraphIndex;

enum Source<'a> {
    Nodes(&'a NodePattern, Option<&'a NodePattern>, Filter<'a>),
    Edges(&'a PathPattern),
    Range { start: i64, end: i64, step: i64 },
}

enum Filter<'a> {
    Always(bool),
    PropertyNull { key: &'a str, negated: bool },
}

struct Arm<'a> {
    source: Source<'a>,
    column: &'a str,
    suppressed: bool,
}

fn literal_map(map: Option<&MapLiteral>) -> Result<bool> {
    let Some(map) = map else { return Ok(true) };
    read_budget::charge_candidate_work(map.entries.len(), "proving scalar scan properties")?;
    Ok(map.entries.iter().all(|(_, value)| {
        matches!(
            value,
            Expr::Null | Expr::Boolean(_) | Expr::Integer(_) | Expr::Float(_) | Expr::String(_)
        )
    }))
}

fn bound(expr: Option<&Expr>) -> Option<u64> {
    match expr {
        None => Some(0),
        Some(Expr::Integer(value)) if *value >= 0 => Some(*value as u64),
        _ => None,
    }
}

fn integer(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Integer(value) => Some(*value),
        Expr::Unary { op, operand } => match (op, operand.as_ref()) {
            (UnaryOp::Negate, Expr::Integer(value)) => value.checked_neg(),
            (UnaryOp::Plus, Expr::Integer(value)) => Some(*value),
            _ => None,
        },
        _ => None,
    }
}

fn count_binding<'a>(projection: &'a Projection, names: &[Option<&str>]) -> Option<&'a str> {
    let [item] = projection.items.as_slice() else {
        return None;
    };
    if projection.star
        || projection.distinct
        || !projection.order_by.is_empty()
        || bound(projection.skip.as_ref()).is_none()
        || bound(projection.limit.as_ref()).is_none()
    {
        return None;
    }
    let Expr::Function {
        name,
        distinct: false,
        star,
        args,
    } = &item.expr
    else {
        return None;
    };
    if !name.eq_ignore_ascii_case("count") {
        return None;
    }
    let counted = (*star && args.is_empty())
        || matches!(args.as_slice(),
        [Expr::Variable(name)] if !star && names.contains(&Some(name.as_str())));
    counted.then_some(item.alias.as_deref().unwrap_or("expr"))
}

fn filter<'a>(expr: Option<&'a Expr>, names: &[Option<&str>]) -> Result<Option<Filter<'a>>> {
    Ok(Some(match expr {
        None | Some(Expr::Boolean(true)) => Filter::Always(true),
        Some(Expr::Null | Expr::Boolean(false)) => Filter::Always(false),
        Some(Expr::Binary {
            op: BinaryOp::Eq | BinaryOp::Ne,
            lhs,
            rhs,
        }) => {
            let (Expr::String(left), Expr::String(right)) = (lhs.as_ref(), rhs.as_ref()) else {
                return Ok(None);
            };
            read_budget::charge_candidate_work(
                left.len().saturating_add(right.len()),
                "comparing constant scan strings",
            )?;
            let negated = matches!(
                expr,
                Some(Expr::Binary {
                    op: BinaryOp::Ne,
                    ..
                })
            );
            Filter::Always((left == right) != negated)
        }
        Some(Expr::IsNull { operand, negated }) => {
            let Expr::Property { base, key } = operand.as_ref() else {
                return Ok(None);
            };
            let Expr::Variable(name) = base.as_ref() else {
                return Ok(None);
            };
            if !names.contains(&Some(name.as_str())) {
                return Ok(None);
            }
            // Node .label is structural and never NULL; .id remains a property,
            // exactly as in the reference evaluator (id(node) is different).
            if key == "label" {
                Filter::Always(*negated)
            } else {
                Filter::PropertyNull {
                    key,
                    negated: *negated,
                }
            }
        }
        _ => return Ok(None),
    }))
}

fn plan_arm(query: &SingleQuery) -> Result<Option<Arm<'_>>> {
    let [source, Clause::Return(ret)] = query.clauses.as_slice() else {
        return Ok(None);
    };
    read_budget::charge_candidate_work(1, "proving scalar scan source")?;
    let (source, column) = match source {
        Clause::Match(m) if !m.optional => {
            let [path] = m.patterns.as_slice() else {
                return Ok(None);
            };
            if path.variable.is_some()
                || path.shortest.is_some()
                || !literal_map(path.start.properties.as_ref())?
            {
                return Ok(None);
            }
            let first = &path.start;
            match path.segments.as_slice() {
                [] => {
                    let names = [first.variable.as_deref()];
                    let Some(column) = count_binding(&ret.projection, &names) else {
                        return Ok(None);
                    };
                    let Some(filter) = filter(m.where_clause.as_ref(), &names)? else {
                        return Ok(None);
                    };
                    (Source::Nodes(first, None, filter), column)
                }
                [segment] => {
                    let rel = &segment.relationship;
                    let last = &segment.node;
                    if !literal_map(last.properties.as_ref())?
                        || !literal_map(rel.properties.as_ref())?
                    {
                        return Ok(None);
                    }
                    let names = [
                        first.variable.as_deref(),
                        last.variable.as_deref(),
                        rel.variable.as_deref(),
                    ];
                    let Some(column) = count_binding(&ret.projection, &names) else {
                        return Ok(None);
                    };
                    match rel.length {
                        Some(RangeLiteral {
                            min: Some(0),
                            max: Some(0),
                        }) if rel.variable.is_none() && rel.properties.is_none() => {
                            let Some(filter) = filter(m.where_clause.as_ref(), &names[..2])? else {
                                return Ok(None);
                            };
                            (Source::Nodes(first, Some(last), filter), column)
                        }
                        None if m.where_clause.is_none() => (Source::Edges(path), column),
                        _ => return Ok(None),
                    }
                }
                _ => return Ok(None),
            }
        }
        Clause::Unwind(u) => {
            let Expr::Function {
                name,
                distinct: false,
                star: false,
                args,
            } = &u.expr
            else {
                return Ok(None);
            };
            if !name.eq_ignore_ascii_case("range") {
                return Ok(None);
            }
            let (start, end, step) = match args.as_slice() {
                [start, end] => (integer(start), integer(end), Some(1)),
                [start, end, step] => (integer(start), integer(end), integer(step)),
                _ => return Ok(None),
            };
            let (Some(start), Some(end), Some(step)) = (start, end, step) else {
                return Ok(None);
            };
            let Some(column) = count_binding(&ret.projection, &[Some(&u.alias)]) else {
                return Ok(None);
            };
            (Source::Range { start, end, step }, column)
        }
        _ => return Ok(None),
    };
    Ok(Some(Arm {
        source,
        column,
        suppressed: bound(ret.projection.skip.as_ref()).unwrap() > 0
            || ret.projection.limit == Some(Expr::Integer(0)),
    }))
}

fn plan(query: &Query) -> Result<Option<Vec<Arm<'_>>>> {
    if query.parts.is_empty() {
        return Ok(None);
    }
    // Reject complex clause/path shapes before allocating arm storage. The
    // complete proof below still checks bindings, filters and projections.
    for part in &query.parts {
        read_budget::charge_candidate_work(1, "checking scalar scan shape")?;
        match part.query.clauses.as_slice() {
            [Clause::Match(m), Clause::Return(_)]
                if !m.optional && m.patterns.len() == 1 && m.patterns[0].segments.len() <= 1 => {}
            [Clause::Unwind(_), Clause::Return(_)] => {}
            _ => return Ok(None),
        }
    }
    read_budget::charge_intermediate_bytes(
        query
            .parts
            .len()
            .saturating_mul(std::mem::size_of::<Arm<'_>>()),
        "planning scalar scan arms",
    )?;
    let mut arms: Vec<Arm<'_>> = Vec::with_capacity(query.parts.len());
    for part in &query.parts {
        let Some(arm) = plan_arm(&part.query)? else {
            return Ok(None);
        };
        if arms.first().is_some_and(|first| first.column != arm.column) {
            return Ok(None);
        }
        arms.push(arm);
    }
    Ok(Some(arms))
}

pub(super) fn supports(query: &Query) -> Result<bool> {
    Ok(plan(query)?.is_some())
}

fn unfiltered(node: &NodePattern) -> bool {
    node.properties
        .as_ref()
        .is_none_or(|map| map.entries.is_empty())
}

fn count_nodes(
    index: &TypedGraphIndex,
    first: &NodePattern,
    last: Option<&NodePattern>,
    filter: &Filter<'_>,
    params: &CypherParameters,
) -> Result<u64> {
    read_budget::charge_candidate_work(1, "counting indexed scan candidates")?;
    if matches!(filter, Filter::Always(false)) {
        return Ok(0);
    }
    let labels = first
        .labels
        .iter()
        .chain(last.into_iter().flat_map(|node| &node.labels));
    let mut required: Option<&str> = None;
    for label in labels {
        read_budget::charge_candidate_work(1, "checking scalar scan labels")?;
        if let Some(required) = required {
            if required.len() != label.len() {
                return Ok(0);
            }
            read_budget::charge_candidate_work(label.len(), "comparing scalar scan labels")?;
            if required != label.as_str() {
                return Ok(0);
            }
        }
        required = Some(label.as_str());
    }
    let candidates = if let Some(label) = required {
        // Borrowed keys still incur hashing and possible equality on a miss.
        read_budget::charge_candidate_work(
            label.len().saturating_mul(2).saturating_add(1),
            "looking up scalar scan labels",
        )?;
        Some(index.vertices_with_label(label))
    } else {
        None
    };
    let count = candidates.map_or(index.graph().nodes.len(), <[u32]>::len);
    if matches!(filter, Filter::Always(true)) && unfiltered(first) && last.is_none_or(unfiltered) {
        return Ok(count as u64);
    }
    let mut matched = 0;
    for offset in 0..count {
        read_budget::charge_candidate_work(1, "scanning indexed count vertices")?;
        let slot = candidates.map_or(offset, |slots| slots[offset] as usize);
        let node = &index.graph().nodes[slot];
        // Label consistency was proved above, and candidates use that label.
        if !props_match(&node.props, first.properties.as_ref(), params)? {
            continue;
        }
        // The second pattern can have different properties even with identical labels.
        if let Some(last) = last
            && !props_match(&node.props, last.properties.as_ref(), params)?
        {
            continue;
        }
        let keep = match filter {
            Filter::Always(value) => *value,
            Filter::PropertyNull { key, negated } => {
                let comparisons = node.props.len().checked_ilog2().unwrap_or(0) as usize + 1;
                read_budget::charge_candidate_work(
                    key.len().saturating_add(1).saturating_mul(comparisons),
                    "checking count property nullness",
                )?;
                // JSON null stored inside Value::Json is not Value::Null.
                node.props
                    .get(*key)
                    .is_none_or(|value| matches!(value, Value::Null))
                    != *negated
            }
        };
        matched += u64::from(keep);
    }
    Ok(matched)
}

fn bare(node: &NodePattern) -> bool {
    node.labels.is_empty() && unfiltered(node)
}

fn count_edges(
    index: &TypedGraphIndex,
    path: &PathPattern,
    params: &CypherParameters,
) -> Result<u64> {
    let segment = &path.segments[0];
    let rel = &segment.relationship;
    let same_node = path.start.variable.is_some() && path.start.variable == segment.node.variable;
    read_budget::charge_candidate_work(1, "counting indexed scan edges")?;
    if rel.direction != ast::Direction::Undirected
        && !same_node
        && rel.types.is_empty()
        && rel.properties.is_none()
        && bare(&path.start)
        && bare(&segment.node)
    {
        return Ok(index.graph().edges.len() as u64);
    }
    let mut count = 0;
    for edge in &index.graph().edges {
        read_budget::charge_candidate_work(1, "scanning indexed count edges")?;
        if !rel.types.is_empty() && !rel.types.iter().any(|kind| kind == edge.label.as_str())
            || !props_match(&edge.props, rel.properties.as_ref(), params)?
        {
            continue;
        }
        let from = index
            .vertex_index(edge.from.as_str())
            .expect("index validates endpoints");
        let to = index
            .vertex_index(edge.to.as_str())
            .expect("index validates endpoints");
        if same_node && from != to {
            continue;
        }
        for reverse in [false, true] {
            if (reverse && rel.direction == ast::Direction::Outgoing)
                || (!reverse && rel.direction == ast::Direction::Incoming)
                || (reverse && rel.direction == ast::Direction::Undirected && from == to)
            {
                continue;
            }
            read_budget::charge_candidate_work(1, "checking count edge orientation")?;
            let (left, right) = if reverse { (to, from) } else { (from, to) };
            if node_matches(&index.graph().nodes[left as usize], &path.start, params)?
                && node_matches(&index.graph().nodes[right as usize], &segment.node, params)?
            {
                count += 1
            }
        }
    }
    Ok(count)
}

fn count_range(start: i64, end: i64, step: i64) -> Result<u64> {
    read_budget::charge_candidate_work(1, "counting range cardinality")?;
    if step == 0 {
        return Err(gql_execution("range() step must not be zero"));
    }
    if (step > 0 && start > end) || (step < 0 && start < end) {
        return Ok(0);
    }
    let distance = (i128::from(end) - i128::from(start)).abs();
    let count = distance / i128::from(step).abs() + 1;
    let count = usize::try_from(count).unwrap_or(usize::MAX);
    // Keep both the universal ceiling and the caller's range/work limits even
    // though this specialized aggregate need not allocate the range values.
    read_budget::check_range_items(count)?;
    Ok(count as u64)
}

pub(super) fn try_execute(
    index: &TypedGraphIndex,
    query: &Query,
    params: &CypherParameters,
) -> Result<Option<CypherResultTable>> {
    let Some(arms) = plan(query)? else {
        return Ok(None);
    };
    let column = arms[0].column;
    let distinct = query
        .parts
        .iter()
        .any(|part| part.union == Some(UnionKind::Distinct));
    let mut seen = std::collections::HashSet::new();
    let mut rows = Vec::new();
    for arm in arms {
        let count = match arm.source {
            Source::Nodes(first, last, filter) => count_nodes(index, first, last, &filter, params)?,
            Source::Edges(path) => count_edges(index, path, params)?,
            Source::Range { start, end, step } => count_range(start, end, step)?,
        };
        let count =
            i64::try_from(count).map_err(|_| gql_execution("count result exceeds int64"))?;
        // Compute even a suppressed scalar: LIMIT 0 cannot hide range errors.
        if arm.suppressed {
            continue;
        }
        read_budget::charge_candidate_work(1, "shaping scalar count arm")?;
        read_budget::charge_intermediate_bytes(128, "shaping scalar count arm")?;
        if !distinct || seen.insert(count) {
            rows.push(vec![Value::Int(count)])
        }
    }
    read_budget::charge_intermediate_bytes(
        column.len().saturating_add(64),
        "shaping scalar count columns",
    )?;
    Ok(Some(CypherResultTable {
        columns: vec![column.into()],
        rows,
    }))
}

#[cfg(test)]
#[path = "count_scan_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "count_scan_budget_tests.rs"]
mod budget_tests;
