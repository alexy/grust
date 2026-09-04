#![cfg(feature = "turso")]

mod support;

use std::collections::BTreeMap;

use grust_core::prelude::{GraphStore, Node, NodeId, Value};
use querygraph_memory::TursoMemoryStore;
use typesec_memory::{
    ConsolidationPlan, ConsolidationStep, EntityRef, MemoryContent, MemoryDraft, MemoryError,
    MemoryStore, Provenance,
};

use support::cognition_vault::CognitionFixture;
use support::{config, record};

#[tokio::test]
async fn conflicting_shared_entity_identity_aborts_the_whole_commit() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_entity_guard"))
        .expect("open store");
    let source = record("source", "source text", None);
    let fixture = CognitionFixture::customize(store, source.clone(), "job", |proposal| {
        proposal.plan = ConsolidationPlan::new().then(ConsolidationStep::Supersede {
            superseded: vec![source.id.clone()],
            replacement: MemoryDraft::new(
                source.kind,
                MemoryContent::text("replacement"),
                Provenance::Operator,
            )
            .with_entities([EntityRef::new("Alice", "person")]),
        });
    });
    let conflicting = Node::new(
        "MemoryEntity",
        NodeId::from("ent:Alice"),
        BTreeMap::from([
            ("name".into(), Value::String("Alice".into())),
            ("kind".into(), Value::String("organization".into())),
        ]),
    );
    fixture
        .store()
        .graph()
        .put_node(&conflicting)
        .await
        .expect("seed conflicting entity");

    assert!(matches!(
        fixture.apply(),
        Err(MemoryError::CognitionCommit(_))
    ));
    assert_eq!(
        fixture
            .store()
            .get(&source.id)
            .expect("read source")
            .expect("source remains")
            .invalid_at,
        None
    );
    assert_eq!(
        fixture
            .store()
            .graph()
            .get_node(&NodeId::from("ent:Alice"))
            .await
            .expect("read entity")
            .expect("entity remains"),
        conflicting
    );
}
