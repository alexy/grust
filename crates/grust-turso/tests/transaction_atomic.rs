//! Atomic Cypher transaction batches on the Turso store.
//!
//! Turso's `apply_mutations` wraps the whole mutation slice in one
//! `BEGIN…COMMIT` SQL transaction and its store reports
//! `GraphMutationAtomicity::Transactional`, so
//! `run_cypher_transaction_script_on_store` commits a multi-statement Cypher
//! batch as a unit here — the end-to-end proof behind the
//! `transaction-control` capability flag.

use grust_core::prelude::*;
use grust_cypher::{CypherMutationOptions, run_cypher_transaction_script_on_store};
use grust_turso::{TursoConfig, TursoGraphStore};

async fn store() -> TursoGraphStore {
    let store = TursoGraphStore::connect(TursoConfig::default())
        .await
        .expect("in-memory Turso store");
    store.bootstrap().await.expect("bootstrap tables");
    store
}

async fn has_node(store: &TursoGraphStore, id: &str) -> bool {
    store
        .get_node(&NodeId::from(id))
        .await
        .expect("get_node")
        .is_some()
}

#[tokio::test]
async fn commit_applies_the_batch_atomically() {
    let store = store().await;
    let report = run_cypher_transaction_script_on_store(
        &store,
        "START TRANSACTION; \
         CREATE (:Person {id: 'p1', name: 'Ada'}); \
         CREATE (:Person {id: 'p2', name: 'Alan'}); \
         COMMIT",
        CypherMutationOptions::default(),
    )
    .await
    .expect("transaction script commits");
    assert_eq!(report.node_upserts, 2);
    assert!(has_node(&store, "p1").await);
    assert!(has_node(&store, "p2").await);
}

#[tokio::test]
async fn rollback_leaves_the_store_untouched() {
    let store = store().await;
    let report = run_cypher_transaction_script_on_store(
        &store,
        "BEGIN; CREATE (:Person {id: 'p1'}); ROLLBACK",
        CypherMutationOptions::default(),
    )
    .await
    .expect("rollback script succeeds");
    assert_eq!(report, GraphMutationReport::default());
    assert!(!has_node(&store, "p1").await);
}

#[tokio::test]
async fn planning_error_aborts_before_any_store_write() {
    let store = store().await;
    // The second statement violates the strict-write surface (no explicit id),
    // so planning fails at add time and the earlier valid statement never
    // reaches the store: all-or-nothing.
    let err = run_cypher_transaction_script_on_store(
        &store,
        "BEGIN; \
         CREATE (:Person {id: 'p1'}); \
         CREATE (:Person {name: 'no id'}); \
         COMMIT",
        CypherMutationOptions::default(),
    )
    .await
    .expect_err("strict-write rejection fails the whole script");
    assert!(
        err.to_string().contains("explicit"),
        "unexpected error: {err}"
    );
    assert!(!has_node(&store, "p1").await);
}
