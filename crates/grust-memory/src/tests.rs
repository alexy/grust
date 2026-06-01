use super::*;

#[test]
fn stores_graph_and_traverses_one_step() {
    let mut builder = GraphBuilder::new();
    let talk = builder.node("Talk", "talk-1").finish();
    let person = builder.node("Person", "person-1").finish();
    builder.edge("PRESENTED_BY", &talk, &person).finish();
    let graph = builder.build();

    let store = MemoryGraphStore::new();
    futures_executor::block_on(store.put_graph(&graph)).unwrap();
    let speakers = futures_executor::block_on(
        store.traverse(
            Traversal::from_node("talk-1")
                .out("PRESENTED_BY")
                .to("Person"),
        ),
    )
    .unwrap();

    assert_eq!(speakers.len(), 1);
    assert_eq!(speakers[0].id, NodeId::from("person-1"));
}
