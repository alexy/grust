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
    /// The `RETURN` projection, run through the shared reference projection.
    projection: Projection,
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

// ---------------------------------------------------------------------------
// SQL dialects
// ---------------------------------------------------------------------------

/// The dialect-specific string formatting a backend needs to render a pushdown
/// scan + filter. Implementations are pure string config — no backend state.
pub trait SqlDialect {
    /// The generic node table name (e.g. `grust_nodes`).
    fn nodes_table(&self) -> &str;
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
    let query = parse_query(cypher).map_err(|e| e.into_grust(cypher))?;
    crate::semantics::analyze(&query)?;
    Ok(lower_query(&query, params))
}

fn lower_query(query: &Query, params: &CypherParameters) -> Option<NodeReadPushdown> {
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

    Some(NodeReadPushdown {
        var,
        label,
        filter,
        projection: return_clause.projection.clone(),
    })
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
        sql
    }

    /// Run the `RETURN` projection over the nodes the backend fetched, producing
    /// the same [`CypherResultTable`] the in-memory reference would.
    pub fn project(
        &self,
        nodes: Vec<Node>,
        params: &CypherParameters,
    ) -> Result<CypherResultTable> {
        crate::read::project_nodes(&self.var, nodes, &self.projection, params)
    }
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
        // Predicate outside the subset: IN.
        assert!(plan("MATCH (n:Person) WHERE n.age IN [1,2] RETURN n").is_none());
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
        let table = plan.project(nodes, &params()).unwrap();
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
        let table = plan.project(nodes, &params()).unwrap();
        assert_eq!(table.columns, vec!["c".to_string()]);
        assert_eq!(table.rows, vec![vec![Value::Int(2)]]);
    }
}
