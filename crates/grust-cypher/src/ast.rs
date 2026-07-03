//! Typed AST for GQL/Cypher (Unit 4 of `docs/GQL_GOAL.md`).
//!
//! These are the syntactic node types produced by the [`crate::parser`] from the
//! [`crate::lexer`] token stream, before semantic analysis and lowering to a
//! logical plan. The AST is deliberately backend-neutral and carries no
//! execution semantics.
//!
//! This module is additive. The existing hand-written `cypher_*` planning
//! entrypoints continue to operate on their own internal representation; the AST
//! path is introduced alongside them and becomes the source of truth as the
//! later Units lower it into the existing logical plans (per GQL_GOAL.md, that
//! integration is sequenced for review).

use crate::lexer::Span;

/// A complete parsed statement.
#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    /// A (possibly multi-part, possibly `UNION`-composed) query.
    Query(Query),
}

/// A query: one or more `UNION`-composed single queries.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub parts: Vec<UnionPart>,
    pub span: Span,
}

/// One arm of a `UNION` chain.
#[derive(Clone, Debug, PartialEq)]
pub struct UnionPart {
    /// `None` for the first arm; `Some(all)` for subsequent `UNION [ALL]` arms.
    pub union: Option<UnionKind>,
    pub query: SingleQuery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnionKind {
    /// `UNION` (distinct).
    Distinct,
    /// `UNION ALL`.
    All,
}

/// A single (non-`UNION`) query: a sequence of clauses.
#[derive(Clone, Debug, PartialEq)]
pub struct SingleQuery {
    pub clauses: Vec<Clause>,
    pub span: Span,
}

/// A top-level clause within a single query.
#[derive(Clone, Debug, PartialEq)]
pub enum Clause {
    Use(UseClause),
    Match(MatchClause),
    Create(CreateClause),
    Merge(MergeClause),
    Delete(DeleteClause),
    Set(SetClause),
    Remove(RemoveClause),
    With(WithClause),
    Unwind(UnwindClause),
    Call(CallClause),
    Subquery(SubqueryClause),
    Return(ReturnClause),
}

/// `CALL { <query> }` — an inline subquery executed once per incoming row.
/// The outer row's bindings are visible inside (correlated, import-all
/// semantics); the subquery's `RETURN` columns join onto the outer row.
#[derive(Clone, Debug, PartialEq)]
pub struct SubqueryClause {
    pub query: Query,
    pub span: Span,
}

/// `CALL <name>() [YIELD col [AS alias], …]` — a read-only catalog procedure.
#[derive(Clone, Debug, PartialEq)]
pub struct CallClause {
    /// Dotted procedure name, e.g. `db.labels`.
    pub name: String,
    /// `YIELD` columns with optional aliases; empty means no explicit `YIELD`.
    pub yields: Vec<(String, Option<String>)>,
    /// Optional `WHERE` filter following `YIELD`.
    pub where_clause: Option<Expr>,
    pub span: Span,
}

impl Clause {
    pub fn span(&self) -> Span {
        match self {
            Clause::Use(c) => c.span,
            Clause::Match(c) => c.span,
            Clause::Create(c) => c.span,
            Clause::Merge(c) => c.span,
            Clause::Delete(c) => c.span,
            Clause::Set(c) => c.span,
            Clause::Remove(c) => c.span,
            Clause::With(c) => c.span,
            Clause::Unwind(c) => c.span,
            Clause::Call(c) => c.span,
            Clause::Subquery(c) => c.span,
            Clause::Return(c) => c.span,
        }
    }

    /// True for read clauses (`MATCH`, `UNWIND`, `CALL`); false for updating clauses.
    pub fn is_reading(&self) -> bool {
        matches!(
            self,
            Clause::Use(_)
                | Clause::Match(_)
                | Clause::Unwind(_)
                | Clause::Call(_)
                | Clause::Subquery(_)
        )
    }

