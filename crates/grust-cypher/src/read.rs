//! Memory reference executor for bounded read-only `MATCH … RETURN`
//! (Unit 6 of `docs/GQL_GOAL.md`).
//!
//! This is the first executable use of the new lexer → parser → semantics
//! pipeline. It runs a bounded read-query subset against an in-memory
//! [`Graph`] snapshot (the deterministic reference; persistent backends will
//! push the same plans down later). It does **not** touch the existing write
//! planner — reads are a new, additive capability.
//!
//! Supported subset:
//! - one or more `MATCH` clauses (node patterns and fixed-length relationship
//!   segments with direction, types, and inline property-equality maps),
//! - an optional `WHERE` per `MATCH` (comparison, boolean with three-valued
//!   NULL logic, `IN`, `IS [NOT] NULL`, `STARTS/ENDS WITH`, `CONTAINS`,
//!   arithmetic),
//! - a final `RETURN` with item aliases, `*`, `DISTINCT`, `ORDER BY`, `SKIP`,
//!   and `LIMIT`.
//!
//! Everything else (OPTIONAL MATCH, WITH, UNION, UNWIND, variable-length and
//! quantified paths, path variables, aggregates, functions, CASE) fails with a
//! feature-tagged [`unsupported_gql_feature`] error rather than a silent wrong
//! answer. Aggregation and the general expression/function registry arrive in
//! Units 7–9.

use std::collections::{BTreeMap, HashMap};

use crate::ast;
use crate::ast::*;
use crate::parser::parse_query;
use crate::*;

/// A value bound to a variable in a candidate row. Pattern matching binds
/// `Node`/`Edge`; `WITH`/`UNWIND` projections bind computed `Value`s.
#[derive(Clone, Debug)]
enum Bound {
    Node(Node),
    Edge(Edge),
    Value(Value),
}

/// One candidate solution: pattern variable -> bound graph element.
type Row = BTreeMap<String, Bound>;

/// Parse, analyze, and execute a read-only query against an in-memory graph.
///
/// Backends that can materialize a [`Graph`] (e.g. `MemoryGraphStore::graph()`)
/// use this directly as the portable reference; it returns the same
/// [`CypherResultTable`] shape as the writable returning path.
pub fn run_read_query(
    graph: &Graph,
    cypher: &str,
    params: &CypherParameters,
) -> Result<CypherResultTable> {
    let query = parse_query(cypher).map_err(|e| e.into_grust(cypher))?;
    // Reuse the shared semantic analyzer for binding/kind checks.
    crate::semantics::analyze(&query)?;
    execute_read_query(graph, &query, params)
}

/// Execute an already-parsed read-only query against an in-memory graph.
pub fn execute_read_query(
    graph: &Graph,
    query: &Query,
    params: &CypherParameters,
) -> Result<CypherResultTable> {
    let mut combined: Option<CypherResultTable> = None;
    // `UNION` (without ALL) deduplicates the whole result; `UNION ALL` keeps
    // duplicates. A mixed chain dedups if any boundary is a distinct UNION.
    let mut distinct = false;

    for part in &query.parts {
        if part.union == Some(UnionKind::Distinct) {
            distinct = true;
        }
        let table = execute_single(graph, &part.query, params)?;
        match combined.as_mut() {
            None => combined = Some(table),
            Some(acc) => {
                if acc.columns != table.columns {
                    return Err(gql_name(
                        "all UNION arms must return the same column names in the same order",
                    ));
                }
                acc.rows.extend(table.rows);
            }
        }
    }

    let mut table = combined.ok_or_else(|| gql_execution("query has no parts"))?;
    if distinct {
        let mut seen = std::collections::HashSet::new();
        let mut deduped = Vec::with_capacity(table.rows.len());
        for values in std::mem::take(&mut table.rows) {
            let key = return_row_key(&values, "UNION")?;
            if seen.insert(key) {
                deduped.push(values);
            }
        }
        table.rows = deduped;
    }
    Ok(table)
}

fn execute_single(
    graph: &Graph,
    query: &SingleQuery,
    params: &CypherParameters,
) -> Result<CypherResultTable> {
    let index = NodeIndex::build(graph);
    let mut rows: Vec<Row> = vec![Row::new()];

    // A clause pipeline: each clause transforms the binding-row stream; RETURN is
    // terminal and produces the result table.
    for clause in &query.clauses {
        match clause {
            Clause::Match(m) if !m.optional => {
                for pattern in &m.patterns {
                    rows = expand_pattern(graph, &index, pattern, rows, params)?;
                }
                if let Some(where_expr) = &m.where_clause {
                    rows = filter_rows(rows, where_expr, params)?;
                }
            }
            Clause::Match(m) => {
                // OPTIONAL MATCH: each incoming row produces its matches, or a
                // single row with this match's new variables NULL-padded.
                let new_vars = pattern_variables(&m.patterns);
                let mut out = Vec::new();
                for row in rows {
                    let mut matched = vec![row.clone()];
                    for pattern in &m.patterns {
                        matched = expand_pattern(graph, &index, pattern, matched, params)?;
                    }
                    if let Some(where_expr) = &m.where_clause {
                        matched = filter_rows(matched, where_expr, params)?;
                    }
                    if matched.is_empty() {
                        let mut padded = row;
                        for var in &new_vars {
                            padded.entry(var.clone()).or_insert(Bound::Value(Value::Null));
                        }
                        out.push(padded);
                    } else {
                        out.extend(matched);
                    }
                }
                rows = out;
            }
            Clause::With(w) => {
                rows = project_to_bindings(&w.projection, rows, params)?;
                if let Some(where_expr) = &w.where_clause {
                    rows = filter_rows(rows, where_expr, params)?;
                }
            }
            Clause::Unwind(u) => {
                rows = unwind_rows(rows, u, params)?;
            }
            Clause::Return(r) => {
                // Terminal: project the current bindings to the result table.
                return project(graph, &rows, &r.projection, params);
            }
            Clause::Create(_)
            | Clause::Merge(_)
            | Clause::Delete(_)
            | Clause::Set(_)
            | Clause::Remove(_) => {
                return Err(gql_execution(
                    "the read reference executor only runs read-only MATCH/WITH/UNWIND/RETURN queries",
                ))
            }
        }
    }

    Err(gql_execution("read query has no RETURN clause"))
}

/// Keep rows whose `where_expr` evaluates to TRUE (NULL/FALSE drop), surfacing
/// evaluation errors instead of silently dropping rows.
fn filter_rows(rows: Vec<Row>, where_expr: &Expr, params: &CypherParameters) -> Result<Vec<Row>> {
    let mut kept = Vec::with_capacity(rows.len());
    for row in rows {
        if matches!(eval(where_expr, &row, params)?, Value::Bool(true)) {
            kept.push(row);
        }
    }
    Ok(kept)
}

/// `UNWIND list AS x`: expand each row into one row per list element. A NULL or
/// empty list yields no rows for that input row.
fn unwind_rows(rows: Vec<Row>, unwind: &UnwindClause, params: &CypherParameters) -> Result<Vec<Row>> {
    let mut out = Vec::new();
    for row in rows {
        // A list literal is evaluated element-wise (round-tripping a list
        // through JSON would be lossy); other expressions evaluate to a value.
        let elements: Vec<Value> = if let Expr::List(items) = &unwind.expr {
            items
                .iter()
                .map(|e| eval(e, &row, params))
                .collect::<Result<_>>()?
        } else {
            match eval(&unwind.expr, &row, params)? {
                Value::Null => continue,
                Value::StringArray(xs) => xs.into_iter().map(Value::String).collect(),
                Value::IntArray(xs) => xs.into_iter().map(Value::Int).collect(),
                Value::FloatArray(xs) => xs.into_iter().map(Value::Float).collect(),
                Value::Json(serde_json::Value::Array(arr)) => {
                    arr.iter().map(json_to_value).collect()
                }
                other => return Err(gql_type(format!("UNWIND expects a list, got {other:?}"))),
            }
        };
        for element in elements {
            let mut next = row.clone();
            next.insert(unwind.alias.clone(), Bound::Value(element));
            out.push(next);
        }
    }
    Ok(out)
}

