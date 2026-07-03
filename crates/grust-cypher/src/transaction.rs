//! Transaction & session control surface (Unit 13 + atomic batch execution).
//!
//! This module owns both slices of transaction control:
//!
//! - **Language + capability reporting** (Unit 13): recognizing standalone
//!   GQL/Cypher transaction commands (`START TRANSACTION`, `BEGIN`, `COMMIT`,
//!   `ROLLBACK`) and reporting per-backend atomicity capability via
//!   [`crate::gql::GqlBackend::transactional`].
//! - **Atomic batch execution** (post write-path-cutover): a
//!   [`CypherTransaction`] accumulates planned write statements between
//!   `START TRANSACTION`/`BEGIN` and `COMMIT` into one
//!   [`GraphMutationPlan`]; [`execute_cypher_transaction_on_store`] submits
//!   the whole batch in a **single** `apply_mutations` call, which backends
//!   whose mutation store reports [`GraphMutationAtomicity::Transactional`]
//!   apply in one backend transaction (e.g. Turso wraps the slice in
//!   `BEGIN…COMMIT` SQL). Non-transactional stores are refused with a
//!   structured, feature-tagged error rather than silently executing the
//!   batch non-atomically. `ROLLBACK` discards the accumulated plan without
//!   touching the store, so it works on every backend.
//!
//! The transaction keywords are deliberately *not* reserved in the lexer (they
//! tokenize as identifiers), so existing queries may still use `start`, `begin`,
//! `read`, `commit`, … as variable or property names. Recognition happens here at
//! the statement level instead.

use grust_core::{GraphMutationAtomicity, GraphMutationPlan, GraphMutationStore, Result};

use crate::gql::{
    GqlBackend, GqlConformanceProfile, GqlFeature, gql_execution, gql_syntax,
    unsupported_gql_feature,
};
use crate::lexer::{Token, tokenize};
use crate::{
    CypherGeneratedNodeId, CypherMutationOptions, CypherMutationReport,
    sail_cypher_mutation_plan_with_options,
};

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
                        ));
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

/// An open atomic statement batch: the execution side of
/// `START TRANSACTION`/`BEGIN` … `COMMIT`.
///
/// Statements are planned **eagerly** on [`add_statement`](Self::add_statement)
/// (a malformed statement fails the transaction before anything reaches a
/// store) and their operations accumulate into one [`GraphMutationPlan`].
/// Dropping the value (or [`rollback`](Self::rollback)) discards the batch
/// without any store interaction.
#[derive(Debug)]
pub struct CypherTransaction {
    access_mode: Option<TransactionAccessMode>,
    options: CypherMutationOptions,
    plan: GraphMutationPlan,
    generated_node_ids: Vec<CypherGeneratedNodeId>,
    statement_count: usize,
}

impl CypherTransaction {
    /// Open a transaction (`BEGIN` / `START TRANSACTION [READ ONLY|READ WRITE]`).
    pub fn begin(
        access_mode: Option<TransactionAccessMode>,
        options: CypherMutationOptions,
    ) -> Self {
        CypherTransaction {
            access_mode,
            options,
            plan: GraphMutationPlan::new(Vec::new()),
            generated_node_ids: Vec::new(),
            statement_count: 0,
        }
    }

    pub fn access_mode(&self) -> Option<TransactionAccessMode> {
        self.access_mode
    }

    /// Plan a write statement into the batch. `READ ONLY` transactions refuse
    /// write statements (the writable planner is the only statement source
    /// here, so they refuse every statement).
    pub fn add_statement(&mut self, cypher: &str) -> Result<()> {
        if self.access_mode == Some(TransactionAccessMode::ReadOnly) {
            return Err(gql_execution(
                "cannot add a write statement to a READ ONLY transaction",
            ));
        }
        let (plan, generated) =
            sail_cypher_mutation_plan_with_options(cypher, self.options.clone())?;
        self.plan.operations.extend(plan.operations);
        self.generated_node_ids.extend(generated);
        self.statement_count += 1;
        Ok(())
    }

    pub fn statement_count(&self) -> usize {
        self.statement_count
    }

    /// The accumulated (not yet executed) plan.
    pub fn plan(&self) -> &GraphMutationPlan {
        &self.plan
    }

    /// Node ids generated while planning (under a generating id policy).
    pub fn generated_node_ids(&self) -> &[CypherGeneratedNodeId] {
        &self.generated_node_ids
    }

