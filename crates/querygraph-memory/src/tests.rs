use super::*;
use grust_memory::MemoryGraphStore;
use typesec_core::policy::{MintOptions, RequestContext, mint_capability_for_id};
use typesec_core::{CanRead, CanWrite, Capability, Resource};
use typesec_memory::conformance::run_store_conformance;
use typesec_memory::{
    EntityRef, Label, MemoryContent, MemoryDraft, MemoryKind, MemorySpace, MemoryVault, Provenance,
};

fn store() -> GraphStoreMemoryStore<MemoryGraphStore> {
    GraphStoreMemoryStore::new(MemoryGraphStore::default())
}

/// The compatibility bar: the full typesec-memory conformance corpus,
/// including the graph reachability cases.
#[test]
fn conforms_to_the_marciana_corpus() {
    run_store_conformance(&store(), true);
}

/// The bridge must be safe when the caller is already inside a tokio runtime
/// (the memory-serve MCP server is exactly this shape).
#[tokio::test(flavor = "multi_thread")]
async fn bridge_survives_being_called_from_inside_tokio() {
    let s = store();
    tokio::task::spawn_blocking(move || run_store_conformance(&s, true))
        .await
        .expect("conformance inside tokio");
}

/// End to end: the capability-gated vault running over a Grust backend.
#[test]
fn vault_over_graphstore_gates_and_traverses() {
    const POLICY: &str = r#"
roles:
  - name: keeper
    permissions: [read, write]
    resources: ["memory/**"]
assignments:
  - subject: "agent:keeper"
    roles: [keeper]
"#;
    let engine = typesec_rbac::RbacEngine::from_yaml(POLICY).expect("policy parses");
    let space = MemorySpace::new("user:alice", "semantic");
    let write: Capability<CanWrite, _> = mint_capability_for_id(
        &engine,
        "agent:keeper",
        space.resource_id(),
        &MintOptions::default(),
    )
    .expect("mint write");
    let read: Capability<CanRead, _> = mint_capability_for_id(
        &engine,
        "agent:keeper",
        space.resource_id(),
        &MintOptions::default(),
    )
    .expect("mint read");

    let vault = MemoryVault::new(store());
    vault
        .remember(
            &space,
            &write,
            MemoryDraft::new(
                MemoryKind::Semantic,
                MemoryContent::text("Alice works at ACME"),
                Provenance::Operator,
            )
            .with_entities([EntityRef::new("ACME", "org")]),
        )
        .expect("remember");
    let secret = vault
        .remember(
            &space,
            &write,
            MemoryDraft::new(
                MemoryKind::Semantic,
                MemoryContent::text("ACME HQ vault code"),
                Provenance::Operator,
            )
            .with_label(Label::Sensitive)
            .with_entities([EntityRef::new("Venice", "place")]),
        )
        .expect("remember sensitive");
    vault
        .store()
        .link("ACME", "based_in", "Venice", &secret)
        .expect("link");

    // Plain recall through the label gate.
    let recall = vault
        .recall::<typesec_core::secure_value::Internal>(
            &space,
            &read,
            typesec_memory::RecallQuery::all(),
            &RequestContext::default(),
        )
        .expect("recall");
    assert_eq!(recall.hits.len(), 1);
    assert_eq!(
        recall.redacted.len(),
        1,
        "sensitive record redacted at Internal"
    );

    // Graph recall: 1 hop from ACME reaches the Venice record, still gated.
    let (hits, redacted) = vault
        .recall_neighborhood(&space, &read, "ACME", 1, Label::Internal)
        .expect("graph recall");
    assert!(
        hits.iter()
            .any(|h| h.content.text.contains("works at ACME"))
    );
    assert_eq!(redacted.len(), 1, "the sensitive neighbor stays sealed");
}
