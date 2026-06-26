//! Semantic-analysis skeleton (Unit 4 of `docs/GQL_GOAL.md`).
//!
//! Walks a parsed [`Query`] AST and performs the structural checks that do not
//! require execution:
//!
//! - **scope + variable binding**: every variable referenced in `WHERE`,
//!   projections, `DELETE`/`SET`/`REMOVE`, etc. must be bound by a preceding
//!   pattern, `UNWIND`, or carried across a `WITH` boundary;
//! - **graph element kind**: a variable bound as a node/relationship/path/value
//!   is used consistently (e.g. `SET v.k = ...` requires `v` to be an entity);
//! - **`WITH` scope boundary**: after `WITH`, only the projected aliases/variables
//!   remain in scope (classic Cypher horizon);
//! - **feature gates**: AST shapes that parse but are not yet executable in the
//!   current surface (`OPTIONAL MATCH`, `UNION`, `WITH`, variable-length paths,
//!   read-only `MATCH ... RETURN`) are reported against the [`GqlFeature`]
//!   manifest as non-fatal notes rather than silently accepted.
//!
//! Real binding/kind violations fail with structured `GqlError`s (name/type
//! kinds); feature-gated constructs are collected into [`SemanticReport`] so the
//! caller (and later lowering Units) can decide how to treat them. This module
//! is additive and does not touch the existing planning entrypoints.

use std::collections::HashMap;

use grust_core::{GrustError, Result};

use crate::ast::*;
use crate::gql::{GqlError, GqlErrorKind, GqlFeature};
use crate::lexer::Span;

/// The kind of value a variable is bound to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElementKind {
    Node,
    Relationship,
    Path,
    /// A scalar/list/map value (e.g. from `UNWIND` or a projected expression).
    Value,
}

impl ElementKind {
    fn is_entity(self) -> bool {
        matches!(self, ElementKind::Node | ElementKind::Relationship)
    }

    fn label(self) -> &'static str {
        match self {
            ElementKind::Node => "node",
            ElementKind::Relationship => "relationship",
            ElementKind::Path => "path",
            ElementKind::Value => "value",
        }
    }
}

/// A feature-gated construct that parsed but is not yet executable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FeatureGate {
    pub feature: GqlFeature,
    pub span: Span,
}

/// Result of semantic analysis when no hard error was found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SemanticReport {
    /// Constructs used by the query that are not yet executable in the current
    /// surface (deduplicated by feature).
    pub feature_gates: Vec<FeatureGate>,
}

impl SemanticReport {
    pub fn uses_feature(&self, feature: GqlFeature) -> bool {
        self.feature_gates.iter().any(|g| g.feature == feature)
    }

    fn note(&mut self, feature: GqlFeature, span: Span) {
        if !self.uses_feature(feature) {
            self.feature_gates.push(FeatureGate { feature, span });
        }
    }
}

/// Lexical scope: variable name -> element kind.
#[derive(Clone, Debug, Default)]
struct Scope {
    vars: HashMap<String, ElementKind>,
}

impl Scope {
    fn get(&self, name: &str) -> Option<ElementKind> {
        self.vars.get(name).copied()
    }

    /// Bind a variable. Re-binding to the same kind is fine; re-binding to a
    /// different entity/value kind is a semantic error the caller surfaces.
    fn bind(&mut self, name: &str, kind: ElementKind) -> std::result::Result<(), ElementKind> {
        if let Some(existing) = self.vars.get(name) {
            if *existing != kind {
                return Err(*existing);
            }
        }
        self.vars.insert(name.to_string(), kind);
        Ok(())
    }
}

fn name_error(message: impl Into<String>) -> GrustError {
    GqlError::new(GqlErrorKind::Name, message).into()
}

fn type_error(message: impl Into<String>) -> GrustError {
    GqlError::new(GqlErrorKind::Type, message).into()
}

/// Analyze a query AST. Returns a [`SemanticReport`] of feature gates on success,
/// or a structured name/type error on a binding/kind violation.
pub fn analyze(query: &Query) -> Result<SemanticReport> {
    let mut report = SemanticReport::default();
    for (idx, part) in query.parts.iter().enumerate() {
        if let Some(_kind) = part.union {
            report.note(GqlFeature::UnionClause, part.query.span);
        }
        // Each UNION arm gets a fresh scope.
        let _ = idx;
        analyze_single(&part.query, &mut report)?;
    }
    Ok(report)
}

