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

#[test]
fn applied_schema_validates_memory_graph_writes() {
    let schema = GraphSchema::builder()
        .node("Person", vec![Field::required("name", FieldType::String)])
        .node("Project", vec![Field::required("name", FieldType::String)])
        .edge(
            "WORKS_ON",
            vec![Label::new("Person")],
            vec![Label::new("Project")],
            Vec::<Field>::new(),
        )
        .build();
    let store = MemoryGraphStore::new();
    futures_executor::block_on(store.apply_schema(&schema)).unwrap();

    let error =
        futures_executor::block_on(store.put_node(&Node::new("Person", "person-1", Props::new())))
            .expect_err("missing required field should fail");

    assert!(error.to_string().contains("missing required field 'name'"));
}

#[test]
fn put_reports_insert_vs_update() {
    let store = MemoryGraphStore::new();
    let node = Node::new("Person", "a", Props::new());

    assert_eq!(
        futures_executor::block_on(store.put_node(&node)).unwrap(),
        PutOutcome::Inserted
    );
    assert_eq!(
        futures_executor::block_on(store.put_node(&node)).unwrap(),
        PutOutcome::Updated
    );
}

#[test]
fn get_nodes_reads_multiple_ids() {
    let store = MemoryGraphStore::new();
    let nodes = vec![
        Node::new("Person", "a", Props::new()),
        Node::new("Person", "b", Props::new()),
    ];
    futures_executor::block_on(store.put_node(&nodes[0])).unwrap();
    futures_executor::block_on(store.put_node(&nodes[1])).unwrap();

    let fetched = futures_executor::block_on(store.get_nodes(&[
        NodeId::new("b"),
        NodeId::new("missing"),
        NodeId::new("a"),
    ]))
    .unwrap();

    assert_eq!(
        fetched
            .iter()
            .map(|node| node.id.clone())
            .collect::<Vec<_>>(),
        vec![NodeId::new("b"), NodeId::new("a")]
    );
}

#[test]
fn delete_node_cascades_to_incident_edges() {
    let store = MemoryGraphStore::new();
    let mut builder = Graph::builder();
    builder.node("Person", "a").finish();
    builder.node("Person", "b").finish();
    builder.edge("KNOWS", "a", "b").finish();
    futures_executor::block_on(store.put_graph(&builder.build())).unwrap();

    futures_executor::block_on(store.delete_node(&NodeId::new("a"))).unwrap();

    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("a")))
            .unwrap()
            .is_none()
    );
    assert!(
        futures_executor::block_on(store.get_edges(EdgeQuery::default()))
            .unwrap()
            .is_empty(),
        "incident edges should be deleted"
    );
    // Deleting again is idempotent.
    futures_executor::block_on(store.delete_node(&NodeId::new("a"))).unwrap();
}

#[test]
fn apply_mutations_upserts_and_deletes() {
    let store = MemoryGraphStore::new();
    let mutations = vec![
        GraphMutation::UpsertNode(Node::new("Person", "a", Props::new())),
        GraphMutation::UpsertNode(Node::new("Person", "b", Props::new())),
        GraphMutation::UpsertEdge(Edge::new("KNOWS", "a", "b", Props::new())),
        GraphMutation::DeleteEdge {
            id: None,
            from: NodeId::new("a"),
            label: Label::new("KNOWS"),
            to: NodeId::new("b"),
        },
        GraphMutation::DeleteNode(NodeId::new("b")),
    ];

    futures_executor::block_on(store.apply_mutations(&mutations)).unwrap();

    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("a")))
            .unwrap()
            .is_some()
    );
    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("b")))
            .unwrap()
            .is_none()
    );
    assert!(
        futures_executor::block_on(store.get_edges(EdgeQuery::default()))
            .unwrap()
            .is_empty()
    );
}
