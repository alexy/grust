use super::*;

#[test]
fn builder_dedupes_nodes_and_edges() {
    let mut builder = GraphBuilder::new();
    let talk = builder
        .node("Talk", "talk-1")
        .prop("title", "A Talk")
        .finish();
    let person = builder.node("Person", "person-1").finish();
    builder
        .node("Talk", "talk-1")
        .prop("description", "Updated")
        .finish();
    builder.edge("PRESENTS", &person, &talk).finish();
    builder.edge("PRESENTS", &person, &talk).finish();

    let graph = builder.build();

    assert_eq!(graph.nodes.len(), 2);
    assert_eq!(graph.edges.len(), 1);
    assert!(graph.nodes.iter().any(|node| {
        node.id == NodeId::from("talk-1")
            && node.props.get("id") == Some(&Value::from("talk-1"))
            && node.props.get("description") == Some(&Value::from("Updated"))
    }));
}
