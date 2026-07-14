#![cfg(feature = "turso")]

use grust_core::prelude::{GraphMutationAtomicity, GraphMutationStore};
use querygraph_memory::TursoMemoryStore;
use querygraph_memory::turso::TursoConfig;
use tempfile::TempDir;
use typesec_core::policy::{MintOptions, mint_capability_for_id};
use typesec_core::{CanWrite, Capability, Resource};
use typesec_memory::conformance::run_store_conformance;
use typesec_memory::{
    ConsolidationPlan, ConsolidationStep, Label, MemoryContent, MemoryDraft, MemoryId, MemoryKind,
    MemorySpace, MemoryStore, MemoryVault, Provenance, StoreQuery,
};

fn config(dir: &TempDir, table_prefix: &str) -> TursoConfig {
    TursoConfig {
        path: dir
            .path()
            .join("querygraph-memory.db")
            .to_string_lossy()
            .into_owned(),
        table_prefix: table_prefix.to_string(),
        batch_size: 64,
        ..TursoConfig::default()
    }
}

#[test]
fn persistent_turso_conforms_and_reopens() {
    let dir = tempfile::tempdir().expect("temporary database directory");
    let config = config(&dir, "memory_conformance");

    {
        let store =
            TursoMemoryStore::open_with_config(config.clone()).expect("open and bootstrap Turso");
        run_store_conformance(&store, true);
        assert!(
            store
                .get(&MemoryId::from_string("fx-acme"))
                .expect("read conformance record")
                .is_some()
        );
    }

    let reopened =
        TursoMemoryStore::open_with_config(config).expect("reopen persistent Turso store");
    assert!(
        reopened
            .get(&MemoryId::from_string("fx-acme"))
            .expect("read record after reopen")
            .is_some(),
        "a non-tombstoned record survives closing and reopening the database"
    );
    let neighborhood = reopened
        .neighborhood("ACME", 1)
        .expect("graph survives closing and reopening");
    assert!(neighborhood.contains(&MemoryId::from_string("fx-acme")));
}

#[test]
fn open_creates_the_database_parent_directory() {
    let dir = tempfile::tempdir().expect("temporary database directory");
    let database = dir.path().join("nested").join("memory.db");

    let store = TursoMemoryStore::open(&database).expect("open nested database path");
    drop(store);

    assert!(database.parent().expect("database parent").is_dir());
}

#[test]
fn consolidation_is_transactional_and_survives_reopen() {
    const POLICY: &str = r#"
roles:
  - name: keeper
    permissions: [read, write]
    resources: ["memory/**"]
assignments:
  - subject: "agent:keeper"
    roles: [keeper]
"#;

    let dir = tempfile::tempdir().expect("temporary database directory");
    let config = config(&dir, "memory_consolidation");
    let space = MemorySpace::new("user:alice", "semantic");
    let engine = typesec_rbac::RbacEngine::from_yaml(POLICY).expect("policy parses");
    let write: Capability<CanWrite, _> = mint_capability_for_id(
        &engine,
        "agent:keeper",
        space.resource_id(),
        &MintOptions::default(),
    )
    .expect("mint write capability");
    let created = {
        let store =
            TursoMemoryStore::open_with_config(config.clone()).expect("open and bootstrap Turso");
        assert_eq!(
            store.graph().mutation_atomicity(),
            GraphMutationAtomicity::Transactional
        );
        let vault = MemoryVault::new(store);
        let draft = |text: &str, label: Label| {
            MemoryDraft::new(
                MemoryKind::Semantic,
                MemoryContent::text(text),
                Provenance::Operator,
            )
            .with_label(label)
        };
        let public = vault
            .remember(&space, &write, draft("likes coffee", Label::Public))
            .expect("write public source");
        let sensitive = vault
            .remember(&space, &write, draft("medical note", Label::Sensitive))
            .expect("write sensitive source");
        let report = vault
            .consolidate(
                &space,
                &write,
                ConsolidationPlan::new().then(ConsolidationStep::Supersede {
                    superseded: vec![public, sensitive],
                    replacement: draft("health and lifestyle summary", Label::Public),
                }),
            )
            .expect("transactional consolidation");
        assert_eq!(report.invalidated.len(), 2);
        assert_eq!(report.created.len(), 1);
        report.created[0].clone()
    };

    let reopened =
        TursoMemoryStore::open_with_config(config).expect("reopen persistent Turso store");
    let records = reopened
        .query(&StoreQuery {
            space_id: Some(space.resource_id().to_string()),
            include_invalidated: true,
            ..StoreQuery::default()
        })
        .expect("query persisted consolidation state");
    assert_eq!(records.len(), 3, "sources and replacement all persist");
    assert_eq!(
        records
            .iter()
            .filter(|record| record.invalid_at.is_some())
            .count(),
        2,
        "both source records were invalidated in the batch"
    );
    assert_eq!(
        reopened
            .get(&created)
            .expect("read replacement after reopen")
            .expect("replacement persists")
            .label,
        Label::Sensitive,
        "the SecLib join survives durable storage"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn turso_store_opens_and_runs_inside_an_existing_runtime() {
    let dir = tempfile::tempdir().expect("temporary database directory");
    let store = TursoMemoryStore::open(dir.path().join("nested-runtime.db"))
        .expect("construct Turso store from inside Tokio");

    run_store_conformance(&store, true);
    assert!(
        store
            .get(&MemoryId::from_string("fx-bob"))
            .expect("read through nested runtime bridge")
            .is_some()
    );
}