/// `WITH` horizon: project the binding-row stream into a new binding-row stream
/// (bare variables keep their node/edge/value binding; computed items bind a
/// value), then apply DISTINCT / ORDER BY / SKIP / LIMIT.
fn project_to_bindings(
    projection: &Projection,
    rows: Vec<Row>,
    params: &CypherParameters,
) -> Result<Vec<Row>> {
    let has_aggregate = projection.items.iter().any(|i| expr_has_aggregate(&i.expr));

    let mut out: Vec<Row> = if has_aggregate {
        if projection.star {
            return Err(unsupported_gql_feature(
                GqlFeature::AggregateFunctionRegistry,
                GqlConformanceProfile::PortableGql,
                "WITH * combined with aggregates is not supported by the read reference executor yet",
            ));
        }
        grouped_bindings(&projection.items, &rows, params)?
    } else {
        let mut produced = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut next = if projection.star { row.clone() } else { Row::new() };
            for item in &projection.items {
                let (name, bound) = binding_for_item(item, row, params)?;
                next.insert(name, bound);
            }
            produced.push(next);
        }
        produced
    };

    if projection.distinct {
        out = dedup_bindings(out, &projection.items, projection.star, params)?;
    }
    order_bindings(&mut out, &projection.order_by, params)?;
    skip_limit_bindings(&mut out, projection, params)?;
    Ok(out)
}

/// The (name, binding) a `WITH`/`RETURN` item contributes. A bare variable keeps
/// its existing binding (so a node stays a node downstream); anything else binds
/// a computed value under its alias.
fn binding_for_item(item: &ReturnItem, row: &Row, params: &CypherParameters) -> Result<(String, Bound)> {
    match &item.expr {
        Expr::Variable(v) => {
            let bound = row
                .get(v)
                .cloned()
                .unwrap_or(Bound::Value(Value::Null));
            Ok((item.alias.clone().unwrap_or_else(|| v.clone()), bound))
        }
        other => {
            let name = item.alias.clone().ok_or_else(|| {
                gql_name("WITH requires an alias (AS ...) for non-variable expressions")
            })?;
            Ok((name, Bound::Value(eval(other, row, params)?)))
        }
    }
}

fn grouped_bindings(
    items: &[ReturnItem],
    rows: &[Row],
    params: &CypherParameters,
) -> Result<Vec<Row>> {
    // Group by the non-aggregate items; carry bare-variable keys as their
    // original binding, computed keys/aggregates as values.
    let key_items: Vec<&ReturnItem> = items
        .iter()
        .filter(|i| !expr_has_aggregate(&i.expr))
        .collect();

    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, (Row, Vec<usize>)> = HashMap::new();
    for (idx, row) in rows.iter().enumerate() {
        let key_values: Vec<Value> = key_items
            .iter()
            .map(|i| eval(&i.expr, row, params))
            .collect::<Result<_>>()?;
        let key = return_row_key(&key_values, "WITH GROUP BY")?;
        groups
            .entry(key.clone())
            .or_insert_with(|| {
                order.push(key);
                (row.clone(), Vec::new())
            })
            .1
            .push(idx);
    }
    if key_items.is_empty() && rows.is_empty() {
        order.push(String::new());
        groups.insert(String::new(), (Row::new(), Vec::new()));
    }

    let mut out = Vec::with_capacity(order.len());
    for key in &order {
        let (representative, group_idxs) = &groups[key];
        let group_rows: Vec<&Row> = group_idxs.iter().map(|&i| &rows[i]).collect();
        let mut next = Row::new();
        for item in items {
            if expr_has_aggregate(&item.expr) {
                let name = item.alias.clone().ok_or_else(|| {
                    gql_name("WITH requires an alias (AS ...) for aggregate expressions")
                })?;
                next.insert(name, Bound::Value(eval_aggregate(&item.expr, &group_rows, params)?));
            } else {
                let (name, bound) = binding_for_item(item, representative, params)?;
                next.insert(name, bound);
            }
        }
        out.push(next);
    }
    Ok(out)
}

fn dedup_bindings(
    rows: Vec<Row>,
    items: &[ReturnItem],
    star: bool,
    params: &CypherParameters,
) -> Result<Vec<Row>> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut values: Vec<Value> = Vec::new();
        if star {
            for (_, bound) in &row {
                values.push(bound_value(bound)?);
            }
        }
        for item in items {
            values.push(eval(&item.expr, &row, params)?);
        }
        if seen.insert(return_row_key(&values, "WITH DISTINCT")?) {
            out.push(row);
        }
    }
    Ok(out)
}

fn order_bindings(rows: &mut [Row], order_by: &[OrderItem], params: &CypherParameters) -> Result<()> {
    if order_by.is_empty() {
        return Ok(());
    }
    let mut keyed: Vec<Vec<Value>> = Vec::with_capacity(rows.len());
    for row in rows.iter() {
        let mut keys = Vec::with_capacity(order_by.len());
        for item in order_by {
            keys.push(eval(&item.expr, row, params)?);
        }
        keyed.push(keys);
    }
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| {
        for (k, item) in order_by.iter().enumerate() {
            let ord = compare_return_values(&keyed[a][k], &keyed[b][k]);
            let ord = if item.descending { ord.reverse() } else { ord };
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });
    let reordered: Vec<Row> = order.iter().map(|&i| rows[i].clone()).collect();
    rows.clone_from_slice(&reordered);
    Ok(())
}

fn skip_limit_bindings(
    rows: &mut Vec<Row>,
    projection: &Projection,
    params: &CypherParameters,
) -> Result<()> {
    if let Some(skip) = &projection.skip {
        let n = eval_usize(skip, params, "SKIP")?;
        rows.drain(0..n.min(rows.len()));
    }
    if let Some(limit) = &projection.limit {
        let n = eval_usize(limit, params, "LIMIT")?;
        rows.truncate(n);
    }
    Ok(())
}

fn bound_value(bound: &Bound) -> Result<Value> {
    match bound {
        Bound::Node(node) => graph_node_value(node),
        Bound::Edge(edge) => graph_edge_value(edge),
        Bound::Value(value) => Ok(value.clone()),
    }
}

// ---------------------------------------------------------------------------
// Pattern matching
// ---------------------------------------------------------------------------

struct NodeIndex {
    by_id: HashMap<String, usize>,
}

impl NodeIndex {
    fn build(graph: &Graph) -> Self {
        let mut by_id = HashMap::new();
        for (i, node) in graph.nodes.iter().enumerate() {
            by_id.insert(node.id.as_str().to_string(), i);
        }
        NodeIndex { by_id }
    }

    fn get<'g>(&self, graph: &'g Graph, id: &str) -> Option<&'g Node> {
        self.by_id.get(id).map(|&i| &graph.nodes[i])
    }
}

/// All variables introduced by a set of path patterns (path, node, and
/// relationship variables), in first-seen order and de-duplicated.
fn pattern_variables(patterns: &[PathPattern]) -> Vec<String> {
    let mut vars: Vec<String> = Vec::new();
    let add = |opt: &Option<String>, vars: &mut Vec<String>| {
        if let Some(name) = opt {
            if !vars.iter().any(|v| v == name) {
                vars.push(name.clone());
            }
        }
    };
    for pattern in patterns {
        add(&pattern.variable, &mut vars);
        add(&pattern.start.variable, &mut vars);
        for segment in &pattern.segments {
            add(&segment.relationship.variable, &mut vars);
            add(&segment.node.variable, &mut vars);
        }
    }
    vars
}

