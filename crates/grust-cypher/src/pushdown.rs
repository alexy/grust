//! Backend-neutral read-query pushdown (Unit 15 of `docs/GQL_GOAL.md`).
//!
//! The Memory reference executor in [`crate::read`] runs the bounded read subset
//! portably over an in-memory [`Graph`]. A persistent backend (Sail/Spark,
//! Turso/SQLite, …) does not want to materialize the whole graph: it wants to
//! push the selective `MATCH`/`WHERE` *filter* down into its own SQL and fetch
//! only the surviving rows. This module performs that **lowering** in a
//! backend-neutral way:
//!
//! 1. [`plan_node_read`] lowers a supported read query into a [`NodeReadPushdown`]
//!    logical descriptor (or returns `Ok(None)` for any shape outside the pushable
//!    subset, so the backend can cleanly fall back to the reference rather than
//!    risk a wrong answer).
//! 2. [`NodeReadPushdown::to_sql`] renders the scan + filter into SQL for a given
//!    [`SqlDialect`] (a small string-formatting config — `GET_JSON_OBJECT` vs
//!    `json_extract`, identifier quoting, numeric casts).
//! 3. [`NodeReadPushdown::project`] takes the nodes the backend fetched and runs
//!    the `RETURN` projection through the **shared reference projection**
//!    ([`crate::read::project_nodes`]), so the pushdown result is byte-identical
//!    to [`crate::read::run_read_query`] by construction.
//!
//! Only the MATCH/WHERE → SQL filter therefore has to be proven equivalent to the
//! reference (the differential row-equality harness does this against an embedded
//! backend). The module has **zero backend dependencies** and is fully unit
//! tested here.
//!
//! ## Pushable subset (milestone 1: single node pattern)
//!
//! `MATCH (var[:Label] [{k: lit, …}]) [WHERE <pred>] RETURN …`, where `<pred>` is
//! a conjunction/disjunction/negation of:
//! - property comparisons `var.key <op> <lit>` (`=`,`<>`,`<`,`<=`,`>`,`>=`) with
//!   an integer/float/string literal (or a parameter resolving to one),
//! - `var.key IS [NOT] NULL`.
//!
//! The `RETURN` projection is unrestricted — it runs in Rust via the reference,
//! so aliases, `*`, `DISTINCT`, `ORDER BY`, `SKIP`/`LIMIT`, and aggregates all
//! work. Everything outside the MATCH/WHERE subset (relationship segments,
//! variable length, OPTIONAL, WITH, UNWIND, UNION, `IN`, `STARTS/ENDS/CONTAINS`,
//! arithmetic predicates, functions in WHERE) yields `Ok(None)` → reference
//! fallback. Wider pushdown lands in later commits, gated by the oracle.

use crate::ast::*;
use crate::parser::parse_query;
use crate::{CypherParameters, CypherResultTable, Result};
use grust_core::{Node, Value};

// ---------------------------------------------------------------------------
// Logical descriptor
// ---------------------------------------------------------------------------

/// A lowered single-node read: a label-scoped node scan, an optional pushable
/// filter, and the `RETURN` projection (executed in Rust on the fetched nodes).
#[derive(Clone, Debug, PartialEq)]
pub struct NodeReadPushdown {
    /// The pattern variable bound to the scanned node (e.g. `n`).
    var: String,
    /// The required node label, if the pattern specified one (`MATCH (n:Person)`).
    label: Option<String>,
    /// The conjunction of inline-property equalities and the `WHERE` predicate,
    /// already lowered to a backend-neutral form. `None` means "no filter".
    filter: Option<Predicate>,
    /// `ORDER BY`/`SKIP`/`LIMIT` lowered for SQL pushdown, when structurally
    /// pushable (no aggregate/`DISTINCT`, keys are scan-var props/label,
    /// skip/limit are non-negative integers). Only emitted by `to_sql` for
    /// dialects whose JSON extraction is typed (see [`SqlDialect::orders_json_typed`]).
    ordering: Option<PushedOrdering>,
    /// The `RETURN` projection, run through the shared reference projection.
    projection: Projection,
}

/// `ORDER BY`/`SKIP`/`LIMIT` resolved for SQL pushdown.
#[derive(Clone, Debug, PartialEq)]
struct PushedOrdering {
    keys: Vec<OrderKey>,
    skip: Option<usize>,
    limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
struct OrderKey {
    prop: PropRef,
    descending: bool,
    /// The property's scalar kind from [`TypeHints`] (`Some(Str)` for the label
    /// column). Used to cast the sort key on untyped-JSON dialects; `None` means
    /// the kind is unknown, so ordering is not pushable on such dialects.
    kind: Option<ScalarKind>,
}

/// A backend-neutral, pushable filter predicate over the scanned node.
#[derive(Clone, Debug, PartialEq)]
enum Predicate {
    And(Box<Predicate>, Box<Predicate>),
    Or(Box<Predicate>, Box<Predicate>),
    Not(Box<Predicate>),
    /// `prop <op> literal`.
    Compare {
        prop: PropRef,
        op: CmpOp,
        value: Scalar,
    },
    /// `prop IS [NOT] NULL`.
    IsNull {
        prop: PropRef,
        negated: bool,
    },
    /// `prop IN [literal, …]` (non-empty, homogeneous literals).
    In {
        prop: PropRef,
        values: Vec<Scalar>,
    },
    /// `prop STARTS WITH / ENDS WITH / CONTAINS 'needle'` (non-empty needle).
    StringPred {
        prop: PropRef,
        op: StrOp,
        needle: String,
    },
    /// `prop = true|false` / `prop <> true|false` (op is `Eq` or `Ne`).
    BoolCompare {
        prop: PropRef,
        op: CmpOp,
        value: bool,
    },
    /// `<arith> <op> <arith>`, where at least one side references a property.
    ArithCompare {
        lhs: ArithExpr,
        op: CmpOp,
        rhs: ArithExpr,
    },
}

/// A pushable arithmetic operand: `+`/`-`/`*` over typed numeric properties and
/// numeric literals. Each property carries its (numeric) kind so it can be cast.
#[derive(Clone, Debug, PartialEq)]
enum ArithExpr {
    Prop(PropRef, ScalarKind),
    Int(i64),
    Float(f64),
    Bin(ArithOp, Box<ArithExpr>, Box<ArithExpr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArithOp {
    Add,
    Sub,
    Mul,
    /// Division — the reference always computes in f64, so it is rendered as
    /// floating-point division (both operands cast to the dialect float type),
    /// making it dialect-equal. (`/0` errors in the reference but yields
    /// NULL→dropped under pushdown; documented caveat.)
    Div,
}

impl ArithOp {
    fn sql(self) -> &'static str {
        match self {
            ArithOp::Add => "+",
            ArithOp::Sub => "-",
            ArithOp::Mul => "*",
            ArithOp::Div => "/",
        }
    }
}

/// A reference to a property of the scanned node. `label` maps to the backend's
/// label column; everything else maps to a JSON property of the `props` column —
/// mirroring [`crate::read`]'s `project_node_value` (label is synthetic; other
/// keys come from the property map).
#[derive(Clone, Debug, PartialEq)]
enum PropRef {
    Label,
    Key(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    fn sql(self) -> &'static str {
        match self {
            CmpOp::Eq => "=",
            CmpOp::Ne => "<>",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        }
    }

    /// The operator with its operands swapped (for `literal <op> prop`).
    fn flipped(self) -> CmpOp {
        match self {
            CmpOp::Eq => CmpOp::Eq,
            CmpOp::Ne => CmpOp::Ne,
            CmpOp::Lt => CmpOp::Gt,
            CmpOp::Le => CmpOp::Ge,
            CmpOp::Gt => CmpOp::Lt,
            CmpOp::Ge => CmpOp::Le,
        }
    }
}

/// A literal comparison operand. Only the types with a clean, dialect-stable SQL
/// rendering are supported in milestone 1 (booleans/temporals are deferred).
#[derive(Clone, Debug, PartialEq)]
enum Scalar {
    Int(i64),
    Float(f64),
    Str(String),
}

/// Coarse kind of a [`Scalar`], for casting and homogeneity checks. Also the
/// vocabulary of [`TypeHints`] (a backend's per-property type knowledge).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarKind {
    Int,
    Float,
    Str,
}

/// A string prefix/suffix/substring predicate operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrOp {
    StartsWith,
    EndsWith,
    Contains,
}

/// Backend-supplied property type knowledge, used to push `ORDER BY` correctly on
/// dialects whose JSON extraction is *untyped* (e.g. Spark `GET_JSON_OBJECT`
/// returns text). With a known numeric kind, the sort key is cast to that type so
/// the SQL order matches the reference; without it, ordering stays in the
/// reference projection. Dialects with typed JSON extraction (SQLite/libSQL)
/// ignore hints. Build one from a `GraphSchema` in the backend.
pub trait TypeHints {
    /// The scalar kind of property `key` on a node of `label` (if known). `label`
    /// is `None` for an unlabeled pattern.
    fn node_property_kind(&self, label: Option<&str>, key: &str) -> Option<ScalarKind>;

    /// The scalar kind of property `key` on a relationship of `edge_type` (if
    /// known). `edge_type` is `None` when the pattern's relationship type is
    /// absent or ambiguous (multiple types), in which case ordering by an edge
    /// property stays in the reference projection on untyped-JSON dialects.
    fn edge_property_kind(&self, _edge_type: Option<&str>, _key: &str) -> Option<ScalarKind> {
        None
    }
}

/// A [`TypeHints`] that knows nothing — every lookup returns `None`. Backends
/// without an applied schema use this; untyped-JSON dialects then keep ordering
/// in the reference projection.
pub struct NoTypeHints;

impl TypeHints for NoTypeHints {
    fn node_property_kind(&self, _label: Option<&str>, _key: &str) -> Option<ScalarKind> {
        None
    }
}

impl Scalar {
    fn kind(&self) -> ScalarKind {
        match self {
            Scalar::Int(_) => ScalarKind::Int,
            Scalar::Float(_) => ScalarKind::Float,
            Scalar::Str(_) => ScalarKind::Str,
        }
    }

    fn render(&self) -> String {
        match self {
            Scalar::Int(n) => n.to_string(),
            Scalar::Float(f) => render_float(*f),
            // String literals are rendered by the dialect (escaping); see callers.
            Scalar::Str(_) => unreachable!("string scalars render via the dialect"),
        }
    }
}

/// Lower an `Expr::List` of constant literals into a non-empty, homogeneous
/// scalar list (the only `IN` right-hand side that is cleanly pushable). Returns
/// `None` for empty, mixed-kind, or non-literal lists → reference fallback.
fn lower_scalar_list(expr: &Expr, params: &CypherParameters) -> Option<Vec<Scalar>> {
    let Expr::List(items) = expr else {
        return None;
    };
    if items.is_empty() {
        return None;
    }
    let values: Vec<Scalar> = items
        .iter()
        .map(|e| lower_scalar(e, params))
        .collect::<Option<_>>()?;
    let kind = values[0].kind();
    if values.iter().all(|v| v.kind() == kind) {
        Some(values)
    } else {
        None
    }
}

/// Apply the numeric cast a comparison/`IN` needs, given the operand's SQL form,
/// whether it is a (text) label column, and the literal kind.
fn cast_for_kind(raw: String, is_label: bool, kind: ScalarKind, dialect: &dyn SqlDialect) -> String {
    match kind {
        ScalarKind::Str => raw,
        _ if is_label => raw,
        ScalarKind::Int => dialect.cast_int(&raw),
        ScalarKind::Float => dialect.cast_float(&raw),
    }
}