fn analyze_single(query: &SingleQuery, report: &mut SemanticReport) -> Result<()> {
    let mut scope = Scope::default();
    let has_update = query.clauses.iter().any(Clause::is_updating);
    let has_return = query.clauses.iter().any(|c| matches!(c, Clause::Return(_)));

    // Read-only MATCH ... RETURN (no updating clause) is not executable yet.
    if has_return && !has_update {
        if let Some(span) = query
            .clauses
            .iter()
            .find(|c| matches!(c, Clause::Match(_)))
            .map(|c| c.span())
        {
            report.note(GqlFeature::ReadOnlyMatchReturn, span);
        }
    }

    for clause in &query.clauses {
        analyze_clause(clause, &mut scope, report)?;
    }
    Ok(())
}

fn analyze_clause(clause: &Clause, scope: &mut Scope, report: &mut SemanticReport) -> Result<()> {
    match clause {
        Clause::Match(m) => {
            if m.optional {
                report.note(GqlFeature::OptionalMatch, m.span);
            }
            for pattern in &m.patterns {
                bind_pattern(pattern, scope, report)?;
            }
            if let Some(where_clause) = &m.where_clause {
                check_expr_bound(where_clause, scope)?;
            }
        }
        Clause::Create(c) => {
            for pattern in &c.patterns {
                // CREATE patterns may reference bound endpoints and bind new ones.
                bind_pattern(pattern, scope, report)?;
            }
        }
        Clause::Merge(m) => {
            bind_pattern(&m.pattern, scope, report)?;
            for item in m.on_create.iter().chain(m.on_match.iter()) {
                check_set_item(item, scope)?;
            }
        }
        Clause::Delete(d) => {
            for target in &d.targets {
                check_expr_bound(target, scope)?;
                if let Expr::Variable(name) = target {
                    if let Some(kind) = scope.get(name) {
                        if kind == ElementKind::Value {
                            return Err(type_error(format!(
                                "cannot DELETE `{name}`: it is bound to a value, not a node, relationship, or path"
                            )));
                        }
                    }
                }
            }
        }
        Clause::Set(s) => {
            for item in &s.items {
                check_set_item(item, scope)?;
            }
        }
        Clause::Remove(r) => {
            for item in &r.items {
                check_remove_item(item, scope)?;
            }
        }
        Clause::Unwind(u) => {
            check_expr_bound(&u.expr, scope)?;
            bind_var(scope, &u.alias, ElementKind::Value)?;
        }
        Clause::With(w) => {
            report.note(GqlFeature::WithClause, w.span);
            let new_scope = project_scope(&w.projection, scope, report)?;
            if let Some(where_clause) = &w.where_clause {
                check_expr_bound(where_clause, &new_scope)?;
            }
            *scope = new_scope;
        }
        Clause::Call(c) => {
            report.note(GqlFeature::ProcedureCall, c.span);
            // YIELD columns (aliased or not) become value-kind bindings.
            for (col, alias) in &c.yields {
                bind_var(scope, alias.as_deref().unwrap_or(col), ElementKind::Value)?;
            }
            if let Some(where_clause) = &c.where_clause {
                check_expr_bound(where_clause, scope)?;
            }
        }
        Clause::Return(r) => {
            // RETURN does not need aliases for non-variable expressions.
            let _ = project_scope_for_return(&r.projection, scope, report)?;
        }
    }
    Ok(())
}

/// Bind the variables introduced by a path pattern; report var-length gates.
fn bind_pattern(
    pattern: &PathPattern,
    scope: &mut Scope,
    report: &mut SemanticReport,
) -> Result<()> {
    if let Some(path_var) = &pattern.variable {
        bind_var(scope, path_var, ElementKind::Path)?;
    }
    bind_node(&pattern.start, scope)?;
    for segment in &pattern.segments {
        if let Some(var) = &segment.relationship.variable {
            bind_var(scope, var, ElementKind::Relationship)?;
        }
        if segment.relationship.length.is_some() {
            report.note(GqlFeature::QuantifiedPathPattern, segment.relationship.span);
        }
        if let Some(props) = &segment.relationship.properties {
            check_map_bound(props, scope)?;
        }
        bind_node(&segment.node, scope)?;
    }
    Ok(())
}

fn bind_node(node: &NodePattern, scope: &mut Scope) -> Result<()> {
    if let Some(var) = &node.variable {
        bind_var(scope, var, ElementKind::Node)?;
    }
    if let Some(props) = &node.properties {
        check_map_bound(props, scope)?;
    }
    Ok(())
}

