//! Recursive-descent parser: lexer tokens -> typed AST (Unit 4 of GQL_GOAL.md).
//!
//! Consumes the [`crate::lexer`] token stream and produces [`crate::ast`] nodes.
//! Expressions are parsed with a Pratt loop driven by [`crate::ast::BinaryOp`]
//! binding powers. Failures are span-bearing [`ParseError`]s; grammar that is
//! recognized but deliberately out of the current scope fails as a
//! feature-tagged unsupported error rather than a generic parse failure.
//!
//! This module is additive: it does not yet replace the hand-written `cypher_*`
//! parser entrypoints. Lowering the AST it produces into the existing logical
//! plans, and swapping the legacy entrypoints to compatibility wrappers over
//! this path, is sequenced for review per GQL_GOAL.md.

use grust_core::GrustError;

use crate::ast::*;
use crate::gql::{GqlConformanceProfile, GqlFeature, gql_syntax, unsupported_gql_feature};
use crate::lexer::{Keyword, Span, SpannedToken, Token, line_col, tokenize};

/// What kind of failure a [`ParseError`] represents.
#[derive(Clone, Debug, PartialEq)]
pub enum ParseErrorKind {
    /// A lexical or grammatical error.
    Syntax,
    /// A recognized construct that is deliberately not yet supported.
    Unsupported(GqlFeature),
}

/// A span-bearing parse error.
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub kind: ParseErrorKind,
    pub span: Span,
    pub message: String,
}

impl ParseError {
    fn syntax(span: Span, message: impl Into<String>) -> Self {
        ParseError {
            kind: ParseErrorKind::Syntax,
            span,
            message: message.into(),
        }
    }

    fn unsupported(span: Span, feature: GqlFeature, message: impl Into<String>) -> Self {
        ParseError {
            kind: ParseErrorKind::Unsupported(feature),
            span,
            message: message.into(),
        }
    }

    /// Render with 1-based line/column resolved against `source`.
    pub fn render(&self, source: &str) -> String {
        let pos = line_col(source, self.span.start);
        format!(
            "{} at line {}, column {} (bytes {})",
            self.message, pos.line, pos.column, self.span
        )
    }

    /// Convert into the structured `GrustError` transport, preserving the
    /// feature tag for unsupported constructs.
    pub fn into_grust(self, source: &str) -> GrustError {
        let rendered = self.render(source);
        match self.kind {
            ParseErrorKind::Syntax => gql_syntax(rendered),
            ParseErrorKind::Unsupported(feature) => {
                unsupported_gql_feature(feature, GqlConformanceProfile::PortableGql, rendered)
            }
        }
    }
}

type PResult<T> = Result<T, ParseError>;

/// Parse a single query (one statement, optional trailing `;`).
pub fn parse_query(source: &str) -> PResult<Query> {
    let mut parser = Parser::new(source)?;
    let query = parser.parse_one_query()?;
    parser.skip_optional_semicolon();
    parser.expect_eof()?;
    Ok(query)
}

/// Parse one or more `;`-separated queries.
pub fn parse_statements(source: &str) -> PResult<Vec<Query>> {
    let mut parser = Parser::new(source)?;
    let mut queries = Vec::new();
    loop {
        parser.skip_semicolons();
        if parser.at_eof() {
            break;
        }
        queries.push(parser.parse_one_query()?);
    }
    Ok(queries)
}

/// Parse a standalone expression (primarily for testing the expression grammar).
pub fn parse_expression(source: &str) -> PResult<Expr> {
    let mut parser = Parser::new(source)?;
    let expr = parser.parse_expr()?;
    parser.expect_eof()?;
    Ok(expr)
}

struct Parser {
    tokens: Vec<SpannedToken>,
    pos: usize,
}

impl Parser {
    fn new(source: &str) -> PResult<Self> {
        let tokens = tokenize(source).map_err(|e| ParseError::syntax(e.span, e.message))?;
        Ok(Parser { tokens, pos: 0 })
    }

    // -- token navigation -------------------------------------------------

    fn peek(&self) -> &Token {
        &self.tokens[self.pos].token
    }

    fn peek_at(&self, ahead: usize) -> &Token {
        self.tokens
            .get(self.pos + ahead)
            .map(|t| &t.token)
            .unwrap_or(&Token::Eof)
    }