    /// Discard the batch (`ROLLBACK`). Nothing was sent to any store.
    pub fn rollback(self) {}
}

/// `COMMIT`: execute an accumulated transaction batch atomically on `store`.
///
/// The whole batch is submitted in a **single** `apply_mutations` call.
/// Stores reporting [`GraphMutationAtomicity::Transactional`] apply that call
/// in one backend transaction (the trait contract), so the batch commits or
/// rolls back as a unit. Stores reporting `OrderedNonAtomic` are refused with
/// a structured error — executing a `COMMIT` without atomicity would be a
/// silent lie; callers who don't need atomicity can execute statements
/// individually through the normal write entrypoints.
pub async fn execute_cypher_transaction_on_store<S>(
    store: &S,
    transaction: CypherTransaction,
) -> Result<CypherMutationReport>
where
    S: GraphMutationStore + ?Sized,
{
    if store.mutation_atomicity() != GraphMutationAtomicity::Transactional {
        return Err(unsupported_gql_feature(
            GqlFeature::TransactionControl,
            GqlConformanceProfile::PortableGql,
            "this backend's mutation store is OrderedNonAtomic: refusing to COMMIT a multi-statement batch without atomicity (execute statements individually instead)",
        ));
    }
    let report = transaction.plan.report();
    let mutations = transaction.plan.into_mutations();
    store.apply_mutations(&mutations).await?;
    Ok(report)
}

/// Execute a whole transaction script — `BEGIN; <statements…>; COMMIT` (or
/// `ROLLBACK`) — atomically on `store`.
///
/// Exactly one transaction per script: it must open with
/// `START TRANSACTION`/`BEGIN`, every statement must sit inside it, and it
/// must close with `COMMIT` (executes the batch atomically) or `ROLLBACK`
/// (discards it; the returned report is empty). Statements are split at
/// top-level `;` boundaries by the lexer, so semicolons inside string
/// literals are safe.
pub async fn run_cypher_transaction_script_on_store<S>(
    store: &S,
    script: &str,
    options: CypherMutationOptions,
) -> Result<CypherMutationReport>
where
    S: GraphMutationStore + ?Sized,
{
    let mut open: Option<CypherTransaction> = None;
    let mut finished: Option<CypherMutationReport> = None;
    for segment in split_statements(script)? {
        if finished.is_some() {
            return Err(gql_syntax(
                "transaction scripts support exactly one transaction; nothing may follow COMMIT/ROLLBACK",
            ));
        }
        match TransactionCommand::parse(segment)? {
            Some(TransactionCommand::Start(mode)) => {
                if open.is_some() {
                    return Err(gql_syntax(
                        "nested START TRANSACTION/BEGIN is not supported",
                    ));
                }
                open = Some(CypherTransaction::begin(mode, options.clone()));
            }
            Some(TransactionCommand::Commit) => {
                let transaction = open
                    .take()
                    .ok_or_else(|| gql_syntax("COMMIT without an open transaction"))?;
                finished = Some(execute_cypher_transaction_on_store(store, transaction).await?);
            }
            Some(TransactionCommand::Rollback) => {
                let transaction = open
                    .take()
                    .ok_or_else(|| gql_syntax("ROLLBACK without an open transaction"))?;
                transaction.rollback();
                finished = Some(CypherMutationReport::default());
            }
            None => {
                let transaction = open.as_mut().ok_or_else(|| {
                    gql_syntax(
                        "statements in a transaction script must appear between START TRANSACTION/BEGIN and COMMIT/ROLLBACK",
                    )
                })?;
                transaction.add_statement(segment)?;
            }
        }
    }
    if open.is_some() {
        return Err(gql_syntax(
            "transaction script ended without COMMIT or ROLLBACK",
        ));
    }
    finished.ok_or_else(|| gql_syntax("transaction script contains no transaction"))
}

