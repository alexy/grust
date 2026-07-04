//! Lexer with source spans (Unit 3 of `docs/GQL_GOAL.md`).
//!
//! Tokenizes GQL/Cypher source text into a flat stream of [`SpannedToken`]s
//! carrying byte-offset [`Span`]s. It recognizes comments, case-insensitive
//! keywords, bare and backtick-quoted identifiers, `$`-parameters, the numeric
//! and string literal families, operators/punctuation (including the
//! relationship arrows and the `..` range), and the `;` statement separator.
//!
//! This module is additive: it does not yet replace the existing hand-written
//! `cypher_*` parser entrypoints (which remain as compatibility wrappers). It is
//! the span-bearing foundation those entrypoints will route through as the typed
//! AST path lands in Unit 4. Lexical failures are reported as span-bearing
//! [`LexError`]s and convert into the standard structured `GqlError` syntax
//! channel via [`LexError::into_grust`].

use std::fmt;

use grust_core::GrustError;

use crate::gql::{GqlError, GqlErrorKind, gql_syntax};

/// A half-open byte range `[start, end)` into the source text.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        debug_assert!(start <= end, "span start must not exceed end");
        Span { start, end }
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// The substring of `source` covered by this span.
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start..self.end]
    }

    /// Merge two spans into the smallest span covering both.
    pub fn to(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// 1-based line and column for a byte offset, for human-readable diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineCol {
    pub line: usize,
    pub column: usize,
}

/// Compute the 1-based line/column of a byte offset in `source`.
pub fn line_col(source: &str, offset: usize) -> LineCol {
    let mut line = 1;
    let mut column = 1;
    for (idx, ch) in source.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    LineCol { line, column }
}

/// Reserved/keyword tokens, recognized case-insensitively.
///
/// Bare words that are not keywords lex as [`Token::Identifier`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Keyword {
    Match,
    Optional,
    Create,
    Merge,
    Delete,
    Detach,
    Set,
    Remove,
    Return,
    Where,
    With,
    Unwind,
    Union,
    All,
    Distinct,
    As,
    Order,
    By,
    Skip,
    Limit,
    Asc,
    Desc,
    And,
    Or,
    Xor,
    Not,
    In,
    Is,
    Null,
    True,
    False,
    StartsWith,
    EndsWith,
    Contains,
    Constraint,
    Index,
    For,
    Require,
    Unique,
    Exists,
    On,
    Drop,
    Use,
    Call,
    Yield,
    Case,
    When,
    Then,
    Else,
    End,
}

impl Keyword {
    /// Map an already-uppercased word to a keyword, if any.
    fn from_upper(word: &str) -> Option<Keyword> {
        Some(match word {
            "MATCH" => Keyword::Match,
            "OPTIONAL" => Keyword::Optional,
            "CREATE" => Keyword::Create,
            "MERGE" => Keyword::Merge,
            "DELETE" => Keyword::Delete,
            "DETACH" => Keyword::Detach,
            "SET" => Keyword::Set,
            "REMOVE" => Keyword::Remove,
            "RETURN" => Keyword::Return,
            "WHERE" => Keyword::Where,
            "WITH" => Keyword::With,
            "UNWIND" => Keyword::Unwind,
            "UNION" => Keyword::Union,
            "ALL" => Keyword::All,
            "DISTINCT" => Keyword::Distinct,
            "AS" => Keyword::As,
            "ORDER" => Keyword::Order,
            "BY" => Keyword::By,
            "SKIP" => Keyword::Skip,
            "LIMIT" => Keyword::Limit,
            "ASC" | "ASCENDING" => Keyword::Asc,
            "DESC" | "DESCENDING" => Keyword::Desc,
            "AND" => Keyword::And,
            "OR" => Keyword::Or,
            "XOR" => Keyword::Xor,
            "NOT" => Keyword::Not,
            "IN" => Keyword::In,
            "IS" => Keyword::Is,
            "NULL" => Keyword::Null,
            "TRUE" => Keyword::True,
            "FALSE" => Keyword::False,
            "CONSTRAINT" => Keyword::Constraint,
            "INDEX" => Keyword::Index,
            "FOR" => Keyword::For,
            "REQUIRE" => Keyword::Require,
            "UNIQUE" => Keyword::Unique,
            "EXISTS" => Keyword::Exists,
            "ON" => Keyword::On,
            "DROP" => Keyword::Drop,
            "USE" => Keyword::Use,
            "CALL" => Keyword::Call,
            "YIELD" => Keyword::Yield,
            "CASE" => Keyword::Case,
            "WHEN" => Keyword::When,
            "THEN" => Keyword::Then,
            "ELSE" => Keyword::Else,
            "END" => Keyword::End,
            // Multi-word predicates (STARTS WITH / ENDS WITH) are matched by the
            // parser from the WITH/individual word tokens; CONTAINS is single.
            "CONTAINS" => Keyword::Contains,
            _ => return None,
        })
    }
}

