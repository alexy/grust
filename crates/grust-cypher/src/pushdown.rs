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
    Ok(lower_query(&query, params, hints))
}

fn lower_query(
    query: &Query,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Option<NodeReadPushdown> {
    // Milestone 1: a single, non-UNION query.
    if query.parts.len() != 1 || query.parts[0].union.is_some() {
        return None;
    }
    let single = &query.parts[0].query;

    // Exactly `MATCH … RETURN …`.
    let (match_clause, return_clause) = match single.clauses.as_slice() {
        [Clause::Match(m), Clause::Return(r)] if !m.optional => (m, r),
        _ => return None,
    };

    // Exactly one node-only pattern with a bound variable, at most one label,
    // and no path variable.
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

    // Inline property equalities, then the WHERE predicate, all anchored on `var`.
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
        let predicate = lower_predicate(where_expr, &var, params)?;
        filter = Some(conjoin(filter, predicate));
    }

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
/// if any part is outside the milestone-1 subset.
fn lower_predicate(expr: &Expr, var: &str, params: &CypherParameters) -> Option<Predicate> {
    match expr {
        Expr::Binary {
            op: BinaryOp::And,
            lhs,
            rhs,
        } => Some(Predicate::And(
            Box::new(lower_predicate(lhs, var, params)?),
            Box::new(lower_predicate(rhs, var, params)?),
        )),
        Expr::Binary {
            op: BinaryOp::Or,
            lhs,
            rhs,
        } => Some(Predicate::Or(
            Box::new(lower_predicate(lhs, var, params)?),
            Box::new(lower_predicate(rhs, var, params)?),
        )),
        Expr::Unary {
            op: UnaryOp::Not,
            operand,
        } => Some(Predicate::Not(Box::new(lower_predicate(
            operand, var, params,
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
        Expr::Binary { op, lhs, rhs } => {
            let cmp = lower_cmp_op(*op)?;
            // Accept `prop <op> literal` or `literal <op> prop`.
            if let Some(prop) = lower_prop_ref(lhs, var) {
                let value = lower_scalar(rhs, params)?;
                Some(Predicate::Compare {
                    prop,
                    op: cmp,
                    value,
                })
            } else if let Some(prop) = lower_prop_ref(rhs, var) {
                let value = lower_scalar(lhs, params)?;
                Some(Predicate::Compare {
                    prop,
                    op: cmp.flipped(),
                    value,
                })
            } else {
                None
            }
        }
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

/// Which endpoint of the segment a property/label refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Endpoint {
    A,
    B,
}

/// An operand of a segment filter: a node label/property of an endpoint, or an
/// edge property of the relationship.
#[derive(Clone, Debug, PartialEq)]
enum SegOperand {
    NodeLabel(Endpoint),
    NodeProp(Endpoint, String),
    EdgeProp(String),
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
}

/// One binding the segment query selects and reconstructs, in SELECT-column order.
#[derive(Clone, Debug, PartialEq)]
enum SelectedBinding {
    /// Endpoint node bound to `var` (3 columns: id, label, props).
    Node { var: String, endpoint: Endpoint },
    /// The relationship edge bound to `var` (5 columns: id, src, dst, type, props).
    Edge { var: String },
}

/// A lowered single directed relationship segment.
#[derive(Clone, Debug, PartialEq)]
pub struct SegmentReadPushdown {
    /// `na`-side endpoint and `nb`-side endpoint join targets depend on direction.
    incoming: bool,
    a_label: Option<String>,
    b_label: Option<String>,
    rel_types: Vec<String>,
    filter: Option<SegPredicate>,
    /// Selected bindings in SELECT order (only pattern vars that are bound).
    selected: Vec<SelectedBinding>,
    /// `ORDER BY`/`SKIP`/`LIMIT` lowered for SQL pushdown, when structurally
    /// pushable (same rules as the node path; keys reference `a`/`r`/`b`).
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
    Ok(lower_segment(&query, params, hints))
}

fn lower_segment(
    query: &Query,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Option<SegmentReadPushdown> {
    if query.parts.len() != 1 || query.parts[0].union.is_some() {
        return None;
    }
    let single = &query.parts[0].query;
    let (match_clause, return_clause) = match single.clauses.as_slice() {
        [Clause::Match(m), Clause::Return(r)] if !m.optional => (m, r),
        _ => return None,
    };
    if match_clause.patterns.len() != 1 {
        return None;
    }
    let pattern = &match_clause.patterns[0];
    if pattern.variable.is_some() || pattern.segments.len() != 1 {
        return None;
    }
    let a = &pattern.start;
    let segment = &pattern.segments[0];
    let rel = &segment.relationship;
    let b = &segment.node;

    // Direction: only directed segments for now.
    let incoming = match rel.direction {
        Direction::Outgoing => false,
        Direction::Incoming => true,
        Direction::Undirected => return None,
    };
    // No variable-length, no relationship var-length bound.
    if rel.length.is_some() {
        return None;
    }
    // At most one label per endpoint.
    if a.labels.len() > 1 || b.labels.len() > 1 {
        return None;
    }

    // Variable identities. Endpoints must differ to keep the join unambiguous.
    let a_var = a.variable.clone();
    let b_var = b.variable.clone();
    let rel_var = rel.variable.clone();
    if let (Some(av), Some(bv)) = (&a_var, &b_var) {
        if av == bv {
            return None;
        }
    }
    // A relationship variable that collides with a node variable is rejected.
    if let Some(rv) = &rel_var {
        if Some(rv) == a_var.as_ref() || Some(rv) == b_var.as_ref() {
            return None;
        }
    }

    // Build the filter: inline endpoint props, inline rel props, then WHERE.
    let mut filter: Option<SegPredicate> = None;
    if let Some(map) = &a.properties {
        for (key, value) in &map.entries {
            filter = Some(seg_conjoin(filter, seg_inline(Endpoint::A, key, value, params)?));
        }
    }
    if let Some(map) = &b.properties {
        for (key, value) in &map.entries {
            filter = Some(seg_conjoin(filter, seg_inline(Endpoint::B, key, value, params)?));
        }
    }
    if let Some(map) = &rel.properties {
        for (key, value) in &map.entries {
            let scalar = lower_scalar(value, params)?;
            let operand = SegOperand::EdgeProp(lower_seg_key(key)?);
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
    if let Some(where_expr) = &match_clause.where_clause {
        let resolver = VarRoles {
            a: a_var.as_deref(),
            b: b_var.as_deref(),
            rel: rel_var.as_deref(),
        };
        filter = Some(seg_conjoin(filter, lower_seg_predicate(where_expr, &resolver, params)?));
    }

    // Selected bindings, in SELECT order a, r, b — only those with a variable.
    let mut selected = Vec::new();
    if let Some(var) = &a_var {
        selected.push(SelectedBinding::Node {
            var: var.clone(),
            endpoint: Endpoint::A,
        });
    }
    if let Some(var) = &rel_var {
        selected.push(SelectedBinding::Edge { var: var.clone() });
    }
    if let Some(var) = &b_var {
        selected.push(SelectedBinding::Node {
            var: var.clone(),
            endpoint: Endpoint::B,
        });
    }

    let a_label = a.labels.first().cloned();
    let b_label = b.labels.first().cloned();
    let roles = VarRoles {
        a: a_var.as_deref(),
        b: b_var.as_deref(),
        rel: rel_var.as_deref(),
    };
    // A single relationship type lets edge-property ordering be typed on untyped
    // dialects; multiple/absent types leave the edge kind unknown.
    let edge_type = match rel.types.as_slice() {
        [one] => Some(one.as_str()),
        _ => None,
    };
    let projection = return_clause.projection.clone();
    let ordering = compute_seg_ordering(
        &projection,
        &roles,
        a_label.as_deref(),
        b_label.as_deref(),
        edge_type,
        params,
        hints,
    );

    Some(SegmentReadPushdown {
        incoming,
        a_label,
        b_label,
        rel_types: rel.types.clone(),
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
    roles: &VarRoles,
    a_label: Option<&str>,
    b_label: Option<&str>,
    edge_type: Option<&str>,
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
            SegOperand::NodeProp(Endpoint::A, key) => hints.node_property_kind(a_label, key),
            SegOperand::NodeProp(Endpoint::B, key) => hints.node_property_kind(b_label, key),
            SegOperand::EdgeProp(key) => hints.edge_property_kind(edge_type, key),
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

/// Variable name → segment role mapping for WHERE lowering.
struct VarRoles<'a> {
    a: Option<&'a str>,
    b: Option<&'a str>,
    rel: Option<&'a str>,
}

fn seg_conjoin(existing: Option<SegPredicate>, next: SegPredicate) -> SegPredicate {
    match existing {
        None => next,
        Some(prev) => SegPredicate::And(Box::new(prev), Box::new(next)),
    }
}

fn seg_inline(
    endpoint: Endpoint,
    key: &str,
    value: &Expr,
    params: &CypherParameters,
) -> Option<SegPredicate> {
    let scalar = lower_scalar(value, params)?;
    let operand = if key == "label" {
        SegOperand::NodeLabel(endpoint)
    } else {
        SegOperand::NodeProp(endpoint, lower_seg_key(key)?)
    };
    Some(SegPredicate::Compare {
        operand,
        op: CmpOp::Eq,
        value: scalar,
    })
}

fn lower_seg_key(key: &str) -> Option<String> {
    if is_safe_key(key) {
        Some(key.to_string())
    } else {
        None
    }
}

fn lower_seg_predicate(
    expr: &Expr,
    roles: &VarRoles,
    params: &CypherParameters,
) -> Option<SegPredicate> {
    match expr {
        Expr::Binary {
            op: BinaryOp::And,
            lhs,
            rhs,
        } => Some(SegPredicate::And(
            Box::new(lower_seg_predicate(lhs, roles, params)?),
            Box::new(lower_seg_predicate(rhs, roles, params)?),
        )),
        Expr::Binary {
            op: BinaryOp::Or,
            lhs,
            rhs,
        } => Some(SegPredicate::Or(
            Box::new(lower_seg_predicate(lhs, roles, params)?),
            Box::new(lower_seg_predicate(rhs, roles, params)?),
        )),
        Expr::Unary {
            op: UnaryOp::Not,
            operand,
        } => Some(SegPredicate::Not(Box::new(lower_seg_predicate(
            operand, roles, params,
        )?))),
        Expr::IsNull { operand, negated } => Some(SegPredicate::IsNull {
            operand: lower_seg_operand(operand, roles)?,
            negated: *negated,
        }),
        Expr::Binary {
            op: BinaryOp::In,
            lhs,
            rhs,
        } => Some(SegPredicate::In {
            operand: lower_seg_operand(lhs, roles)?,
            values: lower_scalar_list(rhs, params)?,
        }),
        Expr::Binary { op, lhs, rhs } => {
            let cmp = lower_cmp_op(*op)?;
            if let Some(operand) = lower_seg_operand(lhs, roles) {
                Some(SegPredicate::Compare {
                    operand,
                    op: cmp,
                    value: lower_scalar(rhs, params)?,
                })
            } else if let Some(operand) = lower_seg_operand(rhs, roles) {
                Some(SegPredicate::Compare {
                    operand,
                    op: cmp.flipped(),
                    value: lower_scalar(lhs, params)?,
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

fn lower_seg_operand(expr: &Expr, roles: &VarRoles) -> Option<SegOperand> {
    let Expr::Property { base, key } = expr else {
        return None;
    };
    let Expr::Variable(name) = base.as_ref() else {
        return None;
    };
    let name = name.as_str();
    if roles.a == Some(name) {
        Some(if key == "label" {
            SegOperand::NodeLabel(Endpoint::A)
        } else {
            SegOperand::NodeProp(Endpoint::A, lower_seg_key(key)?)
        })
    } else if roles.b == Some(name) {
        Some(if key == "label" {
            SegOperand::NodeLabel(Endpoint::B)
        } else {
            SegOperand::NodeProp(Endpoint::B, lower_seg_key(key)?)
        })
    } else if roles.rel == Some(name) {
        // Edges have no synthetic `label`; `r.key` is always a property.
        Some(SegOperand::EdgeProp(lower_seg_key(key)?))
    } else {
        None
    }
}

impl SegmentReadPushdown {
    /// The SQL alias for endpoint A's node table (`na`) / B's (`nb`).
    fn endpoint_alias(endpoint: Endpoint) -> &'static str {
        match endpoint {
            Endpoint::A => "na",
            Endpoint::B => "nb",
        }
    }

    /// The edge column that endpoint A joins on, given direction.
    fn a_edge_col(&self) -> &'static str {
        if self.incoming {
            "dst_id"
        } else {
            "src_id"
        }
    }

    fn b_edge_col(&self) -> &'static str {
        if self.incoming {
            "src_id"
        } else {
            "dst_id"
        }
    }

    /// Render the segment join + filter. The SELECT list emits, in order, the
    /// columns for each selected binding (node: id,label,props; edge:
    /// id,src_id,dst_id,edge_type,props) — all text columns, reconstructed by
    /// [`Self::project_text_rows`].
    pub fn to_sql(&self, dialect: &dyn SqlDialect) -> String {
        let nodes = dialect.quote_ident(dialect.nodes_table());
        let edges = dialect.quote_ident(dialect.edges_table());

        let mut cols: Vec<String> = Vec::new();
        for binding in &self.selected {
            match binding {
                SelectedBinding::Node { endpoint, .. } => {
                    let a = Self::endpoint_alias(*endpoint);
                    cols.push(format!("{a}.id"));
                    cols.push(format!("{a}.label"));
                    cols.push(format!("{a}.props"));
                }
                SelectedBinding::Edge { .. } => {
                    cols.push("re.id".to_string());
                    cols.push("re.src_id".to_string());
                    cols.push("re.dst_id".to_string());
                    cols.push("re.edge_type".to_string());
                    cols.push("re.props".to_string());
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

        let mut sql = format!(
            "SELECT {select_list} FROM {edges} re \
             JOIN {nodes} na ON na.id = re.{a_col} \
             JOIN {nodes} nb ON nb.id = re.{b_col}",
            a_col = self.a_edge_col(),
            b_col = self.b_edge_col(),
        );

        let mut conditions: Vec<String> = Vec::new();
        match self.rel_types.as_slice() {
            [] => {}
            [one] => conditions.push(format!("re.edge_type = {}", dialect.string_literal(one))),
            many => {
                let list = many
                    .iter()
                    .map(|t| dialect.string_literal(t))
                    .collect::<Vec<_>>()
                    .join(", ");
                conditions.push(format!("re.edge_type IN ({list})"));
            }
        }
        if let Some(label) = &self.a_label {
            conditions.push(format!("na.label = {}", dialect.string_literal(label)));
        }
        if let Some(label) = &self.b_label {
            conditions.push(format!("nb.label = {}", dialect.string_literal(label)));
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
        let ordering = self.ordering.as_ref().expect("ordering present");
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
        let mut binding_rows = Vec::with_capacity(rows.len());
        for cells in rows {
            let mut idx = 0;
            let cell = |i: usize| cells.get(i).and_then(|c| c.as_deref());
            let mut bindings = Vec::with_capacity(self.selected.len());
            for binding in &self.selected {
                match binding {
                    SelectedBinding::Node { var, .. } => {
                        let id = cell(idx).unwrap_or_default().to_string();
                        let label = cell(idx + 1).unwrap_or_default().to_string();
                        let props = parse_props(cell(idx + 2))?;
                        idx += 3;
                        bindings.push((
                            var.clone(),
                            PushedBinding::Node(Node {
                                id: NodeId::new(id),
                                label: Label::new(label),
                                props,
                            }),
                        ));
                    }
                    SelectedBinding::Edge { var } => {
                        let id = cell(idx).map(EdgeId::new);
                        let from = cell(idx + 1).unwrap_or_default().to_string();
                        let to = cell(idx + 2).unwrap_or_default().to_string();
                        let label = cell(idx + 3).unwrap_or_default().to_string();
                        let props = parse_props(cell(idx + 4))?;
                        idx += 5;
                        let mut edge = Edge::new(label, from, to, props);
                        edge.id = id;
                        bindings.push((var.clone(), PushedBinding::Edge(edge)));
                    }
                }
            }
            binding_rows.push(bindings);
        }
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
    }
}

fn render_seg_operand(operand: &SegOperand, dialect: &dyn SqlDialect) -> String {
    match operand {
        SegOperand::NodeLabel(endpoint) => {
            format!("{}.label", SegmentReadPushdown::endpoint_alias(*endpoint))
        }
        SegOperand::NodeProp(endpoint, key) => {
            let props = format!("{}.props", SegmentReadPushdown::endpoint_alias(*endpoint));
            dialect.json_property(&props, key)
        }
        SegOperand::EdgeProp(key) => dialect.json_property("re.props", key),
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
        // String predicate.
        assert!(plan("MATCH (n:Person) WHERE n.name STARTS WITH 'A' RETURN n").is_none());
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
            "SELECT na.id, na.label, na.props, nb.id, nb.label, nb.props \
             FROM `grust_edges` re \
             JOIN `grust_nodes` na ON na.id = re.src_id \
             JOIN `grust_nodes` nb ON nb.id = re.dst_id \
             WHERE re.edge_type = 'KNOWS' AND na.label = 'Person' AND nb.label = 'Person'"
        );
    }

    #[test]
    fn incoming_segment_with_rel_var_and_where() {
        assert_eq!(
            seg_spark("MATCH (a:Person)<-[r:KNOWS]-(b) WHERE b.age > 30 RETURN a.name"),
            "SELECT na.id, na.label, na.props, re.id, re.src_id, re.dst_id, re.edge_type, re.props, \
             nb.id, nb.label, nb.props \
             FROM `grust_edges` re \
             JOIN `grust_nodes` na ON na.id = re.dst_id \
             JOIN `grust_nodes` nb ON nb.id = re.src_id \
             WHERE re.edge_type = 'KNOWS' AND na.label = 'Person' \
             AND CAST(GET_JSON_OBJECT(nb.props, '$.age') AS BIGINT) > 30"
        );
    }

    #[test]
    fn segment_multiple_rel_types_and_inline_props() {
        assert_eq!(
            seg_spark("MATCH (a {city:'London'})-[:KNOWS|FOLLOWS]->(b) RETURN a.name, b.name"),
            "SELECT na.id, na.label, na.props, nb.id, nb.label, nb.props \
             FROM `grust_edges` re \
             JOIN `grust_nodes` na ON na.id = re.src_id \
             JOIN `grust_nodes` nb ON nb.id = re.dst_id \
             WHERE re.edge_type IN ('KNOWS', 'FOLLOWS') \
             AND GET_JSON_OBJECT(na.props, '$.city') = 'London'"
        );
    }

    #[test]
    fn segment_sqlite_dialect() {
        assert_eq!(
            seg_plan("MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN b.name")
                .unwrap()
                .to_sql(&SqliteDialect),
            "SELECT na.id, na.label, na.props, nb.id, nb.label, nb.props \
             FROM \"grust_edges\" re \
             JOIN \"grust_nodes\" na ON na.id = re.src_id \
             JOIN \"grust_nodes\" nb ON nb.id = re.dst_id \
             WHERE re.edge_type = 'KNOWS' AND na.label = 'Person' AND nb.label = 'Person'"
        );
    }

    #[test]
    fn segment_edge_property_filter() {
        // Anonymous source endpoint: only `r` and `b` are selected.
        assert_eq!(
            seg_spark("MATCH ()-[r:RATED]->(b) WHERE r.stars >= 4 RETURN b.name"),
            "SELECT re.id, re.src_id, re.dst_id, re.edge_type, re.props, nb.id, nb.label, nb.props \
             FROM `grust_edges` re \
             JOIN `grust_nodes` na ON na.id = re.src_id \
             JOIN `grust_nodes` nb ON nb.id = re.dst_id \
             WHERE re.edge_type = 'RATED' \
             AND CAST(GET_JSON_OBJECT(re.props, '$.stars') AS BIGINT) >= 4"
        );
    }

    #[test]
    fn segment_in_predicate() {
        assert_eq!(
            seg_spark("MATCH (:Person)-[:KNOWS]->(b) WHERE b.name IN ['Alan', 'Grace'] RETURN b.name"),
            "SELECT nb.id, nb.label, nb.props \
             FROM `grust_edges` re \
             JOIN `grust_nodes` na ON na.id = re.src_id \
             JOIN `grust_nodes` nb ON nb.id = re.dst_id \
             WHERE re.edge_type = 'KNOWS' AND na.label = 'Person' \
             AND GET_JSON_OBJECT(nb.props, '$.name') IN ('Alan', 'Grace')"
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
            .ends_with("ORDER BY json_extract(nb.props, '$.age') DESC NULLS FIRST LIMIT 3"));
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
            "ORDER BY CAST(GET_JSON_OBJECT(nb.props, '$.age') AS BIGINT) DESC NULLS FIRST LIMIT 3"
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
            "ORDER BY CAST(GET_JSON_OBJECT(re.props, '$.stars') AS BIGINT) DESC NULLS FIRST"
        ));
        // Typed-JSON dialects order the edge property directly (no hints needed).
        assert!(plan
            .to_sql(&SqliteDialect)
            .ends_with("ORDER BY json_extract(re.props, '$.stars') DESC NULLS FIRST"));
    }

    #[test]
    fn segment_unsupported_shapes_fall_back() {
        // Undirected.
        assert!(seg_plan("MATCH (a)-[:KNOWS]-(b) RETURN a").is_none());
        // Variable length.
        assert!(seg_plan("MATCH (a)-[:KNOWS*1..2]->(b) RETURN a").is_none());
        // Two segments.
        assert!(seg_plan("MATCH (a)-[:KNOWS]->(b)-[:KNOWS]->(c) RETURN a").is_none());
        // Same variable on both endpoints.
        assert!(seg_plan("MATCH (a)-[:KNOWS]->(a) RETURN a").is_none());
        // Path variable.
        assert!(seg_plan("MATCH p = (a)-[:KNOWS]->(b) RETURN a").is_none());
        // Plain node pattern (handled by plan_node_read, not the segment planner).
        assert!(seg_plan("MATCH (n:Person) RETURN n").is_none());
        // WHERE referencing an unbound variable role / unsupported predicate.
        assert!(seg_plan("MATCH (a)-[:KNOWS]->(b) WHERE a.name STARTS WITH 'A' RETURN a").is_none());
    }

    #[test]
    fn segment_reconstructs_and_projects() {
        let plan =
            seg_plan("MATCH (a:Person)-[r:KNOWS]->(b:Person) RETURN a.name, r.since, b.name")
                .expect("pushable");
        // Columns: na(3) + re(5) + nb(3) = 11.
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
}