/// Render an `IN (…)` list: cast the operand by the (homogeneous) element kind
/// and render each element with the dialect.
fn render_in_list(
    raw: String,
    is_label: bool,
    values: &[Scalar],
    dialect: &dyn SqlDialect,
) -> String {
    let lhs = cast_for_kind(raw, is_label, values[0].kind(), dialect);
    let list = values
        .iter()
        .map(|v| match v {
            Scalar::Str(s) => dialect.string_literal(s),
            other => other.render(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{lhs} IN ({list})")
}

// ---------------------------------------------------------------------------
// SQL dialects
// ---------------------------------------------------------------------------

/// The dialect-specific string formatting a backend needs to render a pushdown
/// scan + filter. Implementations are pure string config — no backend state.
pub trait SqlDialect {
    /// The generic node table name (e.g. `grust_nodes`).
    fn nodes_table(&self) -> &str;
    /// The generic edge table name (e.g. `grust_edges`).
    fn edges_table(&self) -> &str {
        "grust_edges"
    }
    /// Quote a table/column identifier (e.g. backticks for Spark).
    fn quote_ident(&self, ident: &str) -> String;
    /// Extract JSON property `key` from the props column, yielding a SQL scalar.
    fn json_property(&self, props_column: &str, key: &str) -> String;
    /// Wrap `expr` in an integer cast.
    fn cast_int(&self, expr: &str) -> String;
    /// Wrap `expr` in a floating-point cast.
    fn cast_float(&self, expr: &str) -> String;
    /// Render a string literal, safely escaped for this dialect.
    fn string_literal(&self, value: &str) -> String;
    /// Render a string prefix/suffix/substring predicate `expr <op> needle`
    /// (`STARTS WITH`/`ENDS WITH`/`CONTAINS`). `needle` is non-empty; the dialect
    /// escapes it. NULL `expr` must yield NULL (so the row is dropped), matching
    /// the reference's null handling.
    fn string_predicate(&self, expr: &str, op: StrOp, needle: &str) -> String;
    /// Render the SQL literal a JSON-extracted boolean compares equal to: SQLite
    /// `json_extract` yields `1`/`0`; Spark `GET_JSON_OBJECT` yields `'true'`/`'false'`.
    fn bool_literal_sql(&self, value: bool) -> String;
    /// Whether `json_property` yields a **natively typed** scalar that sorts
    /// numerically/lexically like the reference (e.g. SQLite/libSQL `json_extract`
    /// returns INTEGER/REAL/TEXT). When false (e.g. Spark `GET_JSON_OBJECT`
    /// returns text, so numeric `ORDER BY` would sort lexicographically),
    /// `ORDER BY`/`SKIP`/`LIMIT` are kept in the reference projection rather than
    /// pushed, to preserve row-equality.
    fn orders_json_typed(&self) -> bool {
        false
    }
}

/// Spark SQL dialect (the Sail backend): `GET_JSON_OBJECT`, backtick idents,
/// `CAST(… AS BIGINT/DOUBLE)`, backslash + quote escaping.
#[derive(Clone, Copy, Debug, Default)]
pub struct SparkDialect;

impl SqlDialect for SparkDialect {
    fn nodes_table(&self) -> &str {
        "grust_nodes"
    }
    fn quote_ident(&self, ident: &str) -> String {
        format!("`{ident}`")
    }
    fn json_property(&self, props_column: &str, key: &str) -> String {
        format!("GET_JSON_OBJECT({props_column}, '$.{key}')")
    }
    fn cast_int(&self, expr: &str) -> String {
        format!("CAST({expr} AS BIGINT)")
    }
    fn cast_float(&self, expr: &str) -> String {
        format!("CAST({expr} AS DOUBLE)")
    }
    fn string_literal(&self, value: &str) -> String {
        // Spark SQL treats backslash as an escape character, so double both
        // backslashes and single quotes (matches grust-sail's `sql_str`).
        format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
    }
    fn string_predicate(&self, expr: &str, op: StrOp, needle: &str) -> String {
        let n = self.string_literal(needle);
        match op {
            StrOp::StartsWith => format!("STARTSWITH({expr}, {n})"),
            StrOp::EndsWith => format!("ENDSWITH({expr}, {n})"),
            StrOp::Contains => format!("CONTAINS({expr}, {n})"),
        }
    }
    fn bool_literal_sql(&self, value: bool) -> String {
        // GET_JSON_OBJECT returns the JSON boolean as text.
        if value { "'true'".to_string() } else { "'false'".to_string() }
    }
}

/// SQLite / libSQL dialect (the Turso backend, also the embedded differential
/// oracle): `json_extract`, bare idents, `CAST(… AS INTEGER/REAL)`, quote
/// doubling (backslash is not special in SQLite string literals).
#[derive(Clone, Copy, Debug, Default)]
pub struct SqliteDialect;

impl SqlDialect for SqliteDialect {
    fn nodes_table(&self) -> &str {
        "grust_nodes"
    }
    fn quote_ident(&self, ident: &str) -> String {
        format!("\"{ident}\"")
    }
    fn json_property(&self, props_column: &str, key: &str) -> String {
        format!("json_extract({props_column}, '$.{key}')")
    }
    fn cast_int(&self, expr: &str) -> String {
        format!("CAST({expr} AS INTEGER)")
    }
    fn cast_float(&self, expr: &str) -> String {
        format!("CAST({expr} AS REAL)")
    }
    fn string_literal(&self, value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }
    fn string_predicate(&self, expr: &str, op: StrOp, needle: &str) -> String {
        // `instr`/`substr` do literal (non-`LIKE`) matching, avoiding wildcard
        // escaping; NULL `expr` propagates to NULL. `needle` is non-empty.
        let n = self.string_literal(needle);
        match op {
            StrOp::StartsWith => format!("instr({expr}, {n}) = 1"),
            StrOp::Contains => format!("instr({expr}, {n}) > 0"),
            // Compare the last `needle.len()` characters (SQLite counts chars).
            StrOp::EndsWith => format!("substr({expr}, -{}) = {n}", needle.chars().count()),
        }
    }
    fn bool_literal_sql(&self, value: bool) -> String {
        // json_extract returns the JSON boolean as integer 1/0.
        if value { "1".to_string() } else { "0".to_string() }
    }
    fn orders_json_typed(&self) -> bool {
        // SQLite / libSQL `json_extract` returns INTEGER/REAL/TEXT, so ORDER BY
        // sorts by type+value the way the reference does.
        true
    }
}

// ---------------------------------------------------------------------------
// Planning (AST -> NodeReadPushdown)
// ---------------------------------------------------------------------------

/// Try to lower `cypher` into a single-node read pushdown.
///
/// Returns `Ok(Some(_))` for the pushable subset, `Ok(None)` for a valid query
/// outside it (the caller should fall back to the reference executor), and `Err`
/// only for genuinely invalid syntax/semantics.
pub fn plan_node_read(
    cypher: &str,
    params: &CypherParameters,
) -> Result<Option<NodeReadPushdown>> {
    plan_node_read_with_hints(cypher, params, &NoTypeHints)
}

/// Like [`plan_node_read`], but resolves `ORDER BY` key types via `hints` so an
/// untyped-JSON dialect (e.g. Spark) can still push numeric ordering by casting.
pub fn plan_node_read_with_hints(
    cypher: &str,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Result<Option<NodeReadPushdown>> {
    let query = parse_query(cypher).map_err(|e| e.into_grust(cypher))?;
    crate::semantics::analyze(&query)?;
    Ok(single_query(&query).and_then(|s| lower_node_single(s, params, hints)))
}

/// The sole [`SingleQuery`] of a non-`UNION` query, or `None`.
fn single_query(query: &Query) -> Option<&SingleQuery> {
    if query.parts.len() == 1 && query.parts[0].union.is_none() {
        Some(&query.parts[0].query)
    } else {
        None
    }
}

fn lower_node_single(
    single: &SingleQuery,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Option<NodeReadPushdown> {
    // Exactly `MATCH … RETURN …`.
    let (match_clause, return_clause) = match single.clauses.as_slice() {
        [Clause::Match(m), Clause::Return(r)] if !m.optional => (m, r),
        _ => return None,
    };
    let (var, label, filter) = lower_node_scan(match_clause, params, hints)?;
    let projection = return_clause.projection.clone();
    let ordering = compute_pushed_ordering(&projection, &var, label.as_deref(), params, hints);
    Some(NodeReadPushdown {
        var,
        label,
        filter,
        ordering,
        projection,
    })
}

/// Lower a single-node `MATCH` clause to its scan target — `(var, label, filter)`
/// — or `None` if it is not a pushable single bound node pattern. Shared by the
/// node leaf and the `WITH`-pipeline leaf.
fn lower_node_scan(
    match_clause: &MatchClause,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Option<(String, Option<String>, Option<Predicate>)> {
    if match_clause.patterns.len() != 1 {
        return None;
    }
    let pattern = &match_clause.patterns[0];
    if pattern.variable.is_some() || !pattern.segments.is_empty() {
        return None;
    }
    let node = &pattern.start;
    let var = node.variable.clone()?;
    if node.labels.len() > 1 {
        return None;
    }
    let label = node.labels.first().cloned();

    let mut filter: Option<Predicate> = None;
    if let Some(map) = &node.properties {
        for (key, value_expr) in &map.entries {
            let scalar = lower_scalar(value_expr, params)?;
            filter = Some(conjoin(
                filter,
                Predicate::Compare {
                    prop: lower_prop_key(key)?,
                    op: CmpOp::Eq,
                    value: scalar,
                },
            ));
        }
    }
    if let Some(where_expr) = &match_clause.where_clause {
        let predicate = lower_predicate(where_expr, &var, label.as_deref(), params, hints)?;
        filter = Some(conjoin(filter, predicate));
    }
    Some((var, label, filter))
}

/// Resolve `ORDER BY`/`SKIP`/`LIMIT` for SQL pushdown over a single scan var, or
/// `None` if the projection is not structurally pushable (so ordering stays in
/// the reference projection). Requires: no aggregates, no `DISTINCT`, a non-empty
/// `ORDER BY` whose keys all resolve (through `RETURN` aliases) to a property or
/// label of `var`, and `SKIP`/`LIMIT` that resolve to non-negative integers.
fn compute_pushed_ordering(
    projection: &Projection,
    var: &str,
    label: Option<&str>,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Option<PushedOrdering> {
    if projection.distinct {
        return None;
    }
    if projection.items.iter().any(|i| crate::read::expr_has_aggregate(&i.expr)) {
        return None;
    }
    // A bare SKIP/LIMIT with no ORDER BY would select an arbitrary subset (not the
    // reference's), so only push when there is a total-ish sort to anchor it.
    if projection.order_by.is_empty() {
        return None;
    }
    // alias -> underlying RETURN expression (ORDER BY may reference an alias).
    let aliases: std::collections::HashMap<&str, &Expr> = projection
        .items
        .iter()
        .filter_map(|i| i.alias.as_deref().map(|a| (a, &i.expr)))
        .collect();

    let mut keys = Vec::with_capacity(projection.order_by.len());
    for item in &projection.order_by {
        let resolved = match &item.expr {
            Expr::Variable(name) => aliases.get(name.as_str()).copied().unwrap_or(&item.expr),
            other => other,
        };
        let prop = lower_prop_ref(resolved, var)?;
        // The label column is text; other keys take their kind from the hints
        // (used only to cast on untyped-JSON dialects).
        let kind = match &prop {
            PropRef::Label => Some(ScalarKind::Str),
            PropRef::Key(key) => hints.node_property_kind(label, key),
        };
        keys.push(OrderKey {
            prop,
            descending: item.descending,
            kind,
        });
    }
    let skip = match &projection.skip {
        None => None,
        Some(e) => Some(lower_usize(e, params)?),
    };
    let limit = match &projection.limit {
        None => None,
        Some(e) => Some(lower_usize(e, params)?),
    };
    Some(PushedOrdering { keys, skip, limit })
}

/// Resolve a `SKIP`/`LIMIT` expression to a non-negative integer (literal or a
/// parameter bound to a non-negative `Value::Int`), or `None` if not.
fn lower_usize(expr: &Expr, params: &CypherParameters) -> Option<usize> {
    let value = match expr {
        Expr::Integer(n) => *n,
        Expr::Parameter(name) => match params.get(name)? {
            Value::Int(n) => *n,
            _ => return None,
        },
        _ => return None,
    };
    usize::try_from(value).ok()
}

fn conjoin(existing: Option<Predicate>, next: Predicate) -> Predicate {
    match existing {
        None => next,
        Some(prev) => Predicate::And(Box::new(prev), Box::new(next)),
    }
}

/// Lower a boolean `WHERE` expression into a pushable [`Predicate`], or `None`
/// if any part is outside the pushable subset. `label`/`hints` resolve property
/// types for arithmetic operands.
fn lower_predicate(
    expr: &Expr,
    var: &str,
    label: Option<&str>,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Option<Predicate> {
    match expr {
        Expr::Binary {
            op: BinaryOp::And,
            lhs,
            rhs,
        } => Some(Predicate::And(
            Box::new(lower_predicate(lhs, var, label, params, hints)?),
            Box::new(lower_predicate(rhs, var, label, params, hints)?),
        )),
        Expr::Binary {
            op: BinaryOp::Or,
            lhs,
            rhs,
        } => Some(Predicate::Or(
            Box::new(lower_predicate(lhs, var, label, params, hints)?),
            Box::new(lower_predicate(rhs, var, label, params, hints)?),
        )),
        Expr::Unary {
            op: UnaryOp::Not,
            operand,
        } => Some(Predicate::Not(Box::new(lower_predicate(
            operand, var, label, params, hints,
        )?))),
        Expr::IsNull { operand, negated } => {
            let prop = lower_prop_ref(operand, var)?;
            Some(Predicate::IsNull {
                prop,
                negated: *negated,
            })
        }
        Expr::Binary {
            op: BinaryOp::In,
            lhs,
            rhs,
        } => {
            let prop = lower_prop_ref(lhs, var)?;
            let values = lower_scalar_list(rhs, params)?;
            Some(Predicate::In { prop, values })
        }
        Expr::Binary {
            op: BinaryOp::StartsWith | BinaryOp::EndsWith | BinaryOp::Contains,
            lhs,
            rhs,
        } => {
            let prop = lower_prop_ref(lhs, var)?;
            let needle = lower_string_needle(rhs, params)?;
            Some(Predicate::StringPred {
                prop,
                op: lower_str_op(expr),
                needle,
            })
        }
        Expr::Binary { op, lhs, rhs } => {
            let cmp = lower_cmp_op(*op)?;
            // `prop = true|false` (Eq/Ne only) — booleans render dialect-specific.
            if matches!(cmp, CmpOp::Eq | CmpOp::Ne) {
                if let (Some(prop), Some(value)) = (lower_prop_ref(lhs, var), lower_bool(rhs, params))
                {
                    return Some(Predicate::BoolCompare { prop, op: cmp, value });
                }
                if let (Some(prop), Some(value)) = (lower_prop_ref(rhs, var), lower_bool(lhs, params))
                {
                    return Some(Predicate::BoolCompare { prop, op: cmp, value });
                }
            }
            // `prop <op> literal` / `literal <op> prop`.
            if let Some(prop) = lower_prop_ref(lhs, var) {
                if let Some(value) = lower_scalar(rhs, params) {
                    return Some(Predicate::Compare { prop, op: cmp, value });
                }
            }
            if let Some(prop) = lower_prop_ref(rhs, var) {
                if let Some(value) = lower_scalar(lhs, params) {
                    return Some(Predicate::Compare {
                        prop,
                        op: cmp.flipped(),
                        value,
                    });
                }
            }
            // Arithmetic comparison (`+`/`-`/`*` over typed numeric properties).
            let l = lower_arith(lhs, var, label, params, hints)?;
            let r = lower_arith(rhs, var, label, params, hints)?;
            if arith_has_prop(&l) || arith_has_prop(&r) {
                Some(Predicate::ArithCompare { lhs: l, op: cmp, rhs: r })
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Lower an expression to a pushable arithmetic operand (`+`/`-`/`*` over typed
/// numeric properties of `var`, numeric literals, and parameters). `None` if any
/// part is non-numeric, an unknown-typed property, or uses `/`/`%`/`^` (which are
/// dialect-divergent for exact reference equality).
fn lower_arith(
    expr: &Expr,
    var: &str,
    label: Option<&str>,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Option<ArithExpr> {
    match expr {
        Expr::Integer(n) => Some(ArithExpr::Int(*n)),
        Expr::Float(f) if f.is_finite() => Some(ArithExpr::Float(*f)),
        Expr::Parameter(name) => match params.get(name)? {
            Value::Int(n) => Some(ArithExpr::Int(*n)),
            Value::Float(f) if f.is_finite() => Some(ArithExpr::Float(*f)),
            _ => None,
        },
        Expr::Property { base, key } => {
            let Expr::Variable(name) = base.as_ref() else {
                return None;
            };
            if name != var {
                return None;
            }
            let kind = match hints.node_property_kind(label, key)? {
                ScalarKind::Int => ScalarKind::Int,
                ScalarKind::Float => ScalarKind::Float,
                ScalarKind::Str => return None,
            };
            Some(ArithExpr::Prop(PropRef::Key(lower_seg_key(key)?), kind))
        }
        Expr::Binary {
            op: op @ (BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide),
            lhs,
            rhs,
        } => {
            let op = match op {
                BinaryOp::Add => ArithOp::Add,
                BinaryOp::Subtract => ArithOp::Sub,
                BinaryOp::Multiply => ArithOp::Mul,
                _ => ArithOp::Div,
            };
            Some(ArithExpr::Bin(
                op,
                Box::new(lower_arith(lhs, var, label, params, hints)?),
                Box::new(lower_arith(rhs, var, label, params, hints)?),
            ))
        }
        _ => None,
    }
}

fn arith_has_prop(expr: &ArithExpr) -> bool {
    match expr {
        ArithExpr::Prop(..) => true,
        ArithExpr::Int(_) | ArithExpr::Float(_) => false,
        ArithExpr::Bin(_, l, r) => arith_has_prop(l) || arith_has_prop(r),
    }
}

/// Lower an expression to a boolean literal (literal or a parameter bound to a
/// `Value::Bool`), else `None`.
fn lower_bool(expr: &Expr, params: &CypherParameters) -> Option<bool> {
    match expr {
        Expr::Boolean(b) => Some(*b),
        Expr::Parameter(name) => match params.get(name)? {
            Value::Bool(b) => Some(*b),
            _ => None,
        },
        _ => None,
    }
}

fn lower_cmp_op(op: BinaryOp) -> Option<CmpOp> {
    match op {
        BinaryOp::Eq => Some(CmpOp::Eq),
        BinaryOp::Ne => Some(CmpOp::Ne),
        BinaryOp::Lt => Some(CmpOp::Lt),
        BinaryOp::Le => Some(CmpOp::Le),
        BinaryOp::Gt => Some(CmpOp::Gt),
        BinaryOp::Ge => Some(CmpOp::Ge),
        _ => None,
    }
}

/// Lower `var.key` (anchored on the scan variable) to a [`PropRef`].
fn lower_prop_ref(expr: &Expr, var: &str) -> Option<PropRef> {
    match expr {
        Expr::Property { base, key } => match base.as_ref() {
            Expr::Variable(name) if name == var => lower_prop_key(key),
            _ => None,
        },
        _ => None,
    }
}

fn lower_prop_key(key: &str) -> Option<PropRef> {
    if key == "label" {
        return Some(PropRef::Label);
    }
    // Restrict JSON-path keys to safe identifiers so the inlined `$.key` path
    // cannot inject. Reference property keys are identifiers in practice.
    if is_safe_key(key) {
        Some(PropRef::Key(key.to_string()))
    } else {
        None
    }
}

fn is_safe_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Lower a literal/parameter expression to a [`Scalar`], or `None` if it is not
/// a milestone-1 scalar (int/float/string).
fn lower_scalar(expr: &Expr, params: &CypherParameters) -> Option<Scalar> {
    match expr {
        Expr::Integer(n) => Some(Scalar::Int(*n)),
        Expr::Float(f) if f.is_finite() => Some(Scalar::Float(*f)),
        Expr::String(s) => Some(Scalar::Str(s.clone())),
        Expr::Parameter(name) => scalar_from_value(params.get(name)?),
        Expr::Unary {
            op: UnaryOp::Negate,
            operand,
        } => match lower_scalar(operand, params)? {
            Scalar::Int(n) => Some(Scalar::Int(-n)),
            Scalar::Float(f) => Some(Scalar::Float(-f)),
            Scalar::Str(_) => None,
        },
        _ => None,
    }
}

fn scalar_from_value(value: &Value) -> Option<Scalar> {
    match value {
        Value::Int(n) => Some(Scalar::Int(*n)),
        Value::Float(f) if f.is_finite() => Some(Scalar::Float(*f)),
        Value::String(s) => Some(Scalar::Str(s.clone())),
        _ => None,
    }
}

/// Lower a `STARTS/ENDS/CONTAINS` right-hand side to a **non-empty** string
/// needle (literal or a parameter bound to a non-empty string), else `None`.
/// Empty needles are excluded to avoid dialect edge cases.
fn lower_string_needle(expr: &Expr, params: &CypherParameters) -> Option<String> {
    match lower_scalar(expr, params)? {
        Scalar::Str(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}

/// The [`StrOp`] for a `STARTS/ENDS/CONTAINS` binary expression.
fn lower_str_op(expr: &Expr) -> StrOp {
    match expr {
        Expr::Binary {
            op: BinaryOp::StartsWith,
            ..
        } => StrOp::StartsWith,
        Expr::Binary {
            op: BinaryOp::EndsWith,
            ..
        } => StrOp::EndsWith,
        _ => StrOp::Contains,
    }
}

// ---------------------------------------------------------------------------
// SQL rendering + projection
// ---------------------------------------------------------------------------

impl NodeReadPushdown {
    /// The scan variable.
    pub fn variable(&self) -> &str {
        &self.var
    }

    /// Render the scan + filter as `SELECT id, label, props FROM <nodes> [WHERE …]`.
    ///
    /// The fixed `id, label, props` projection lets the backend reconstruct the
    /// surviving [`Node`]s and hand them to [`Self::project`]; the `RETURN`
    /// projection itself is *not* pushed in milestone 1 (it runs in Rust against
    /// the shared reference, guaranteeing identical output).
    pub fn to_sql(&self, dialect: &dyn SqlDialect) -> String {
        let table = dialect.quote_ident(dialect.nodes_table());
        let mut sql = format!("SELECT id, label, props FROM {table}");
        let mut conditions: Vec<String> = Vec::new();
        if let Some(label) = &self.label {
            conditions.push(format!("label = {}", dialect.string_literal(label)));
        }
        if let Some(filter) = &self.filter {
            conditions.push(render_predicate(filter, dialect));
        }
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        if self.pushes_ordering(dialect) {
            sql.push_str(&render_order_limit(self.ordering.as_ref().unwrap(), dialect));
        }
        sql
    }

    /// Whether `to_sql` pushes `ORDER BY`/`SKIP`/`LIMIT` for this dialect (so the
    /// backend must NOT re-apply them in the projection).
    ///
    /// Typed-JSON dialects can always push a structurally-pushable ordering.
    /// Untyped-JSON dialects (Spark) can push only when every sort key's type is
    /// known (resolved from `TypeHints` at plan time), so numeric keys can be
    /// cast; otherwise ordering stays in the reference projection.
    pub fn pushes_ordering(&self, dialect: &dyn SqlDialect) -> bool {
        match &self.ordering {
            None => false,
            Some(ordering) => {
                dialect.orders_json_typed() || ordering.keys.iter().all(|k| k.kind.is_some())
            }
        }
    }

    /// Run the `RETURN` projection over the nodes the backend fetched, producing
    /// the same [`CypherResultTable`] the in-memory reference would. When the
    /// dialect pushed `ORDER BY`/`SKIP`/`LIMIT`, those are dropped from the
    /// in-Rust projection (the SQL already applied them).
    pub fn project(
        &self,
        dialect: &dyn SqlDialect,
        nodes: Vec<Node>,
        params: &CypherParameters,
    ) -> Result<CypherResultTable> {
        if self.pushes_ordering(dialect) {
            let projection = strip_order_limit(&self.projection);
            crate::read::project_nodes(&self.var, nodes, &projection, params)
        } else {
            crate::read::project_nodes(&self.var, nodes, &self.projection, params)
        }
    }

    /// The number of text columns `to_sql` emits (`id, label, props`).
    pub fn column_count(&self) -> usize {
        3
    }

    /// Reconstruct the scanned nodes from the backend's text rows and project —
    /// the uniform text-rows counterpart of [`Self::project`].
    pub fn project_text_rows(
        &self,
        dialect: &dyn SqlDialect,
        rows: Vec<Vec<Option<String>>>,
        params: &CypherParameters,
    ) -> Result<CypherResultTable> {
        let selected = [SelectedBinding::Node {
            var: self.var.clone(),
            node: 0,
            optional: false,
        }];
        let binding_rows = reconstruct_bindings(&selected, rows)?;
        if self.pushes_ordering(dialect) {
            let projection = strip_order_limit(&self.projection);
            crate::read::project_bindings(binding_rows, &projection, params)
        } else {
            crate::read::project_bindings(binding_rows, &self.projection, params)
        }
    }
}

/// Render the trailing ` ORDER BY … [LIMIT …] [OFFSET …]` for a pushed ordering.
/// `NULLS LAST` (asc) / `NULLS FIRST` (desc) match the reference, where NULL
/// sorts as the maximum value.
fn render_order_limit(ordering: &PushedOrdering, dialect: &dyn SqlDialect) -> String {
    let mut sql = String::new();
    let keys = ordering
        .keys
        .iter()
        .map(|k| {
            let col = render_prop(&k.prop, dialect);
            // Typed-JSON dialects sort the extracted value directly; untyped ones
            // cast numeric keys to their known type (label/strings sort as text).
            let col = if dialect.orders_json_typed() {
                col
            } else {
                match k.kind {
                    Some(ScalarKind::Int) => dialect.cast_int(&col),
                    Some(ScalarKind::Float) => dialect.cast_float(&col),
                    _ => col,
                }
            };
            if k.descending {
                format!("{col} DESC NULLS FIRST")
            } else {
                format!("{col} ASC NULLS LAST")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    sql.push_str(" ORDER BY ");
    sql.push_str(&keys);
    match (ordering.limit, ordering.skip) {
        (Some(n), Some(s)) => sql.push_str(&format!(" LIMIT {n} OFFSET {s}")),
        (Some(n), None) => sql.push_str(&format!(" LIMIT {n}")),
        // SKIP without LIMIT: `LIMIT -1 OFFSET s` returns all rows after the skip.
        (None, Some(s)) => sql.push_str(&format!(" LIMIT -1 OFFSET {s}")),
        (None, None) => {}
    }
    sql
}

/// A copy of `projection` with `ORDER BY`/`SKIP`/`LIMIT` cleared (used when the
/// backend pushed them into SQL).
fn strip_order_limit(projection: &Projection) -> Projection {
    let mut projection = projection.clone();
    projection.order_by.clear();
    projection.skip = None;
    projection.limit = None;
    projection
}

fn render_predicate(pred: &Predicate, dialect: &dyn SqlDialect) -> String {
    match pred {
        Predicate::And(lhs, rhs) => format!(
            "({} AND {})",
            render_predicate(lhs, dialect),
            render_predicate(rhs, dialect)
        ),
        Predicate::Or(lhs, rhs) => format!(
            "({} OR {})",
            render_predicate(lhs, dialect),
            render_predicate(rhs, dialect)
        ),
        Predicate::Not(inner) => format!("(NOT {})", render_predicate(inner, dialect)),
        Predicate::Compare { prop, op, value } => render_compare(prop, *op, value, dialect),
        Predicate::IsNull { prop, negated } => {
            let p = render_prop(prop, dialect);
            if *negated {
                format!("{p} IS NOT NULL")
            } else {
                format!("{p} IS NULL")
            }
        }
        Predicate::In { prop, values } => {
            let raw = render_prop(prop, dialect);
            render_in_list(raw, matches!(prop, PropRef::Label), values, dialect)
        }
        Predicate::StringPred { prop, op, needle } => {
            dialect.string_predicate(&render_prop(prop, dialect), *op, needle)
        }
        Predicate::BoolCompare { prop, op, value } => {
            format!(
                "{} {} {}",
                render_prop(prop, dialect),
                op.sql(),
                dialect.bool_literal_sql(*value)
            )
        }
        Predicate::ArithCompare { lhs, op, rhs } => {
            format!(
                "{} {} {}",
                render_arith(lhs, dialect),
                op.sql(),
                render_arith(rhs, dialect)
            )
        }
    }
}

/// Render an arithmetic operand. Properties are cast to their known numeric type
/// (so untyped-JSON dialects compute numerically, not lexically); the cast is
/// harmless on typed-JSON dialects.
fn render_arith(expr: &ArithExpr, dialect: &dyn SqlDialect) -> String {
    match expr {
        ArithExpr::Prop(prop, ScalarKind::Int) => dialect.cast_int(&render_prop(prop, dialect)),
        ArithExpr::Prop(prop, _) => dialect.cast_float(&render_prop(prop, dialect)),
        ArithExpr::Int(n) => n.to_string(),
        ArithExpr::Float(f) => render_float(*f),
        ArithExpr::Bin(ArithOp::Div, l, r) => {
            // Force floating-point division (the reference's `/` is always f64).
            format!(
                "({} / {})",
                dialect.cast_float(&render_arith(l, dialect)),
                dialect.cast_float(&render_arith(r, dialect))
            )
        }
        ArithExpr::Bin(op, l, r) => {
            format!("({} {} {})", render_arith(l, dialect), op.sql(), render_arith(r, dialect))
        }
    }
}

/// Render `prop` as a SQL scalar: the `label` column, or a JSON property.
fn render_prop(prop: &PropRef, dialect: &dyn SqlDialect) -> String {
    match prop {
        PropRef::Label => "label".to_string(),
        PropRef::Key(key) => dialect.json_property("props", key),
    }
}

fn render_compare(prop: &PropRef, op: CmpOp, value: &Scalar, dialect: &dyn SqlDialect) -> String {
    let raw = render_prop(prop, dialect);
    // Cast the (string-typed for some dialects) JSON scalar to match the
    // literal's type, mirroring the reference's numeric/string comparison. The
    // `label` column is already text, so it is compared without a cast.
    let (lhs, rhs) = match value {
        Scalar::Int(n) => {
            let lhs = match prop {
                PropRef::Label => raw,
                PropRef::Key(_) => dialect.cast_int(&raw),
            };
            (lhs, n.to_string())
        }
        Scalar::Float(f) => {
            let lhs = match prop {
                PropRef::Label => raw,
                PropRef::Key(_) => dialect.cast_float(&raw),
            };
            (lhs, render_float(*f))
        }
        Scalar::Str(s) => (raw, dialect.string_literal(s)),
    };
    format!("{lhs} {} {rhs}", op.sql())
}

/// Render a finite f64 as a SQL numeric literal that always carries a decimal
/// point (so it is parsed as floating point, not integer).
fn render_float(f: f64) -> String {
    let s = format!("{f}");
    if s.contains(['.', 'e', 'E', 'n', 'N']) {
        s
    } else {
        format!("{s}.0")
    }
}

// ===========================================================================
// Relationship-segment pushdown (milestone 2: one directed segment)
// ===========================================================================
//
// `MATCH (a[:LA] [{..}])-[r?:T [{..}]]->(b[:LB] [{..}]) [WHERE pred] RETURN …`
// (and the `<-[..]-` incoming form) lowers to a join of `grust_edges` against
// `grust_nodes` twice. The backend executes the SQL and returns the selected
// columns as text rows; [`SegmentReadPushdown::project_text_rows`] reconstructs
// the `(a, r, b)` bindings (parsing the JSON `props` columns) and runs the
// shared reference projection — so the result is identical to
// [`crate::read::run_read_query`] by construction.

use crate::read::PushedBinding;
use grust_core::{Edge, EdgeId, Label, NodeId, Props};

/// The role a pattern variable plays: a node at position `i`, or an edge at
/// segment `j`. Used to resolve `WHERE`/`ORDER BY` operands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Node(usize),
    Edge(usize),
}

/// An operand of a path filter: a node label/property at position `i`, or an edge
/// property at segment `j`.
#[derive(Clone, Debug, PartialEq)]
enum SegOperand {
    NodeLabel(usize),
    NodeProp(usize, String),
    EdgeProp(usize, String),
}

/// A pushable segment filter predicate.
#[derive(Clone, Debug, PartialEq)]
enum SegPredicate {
    And(Box<SegPredicate>, Box<SegPredicate>),
    Or(Box<SegPredicate>, Box<SegPredicate>),
    Not(Box<SegPredicate>),
    Compare {
        operand: SegOperand,
        op: CmpOp,
        value: Scalar,
    },
    IsNull {
        operand: SegOperand,
        negated: bool,
    },
    In {
        operand: SegOperand,
        values: Vec<Scalar>,
    },
    StringPred {
        operand: SegOperand,
        op: StrOp,
        needle: String,
    },
    BoolCompare {
        operand: SegOperand,
        op: CmpOp,
        value: bool,
    },
    ArithCompare {
        lhs: SegArithExpr,
        op: CmpOp,
        rhs: SegArithExpr,
    },
}

/// Segment-path arithmetic operand (mirrors [`ArithExpr`] with [`SegOperand`]s).
#[derive(Clone, Debug, PartialEq)]
enum SegArithExpr {
    Operand(SegOperand, ScalarKind),
    Int(i64),
    Float(f64),
    Bin(ArithOp, Box<SegArithExpr>, Box<SegArithExpr>),
}

/// One binding the path query selects and reconstructs, in SELECT-column order.
/// `optional` bindings come from an `OPTIONAL MATCH` (a `LEFT JOIN`): when their
/// presence column is NULL the binding is reconstructed as `null` (the
/// reference's null-padding) rather than a graph element.
#[derive(Clone, Debug, PartialEq)]
enum SelectedBinding {
    /// Node at position `node` bound to `var` (3 columns: id, label, props).
    Node {
        var: String,
        node: usize,
        optional: bool,
    },
    /// Edge at segment `edge` bound to `var` (5 columns: id, src, dst, type, props).
    Edge {
        var: String,
        edge: usize,
        optional: bool,
    },
}

/// One segment of the path: its direction and relationship types.
#[derive(Clone, Debug, PartialEq)]
struct SegSpec {
    direction: SegDirection,
    rel_types: Vec<String>,
}

/// The traversal direction of a lowered relationship segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SegDirection {
    /// `(a)-[..]->(b)`: a = src, b = dst.
    Outgoing,
    /// `(a)<-[..]-(b)`: a = dst, b = src.
    Incoming,
    /// `(a)-[..]-(b)`: either orientation (both appear, like the reference).
    Undirected,
}

/// A lowered relationship path of one or more fixed-length segments.
#[derive(Clone, Debug, PartialEq)]
pub struct SegmentReadPushdown {
    /// Label per node position (`n0..=nK`); `None` for an unlabeled node.
    node_labels: Vec<Option<String>>,
    /// One spec per segment (`e0..e{K-1}`).
    segments: Vec<SegSpec>,
    filter: Option<SegPredicate>,
    /// Selected bindings in SELECT order (only pattern vars that are bound).
    selected: Vec<SelectedBinding>,
    /// `ORDER BY`/`SKIP`/`LIMIT` lowered for SQL pushdown, when structurally
    /// pushable (same rules as the node path; keys reference any path variable).
    ordering: Option<SegPushedOrdering>,
    projection: Projection,
}

/// Segment `ORDER BY`/`SKIP`/`LIMIT` resolved for SQL pushdown.
#[derive(Clone, Debug, PartialEq)]
struct SegPushedOrdering {
    keys: Vec<SegOrderKey>,
    skip: Option<usize>,
    limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
struct SegOrderKey {
    operand: SegOperand,
    descending: bool,
    /// Scalar kind for casting on untyped-JSON dialects (`None` = unknown →
    /// not pushable there). Edge properties are left `None` for now.
    kind: Option<ScalarKind>,
}

/// Try to lower `cypher` into a relationship-segment read pushdown.
///
/// Same contract as [`plan_node_read`]: `Ok(Some(_))` for the pushable subset,
/// `Ok(None)` for a valid query outside it, `Err` only for invalid input.
pub fn plan_segment_read(
    cypher: &str,
    params: &CypherParameters,
) -> Result<Option<SegmentReadPushdown>> {
    plan_segment_read_with_hints(cypher, params, &NoTypeHints)
}

/// Like [`plan_segment_read`], but resolves `ORDER BY` key types via `hints` so an
/// untyped-JSON dialect (e.g. Spark) can push numeric ordering by casting.
pub fn plan_segment_read_with_hints(
    cypher: &str,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Result<Option<SegmentReadPushdown>> {
    let query = parse_query(cypher).map_err(|e| e.into_grust(cypher))?;
    crate::semantics::analyze(&query)?;
    Ok(single_query(&query).and_then(|s| lower_segment_single(s, params, hints)))
}

fn lower_segment_single(
    single: &SingleQuery,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Option<SegmentReadPushdown> {
    let (match_clause, return_clause) = match single.clauses.as_slice() {
        [Clause::Match(m), Clause::Return(r)] if !m.optional => (m, r),
        _ => return None,
    };
    if match_clause.patterns.len() != 1 {
        return None;
    }
    let pattern = &match_clause.patterns[0];
    // No path variable; at least one segment (node-only is handled by the node
    // planner).
    if pattern.variable.is_some() || pattern.segments.is_empty() {
        return None;
    }
    let k = pattern.segments.len();

    // Node patterns by position: [start, seg0.node, seg1.node, …]; ≤1 label each.
    let mut node_patterns: Vec<&NodePattern> = Vec::with_capacity(k + 1);
    node_patterns.push(&pattern.start);
    for seg in &pattern.segments {
        node_patterns.push(&seg.node);
    }
    let mut node_labels = Vec::with_capacity(k + 1);
    for np in &node_patterns {
        if np.labels.len() > 1 {
            return None;
        }
        node_labels.push(np.labels.first().cloned());
    }

    // Segment specs; no variable-length bounds.
    let mut segments = Vec::with_capacity(k);
    for seg in &pattern.segments {
        let rel = &seg.relationship;
        if rel.length.is_some() {
            return None;
        }
        let direction = match rel.direction {
            Direction::Outgoing => SegDirection::Outgoing,
            Direction::Incoming => SegDirection::Incoming,
            Direction::Undirected => SegDirection::Undirected,
        };
        segments.push(SegSpec {
            direction,
            rel_types: rel.types.clone(),
        });
    }

    // Variable -> role. A repeated variable (any position) is rejected so the
    // join stays unambiguous; the reference would equate them, we fall back.
    let mut roles: std::collections::HashMap<String, Role> = std::collections::HashMap::new();
    for (i, np) in node_patterns.iter().enumerate() {
        if let Some(v) = &np.variable {
            if roles.insert(v.clone(), Role::Node(i)).is_some() {
                return None;
            }
        }
    }
    for (j, seg) in pattern.segments.iter().enumerate() {
        if let Some(v) = &seg.relationship.variable {
            if roles.insert(v.clone(), Role::Edge(j)).is_some() {
                return None;
            }
        }
    }

    // Filter: inline node props, inline edge props, then WHERE.
    let mut filter: Option<SegPredicate> = None;
    for (i, np) in node_patterns.iter().enumerate() {
        if let Some(map) = &np.properties {
            for (key, value) in &map.entries {
                let scalar = lower_scalar(value, params)?;
                let operand = if key == "label" {
                    SegOperand::NodeLabel(i)
                } else {
                    SegOperand::NodeProp(i, lower_seg_key(key)?)
                };
                filter = Some(seg_conjoin(
                    filter,
                    SegPredicate::Compare {
                        operand,
                        op: CmpOp::Eq,
                        value: scalar,
                    },
                ));
            }
        }
    }
    for (j, seg) in pattern.segments.iter().enumerate() {
        if let Some(map) = &seg.relationship.properties {
            for (key, value) in &map.entries {
                let scalar = lower_scalar(value, params)?;
                filter = Some(seg_conjoin(
                    filter,
                    SegPredicate::Compare {
                        operand: SegOperand::EdgeProp(j, lower_seg_key(key)?),
                        op: CmpOp::Eq,
                        value: scalar,
                    },
                ));
            }
        }
    }
    if let Some(where_expr) = &match_clause.where_clause {
        let ctx = SegCtx {
            roles: &roles,
            node_labels: &node_labels,
            segments: &segments,
            hints,
        };
        filter = Some(seg_conjoin(filter, lower_seg_predicate(where_expr, &ctx, params)?));
    }

    // Selected bindings in SELECT order: n0, e0, n1, e1, …, nK (vars only).
    let mut selected = Vec::new();
    if let Some(var) = &node_patterns[0].variable {
        selected.push(SelectedBinding::Node {
            var: var.clone(),
            node: 0,
            optional: false,
        });
    }
    for (j, seg) in pattern.segments.iter().enumerate() {
        if let Some(var) = &seg.relationship.variable {
            selected.push(SelectedBinding::Edge {
                var: var.clone(),
                edge: j,
                optional: false,
            });
        }
        if let Some(var) = &node_patterns[j + 1].variable {
            selected.push(SelectedBinding::Node {
                var: var.clone(),
                node: j + 1,
                optional: false,
            });
        }
    }

    let projection = return_clause.projection.clone();
    let ordering = compute_seg_ordering(&projection, &roles, &node_labels, &segments, params, hints);

    Some(SegmentReadPushdown {
        node_labels,
        segments,
        filter,
        selected,
        ordering,
        projection,
    })
}

/// Resolve segment `ORDER BY`/`SKIP`/`LIMIT` for SQL pushdown, or `None` if not
/// structurally pushable. Mirrors `compute_pushed_ordering` but resolves sort
/// keys against the `a`/`r`/`b` roles; node-property kinds come from `hints`
/// keyed by the relevant endpoint label (edge properties are left unknown).
fn compute_seg_ordering(
    projection: &Projection,
    roles: &std::collections::HashMap<String, Role>,
    node_labels: &[Option<String>],
    segments: &[SegSpec],
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Option<SegPushedOrdering> {
    if projection.distinct {
        return None;
    }
    if projection.items.iter().any(|i| crate::read::expr_has_aggregate(&i.expr)) {
        return None;
    }
    if projection.order_by.is_empty() {
        return None;
    }
    let aliases: std::collections::HashMap<&str, &Expr> = projection
        .items
        .iter()
        .filter_map(|i| i.alias.as_deref().map(|a| (a, &i.expr)))
        .collect();

    let mut keys = Vec::with_capacity(projection.order_by.len());
    for item in &projection.order_by {
        let resolved = match &item.expr {
            Expr::Variable(name) => aliases.get(name.as_str()).copied().unwrap_or(&item.expr),
            other => other,
        };
        let operand = lower_seg_operand(resolved, roles)?;
        let kind = match &operand {
            SegOperand::NodeLabel(_) => Some(ScalarKind::Str),
            SegOperand::NodeProp(i, key) => {
                hints.node_property_kind(node_labels[*i].as_deref(), key)
            }
            SegOperand::EdgeProp(j, key) => {
                // A single relationship type lets the edge property be typed.
                let edge_type = match segments[*j].rel_types.as_slice() {
                    [one] => Some(one.as_str()),
                    _ => None,
                };
                hints.edge_property_kind(edge_type, key)
            }
        };
        keys.push(SegOrderKey {
            operand,
            descending: item.descending,
            kind,
        });
    }
    let skip = match &projection.skip {
        None => None,
        Some(e) => Some(lower_usize(e, params)?),
    };
    let limit = match &projection.limit {
        None => None,
        Some(e) => Some(lower_usize(e, params)?),
    };
    Some(SegPushedOrdering { keys, skip, limit })
}

/// A map from variable name to its path role, for WHERE/ORDER lowering.
type RoleMap = std::collections::HashMap<String, Role>;

fn seg_conjoin(existing: Option<SegPredicate>, next: SegPredicate) -> SegPredicate {
    match existing {
        None => next,
        Some(prev) => SegPredicate::And(Box::new(prev), Box::new(next)),
    }
}

fn lower_seg_key(key: &str) -> Option<String> {
    if is_safe_key(key) {
        Some(key.to_string())
    } else {
        None
    }
}

/// Context for lowering a segment `WHERE` clause: variable roles plus the type
/// information arithmetic operands need.
struct SegCtx<'a> {
    roles: &'a RoleMap,
    node_labels: &'a [Option<String>],
    segments: &'a [SegSpec],
    hints: &'a dyn TypeHints,
}

fn lower_seg_predicate(
    expr: &Expr,
    ctx: &SegCtx,
    params: &CypherParameters,
) -> Option<SegPredicate> {
    match expr {
        Expr::Binary {
            op: BinaryOp::And,
            lhs,
            rhs,
        } => Some(SegPredicate::And(
            Box::new(lower_seg_predicate(lhs, ctx, params)?),
            Box::new(lower_seg_predicate(rhs, ctx, params)?),
        )),
        Expr::Binary {
            op: BinaryOp::Or,
            lhs,
            rhs,
        } => Some(SegPredicate::Or(
            Box::new(lower_seg_predicate(lhs, ctx, params)?),
            Box::new(lower_seg_predicate(rhs, ctx, params)?),
        )),
        Expr::Unary {
            op: UnaryOp::Not,
            operand,
        } => Some(SegPredicate::Not(Box::new(lower_seg_predicate(
            operand, ctx, params,
        )?))),
        Expr::IsNull { operand, negated } => Some(SegPredicate::IsNull {
            operand: lower_seg_operand(operand, ctx.roles)?,
            negated: *negated,
        }),
        Expr::Binary {
            op: BinaryOp::In,
            lhs,
            rhs,
        } => Some(SegPredicate::In {
            operand: lower_seg_operand(lhs, ctx.roles)?,
            values: lower_scalar_list(rhs, params)?,
        }),
        Expr::Binary {
            op: BinaryOp::StartsWith | BinaryOp::EndsWith | BinaryOp::Contains,
            lhs,
            rhs,
        } => Some(SegPredicate::StringPred {
            operand: lower_seg_operand(lhs, ctx.roles)?,
            op: lower_str_op(expr),
            needle: lower_string_needle(rhs, params)?,
        }),
        Expr::Binary { op, lhs, rhs } => {
            let cmp = lower_cmp_op(*op)?;
            if matches!(cmp, CmpOp::Eq | CmpOp::Ne) {
                if let (Some(operand), Some(value)) =
                    (lower_seg_operand(lhs, ctx.roles), lower_bool(rhs, params))
                {
                    return Some(SegPredicate::BoolCompare { operand, op: cmp, value });
                }
                if let (Some(operand), Some(value)) =
                    (lower_seg_operand(rhs, ctx.roles), lower_bool(lhs, params))
                {
                    return Some(SegPredicate::BoolCompare { operand, op: cmp, value });
                }
            }
            if let Some(operand) = lower_seg_operand(lhs, ctx.roles) {
                if let Some(value) = lower_scalar(rhs, params) {
                    return Some(SegPredicate::Compare { operand, op: cmp, value });
                }
            }
            if let Some(operand) = lower_seg_operand(rhs, ctx.roles) {
                if let Some(value) = lower_scalar(lhs, params) {
                    return Some(SegPredicate::Compare {
                        operand,
                        op: cmp.flipped(),
                        value,
                    });
                }
            }
            // Arithmetic comparison (`+`/`-`/`*` over typed numeric properties).
            let l = lower_seg_arith(lhs, ctx, params)?;
            let r = lower_seg_arith(rhs, ctx, params)?;
            if seg_arith_has_operand(&l) || seg_arith_has_operand(&r) {
                Some(SegPredicate::ArithCompare { lhs: l, op: cmp, rhs: r })
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Lower a segment-path arithmetic operand. See [`lower_arith`].
fn lower_seg_arith(expr: &Expr, ctx: &SegCtx, params: &CypherParameters) -> Option<SegArithExpr> {
    match expr {
        Expr::Integer(n) => Some(SegArithExpr::Int(*n)),
        Expr::Float(f) if f.is_finite() => Some(SegArithExpr::Float(*f)),
        Expr::Parameter(name) => match params.get(name)? {
            Value::Int(n) => Some(SegArithExpr::Int(*n)),
            Value::Float(f) if f.is_finite() => Some(SegArithExpr::Float(*f)),
            _ => None,
        },
        Expr::Property { .. } => {
            let operand = lower_seg_operand(expr, ctx.roles)?;
            let kind = match &operand {
                SegOperand::NodeLabel(_) => return None,
                SegOperand::NodeProp(i, key) => {
                    ctx.hints.node_property_kind(ctx.node_labels[*i].as_deref(), key)?
                }
                SegOperand::EdgeProp(j, key) => {
                    let edge_type = match ctx.segments[*j].rel_types.as_slice() {
                        [one] => Some(one.as_str()),
                        _ => None,
                    };
                    ctx.hints.edge_property_kind(edge_type, key)?
                }
            };
            match kind {
                ScalarKind::Int | ScalarKind::Float => Some(SegArithExpr::Operand(operand, kind)),
                ScalarKind::Str => None,
            }
        }
        Expr::Binary {
            op: op @ (BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide),
            lhs,
            rhs,
        } => {
            let op = match op {
                BinaryOp::Add => ArithOp::Add,
                BinaryOp::Subtract => ArithOp::Sub,
                BinaryOp::Multiply => ArithOp::Mul,
                _ => ArithOp::Div,
            };
            Some(SegArithExpr::Bin(
                op,
                Box::new(lower_seg_arith(lhs, ctx, params)?),
                Box::new(lower_seg_arith(rhs, ctx, params)?),
            ))
        }
        _ => None,
    }
}

fn seg_arith_has_operand(expr: &SegArithExpr) -> bool {
    match expr {
        SegArithExpr::Operand(..) => true,
        SegArithExpr::Int(_) | SegArithExpr::Float(_) => false,
        SegArithExpr::Bin(_, l, r) => seg_arith_has_operand(l) || seg_arith_has_operand(r),
    }
}

fn lower_seg_operand(expr: &Expr, roles: &RoleMap) -> Option<SegOperand> {
    let Expr::Property { base, key } = expr else {
        return None;
    };
    let Expr::Variable(name) = base.as_ref() else {
        return None;
    };
    match roles.get(name.as_str())? {
        Role::Node(i) => Some(if key == "label" {
            SegOperand::NodeLabel(*i)
        } else {
            SegOperand::NodeProp(*i, lower_seg_key(key)?)
        }),
        // Edges have no synthetic `label`; `r.key` is always a property.
        Role::Edge(j) => Some(SegOperand::EdgeProp(*j, lower_seg_key(key)?)),
    }
}

impl SegmentReadPushdown {
    /// The `e{j}`/`n{j+1}` join clauses connecting node `j` to node `j+1` through
    /// segment `j`. Undirected matches either orientation, reproducing the
    /// reference's two-orientation behavior for non-self-loop edges.
    fn segment_join(j: usize, direction: SegDirection) -> (String, String) {
        let (nj, ej, nj1) = (format!("n{j}"), format!("e{j}"), format!("n{}", j + 1));
        match direction {
            SegDirection::Outgoing => (
                format!("{ej}.src_id = {nj}.id"),
                format!("{nj1}.id = {ej}.dst_id"),
            ),
            SegDirection::Incoming => (
                format!("{ej}.dst_id = {nj}.id"),
                format!("{nj1}.id = {ej}.src_id"),
            ),
            SegDirection::Undirected => (
                format!("({ej}.src_id = {nj}.id OR {ej}.dst_id = {nj}.id)"),
                format!(
                    "(({ej}.src_id = {nj}.id AND {nj1}.id = {ej}.dst_id) \
                     OR ({ej}.dst_id = {nj}.id AND {nj1}.id = {ej}.src_id))"
                ),
            ),
        }
    }

    /// Render the path join chain + filter. The SELECT list emits, in order, the
    /// columns for each selected binding (node: id,label,props; edge:
    /// id,src_id,dst_id,edge_type,props) — all text columns, reconstructed by
    /// [`Self::project_text_rows`].
    pub fn to_sql(&self, dialect: &dyn SqlDialect) -> String {
        let nodes = dialect.quote_ident(dialect.nodes_table());
        let edges = dialect.quote_ident(dialect.edges_table());

        let mut cols: Vec<String> = Vec::new();
        for binding in &self.selected {
            match binding {
                SelectedBinding::Node { node, .. } => {
                    cols.push(format!("n{node}.id"));
                    cols.push(format!("n{node}.label"));
                    cols.push(format!("n{node}.props"));
                }
                SelectedBinding::Edge { edge, .. } => {
                    cols.push(format!("e{edge}.id"));
                    cols.push(format!("e{edge}.src_id"));
                    cols.push(format!("e{edge}.dst_id"));
                    cols.push(format!("e{edge}.edge_type"));
                    cols.push(format!("e{edge}.props"));
                }
            }
        }
        // No selected binding (e.g. RETURN count(*)): still need one column so the
        // backend gets one row per match.
        let select_list = if cols.is_empty() {
            "1".to_string()
        } else {
            cols.join(", ")
        };

        // FROM n0, then chain: JOIN e{j} JOIN n{j+1} per segment.
        let mut sql = format!("SELECT {select_list} FROM {nodes} n0");
        for (j, seg) in self.segments.iter().enumerate() {
            let (edge_on, node_on) = Self::segment_join(j, seg.direction);
            sql.push_str(&format!(
                " JOIN {edges} e{j} ON {edge_on} JOIN {nodes} n{} ON {node_on}",
                j + 1
            ));
        }

        let mut conditions: Vec<String> = Vec::new();
        for (j, seg) in self.segments.iter().enumerate() {
            match seg.rel_types.as_slice() {
                [] => {}
                [one] => {
                    conditions.push(format!("e{j}.edge_type = {}", dialect.string_literal(one)))
                }
                many => {
                    let list = many
                        .iter()
                        .map(|t| dialect.string_literal(t))
                        .collect::<Vec<_>>()
                        .join(", ");
                    conditions.push(format!("e{j}.edge_type IN ({list})"));
                }
            }
        }
        for (i, label) in self.node_labels.iter().enumerate() {
            if let Some(label) = label {
                conditions.push(format!("n{i}.label = {}", dialect.string_literal(label)));
            }
        }
        if let Some(filter) = &self.filter {
            conditions.push(render_seg_predicate(filter, dialect));
        }
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        if self.pushes_ordering(dialect) {
            sql.push_str(&self.render_order_limit(dialect));
        }
        sql
    }

    /// Whether `to_sql` pushes `ORDER BY`/`SKIP`/`LIMIT` for this dialect (so the
    /// backend must NOT re-apply them in the projection). See
    /// [`NodeReadPushdown::pushes_ordering`].
    pub fn pushes_ordering(&self, dialect: &dyn SqlDialect) -> bool {
        match &self.ordering {
            None => false,
            Some(ordering) => {
                dialect.orders_json_typed() || ordering.keys.iter().all(|k| k.kind.is_some())
            }
        }
    }

    fn render_order_limit(&self, dialect: &dyn SqlDialect) -> String {
        render_seg_order_limit(self.ordering.as_ref().expect("ordering present"), dialect)
    }

    /// Number of text columns the SQL emits (for the backend to read row cells).
    pub fn column_count(&self) -> usize {
        self.selected
            .iter()
            .map(|b| match b {
                SelectedBinding::Node { .. } => 3,
                SelectedBinding::Edge { .. } => 5,
            })
            .sum::<usize>()
            .max(1)
    }

    /// Reconstruct the `(a, r, b)` bindings from the backend's text rows and run
    /// the shared reference projection. When the dialect pushed
    /// `ORDER BY`/`SKIP`/`LIMIT`, those are dropped from the in-Rust projection.
    pub fn project_text_rows(
        &self,
        dialect: &dyn SqlDialect,
        rows: Vec<Vec<Option<String>>>,
        params: &CypherParameters,
    ) -> Result<CypherResultTable> {
        let binding_rows = reconstruct_bindings(&self.selected, rows)?;
        if self.pushes_ordering(dialect) {
            let projection = strip_order_limit(&self.projection);
            crate::read::project_bindings(binding_rows, &projection, params)
        } else {
            crate::read::project_bindings(binding_rows, &self.projection, params)
        }
    }
}

fn render_seg_predicate(pred: &SegPredicate, dialect: &dyn SqlDialect) -> String {
    match pred {
        SegPredicate::And(lhs, rhs) => format!(
            "({} AND {})",
            render_seg_predicate(lhs, dialect),
            render_seg_predicate(rhs, dialect)
        ),
        SegPredicate::Or(lhs, rhs) => format!(
            "({} OR {})",
            render_seg_predicate(lhs, dialect),
            render_seg_predicate(rhs, dialect)
        ),
        SegPredicate::Not(inner) => format!("(NOT {})", render_seg_predicate(inner, dialect)),
        SegPredicate::Compare { operand, op, value } => {
            render_seg_compare(operand, *op, value, dialect)
        }
        SegPredicate::IsNull { operand, negated } => {
            let p = render_seg_operand(operand, dialect);
            if *negated {
                format!("{p} IS NOT NULL")
            } else {
                format!("{p} IS NULL")
            }
        }
        SegPredicate::In { operand, values } => {
            let raw = render_seg_operand(operand, dialect);
            let is_label = matches!(operand, SegOperand::NodeLabel(_));
            render_in_list(raw, is_label, values, dialect)
        }
        SegPredicate::StringPred { operand, op, needle } => {
            dialect.string_predicate(&render_seg_operand(operand, dialect), *op, needle)
        }
        SegPredicate::BoolCompare { operand, op, value } => {
            format!(
                "{} {} {}",
                render_seg_operand(operand, dialect),
                op.sql(),
                dialect.bool_literal_sql(*value)
            )
        }
        SegPredicate::ArithCompare { lhs, op, rhs } => {
            format!(
                "{} {} {}",
                render_seg_arith(lhs, dialect),
                op.sql(),
                render_seg_arith(rhs, dialect)
            )
        }
    }
}

fn render_seg_arith(expr: &SegArithExpr, dialect: &dyn SqlDialect) -> String {
    match expr {
        SegArithExpr::Operand(operand, ScalarKind::Int) => {
            dialect.cast_int(&render_seg_operand(operand, dialect))
        }
        SegArithExpr::Operand(operand, _) => {
            dialect.cast_float(&render_seg_operand(operand, dialect))
        }
        SegArithExpr::Int(n) => n.to_string(),
        SegArithExpr::Float(f) => render_float(*f),
        SegArithExpr::Bin(ArithOp::Div, l, r) => format!(
            "({} / {})",
            dialect.cast_float(&render_seg_arith(l, dialect)),
            dialect.cast_float(&render_seg_arith(r, dialect))
        ),
        SegArithExpr::Bin(op, l, r) => format!(
            "({} {} {})",
            render_seg_arith(l, dialect),
            op.sql(),
            render_seg_arith(r, dialect)
        ),
    }
}

fn render_seg_operand(operand: &SegOperand, dialect: &dyn SqlDialect) -> String {
    match operand {
        SegOperand::NodeLabel(i) => format!("n{i}.label"),
        SegOperand::NodeProp(i, key) => dialect.json_property(&format!("n{i}.props"), key),
        SegOperand::EdgeProp(j, key) => dialect.json_property(&format!("e{j}.props"), key),
    }
}

fn render_seg_compare(
    operand: &SegOperand,
    op: CmpOp,
    value: &Scalar,
    dialect: &dyn SqlDialect,
) -> String {
    let raw = render_seg_operand(operand, dialect);
    let is_label = matches!(operand, SegOperand::NodeLabel(_));
    let (lhs, rhs) = match value {
        Scalar::Int(n) => {
            let lhs = if is_label { raw } else { dialect.cast_int(&raw) };
            (lhs, n.to_string())
        }
        Scalar::Float(f) => {
            let lhs = if is_label { raw } else { dialect.cast_float(&raw) };
            (lhs, render_float(*f))
        }
        Scalar::Str(s) => (raw, dialect.string_literal(s)),
    };
    format!("{lhs} {} {rhs}", op.sql())
}

/// Render ` ORDER BY … [LIMIT …] [OFFSET …]` for a path/var-length ordering,
/// casting numeric keys on untyped-JSON dialects. Shared by both planners.
fn render_seg_order_limit(ordering: &SegPushedOrdering, dialect: &dyn SqlDialect) -> String {
    let keys = ordering
        .keys
        .iter()
        .map(|k| {
            let col = render_seg_operand(&k.operand, dialect);
            let col = if dialect.orders_json_typed() {
                col
            } else {
                match k.kind {
                    Some(ScalarKind::Int) => dialect.cast_int(&col),
                    Some(ScalarKind::Float) => dialect.cast_float(&col),
                    _ => col,
                }
            };
            if k.descending {
                format!("{col} DESC NULLS FIRST")
            } else {
                format!("{col} ASC NULLS LAST")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut sql = format!(" ORDER BY {keys}");
    match (ordering.limit, ordering.skip) {
        (Some(n), Some(s)) => sql.push_str(&format!(" LIMIT {n} OFFSET {s}")),
        (Some(n), None) => sql.push_str(&format!(" LIMIT {n}")),
        (None, Some(s)) => sql.push_str(&format!(" LIMIT -1 OFFSET {s}")),
        (None, None) => {}
    }
    sql
}

/// Reconstruct binding rows from a backend's text rows, consuming columns per
/// selected binding (node: id,label,props; edge: id,src,dst,type,props).
fn reconstruct_bindings(
    selected: &[SelectedBinding],
    rows: Vec<Vec<Option<String>>>,
) -> Result<Vec<Vec<(String, PushedBinding)>>> {
    let mut binding_rows = Vec::with_capacity(rows.len());
    for cells in rows {
        let cell = |i: usize| cells.get(i).and_then(|c| c.as_deref());
        let mut idx = 0;
        let mut bindings = Vec::with_capacity(selected.len());
        for binding in selected {
            match binding {
                SelectedBinding::Node { var, optional, .. } => {
                    // Presence column for a node is its (non-null) id; an optional
                    // node with no LEFT JOIN match → null-padded binding.
                    if *optional && cell(idx).is_none() {
                        bindings.push((var.clone(), PushedBinding::Null));
                    } else {
                        bindings.push((
                            var.clone(),
                            PushedBinding::Node(Node {
                                id: NodeId::new(cell(idx).unwrap_or_default()),
                                label: Label::new(cell(idx + 1).unwrap_or_default()),
                                props: parse_props(cell(idx + 2))?,
                            }),
                        ));
                    }
                    idx += 3;
                }
                SelectedBinding::Edge { var, optional, .. } => {
                    // An edge's id may legitimately be NULL, so use src_id as the
                    // presence column for an optional (LEFT JOIN) edge.
                    if *optional && cell(idx + 1).is_none() {
                        bindings.push((var.clone(), PushedBinding::Null));
                    } else {
                        let mut edge = Edge::new(
                            cell(idx + 3).unwrap_or_default(),
                            cell(idx + 1).unwrap_or_default(),
                            cell(idx + 2).unwrap_or_default(),
                            parse_props(cell(idx + 4))?,
                        );
                        edge.id = cell(idx).map(EdgeId::new);
                        bindings.push((var.clone(), PushedBinding::Edge(edge)));
                    }
                    idx += 5;
                }
            }
        }
        binding_rows.push(bindings);
    }
    Ok(binding_rows)
}

// ===========================================================================
// Variable-length path pushdown (`(a)-[:T*m..n]->(b)`, anonymous relationship)
// ===========================================================================
//
// Lowers a single variable-length segment to a recursive CTE that enumerates
// **simple paths** (no repeated nodes, matching the reference) of length in
// `[min, max]`, then joins the start/end nodes for projection. The relationship
// must be anonymous (no edge-list binding) and there is no path variable. Node
// `a` is aliased `n0`, end node `b` is `n1`, so the segment filter/ordering
// machinery (operand indices 0 and 1) is reused.
//
// NOTE: verified for typed-JSON/SQLite via the differential oracle; the Spark
// rendering is golden-tested only and depends on recursive-CTE support.

/// A lowered single variable-length relationship segment.
#[derive(Clone, Debug, PartialEq)]
pub struct VarLengthReadPushdown {
    direction: SegDirection,
    rel_types: Vec<String>,
    /// Inclusive lower bound (default 1) and optional upper bound (open if None).
    min: u64,
    max: Option<u64>,
    a_label: Option<String>,
    b_label: Option<String>,
    filter: Option<SegPredicate>,
    /// Selected bindings (`a` at node 0, `b` at node 1) in SELECT order.
    selected: Vec<SelectedBinding>,
    ordering: Option<SegPushedOrdering>,
    projection: Projection,
}

/// Try to lower `cypher` into a variable-length path pushdown. Same contract as
/// the other planners.
pub fn plan_var_length_read(
    cypher: &str,
    params: &CypherParameters,
) -> Result<Option<VarLengthReadPushdown>> {
    plan_var_length_read_with_hints(cypher, params, &NoTypeHints)
}

/// Like [`plan_var_length_read`], with `ORDER BY` type hints for untyped dialects.
pub fn plan_var_length_read_with_hints(
    cypher: &str,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Result<Option<VarLengthReadPushdown>> {
    let query = parse_query(cypher).map_err(|e| e.into_grust(cypher))?;
    crate::semantics::analyze(&query)?;
    Ok(single_query(&query).and_then(|s| lower_var_length_single(s, params, hints)))
}

fn lower_var_length_single(
    single: &SingleQuery,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Option<VarLengthReadPushdown> {
    let (match_clause, return_clause) = match single.clauses.as_slice() {
        [Clause::Match(m), Clause::Return(r)] if !m.optional => (m, r),
        _ => return None,
    };
    if match_clause.patterns.len() != 1 {
        return None;
    }
    let pattern = &match_clause.patterns[0];
    // Exactly one segment, which must be variable-length, no path variable.
    if pattern.variable.is_some() || pattern.segments.len() != 1 {
        return None;
    }
    let a = &pattern.start;
    let segment = &pattern.segments[0];
    let rel = &segment.relationship;
    let b = &segment.node;
    let range = rel.length?; // must be variable-length
    // An edge-list binding (named relationship) is not reconstructed here.
    if rel.variable.is_some() {
        return None;
    }
    if a.labels.len() > 1 || b.labels.len() > 1 {
        return None;
    }
    let a_var = a.variable.clone();
    let b_var = b.variable.clone();
    if let (Some(av), Some(bv)) = (&a_var, &b_var) {
        if av == bv {
            return None;
        }
    }
    let direction = match rel.direction {
        Direction::Outgoing => SegDirection::Outgoing,
        Direction::Incoming => SegDirection::Incoming,
        Direction::Undirected => SegDirection::Undirected,
    };
    // Match the reference: min defaults to 1, max is open if unspecified.
    let min = range.min.unwrap_or(1);
    let max = range.max;

    // Roles: a -> node 0, b -> node 1 (reuse the segment operand machinery).
    let mut roles: RoleMap = std::collections::HashMap::new();
    if let Some(v) = &a_var {
        roles.insert(v.clone(), Role::Node(0));
    }
    if let Some(v) = &b_var {
        if roles.insert(v.clone(), Role::Node(1)).is_some() {
            return None;
        }
    }

    // Filter: inline props on a/b, then WHERE (over a/b only).
    let mut filter: Option<SegPredicate> = None;
    for (i, np) in [a, b].iter().enumerate() {
        if let Some(map) = &np.properties {
            for (key, value) in &map.entries {
                let scalar = lower_scalar(value, params)?;
                let operand = if key == "label" {
                    SegOperand::NodeLabel(i)
                } else {
                    SegOperand::NodeProp(i, lower_seg_key(key)?)
                };
                filter = Some(seg_conjoin(
                    filter,
                    SegPredicate::Compare {
                        operand,
                        op: CmpOp::Eq,
                        value: scalar,
                    },
                ));
            }
        }
    }
    let a_label = a.labels.first().cloned();
    let b_label = b.labels.first().cloned();
    let node_labels = [a_label.clone(), b_label.clone()];
    if let Some(where_expr) = &match_clause.where_clause {
        let ctx = SegCtx {
            roles: &roles,
            node_labels: &node_labels,
            segments: &[],
            hints,
        };
        filter = Some(seg_conjoin(filter, lower_seg_predicate(where_expr, &ctx, params)?));
    }

    let mut selected = Vec::new();
    if let Some(var) = &a_var {
        selected.push(SelectedBinding::Node {
            var: var.clone(),
            node: 0,
            optional: false,
        });
    }
    if let Some(var) = &b_var {
        selected.push(SelectedBinding::Node {
            var: var.clone(),
            node: 1,
            optional: false,
        });
    }

    let projection = return_clause.projection.clone();
    let ordering = compute_seg_ordering(&projection, &roles, &node_labels, &[], params, hints);

    Some(VarLengthReadPushdown {
        direction,
        rel_types: rel.types.clone(),
        min,
        max,
        a_label,
        b_label,
        filter,
        selected,
        ordering,
        projection,
    })
}

impl VarLengthReadPushdown {
    pub fn pushes_ordering(&self, dialect: &dyn SqlDialect) -> bool {
        match &self.ordering {
            None => false,
            Some(ordering) => {
                dialect.orders_json_typed() || ordering.keys.iter().all(|k| k.kind.is_some())
            }
        }
    }

    pub fn column_count(&self) -> usize {
        (self.selected.len() * 3).max(1)
    }

    /// The recursive-CTE `next`-endpoint expression and the edge-incidence
    /// predicate connecting walk row `w` to an edge `ed`, per direction.
    fn step(&self) -> (String, String) {
        match self.direction {
            SegDirection::Outgoing => {
                ("ed.dst_id".to_string(), "ed.src_id = w.e".to_string())
            }
            SegDirection::Incoming => {
                ("ed.src_id".to_string(), "ed.dst_id = w.e".to_string())
            }
            SegDirection::Undirected => (
                "CASE WHEN ed.src_id = w.e THEN ed.dst_id ELSE ed.src_id END".to_string(),
                "(ed.src_id = w.e OR ed.dst_id = w.e)".to_string(),
            ),
        }
    }

    pub fn to_sql(&self, dialect: &dyn SqlDialect) -> String {
        let nodes = dialect.quote_ident(dialect.nodes_table());
        let edges = dialect.quote_ident(dialect.edges_table());
        // U+001F (unit separator) is the delimiter Grust already reserves in keys,
        // so wrapping ids with it makes the visited-set membership unambiguous.
        let sep = dialect.string_literal("\u{1f}");
        let (next, incident) = self.step();

        // Recursion edge-type filter.
        let edge_filter = match self.rel_types.as_slice() {
            [] => String::new(),
            [one] => format!(" AND ed.edge_type = {}", dialect.string_literal(one)),
            many => {
                let list = many
                    .iter()
                    .map(|t| dialect.string_literal(t))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(" AND ed.edge_type IN ({list})")
            }
        };
        let depth_cap = match self.max {
            Some(m) => format!(" AND w.depth + 1 <= {m}"),
            None => String::new(),
        };

        let mut cte = format!(
            "WITH RECURSIVE walk(s, e, depth, visited) AS (\
             SELECT id, id, 0, {sep} || id || {sep} FROM {nodes} \
             UNION ALL \
             SELECT w.s, {next}, w.depth + 1, w.visited || {next} || {sep} \
             FROM walk w JOIN {edges} ed ON {incident} \
             WHERE instr(w.visited, {sep} || {next} || {sep}) = 0{edge_filter}{depth_cap}) ",
        );

        let mut cols: Vec<String> = Vec::new();
        for binding in &self.selected {
            if let SelectedBinding::Node { node, .. } = binding {
                cols.push(format!("n{node}.id"));
                cols.push(format!("n{node}.label"));
                cols.push(format!("n{node}.props"));
            }
        }
        let select_list = if cols.is_empty() {
            "1".to_string()
        } else {
            cols.join(", ")
        };

        cte.push_str(&format!(
            "SELECT {select_list} FROM walk w \
             JOIN {nodes} n0 ON n0.id = w.s \
             JOIN {nodes} n1 ON n1.id = w.e",
        ));

        let mut conditions = vec![format!("w.depth >= {}", self.min)];
        if let Some(label) = &self.a_label {
            conditions.push(format!("n0.label = {}", dialect.string_literal(label)));
        }
        if let Some(label) = &self.b_label {
            conditions.push(format!("n1.label = {}", dialect.string_literal(label)));
        }
        if let Some(filter) = &self.filter {
            conditions.push(render_seg_predicate(filter, dialect));
        }
        cte.push_str(" WHERE ");
        cte.push_str(&conditions.join(" AND "));
        if self.pushes_ordering(dialect) {
            cte.push_str(&render_seg_order_limit(
                self.ordering.as_ref().unwrap(),
                dialect,
            ));
        }
        cte
    }

    pub fn project_text_rows(
        &self,
        dialect: &dyn SqlDialect,
        rows: Vec<Vec<Option<String>>>,
        params: &CypherParameters,
    ) -> Result<CypherResultTable> {
        let binding_rows = reconstruct_bindings(&self.selected, rows)?;
        if self.pushes_ordering(dialect) {
            let projection = strip_order_limit(&self.projection);
            crate::read::project_bindings(binding_rows, &projection, params)
        } else {
            crate::read::project_bindings(binding_rows, &self.projection, params)
        }
    }
}

// ===========================================================================
// OPTIONAL MATCH pushdown (mandatory node + one optional directed segment)
// ===========================================================================
//
// `MATCH (a[:LA] [{..}]) [WHERE wa] OPTIONAL MATCH (a)-[r?:T [{..}]]->(b[:LB]
// [{..}]) [WHERE wb] RETURN …` lowers to a LEFT JOIN of the mandatory node `a`
// (`n0`) against a subquery that is the *whole* optional segment (edge `e0` ⋈ end
// node `n1`, with all optional conditions). The subquery makes the optional match
// atomic — when nothing matches, every optional column is NULL and `r`/`b` are
// null-padded, exactly like the reference. `wa` references only `a`; `wb`/inline
// props reference only `r`/`b` (else the query falls back).

/// A lowered `OPTIONAL MATCH` (mandatory node + one optional directed segment).
#[derive(Clone, Debug, PartialEq)]
pub struct OptionalReadPushdown {
    a_label: Option<String>,
    /// Filter over the mandatory node `a` (`n0`) — outer `WHERE`.
    a_filter: Option<SegPredicate>,
    /// Outgoing/Incoming only (undirected falls back).
    direction: SegDirection,
    rel_types: Vec<String>,
    b_label: Option<String>,
    /// Filter over the optional `r`/`b` (`e0`/`n1`) — inside the subquery.
    opt_filter: Option<SegPredicate>,
    /// `a` (mandatory), then optional `r`/`b` (in SELECT order).
    selected: Vec<SelectedBinding>,
    projection: Projection,
}

fn lower_optional_single(
    single: &SingleQuery,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Option<OptionalReadPushdown> {
    // Exactly `MATCH (a …) [WHERE] OPTIONAL MATCH (a)-[r?]->(b …) [WHERE] RETURN`.
    let (m, o, return_clause) = match single.clauses.as_slice() {
        [Clause::Match(m), Clause::Match(o), Clause::Return(r)] if !m.optional && o.optional => {
            (m, o, r)
        }
        _ => return None,
    };
    // Mandatory part: a single bound node pattern, ≤1 label.
    if m.patterns.len() != 1 {
        return None;
    }
    let a_pat = &m.patterns[0];
    if a_pat.variable.is_some() || !a_pat.segments.is_empty() || a_pat.start.labels.len() > 1 {
        return None;
    }
    let a_node = &a_pat.start;
    let a_var = a_node.variable.clone()?;

    // Optional part: a single segment continuing from `a`.
    if o.patterns.len() != 1 {
        return None;
    }
    let o_pat = &o.patterns[0];
    if o_pat.variable.is_some() || o_pat.segments.len() != 1 {
        return None;
    }
    // The optional start must be a bare back-reference to `a`.
    if o_pat.start.variable.as_deref() != Some(a_var.as_str())
        || !o_pat.start.labels.is_empty()
        || o_pat.start.properties.is_some()
    {
        return None;
    }
    let seg = &o_pat.segments[0];
    let rel = &seg.relationship;
    let b_node = &seg.node;
    if rel.length.is_some() || b_node.labels.len() > 1 {
        return None;
    }
    let direction = match rel.direction {
        Direction::Outgoing => SegDirection::Outgoing,
        Direction::Incoming => SegDirection::Incoming,
        Direction::Undirected => return None,
    };
    let b_var = b_node.variable.clone();
    let r_var = rel.variable.clone();
    // Variables must be distinct from `a` and each other.
    for v in [b_var.as_ref(), r_var.as_ref()].into_iter().flatten() {
        if v == &a_var {
            return None;
        }
    }
    if let (Some(rv), Some(bv)) = (&r_var, &b_var) {
        if rv == bv {
            return None;
        }
    }

    let a_label = a_node.labels.first().cloned();
    let b_label = b_node.labels.first().cloned();
    let node_labels = [a_label.clone(), b_label.clone()];
    let segments = [SegSpec {
        direction,
        rel_types: rel.types.clone(),
    }];

    // Mandatory filter over `a` (node 0): inline props + WHERE; must not touch r/b.
    let a_roles: RoleMap = std::iter::once((a_var.clone(), Role::Node(0))).collect();
    let mut a_filter = None;
    if let Some(map) = &a_node.properties {
        for (key, value) in &map.entries {
            let scalar = lower_scalar(value, params)?;
            let operand = if key == "label" {
                SegOperand::NodeLabel(0)
            } else {
                SegOperand::NodeProp(0, lower_seg_key(key)?)
            };
            a_filter = Some(seg_conjoin(
                a_filter,
                SegPredicate::Compare {
                    operand,
                    op: CmpOp::Eq,
                    value: scalar,
                },
            ));
        }
    }
    if let Some(where_expr) = &m.where_clause {
        let ctx = SegCtx {
            roles: &a_roles,
            node_labels: &node_labels,
            segments: &segments,
            hints,
        };
        a_filter = Some(seg_conjoin(a_filter, lower_seg_predicate(where_expr, &ctx, params)?));
    }

    // Optional filter over `r` (edge 0) and `b` (node 1): inline props + WHERE;
    // must not reference `a` (the subquery cannot see the outer node).
    let opt_roles: RoleMap = {
        let mut m = std::collections::HashMap::new();
        if let Some(v) = &r_var {
            m.insert(v.clone(), Role::Edge(0));
        }
        if let Some(v) = &b_var {
            m.insert(v.clone(), Role::Node(1));
        }
        m
    };
    let mut opt_filter = None;
    if let Some(map) = &rel.properties {
        for (key, value) in &map.entries {
            let scalar = lower_scalar(value, params)?;
            opt_filter = Some(seg_conjoin(
                opt_filter,
                SegPredicate::Compare {
                    operand: SegOperand::EdgeProp(0, lower_seg_key(key)?),
                    op: CmpOp::Eq,
                    value: scalar,
                },
            ));
        }
    }
    if let Some(map) = &b_node.properties {
        for (key, value) in &map.entries {
            let scalar = lower_scalar(value, params)?;
            let operand = if key == "label" {
                SegOperand::NodeLabel(1)
            } else {
                SegOperand::NodeProp(1, lower_seg_key(key)?)
            };
            opt_filter = Some(seg_conjoin(
                opt_filter,
                SegPredicate::Compare {
                    operand,
                    op: CmpOp::Eq,
                    value: scalar,
                },
            ));
        }
    }
    if let Some(where_expr) = &o.where_clause {
        let ctx = SegCtx {
            roles: &opt_roles,
            node_labels: &node_labels,
            segments: &segments,
            hints,
        };
        opt_filter = Some(seg_conjoin(opt_filter, lower_seg_predicate(where_expr, &ctx, params)?));
    }

    let mut selected = vec![SelectedBinding::Node {
        var: a_var,
        node: 0,
        optional: false,
    }];
    if let Some(var) = r_var {
        selected.push(SelectedBinding::Edge {
            var,
            edge: 0,
            optional: true,
        });
    }
    if let Some(var) = b_var {
        selected.push(SelectedBinding::Node {
            var,
            node: 1,
            optional: true,
        });
    }

    Some(OptionalReadPushdown {
        a_label,
        a_filter,
        direction,
        rel_types: rel.types.clone(),
        b_label,
        opt_filter,
        selected,
        projection: return_clause.projection.clone(),
    })
}

impl OptionalReadPushdown {
    pub fn column_count(&self) -> usize {
        self.selected
            .iter()
            .map(|b| match b {
                SelectedBinding::Node { .. } => 3,
                SelectedBinding::Edge { .. } => 5,
            })
            .sum()
    }

    pub fn to_sql(&self, dialect: &dyn SqlDialect) -> String {
        let nodes = dialect.quote_ident(dialect.nodes_table());
        let edges = dialect.quote_ident(dialect.edges_table());
        // Subquery = the whole optional segment (edge e0 ⋈ end node n1), keyed by
        // the anchor endpoint that joins to the outer node n0.
        let (anchor, b_join) = match self.direction {
            SegDirection::Outgoing => ("e0.src_id", "n1.id = e0.dst_id"),
            // Undirected is rejected at lowering.
            _ => ("e0.dst_id", "n1.id = e0.src_id"),
        };
        let mut sub_conds: Vec<String> = Vec::new();
        match self.rel_types.as_slice() {
            [] => {}
            [one] => sub_conds.push(format!("e0.edge_type = {}", dialect.string_literal(one))),
            many => {
                let list = many
                    .iter()
                    .map(|t| dialect.string_literal(t))
                    .collect::<Vec<_>>()
                    .join(", ");
                sub_conds.push(format!("e0.edge_type IN ({list})"));
            }
        }
        if let Some(label) = &self.b_label {
            sub_conds.push(format!("n1.label = {}", dialect.string_literal(label)));
        }
        if let Some(filter) = &self.opt_filter {
            sub_conds.push(render_seg_predicate(filter, dialect));
        }
        let sub_where = if sub_conds.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", sub_conds.join(" AND "))
        };
        let subquery = format!(
            "SELECT {anchor} AS anchor, \
             e0.id AS r_id, e0.src_id AS r_src, e0.dst_id AS r_dst, e0.edge_type AS r_type, \
             e0.props AS r_props, n1.id AS b_id, n1.label AS b_label, n1.props AS b_props \
             FROM {edges} e0 JOIN {nodes} n1 ON {b_join}{sub_where}"
        );

        // Outer SELECT: n0 columns, then opt.* per selected optional binding.
        let mut cols: Vec<String> = Vec::new();
        for binding in &self.selected {
            match binding {
                SelectedBinding::Node { optional: false, .. } => {
                    cols.extend(["n0.id".into(), "n0.label".into(), "n0.props".into()]);
                }
                SelectedBinding::Edge { .. } => cols.extend([
                    "opt.r_id".into(),
                    "opt.r_src".into(),
                    "opt.r_dst".into(),
                    "opt.r_type".into(),
                    "opt.r_props".into(),
                ]),
                SelectedBinding::Node { optional: true, .. } => {
                    cols.extend(["opt.b_id".into(), "opt.b_label".into(), "opt.b_props".into()]);
                }
            }
        }
        let mut sql = format!(
            "SELECT {} FROM {nodes} n0 LEFT JOIN ({subquery}) opt ON opt.anchor = n0.id",
            cols.join(", ")
        );
        let mut outer: Vec<String> = Vec::new();
        if let Some(label) = &self.a_label {
            outer.push(format!("n0.label = {}", dialect.string_literal(label)));
        }
        if let Some(filter) = &self.a_filter {
            outer.push(render_seg_predicate(filter, dialect));
        }
        if !outer.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&outer.join(" AND "));
        }
        sql
    }

    pub fn project_text_rows(
        &self,
        _dialect: &dyn SqlDialect,
        rows: Vec<Vec<Option<String>>>,
        params: &CypherParameters,
    ) -> Result<CypherResultTable> {
        let binding_rows = reconstruct_bindings(&self.selected, rows)?;
        // Ordering/limit are not pushed for OPTIONAL; the reference projection
        // applies them over the null-padded rows.
        crate::read::project_bindings(binding_rows, &self.projection, params)
    }
}

// ===========================================================================
// Multi-pattern MATCH pushdown (comma patterns → cross / shared-variable joins)
// ===========================================================================
//
// `MATCH (a)-[:T]->(b), (a)-[:U]->(c) [WHERE …] RETURN …` (and bare cross
// products like `MATCH (a), (b)`) lower to a comma-join of every node/edge alias
// with all connectivity + filters in `WHERE`. A variable shared across patterns
// reuses its alias, so the join unifies it; patterns with no shared variable
// cross-join. Directed segments only (undirected falls back). Tried after the
// single-path segment planner, so it handles ≥2 patterns and single patterns
// that reuse a variable.

/// One global edge: the node indices its stored `src_id`/`dst_id` join to.
#[derive(Clone, Debug, PartialEq)]
struct GlobalEdge {
    src_node: usize,
    dst_node: usize,
    rel_types: Vec<String>,
}

/// A lowered multi-pattern (or shared-variable) `MATCH`.
#[derive(Clone, Debug, PartialEq)]
pub struct MultiPatternReadPushdown {
    node_count: usize,
    node_labels: Vec<Option<String>>,
    edges: Vec<GlobalEdge>,
    filter: Option<SegPredicate>,
    selected: Vec<SelectedBinding>,
    projection: Projection,
}

fn lower_multi_pattern_single(
    single: &SingleQuery,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Option<MultiPatternReadPushdown> {
    let (match_clause, return_clause) = match single.clauses.as_slice() {
        [Clause::Match(m), Clause::Return(r)] if !m.optional => (m, r),
        _ => return None,
    };
    if match_clause.patterns.is_empty() {
        return None;
    }

    let mut node_labels: Vec<Option<String>> = Vec::new();
    let mut var_node: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut roles: RoleMap = std::collections::HashMap::new();
    let mut edges: Vec<GlobalEdge> = Vec::new();
    let mut selected: Vec<SelectedBinding> = Vec::new();
    let mut filter: Option<SegPredicate> = None;

    // Resolve a node occurrence to a global index, recording labels/inline props
    // for fresh nodes; a re-referenced variable must be a bare back-reference.
    let mut resolve_node = |np: &NodePattern,
                            node_labels: &mut Vec<Option<String>>,
                            roles: &mut RoleMap,
                            selected: &mut Vec<SelectedBinding>,
                            filter: &mut Option<SegPredicate>|
     -> Option<usize> {
        if let Some(v) = &np.variable {
            if let Some(&idx) = var_node.get(v) {
                if !np.labels.is_empty() || np.properties.is_some() {
                    return None; // re-reference must be bare
                }
                return Some(idx);
            }
        }
        if np.labels.len() > 1 {
            return None;
        }
        let idx = node_labels.len();
        node_labels.push(np.labels.first().cloned());
        if let Some(v) = &np.variable {
            var_node.insert(v.clone(), idx);
            roles.insert(v.clone(), Role::Node(idx));
            selected.push(SelectedBinding::Node {
                var: v.clone(),
                node: idx,
                optional: false,
            });
        }
        if let Some(map) = &np.properties {
            for (key, value) in &map.entries {
                let scalar = lower_scalar(value, params)?;
                let operand = if key == "label" {
                    SegOperand::NodeLabel(idx)
                } else {
                    SegOperand::NodeProp(idx, lower_seg_key(key)?)
                };
                *filter = Some(seg_conjoin(
                    filter.take(),
                    SegPredicate::Compare {
                        operand,
                        op: CmpOp::Eq,
                        value: scalar,
                    },
                ));
            }
        }
        Some(idx)
    };

    for pattern in &match_clause.patterns {
        if pattern.variable.is_some() {
            return None; // no path variable
        }
        let mut prev = resolve_node(
            &pattern.start,
            &mut node_labels,
            &mut roles,
            &mut selected,
            &mut filter,
        )?;
        for seg in &pattern.segments {
            let rel = &seg.relationship;
            if rel.length.is_some() {
                return None;
            }
            let node_idx = resolve_node(
                &seg.node,
                &mut node_labels,
                &mut roles,
                &mut selected,
                &mut filter,
            )?;
            let (src_node, dst_node) = match rel.direction {
                Direction::Outgoing => (prev, node_idx),
                Direction::Incoming => (node_idx, prev),
                Direction::Undirected => return None,
            };
            let edge_idx = edges.len();
            if let Some(v) = &rel.variable {
                if roles.insert(v.clone(), Role::Edge(edge_idx)).is_some() {
                    return None;
                }
                selected.push(SelectedBinding::Edge {
                    var: v.clone(),
                    edge: edge_idx,
                    optional: false,
                });
            }
            if let Some(map) = &rel.properties {
                for (key, value) in &map.entries {
                    let scalar = lower_scalar(value, params)?;
                    filter = Some(seg_conjoin(
                        filter,
                        SegPredicate::Compare {
                            operand: SegOperand::EdgeProp(edge_idx, lower_seg_key(key)?),
                            op: CmpOp::Eq,
                            value: scalar,
                        },
                    ));
                }
            }
            edges.push(GlobalEdge {
                src_node,
                dst_node,
                rel_types: rel.types.clone(),
            });
            prev = node_idx;
        }
    }

    if let Some(where_expr) = &match_clause.where_clause {
        let segs: Vec<SegSpec> = edges
            .iter()
            .map(|e| SegSpec {
                direction: SegDirection::Outgoing,
                rel_types: e.rel_types.clone(),
            })
            .collect();
        let ctx = SegCtx {
            roles: &roles,
            node_labels: &node_labels,
            segments: &segs,
            hints,
        };
        filter = Some(seg_conjoin(filter, lower_seg_predicate(where_expr, &ctx, params)?));
    }

    Some(MultiPatternReadPushdown {
        node_count: node_labels.len(),
        node_labels,
        edges,
        filter,
        selected,
        projection: return_clause.projection.clone(),
    })
}

impl MultiPatternReadPushdown {
    pub fn column_count(&self) -> usize {
        self.selected
            .iter()
            .map(|b| match b {
                SelectedBinding::Node { .. } => 3,
                SelectedBinding::Edge { .. } => 5,
            })
            .sum()
    }

    pub fn to_sql(&self, dialect: &dyn SqlDialect) -> String {
        let nodes = dialect.quote_ident(dialect.nodes_table());
        let edges_tbl = dialect.quote_ident(dialect.edges_table());

        let mut cols: Vec<String> = Vec::new();
        for binding in &self.selected {
            match binding {
                SelectedBinding::Node { node, .. } => cols.extend([
                    format!("n{node}.id"),
                    format!("n{node}.label"),
                    format!("n{node}.props"),
                ]),
                SelectedBinding::Edge { edge, .. } => cols.extend([
                    format!("e{edge}.id"),
                    format!("e{edge}.src_id"),
                    format!("e{edge}.dst_id"),
                    format!("e{edge}.edge_type"),
                    format!("e{edge}.props"),
                ]),
            }
        }
        let select_list = if cols.is_empty() {
            "1".to_string()
        } else {
            cols.join(", ")
        };

        // Comma-join every node and edge alias; connectivity + filters in WHERE.
        let mut from: Vec<String> = (0..self.node_count)
            .map(|i| format!("{nodes} n{i}"))
            .collect();
        for j in 0..self.edges.len() {
            from.push(format!("{edges_tbl} e{j}"));
        }

        let mut conds: Vec<String> = Vec::new();
        for (j, e) in self.edges.iter().enumerate() {
            conds.push(format!("e{j}.src_id = n{}.id", e.src_node));
            conds.push(format!("e{j}.dst_id = n{}.id", e.dst_node));
            match e.rel_types.as_slice() {
                [] => {}
                [one] => conds.push(format!("e{j}.edge_type = {}", dialect.string_literal(one))),
                many => {
                    let list = many
                        .iter()
                        .map(|t| dialect.string_literal(t))
                        .collect::<Vec<_>>()
                        .join(", ");
                    conds.push(format!("e{j}.edge_type IN ({list})"));
                }
            }
        }
        for (i, label) in self.node_labels.iter().enumerate() {
            if let Some(label) = label {
                conds.push(format!("n{i}.label = {}", dialect.string_literal(label)));
            }
        }
        if let Some(filter) = &self.filter {
            conds.push(render_seg_predicate(filter, dialect));
        }

        let mut sql = format!("SELECT {select_list} FROM {}", from.join(", "));
        if !conds.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
        }
        sql
    }

    pub fn project_text_rows(
        &self,
        _dialect: &dyn SqlDialect,
        rows: Vec<Vec<Option<String>>>,
        params: &CypherParameters,
    ) -> Result<CypherResultTable> {
        let binding_rows = reconstruct_bindings(&self.selected, rows)?;
        crate::read::project_bindings(binding_rows, &self.projection, params)
    }
}

// ===========================================================================
// WITH-horizon pushdown (push the leading node scan; run the tail in Rust)
// ===========================================================================
//
// `MATCH (n[:L] [{..}]) [WHERE p] WITH … [WHERE] [UNWIND …] RETURN …` pushes the
// leading single-node scan + filter into SQL, then runs the `WITH`/`UNWIND`/
// `RETURN` horizon over the fetched nodes through the shared reference pipeline
// (`read::project_binding_pipeline`) — identical to the reference by construction.
// The tail must not contain a further `MATCH` (that needs graph access).

/// A lowered `MATCH (single node) … WITH … RETURN` with a `WITH`/`UNWIND` horizon.
#[derive(Clone, Debug, PartialEq)]
pub struct PipelineReadPushdown {
    var: String,
    label: Option<String>,
    filter: Option<Predicate>,
    /// Clauses after the leading `MATCH` (a `WITH`/`UNWIND` horizon ending in
    /// `RETURN`), run through the reference pipeline.
    tail: Vec<Clause>,
}

fn lower_pipeline_single(
    single: &SingleQuery,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Option<PipelineReadPushdown> {
    let clauses = &single.clauses;
    // Leading non-optional MATCH, a horizon, then RETURN — at least one clause
    // between MATCH and RETURN (else the plain node leaf handles it).
    let match_clause = match clauses.first() {
        Some(Clause::Match(m)) if !m.optional => m,
        _ => return None,
    };
    if clauses.len() < 3 || !matches!(clauses.last(), Some(Clause::Return(_))) {
        return None;
    }
    let tail = &clauses[1..];
    // The horizon may only re-shape rows (no further graph access / writes).
    if !tail
        .iter()
        .all(|c| matches!(c, Clause::With(_) | Clause::Unwind(_) | Clause::Return(_)))
    {
        return None;
    }
    let (var, label, filter) = lower_node_scan(match_clause, params, hints)?;
    Some(PipelineReadPushdown {
        var,
        label,
        filter,
        tail: tail.to_vec(),
    })
}

impl PipelineReadPushdown {
    pub fn column_count(&self) -> usize {
        3
    }

    pub fn to_sql(&self, dialect: &dyn SqlDialect) -> String {
        let table = dialect.quote_ident(dialect.nodes_table());
        let mut sql = format!("SELECT id, label, props FROM {table}");
        let mut conditions: Vec<String> = Vec::new();
        if let Some(label) = &self.label {
            conditions.push(format!("label = {}", dialect.string_literal(label)));
        }
        if let Some(filter) = &self.filter {
            conditions.push(render_predicate(filter, dialect));
        }
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        sql
    }

    pub fn project_text_rows(
        &self,
        _dialect: &dyn SqlDialect,
        rows: Vec<Vec<Option<String>>>,
        params: &CypherParameters,
    ) -> Result<CypherResultTable> {
        let selected = [SelectedBinding::Node {
            var: self.var.clone(),
            node: 0,
            optional: false,
        }];
        let binding_rows = reconstruct_bindings(&selected, rows)?;
        crate::read::project_binding_pipeline(binding_rows, &self.tail, params)
    }
}

/// Parse a JSON `props` text cell (untagged) into [`Props`]. NULL/empty → empty.
fn parse_props(text: Option<&str>) -> Result<Props> {
    match text {
        None => Ok(Props::new()),
        Some(s) if s.is_empty() => Ok(Props::new()),
        Some(s) => {
            let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(s)
                .map_err(|e| crate::gql::gql_execution(format!("pushdown props JSON parse: {e}")))?;
            Ok(map
                .into_iter()
                .map(|(k, v)| (k, Value::from_json(v)))
                .collect())
        }
    }
}

// ===========================================================================
// Unified read pushdown (single-query leaves + UNION composition)
// ===========================================================================

/// A pushable read: one of the single-query leaves, or a `UNION` of leaves.
///
/// Leaves expose a uniform text-rows execution contract ([`Self::to_sql`],
/// [`Self::column_count`], [`Self::project_text_rows`]); a backend executes the
/// SQL and hands the text rows back to project. `Union` is executed by running
/// each arm and combining the result tables with [`combine_union`].
#[derive(Clone, Debug, PartialEq)]
pub enum ReadPushdown {
    Node(NodeReadPushdown),
    Segment(SegmentReadPushdown),
    VarLength(VarLengthReadPushdown),
    Optional(OptionalReadPushdown),
    MultiPattern(MultiPatternReadPushdown),
    Pipeline(PipelineReadPushdown),
    /// `UNION` / `UNION ALL` of pushable arms; `distinct` is true when any
    /// boundary is a (deduplicating) `UNION`.
    Union {
        arms: Vec<ReadPushdown>,
        distinct: bool,
    },
}

impl ReadPushdown {
    /// True for a `UNION` composition (the backend must run each arm and
    /// [`combine_union`] the tables, not call the leaf methods).
    pub fn is_union(&self) -> bool {
        matches!(self, ReadPushdown::Union { .. })
    }

    /// The arms of a `UNION` (empty for a leaf) and whether it deduplicates.
    pub fn union_arms(&self) -> Option<(&[ReadPushdown], bool)> {
        match self {
            ReadPushdown::Union { arms, distinct } => Some((arms, *distinct)),
            _ => None,
        }
    }

    /// Render the leaf's SQL (panics on `Union` — run its arms instead).
    pub fn to_sql(&self, dialect: &dyn SqlDialect) -> String {
        match self {
            ReadPushdown::Node(p) => p.to_sql(dialect),
            ReadPushdown::Segment(p) => p.to_sql(dialect),
            ReadPushdown::VarLength(p) => p.to_sql(dialect),
            ReadPushdown::Optional(p) => p.to_sql(dialect),
            ReadPushdown::MultiPattern(p) => p.to_sql(dialect),
            ReadPushdown::Pipeline(p) => p.to_sql(dialect),
            ReadPushdown::Union { .. } => unreachable!("call union_arms for a UNION"),
        }
    }

    /// The number of text columns the leaf's SQL emits.
    pub fn column_count(&self) -> usize {
        match self {
            ReadPushdown::Node(p) => p.column_count(),
            ReadPushdown::Segment(p) => p.column_count(),
            ReadPushdown::VarLength(p) => p.column_count(),
            ReadPushdown::Optional(p) => p.column_count(),
            ReadPushdown::MultiPattern(p) => p.column_count(),
            ReadPushdown::Pipeline(p) => p.column_count(),
            ReadPushdown::Union { .. } => unreachable!("call union_arms for a UNION"),
        }
    }

    /// Reconstruct + project the leaf's text rows.
    pub fn project_text_rows(
        &self,
        dialect: &dyn SqlDialect,
        rows: Vec<Vec<Option<String>>>,
        params: &CypherParameters,
    ) -> Result<CypherResultTable> {
        match self {
            ReadPushdown::Node(p) => p.project_text_rows(dialect, rows, params),
            ReadPushdown::Segment(p) => p.project_text_rows(dialect, rows, params),
            ReadPushdown::VarLength(p) => p.project_text_rows(dialect, rows, params),
            ReadPushdown::Optional(p) => p.project_text_rows(dialect, rows, params),
            ReadPushdown::MultiPattern(p) => p.project_text_rows(dialect, rows, params),
            ReadPushdown::Pipeline(p) => p.project_text_rows(dialect, rows, params),
            ReadPushdown::Union { .. } => unreachable!("call union_arms for a UNION"),
        }
    }
}

/// Try to lower `cypher` into a unified [`ReadPushdown`] — a single-query leaf
/// (node / segment / variable-length), or a `UNION` of such leaves. `Ok(None)`
/// for any valid query outside the pushable subset (caller falls back to the
/// reference), `Err` only for invalid syntax/semantics.
pub fn plan_read(
    cypher: &str,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Result<Option<ReadPushdown>> {
    let query = parse_query(cypher).map_err(|e| e.into_grust(cypher))?;
    crate::semantics::analyze(&query)?;

    if query.parts.len() == 1 && query.parts[0].union.is_none() {
        return Ok(lower_single(&query.parts[0].query, params, hints));
    }
    // UNION: every arm must be a pushable single-query leaf.
    let mut arms = Vec::with_capacity(query.parts.len());
    let mut distinct = false;
    for part in &query.parts {
        if part.union == Some(UnionKind::Distinct) {
            distinct = true;
        }
        match lower_single(&part.query, params, hints) {
            Some(leaf) => arms.push(leaf),
            None => return Ok(None),
        }
    }
    Ok(Some(ReadPushdown::Union { arms, distinct }))
}

/// Lower one `SingleQuery` to a leaf, trying node → segment → variable-length.
fn lower_single(
    single: &SingleQuery,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Option<ReadPushdown> {
    if let Some(p) = lower_node_single(single, params, hints) {
        return Some(ReadPushdown::Node(p));
    }
    if let Some(p) = lower_segment_single(single, params, hints) {
        return Some(ReadPushdown::Segment(p));
    }
    if let Some(p) = lower_var_length_single(single, params, hints) {
        return Some(ReadPushdown::VarLength(p));
    }
    if let Some(p) = lower_optional_single(single, params, hints) {
        return Some(ReadPushdown::Optional(p));
    }
    if let Some(p) = lower_multi_pattern_single(single, params, hints) {
        return Some(ReadPushdown::MultiPattern(p));
    }
    lower_pipeline_single(single, params, hints).map(ReadPushdown::Pipeline)
}

/// Combine `UNION` arm result tables, mirroring [`crate::read::run_read_query`]:
/// all arms must share column names; rows are concatenated, then deduplicated
/// when `distinct`.
pub fn combine_union(
    tables: Vec<CypherResultTable>,
    distinct: bool,
) -> Result<CypherResultTable> {
    let mut tables = tables.into_iter();
    let mut combined = tables
        .next()
        .ok_or_else(|| crate::gql::gql_execution("UNION has no arms"))?;
    for table in tables {
        if table.columns != combined.columns {
            return Err(crate::gql::gql_name(
                "all UNION arms must return the same column names in the same order",
            ));
        }
        combined.rows.extend(table.rows);
    }
    if distinct {
        combined.rows = crate::read::dedup_return_rows(combined.rows, "UNION")?;
    }
    Ok(combined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use grust_core::Props;

    fn params() -> CypherParameters {
        CypherParameters::new()
    }

    fn plan(cypher: &str) -> Option<NodeReadPushdown> {
        plan_node_read(cypher, &params()).unwrap()
    }

    fn spark_sql(cypher: &str) -> String {
        plan(cypher).expect("pushable").to_sql(&SparkDialect)
    }

    fn sqlite_sql(cypher: &str) -> String {
        plan(cypher).expect("pushable").to_sql(&SqliteDialect)
    }

    #[test]
    fn label_only_scan() {
        assert_eq!(
            spark_sql("MATCH (n:Person) RETURN n.name"),
            "SELECT id, label, props FROM `grust_nodes` WHERE label = 'Person'"
        );
        assert_eq!(
            sqlite_sql("MATCH (n:Person) RETURN n.name"),
            "SELECT id, label, props FROM \"grust_nodes\" WHERE label = 'Person'"
        );
    }

    #[test]
    fn unlabeled_scan_has_no_where() {
        assert_eq!(
            spark_sql("MATCH (n) RETURN n"),
            "SELECT id, label, props FROM `grust_nodes`"
        );
    }

    #[test]
    fn numeric_comparison_casts_by_literal_type() {
        assert_eq!(
            spark_sql("MATCH (n:Person) WHERE n.age >= 40 RETURN n.name"),
            "SELECT id, label, props FROM `grust_nodes` \
             WHERE label = 'Person' AND CAST(GET_JSON_OBJECT(props, '$.age') AS BIGINT) >= 40"
        );
        assert_eq!(
            sqlite_sql("MATCH (n:Person) WHERE n.score > 1.5 RETURN n"),
            "SELECT id, label, props FROM \"grust_nodes\" \
             WHERE label = 'Person' AND CAST(json_extract(props, '$.score') AS REAL) > 1.5"
        );
    }

    #[test]
    fn inline_props_and_where_conjoin() {
        assert_eq!(
            spark_sql("MATCH (n:Person {name:'Ada'}) WHERE n.age >= 40 RETURN n"),
            "SELECT id, label, props FROM `grust_nodes` \
             WHERE label = 'Person' AND (GET_JSON_OBJECT(props, '$.name') = 'Ada' \
             AND CAST(GET_JSON_OBJECT(props, '$.age') AS BIGINT) >= 40)"
        );
    }

    #[test]
    fn boolean_structure_and_is_null() {
        assert_eq!(
            spark_sql(
                "MATCH (n:Person) WHERE n.age > 30 AND (n.name = 'Ada' OR n.city IS NULL) RETURN n"
            ),
            "SELECT id, label, props FROM `grust_nodes` WHERE label = 'Person' AND \
             (CAST(GET_JSON_OBJECT(props, '$.age') AS BIGINT) > 30 AND \
             (GET_JSON_OBJECT(props, '$.name') = 'Ada' OR GET_JSON_OBJECT(props, '$.city') IS NULL))"
        );
    }

    #[test]
    fn in_list_predicate() {
        assert_eq!(
            spark_sql("MATCH (n:Person) WHERE n.age IN [30, 40, 50] RETURN n.name"),
            "SELECT id, label, props FROM `grust_nodes` \
             WHERE label = 'Person' AND CAST(GET_JSON_OBJECT(props, '$.age') AS BIGINT) IN (30, 40, 50)"
        );
        // NOT IN via the Not wrapper; string list rendered/escaped by the dialect.
        assert_eq!(
            sqlite_sql("MATCH (n:Person) WHERE NOT n.name IN ['Ada', 'Alan'] RETURN n"),
            "SELECT id, label, props FROM \"grust_nodes\" \
             WHERE label = 'Person' AND (NOT json_extract(props, '$.name') IN ('Ada', 'Alan'))"
        );
    }

    #[test]
    fn string_predicates() {
        assert_eq!(
            spark_sql("MATCH (n:Person) WHERE n.name STARTS WITH 'Ad' RETURN n"),
            "SELECT id, label, props FROM `grust_nodes` \
             WHERE label = 'Person' AND STARTSWITH(GET_JSON_OBJECT(props, '$.name'), 'Ad')"
        );
        assert_eq!(
            spark_sql("MATCH (n:Person) WHERE n.name CONTAINS 'da' RETURN n"),
            "SELECT id, label, props FROM `grust_nodes` \
             WHERE label = 'Person' AND CONTAINS(GET_JSON_OBJECT(props, '$.name'), 'da')"
        );
        // SQLite uses instr/substr (literal, NULL-propagating).
        assert_eq!(
            sqlite_sql("MATCH (n:Person) WHERE n.name ENDS WITH 'da' RETURN n"),
            "SELECT id, label, props FROM \"grust_nodes\" \
             WHERE label = 'Person' AND substr(json_extract(props, '$.name'), -2) = 'da'"
        );
        assert_eq!(
            sqlite_sql("MATCH (n:Person) WHERE n.name STARTS WITH 'Ad' RETURN n"),
            "SELECT id, label, props FROM \"grust_nodes\" \
             WHERE label = 'Person' AND instr(json_extract(props, '$.name'), 'Ad') = 1"
        );
    }

    #[test]
    fn arithmetic_comparison() {
        let plan = plan_node_read_with_hints(
            "MATCH (n:Person) WHERE n.age + 1 > 40 RETURN n.name",
            &params(),
            &TestHints,
        )
        .unwrap()
        .expect("pushable");
        assert_eq!(
            plan.to_sql(&SparkDialect),
            "SELECT id, label, props FROM `grust_nodes` WHERE label = 'Person' \
             AND (CAST(GET_JSON_OBJECT(props, '$.age') AS BIGINT) + 1) > 40"
        );
        // Float property * literal, SQLite casts.
        let f = plan_node_read_with_hints(
            "MATCH (n:Person) WHERE n.score * 2 >= 15.0 RETURN n.name",
            &params(),
            &TestHints,
        )
        .unwrap()
        .expect("pushable")
        .to_sql(&SqliteDialect);
        assert!(
            f.contains("(CAST(json_extract(props, '$.score') AS REAL) * 2) >= 15.0"),
            "{f}"
        );
        // Without type hints the property type is unknown → not pushable.
        assert!(plan_node_read("MATCH (n:Person) WHERE n.age + 1 > 40 RETURN n", &params())
            .unwrap()
            .is_none());
        // Division renders as floating-point division (reference `/` is f64).
        let d = plan_node_read_with_hints(
            "MATCH (n:Person) WHERE n.age / 2 > 20 RETURN n.name",
            &params(),
            &TestHints,
        )
        .unwrap()
        .expect("pushable")
        .to_sql(&SqliteDialect);
        assert!(
            d.contains(
                "(CAST(CAST(json_extract(props, '$.age') AS INTEGER) AS REAL) / CAST(2 AS REAL)) > 20"
            ),
            "{d}"
        );
        // Modulo / power remain dialect-divergent → not pushable.
        assert!(plan_node_read_with_hints(
            "MATCH (n:Person) WHERE n.age % 2 = 0 RETURN n",
            &params(),
            &TestHints,
        )
        .unwrap()
        .is_none());
        // String property in arithmetic → not pushable.
        assert!(plan_node_read_with_hints(
            "MATCH (n:Person) WHERE n.name + 1 > 40 RETURN n",
            &params(),
            &TestHints,
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn boolean_comparison() {
        // SQLite json_extract yields integer 1/0.
        assert_eq!(
            sqlite_sql("MATCH (n:Person) WHERE n.active = true RETURN n.name"),
            "SELECT id, label, props FROM \"grust_nodes\" \
             WHERE label = 'Person' AND json_extract(props, '$.active') = 1"
        );
        // Spark GET_JSON_OBJECT yields the boolean as text.
        assert_eq!(
            spark_sql("MATCH (n:Person) WHERE n.active <> false RETURN n.name"),
            "SELECT id, label, props FROM `grust_nodes` \
             WHERE label = 'Person' AND GET_JSON_OBJECT(props, '$.active') <> 'false'"
        );
    }

    #[test]
    fn segment_string_predicate() {
        assert_eq!(
            seg_spark("MATCH (:Person)-[:KNOWS]->(b) WHERE b.name CONTAINS 'ra' RETURN b.name"),
            "SELECT n1.id, n1.label, n1.props \
             FROM `grust_nodes` n0 \
             JOIN `grust_edges` e0 ON e0.src_id = n0.id \
             JOIN `grust_nodes` n1 ON n1.id = e0.dst_id \
             WHERE e0.edge_type = 'KNOWS' AND n0.label = 'Person' \
             AND CONTAINS(GET_JSON_OBJECT(n1.props, '$.name'), 'ra')"
        );
    }

    #[test]
    fn literal_on_left_flips_operator() {
        assert_eq!(
            spark_sql("MATCH (n:Person) WHERE 40 <= n.age RETURN n"),
            "SELECT id, label, props FROM `grust_nodes` \
             WHERE label = 'Person' AND CAST(GET_JSON_OBJECT(props, '$.age') AS BIGINT) >= 40"
        );
    }

    #[test]
    fn parameter_resolves_to_literal() {
        let mut p = params();
        p.insert("min".to_string(), Value::Int(40));
        let sql = plan_node_read("MATCH (n:Person) WHERE n.age >= $min RETURN n", &p)
            .unwrap()
            .expect("pushable")
            .to_sql(&SparkDialect);
        assert_eq!(
            sql,
            "SELECT id, label, props FROM `grust_nodes` \
             WHERE label = 'Person' AND CAST(GET_JSON_OBJECT(props, '$.age') AS BIGINT) >= 40"
        );
    }

    #[test]
    fn label_property_uses_column_without_cast() {
        assert_eq!(
            spark_sql("MATCH (n) WHERE n.label = 'Person' RETURN n"),
            "SELECT id, label, props FROM `grust_nodes` WHERE label = 'Person'"
        );
    }

    #[test]
    fn string_literals_are_escaped() {
        assert_eq!(
            spark_sql("MATCH (n) WHERE n.name = 'O\\'Hara' RETURN n"),
            "SELECT id, label, props FROM `grust_nodes` \
             WHERE GET_JSON_OBJECT(props, '$.name') = 'O''Hara'"
        );
    }

    #[test]
    fn unsupported_shapes_fall_back_to_none() {
        // Relationship segment.
        assert!(plan("MATCH (a)-[:KNOWS]->(b) RETURN a").is_none());
        // Variable length.
        assert!(plan("MATCH (a:Person)-[:KNOWS*1..2]->(b) RETURN b").is_none());
        // OPTIONAL MATCH.
        assert!(plan("MATCH (a) OPTIONAL MATCH (b) RETURN a").is_none());
        // WITH horizon.
        assert!(plan("MATCH (n:Person) WITH n RETURN n").is_none());
        // UNWIND.
        assert!(plan("UNWIND [1,2] AS x RETURN x").is_none());
        // UNION.
        assert!(plan("MATCH (n:A) RETURN n.label AS l UNION MATCH (m:B) RETURN m.label AS l")
            .is_none());
        // Empty / mixed-kind IN lists are not pushable (reference fallback).
        assert!(plan("MATCH (n:Person) WHERE n.age IN [] RETURN n").is_none());
        assert!(plan("MATCH (n:Person) WHERE n.age IN [1, 'x'] RETURN n").is_none());
        // Empty needle string predicate is not pushable (dialect edge cases).
        assert!(plan("MATCH (n:Person) WHERE n.name STARTS WITH '' RETURN n").is_none());
        // Function in WHERE.
        assert!(plan("MATCH (n:Person) WHERE toUpper(n.name) = 'A' RETURN n").is_none());
        // Property-to-property comparison.
        assert!(plan("MATCH (n:Person) WHERE n.age = n.height RETURN n").is_none());
        // Arithmetic predicate.
        assert!(plan("MATCH (n:Person) WHERE n.age + 1 > 40 RETURN n").is_none());
        // Multi-label pattern.
        assert!(plan("MATCH (n:Person:Admin) RETURN n").is_none());
        // Path variable.
        assert!(plan("MATCH p = (n:Person) RETURN n").is_none());
    }

    #[test]
    fn writes_are_not_pushable() {
        assert!(plan("CREATE (:Person {id:'x'})").is_none());
    }

    fn node(label: &str, id: &str, props: &[(&str, Value)]) -> Node {
        let mut p = Props::new();
        for (k, v) in props {
            p.insert((*k).to_string(), v.clone());
        }
        Node::new(label, id, p)
    }

    #[test]
    fn project_runs_the_reference_projection() {
        // The pushdown projection must equal the reference over the same nodes.
        let nodes = vec![
            node("Person", "p2", &[("name", Value::from("Alan")), ("age", Value::Int(41))]),
            node("Person", "p3", &[("name", Value::from("Grace")), ("age", Value::Int(85))]),
        ];
        let plan = plan("MATCH (n:Person) WHERE n.age >= 40 RETURN n.name ORDER BY n.name")
            .expect("pushable");
        // SparkDialect does not push ordering, so the Rust projection sorts.
        let table = plan.project(&SparkDialect, nodes, &params()).unwrap();
        assert_eq!(table.columns, vec!["n.name".to_string()]);
        assert_eq!(
            table.rows,
            vec![vec![Value::from("Alan")], vec![Value::from("Grace")]]
        );
    }

    #[test]
    fn project_supports_aggregates_and_distinct() {
        let nodes = vec![
            node("Person", "p1", &[("age", Value::Int(40))]),
            node("Person", "p2", &[("age", Value::Int(50))]),
        ];
        let plan = plan("MATCH (n:Person) RETURN count(*) AS c").expect("pushable");
        let table = plan.project(&SparkDialect, nodes, &params()).unwrap();
        assert_eq!(table.columns, vec!["c".to_string()]);
        assert_eq!(table.rows, vec![vec![Value::Int(2)]]);
    }

    #[test]
    fn order_limit_pushed_only_for_typed_dialects() {
        let cypher = "MATCH (n:Person) WHERE n.age >= 40 RETURN n.name ORDER BY n.age DESC SKIP 1 LIMIT 2";
        let plan = plan(cypher).expect("pushable");
        // SQLite extracts typed JSON → ORDER BY/SKIP/LIMIT pushed with NULLS rules.
        assert_eq!(
            plan.to_sql(&SqliteDialect),
            "SELECT id, label, props FROM \"grust_nodes\" \
             WHERE label = 'Person' AND CAST(json_extract(props, '$.age') AS INTEGER) >= 40 \
             ORDER BY json_extract(props, '$.age') DESC NULLS FIRST LIMIT 2 OFFSET 1"
        );
        assert!(plan.pushes_ordering(&SqliteDialect));
        // Spark's GET_JSON_OBJECT returns text → ordering stays in the reference.
        assert!(!plan.to_sql(&SparkDialect).contains("ORDER BY"));
        assert!(!plan.pushes_ordering(&SparkDialect));
    }

    struct TestHints;
    impl TypeHints for TestHints {
        fn node_property_kind(&self, label: Option<&str>, key: &str) -> Option<ScalarKind> {
            match (label, key) {
                (Some("Person"), "age") => Some(ScalarKind::Int),
                (Some("Person"), "score") => Some(ScalarKind::Float),
                (Some("Person"), "name") => Some(ScalarKind::Str),
                _ => None,
            }
        }
        fn edge_property_kind(&self, edge_type: Option<&str>, key: &str) -> Option<ScalarKind> {
            match (edge_type, key) {
                (Some("RATED"), "stars") => Some(ScalarKind::Int),
                _ => None,
            }
        }
    }

    #[test]
    fn schema_hints_let_spark_push_numeric_ordering() {
        let plan = plan_node_read_with_hints(
            "MATCH (n:Person) RETURN n.name ORDER BY n.age DESC LIMIT 5",
            &params(),
            &TestHints,
        )
        .unwrap()
        .expect("pushable");
        // With a known Int type, Spark casts the JSON sort key so it orders
        // numerically (not lexicographically).
        assert!(plan.pushes_ordering(&SparkDialect));
        assert_eq!(
            plan.to_sql(&SparkDialect),
            "SELECT id, label, props FROM `grust_nodes` WHERE label = 'Person' \
             ORDER BY CAST(GET_JSON_OBJECT(props, '$.age') AS BIGINT) DESC NULLS FIRST LIMIT 5"
        );
        // An unknown property type is still not pushable on Spark.
        let plan = plan_node_read_with_hints(
            "MATCH (n:Person) RETURN n.name ORDER BY n.height",
            &params(),
            &TestHints,
        )
        .unwrap()
        .expect("pushable");
        assert!(!plan.pushes_ordering(&SparkDialect));
    }

    #[test]
    fn order_by_alias_and_label_are_pushable() {
        // ORDER BY an output alias that resolves to a scan-var property.
        let sql = plan("MATCH (n:Person) RETURN n.age AS a ORDER BY a")
            .expect("pushable")
            .to_sql(&SqliteDialect);
        assert!(sql.ends_with("ORDER BY json_extract(props, '$.age') ASC NULLS LAST"), "{sql}");
        // ORDER BY label uses the column directly.
        let sql = plan("MATCH (n) RETURN n.label ORDER BY n.label")
            .expect("pushable")
            .to_sql(&SqliteDialect);
        assert!(sql.ends_with("ORDER BY label ASC NULLS LAST"), "{sql}");
    }

    #[test]
    fn non_pushable_ordering_stays_in_rust() {
        // Aggregate, DISTINCT, computed ORDER key, and bare LIMIT are not pushed.
        for cypher in [
            "MATCH (n:Person) RETURN count(*) AS c ORDER BY c",
            "MATCH (n:Person) RETURN DISTINCT n.age AS a ORDER BY a",
            "MATCH (n:Person) RETURN n.age AS a ORDER BY n.age + 1",
            "MATCH (n:Person) RETURN n.name LIMIT 3",
        ] {
            let p = plan(cypher).expect("node-pushable");
            assert!(!p.pushes_ordering(&SqliteDialect), "should not push: {cypher}");
        }
    }

    // ---- relationship segment pushdown ------------------------------------

    fn seg_plan(cypher: &str) -> Option<SegmentReadPushdown> {
        plan_segment_read(cypher, &params()).unwrap()
    }

    fn seg_spark(cypher: &str) -> String {
        seg_plan(cypher).expect("pushable").to_sql(&SparkDialect)
    }

    #[test]
    fn outgoing_segment_join() {
        assert_eq!(
            seg_spark("MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN b.name"),
            "SELECT n0.id, n0.label, n0.props, n1.id, n1.label, n1.props \
             FROM `grust_nodes` n0 \
             JOIN `grust_edges` e0 ON e0.src_id = n0.id \
             JOIN `grust_nodes` n1 ON n1.id = e0.dst_id \
             WHERE e0.edge_type = 'KNOWS' AND n0.label = 'Person' AND n1.label = 'Person'"
        );
    }

    #[test]
    fn two_segment_path_join() {
        // Friend-of-friend: a chained join over two segments.
        assert_eq!(
            seg_spark("MATCH (a:Person)-[:KNOWS]->()-[:KNOWS]->(c) RETURN a.name, c.name"),
            "SELECT n0.id, n0.label, n0.props, n2.id, n2.label, n2.props \
             FROM `grust_nodes` n0 \
             JOIN `grust_edges` e0 ON e0.src_id = n0.id \
             JOIN `grust_nodes` n1 ON n1.id = e0.dst_id \
             JOIN `grust_edges` e1 ON e1.src_id = n1.id \
             JOIN `grust_nodes` n2 ON n2.id = e1.dst_id \
             WHERE e0.edge_type = 'KNOWS' AND e1.edge_type = 'KNOWS' AND n0.label = 'Person'"
        );
    }

    #[test]
    fn incoming_segment_with_rel_var_and_where() {
        assert_eq!(
            seg_spark("MATCH (a:Person)<-[r:KNOWS]-(b) WHERE b.age > 30 RETURN a.name"),
            "SELECT n0.id, n0.label, n0.props, e0.id, e0.src_id, e0.dst_id, e0.edge_type, e0.props, \
             n1.id, n1.label, n1.props \
             FROM `grust_nodes` n0 \
             JOIN `grust_edges` e0 ON e0.dst_id = n0.id \
             JOIN `grust_nodes` n1 ON n1.id = e0.src_id \
             WHERE e0.edge_type = 'KNOWS' AND n0.label = 'Person' \
             AND CAST(GET_JSON_OBJECT(n1.props, '$.age') AS BIGINT) > 30"
        );
    }

    #[test]
    fn segment_multiple_rel_types_and_inline_props() {
        assert_eq!(
            seg_spark("MATCH (a {city:'London'})-[:KNOWS|FOLLOWS]->(b) RETURN a.name, b.name"),
            "SELECT n0.id, n0.label, n0.props, n1.id, n1.label, n1.props \
             FROM `grust_nodes` n0 \
             JOIN `grust_edges` e0 ON e0.src_id = n0.id \
             JOIN `grust_nodes` n1 ON n1.id = e0.dst_id \
             WHERE e0.edge_type IN ('KNOWS', 'FOLLOWS') \
             AND GET_JSON_OBJECT(n0.props, '$.city') = 'London'"
        );
    }

    #[test]
    fn segment_sqlite_dialect() {
        assert_eq!(
            seg_plan("MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN b.name")
                .unwrap()
                .to_sql(&SqliteDialect),
            "SELECT n0.id, n0.label, n0.props, n1.id, n1.label, n1.props \
             FROM \"grust_nodes\" n0 \
             JOIN \"grust_edges\" e0 ON e0.src_id = n0.id \
             JOIN \"grust_nodes\" n1 ON n1.id = e0.dst_id \
             WHERE e0.edge_type = 'KNOWS' AND n0.label = 'Person' AND n1.label = 'Person'"
        );
    }

    #[test]
    fn segment_edge_property_filter() {
        // Anonymous source endpoint: only `r` and `b` are selected.
        assert_eq!(
            seg_spark("MATCH ()-[r:RATED]->(b) WHERE r.stars >= 4 RETURN b.name"),
            "SELECT e0.id, e0.src_id, e0.dst_id, e0.edge_type, e0.props, n1.id, n1.label, n1.props \
             FROM `grust_nodes` n0 \
             JOIN `grust_edges` e0 ON e0.src_id = n0.id \
             JOIN `grust_nodes` n1 ON n1.id = e0.dst_id \
             WHERE e0.edge_type = 'RATED' \
             AND CAST(GET_JSON_OBJECT(e0.props, '$.stars') AS BIGINT) >= 4"
        );
    }

    #[test]
    fn segment_in_predicate() {
        assert_eq!(
            seg_spark("MATCH (:Person)-[:KNOWS]->(b) WHERE b.name IN ['Alan', 'Grace'] RETURN b.name"),
            "SELECT n1.id, n1.label, n1.props \
             FROM `grust_nodes` n0 \
             JOIN `grust_edges` e0 ON e0.src_id = n0.id \
             JOIN `grust_nodes` n1 ON n1.id = e0.dst_id \
             WHERE e0.edge_type = 'KNOWS' AND n0.label = 'Person' \
             AND GET_JSON_OBJECT(n1.props, '$.name') IN ('Alan', 'Grace')"
        );
    }

    #[test]
    fn segment_order_limit_pushed_for_typed_dialect() {
        let plan = seg_plan(
            "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN b.name ORDER BY b.age DESC LIMIT 3",
        )
        .expect("pushable");
        assert!(plan.pushes_ordering(&SqliteDialect));
        assert!(plan
            .to_sql(&SqliteDialect)
            .ends_with("ORDER BY json_extract(n1.props, '$.age') DESC NULLS FIRST LIMIT 3"));
        // Spark without hints can't type the JSON sort key → not pushed.
        assert!(!plan.pushes_ordering(&SparkDialect));
    }

    #[test]
    fn segment_order_pushed_for_spark_with_hints() {
        let plan = plan_segment_read_with_hints(
            "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN b.name ORDER BY b.age DESC LIMIT 3",
            &params(),
            &TestHints,
        )
        .unwrap()
        .expect("pushable");
        assert!(plan.pushes_ordering(&SparkDialect));
        assert!(plan.to_sql(&SparkDialect).ends_with(
            "ORDER BY CAST(GET_JSON_OBJECT(n1.props, '$.age') AS BIGINT) DESC NULLS FIRST LIMIT 3"
        ));
    }

    #[test]
    fn segment_edge_property_ordering_with_hints() {
        // A single rel type lets an edge-property sort key be typed on Spark.
        let plan = plan_segment_read_with_hints(
            "MATCH (a)-[r:RATED]->(b) RETURN b.name ORDER BY r.stars DESC",
            &params(),
            &TestHints,
        )
        .unwrap()
        .expect("pushable");
        assert!(plan.pushes_ordering(&SparkDialect));
        assert!(plan.to_sql(&SparkDialect).ends_with(
            "ORDER BY CAST(GET_JSON_OBJECT(e0.props, '$.stars') AS BIGINT) DESC NULLS FIRST"
        ));
        // Typed-JSON dialects order the edge property directly (no hints needed).
        assert!(plan
            .to_sql(&SqliteDialect)
            .ends_with("ORDER BY json_extract(e0.props, '$.stars') DESC NULLS FIRST"));
    }

    #[test]
    fn undirected_segment_join() {
        // Either orientation matches; the OR join reproduces both like the reference.
        assert_eq!(
            seg_spark("MATCH (a:Person)-[:KNOWS]-(b:Person) RETURN a.name, b.name"),
            "SELECT n0.id, n0.label, n0.props, n1.id, n1.label, n1.props \
             FROM `grust_nodes` n0 \
             JOIN `grust_edges` e0 ON (e0.src_id = n0.id OR e0.dst_id = n0.id) \
             JOIN `grust_nodes` n1 ON ((e0.src_id = n0.id AND n1.id = e0.dst_id) \
             OR (e0.dst_id = n0.id AND n1.id = e0.src_id)) \
             WHERE e0.edge_type = 'KNOWS' AND n0.label = 'Person' AND n1.label = 'Person'"
        );
    }

    #[test]
    fn segment_arithmetic_comparison() {
        let sql = plan_segment_read_with_hints(
            "MATCH (a:Person)-[:KNOWS]->(b:Person) WHERE b.age + 1 > 40 RETURN b.name",
            &params(),
            &TestHints,
        )
        .unwrap()
        .expect("pushable")
        .to_sql(&SparkDialect);
        assert!(
            sql.ends_with("AND (CAST(GET_JSON_OBJECT(n1.props, '$.age') AS BIGINT) + 1) > 40"),
            "{sql}"
        );
        // Edge-property arithmetic.
        let e = plan_segment_read_with_hints(
            "MATCH (a)-[r:RATED]->(b) WHERE r.stars * 2 >= 8 RETURN b.name",
            &params(),
            &TestHints,
        )
        .unwrap()
        .expect("pushable")
        .to_sql(&SqliteDialect);
        assert!(
            e.contains("(CAST(json_extract(e0.props, '$.stars') AS INTEGER) * 2) >= 8"),
            "{e}"
        );
        // No hints → unknown types → not pushable.
        assert!(plan_segment_read(
            "MATCH (a:Person)-[:KNOWS]->(b:Person) WHERE b.age + 1 > 40 RETURN b.name",
            &params(),
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn segment_unsupported_shapes_fall_back() {
        // Variable length.
        assert!(seg_plan("MATCH (a)-[:KNOWS*1..2]->(b) RETURN a").is_none());
        // Repeated variable across positions (the reference would equate them).
        assert!(seg_plan("MATCH (a)-[:KNOWS]->(a) RETURN a").is_none());
        assert!(seg_plan("MATCH (a)-[:KNOWS]->(b)-[:KNOWS]->(a) RETURN a").is_none());
        // Path variable.
        assert!(seg_plan("MATCH p = (a)-[:KNOWS]->(b) RETURN a").is_none());
        // Plain node pattern (handled by plan_node_read, not the segment planner).
        assert!(seg_plan("MATCH (n:Person) RETURN n").is_none());
        // Property-to-property comparison (neither side is a literal).
        assert!(seg_plan("MATCH (a)-[:KNOWS]->(b) WHERE a.age = b.age RETURN a").is_none());
    }

    #[test]
    fn segment_reconstructs_and_projects() {
        let plan =
            seg_plan("MATCH (a:Person)-[r:KNOWS]->(b:Person) RETURN a.name, r.since, b.name")
                .expect("pushable");
        // Columns: n0(3) + e0(5) + n1(3) = 11.
        assert_eq!(plan.column_count(), 11);
        let row = vec![
            some("p1"),
            some("Person"),
            some("{\"name\":\"Ada\"}"),
            None, // edge id
            some("p1"),
            some("p2"),
            some("KNOWS"),
            some("{\"since\":2020}"),
            some("p2"),
            some("Person"),
            some("{\"name\":\"Alan\"}"),
        ];
        let table = plan
            .project_text_rows(&SparkDialect, vec![row], &params())
            .unwrap();
        assert_eq!(
            table.columns,
            vec!["a.name".to_string(), "r.since".to_string(), "b.name".to_string()]
        );
        assert_eq!(
            table.rows,
            vec![vec![Value::from("Ada"), Value::Int(2020), Value::from("Alan")]]
        );
    }

    fn some(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    // ---- UNION composition ------------------------------------------------

    #[test]
    fn union_composition() {
        let plan = plan_read(
            "MATCH (n:Person) RETURN n.name AS x UNION MATCH (c:City) RETURN c.name AS x",
            &params(),
            &NoTypeHints,
        )
        .unwrap()
        .expect("pushable");
        let (arms, distinct) = plan.union_arms().expect("union");
        assert_eq!(arms.len(), 2);
        assert!(distinct);
        assert_eq!(
            arms[0].to_sql(&SqliteDialect),
            "SELECT id, label, props FROM \"grust_nodes\" WHERE label = 'Person'"
        );
        // UNION ALL → not distinct; arms may be different leaf kinds.
        let all = plan_read(
            "MATCH (n:Person) RETURN n.name AS x \
             UNION ALL MATCH (:Person)-[:KNOWS]->(b) RETURN b.name AS x",
            &params(),
            &NoTypeHints,
        )
        .unwrap()
        .expect("pushable");
        let (arms, distinct) = all.union_arms().unwrap();
        assert_eq!(arms.len(), 2);
        assert!(!distinct);
        assert!(matches!(arms[0], ReadPushdown::Node(_)));
        assert!(matches!(arms[1], ReadPushdown::Segment(_)));
        // A single query is a leaf, not a union.
        assert!(!plan_read("MATCH (n:Person) RETURN n.name", &params(), &NoTypeHints)
            .unwrap()
            .unwrap()
            .is_union());
        // A non-pushable arm makes the whole UNION fall back.
        assert!(plan_read(
            "MATCH (n:Person) RETURN n.name AS x UNION UNWIND [1] AS x RETURN x",
            &params(),
            &NoTypeHints,
        )
        .unwrap()
        .is_none());
    }

    // ---- WITH horizon -----------------------------------------------------

    #[test]
    fn with_horizon_pipeline() {
        let plan = plan_read(
            "MATCH (n:Person) WHERE n.age >= 40 WITH n.age AS age RETURN avg(age) AS mean",
            &params(),
            &NoTypeHints,
        )
        .unwrap()
        .expect("pushable");
        assert!(matches!(plan, ReadPushdown::Pipeline(_)));
        // Only the leading MATCH scan + filter is pushed; the horizon runs in Rust.
        assert_eq!(
            plan.to_sql(&SqliteDialect),
            "SELECT id, label, props FROM \"grust_nodes\" \
             WHERE label = 'Person' AND CAST(json_extract(props, '$.age') AS INTEGER) >= 40"
        );
        // A tail containing a further MATCH (needs graph access) falls back.
        assert!(plan_read(
            "MATCH (n:Person) WITH n MATCH (n)-[:KNOWS]->(b) RETURN b.name",
            &params(),
            &NoTypeHints,
        )
        .unwrap()
        .is_none());
    }

    // ---- multi-pattern MATCH ----------------------------------------------

    #[test]
    fn multi_pattern_shared_var_and_cross() {
        // Shared variable `a` across two patterns → one alias, joined.
        let plan = plan_read(
            "MATCH (a:Person)-[:KNOWS]->(b), (a)-[:RATED]->(c) RETURN a.name, b.name, c.name",
            &params(),
            &NoTypeHints,
        )
        .unwrap()
        .expect("pushable");
        assert!(matches!(plan, ReadPushdown::MultiPattern(_)));
        let sql = plan.to_sql(&SqliteDialect);
        assert!(
            sql.contains(
                "FROM \"grust_nodes\" n0, \"grust_nodes\" n1, \"grust_nodes\" n2, \
                 \"grust_edges\" e0, \"grust_edges\" e1"
            ),
            "{sql}"
        );
        assert!(sql.contains("e0.src_id = n0.id AND e0.dst_id = n1.id"), "{sql}");
        assert!(sql.contains("e1.src_id = n0.id AND e1.dst_id = n2.id"), "{sql}");
        assert!(
            sql.contains("e0.edge_type = 'KNOWS'") && sql.contains("e1.edge_type = 'RATED'"),
            "{sql}"
        );

        // Cross product (no shared variable).
        let cross =
            plan_read("MATCH (a:Person), (c:City) RETURN a.name, c.name", &params(), &NoTypeHints)
                .unwrap()
                .expect("pushable")
                .to_sql(&SqliteDialect);
        assert!(
            cross.contains(
                "FROM \"grust_nodes\" n0, \"grust_nodes\" n1 \
                 WHERE n0.label = 'Person' AND n1.label = 'City'"
            ),
            "{cross}"
        );

        // Undirected segment in a multi-pattern falls back.
        assert!(plan_read(
            "MATCH (a)-[:KNOWS]->(b), (a)-[:KNOWS]-(c) RETURN a",
            &params(),
            &NoTypeHints,
        )
        .unwrap()
        .is_none());
    }

    // ---- OPTIONAL MATCH ---------------------------------------------------

    #[test]
    fn optional_match_left_join() {
        let plan = plan_read(
            "MATCH (a:Person) OPTIONAL MATCH (a)-[:KNOWS]->(b:Person) RETURN a.name, b.name",
            &params(),
            &NoTypeHints,
        )
        .unwrap()
        .expect("pushable");
        assert!(matches!(plan, ReadPushdown::Optional(_)));
        let sql = plan.to_sql(&SqliteDialect);
        assert!(sql.starts_with("SELECT n0.id, n0.label, n0.props, opt.b_id, opt.b_label, opt.b_props \
             FROM \"grust_nodes\" n0 LEFT JOIN ("), "{sql}");
        assert!(
            sql.contains("FROM \"grust_edges\" e0 JOIN \"grust_nodes\" n1 ON n1.id = e0.dst_id"),
            "{sql}"
        );
        assert!(sql.contains("WHERE e0.edge_type = 'KNOWS' AND n1.label = 'Person'"), "{sql}");
        assert!(sql.ends_with(") opt ON opt.anchor = n0.id WHERE n0.label = 'Person'"), "{sql}");

        // OPTIONAL WHERE referencing the mandatory node `a` is not pushable.
        assert!(plan_read(
            "MATCH (a:Person) OPTIONAL MATCH (a)-[:KNOWS]->(b) WHERE b.age > a.age RETURN a.name",
            &params(),
            &NoTypeHints,
        )
        .unwrap()
        .is_none());
    }

    // ---- variable-length path pushdown ------------------------------------

    fn varlen(cypher: &str) -> Option<VarLengthReadPushdown> {
        plan_var_length_read(cypher, &params()).unwrap()
    }

    #[test]
    fn var_length_recursive_cte() {
        let sql = varlen("MATCH (a:Person {name:'Ada'})-[:KNOWS*1..2]->(b) RETURN b.name")
            .expect("pushable")
            .to_sql(&SqliteDialect);
        assert!(
            sql.starts_with("WITH RECURSIVE walk(s, e, depth, visited) AS ("),
            "{sql}"
        );
        // Outgoing step + simple-path (no-repeat) membership via instr.
        assert!(sql.contains("JOIN \"grust_edges\" ed ON ed.src_id = w.e"), "{sql}");
        assert!(sql.contains("WHERE instr(w.visited,"), "{sql}");
        assert!(sql.contains("AND ed.edge_type = 'KNOWS'"), "{sql}");
        assert!(sql.contains("AND w.depth + 1 <= 2"), "{sql}");
        assert!(sql.contains(" WHERE w.depth >= 1"), "{sql}");
        assert!(sql.contains("json_extract(n0.props, '$.name') = 'Ada'"), "{sql}");
        assert!(sql.contains("n0.label = 'Person'"), "{sql}");

        // Incoming and undirected step expressions.
        let inc = varlen("MATCH (a)<-[:KNOWS*1..3]-(b) RETURN b.name")
            .expect("pushable")
            .to_sql(&SqliteDialect);
        assert!(inc.contains("JOIN \"grust_edges\" ed ON ed.dst_id = w.e"), "{inc}");
        assert!(inc.contains("AND w.depth + 1 <= 3"), "{inc}");
        let und = varlen("MATCH (a)-[:KNOWS*2..3]-(b) RETURN b.name")
            .expect("pushable")
            .to_sql(&SqliteDialect);
        assert!(
            und.contains("CASE WHEN ed.src_id = w.e THEN ed.dst_id ELSE ed.src_id END"),
            "{und}"
        );
        assert!(und.contains(" WHERE w.depth >= 2"), "{und}");

        // Spark renders the same recursive shape with GET_JSON_OBJECT.
        let spark = varlen("MATCH (a:Person {name:'Ada'})-[:KNOWS*1..2]->(b) RETURN b.name")
            .expect("pushable")
            .to_sql(&SparkDialect);
        assert!(spark.contains("GET_JSON_OBJECT(n0.props, '$.name') = 'Ada'"), "{spark}");
    }

    #[test]
    fn var_length_unsupported_shapes_fall_back() {
        // Named relationship binds an edge list — not reconstructed here.
        assert!(varlen("MATCH (a)-[r:KNOWS*1..2]->(b) RETURN b").is_none());
        // Fixed-length segment is handled by the segment planner, not here.
        assert!(varlen("MATCH (a)-[:KNOWS]->(b) RETURN b").is_none());
        // Plain node pattern.
        assert!(varlen("MATCH (n) RETURN n").is_none());
        // Path variable.
        assert!(varlen("MATCH p = (a)-[:KNOWS*1..2]->(b) RETURN b").is_none());
    }
}