fn bind_var(scope: &mut Scope, name: &str, kind: ElementKind) -> Result<()> {
    scope.bind(name, kind).map_err(|existing| {
        type_error(format!(
            "variable `{name}` is already bound as a {} but reused as a {}",
            existing.label(),
            kind.label()
        ))
    })
}

/// Build the post-`WITH` scope. `WITH` requires that non-variable projection
/// items be aliased.
fn project_scope(
    projection: &Projection,
    scope: &Scope,
    report: &mut SemanticReport,
) -> Result<Scope> {
    let mut new_scope = Scope::default();
    if projection.star {
        new_scope = scope.clone();
    }
    for item in &projection.items {
        check_expr_bound(&item.expr, scope)?;
        match (&item.alias, &item.expr) {
            (Some(alias), Expr::Variable(v)) => {
                let kind = scope.get(v).unwrap_or(ElementKind::Value);
                bind_var(&mut new_scope, alias, kind)?;
            }
            (Some(alias), _) => {
                bind_var(&mut new_scope, alias, ElementKind::Value)?;
            }
            (None, Expr::Variable(v)) => {
                let kind = scope.get(v).unwrap_or(ElementKind::Value);
                bind_var(&mut new_scope, v, kind)?;
            }
            (None, _) => {
                return Err(name_error(
                    "WITH requires an alias (AS ...) for non-variable expressions",
                ));
            }
        }
    }
    check_order_skip_limit(projection, &new_scope, report)?;
    Ok(new_scope)
}

/// `RETURN` projection: like `WITH` but non-variable expressions need no alias,
/// and the resulting scope is terminal (used only to validate ORDER BY/SKIP/LIMIT).
fn project_scope_for_return(
    projection: &Projection,
    scope: &Scope,
    report: &mut SemanticReport,
) -> Result<Scope> {
    let mut out_scope = Scope::default();
    if projection.star {
        out_scope = scope.clone();
    }
    for item in &projection.items {
        check_expr_bound(&item.expr, scope)?;
        if let Some(alias) = &item.alias {
            let kind = match &item.expr {
                Expr::Variable(v) => scope.get(v).unwrap_or(ElementKind::Value),
                _ => ElementKind::Value,
            };
            bind_var(&mut out_scope, alias, kind)?;
        } else if let Expr::Variable(v) = &item.expr {
            let kind = scope.get(v).unwrap_or(ElementKind::Value);
            bind_var(&mut out_scope, v, kind)?;
        }
    }
    // ORDER BY may reference both pre-projection variables and output aliases.
    check_order_skip_limit_for_return(projection, scope, &out_scope, report)?;
    Ok(out_scope)
}

fn check_order_skip_limit(
    projection: &Projection,
    scope: &Scope,
    _report: &mut SemanticReport,
) -> Result<()> {
    for order in &projection.order_by {
        check_expr_bound(&order.expr, scope)?;
    }
    if let Some(skip) = &projection.skip {
        check_expr_bound(skip, scope)?;
    }
    if let Some(limit) = &projection.limit {
        check_expr_bound(limit, scope)?;
    }
    Ok(())
}

fn check_order_skip_limit_for_return(
    projection: &Projection,
    pre_scope: &Scope,
    out_scope: &Scope,
    _report: &mut SemanticReport,
) -> Result<()> {
    // ORDER BY can see either the input scope or the projected aliases.
    let mut merged = pre_scope.clone();
    for (k, v) in &out_scope.vars {
        merged.vars.insert(k.clone(), *v);
    }
    for order in &projection.order_by {
        check_expr_bound(&order.expr, &merged)?;
    }
    if let Some(skip) = &projection.skip {
        check_expr_bound(skip, pre_scope)?;
    }
    if let Some(limit) = &projection.limit {
        check_expr_bound(limit, pre_scope)?;
    }
    Ok(())
}

fn check_set_item(item: &SetItem, scope: &Scope) -> Result<()> {
    match item {
        SetItem::Property { target, value } => {
            check_expr_bound(target, scope)?;
            check_expr_bound(value, scope)?;
            if let Some(root) = root_variable(target) {
                require_entity(root, scope, "SET a property on")?;
            }
        }
        SetItem::Properties {
            variable, value, ..
        } => {
            require_bound(variable, scope)?;
            require_entity(variable, scope, "SET properties on")?;
            check_expr_bound(value, scope)?;
        }
        SetItem::Labels { variable, .. } => {
            require_bound(variable, scope)?;
            if let Some(kind) = scope.get(variable) {
                if kind != ElementKind::Node {
                    return Err(type_error(format!(
                        "cannot SET labels on `{variable}`: it is a {}, not a node",
                        kind.label()
                    )));
                }
            }
        }
    }
    Ok(())
}

