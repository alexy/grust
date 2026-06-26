//! Transaction & session control command surface (Unit 13).
//!
//! This is the **language + capability-reporting** slice of transaction control:
//! it recognizes standalone GQL/Cypher transaction commands (`START TRANSACTION`,
//! `BEGIN`, `COMMIT`, `ROLLBACK`) in the new pipeline and reports per-backend
//! atomicity capability via [`crate::gql::GqlBackend::transactional`].
//!
//! Execution — actually wrapping a statement batch so it commits or rolls back
//! atomically — is delegated to the backend's mutation store
//! (`GraphMutationAtomicity`) and is wired through the write path; that wiring is
//! sequenced after the write-path cutover (Unit 10) per `docs/GQL_GOAL.md`.
//!
//! The transaction keywords are deliberately *not* reserved in the lexer (they
//! tokenize as identifiers), so existing queries may still use `start`, `begin`,
//! `read`, `commit`, … as variable or property names. Recognition happens here at
//! the statement level instead.

use grust_core::Result;

use crate::gql::{gql_syntax, GqlBackend};
use crate::lexer::{tokenize, Token};

/// Access mode characteristic of `START TRANSACTION`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransactionAccessMode {
    /// `READ ONLY`.
    ReadOnly,
    /// `READ WRITE`.
    ReadWrite,
}

/// A standalone transaction-control command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransactionCommand {
    /// `START TRANSACTION [READ ONLY | READ WRITE]` or `BEGIN`.
    Start(Option<TransactionAccessMode>),
    /// `COMMIT`.
    Commit,
    /// `ROLLBACK`.
    Rollback,
}

impl TransactionCommand {
    /// Recognize a standalone transaction command.
    ///
    /// Returns `Ok(None)` when `source` is not a transaction command (so the
    /// caller falls back to query parsing), and `Err` when it begins like one but
    /// is malformed (e.g. `START TRANSACTION FOO`).
    pub fn parse(source: &str) -> Result<Option<TransactionCommand>> {
        let spanned = match tokenize(source) {
            Ok(tokens) => tokens,
            // Defer lexical errors to the normal query parser's diagnostics.
            Err(_) => return Ok(None),
        };
        // Meaningful tokens, minus a trailing `;` and the EOF sentinel.
        let words: Vec<String> = spanned
            .iter()
            .map(|t| &t.token)
            .filter(|t| !matches!(t, Token::Eof | Token::Semicolon))
            .map(ident_lower)
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default();
        let Some(first) = words.first().map(String::as_str) else {
            return Ok(None);
        };
        match first {
            "begin" if words.len() == 1 => Ok(Some(TransactionCommand::Start(None))),
            "commit" if words.len() == 1 => Ok(Some(TransactionCommand::Commit)),
            "rollback" if words.len() == 1 => Ok(Some(TransactionCommand::Rollback)),
            "start" => {
                if words.get(1).map(String::as_str) != Some("transaction") {
                    // `start` as a bare identifier is not a transaction command.
                    return Ok(None);
                }
                let mode = match words[2..].iter().map(String::as_str).collect::<Vec<_>>()[..] {
                    [] => None,
                    ["read", "only"] => Some(TransactionAccessMode::ReadOnly),
                    ["read", "write"] => Some(TransactionAccessMode::ReadWrite),
                    _ => {
                        return Err(gql_syntax(
                            "START TRANSACTION accepts only an optional READ ONLY / READ WRITE characteristic",
                        ))
                    }
                };
                Ok(Some(TransactionCommand::Start(mode)))
            }
            _ => Ok(None),
        }
    }
}

/// All catalogued backends whose mutation store reports `Transactional`
/// atomicity — i.e. those that can honor `COMMIT`/`ROLLBACK` over a batch.
pub fn transactional_backends() -> Vec<GqlBackend> {
    GqlBackend::ALL
        .iter()
        .copied()
        .filter(|b| b.transactional())
        .collect()
}

/// Tokens that are (possibly quoted) identifiers lower-cased; `None` otherwise.
fn ident_lower(token: &Token) -> Option<String> {
    match token {
        Token::Identifier(s) | Token::QuotedIdentifier(s) => Some(s.to_ascii_lowercase()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Option<TransactionCommand> {
        TransactionCommand::parse(s).unwrap()
    }

    #[test]
    fn recognizes_transaction_commands() {
        assert_eq!(parse("BEGIN"), Some(TransactionCommand::Start(None)));
        assert_eq!(parse("begin;"), Some(TransactionCommand::Start(None)));
        assert_eq!(parse("COMMIT"), Some(TransactionCommand::Commit));
        assert_eq!(parse("ROLLBACK"), Some(TransactionCommand::Rollback));
        assert_eq!(
            parse("START TRANSACTION"),
            Some(TransactionCommand::Start(None))
        );
        assert_eq!(
            parse("start transaction read only"),
            Some(TransactionCommand::Start(Some(TransactionAccessMode::ReadOnly)))
        );
        assert_eq!(
            parse("START TRANSACTION READ WRITE"),
            Some(TransactionCommand::Start(Some(TransactionAccessMode::ReadWrite)))
        );
    }

    #[test]
    fn non_transaction_sources_are_none() {
        assert_eq!(parse("MATCH (n) RETURN n"), None);
        assert_eq!(parse("CREATE (n:Person {id: 'p1'})"), None);
        // `start` / `commit` as identifiers in a real query are not commands.
        assert_eq!(parse("RETURN start"), None);
        // `start` alone (no TRANSACTION) is not a transaction command.
        assert_eq!(parse("start"), None);
    }

    #[test]
    fn malformed_transaction_command_errors() {
        assert!(TransactionCommand::parse("START TRANSACTION FOO").is_err());
        assert!(TransactionCommand::parse("START TRANSACTION READ SOMETIMES").is_err());
    }

    #[test]
    fn transactional_capability_is_reported() {
        let tx = transactional_backends();
        assert!(tx.contains(&GqlBackend::Turso));
        assert!(tx.contains(&GqlBackend::Postgres));
        assert!(tx.contains(&GqlBackend::PostgresPgq));
        // Memory is OrderedNonAtomic; Sail does not override atomicity.
        assert!(!GqlBackend::Memory.transactional());
        assert!(!GqlBackend::Sail.transactional());
    }
}
