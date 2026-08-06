//! Turso's durable guarded-commit implementation.

use async_trait::async_trait;
use grust_core::prelude::{
    GraphCommitReceipt, GraphCommitStore, GraphExpectation, GrustError, GuardedGraphCommit, Node,
    NodeId, Result,
};

use crate::{TursoConfig, TursoGraphStore, TursoJournalMode, quote_ident};

const MAX_TRANSACTION_ATTEMPTS: usize = 8;

pub(super) fn ledger_bootstrap_sql(config: &TursoConfig) -> String {
    let table = quote_ident(&format!("{}_commits", config.table_prefix));
    format!(
        "CREATE TABLE IF NOT EXISTS {table} (
            idempotency_key TEXT PRIMARY KEY,
            request_digest TEXT NOT NULL,
            commit_id TEXT NOT NULL UNIQUE,
            committed_at TEXT NOT NULL
         );"
    )
}

#[async_trait]
impl GraphCommitStore for TursoGraphStore {
    async fn commit_guarded(&self, commit: &GuardedGraphCommit) -> Result<GraphCommitReceipt> {
        validate_commit(commit)?;
        let statements = commit
            .mutations
            .iter()
            .map(|mutation| super::mutation_sql(&self.nodes_table(), &self.edges_table(), mutation))
            .collect::<Result<Vec<_>>>()?;

        let _gate = self.connection_gate.lock().await;
        for attempt in 1..=MAX_TRANSACTION_ATTEMPTS {
            match self.commit_guarded_once(commit, &statements).await {
                Ok(receipt) => return Ok(receipt),
                Err(error)
                    if attempt < MAX_TRANSACTION_ATTEMPTS
                        && is_retryable_guarded_conflict(&error) =>
                {
                    tokio::task::yield_now().await;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("the bounded guarded-commit loop always returns")
    }

    async fn recover_guarded_commit(
        &self,
        idempotency_key: &str,
        request_digest: &str,
    ) -> Result<Option<GraphCommitReceipt>> {
        validate_commit_identity(idempotency_key, request_digest)?;
        let _gate = self.connection_gate.lock().await;
        let Some((stored_digest, mut receipt)) = self.load_commit_receipt(idempotency_key).await?
        else {
            return Ok(None);
        };
        if stored_digest != request_digest {
            return Err(GrustError::GraphIdempotencyConflict(
                idempotency_key.to_owned(),
            ));
        }
        receipt.replayed = true;
        Ok(Some(receipt))
    }
}

impl TursoGraphStore {
    async fn commit_guarded_once(
        &self,
        commit: &GuardedGraphCommit,
        statements: &[String],
    ) -> Result<GraphCommitReceipt> {
        let begin = match self.config.journal_mode {
            TursoJournalMode::Wal => "BEGIN IMMEDIATE",
            TursoJournalMode::Mvcc => "BEGIN CONCURRENT",
        };
        self.conn.execute(begin, ()).await.map_err(|error| {
            GrustError::Backend(format!("Turso guarded transaction begin failed: {error}"))
        })?;

        let result = self.commit_guarded_in_transaction(commit, statements).await;
        match result {
            Ok(receipt) => match self.conn.execute("COMMIT", ()).await {
                Ok(_) => Ok(receipt),
                Err(error) => {
                    let _ = self.conn.execute("ROLLBACK", ()).await;
                    Err(GrustError::Backend(format!(
                        "Turso guarded transaction commit failed: {error}"
                    )))
                }
            },
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    async fn commit_guarded_in_transaction(
        &self,
        commit: &GuardedGraphCommit,
        statements: &[String],
    ) -> Result<GraphCommitReceipt> {
        if let Some((stored_digest, mut receipt)) =
            self.load_commit_receipt(&commit.idempotency_key).await?
        {
            if stored_digest != commit.request_digest {
                return Err(GrustError::GraphIdempotencyConflict(
                    commit.idempotency_key.clone(),
                ));
            }
            receipt.replayed = true;
            return Ok(receipt);
        }

        for expectation in &commit.expectations {
            self.check_expectation(expectation).await?;
        }
        for statement in statements {
            self.conn.execute_batch(statement).await.map_err(|error| {
                GrustError::Backend(format!("Turso guarded graph mutation failed: {error}"))
            })?;
        }
        self.insert_commit_receipt(&commit.idempotency_key, &commit.request_digest)
            .await
    }

    async fn load_commit_receipt(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<(String, GraphCommitReceipt)>> {
        let sql = format!(
            "SELECT request_digest, commit_id, committed_at FROM {} \
             WHERE idempotency_key = ?1 LIMIT 1",
            self.commits_table()
        );
        let mut rows = self
            .conn
            .query(&sql, (idempotency_key,))
            .await
            .map_err(|error| {
                GrustError::Backend(format!("Turso commit ledger lookup failed: {error}"))
            })?;
        let Some(row) = rows.next().await.map_err(|error| {
            GrustError::Backend(format!("Turso commit ledger row read failed: {error}"))
        })?
        else {
            return Ok(None);
        };
        Ok(Some((
            super::row_text(&row, 0, "commit request digest")?,
            GraphCommitReceipt {
                commit_id: super::row_text(&row, 1, "commit id")?,
                committed_at: super::row_text(&row, 2, "commit timestamp")?,
                replayed: false,
            },
        )))
    }

    async fn check_expectation(&self, expectation: &GraphExpectation) -> Result<()> {
        let actual = self.load_node_for_guard(expectation.node_id()).await?;
        match (expectation, actual) {
            (GraphExpectation::Absent(_), None) => Ok(()),
            (GraphExpectation::Absent(id), Some(_)) => Err(GrustError::GraphExpectationFailed(
                format!("node {id} was present"),
            )),
            (GraphExpectation::Exact(expected), Some(actual)) if expected == &actual => Ok(()),
            (GraphExpectation::Exact(expected), None) => Err(GrustError::GraphExpectationFailed(
                format!("node {} was absent", expected.id),
            )),
            (GraphExpectation::Exact(expected), Some(_)) => Err(
                GrustError::GraphExpectationFailed(format!("node {} changed", expected.id)),
            ),
        }
    }

    async fn load_node_for_guard(&self, id: &NodeId) -> Result<Option<Node>> {
        let sql = format!(
            "SELECT id, label, props FROM {} WHERE id = ?1 LIMIT 1",
            self.nodes_table()
        );
        let mut rows = self
            .conn
            .query(&sql, (id.as_str(),))
            .await
            .map_err(|error| {
                GrustError::Backend(format!("Turso guarded node lookup failed: {error}"))
            })?;
        rows.next()
            .await
            .map_err(|error| {
                GrustError::Backend(format!("Turso guarded node row read failed: {error}"))
            })?
            .map(|row| super::row_to_node(&row))
            .transpose()
    }

    async fn insert_commit_receipt(
        &self,
        idempotency_key: &str,
        request_digest: &str,
    ) -> Result<GraphCommitReceipt> {
        let sql = format!(
            "INSERT INTO {} (idempotency_key, request_digest, commit_id, committed_at) \
             VALUES (?1, ?2, 'turso:' || lower(hex(randomblob(16))), \
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now')) \
             RETURNING commit_id, committed_at",
            self.commits_table()
        );
        let mut rows = self
            .conn
            .query(&sql, (idempotency_key, request_digest))
            .await
            .map_err(|error| {
                GrustError::Backend(format!("Turso commit ledger insert failed: {error}"))
            })?;
        let row = rows
            .next()
            .await
            .map_err(|error| {
                GrustError::Backend(format!("Turso commit receipt row read failed: {error}"))
            })?
            .ok_or_else(|| {
                GrustError::Backend("Turso commit ledger returned no receipt".to_string())
            })?;
        Ok(GraphCommitReceipt {
            commit_id: super::row_text(&row, 0, "commit id")?,
            committed_at: super::row_text(&row, 1, "commit timestamp")?,
            replayed: false,
        })
    }
}

fn validate_commit(commit: &GuardedGraphCommit) -> Result<()> {
    validate_commit_identity(&commit.idempotency_key, &commit.request_digest)
}

fn validate_commit_identity(idempotency_key: &str, request_digest: &str) -> Result<()> {
    if idempotency_key.trim().is_empty() {
        return Err(GrustError::Schema(
            "guarded commit idempotency key must not be empty".to_string(),
        ));
    }
    if request_digest.trim().is_empty() {
        return Err(GrustError::Schema(
            "guarded commit request digest must not be empty".to_string(),
        ));
    }
    Ok(())
}

fn is_retryable_guarded_conflict(error: &GrustError) -> bool {
    if super::is_mvcc_conflict(error) {
        return true;
    }
    let message = error.to_string().to_ascii_lowercase();
    message.contains("commit ledger insert failed")
        && (message.contains("unique") || message.contains("constraint"))
}
