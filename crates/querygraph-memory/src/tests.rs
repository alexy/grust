use super::*;
use chrono::{TimeZone, Utc};
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

/// The production bridge must own time and I/O drivers, not merely an
/// executor capable of polling the in-memory backend.
#[test]
fn bridge_owns_tokio_drivers() {
    let bridge = Bridge::new();
    bridge.run(async {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    });
}

/// Consolidation over the Grust backend commits the supersede as one
/// `apply_mutations` batch, and the superseded fact survives as bi-temporal
/// history (invalidated, not destroyed).
#[test]
fn consolidation_over_graphstore_supersedes_atomically() {
    use typesec_memory::{ConsolidationPlan, ConsolidationStep, RecallQuery};

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
    let draft = |t: &str, label: Label| {
        MemoryDraft::new(
            MemoryKind::Semantic,
            MemoryContent::text(t),
            Provenance::Operator,
        )
        .with_label(label)
    };
    let coffee = vault
        .remember(&space, &write, draft("likes coffee", Label::Public))
        .unwrap();
    let med = vault
        .remember(&space, &write, draft("medical note", Label::Sensitive))
        .unwrap();

    let plan = ConsolidationPlan::new().then(ConsolidationStep::Supersede {
        superseded: vec![coffee, med],
        replacement: draft("health & lifestyle summary", Label::Public),
    });
    let report = vault
        .consolidate(&space, &write, plan)
        .expect("consolidate");
    assert_eq!(report.invalidated.len(), 2);
    assert_eq!(report.created.len(), 1);

    // Live recall at Sensitive: exactly the summary (the two sources are
    // invalidated, not gone — bi-temporal history).
    let live = vault
        .recall::<typesec_core::secure_value::Sensitive>(
            &space,
            &read,
            RecallQuery::all(),
            &RequestContext::default(),
        )
        .expect("recall");
    assert_eq!(live.hits.len(), 1);
    assert_eq!(live.hits[0].content.text, "health & lifestyle summary");
    assert_eq!(
        live.hits[0].label,
        Label::Sensitive,
        "join raised the summary"
    );

    // Point-in-time before consolidation resurrects the superseded facts.
    let before = vault
        .store()
        .query(&typesec_memory::StoreQuery {
            space_id: Some(space.resource_id().to_string()),
            include_invalidated: true,
            ..Default::default()
        })
        .expect("raw query");
    assert_eq!(
        before.len(),
        3,
        "two invalidated sources + the summary all persist"
    );
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
        .recall_neighborhood(
            &space,
            &read,
            "ACME",
            1,
            Label::Internal,
            &typesec_core::policy::RequestContext::default(),
        )
        .expect("graph recall");
    assert!(
        hits.iter()
            .any(|h| h.content.text.contains("works at ACME"))
    );
    assert_eq!(redacted.len(), 1, "the sensitive neighbor stays sealed");
}

/// Analytics propose a plan; the vault applies it. The invariant is that
/// batch cognition never writes storage directly — it hands the vault a
/// ConsolidationPlan and the vault does the label-join, invalidation, and
/// audit. Here a contradiction analyzer retracts a superseded belief.
#[test]
fn analytics_plan_flows_through_the_vault_front_door() {
    use crate::analytics::contradiction_plan;
    use typesec_memory::RecallQuery;

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
    .unwrap();
    let read: Capability<CanRead, _> = mint_capability_for_id(
        &engine,
        "agent:keeper",
        space.resource_id(),
        &MintOptions::default(),
    )
    .unwrap();

    let vault = MemoryVault::new(store());
    vault
        .remember(
            &space,
            &write,
            MemoryDraft::new(
                MemoryKind::Semantic,
                MemoryContent::text("Alice lives in Rome"),
                Provenance::Operator,
            )
            .valid_from(Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap()),
        )
        .unwrap();
    vault
        .remember(
            &space,
            &write,
            MemoryDraft::new(
                MemoryKind::Semantic,
                MemoryContent::text("Alice lives in Venice"),
                Provenance::Operator,
            )
            .valid_from(Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap()),
        )
        .unwrap();

    // Recall the current view, run analytics on it, apply the plan.
    let view = vault
        .recall::<typesec_core::secure_value::Internal>(
            &space,
            &read,
            RecallQuery::all(),
            &RequestContext::default(),
        )
        .unwrap();
    let (found, plan) = contradiction_plan(&view.hits);
    assert_eq!(found.len(), 1, "Rome vs Venice is a contradiction");

    let report = vault
        .consolidate(&space, &write, plan)
        .expect("apply through vault");
    assert_eq!(report.invalidated.len(), 1);

    // Only the current belief remains live; the retracted one is history.
    let after = vault
        .recall::<typesec_core::secure_value::Internal>(
            &space,
            &read,
            RecallQuery::all(),
            &RequestContext::default(),
        )
        .unwrap();
    assert_eq!(after.hits.len(), 1);
    assert_eq!(after.hits[0].content.text, "Alice lives in Venice");
}