fn check_remove_item(item: &RemoveItem, scope: &Scope) -> Result<()> {
    match item {
        RemoveItem::Property { target } => {
            check_expr_bound(target, scope)?;
            if let Some(root) = root_variable(target) {
                require_entity(root, scope, "REMOVE a property from")?;
            }
        }
        RemoveItem::Labels { variable, .. } => {
            require_bound(variable, scope)?;
            if let Some(kind) = scope.get(variable) {
                if kind != ElementKind::Node {
                    return Err(type_error(format!(
                        "cannot REMOVE labels from `{variable}`: it is a {}, not a node",
                        kind.label()
                    )));
                }
            }
        }
    }
    Ok(())
}

fn require_bound(name: &str, scope: &Scope) -> Result<()> {
    if scope.get(name).is_some() {
        Ok(())
    } else {
        Err(name_error(format!(
            "variable `{name}` is not bound in this scope"
        )))
    }
}

fn require_entity(name: &str, scope: &Scope, action: &str) -> Result<()> {
    match scope.get(name) {
        Some(kind) if kind.is_entity() => Ok(()),
        Some(kind) => Err(type_error(format!(
            "cannot {action} `{name}`: it is a {}, not a node or relationship",
            kind.label()
        ))),
        None => Err(name_error(format!(
            "variable `{name}` is not bound in this scope"
        ))),
    }
}

/// Ensure every variable referenced by `expr` is bound in `scope`.
fn check_expr_bound(expr: &Expr, scope: &Scope) -> Result<()> {
    let mut unbound = None;
    visit_variables(expr, &mut |name| {
        if unbound.is_none() && scope.get(name).is_none() {
            unbound = Some(name.to_string());
        }
    });
    match unbound {
        Some(name) => Err(name_error(format!(
            "variable `{name}` is not bound in this scope"
        ))),
        None => Ok(()),
    }
}

fn check_map_bound(map: &MapLiteral, scope: &Scope) -> Result<()> {
    for (_, value) in &map.entries {
        check_expr_bound(value, scope)?;
    }
    Ok(())
}

/// The root variable of a property/index chain, if any (`a.b.c` -> `a`).
fn root_variable(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Variable(name) => Some(name),
        Expr::Property { base, .. } => root_variable(base),
        Expr::Index { base, .. } => root_variable(base),
        _ => None,
    }
}

