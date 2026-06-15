use grust_core::prelude::*;
use tempfile::TempDir;

use crate::IssunGraphStore;

fn store() -> (TempDir, IssunGraphStore) {
    let dir = TempDir::new().expect("create temp dir");
    let store = IssunGraphStore::open(dir.path()).expect("open issundb store");
    (dir, store)
}

fn person(id: &str, name: &str) -> Node {
    let mut props = Props::new();
    props.insert("name".to_string(), Value::from(name));
    Node::new("Person", id, props)
}

#[tokio::test]
async fn put_and_get_node_round_trips() {
    let (_dir, store) = store();

    let outcome = store.put_node(&person("alice", "Alice")).await.unwrap();
    assert_eq!(outcome, PutOutcome::Inserted);

    let fetched = store
        .get_node(&NodeId::new("alice"))
        .await
        .unwrap()
        .expect("node present");
    assert_eq!(fetched.id, NodeId::new("alice"));
    assert_eq!(fetched.label, Label::new("Person"));
    assert_eq!(fetched.props.get("name"), Some(&Value::from("Alice")));

    // Writing the same id again updates in place.
    let outcome = store.put_node(&person("alice", "Alice B.")).await.unwrap();
    assert_eq!(outcome, PutOutcome::Updated);
    let fetched = store
        .get_node(&NodeId::new("alice"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.props.get("name"), Some(&Value::from("Alice B.")));
}

#[tokio::test]
async fn edges_and_traversal() {
    let (_dir, store) = store();
    store.put_node(&person("alice", "Alice")).await.unwrap();
    store.put_node(&person("bob", "Bob")).await.unwrap();

    let mut edge_props = Props::new();
    edge_props.insert("since".to_string(), Value::Int(2021));
    let edge = Edge::new("KNOWS", "alice", "bob", edge_props);
    assert_eq!(store.put_edge(&edge).await.unwrap(), PutOutcome::Inserted);

    let edges = store
        .get_edges(EdgeQuery {
            from: Some(NodeId::new("alice")),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].to, NodeId::new("bob"));
    assert_eq!(edges[0].label, Label::new("KNOWS"));
    assert_eq!(edges[0].props.get("since"), Some(&Value::Int(2021)));

    let neighbors = store
        .traverse(Traversal::from_node("alice").out("KNOWS"))
        .await
        .unwrap();
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0].id, NodeId::new("bob"));
}

#[tokio::test]
async fn delete_node_and_edge() {
    let (_dir, store) = store();
    store.put_node(&person("alice", "Alice")).await.unwrap();
    store.put_node(&person("bob", "Bob")).await.unwrap();
    store
        .put_edge(&Edge::new("KNOWS", "alice", "bob", Props::new()))
        .await
        .unwrap();

    store
        .delete_edge(
            &NodeId::new("alice"),
            &Label::new("KNOWS"),
            &NodeId::new("bob"),
        )
        .await
        .unwrap();
    let edges = store
        .get_edges(EdgeQuery {
            from: Some(NodeId::new("alice")),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(edges.is_empty());

    store.delete_node(&NodeId::new("alice")).await.unwrap();
    assert!(
        store
            .get_node(&NodeId::new("alice"))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn reopen_rebuilds_id_index() {
    let dir = TempDir::new().unwrap();
    {
        let store = IssunGraphStore::open(dir.path()).unwrap();
        store.put_node(&person("alice", "Alice")).await.unwrap();
    }
    // A fresh handle over the same files recovers the string-id index.
    let store = IssunGraphStore::open(dir.path()).unwrap();
    let fetched = store.get_node(&NodeId::new("alice")).await.unwrap();
    assert_eq!(fetched.map(|n| n.id), Some(NodeId::new("alice")));
}