/// Split a script into statement segments at top-level `;` boundaries using
/// the lexer (semicolons inside string literals do not split). Empty segments
/// (e.g. trailing `;`) are dropped.
fn split_statements(source: &str) -> Result<Vec<&str>> {
    let tokens = tokenize(source).map_err(|e| {
        gql_syntax(format!(
            "transaction script failed to tokenize: {}",
            e.message
        ))
    })?;
    let mut segments = Vec::new();
    let mut start = 0usize;
    for spanned in &tokens {
        match spanned.token {
            Token::Semicolon => {
                segments.push(&source[start..spanned.span.start]);
                start = spanned.span.end;
            }
            Token::Eof => {
                segments.push(&source[start..spanned.span.start]);
            }
            _ => {}
        }
    }
    Ok(segments
        .into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect())
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
            Some(TransactionCommand::Start(Some(
                TransactionAccessMode::ReadOnly
            )))
        );
        assert_eq!(
            parse("START TRANSACTION READ WRITE"),
            Some(TransactionCommand::Start(Some(
                TransactionAccessMode::ReadWrite
            )))
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
    fn transaction_batch_accumulates_planned_statements() {
        let mut txn = CypherTransaction::begin(None, CypherMutationOptions::default());
        txn.add_statement("CREATE (:Person {id: 'p1', name: 'Ada'})")
            .unwrap();
        txn.add_statement("CREATE (:Person {id: 'p2'})").unwrap();
        txn.add_statement("CREATE (:City {id: 'c1'})").unwrap();
        assert_eq!(txn.statement_count(), 3);
        assert_eq!(txn.plan().operations.len(), 3);
        assert_eq!(txn.plan().report().node_upserts, 3);
        // ROLLBACK is a pure drop: no store was involved.
        txn.rollback();
    }

    #[test]
    fn transaction_batch_fails_eagerly_on_malformed_statement() {
        let mut txn = CypherTransaction::begin(None, CypherMutationOptions::default());
        txn.add_statement("CREATE (:Person {id: 'p1'})").unwrap();
        // Planning errors surface at add time, before anything reaches a store.
        assert!(
            txn.add_statement("CREATE (:Person {name: 'no id'})")
                .is_err()
        );
    }

    #[test]
    fn read_only_transaction_refuses_writes() {
        let mut txn = CypherTransaction::begin(
            Some(TransactionAccessMode::ReadOnly),
            CypherMutationOptions::default(),
        );
        let err = txn
            .add_statement("CREATE (:Person {id: 'p1'})")
            .unwrap_err();
        assert!(err.to_string().contains("READ ONLY"));
    }

    #[test]
    fn commit_on_non_transactional_store_is_refused() {
        use futures_executor::block_on;
        use grust_memory::MemoryGraphStore;

        let store = MemoryGraphStore::new();
        let mut txn = CypherTransaction::begin(None, CypherMutationOptions::default());
        txn.add_statement("CREATE (:Person {id: 'p1'})").unwrap();
        let err = block_on(execute_cypher_transaction_on_store(&store, txn)).unwrap_err();
        assert!(matches!(err, grust_core::GrustError::Unsupported(_)));
        assert!(err.to_string().contains("feature=transaction-control"));
        // Nothing reached the store.
        assert!(store.graph().nodes.is_empty());
    }

    #[test]
    fn rollback_script_touches_no_store_on_any_backend() {
        use futures_executor::block_on;
        use grust_memory::MemoryGraphStore;

        let store = MemoryGraphStore::new();
        let report = block_on(run_cypher_transaction_script_on_store(
            &store,
            "BEGIN; CREATE (:Person {id: 'p1'}); ROLLBACK",
            CypherMutationOptions::default(),
        ))
        .unwrap();
        assert_eq!(report, CypherMutationReport::default());
        assert!(store.graph().nodes.is_empty());
    }

    #[test]
    fn transaction_script_shape_is_validated() {
        use futures_executor::block_on;
        use grust_memory::MemoryGraphStore;

        let store = MemoryGraphStore::new();
        let run = |script: &str| {
            block_on(run_cypher_transaction_script_on_store(
                &store,
                script,
                CypherMutationOptions::default(),
            ))
        };
        // COMMIT/ROLLBACK without an open transaction.
        assert!(run("COMMIT").is_err());
        assert!(run("ROLLBACK").is_err());
        // Statement outside the transaction.
        assert!(run("CREATE (:Person {id: 'p1'}); BEGIN; COMMIT").is_err());
        // Missing COMMIT/ROLLBACK.
        assert!(run("BEGIN; CREATE (:Person {id: 'p1'})").is_err());
        // Nested BEGIN.
        assert!(run("BEGIN; BEGIN; COMMIT").is_err());
        // Anything after the transaction closes.
        assert!(run("BEGIN; ROLLBACK; BEGIN; ROLLBACK").is_err());
        // Semicolons inside string literals do not split statements.
        let report = run("BEGIN; CREATE (:Person {id: 'p;1'}); ROLLBACK").unwrap();
        assert_eq!(report, CypherMutationReport::default());
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