/// Invoke `f` for every free variable reference in `expr`. Property keys and map
/// keys are identifiers, not variables, so they are not visited.
fn visit_variables(expr: &Expr, f: &mut impl FnMut(&str)) {
    match expr {
        Expr::Variable(name) => f(name),
        Expr::Property { base, .. } => visit_variables(base, f),
        Expr::Index { base, index } => {
            visit_variables(base, f);
            visit_variables(index, f);
        }
        Expr::List(items) => items.iter().for_each(|e| visit_variables(e, f)),
        Expr::Map(entries) => entries.iter().for_each(|(_, e)| visit_variables(e, f)),
        Expr::Function { args, .. } => args.iter().for_each(|e| visit_variables(e, f)),
        Expr::Unary { operand, .. } => visit_variables(operand, f),
        Expr::Binary { lhs, rhs, .. } => {
            visit_variables(lhs, f);
            visit_variables(rhs, f);
        }
        Expr::IsNull { operand, .. } => visit_variables(operand, f),
        Expr::Case {
            operand,
            branches,
            default,
        } => {
            if let Some(op) = operand {
                visit_variables(op, f);
            }
            for branch in branches {
                visit_variables(&branch.when, f);
                visit_variables(&branch.then, f);
            }
            if let Some(d) = default {
                visit_variables(d, f);
            }
        }
        Expr::Null
        | Expr::Boolean(_)
        | Expr::Integer(_)
        | Expr::Float(_)
        | Expr::String(_)
        | Expr::Parameter(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_query;

    fn analyze_src(source: &str) -> Result<SemanticReport> {
        let query = parse_query(source).expect("should parse");
        analyze(&query)
    }

    #[test]
    fn binds_pattern_variables_and_resolves_where() {
        let report = analyze_src("MATCH (n:Person {id:'p1'}) WHERE n.age > 18 DELETE n").unwrap();
        // write present, so no read-only gate
        assert!(!report.uses_feature(GqlFeature::ReadOnlyMatchReturn));
    }

    #[test]
    fn unbound_variable_in_where_is_an_error() {
        let err = analyze_src("MATCH (n:Person) WHERE m.age > 18 DELETE n").unwrap_err();
        assert!(matches!(err, GrustError::CypherUnresolvedIdentity(_)));
        assert!(err.to_string().contains("not bound"));
    }

    #[test]
    fn unbound_variable_in_delete_is_an_error() {
        let err = analyze_src("MATCH (n:Person) DELETE x").unwrap_err();
        assert!(matches!(err, GrustError::CypherUnresolvedIdentity(_)));
    }

    #[test]
    fn set_property_on_value_is_a_type_error() {
        // x is bound to a value by UNWIND, then used as an entity in SET.
        let err = analyze_src("UNWIND [1,2,3] AS x SET x.k = 1").unwrap_err();
        assert!(matches!(err, GrustError::CypherExecution(_))); // type kind maps to execution transport
        assert!(err.to_string().contains("gql:type"));
    }

    #[test]
    fn delete_value_is_a_type_error() {
        let err = analyze_src("UNWIND [1,2,3] AS x DELETE x").unwrap_err();
        assert!(err.to_string().contains("gql:type"));
        assert!(err.to_string().contains("value"));
    }

    #[test]
    fn reusing_node_var_as_relationship_is_a_kind_error() {
        // bind n as a node, then reuse n as a relationship variable
        let err = analyze_src("MATCH (n)-[n:R]->(b) RETURN n").unwrap_err();
        assert!(err.to_string().contains("gql:type"));
        assert!(err.to_string().contains("already bound"));
    }

    #[test]
    fn with_horizon_drops_out_of_scope_variables() {
        // After WITH n, the variable m is no longer in scope.
        let err = analyze_src("MATCH (n)-[r]->(m) WITH n RETURN m").unwrap_err();
        assert!(matches!(err, GrustError::CypherUnresolvedIdentity(_)));
    }

    #[test]
    fn with_carries_aliases_forward() {
        let report = analyze_src(
            "MATCH (n:Person) WITH n.name AS name WHERE name STARTS WITH 'A' RETURN name",
        )
        .unwrap();
        assert!(report.uses_feature(GqlFeature::WithClause));
    }

    #[test]
    fn with_requires_alias_for_nonvariable() {
        let err = analyze_src("MATCH (n) WITH n.name RETURN n").unwrap_err();
        assert!(matches!(err, GrustError::CypherUnresolvedIdentity(_)));
        assert!(err.to_string().contains("requires an alias"));
    }

    #[test]
    fn feature_gates_are_collected() {
        let report =
            analyze_src("MATCH (a) OPTIONAL MATCH (a)-[:R*1..3]->(b) RETURN a, b").unwrap();
        assert!(report.uses_feature(GqlFeature::OptionalMatch));
        assert!(report.uses_feature(GqlFeature::QuantifiedPathPattern));
        assert!(report.uses_feature(GqlFeature::ReadOnlyMatchReturn));
    }

    #[test]
    fn union_is_gated() {
        let report = analyze_src("MATCH (a:A) RETURN a UNION MATCH (b:B) RETURN b").unwrap();
        assert!(report.uses_feature(GqlFeature::UnionClause));
    }

    #[test]
    fn order_by_can_reference_alias() {
        let report =
            analyze_src("MATCH (n:Person) WITH n.age AS age RETURN age ORDER BY age DESC").unwrap();
        assert!(report.uses_feature(GqlFeature::WithClause));
    }

    #[test]
    fn create_then_set_is_well_scoped() {
        let report = analyze_src("CREATE (n:Person {id:'p1'}) SET n.active = true").unwrap();
        // updating query, no read-only gate
        assert!(!report.uses_feature(GqlFeature::ReadOnlyMatchReturn));
    }

    #[test]
    fn return_allows_unaliased_expression() {
        // read-only, so gated, but not a hard error
        let report = analyze_src("MATCH (n:Person) RETURN n.name, count(*)").unwrap();
        assert!(report.uses_feature(GqlFeature::ReadOnlyMatchReturn));
    }
}
