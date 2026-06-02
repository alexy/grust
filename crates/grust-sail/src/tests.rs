use std::net::TcpStream;

use grust_core::prelude::*;

use super::*;

fn sail_available() -> bool {
    TcpStream::connect("127.0.0.1:50051").is_ok()
}

async fn store() -> Option<SailGraphStore> {
    if !sail_available() {
        return None;
    }
    let store = SailGraphStore::connect(SailConfig::default()).await.ok()?;
    store.bootstrap().await.ok()?;
    store.clear().await.ok()?;
    Some(store)
}

fn sample_graph() -> Graph {
    let mut b = Graph::builder();
    b.node("Person", "person-1")
        .prop("name", "Ada Lovelace")
        .prop("age", 36i64)
        .finish();
    b.node("Talk", "talk-1")
        .prop("title", "Analytical Engine")
        .finish();
    b.edge("presents", "person-1", "talk-1").finish();
    b.build()
}

#[tokio::test]
async fn test_put_and_get_node() {
    let Some(store) = store().await else { return };

    let node = Node::new("Person", "person-1", {
        let mut p = Props::new();
        p.insert("name".into(), Value::from("Ada Lovelace"));
        p
    });
    let id = store.put_node(&node).await.expect("put_node");
    assert_eq!(id.as_str(), "person-1");

    let fetched = store.get_node(&id).await.expect("get_node");
    assert!(fetched.is_some(), "node should exist after put");
    let fetched = fetched.unwrap();
    assert_eq!(fetched.id.as_str(), "person-1");
    assert_eq!(fetched.label.as_str(), "Person");
}

#[tokio::test]
async fn test_put_graph_and_traverse() {
    let Some(store) = store().await else { return };

    let graph = sample_graph();
    let report = store.put_graph(&graph).await.expect("put_graph");
    assert_eq!(report.nodes, 2);
    assert_eq!(report.edges, 1);

    let result = store
        .traverse(Traversal::from_node("person-1").out("presents"))
        .await
        .expect("traverse");
    assert!(
        !result.is_empty(),
        "traversal should return destination node"
    );
    assert_eq!(result[0].id.as_str(), "talk-1");
}

#[tokio::test]
async fn test_get_edges() {
    let Some(store) = store().await else { return };

    let graph = sample_graph();
    store.put_graph(&graph).await.expect("put_graph");

    let edges = store
        .get_edges(EdgeQuery {
            from: Some(NodeId::new("person-1")),
            ..Default::default()
        })
        .await
        .expect("get_edges");
    assert!(!edges.is_empty());
    assert_eq!(edges[0].label.as_str(), "presents");
}

#[tokio::test]
async fn test_idempotent_put_node() {
    let Some(store) = store().await else { return };

    let node = Node::new("Person", "person-1", {
        let mut p = Props::new();
        p.insert("name".into(), Value::from("Ada v1"));
        p
    });
    store.put_node(&node).await.expect("first put");

    let updated = Node::new("Person", "person-1", {
        let mut p = Props::new();
        p.insert("name".into(), Value::from("Ada v2"));
        p
    });
    store.put_node(&updated).await.expect("second put");

    let fetched = store
        .get_node(&NodeId::new("person-1"))
        .await
        .expect("get_node")
        .expect("node missing");
    let name = fetched
        .props
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(name, "Ada v2", "second put should overwrite props");
}