/// A lexical token.
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    // literals
    Integer(i64),
    Float(f64),
    String(String),

    // names
    Identifier(String),
    QuotedIdentifier(String),
    Parameter(String),
    Keyword(Keyword),

    // grouping
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,

    // punctuation
    Colon,
    Comma,
    Dot,
    DotDot,
    Semicolon,
    Pipe,

    // comparison
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,

    // arithmetic / assignment
    Plus,
    PlusEq,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,

    // relationship arrows
    Arrow,     // ->
    ArrowLeft, // <-

    /// End of input.
    Eof,
}

impl Token {
    /// True when this token is a keyword equal to `kw`.
    pub fn is_keyword(&self, kw: Keyword) -> bool {
        matches!(self, Token::Keyword(k) if *k == kw)
    }
}

/// A token paired with its source span.
#[derive(Clone, Debug, PartialEq)]
pub struct SpannedToken {
    pub token: Token,
    pub span: Span,
}

/// A span-bearing lexical error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexError {
    pub span: Span,
    pub message: String,
}

impl LexError {
    fn new(span: Span, message: impl Into<String>) -> Self {
        LexError {
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

    /// Convert into the standard structured GQL syntax error, embedding the
    /// resolved line/column so the span survives the transport conversion.
    pub fn into_grust(self, source: &str) -> GrustError {
        gql_syntax(self.render(source))
    }

    /// Build a structured [`GqlError`] (syntax kind) without a source handle.
    pub fn into_gql_error(self) -> GqlError {
        GqlError::new(
            GqlErrorKind::Syntax,
            format!("{} (bytes {})", self.message, self.span),
        )
    }
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (bytes {})", self.message, self.span)
    }
}

impl std::error::Error for LexError {}

/// Tokenize `source` into a vector of spanned tokens terminated by [`Token::Eof`].
///
/// Comments and whitespace are skipped. Lexical failures (unterminated strings
/// or block comments, invalid characters, malformed numbers) are returned as a
/// span-bearing [`LexError`].
pub fn tokenize(source: &str) -> Result<Vec<SpannedToken>, LexError> {
    Lexer::new(source).run()
}

/// Tokenize and split into per-statement token slices on top-level `;`.
///
/// Each returned statement includes its own trailing [`Token::Eof`]. Empty
/// statements (e.g. a trailing `;`) are dropped. This is the lexical half of the
/// existing statement-splitting behavior, now span-aware.
pub fn split_statements(source: &str) -> Result<Vec<Vec<SpannedToken>>, LexError> {
    let tokens = tokenize(source)?;
    let mut statements = Vec::new();
    let mut current: Vec<SpannedToken> = Vec::new();
    let eof_span = tokens
        .last()
        .map(|t| t.span)
        .unwrap_or_else(|| Span::new(source.len(), source.len()));

    for spanned in tokens {
        match spanned.token {
            Token::Semicolon => {
                if !current.is_empty() {
                    current.push(SpannedToken {
                        token: Token::Eof,
                        span: spanned.span,
                    });
                    statements.push(std::mem::take(&mut current));
                }
            }
            Token::Eof => {
                if !current.is_empty() {
                    current.push(SpannedToken {
                        token: Token::Eof,
                        span: spanned.span,
                    });
                    statements.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(spanned),
        }
    }
    // Defensive: if the source had no Eof token (cannot happen via tokenize),
    // flush any remaining tokens.
    if !current.is_empty() {
        current.push(SpannedToken {
            token: Token::Eof,
            span: eof_span,
        });
        statements.push(current);
    }
    Ok(statements)
}

struct Lexer<'a> {
    source: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Lexer {
            source,
            bytes: source.as_bytes(),
            pos: 0,
        }
    }

    fn run(mut self) -> Result<Vec<SpannedToken>, LexError> {
        let mut out = Vec::new();
        loop {
            self.skip_trivia()?;
            if self.pos >= self.bytes.len() {
                out.push(SpannedToken {
                    token: Token::Eof,
                    span: Span::new(self.pos, self.pos),
                });
                return Ok(out);
            }
            let token = self.next_token()?;
            out.push(token);
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<u8> {
        self.bytes.get(self.pos + ahead).copied()
    }

    /// Skip whitespace and comments; error on an unterminated block comment.
    fn skip_trivia(&mut self) -> Result<(), LexError> {
        loop {
            match self.peek() {
                Some(b) if b.is_ascii_whitespace() => {
                    self.pos += 1;
                }
                Some(b'/') if self.peek_at(1) == Some(b'/') => {
                    self.pos += 2;
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                Some(b'/') if self.peek_at(1) == Some(b'*') => {
                    let start = self.pos;
                    self.pos += 2;
                    loop {
                        match self.peek() {
                            Some(b'*') if self.peek_at(1) == Some(b'/') => {
                                self.pos += 2;
                                break;
                            }
                            Some(_) => self.pos += 1,
                            None => {
                                return Err(LexError::new(
                                    Span::new(start, self.pos),
                                    "unterminated block comment",
                                ));
                            }
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn next_token(&mut self) -> Result<SpannedToken, LexError> {
        let start = self.pos;
        let b = self.peek().expect("next_token called at EOF");

        // identifiers / keywords
        if is_ident_start(b) {
            return Ok(self.lex_word(start));
        }
        // numbers
        if b.is_ascii_digit() {
            return self.lex_number(start);
        }
        // a leading `.` may begin `.5` or be `.`/`..`
        if b == b'.' {
            if self.peek_at(1) == Some(b'.') {
                self.pos += 2;
                return Ok(self.spanned(Token::DotDot, start));
            }
            if self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
                return self.lex_number(start);
            }
            self.pos += 1;
            return Ok(self.spanned(Token::Dot, start));
        }

        match b {
            b'`' => self.lex_quoted_identifier(start),
            b'\'' | b'"' => self.lex_string(start, b),
            b'$' => self.lex_parameter(start),
            _ => self.lex_operator(start, b),
        }
    }

    fn spanned(&self, token: Token, start: usize) -> SpannedToken {
        SpannedToken {
            token,
            span: Span::new(start, self.pos),
        }
    }

    fn lex_word(&mut self, start: usize) -> SpannedToken {
        self.pos += 1;
        while self.peek().is_some_and(is_ident_continue) {
            self.pos += 1;
        }
        let text = &self.source[start..self.pos];
        let upper = text.to_ascii_uppercase();
        let token = match Keyword::from_upper(&upper) {
            Some(kw) => Token::Keyword(kw),
            None => Token::Identifier(text.to_string()),
        };
        self.spanned(token, start)
    }

    fn lex_number(&mut self, start: usize) -> Result<SpannedToken, LexError> {
        let mut is_float = false;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        // fraction (but not the `..` range operator)
        if self.peek() == Some(b'.') && self.peek_at(1) != Some(b'.') {
            // `.` followed by a digit or end-of-number both count as a float here
            is_float = true;
            self.pos += 1;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        // exponent
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !self.peek().is_some_and(|c| c.is_ascii_digit()) {
                return Err(LexError::new(
                    Span::new(start, self.pos),
                    "malformed numeric literal: exponent has no digits",
                ));
            }
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        }

        let text = &self.source[start..self.pos];
        if is_float {
            let value: f64 = text.parse().map_err(|_| {
                LexError::new(
                    Span::new(start, self.pos),
                    format!("invalid float literal: {text}"),
                )
            })?;
            Ok(self.spanned(Token::Float(value), start))
        } else {
            let value: i64 = text.parse().map_err(|_| {
                LexError::new(
                    Span::new(start, self.pos),
                    format!("integer literal out of range: {text}"),
                )
            })?;
            Ok(self.spanned(Token::Integer(value), start))
        }
    }

    fn lex_quoted_identifier(&mut self, start: usize) -> Result<SpannedToken, LexError> {
        self.pos += 1; // opening backtick
        let mut value = String::new();
        loop {
            match self.peek() {
                Some(b'`') => {
                    // doubled backtick is an escaped backtick
                    if self.peek_at(1) == Some(b'`') {
                        value.push('`');
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                        return Ok(self.spanned(Token::QuotedIdentifier(value), start));
                    }
                }
                Some(_) => {
                    let ch = self.current_char();
                    value.push(ch);
                    self.pos += ch.len_utf8();
                }
                None => {
                    return Err(LexError::new(
                        Span::new(start, self.pos),
                        "unterminated quoted identifier",
                    ));
                }
            }
        }
    }

    fn lex_string(&mut self, start: usize, quote: u8) -> Result<SpannedToken, LexError> {
        self.pos += 1; // opening quote
        let mut value = String::new();
        loop {
            match self.peek() {
                Some(b'\\') => {
                    self.pos += 1;
                    let esc = self.peek().ok_or_else(|| {
                        LexError::new(
                            Span::new(start, self.pos),
                            "unterminated escape in string literal",
                        )
                    })?;
                    match esc {
                        b'n' => value.push('\n'),
                        b't' => value.push('\t'),
                        b'r' => value.push('\r'),
                        b'\\' => value.push('\\'),
                        b'\'' => value.push('\''),
                        b'"' => value.push('"'),
                        b'`' => value.push('`'),
                        b'0' => value.push('\0'),
                        b'u' => {
                            self.pos += 1;
                            let ch = self.lex_unicode_escape(start)?;
                            value.push(ch);
                            continue;
                        }
                        other => {
                            return Err(LexError::new(
                                Span::new(self.pos, self.pos + 1),
                                format!("invalid escape sequence: \\{}", other as char),
                            ));
                        }
                    }
                    self.pos += 1;
                }
                Some(b) if b == quote => {
                    self.pos += 1;
                    return Ok(self.spanned(Token::String(value), start));
                }
                Some(_) => {
                    let ch = self.current_char();
                    value.push(ch);
                    self.pos += ch.len_utf8();
                }
                None => {
                    return Err(LexError::new(
                        Span::new(start, self.pos),
                        "unterminated string literal",
                    ));
                }
            }
        }
    }

    /// Lex a `\u{XXXX}` or `\uXXXX` unicode escape; `self.pos` is just past `u`.
    fn lex_unicode_escape(&mut self, start: usize) -> Result<char, LexError> {
        let braced = self.peek() == Some(b'{');
        if braced {
            self.pos += 1;
        }
        let hex_start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
            self.pos += 1;
        }
        let hex = &self.source[hex_start..self.pos];
        if braced {
            if self.peek() != Some(b'}') {
                return Err(LexError::new(
                    Span::new(start, self.pos),
                    "unterminated \\u{...} escape",
                ));
            }
            self.pos += 1;
        } else if hex.len() != 4 {
            return Err(LexError::new(
                Span::new(hex_start, self.pos),
                "\\u escape requires exactly 4 hex digits (or use \\u{...})",
            ));
        }
        let code = u32::from_str_radix(hex, 16)
            .map_err(|_| LexError::new(Span::new(hex_start, self.pos), "invalid unicode escape"))?;
        char::from_u32(code).ok_or_else(|| {
            LexError::new(
                Span::new(hex_start, self.pos),
                format!("invalid unicode scalar value: {code:#x}"),
            )
        })
    }

    fn lex_parameter(&mut self, start: usize) -> Result<SpannedToken, LexError> {
        self.pos += 1; // `$`
        let name_start = self.pos;
        // `$0` positional or `$name`
        if self.peek().is_some_and(|c| c.is_ascii_digit()) {
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        } else if self.peek().is_some_and(is_ident_start) {
            self.pos += 1;
            while self.peek().is_some_and(is_ident_continue) {
                self.pos += 1;
            }
        } else {
            return Err(LexError::new(
                Span::new(start, self.pos),
                "expected a parameter name after '$'",
            ));
        }
        let name = self.source[name_start..self.pos].to_string();
        Ok(self.spanned(Token::Parameter(name), start))
    }

    fn lex_operator(&mut self, start: usize, b: u8) -> Result<SpannedToken, LexError> {
        let two = |s: &Self| (b, s.peek_at(1));
        let token = match two(self) {
            (b'-', Some(b'>')) => {
                self.pos += 2;
                Token::Arrow
            }
            (b'<', Some(b'-')) => {
                self.pos += 2;
                Token::ArrowLeft
            }
            (b'<', Some(b'=')) => {
                self.pos += 2;
                Token::Le
            }
            (b'>', Some(b'=')) => {
                self.pos += 2;
                Token::Ge
            }
            (b'<', Some(b'>')) => {
                self.pos += 2;
                Token::Ne
            }
            (b'!', Some(b'=')) => {
                self.pos += 2;
                Token::Ne
            }
            (b'+', Some(b'=')) => {
                self.pos += 2;
                Token::PlusEq
            }
            _ => {
                self.pos += 1;
                match b {
                    b'(' => Token::LParen,
                    b')' => Token::RParen,
                    b'{' => Token::LBrace,
                    b'}' => Token::RBrace,
                    b'[' => Token::LBracket,
                    b']' => Token::RBracket,
                    b':' => Token::Colon,
                    b',' => Token::Comma,
                    b';' => Token::Semicolon,
                    b'|' => Token::Pipe,
                    b'=' => Token::Eq,
                    b'<' => Token::Lt,
                    b'>' => Token::Gt,
                    b'+' => Token::Plus,
                    b'-' => Token::Minus,
                    b'*' => Token::Star,
                    b'/' => Token::Slash,
                    b'%' => Token::Percent,
                    b'^' => Token::Caret,
                    _ => {
                        return Err(LexError::new(
                            Span::new(start, self.pos),
                            format!("unexpected character: {:?}", b as char),
                        ));
                    }
                }
            }
        };
        Ok(self.spanned(token, start))
    }

    /// The full Unicode char at the current byte position.
    fn current_char(&self) -> char {
        self.source[self.pos..]
            .chars()
            .next()
            .expect("current_char called at EOF")
    }
}

fn is_ident_start(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphabetic()
}

fn is_ident_continue(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(source: &str) -> Vec<Token> {
        tokenize(source)
            .expect("should tokenize")
            .into_iter()
            .map(|s| s.token)
            .collect()
    }

    #[test]
    fn empty_input_is_just_eof() {
        assert_eq!(toks(""), vec![Token::Eof]);
        assert_eq!(toks("   \n\t  "), vec![Token::Eof]);
    }

    #[test]
    fn keywords_are_case_insensitive() {
        assert_eq!(
            toks("MATCH match MaTcH"),
            vec![
                Token::Keyword(Keyword::Match),
                Token::Keyword(Keyword::Match),
                Token::Keyword(Keyword::Match),
                Token::Eof
            ]
        );
    }

    #[test]
    fn identifiers_vs_keywords() {
        assert_eq!(
            toks("matcher n123 _x"),
            vec![
                Token::Identifier("matcher".into()),
                Token::Identifier("n123".into()),
                Token::Identifier("_x".into()),
                Token::Eof
            ]
        );
    }

    #[test]
    fn line_and_block_comments_are_skipped() {
        assert_eq!(
            toks("CREATE // line comment\n  /* block\n comment */ MERGE"),
            vec![
                Token::Keyword(Keyword::Create),
                Token::Keyword(Keyword::Merge),
                Token::Eof
            ]
        );
    }

    #[test]
    fn backtick_identifiers_with_escapes() {
        assert_eq!(
            toks("`weird name` `a``b`"),
            vec![
                Token::QuotedIdentifier("weird name".into()),
                Token::QuotedIdentifier("a`b".into()),
                Token::Eof
            ]
        );
    }

    #[test]
    fn parameters() {
        assert_eq!(
            toks("$id $0 $userName"),
            vec![
                Token::Parameter("id".into()),
                Token::Parameter("0".into()),
                Token::Parameter("userName".into()),
                Token::Eof
            ]
        );
    }

    #[test]
    fn string_literals_with_escapes() {
        assert_eq!(
            toks(r#" 'hello' "world" 'line\nbreak' 'quote\'s' "#),
            vec![
                Token::String("hello".into()),
                Token::String("world".into()),
                Token::String("line\nbreak".into()),
                Token::String("quote's".into()),
                Token::Eof
            ]
        );
    }

    #[test]
    fn unicode_escape_forms() {
        assert_eq!(
            toks(r#" 'A' '\u{1F600}' "#),
            vec![
                Token::String("A".into()),
                Token::String("\u{1F600}".into()),
                Token::Eof
            ]
        );
    }

    #[test]
    fn numeric_families() {
        assert_eq!(
            toks("42 3.14 .5 1e10 2.5E-3 100"),
            vec![
                Token::Integer(42),
                Token::Float(3.14),
                Token::Float(0.5),
                Token::Float(1e10),
                Token::Float(2.5e-3),
                Token::Integer(100),
                Token::Eof
            ]
        );
    }

    #[test]
    fn range_dotdot_is_not_a_float() {
        assert_eq!(
            toks("1..3"),
            vec![
                Token::Integer(1),
                Token::DotDot,
                Token::Integer(3),
                Token::Eof
            ]
        );
    }

    #[test]
    fn property_access_dot_then_ident() {
        assert_eq!(
            toks("n.name"),
            vec![
                Token::Identifier("n".into()),
                Token::Dot,
                Token::Identifier("name".into()),
                Token::Eof
            ]
        );
    }

    #[test]
    fn relationship_arrows_and_operators() {
        assert_eq!(
            toks("-> <- <= >= <> != += < > = + - * / % ^ | :"),
            vec![
                Token::Arrow,
                Token::ArrowLeft,
                Token::Le,
                Token::Ge,
                Token::Ne,
                Token::Ne,
                Token::PlusEq,
                Token::Lt,
                Token::Gt,
                Token::Eq,
                Token::Plus,
                Token::Minus,
                Token::Star,
                Token::Slash,
                Token::Percent,
                Token::Caret,
                Token::Pipe,
                Token::Colon,
                Token::Eof
            ]
        );
    }

    #[test]
    fn representative_pattern_tokenizes() {
        let source = "MATCH (a:Person {id: 'p1'})-[e:KNOWS]->(b:Person) RETURN a, e, b";
        let tokens = toks(source);
        assert_eq!(tokens.first(), Some(&Token::Keyword(Keyword::Match)));
        assert!(tokens.contains(&Token::Arrow));
        assert!(tokens.contains(&Token::Keyword(Keyword::Return)));
        assert_eq!(tokens.last(), Some(&Token::Eof));
    }

    #[test]
    fn spans_point_at_the_token_text() {
        let source = "MATCH  (n)";
        let spanned = tokenize(source).unwrap();
        assert_eq!(spanned[0].span.text(source), "MATCH");
        assert_eq!(spanned[1].span.text(source), "(");
        assert_eq!(spanned[2].span.text(source), "n");
        assert_eq!(spanned[3].span.text(source), ")");
    }

    #[test]
    fn unterminated_string_reports_span() {
        let err = tokenize("RETURN 'oops").unwrap_err();
        assert!(err.message.contains("unterminated string"));
        // span starts at the opening quote
        assert_eq!(err.span.start, 7);
        let rendered = err.render("RETURN 'oops");
        assert!(rendered.contains("line 1"));
    }

    #[test]
    fn unterminated_block_comment_reports_span() {
        let err = tokenize("CREATE /* never ends").unwrap_err();
        assert!(err.message.contains("unterminated block comment"));
        assert_eq!(err.span.start, 7);
    }

    #[test]
    fn invalid_character_reports_span() {
        let err = tokenize("RETURN #").unwrap_err();
        assert!(err.message.contains("unexpected character"));
        assert_eq!(err.span.text("RETURN #"), "#");
    }

    #[test]
    fn malformed_exponent_is_an_error() {
        let err = tokenize("RETURN 1e").unwrap_err();
        assert!(err.message.contains("exponent has no digits"));
    }

    #[test]
    fn line_col_resolves_multiline() {
        let source = "MATCH\n  (n)\nRETURN n";
        let lc = line_col(source, source.find("RETURN").unwrap());
        assert_eq!(lc, LineCol { line: 3, column: 1 });
    }

    #[test]
    fn statement_splitting_on_semicolons() {
        let stmts = split_statements("CREATE (:A {id:'1'}); MERGE (:B {id:'2'})").unwrap();
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0][0].token.is_keyword(Keyword::Create));
        assert!(stmts[1][0].token.is_keyword(Keyword::Merge));
        // each statement is Eof-terminated
        assert_eq!(stmts[0].last().map(|t| &t.token), Some(&Token::Eof));
        assert_eq!(stmts[1].last().map(|t| &t.token), Some(&Token::Eof));
    }

    #[test]
    fn trailing_and_empty_statements_are_dropped() {
        let stmts = split_statements("CREATE (:A {id:'1'}); ; ").unwrap();
        assert_eq!(stmts.len(), 1);
        let none = split_statements("   ;  ; ").unwrap();
        assert_eq!(none.len(), 0);
    }

    #[test]
    fn lex_error_converts_to_structured_syntax_error() {
        let err = tokenize("RETURN 'oops").unwrap_err();
        let grust = err.into_grust("RETURN 'oops");
        assert!(matches!(grust, GrustError::CypherSyntax(_)));
        assert!(grust.to_string().contains("gql:syntax"));
    }
}