    /// True for the updating clauses (`CREATE`/`MERGE`/`SET`/`REMOVE`/`DELETE`).
    pub fn is_updating(&self) -> bool {
        matches!(
            self,
            Clause::Create(_)
                | Clause::Merge(_)
                | Clause::Set(_)
                | Clause::Remove(_)
                | Clause::Delete(_)
        )
    }
}

// ---------------------------------------------------------------------------
// Reading clauses
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct UseClause {
    pub graph: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MatchClause {
    pub optional: bool,
    pub patterns: Vec<PathPattern>,
    pub where_clause: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UnwindClause {
    pub expr: Expr,
    pub alias: String,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Updating clauses
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct CreateClause {
    pub patterns: Vec<PathPattern>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MergeClause {
    pub pattern: PathPattern,
    pub on_create: Vec<SetItem>,
    pub on_match: Vec<SetItem>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeleteClause {
    pub detach: bool,
    pub targets: Vec<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SetClause {
    pub items: Vec<SetItem>,
    pub span: Span,
}

/// A single assignment within a `SET` clause (or `ON CREATE`/`ON MATCH`).
#[derive(Clone, Debug, PartialEq)]
pub enum SetItem {
    /// `target.key = value` (target is a variable or property expression).
    Property { target: Expr, value: Expr },
    /// `var = { .. }` (replace) or `var += { .. }` (merge).
    Properties {
        variable: String,
        merge: bool,
        value: Expr,
    },
    /// `var:Label1:Label2`.
    Labels {
        variable: String,
        labels: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoveClause {
    pub items: Vec<RemoveItem>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RemoveItem {
    /// `REMOVE n.key`.
    Property { target: Expr },
    /// `REMOVE n:Label`.
    Labels {
        variable: String,
        labels: Vec<String>,
    },
}

// ---------------------------------------------------------------------------
// Projection clauses
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct WithClause {
    pub projection: Projection,
    pub where_clause: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReturnClause {
    pub projection: Projection,
    pub span: Span,
}

/// The shared projection body of `RETURN` and `WITH`.
#[derive(Clone, Debug, PartialEq)]
pub struct Projection {
    pub distinct: bool,
    /// `true` for `RETURN *` / `WITH *`.
    pub star: bool,
    pub items: Vec<ReturnItem>,
    pub order_by: Vec<OrderItem>,
    pub skip: Option<Expr>,
    pub limit: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReturnItem {
    pub expr: Expr,
    pub alias: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OrderItem {
    pub expr: Expr,
    pub descending: bool,
}

// ---------------------------------------------------------------------------
// Patterns
// ---------------------------------------------------------------------------

/// A path pattern: a node, then zero or more `(relationship, node)` segments.
/// An optional path variable binds the whole path (`p = (a)-[r]->(b)`).
#[derive(Clone, Debug, PartialEq)]
pub struct PathPattern {
    pub variable: Option<String>,
    pub start: NodePattern,
    pub segments: Vec<PathSegment>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PathSegment {
    pub relationship: RelationshipPattern,
    pub node: NodePattern,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodePattern {
    pub variable: Option<String>,
    pub labels: Vec<String>,
    pub properties: Option<MapLiteral>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RelationshipPattern {
    pub variable: Option<String>,
    pub types: Vec<String>,
    pub direction: Direction,
    pub properties: Option<MapLiteral>,
    /// Variable-length bound `*min..max` (each side optional).
    pub length: Option<RangeLiteral>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    /// `-[..]->`
    Outgoing,
    /// `<-[..]-`
    Incoming,
    /// `-[..]-`
    Undirected,
}

/// A variable-length relationship bound, e.g. `*`, `*2`, `*1..3`, `*..5`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RangeLiteral {
    pub min: Option<u64>,
    pub max: Option<u64>,
}

/// An inline map literal `{ key: expr, ... }` used in patterns and expressions.
#[derive(Clone, Debug, PartialEq)]
pub struct MapLiteral {
    pub entries: Vec<(String, Expr)>,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

/// An expression node.
///
/// Spans are tracked by the parser at clause boundaries for diagnostics; the
/// expression tree itself is kept span-free for ergonomics in later evaluation
/// units (Unit 7 builds the evaluator over this shape).
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Parameter(String),
    Variable(String),
    /// `base.key`
    Property {
        base: Box<Expr>,
        key: String,
    },
    /// `[a, b, c]`
    List(Vec<Expr>),
    /// `{ k: v, ... }`
    Map(Vec<(String, Expr)>),
    /// `f(args)` / `count(*)` / `count(DISTINCT x)`
    Function {
        name: String,
        distinct: bool,
        /// `true` for the `count(*)` star argument form.
        star: bool,
        args: Vec<Expr>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `x IS NULL` / `x IS NOT NULL`.
    IsNull {
        operand: Box<Expr>,
        negated: bool,
    },
    /// `CASE [operand] WHEN .. THEN .. [ELSE ..] END`.
    Case {
        operand: Option<Box<Expr>>,
        branches: Vec<CaseBranch>,
        default: Option<Box<Expr>>,
    },
    /// `list[index]`.
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct CaseBranch {
    pub when: Expr,
    pub then: Expr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    Not,
    Negate,
    Plus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOp {
    // arithmetic
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Power,
    // comparison
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    // boolean
    And,
    Or,
    Xor,
    // membership
    In,
    // string
    StartsWith,
    EndsWith,
    Contains,
}

impl BinaryOp {
    /// Binding power for Pratt parsing; higher binds tighter.
    pub fn binding_power(self) -> u8 {
        match self {
            BinaryOp::Or => 1,
            BinaryOp::Xor => 2,
            BinaryOp::And => 3,
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge
            | BinaryOp::In
            | BinaryOp::StartsWith
            | BinaryOp::EndsWith
            | BinaryOp::Contains => 4,
            BinaryOp::Add | BinaryOp::Subtract => 5,
            BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Modulo => 6,
            BinaryOp::Power => 7,
        }
    }
}

impl Expr {
    /// Convenience constructor for a binary expression.
    pub fn binary(op: BinaryOp, lhs: Expr, rhs: Expr) -> Expr {
        Expr::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        }
    }

    /// Convenience constructor for a property access.
    pub fn property(base: Expr, key: impl Into<String>) -> Expr {
        Expr::Property {
            base: Box::new(base),
            key: key.into(),
        }
    }

    /// True if this expression is a literal constant (no variables/params/calls).
    pub fn is_constant(&self) -> bool {
        match self {
            Expr::Null | Expr::Boolean(_) | Expr::Integer(_) | Expr::Float(_) | Expr::String(_) => {
                true
            }
            Expr::List(items) => items.iter().all(Expr::is_constant),
            Expr::Map(entries) => entries.iter().all(|(_, v)| v.is_constant()),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp() -> Span {
        Span::new(0, 0)
    }

    #[test]
    fn clause_classification() {
        let m = Clause::Match(MatchClause {
            optional: false,
            patterns: vec![],
            where_clause: None,
            span: sp(),
        });
        assert!(m.is_reading());
        assert!(!m.is_updating());

        let c = Clause::Create(CreateClause {
            patterns: vec![],
            span: sp(),
        });
        assert!(c.is_updating());
        assert!(!c.is_reading());
    }

    #[test]
    fn binary_binding_power_orders_operators() {
        assert!(BinaryOp::Multiply.binding_power() > BinaryOp::Add.binding_power());
        assert!(BinaryOp::Add.binding_power() > BinaryOp::Eq.binding_power());
        assert!(BinaryOp::Eq.binding_power() > BinaryOp::And.binding_power());
        assert!(BinaryOp::And.binding_power() > BinaryOp::Or.binding_power());
        assert!(BinaryOp::Power.binding_power() > BinaryOp::Multiply.binding_power());
    }

    #[test]
    fn expr_constant_detection() {
        assert!(Expr::Integer(1).is_constant());
        assert!(Expr::List(vec![Expr::Integer(1), Expr::String("x".into())]).is_constant());
        assert!(!Expr::Variable("n".into()).is_constant());
        assert!(!Expr::List(vec![Expr::Variable("n".into())]).is_constant());
        assert!(!Expr::property(Expr::Variable("n".into()), "id").is_constant());
    }

    #[test]
    fn expr_constructors() {
        let e = Expr::binary(BinaryOp::Add, Expr::Integer(1), Expr::Integer(2));
        match e {
            Expr::Binary { op, .. } => assert_eq!(op, BinaryOp::Add),
            _ => panic!("expected binary"),
        }
    }

    #[test]
    fn direction_and_range_literals() {
        let rel = RelationshipPattern {
            variable: Some("r".into()),
            types: vec!["KNOWS".into()],
            direction: Direction::Outgoing,
            properties: None,
            length: Some(RangeLiteral {
                min: Some(1),
                max: Some(3),
            }),
            span: sp(),
        };
        assert_eq!(rel.direction, Direction::Outgoing);
        assert_eq!(rel.length.unwrap().max, Some(3));
    }
}