    fn span_here(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn advance(&mut self) -> SpannedToken {
        let tok = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), Token::Eof)
    }

    fn eat(&mut self, token: &Token) -> bool {
        if self.peek() == token {
            self.advance();
            true
        } else {
            false
        }
    }

    fn eat_keyword(&mut self, kw: Keyword) -> bool {
        if self.peek().is_keyword(kw) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &Token, what: &str) -> PResult<()> {
        if self.eat(token) {
            Ok(())
        } else {
            Err(ParseError::syntax(
                self.span_here(),
                format!("expected {what}, found {:?}", self.peek()),
            ))
        }
    }

    fn skip_optional_semicolon(&mut self) {
        let _ = self.eat(&Token::Semicolon);
    }

    fn skip_semicolons(&mut self) {
        while self.eat(&Token::Semicolon) {}
    }

    fn expect_eof(&mut self) -> PResult<()> {
        if self.at_eof() {
            Ok(())
        } else {
            Err(ParseError::syntax(
                self.span_here(),
                format!("unexpected trailing input: {:?}", self.peek()),
            ))
        }
    }

    /// Parse a name (identifier or backtick-quoted identifier).
    fn parse_name(&mut self, what: &str) -> PResult<String> {
        match self.peek().clone() {
            Token::Identifier(name) | Token::QuotedIdentifier(name) => {
                self.advance();
                Ok(name)
            }
            _ => Err(ParseError::syntax(
                self.span_here(),
                format!("expected {what}, found {:?}", self.peek()),
            )),
        }
    }

    // -- queries / clauses ------------------------------------------------

    fn parse_one_query(&mut self) -> PResult<Query> {
        let start = self.span_here();
        let mut parts = Vec::new();

        let first = self.parse_single_query()?;
        let mut last_span = first.span;
        parts.push(UnionPart {
            union: None,
            query: first,
        });

        while self.eat_keyword(Keyword::Union) {
            let kind = if self.eat_keyword(Keyword::All) {
                UnionKind::All
            } else {
                UnionKind::Distinct
            };
            let q = self.parse_single_query()?;
            last_span = q.span;
            parts.push(UnionPart {
                union: Some(kind),
                query: q,
            });
        }

        Ok(Query {
            parts,
            span: start.to(last_span),
        })
    }

    fn parse_single_query(&mut self) -> PResult<SingleQuery> {
        let start = self.span_here();
        let mut clauses = Vec::new();
        loop {
            match self.peek() {
                Token::Keyword(Keyword::Match) => {
                    clauses.push(Clause::Match(self.parse_match(false)?))
                }
                Token::Keyword(Keyword::Optional) => {
                    // OPTIONAL MATCH
                    self.advance();
                    if !self.peek().is_keyword(Keyword::Match) {
                        return Err(ParseError::syntax(
                            self.span_here(),
                            "expected MATCH after OPTIONAL",
                        ));
                    }
                    clauses.push(Clause::Match(self.parse_match(true)?));
                }
                Token::Keyword(Keyword::Create) => {
                    clauses.push(Clause::Create(self.parse_create()?))
                }
                Token::Keyword(Keyword::Merge) => clauses.push(Clause::Merge(self.parse_merge()?)),
                Token::Keyword(Keyword::Delete) | Token::Keyword(Keyword::Detach) => {
                    clauses.push(Clause::Delete(self.parse_delete()?))
                }
                Token::Keyword(Keyword::Set) => clauses.push(Clause::Set(self.parse_set()?)),
                Token::Keyword(Keyword::Remove) => {
                    clauses.push(Clause::Remove(self.parse_remove()?))
                }
                Token::Keyword(Keyword::With) => clauses.push(Clause::With(self.parse_with()?)),
                Token::Keyword(Keyword::Unwind) => {
                    clauses.push(Clause::Unwind(self.parse_unwind()?))
                }
                Token::Keyword(Keyword::Return) => {
                    clauses.push(Clause::Return(self.parse_return()?));
                    break; // RETURN ends a single query
                }
                Token::Keyword(Keyword::Call) => {
                    clauses.push(Clause::Call(self.parse_call()?))
                }
                _ => break,
            }
            // a UNION or EOF/semicolon ends the single query
            if matches!(
                self.peek(),
                Token::Keyword(Keyword::Union) | Token::Eof | Token::Semicolon
            ) {
                break;
            }
        }

        if clauses.is_empty() {
            return Err(ParseError::syntax(
                self.span_here(),
                format!("expected a clause, found {:?}", self.peek()),
            ));
        }

        let end = clauses.last().map(|c| c.span()).unwrap_or(start);
        Ok(SingleQuery {
            clauses,
            span: start.to(end),
        })
    }

    fn parse_match(&mut self, optional: bool) -> PResult<MatchClause> {
        let start = self.span_here();
        self.expect(&Token::Keyword(Keyword::Match), "MATCH")?;
        let patterns = self.parse_pattern_list()?;
        let where_clause = if self.eat_keyword(Keyword::Where) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(MatchClause {
            optional,
            patterns,
            where_clause,
            span: start.to(self.prev_span()),
        })
    }

    fn parse_create(&mut self) -> PResult<CreateClause> {
        let start = self.span_here();
        self.expect(&Token::Keyword(Keyword::Create), "CREATE")?;
        let patterns = self.parse_pattern_list()?;
        Ok(CreateClause {
            patterns,
            span: start.to(self.prev_span()),
        })
    }

    fn parse_merge(&mut self) -> PResult<MergeClause> {
        let start = self.span_here();
        self.expect(&Token::Keyword(Keyword::Merge), "MERGE")?;
        let pattern = self.parse_path_pattern()?;
        let mut on_create = Vec::new();
        let mut on_match = Vec::new();
        while self.eat_keyword(Keyword::On) {
            if self.eat_keyword(Keyword::Create) {
                self.expect(&Token::Keyword(Keyword::Set), "SET after ON CREATE")?;
                on_create = self.parse_set_items()?;
            } else if self.eat_keyword(Keyword::Match) {
                self.expect(&Token::Keyword(Keyword::Set), "SET after ON MATCH")?;
                on_match = self.parse_set_items()?;
            } else {
                return Err(ParseError::syntax(
                    self.span_here(),
                    "expected CREATE or MATCH after ON",
                ));
            }
        }
        Ok(MergeClause {
            pattern,
            on_create,
            on_match,
            span: start.to(self.prev_span()),
        })
    }

    fn parse_delete(&mut self) -> PResult<DeleteClause> {
        let start = self.span_here();
        let detach = self.eat_keyword(Keyword::Detach);
        self.expect(&Token::Keyword(Keyword::Delete), "DELETE")?;
        let mut targets = vec![self.parse_expr()?];
        while self.eat(&Token::Comma) {
            targets.push(self.parse_expr()?);
        }
        Ok(DeleteClause {
            detach,
            targets,
            span: start.to(self.prev_span()),
        })
    }

    fn parse_set(&mut self) -> PResult<SetClause> {
        let start = self.span_here();
        self.expect(&Token::Keyword(Keyword::Set), "SET")?;
        let items = self.parse_set_items()?;
        Ok(SetClause {
            items,
            span: start.to(self.prev_span()),
        })
    }

    fn parse_set_items(&mut self) -> PResult<Vec<SetItem>> {
        let mut items = vec![self.parse_set_item()?];
        while self.eat(&Token::Comma) {
            items.push(self.parse_set_item()?);
        }
        Ok(items)
    }

    fn parse_set_item(&mut self) -> PResult<SetItem> {
        let name = self.parse_name("a variable in SET")?;
        // var:Label...
        if self.peek() == &Token::Colon {
            let labels = self.parse_label_list()?;
            return Ok(SetItem::Labels {
                variable: name,
                labels,
            });
        }
        // var += map  /  var = map
        if self.eat(&Token::PlusEq) {
            let value = self.parse_expr()?;
            return Ok(SetItem::Properties {
                variable: name,
                merge: true,
                value,
            });
        }
        // property path: var(.key)+ = value
        if self.peek() == &Token::Dot {
            let mut target = Expr::Variable(name);
            while self.eat(&Token::Dot) {
                let key = self.parse_name("a property key")?;
                target = Expr::property(target, key);
            }
            self.expect(&Token::Eq, "= in SET property assignment")?;
            let value = self.parse_expr()?;
            return Ok(SetItem::Property { target, value });
        }
        // var = map (whole-entity replace)
        if self.eat(&Token::Eq) {
            let value = self.parse_expr()?;
            return Ok(SetItem::Properties {
                variable: name,
                merge: false,
                value,
            });
        }
        Err(ParseError::syntax(
            self.span_here(),
            "expected ':', '+=', '=', or '.' in SET item",
        ))
    }

    fn parse_remove(&mut self) -> PResult<RemoveClause> {
        let start = self.span_here();
        self.expect(&Token::Keyword(Keyword::Remove), "REMOVE")?;
        let mut items = vec![self.parse_remove_item()?];
        while self.eat(&Token::Comma) {
            items.push(self.parse_remove_item()?);
        }
        Ok(RemoveClause {
            items,
            span: start.to(self.prev_span()),
        })
    }

    fn parse_remove_item(&mut self) -> PResult<RemoveItem> {
        let name = self.parse_name("a variable in REMOVE")?;
        if self.peek() == &Token::Colon {
            let labels = self.parse_label_list()?;
            return Ok(RemoveItem::Labels {
                variable: name,
                labels,
            });
        }
        if self.peek() == &Token::Dot {
            let mut target = Expr::Variable(name);
            while self.eat(&Token::Dot) {
                let key = self.parse_name("a property key")?;
                target = Expr::property(target, key);
            }
            return Ok(RemoveItem::Property { target });
        }
        Err(ParseError::syntax(
            self.span_here(),
            "expected '.key' or ':Label' in REMOVE item",
        ))
    }

    fn parse_with(&mut self) -> PResult<WithClause> {
        let start = self.span_here();
        self.expect(&Token::Keyword(Keyword::With), "WITH")?;
        let projection = self.parse_projection()?;
        let where_clause = if self.eat_keyword(Keyword::Where) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(WithClause {
            projection,
            where_clause,
            span: start.to(self.prev_span()),
        })
    }

    fn parse_return(&mut self) -> PResult<ReturnClause> {
        let start = self.span_here();
        self.expect(&Token::Keyword(Keyword::Return), "RETURN")?;
        let projection = self.parse_projection()?;
        Ok(ReturnClause {
            projection,
            span: start.to(self.prev_span()),
        })
    }

    fn parse_unwind(&mut self) -> PResult<UnwindClause> {
        let start = self.span_here();
        self.expect(&Token::Keyword(Keyword::Unwind), "UNWIND")?;
        let expr = self.parse_expr()?;
        self.expect(&Token::Keyword(Keyword::As), "AS in UNWIND")?;
        let alias = self.parse_name("an UNWIND alias")?;
        Ok(UnwindClause {
            expr,
            alias,
            span: start.to(self.prev_span()),
        })
    }

    /// `CALL <dotted.name>() [YIELD col [AS alias], …]`
    ///
    /// Only nullary read-only catalog procedures are accepted (Unit 14); a
    /// non-empty argument list is feature-tagged as unsupported.
    fn parse_call(&mut self) -> PResult<CallClause> {
        let start = self.span_here();
        self.expect(&Token::Keyword(Keyword::Call), "CALL")?;
        let mut name = self.parse_name("a procedure name")?;
        while self.eat(&Token::Dot) {
            name.push('.');
            name.push_str(&self.parse_name("a procedure name segment")?);
        }
        self.expect(&Token::LParen, "( after procedure name")?;
        if self.peek() != &Token::RParen {
            return Err(ParseError::unsupported(
                self.span_here(),
                GqlFeature::ProcedureCall,
                "procedure arguments are not supported yet (Unit 14)",
            ));
        }
        self.expect(&Token::RParen, ") after procedure arguments")?;
        let mut yields = Vec::new();
        let mut where_clause = None;
        if self.eat_keyword(Keyword::Yield) {
            loop {
                let col = self.parse_name("a YIELD column")?;
                let alias = if self.eat_keyword(Keyword::As) {
                    Some(self.parse_name("a YIELD alias")?)
                } else {
                    None
                };
                yields.push((col, alias));
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
            // `YIELD … WHERE <expr>` filters the procedure rows.
            if self.eat_keyword(Keyword::Where) {
                where_clause = Some(self.parse_expr()?);
            }
        }
        Ok(CallClause {
            name,
            yields,
            where_clause,
            span: start.to(self.prev_span()),
        })
    }

    fn parse_projection(&mut self) -> PResult<Projection> {
        let distinct = self.eat_keyword(Keyword::Distinct);
        let mut star = false;
        let mut items = Vec::new();
        if self.eat(&Token::Star) {
            star = true;
            // RETURN *, extra  (optional additional items)
            while self.eat(&Token::Comma) {
                items.push(self.parse_return_item()?);
            }
        } else {
            items.push(self.parse_return_item()?);
            while self.eat(&Token::Comma) {
                items.push(self.parse_return_item()?);
            }
        }

        let mut order_by = Vec::new();
        if self.eat_keyword(Keyword::Order) {
            self.expect(&Token::Keyword(Keyword::By), "BY after ORDER")?;
            order_by.push(self.parse_order_item()?);
            while self.eat(&Token::Comma) {
                order_by.push(self.parse_order_item()?);
            }
        }
        let skip = if self.eat_keyword(Keyword::Skip) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        let limit = if self.eat_keyword(Keyword::Limit) {
            Some(self.parse_expr()?)
        } else {
            None
        };

        Ok(Projection {
            distinct,
            star,
            items,
            order_by,
            skip,
            limit,
        })
    }

    fn parse_return_item(&mut self) -> PResult<ReturnItem> {
        let expr = self.parse_expr()?;
        let alias = if self.eat_keyword(Keyword::As) {
            Some(self.parse_name("an alias after AS")?)
        } else {
            None
        };
        Ok(ReturnItem { expr, alias })
    }

    fn parse_order_item(&mut self) -> PResult<OrderItem> {
        let expr = self.parse_expr()?;
        let descending = if self.eat_keyword(Keyword::Desc) {
            true
        } else {
            let _ = self.eat_keyword(Keyword::Asc);
            false
        };
        Ok(OrderItem { expr, descending })
    }

    // -- patterns ---------------------------------------------------------

    fn parse_pattern_list(&mut self) -> PResult<Vec<PathPattern>> {
        let mut patterns = vec![self.parse_path_pattern()?];
        while self.eat(&Token::Comma) {
            patterns.push(self.parse_path_pattern()?);
        }
        Ok(patterns)
    }

    fn parse_path_pattern(&mut self) -> PResult<PathPattern> {
        let start = self.span_here();
        // optional path variable: `p = (...)`
        let variable = if matches!(
            self.peek(),
            Token::Identifier(_) | Token::QuotedIdentifier(_)
        ) && self.peek_at(1) == &Token::Eq
        {
            let name = self.parse_name("a path variable")?;
            self.expect(&Token::Eq, "= in path assignment")?;
            Some(name)
        } else {
            None
        };

        let node_start = self.parse_node_pattern()?;
        let mut segments = Vec::new();
        while matches!(self.peek(), Token::Minus | Token::ArrowLeft) {
            let relationship = self.parse_relationship_pattern()?;
            let node = self.parse_node_pattern()?;
            segments.push(PathSegment { relationship, node });
        }

        Ok(PathPattern {
            variable,
            start: node_start,
            segments,
            span: start.to(self.prev_span()),
        })
    }

    fn parse_node_pattern(&mut self) -> PResult<NodePattern> {
        let start = self.span_here();
        self.expect(&Token::LParen, "'(' to start a node pattern")?;
        let variable = if matches!(
            self.peek(),
            Token::Identifier(_) | Token::QuotedIdentifier(_)
        ) {
            Some(self.parse_name("a node variable")?)
        } else {
            None
        };
        let labels = if self.peek() == &Token::Colon {
            self.parse_label_list()?
        } else {
            Vec::new()
        };
        let properties = if self.peek() == &Token::LBrace {
            Some(self.parse_map_literal()?)
        } else {
            None
        };
        self.expect(&Token::RParen, "')' to close a node pattern")?;
        Ok(NodePattern {
            variable,
            labels,
            properties,
            span: start.to(self.prev_span()),
        })
    }

    fn parse_relationship_pattern(&mut self) -> PResult<RelationshipPattern> {
        let start = self.span_here();
        let left_in = match self.peek() {
            Token::ArrowLeft => {
                self.advance();
                true
            }
            Token::Minus => {
                self.advance();
                false
            }
            _ => {
                return Err(ParseError::syntax(
                    self.span_here(),
                    "expected a relationship pattern",
                ));
            }
        };

        let mut variable = None;
        let mut types = Vec::new();
        let mut properties = None;
        let mut length = None;
        if self.eat(&Token::LBracket) {
            if matches!(
                self.peek(),
                Token::Identifier(_) | Token::QuotedIdentifier(_)
            ) {
                variable = Some(self.parse_name("a relationship variable")?);
            }
            if self.peek() == &Token::Colon {
                self.advance();
                types.push(self.parse_name("a relationship type")?);
                while self.eat(&Token::Pipe) {
                    types.push(self.parse_name("a relationship type")?);
                }
            }
            if self.eat(&Token::Star) {
                length = Some(self.parse_range_literal()?);
            }
            if self.peek() == &Token::LBrace {
                properties = Some(self.parse_map_literal()?);
            }
            self.expect(&Token::RBracket, "']' to close a relationship pattern")?;
        }

        let right_out = match self.peek() {
            Token::Arrow => {
                self.advance();
                true
            }
            Token::Minus => {
                self.advance();
                false
            }
            _ => {
                return Err(ParseError::syntax(
                    self.span_here(),
                    "expected '-' or '->' to close a relationship pattern",
                ));
            }
        };

        let direction = match (left_in, right_out) {
            (true, false) => Direction::Incoming,
            (false, true) => Direction::Outgoing,
            (false, false) => Direction::Undirected,
            (true, true) => {
                return Err(ParseError::syntax(
                    start.to(self.prev_span()),
                    "a relationship cannot point in both directions (<-...->)",
                ));
            }
        };

        Ok(RelationshipPattern {
            variable,
            types,
            direction,
            properties,
            length,
            span: start.to(self.prev_span()),
        })
    }

    fn parse_range_literal(&mut self) -> PResult<RangeLiteral> {
        // already consumed `*`
        let min = if let Token::Integer(n) = self.peek() {
            let n = *n;
            self.advance();
            Some(n as u64)
        } else {
            None
        };
        if self.eat(&Token::DotDot) {
            let max = if let Token::Integer(n) = self.peek() {
                let n = *n;
                self.advance();
                Some(n as u64)
            } else {
                None
            };
            Ok(RangeLiteral { min, max })
        } else {
            // `*` or `*n` (exact: min == max when n given, unbounded otherwise)
            Ok(RangeLiteral { min, max: min })
        }
    }

    fn parse_label_list(&mut self) -> PResult<Vec<String>> {
        let mut labels = Vec::new();
        while self.eat(&Token::Colon) {
            labels.push(self.parse_name("a label")?);
        }
        Ok(labels)
    }

    fn parse_map_literal(&mut self) -> PResult<MapLiteral> {
        let start = self.span_here();
        self.expect(&Token::LBrace, "'{' to start a map")?;
        let mut entries = Vec::new();
        if self.peek() != &Token::RBrace {
            loop {
                let key = self.parse_map_key()?;
                self.expect(&Token::Colon, "':' in a map entry")?;
                let value = self.parse_expr()?;
                entries.push((key, value));
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
        }
        self.expect(&Token::RBrace, "'}' to close a map")?;
        Ok(MapLiteral {
            entries,
            span: start.to(self.prev_span()),
        })
    }

    fn parse_map_key(&mut self) -> PResult<String> {
        match self.peek().clone() {
            Token::Identifier(name) | Token::QuotedIdentifier(name) | Token::String(name) => {
                self.advance();
                Ok(name)
            }
            _ => Err(ParseError::syntax(
                self.span_here(),
                format!("expected a map key, found {:?}", self.peek()),
            )),
        }
    }

    // -- expressions (Pratt) ---------------------------------------------

    fn parse_expr(&mut self) -> PResult<Expr> {
        self.parse_expr_bp(0)
    }

    fn parse_expr_bp(&mut self, min_bp: u8) -> PResult<Expr> {
        let mut lhs = self.parse_prefix()?;

        loop {
            // postfix: IS [NOT] NULL
            if self.peek().is_keyword(Keyword::Is) {
                self.advance();
                let negated = self.eat_keyword(Keyword::Not);
                self.expect(&Token::Keyword(Keyword::Null), "NULL after IS")?;
                lhs = Expr::IsNull {
                    operand: Box::new(lhs),
                    negated,
                };
                continue;
            }

            let Some((op, consumed)) = self.peek_binary_op() else {
                break;
            };
            let bp = op.binding_power();
            if bp < min_bp {
                break;
            }
            // consume the operator token(s)
            for _ in 0..consumed {
                self.advance();
            }
            // left-associative: parse rhs with bp+1
            let rhs = self.parse_expr_bp(bp + 1)?;
            lhs = Expr::binary(op, lhs, rhs);
        }
        Ok(lhs)
    }

    /// Inspect the current token(s) for an infix binary operator, returning the
    /// operator and how many tokens it spans (1 normally, 2 for STARTS/ENDS WITH).
    fn peek_binary_op(&self) -> Option<(BinaryOp, usize)> {
        let op = match self.peek() {
            Token::Plus => BinaryOp::Add,
            Token::Minus => BinaryOp::Subtract,
            Token::Star => BinaryOp::Multiply,
            Token::Slash => BinaryOp::Divide,
            Token::Percent => BinaryOp::Modulo,
            Token::Caret => BinaryOp::Power,
            Token::Eq => BinaryOp::Eq,
            Token::Ne => BinaryOp::Ne,
            Token::Lt => BinaryOp::Lt,
            Token::Le => BinaryOp::Le,
            Token::Gt => BinaryOp::Gt,
            Token::Ge => BinaryOp::Ge,
            Token::Keyword(Keyword::And) => BinaryOp::And,
            Token::Keyword(Keyword::Or) => BinaryOp::Or,
            Token::Keyword(Keyword::Xor) => BinaryOp::Xor,
            Token::Keyword(Keyword::In) => BinaryOp::In,
            Token::Keyword(Keyword::Contains) => BinaryOp::Contains,
            // STARTS WITH / ENDS WITH appear as Identifier + WITH in infix position
            Token::Identifier(word)
                if word.eq_ignore_ascii_case("starts")
                    && self.peek_at(1).is_keyword(Keyword::With) =>
            {
                return Some((BinaryOp::StartsWith, 2));
            }
            Token::Identifier(word)
                if word.eq_ignore_ascii_case("ends")
                    && self.peek_at(1).is_keyword(Keyword::With) =>
            {
                return Some((BinaryOp::EndsWith, 2));
            }
            _ => return None,
        };
        Some((op, 1))
    }

    fn parse_prefix(&mut self) -> PResult<Expr> {
        // unary operators
        if self.eat_keyword(Keyword::Not) {
            // NOT binds looser than comparison (so it captures `x = y`) but
            // tighter than AND/XOR/OR (so `NOT a AND b` parses as `(NOT a) AND b`).
            let operand = self.parse_expr_bp(BinaryOp::Eq.binding_power())?;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                operand: Box::new(operand),
            });
        }
        if self.eat(&Token::Minus) {
            let operand = self.parse_prefix()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Negate,
                operand: Box::new(operand),
            });
        }
        if self.eat(&Token::Plus) {
            let operand = self.parse_prefix()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Plus,
                operand: Box::new(operand),
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut expr = self.parse_atom()?;
        loop {
            match self.peek() {
                Token::Dot => {
                    self.advance();
                    let key = self.parse_name("a property key")?;
                    expr = Expr::property(expr, key);
                }
                Token::LBracket => {
                    self.advance();
                    let index = self.parse_expr()?;
                    self.expect(&Token::RBracket, "']' to close an index")?;
                    expr = Expr::Index {
                        base: Box::new(expr),
                        index: Box::new(index),
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_atom(&mut self) -> PResult<Expr> {
        match self.peek().clone() {
            Token::Keyword(Keyword::Null) => {
                self.advance();
                Ok(Expr::Null)
            }
            Token::Keyword(Keyword::True) => {
                self.advance();
                Ok(Expr::Boolean(true))
            }
            Token::Keyword(Keyword::False) => {
                self.advance();
                Ok(Expr::Boolean(false))
            }
            Token::Integer(n) => {
                self.advance();
                Ok(Expr::Integer(n))
            }
            Token::Float(f) => {
                self.advance();
                Ok(Expr::Float(f))
            }
            Token::String(s) => {
                self.advance();
                Ok(Expr::String(s))
            }
            Token::Parameter(name) => {
                self.advance();
                Ok(Expr::Parameter(name))
            }
            Token::LParen => {
                self.advance();
                let inner = self.parse_expr()?;
                self.expect(&Token::RParen, "')' to close a parenthesized expression")?;
                Ok(inner)
            }
            Token::LBracket => self.parse_list_literal(),
            Token::LBrace => {
                let map = self.parse_map_literal()?;
                Ok(Expr::Map(map.entries))
            }
            Token::Keyword(Keyword::Case) => self.parse_case(),
            Token::Keyword(Keyword::Exists) if self.peek_at(1) == &Token::LParen => {
                self.advance();
                self.parse_call_args("exists", false)
            }
            Token::Identifier(name) => {
                self.advance();
                if self.peek() == &Token::LParen {
                    let distinct_or_star = self.parse_call_args(&name, true)?;
                    Ok(distinct_or_star)
                } else {
                    Ok(Expr::Variable(name))
                }
            }
            Token::QuotedIdentifier(name) => {
                self.advance();
                Ok(Expr::Variable(name))
            }
            other => Err(ParseError::syntax(
                self.span_here(),
                format!("expected an expression, found {other:?}"),
            )),
        }
    }

    fn parse_call_args(&mut self, name: &str, allow_distinct: bool) -> PResult<Expr> {
        self.expect(&Token::LParen, "'(' for a function call")?;
        let mut distinct = false;
        let mut star = false;
        let mut args = Vec::new();
        if self.peek() == &Token::Star {
            self.advance();
            star = true;
        } else if self.peek() != &Token::RParen {
            if allow_distinct && self.eat_keyword(Keyword::Distinct) {
                distinct = true;
            }
            args.push(self.parse_expr()?);
            while self.eat(&Token::Comma) {
                args.push(self.parse_expr()?);
            }
        }
        self.expect(&Token::RParen, "')' to close a function call")?;
        Ok(Expr::Function {
            name: name.to_string(),
            distinct,
            star,
            args,
        })
    }

    fn parse_list_literal(&mut self) -> PResult<Expr> {
        self.expect(&Token::LBracket, "'[' to start a list")?;
        let mut items = Vec::new();
        if self.peek() != &Token::RBracket {
            items.push(self.parse_expr()?);
            while self.eat(&Token::Comma) {
                items.push(self.parse_expr()?);
            }
        }
        self.expect(&Token::RBracket, "']' to close a list")?;
        Ok(Expr::List(items))
    }

    fn parse_case(&mut self) -> PResult<Expr> {
        self.expect(&Token::Keyword(Keyword::Case), "CASE")?;
        // optional operand (simple CASE)
        let operand = if self.peek().is_keyword(Keyword::When) {
            None
        } else {
            Some(Box::new(self.parse_expr()?))
        };
        let mut branches = Vec::new();
        while self.eat_keyword(Keyword::When) {
            let when = self.parse_expr()?;
            self.expect(&Token::Keyword(Keyword::Then), "THEN in CASE")?;
            let then = self.parse_expr()?;
            branches.push(CaseBranch { when, then });
        }
        if branches.is_empty() {
            return Err(ParseError::syntax(
                self.span_here(),
                "CASE requires at least one WHEN branch",
            ));
        }
        let default = if self.eat_keyword(Keyword::Else) {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        self.expect(&Token::Keyword(Keyword::End), "END to close CASE")?;
        Ok(Expr::Case {
            operand,
            branches,
            default,
        })
    }

    /// Span of the token just consumed (for end-of-node spans).
    fn prev_span(&self) -> Span {
        let idx = self.pos.saturating_sub(1);
        self.tokens[idx].span
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(source: &str) -> Query {
        parse_query(source).unwrap_or_else(|e| panic!("parse failed: {}", e.render(source)))
    }

    fn e(source: &str) -> Expr {
        parse_expression(source).unwrap_or_else(|e| panic!("parse failed: {}", e.render(source)))
    }

    #[test]
    fn parses_simple_match_return() {
        let query = q("MATCH (n:Person) RETURN n");
        let sq = &query.parts[0].query;
        assert_eq!(sq.clauses.len(), 2);
        assert!(matches!(sq.clauses[0], Clause::Match(_)));
        assert!(matches!(sq.clauses[1], Clause::Return(_)));
    }

    #[test]
    fn parses_relationship_pattern_with_direction() {
        let query = q("MATCH (a:Person {id: 'p1'})-[e:KNOWS]->(b:Person) RETURN e");
        let Clause::Match(m) = &query.parts[0].query.clauses[0] else {
            panic!("expected match");
        };
        let path = &m.patterns[0];
        assert_eq!(path.start.variable.as_deref(), Some("a"));
        assert_eq!(path.segments.len(), 1);
        assert_eq!(path.segments[0].relationship.direction, Direction::Outgoing);
        assert_eq!(
            path.segments[0].relationship.types,
            vec!["KNOWS".to_string()]
        );
    }

    #[test]
    fn parses_incoming_and_undirected() {
        let q1 = q("MATCH (a)<-[:R]-(b) RETURN a");
        let Clause::Match(m1) = &q1.parts[0].query.clauses[0] else {
            panic!()
        };
        assert_eq!(
            m1.patterns[0].segments[0].relationship.direction,
            Direction::Incoming
        );

        let q2 = q("MATCH (a)-[:R]-(b) RETURN a");
        let Clause::Match(m2) = &q2.parts[0].query.clauses[0] else {
            panic!()
        };
        assert_eq!(
            m2.patterns[0].segments[0].relationship.direction,
            Direction::Undirected
        );
    }

    #[test]
    fn parses_variable_length_range() {
        let query = q("MATCH (a)-[:R*1..3]->(b) RETURN b");
        let Clause::Match(m) = &query.parts[0].query.clauses[0] else {
            panic!()
        };
        let len = m.patterns[0].segments[0].relationship.length.unwrap();
        assert_eq!(len.min, Some(1));
        assert_eq!(len.max, Some(3));
    }

    #[test]
    fn parses_node_properties_map() {
        let query = q("CREATE (:Person {id: 'p1', age: 30, active: true})");
        let Clause::Create(c) = &query.parts[0].query.clauses[0] else {
            panic!()
        };
        let props = c.patterns[0].start.properties.as_ref().unwrap();
        assert_eq!(props.entries.len(), 3);
        assert_eq!(props.entries[0].0, "id");
    }

    #[test]
    fn expression_precedence_is_correct() {
        // 1 + 2 * 3 => 1 + (2 * 3)
        let expr = e("1 + 2 * 3");
        match expr {
            Expr::Binary {
                op: BinaryOp::Add,
                rhs,
                ..
            } => {
                assert!(matches!(
                    *rhs,
                    Expr::Binary {
                        op: BinaryOp::Multiply,
                        ..
                    }
                ));
            }
            _ => panic!("expected add at top"),
        }
    }

    #[test]
    fn boolean_precedence_and_over_or() {
        // a OR b AND c => a OR (b AND c)
        let expr = e("a OR b AND c");
        match expr {
            Expr::Binary {
                op: BinaryOp::Or,
                rhs,
                ..
            } => {
                assert!(matches!(
                    *rhs,
                    Expr::Binary {
                        op: BinaryOp::And,
                        ..
                    }
                ));
            }
            _ => panic!("expected OR at top"),
        }
    }

    #[test]
    fn comparison_and_membership_and_string_ops() {
        assert!(matches!(
            e("n.age >= 18"),
            Expr::Binary {
                op: BinaryOp::Ge,
                ..
            }
        ));
        assert!(matches!(
            e("x IN [1, 2, 3]"),
            Expr::Binary {
                op: BinaryOp::In,
                ..
            }
        ));
        assert!(matches!(
            e("n.name STARTS WITH 'A'"),
            Expr::Binary {
                op: BinaryOp::StartsWith,
                ..
            }
        ));
        assert!(matches!(
            e("n.name ENDS WITH 'z'"),
            Expr::Binary {
                op: BinaryOp::EndsWith,
                ..
            }
        ));
        assert!(matches!(
            e("n.name CONTAINS 'oo'"),
            Expr::Binary {
                op: BinaryOp::Contains,
                ..
            }
        ));
    }

    #[test]
    fn is_null_postfix() {
        assert!(matches!(
            e("n.x IS NULL"),
            Expr::IsNull { negated: false, .. }
        ));
        assert!(matches!(
            e("n.x IS NOT NULL"),
            Expr::IsNull { negated: true, .. }
        ));
    }

    #[test]
    fn property_and_index_access() {
        assert!(matches!(e("n.name"), Expr::Property { .. }));
        assert!(matches!(e("xs[0]"), Expr::Index { .. }));
        // chained
        match e("a.b.c") {
            Expr::Property { key, .. } => assert_eq!(key, "c"),
            _ => panic!(),
        }
    }

    #[test]
    fn function_calls_count_star_and_distinct() {
        match e("count(*)") {
            Expr::Function { name, star, .. } => {
                assert_eq!(name, "count");
                assert!(star);
            }
            _ => panic!(),
        }
        match e("count(DISTINCT n.id)") {
            Expr::Function { distinct, .. } => assert!(distinct),
            _ => panic!(),
        }
        match e("coalesce(a, b, 0)") {
            Expr::Function { args, .. } => assert_eq!(args.len(), 3),
            _ => panic!(),
        }
    }

    #[test]
    fn case_expression() {
        let expr = e("CASE WHEN n.age >= 18 THEN 'adult' ELSE 'minor' END");
        match expr {
            Expr::Case {
                branches,
                default,
                operand,
            } => {
                assert!(operand.is_none());
                assert_eq!(branches.len(), 1);
                assert!(default.is_some());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_with_pipeline_and_where() {
        let query = q("MATCH (n:Person) WITH n WHERE n.age > 18 RETURN n.name AS name");
        let clauses = &query.parts[0].query.clauses;
        assert!(matches!(clauses[0], Clause::Match(_)));
        assert!(matches!(clauses[1], Clause::With(_)));
        let Clause::With(w) = &clauses[1] else {
            panic!()
        };
        assert!(w.where_clause.is_some());
        let Clause::Return(r) = &clauses[2] else {
            panic!()
        };
        assert_eq!(r.projection.items[0].alias.as_deref(), Some("name"));
    }

    #[test]
    fn parses_return_distinct_star_order_skip_limit() {
        let query = q("MATCH (n) RETURN DISTINCT * ORDER BY n.name DESC SKIP 5 LIMIT 10");
        let Clause::Return(r) = query.parts[0].query.clauses.last().unwrap() else {
            panic!()
        };
        assert!(r.projection.distinct);
        assert!(r.projection.star);
        assert_eq!(r.projection.order_by.len(), 1);
        assert!(r.projection.order_by[0].descending);
        assert!(r.projection.skip.is_some());
        assert!(r.projection.limit.is_some());
    }

    #[test]
    fn parses_set_remove_delete_forms() {
        let s = q("MATCH (n:Person {id:'p1'}) SET n.active = true, n += {score: 9} REMOVE n.tmp");
        let clauses = &s.parts[0].query.clauses;
        let Clause::Set(set) = &clauses[1] else {
            panic!("expected SET")
        };
        assert_eq!(set.items.len(), 2);
        assert!(matches!(set.items[0], SetItem::Property { .. }));
        assert!(matches!(
            set.items[1],
            SetItem::Properties { merge: true, .. }
        ));
        assert!(matches!(&clauses[2], Clause::Remove(_)));

        let d = q("MATCH (n) DETACH DELETE n");
        let Clause::Delete(del) = d.parts[0].query.clauses.last().unwrap() else {
            panic!()
        };
        assert!(del.detach);
    }

    #[test]
    fn parses_merge_with_on_create_on_match() {
        let query = q(
            "MERGE (n:Person {id:'p1'}) ON CREATE SET n.created = true ON MATCH SET n.seen = true",
        );
        let Clause::Merge(m) = &query.parts[0].query.clauses[0] else {
            panic!()
        };
        assert_eq!(m.on_create.len(), 1);
        assert_eq!(m.on_match.len(), 1);
    }

    #[test]
    fn parses_union_all() {
        let query = q("MATCH (a:A) RETURN a UNION ALL MATCH (b:B) RETURN b");
        assert_eq!(query.parts.len(), 2);
        assert_eq!(query.parts[1].union, Some(UnionKind::All));
    }

    #[test]
    fn parses_unwind() {
        let query = q("UNWIND [1, 2, 3] AS x RETURN x");
        assert!(matches!(query.parts[0].query.clauses[0], Clause::Unwind(_)));
    }

    #[test]
    fn parses_optional_match() {
        let query = q("MATCH (a:Person) OPTIONAL MATCH (a)-[:KNOWS]->(b) RETURN a, b");
        let Clause::Match(m) = &query.parts[0].query.clauses[1] else {
            panic!()
        };
        assert!(m.optional);
    }

    #[test]
    fn parses_path_variable() {
        let query = q("MATCH p = (a)-[:R]->(b) RETURN p");
        let Clause::Match(m) = &query.parts[0].query.clauses[0] else {
            panic!()
        };
        assert_eq!(m.patterns[0].variable.as_deref(), Some("p"));
    }

    #[test]
    fn parameters_parse() {
        assert!(matches!(e("$id"), Expr::Parameter(_)));
        let query = q("MATCH (n:Person {id: $id}) RETURN n");
        let Clause::Match(m) = &query.parts[0].query.clauses[0] else {
            panic!()
        };
        let props = m.patterns[0].start.properties.as_ref().unwrap();
        assert!(matches!(props.entries[0].1, Expr::Parameter(_)));
    }

    // -- error / span behavior -------------------------------------------

    #[test]
    fn syntax_error_carries_span() {
        let err = parse_query("MATCH (n RETURN n").unwrap_err();
        assert_eq!(err.kind, ParseErrorKind::Syntax);
        let rendered = err.render("MATCH (n RETURN n");
        assert!(rendered.contains("line 1"));
    }

    #[test]
    fn lexical_error_propagates_as_syntax() {
        let err = parse_query("RETURN 'unterminated").unwrap_err();
        assert_eq!(err.kind, ParseErrorKind::Syntax);
    }

    #[test]
    fn trailing_input_is_rejected() {
        assert!(parse_query("RETURN 1 RETURN 2 garbage").is_ok() == false);
    }

    #[test]
    fn multiple_statements_split() {
        let queries = parse_statements("CREATE (:A {id:'1'}); CREATE (:B {id:'2'})").unwrap();
        assert_eq!(queries.len(), 2);
    }

    #[test]
    fn parse_error_converts_to_grust_syntax() {
        let src = "MATCH (n RETURN n";
        let err = parse_query(src).unwrap_err().into_grust(src);
        assert!(matches!(err, GrustError::CypherSyntax(_)));
    }

    #[test]
    fn recognized_unsupported_construct_is_feature_tagged() {
        // Nullary catalog procedures now parse (Unit 14); procedure *arguments*
        // remain the recognized-but-unsupported construct.
        let err = parse_query("CALL db.foo(1)").unwrap_err();
        assert_eq!(
            err.kind,
            ParseErrorKind::Unsupported(GqlFeature::ProcedureCall)
        );
        // and it converts into the structured unsupported-feature transport
        let grust = err.into_grust("CALL db.foo(1)");
        assert!(matches!(grust, GrustError::Unsupported(_)));
        assert!(grust.to_string().contains("feature=procedure-call"));
    }

    #[test]
    fn parse_standalone_call() {
        let q = &parse_query("CALL db.labels()").unwrap();
        match &q.parts[0].query.clauses[0] {
            Clause::Call(c) => {
                assert_eq!(c.name, "db.labels");
                assert!(c.yields.is_empty());
            }
            other => panic!("expected CALL clause, got {other:?}"),
        }
    }

    #[test]
    fn parse_call_yield_with_alias() {
        let q = &parse_query("CALL db.labels() YIELD label AS l RETURN l").unwrap();
        match &q.parts[0].query.clauses[0] {
            Clause::Call(c) => {
                assert_eq!(c.name, "db.labels");
                assert_eq!(c.yields, vec![("label".to_string(), Some("l".to_string()))]);
            }
            other => panic!("expected CALL clause, got {other:?}"),
        }
    }

    #[test]
    fn not_binds_tighter_than_and() {
        // NOT a AND b  ==  (NOT a) AND b
        match e("NOT a AND b") {
            Expr::Binary {
                op: BinaryOp::And,
                lhs,
                ..
            } => {
                assert!(matches!(
                    *lhs,
                    Expr::Unary {
                        op: UnaryOp::Not,
                        ..
                    }
                ));
            }
            other => panic!("expected AND at top, got {other:?}"),
        }
    }

    #[test]
    fn not_captures_comparison() {
        // NOT x = y  ==  NOT (x = y)
        match e("NOT x = y") {
            Expr::Unary {
                op: UnaryOp::Not,
                operand,
            } => {
                assert!(matches!(
                    *operand,
                    Expr::Binary {
                        op: BinaryOp::Eq,
                        ..
                    }
                ));
            }
            other => panic!("expected NOT at top, got {other:?}"),
        }
    }
}