fn expand_pattern(
    graph: &Graph,
    index: &NodeIndex,
    pattern: &PathPattern,
    base_rows: Vec<Row>,
    params: &CypherParameters,
) -> Result<Vec<Row>> {
    if pattern.variable.is_some() {
        return Err(unsupported_gql_feature(
            GqlFeature::PathVariableBinding,
            GqlConformanceProfile::PortableGql,
            "path variables are not supported by the read reference executor yet",
        ));
    }
    let mut out = Vec::new();
    for row in base_rows {
        for start in node_candidates(graph, &pattern.start, &row, params)? {
            let mut next_row = row.clone();
            if let Some(var) = &pattern.start.variable {
                next_row.insert(var.clone(), Bound::Node(start.clone()));
            }
            expand_segments(graph, index, &pattern.segments, 0, &start, next_row, params, &mut out)?;
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn expand_segments(
    graph: &Graph,
    index: &NodeIndex,
    segments: &[PathSegment],
    idx: usize,
    current: &Node,
    row: Row,
    params: &CypherParameters,
    out: &mut Vec<Row>,
) -> Result<()> {
    if idx == segments.len() {
        out.push(row);
        return Ok(());
    }
    let segment = &segments[idx];
    let rel = &segment.relationship;
    if rel.length.is_some() {
        return Err(unsupported_gql_feature(
            GqlFeature::QuantifiedPathPattern,
            GqlConformanceProfile::PortableGql,
            "variable-length relationships are not supported by the read reference executor yet",
        ));
    }

    for edge in &graph.edges {
        let Some(next_id) = edge_other_endpoint(edge, current, rel.direction) else {
            continue;
        };
        if !rel.types.is_empty() && !rel.types.iter().any(|t| t == edge.label.as_str()) {
            continue;
        }
        if !props_match(&edge.props, rel.properties.as_ref(), params)? {
            continue;
        }
        let Some(next_node) = index.get(graph, next_id) else {
            continue;
        };
        if !node_matches(next_node, &segment.node, params)? {
            continue;
        }
        // consistency with an already-bound next-node variable
        if let Some(var) = &segment.node.variable {
            if let Some(Bound::Node(bound)) = row.get(var) {
                if bound.id != next_node.id {
                    continue;
                }
            }
        }
        let mut next_row = row.clone();
        if let Some(var) = &rel.variable {
            next_row.insert(var.clone(), Bound::Edge(edge.clone()));
        }
        if let Some(var) = &segment.node.variable {
            next_row.insert(var.clone(), Bound::Node(next_node.clone()));
        }
        expand_segments(graph, index, segments, idx + 1, next_node, next_row, params, out)?;
    }
    Ok(())
}

/// The id of the endpoint reached from `current` along `edge` given `direction`,
/// or `None` if `edge` does not connect to `current` in that direction.
fn edge_other_endpoint<'e>(
    edge: &'e Edge,
    current: &Node,
    direction: ast::Direction,
) -> Option<&'e str> {
    let from = edge.from.as_str();
    let to = edge.to.as_str();
    let cur = current.id.as_str();
    match direction {
        ast::Direction::Outgoing => (from == cur).then_some(to),
        ast::Direction::Incoming => (to == cur).then_some(from),
        ast::Direction::Undirected => {
            if from == cur {
                Some(to)
            } else if to == cur {
                Some(from)
            } else {
                None
            }
        }
    }
}

fn node_candidates(
    graph: &Graph,
    np: &NodePattern,
    row: &Row,
    params: &CypherParameters,
) -> Result<Vec<Node>> {
    // Already bound? Filter to the bound node if it still matches.
    if let Some(var) = &np.variable {
        if let Some(Bound::Node(bound)) = row.get(var) {
            return Ok(if node_matches(bound, np, params)? {
                vec![bound.clone()]
            } else {
                vec![]
            });
        }
    }
    let mut out = Vec::new();
    for node in &graph.nodes {
        if node_matches(node, np, params)? {
            out.push(node.clone());
        }
    }
    Ok(out)
}

fn node_matches(node: &Node, np: &NodePattern, params: &CypherParameters) -> Result<bool> {
    if np.labels.len() > 1 {
        return Err(unsupported_gql_feature(
            GqlFeature::LabelTypePredicateMatch,
            GqlConformanceProfile::PortableGql,
            "multi-label node patterns are not supported by the read reference executor yet",
        ));
    }
    if let Some(label) = np.labels.first() {
        if node.label.as_str() != label {
            return Ok(false);
        }
    }
    props_match(&node.props, np.properties.as_ref(), params)
}

/// True when every entry in the inline pattern map equals the element's property.
fn props_match(
    props: &Props,
    map: Option<&MapLiteral>,
    params: &CypherParameters,
) -> Result<bool> {
    let Some(map) = map else {
        return Ok(true);
    };
    for (key, expr) in &map.entries {
        let expected = eval_constant(expr, params)?;
        match props.get(key) {
            Some(actual) if values_equal(actual, &expected) == Some(true) => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// Projection
// ---------------------------------------------------------------------------

fn project(
    graph: &Graph,
    rows: &[Row],
    projection: &Projection,
    params: &CypherParameters,
) -> Result<CypherResultTable> {
    let _ = graph;
    // Resolve which items to project (star expands to bound variables, sorted).
    let mut columns: Vec<String> = Vec::new();
    let mut exprs: Vec<Expr> = Vec::new();

    if projection.star {
        let mut vars: BTreeMap<String, ()> = BTreeMap::new();
        for row in rows {
            for var in row.keys() {
                vars.insert(var.clone(), ());
            }
        }
        for var in vars.keys() {
            columns.push(var.clone());
            exprs.push(Expr::Variable(var.clone()));
        }
    }
    for item in &projection.items {
        columns.push(item.alias.clone().unwrap_or_else(|| column_name(&item.expr)));
        exprs.push(item.expr.clone());
    }
    if exprs.is_empty() {
        return Err(gql_syntax("RETURN requires at least one item"));
    }

    // alias -> expr, so ORDER BY can reference output aliases.
    let aliases: HashMap<String, Expr> = projection
        .items
        .iter()
        .filter_map(|i| i.alias.clone().map(|a| (a, i.expr.clone())))
        .collect();

    let has_aggregate = exprs.iter().any(expr_has_aggregate);

    let mut out_rows: Vec<Vec<Value>> = if has_aggregate {
        if projection.star {
            return Err(unsupported_gql_feature(
                GqlFeature::AggregateFunctionRegistry,
                GqlConformanceProfile::PortableGql,
                "RETURN * combined with aggregates is not supported by the read reference executor yet",
            ));
        }
        grouped_project(&exprs, rows, params)?
    } else {
        let mut rs = Vec::with_capacity(rows.len());
        for row in rows {
            let mut values = Vec::with_capacity(exprs.len());
            for expr in &exprs {
                values.push(eval(expr, row, params)?);
            }
            rs.push(values);
        }
        rs
    };

    if projection.distinct {
        let mut seen = std::collections::HashSet::new();
        let mut deduped = Vec::new();
        for values in out_rows {
            let key = return_row_key(&values, "RETURN DISTINCT")?;
            if seen.insert(key) {
                deduped.push(values);
            }
        }
        out_rows = deduped;
    }

    if has_aggregate {
        // Grouped rows no longer align with the source bindings, so ORDER BY
        // resolves against the output columns instead.
        order_by_columns(&mut out_rows, &columns, &projection.order_by)?;
    } else {
        apply_order_by(&mut out_rows, &projection.order_by, rows, &aliases, params)?;
    }
    apply_skip_limit(&mut out_rows, projection, params)?;

    Ok(CypherResultTable {
        columns,
        rows: out_rows,
    })
}

/// Aggregate function names recognized by the read reference executor.
fn is_aggregate_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "count" | "sum" | "avg" | "min" | "max" | "collect"
    )
}

/// True if `expr` contains an aggregate function call anywhere.
fn expr_has_aggregate(expr: &Expr) -> bool {
    match expr {
        Expr::Function { name, args, .. } => {
            is_aggregate_name(name) || args.iter().any(expr_has_aggregate)
        }
        Expr::Property { base, .. } => expr_has_aggregate(base),
        Expr::Index { base, index } => expr_has_aggregate(base) || expr_has_aggregate(index),
        Expr::List(items) => items.iter().any(expr_has_aggregate),
        Expr::Map(entries) => entries.iter().any(|(_, e)| expr_has_aggregate(e)),
        Expr::Unary { operand, .. } => expr_has_aggregate(operand),
        Expr::Binary { lhs, rhs, .. } => expr_has_aggregate(lhs) || expr_has_aggregate(rhs),
        Expr::IsNull { operand, .. } => expr_has_aggregate(operand),
        Expr::Case {
            operand,
            branches,
            default,
        } => {
            operand.as_deref().is_some_and(expr_has_aggregate)
                || branches
                    .iter()
                    .any(|b| expr_has_aggregate(&b.when) || expr_has_aggregate(&b.then))
                || default.as_deref().is_some_and(expr_has_aggregate)
        }
        _ => false,
    }
}

/// Group rows by the non-aggregate (grouping-key) projection items and fold the
/// aggregate items per group. Each item must be either aggregate-free (a key) or
/// a single top-level aggregate call; nested aggregates are not supported.
fn grouped_project(
    exprs: &[Expr],
    rows: &[Row],
    params: &CypherParameters,
) -> Result<Vec<Vec<Value>>> {
    enum Kind<'a> {
        Key(&'a Expr),
        Aggregate(&'a Expr),
    }
    let mut kinds = Vec::with_capacity(exprs.len());
    for expr in exprs {
        match expr {
            Expr::Function { name, .. } if is_aggregate_name(name) => {
                kinds.push(Kind::Aggregate(expr))
            }
            _ if expr_has_aggregate(expr) => {
                return Err(unsupported_gql_feature(
                    GqlFeature::AggregateFunctionRegistry,
                    GqlConformanceProfile::PortableGql,
                    "aggregates nested inside larger expressions are not supported by the read reference executor yet",
                ))
            }
            _ => kinds.push(Kind::Key(expr)),
        }
    }

    let key_exprs: Vec<&Expr> = kinds
        .iter()
        .filter_map(|k| match k {
            Kind::Key(e) => Some(*e),
            Kind::Aggregate(_) => None,
        })
        .collect();

    // Group rows, preserving first-seen order for deterministic output.
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, (Vec<Value>, Vec<&Row>)> = HashMap::new();
    for row in rows {
        let key_values: Vec<Value> = key_exprs
            .iter()
            .map(|e| eval(e, row, params))
            .collect::<Result<_>>()?;
        let key = return_row_key(&key_values, "GROUP BY")?;
        groups
            .entry(key.clone())
            .or_insert_with(|| {
                order.push(key);
                (key_values, Vec::new())
            })
            .1
            .push(row);
    }

    // A pure aggregate with no grouping keys yields exactly one row even over an
    // empty match (e.g. `RETURN count(*)` -> 0).
    if key_exprs.is_empty() && rows.is_empty() {
        order.push(String::new());
        groups.insert(String::new(), (Vec::new(), Vec::new()));
    }

    let mut out_rows = Vec::with_capacity(order.len());
    for key in &order {
        let (key_values, group_rows) = &groups[key];
        let mut key_iter = key_values.iter();
        let mut values = Vec::with_capacity(kinds.len());
        for kind in &kinds {
            match kind {
                Kind::Key(_) => values.push(
                    key_iter
                        .next()
                        .cloned()
                        .unwrap_or(Value::Null),
                ),
                Kind::Aggregate(expr) => {
                    values.push(eval_aggregate(expr, group_rows, params)?)
                }
            }
        }
        out_rows.push(values);
    }
    Ok(out_rows)
}

fn eval_aggregate(expr: &Expr, group_rows: &[&Row], params: &CypherParameters) -> Result<Value> {
    let Expr::Function {
        name,
        distinct,
        star,
        args,
    } = expr
    else {
        unreachable!("eval_aggregate called on a non-function expression");
    };
    let name = name.to_ascii_lowercase();

    if name == "count" && *star {
        return count_value(group_rows.len());
    }
    if args.len() != 1 {
        return Err(gql_type(format!(
            "aggregate {name}() expects exactly one argument"
        )));
    }
    let arg = &args[0];

    // Evaluate the argument per row, dropping NULLs (standard aggregate rule).
    let mut values: Vec<Value> = Vec::with_capacity(group_rows.len());
    for row in group_rows {
        if let Some(v) = non_null_return_value(eval(arg, row, params)?) {
            values.push(v);
        }
    }
    if *distinct {
        values = distinct_return_values(values)?;
    }

    match name.as_str() {
        "count" => count_value(values.len()),
        "sum" => sum_return_values(&values),
        "avg" => avg_return_values(&values),
        "collect" => {
            let json: Vec<serde_json::Value> = values.iter().map(value_to_json).collect();
            Ok(Value::Json(serde_json::Value::Array(json)))
        }
        "min" | "max" => {
            let want_max = name == "max";
            let mut best: Option<Value> = None;
            for v in values {
                best = Some(match best {
                    None => v,
                    Some(current) => {
                        let ord = compare_return_values(&v, &current);
                        let pick_v = if want_max {
                            ord.is_gt()
                        } else {
                            ord.is_lt()
                        };
                        if pick_v {
                            v
                        } else {
                            current
                        }
                    }
                });
            }
            Ok(best.unwrap_or(Value::Null))
        }
        _ => unreachable!("is_aggregate_name gates the set"),
    }
}

/// ORDER BY for the aggregate path: keys must reference output columns by name.
fn order_by_columns(
    out_rows: &mut [Vec<Value>],
    columns: &[String],
    order_by: &[OrderItem],
) -> Result<()> {
    if order_by.is_empty() {
        return Ok(());
    }
    let mut keys = Vec::with_capacity(order_by.len());
    for item in order_by {
        let name = match &item.expr {
            Expr::Variable(name) => name.clone(),
            Expr::Property { base, key } => format!("{}.{}", column_name(base), key),
            _ => {
                return Err(unsupported_gql_feature(
                    GqlFeature::AggregateFunctionRegistry,
                    GqlConformanceProfile::PortableGql,
                    "ORDER BY with aggregates must reference an output column by name",
                ))
            }
        };
        let idx = columns.iter().position(|c| c == &name).ok_or_else(|| {
            gql_name(format!("ORDER BY column `{name}` is not in the RETURN list"))
        })?;
        keys.push((idx, item.descending));
    }
    out_rows.sort_by(|a, b| {
        for (idx, descending) in &keys {
            let ord = compare_return_values(&a[*idx], &b[*idx]);
            let ord = if *descending { ord.reverse() } else { ord };
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });
    Ok(())
}

fn apply_order_by(
    out_rows: &mut [Vec<Value>],
    order_by: &[OrderItem],
    rows: &[Row],
    aliases: &HashMap<String, Expr>,
    params: &CypherParameters,
) -> Result<()> {
    if order_by.is_empty() {
        return Ok(());
    }
    // Compute sort keys against the source bindings (resolving aliases).
    let mut keyed: Vec<(usize, Vec<Value>)> = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let mut keys = Vec::with_capacity(order_by.len());
        for item in order_by {
            let expr = match &item.expr {
                Expr::Variable(name) if aliases.contains_key(name) => &aliases[name],
                other => other,
            };
            keys.push(eval(expr, row, params)?);
        }
        keyed.push((i, keys));
    }
    // Sort indices, then reorder out_rows to match. rows and out_rows align 1:1
    // only when there is no DISTINCT; guard that.
    if keyed.len() != out_rows.len() {
        return Err(gql_execution(
            "ORDER BY with DISTINCT is not supported by the read reference executor yet",
        ));
    }
    let mut order: Vec<usize> = (0..out_rows.len()).collect();
    order.sort_by(|&a, &b| {
        for (k, item) in order_by.iter().enumerate() {
            let ord = compare_return_values(&keyed[a].1[k], &keyed[b].1[k]);
            let ord = if item.descending { ord.reverse() } else { ord };
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });
    let reordered: Vec<Vec<Value>> = order.iter().map(|&i| out_rows[i].clone()).collect();
    out_rows.clone_from_slice(&reordered);
    Ok(())
}

fn apply_skip_limit(
    out_rows: &mut Vec<Vec<Value>>,
    projection: &Projection,
    params: &CypherParameters,
) -> Result<()> {
    if let Some(skip) = &projection.skip {
        let n = eval_usize(skip, params, "SKIP")?;
        out_rows.drain(0..n.min(out_rows.len()));
    }
    if let Some(limit) = &projection.limit {
        let n = eval_usize(limit, params, "LIMIT")?;
        out_rows.truncate(n);
    }
    Ok(())
}

fn column_name(expr: &Expr) -> String {
    match expr {
        Expr::Variable(name) => name.clone(),
        Expr::Property { base, key } => format!("{}.{}", column_name(base), key),
        Expr::Parameter(name) => format!("${name}"),
        _ => "expr".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Expression evaluation (bounded; general engine is Unit 7)
// ---------------------------------------------------------------------------

/// Evaluate an expression that must be a constant (no bound variables) — used
/// for inline pattern-map values.
fn eval_constant(expr: &Expr, params: &CypherParameters) -> Result<Value> {
    eval(expr, &Row::new(), params)
}

fn eval_usize(expr: &Expr, params: &CypherParameters, what: &str) -> Result<usize> {
    match eval(expr, &Row::new(), params)? {
        Value::Int(n) if n >= 0 => Ok(n as usize),
        other => Err(gql_type(format!("{what} expects a non-negative integer, got {other:?}"))),
    }
}

fn eval(expr: &Expr, row: &Row, params: &CypherParameters) -> Result<Value> {
    match expr {
        Expr::Null => Ok(Value::Null),
        Expr::Boolean(b) => Ok(Value::Bool(*b)),
        Expr::Integer(n) => Ok(Value::Int(*n)),
        Expr::Float(f) => Ok(Value::Float(*f)),
        Expr::String(s) => Ok(Value::String(s.clone())),
        Expr::Parameter(name) => params
            .get(name)
            .cloned()
            .ok_or_else(|| gql_name(format!("parameter ${name} was not provided"))),
        Expr::Variable(name) => match row.get(name) {
            Some(Bound::Node(node)) => graph_node_value(node),
            Some(Bound::Edge(edge)) => graph_edge_value(edge),
            Some(Bound::Value(value)) => Ok(value.clone()),
            None => Err(gql_name(format!("variable `{name}` is not bound"))),
        },
        Expr::Property { base, key } => eval_property(base, key, row, params),
        Expr::List(items) => {
            let mut out = Vec::new();
            for item in items {
                out.push(value_to_json(&eval(item, row, params)?));
            }
            Ok(Value::Json(serde_json::Value::Array(out)))
        }
        Expr::Unary { op, operand } => eval_unary(*op, operand, row, params),
        Expr::Binary { op, lhs, rhs } => eval_binary(*op, lhs, rhs, row, params),
        Expr::IsNull { operand, negated } => {
            let is_null = matches!(eval(operand, row, params)?, Value::Null);
            Ok(Value::Bool(is_null != *negated))
        }
        Expr::Function {
            name,
            distinct,
            star,
            args,
        } => {
            if is_aggregate_name(name) {
                return Err(gql_execution(format!(
                    "aggregate {name}() is only allowed as a top-level RETURN item"
                )));
            }
            if *star || *distinct {
                return Err(unsupported_gql_feature(
                    GqlFeature::ScalarFunctionRegistry,
                    GqlConformanceProfile::PortableGql,
                    format!("`{name}(DISTINCT ...)`/`{name}(*)` is not a scalar function form"),
                ));
            }
            eval_scalar_function(name, args, row, params)
        }
        Expr::Case {
            operand,
            branches,
            default,
        } => eval_case(operand.as_deref(), branches, default.as_deref(), row, params),
        Expr::Map(_) | Expr::Index { .. } => Err(unsupported_gql_feature(
            GqlFeature::GeneralExpressionTree,
            GqlConformanceProfile::PortableGql,
            "map literals and indexing are not supported by the read reference executor yet (Unit 7)",
        )),
    }
}

/// Evaluate a `CASE` expression. Simple form (`CASE x WHEN v THEN ...`) compares
/// `x` to each branch value; searched form (`CASE WHEN cond THEN ...`) takes the
/// first branch whose condition is TRUE. Falls back to `ELSE` / NULL.
fn eval_case(
    operand: Option<&Expr>,
    branches: &[CaseBranch],
    default: Option<&Expr>,
    row: &Row,
    params: &CypherParameters,
) -> Result<Value> {
    match operand {
        Some(op) => {
            let target = eval(op, row, params)?;
            for branch in branches {
                let candidate = eval(&branch.when, row, params)?;
                if values_equal(&target, &candidate) == Some(true) {
                    return eval(&branch.then, row, params);
                }
            }
        }
        None => {
            for branch in branches {
                if matches!(eval(&branch.when, row, params)?, Value::Bool(true)) {
                    return eval(&branch.then, row, params);
                }
            }
        }
    }
    match default {
        Some(d) => eval(d, row, params),
        None => Ok(Value::Null),
    }
}

/// Scalar function registry for the read reference executor. Reuses the crate's
/// existing `restricted_*_value` evaluators rather than reimplementing them.
fn eval_scalar_function(
    name: &str,
    args: &[Expr],
    row: &Row,
    params: &CypherParameters,
) -> Result<Value> {
    let lower = name.to_ascii_lowercase();

    // `coalesce` is variadic and null-aware; handle it before the unary helpers.
    if lower == "coalesce" {
        if args.is_empty() {
            return Err(gql_type("coalesce() expects at least one argument".to_string()));
        }
        for arg in args {
            let value = eval(arg, row, params)?;
            if !matches!(value, Value::Null) {
                return Ok(value);
            }
        }
        return Ok(Value::Null);
    }

    // All remaining functions are unary.
    let [arg] = args else {
        return Err(unsupported_gql_feature(
            GqlFeature::ScalarFunctionRegistry,
            GqlConformanceProfile::PortableGql,
            format!("scalar function `{name}` with {} arguments is not supported yet", args.len()),
        ));
    };
    let value = eval(arg, row, params)?;
    // Cypher scalar functions are null-propagating.
    if matches!(value, Value::Null) {
        return Ok(Value::Null);
    }

    match lower.as_str() {
        "toupper" => restricted_string_transform_value(value, CypherReturnStringTransform::Upper),
        "tolower" => restricted_string_transform_value(value, CypherReturnStringTransform::Lower),
        "trim" => restricted_string_trim_value(value, CypherReturnStringTrim::Both),
        "ltrim" => restricted_string_trim_value(value, CypherReturnStringTrim::Left),
        "rtrim" => restricted_string_trim_value(value, CypherReturnStringTrim::Right),
        "reverse" => restricted_string_reverse_value(value),
        "tostring" => restricted_to_string_value(value),
        "tointeger" => restricted_to_integer_value(value),
        "tofloat" => restricted_to_float_value(value),
        "toboolean" => restricted_to_boolean_value(value),
        "abs" => restricted_abs_value(value),
        "sign" => restricted_numeric_sign_value(value),
        "ceil" => restricted_numeric_round_value(value, CypherReturnNumericRound::Ceil),
        "floor" => restricted_numeric_round_value(value, CypherReturnNumericRound::Floor),
        "round" => match value {
            Value::Int(n) => Ok(Value::Int(n)),
            Value::Float(f) => Ok(Value::Float(f.round())),
            other => Err(gql_type(format!("round() expects a number, got {other:?}"))),
        },
        "size" | "length" => restricted_size_value(value),
        "isempty" => restricted_is_empty_value(value),
        _ => Err(unsupported_gql_feature(
            GqlFeature::ScalarFunctionRegistry,
            GqlConformanceProfile::PortableGql,
            format!("scalar function `{name}` is not supported by the read reference executor yet"),
        )),
    }
}

fn eval_property(base: &Expr, key: &str, row: &Row, params: &CypherParameters) -> Result<Value> {
    if let Expr::Variable(name) = base {
        return match row.get(name) {
            Some(Bound::Node(node)) => Ok(project_node_value(node, key)),
            Some(Bound::Edge(edge)) => Ok(project_edge_value(edge, key)),
            Some(Bound::Value(Value::Json(serde_json::Value::Object(map)))) => {
                Ok(map.get(key).map(json_to_value).unwrap_or(Value::Null))
            }
            Some(Bound::Value(_)) => Ok(Value::Null),
            None => Err(gql_name(format!("variable `{name}` is not bound"))),
        };
    }
    // Nested access: evaluate the base to a JSON object and index it.
    match eval(base, row, params)? {
        Value::Json(serde_json::Value::Object(map)) => Ok(map
            .get(key)
            .map(json_to_value)
            .unwrap_or(Value::Null)),
        _ => Ok(Value::Null),
    }
}

fn eval_unary(op: UnaryOp, operand: &Expr, row: &Row, params: &CypherParameters) -> Result<Value> {
    let value = eval(operand, row, params)?;
    match op {
        UnaryOp::Not => match value {
            Value::Bool(b) => Ok(Value::Bool(!b)),
            Value::Null => Ok(Value::Null),
            other => Err(gql_type(format!("NOT expects a boolean, got {other:?}"))),
        },
        UnaryOp::Negate => match value {
            Value::Int(n) => Ok(Value::Int(-n)),
            Value::Float(f) => Ok(Value::Float(-f)),
            Value::Null => Ok(Value::Null),
            other => Err(gql_type(format!("unary - expects a number, got {other:?}"))),
        },
        UnaryOp::Plus => match value {
            v @ (Value::Int(_) | Value::Float(_) | Value::Null) => Ok(v),
            other => Err(gql_type(format!("unary + expects a number, got {other:?}"))),
        },
    }
}

fn eval_binary(
    op: BinaryOp,
    lhs: &Expr,
    rhs: &Expr,
    row: &Row,
    params: &CypherParameters,
) -> Result<Value> {
    use BinaryOp::*;
    // Boolean connectives implement three-valued logic with short-circuit-free
    // evaluation (both sides are pure here).
    if matches!(op, And | Or | Xor) {
        let a = as_bool(eval(lhs, row, params)?)?;
        let b = as_bool(eval(rhs, row, params)?)?;
        return Ok(three_valued(op, a, b));
    }

    // `IN` is handled before evaluating the right side so a list literal stays a
    // list of `Value`s (round-tripping through JSON would be lossy).
    if op == In {
        let a = eval(lhs, row, params)?;
        return membership(&a, rhs, row, params);
    }

    let a = eval(lhs, row, params)?;
    let b = eval(rhs, row, params)?;
    match op {
        Add | Subtract | Multiply | Divide | Modulo | Power => arithmetic(op, a, b),
        Eq | Ne | Lt | Le | Gt | Ge => Ok(comparison(op, &a, &b)),
        StartsWith | EndsWith | Contains => string_predicate(op, &a, &b),
        In | And | Or | Xor => unreachable!("handled above"),
    }
}

/// Coerce a value to an optional boolean (None == NULL/UNKNOWN).
fn as_bool(value: Value) -> Result<Option<bool>> {
    match value {
        Value::Bool(b) => Ok(Some(b)),
        Value::Null => Ok(None),
        other => Err(gql_type(format!("expected a boolean, got {other:?}"))),
    }
}

fn three_valued(op: BinaryOp, a: Option<bool>, b: Option<bool>) -> Value {
    let result = match op {
        BinaryOp::And => match (a, b) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (Some(true), Some(true)) => Some(true),
            _ => None,
        },
        BinaryOp::Or => match (a, b) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        },
        BinaryOp::Xor => match (a, b) {
            (Some(x), Some(y)) => Some(x != y),
            _ => None,
        },
        _ => None,
    };
    match result {
        Some(b) => Value::Bool(b),
        None => Value::Null,
    }
}

fn comparison(op: BinaryOp, a: &Value, b: &Value) -> Value {
    match op {
        BinaryOp::Eq => to_bool_or_null(values_equal(a, b)),
        BinaryOp::Ne => to_bool_or_null(values_equal(a, b).map(|eq| !eq)),
        _ => match value_order(a, b) {
            Some(ord) => Value::Bool(match op {
                BinaryOp::Lt => ord.is_lt(),
                BinaryOp::Le => ord.is_le(),
                BinaryOp::Gt => ord.is_gt(),
                BinaryOp::Ge => ord.is_ge(),
                _ => unreachable!(),
            }),
            None => Value::Null,
        },
    }
}

fn to_bool_or_null(b: Option<bool>) -> Value {
    match b {
        Some(b) => Value::Bool(b),
        None => Value::Null,
    }
}

fn membership(a: &Value, list_expr: &Expr, row: &Row, params: &CypherParameters) -> Result<Value> {
    if matches!(a, Value::Null) {
        return Ok(Value::Null);
    }
    // Evaluate the candidate values directly (no JSON round trip).
    let candidates: Vec<Value> = match list_expr {
        Expr::List(items) => items
            .iter()
            .map(|e| eval(e, row, params))
            .collect::<Result<_>>()?,
        other => match eval(other, row, params)? {
            Value::StringArray(xs) => xs.into_iter().map(Value::String).collect(),
            Value::IntArray(xs) => xs.into_iter().map(Value::Int).collect(),
            Value::FloatArray(xs) => xs.into_iter().map(Value::Float).collect(),
            Value::Json(serde_json::Value::Array(arr)) => arr.iter().map(json_to_value).collect(),
            Value::Null => return Ok(Value::Null),
            other => return Err(gql_type(format!("IN expects a list, got {other:?}"))),
        },
    };
    let mut saw_null = false;
    for v in &candidates {
        match values_equal(a, v) {
            Some(true) => return Ok(Value::Bool(true)),
            None => saw_null = true,
            Some(false) => {}
        }
    }
    Ok(if saw_null { Value::Null } else { Value::Bool(false) })
}

fn string_predicate(op: BinaryOp, a: &Value, b: &Value) -> Result<Value> {
    let (Value::String(haystack), Value::String(needle)) = (a, b) else {
        if matches!(a, Value::Null) || matches!(b, Value::Null) {
            return Ok(Value::Null);
        }
        return Err(gql_type("string predicates expect string operands".to_string()));
    };
    let result = match op {
        BinaryOp::StartsWith => haystack.starts_with(needle),
        BinaryOp::EndsWith => haystack.ends_with(needle),
        BinaryOp::Contains => haystack.contains(needle),
        _ => unreachable!(),
    };
    Ok(Value::Bool(result))
}

fn arithmetic(op: BinaryOp, a: Value, b: Value) -> Result<Value> {
    if matches!(a, Value::Null) || matches!(b, Value::Null) {
        return Ok(Value::Null);
    }
    // String concatenation with `+`.
    if op == BinaryOp::Add {
        if let (Value::String(x), Value::String(y)) = (&a, &b) {
            return Ok(Value::String(format!("{x}{y}")));
        }
    }
    match (numeric(&a), numeric(&b)) {
        (Some(x), Some(y)) => {
            let both_int = matches!(a, Value::Int(_)) && matches!(b, Value::Int(_));
            let r = match op {
                BinaryOp::Add => x + y,
                BinaryOp::Subtract => x - y,
                BinaryOp::Multiply => x * y,
                BinaryOp::Divide => {
                    if y == 0.0 {
                        return Err(gql_execution("division by zero"));
                    }
                    x / y
                }
                BinaryOp::Modulo => {
                    if y == 0.0 {
                        return Err(gql_execution("modulo by zero"));
                    }
                    x % y
                }
                BinaryOp::Power => x.powf(y),
                _ => unreachable!(),
            };
            if both_int && op != BinaryOp::Divide && op != BinaryOp::Power && r.fract() == 0.0 {
                Ok(Value::Int(r as i64))
            } else {
                Ok(Value::Float(r))
            }
        }
        _ => Err(gql_type("arithmetic expects numeric operands".to_string())),
    }
}

fn numeric(value: &Value) -> Option<f64> {
    match value {
        Value::Int(n) => Some(*n as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

/// Equality with NULL propagation (None == one side NULL / incomparable types).
fn values_equal(a: &Value, b: &Value) -> Option<bool> {
    if matches!(a, Value::Null) || matches!(b, Value::Null) {
        return None;
    }
    if let (Some(x), Some(y)) = (numeric(a), numeric(b)) {
        return Some(x == y);
    }
    match (a, b) {
        (Value::String(x), Value::String(y)) => Some(x == y),
        (Value::Bool(x), Value::Bool(y)) => Some(x == y),
        _ => Some(value_to_json(a) == value_to_json(b)),
    }
}

/// Ordering with NULL propagation (None == one side NULL / incomparable).
fn value_order(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    if matches!(a, Value::Null) || matches!(b, Value::Null) {
        return None;
    }
    if let (Some(x), Some(y)) = (numeric(a), numeric(b)) {
        return x.partial_cmp(&y);
    }
    match (a, b) {
        (Value::String(x), Value::String(y)) => Some(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

fn value_to_json(value: &Value) -> serde_json::Value {
    match serde_json::to_value(value) {
        Ok(json) => json,
        Err(_) => serde_json::Value::Null,
    }
}

fn json_to_value(json: &serde_json::Value) -> Value {
    Value::from(json.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(label: &str, id: &str, props: &[(&str, Value)]) -> Node {
        let mut p = Props::new();
        for (k, v) in props {
            p.insert((*k).to_string(), v.clone());
        }
        Node::new(label, id, p)
    }

    fn graph() -> Graph {
        let nodes = vec![
            node("Person", "p1", &[("name", Value::from("Ada")), ("age", Value::Int(36))]),
            node("Person", "p2", &[("name", Value::from("Alan")), ("age", Value::Int(41))]),
            node("Person", "p3", &[("name", Value::from("Grace")), ("age", Value::Int(85))]),
            node("City", "c1", &[("name", Value::from("London"))]),
        ];
        let edges = vec![
            Edge::new("KNOWS", "p1", "p2", Props::new()),
            Edge::new("KNOWS", "p2", "p3", Props::new()),
            Edge::new("LIVES_IN", "p1", "c1", Props::new()),
        ];
        Graph::new(nodes, edges)
    }

    fn run(cypher: &str) -> CypherResultTable {
        run_read_query(&graph(), cypher, &CypherParameters::new())
            .unwrap_or_else(|e| panic!("query failed: {e}"))
    }

    #[test]
    fn match_all_by_label() {
        let t = run("MATCH (n:Person) RETURN n.name");
        assert_eq!(t.columns, vec!["n.name".to_string()]);
        assert_eq!(t.rows.len(), 3);
    }

    #[test]
    fn match_with_inline_property() {
        let t = run("MATCH (n:Person {name: 'Ada'}) RETURN n.age");
        assert_eq!(t.rows, vec![vec![Value::Int(36)]]);
    }

    #[test]
    fn where_comparison_filters() {
        let t = run("MATCH (n:Person) WHERE n.age >= 40 RETURN n.name ORDER BY n.name");
        let names: Vec<_> = t.rows.iter().map(|r| r[0].clone()).collect();
        assert_eq!(names, vec![Value::from("Alan"), Value::from("Grace")]);
    }

    #[test]
    fn where_boolean_and_or() {
        let t = run("MATCH (n:Person) WHERE n.age < 40 OR n.name = 'Grace' RETURN n.name ORDER BY n.name");
        assert_eq!(t.rows.len(), 2);
    }

    #[test]
    fn where_in_list() {
        let t = run("MATCH (n:Person) WHERE n.name IN ['Ada', 'Grace'] RETURN n.name ORDER BY n.name");
        assert_eq!(
            t.rows.iter().map(|r| r[0].clone()).collect::<Vec<_>>(),
            vec![Value::from("Ada"), Value::from("Grace")]
        );
    }

    #[test]
    fn where_starts_with() {
        let t = run("MATCH (n:Person) WHERE n.name STARTS WITH 'A' RETURN n.name ORDER BY n.name");
        assert_eq!(t.rows.len(), 2);
    }

    #[test]
    fn relationship_hop() {
        let t = run("MATCH (a:Person {name: 'Ada'})-[:KNOWS]->(b:Person) RETURN b.name");
        assert_eq!(t.rows, vec![vec![Value::from("Alan")]]);
    }

    #[test]
    fn relationship_incoming() {
        let t = run("MATCH (a:Person)<-[:KNOWS]-(b:Person {name: 'Ada'}) RETURN a.name");
        assert_eq!(t.rows, vec![vec![Value::from("Alan")]]);
    }

    #[test]
    fn two_hop_path() {
        let t = run("MATCH (a:Person {name:'Ada'})-[:KNOWS]->(b)-[:KNOWS]->(c) RETURN c.name");
        assert_eq!(t.rows, vec![vec![Value::from("Grace")]]);
    }

    #[test]
    fn order_by_desc_skip_limit() {
        let t = run("MATCH (n:Person) RETURN n.name AS name ORDER BY n.age DESC SKIP 1 LIMIT 1");
        assert_eq!(t.columns, vec!["name".to_string()]);
        assert_eq!(t.rows, vec![vec![Value::from("Alan")]]);
    }

    #[test]
    fn distinct_dedups() {
        let t = run("MATCH (n:Person) RETURN DISTINCT n.label");
        assert_eq!(t.rows, vec![vec![Value::from("Person")]]);
    }

    #[test]
    fn return_star_lists_bound_variables() {
        let t = run("MATCH (n:Person {name:'Ada'}) RETURN *");
        assert_eq!(t.columns, vec!["n".to_string()]);
        assert_eq!(t.rows.len(), 1);
    }

    #[test]
    fn parameters_bind() {
        let mut params = CypherParameters::new();
        params.insert("min".to_string(), Value::Int(40));
        let t = run_read_query(&graph(), "MATCH (n:Person) WHERE n.age >= $min RETURN n.name", &params)
            .unwrap();
        assert_eq!(t.rows.len(), 2);
    }

    #[test]
    fn null_comparison_is_unknown_and_filters() {
        // p? has no `age`; comparison yields NULL -> row filtered.
        let t = run("MATCH (n:City) WHERE n.population > 0 RETURN n.name");
        assert!(t.rows.is_empty());
    }

    #[test]
    fn is_null_predicate() {
        let t = run("MATCH (n) WHERE n.population IS NULL RETURN n.name ORDER BY n.name");
        // all 4 nodes lack `population`
        assert_eq!(t.rows.len(), 4);
    }

    #[test]
    fn arithmetic_projection() {
        let t = run("MATCH (n:Person {name:'Ada'}) RETURN n.age + 4");
        assert_eq!(t.rows, vec![vec![Value::Int(40)]]);
    }

    #[test]
    fn unsupported_shapes_are_feature_tagged() {
        let err = run_read_query(&graph(), "MATCH (a)-[:KNOWS*1..3]->(b) RETURN b", &CypherParameters::new())
            .unwrap_err();
        assert!(matches!(err, GrustError::Unsupported(_)));
        // Map literals in projections are still unsupported.
        let err = run_read_query(
            &graph(),
            "MATCH (n:Person) RETURN {age: n.age}",
            &CypherParameters::new(),
        )
        .unwrap_err();
        assert!(matches!(err, GrustError::Unsupported(_)));
    }

    #[test]
    fn union_all_concatenates() {
        let t = run("MATCH (n:Person {name:'Ada'}) RETURN n.name AS x UNION ALL MATCH (m:City) RETURN m.name AS x");
        assert_eq!(t.columns, vec!["x".to_string()]);
        assert_eq!(t.rows.len(), 2);
    }

    #[test]
    fn union_deduplicates() {
        // The same row from both arms collapses under UNION (distinct).
        let t = run("MATCH (n:Person {name:'Ada'}) RETURN n.label AS l UNION MATCH (m:Person {name:'Alan'}) RETURN m.label AS l");
        assert_eq!(t.rows, vec![vec![Value::from("Person")]]);
    }

    #[test]
    fn union_all_keeps_duplicates() {
        let t = run("MATCH (n:Person {name:'Ada'}) RETURN n.label AS l UNION ALL MATCH (m:Person {name:'Alan'}) RETURN m.label AS l");
        assert_eq!(t.rows.len(), 2);
    }

    #[test]
    fn union_mismatched_columns_rejected() {
        let err = run_read_query(
            &graph(),
            "MATCH (n:Person) RETURN n.name AS a UNION MATCH (m:City) RETURN m.name AS b",
            &CypherParameters::new(),
        )
        .unwrap_err();
        assert!(matches!(err, GrustError::CypherUnresolvedIdentity(_)));
    }

    #[test]
    fn with_carry_and_filter() {
        let t = run("MATCH (n:Person) WITH n WHERE n.age > 40 RETURN n.name ORDER BY n.name");
        assert_eq!(
            t.rows.iter().map(|r| r[0].clone()).collect::<Vec<_>>(),
            vec![Value::from("Alan"), Value::from("Grace")]
        );
    }

    #[test]
    fn with_computed_alias() {
        let t = run("MATCH (n:Person) WITH n.age AS age WHERE age >= 40 RETURN age ORDER BY age");
        assert_eq!(t.rows, vec![vec![Value::Int(41)], vec![Value::Int(85)]]);
    }

    #[test]
    fn with_aggregate_then_return() {
        let t = run("MATCH (n) WITH n.label AS label, count(*) AS c RETURN label, c ORDER BY label");
        assert_eq!(
            t.rows,
            vec![
                vec![Value::from("City"), Value::Int(1)],
                vec![Value::from("Person"), Value::Int(3)],
            ]
        );
    }

    #[test]
    fn with_order_limit_horizon() {
        let t = run("MATCH (n:Person) WITH n ORDER BY n.age DESC LIMIT 1 RETURN n.name");
        assert_eq!(t.rows, vec![vec![Value::from("Grace")]]);
    }

    #[test]
    fn with_carries_node_into_later_match() {
        let t = run("MATCH (a:Person {name:'Ada'}) WITH a MATCH (a)-[:KNOWS]->(b) RETURN b.name");
        assert_eq!(t.rows, vec![vec![Value::from("Alan")]]);
    }

    #[test]
    fn optional_match_null_pads() {
        let t = run(
            "MATCH (a:Person) OPTIONAL MATCH (a)-[:KNOWS]->(b) RETURN a.name, b.name ORDER BY a.name",
        );
        assert_eq!(
            t.rows,
            vec![
                vec![Value::from("Ada"), Value::from("Alan")],
                vec![Value::from("Alan"), Value::from("Grace")],
                vec![Value::from("Grace"), Value::Null],
            ]
        );
    }

    #[test]
    fn optional_match_where_excludes_all_null_pads() {
        let t = run(
            "MATCH (a:Person {name:'Ada'}) OPTIONAL MATCH (a)-[:KNOWS]->(b) WHERE b.name = 'Zzz' RETURN b.name",
        );
        assert_eq!(t.rows, vec![vec![Value::Null]]);
    }

    #[test]
    fn unwind_list() {
        let t = run("UNWIND [1, 2, 3] AS x RETURN x");
        assert_eq!(
            t.rows,
            vec![vec![Value::Int(1)], vec![Value::Int(2)], vec![Value::Int(3)]]
        );
    }

    #[test]
    fn unwind_cross_product_with_match() {
        let t = run("MATCH (n:Person) UNWIND [1, 2] AS k RETURN n.name, k");
        assert_eq!(t.rows.len(), 6);
    }

    #[test]
    fn searched_case_expression() {
        let t = run(
            "MATCH (n:Person) RETURN CASE WHEN n.age >= 80 THEN 'senior' WHEN n.age >= 40 THEN 'mid' ELSE 'young' END AS bucket ORDER BY n.name",
        );
        assert_eq!(
            t.rows.iter().map(|r| r[0].clone()).collect::<Vec<_>>(),
            vec![Value::from("young"), Value::from("mid"), Value::from("senior")]
        );
    }

    #[test]
    fn simple_case_expression() {
        let t = run("MATCH (n:Person {name:'Ada'}) RETURN CASE n.age WHEN 36 THEN 'yes' ELSE 'no' END AS m");
        assert_eq!(t.rows, vec![vec![Value::from("yes")]]);
    }

    #[test]
    fn case_without_else_is_null() {
        let t = run("MATCH (n:Person {name:'Ada'}) RETURN CASE WHEN n.age > 100 THEN 'old' END AS m");
        assert_eq!(t.rows, vec![vec![Value::Null]]);
    }

    #[test]
    fn unbound_variable_rejected_by_semantics() {
        let err = run_read_query(&graph(), "MATCH (n:Person) RETURN m.name", &CypherParameters::new())
            .unwrap_err();
        assert!(matches!(err, GrustError::CypherUnresolvedIdentity(_)));
    }

    #[test]
    fn executes_against_memory_graph_store() {
        use futures_executor::block_on;
        use grust_memory::MemoryGraphStore;

        let store = MemoryGraphStore::new();
        block_on(async {
            store
                .put_node(&node("Person", "p1", &[("name", Value::from("Ada"))]))
                .await
                .unwrap();
            store
                .put_node(&node("Person", "p2", &[("name", Value::from("Alan"))]))
                .await
                .unwrap();
            store
                .put_edge(&Edge::new("KNOWS", "p1", "p2", Props::new()))
                .await
                .unwrap();
        });

        // The Memory reference executor reads the materialized graph snapshot.
        let snapshot = store.graph();
        let table = run_read_query(
            &snapshot,
            "MATCH (a:Person {name: 'Ada'})-[:KNOWS]->(b:Person) RETURN b.name AS friend",
            &CypherParameters::new(),
        )
        .unwrap();
        assert_eq!(table.columns, vec!["friend".to_string()]);
        assert_eq!(table.rows, vec![vec![Value::from("Alan")]]);
    }

    #[test]
    fn count_star() {
        let t = run("MATCH (n:Person) RETURN count(*)");
        assert_eq!(t.rows, vec![vec![Value::Int(3)]]);
    }

    #[test]
    fn count_over_empty_match_is_zero() {
        let t = run("MATCH (n:Nonexistent) RETURN count(*)");
        assert_eq!(t.rows, vec![vec![Value::Int(0)]]);
    }

    #[test]
    fn min_max_avg() {
        let t = run("MATCH (n:Person) RETURN min(n.age) AS lo, max(n.age) AS hi");
        assert_eq!(t.columns, vec!["lo".to_string(), "hi".to_string()]);
        assert_eq!(t.rows, vec![vec![Value::Int(36), Value::Int(85)]]);
    }

    #[test]
    fn group_by_label_with_count() {
        let t = run("MATCH (n) RETURN n.label AS label, count(*) AS c ORDER BY label");
        assert_eq!(
            t.rows,
            vec![
                vec![Value::from("City"), Value::Int(1)],
                vec![Value::from("Person"), Value::Int(3)],
            ]
        );
    }

    #[test]
    fn count_property_skips_nulls() {
        // No Person has `population`, so count(n.population) == 0.
        let t = run("MATCH (n:Person) RETURN count(n.population)");
        assert_eq!(t.rows, vec![vec![Value::Int(0)]]);
    }

    #[test]
    fn collect_gathers_values() {
        let t = run("MATCH (n:Person) RETURN collect(n.name) AS names");
        assert_eq!(t.rows.len(), 1);
        match &t.rows[0][0] {
            Value::Json(serde_json::Value::Array(items)) => assert_eq!(items.len(), 3),
            other => panic!("expected a list, got {other:?}"),
        }
    }

    #[test]
    fn count_distinct() {
        let t = run("MATCH (n) RETURN count(DISTINCT n.label) AS kinds");
        assert_eq!(t.rows, vec![vec![Value::Int(2)]]);
    }

    #[test]
    fn scalar_string_functions() {
        let t = run("MATCH (n:Person {name:'Ada'}) RETURN toUpper(n.name) AS u, size(n.name) AS s");
        assert_eq!(t.rows, vec![vec![Value::from("ADA"), Value::Int(3)]]);
    }

    #[test]
    fn scalar_numeric_functions() {
        let t = run("MATCH (n:Person {name:'Ada'}) RETURN abs(0 - n.age) AS a, toString(n.age) AS s");
        assert_eq!(t.rows, vec![vec![Value::Int(36), Value::from("36")]]);
    }

    #[test]
    fn coalesce_picks_first_non_null() {
        let t = run("MATCH (n:Person {name:'Ada'}) RETURN coalesce(n.population, n.age, 0) AS c");
        assert_eq!(t.rows, vec![vec![Value::Int(36)]]);
    }

    #[test]
    fn scalar_function_null_propagates() {
        let t = run("MATCH (n:Person {name:'Ada'}) RETURN toUpper(n.population) AS u");
        assert_eq!(t.rows, vec![vec![Value::Null]]);
    }

    #[test]
    fn scalar_in_where_filters() {
        let t = run("MATCH (n:Person) WHERE toUpper(n.name) = 'ADA' RETURN n.name");
        assert_eq!(t.rows, vec![vec![Value::from("Ada")]]);
    }

    #[test]
    fn unknown_scalar_function_is_feature_tagged() {
        let err = run_read_query(
            &graph(),
            "MATCH (n:Person) RETURN sqrt(n.age)",
            &CypherParameters::new(),
        )
        .unwrap_err();
        assert!(matches!(err, GrustError::Unsupported(_)));
    }
}
