use grust_core::prelude::*;
use tempfile::tempdir;

use super::*;

async fn store() -> LanceDbGraphStore {
    let dir = tempdir().expect("tempdir");
    let uri = dir.keep().display().to_string();
    let store = LanceDbGraphStore::connect(LanceDbConfig {
        uri,
        table_prefix: "test_graph".to_string(),
        batch_size: 2,
    })
    .await
    .expect("connect");
    store.bootstrap().await.expect("bootstrap");
    store.clear().await.expect("clear");
    store
}

fn sample_graph() -> Graph {
    let mut builder = Graph::builder();
    builder
        .node("Person", "person-1")
        .prop("name", "Ada")
        .prop("age", 36i64)
        .finish();
    builder
        .node("Talk", "talk-1")
        .prop("title", "Analytical Engine")
        .finish();
    builder.node("Room", "room-1").prop("name", "Main").finish();
    builder
        .edge("PRESENTS", "person-1", "talk-1")
        .prop("source", "schedule")
        .finish();
    builder.edge("HOSTED_IN", "talk-1", "room-1").finish();
    builder.build()
}

#[tokio::test]
async fn bootstrap_creates_empty_tables() {
    let store = store().await;

    let nodes = store.open_nodes().await.expect("nodes table");
    let edges = store.open_edges().await.expect("edges table");

    assert_eq!(nodes.count_rows(None).await.expect("node count"), 0);
    assert_eq!(edges.count_rows(None).await.expect("edge count"), 0);
}

#[tokio::test]
async fn apply_schema_creates_typed_tables_and_mirrors_writes() {
    let store = store().await;
    let schema = GraphSchema::builder()
        .node(
            "Person",
            vec![
                grust_core::Field::required("name", FieldType::String),
                grust_core::Field::optional("age", FieldType::Int),
            ],
        )
        .node(
            "Talk",
            vec![grust_core::Field::required("title", FieldType::String)],
        )
        .node(
            "Room",
            vec![grust_core::Field::required("name", FieldType::String)],
        )
        .edge(
            "PRESENTS",
            vec![Label::new("Person")],
            vec![Label::new("Talk")],
            vec![grust_core::Field::required("source", FieldType::String)],
        )
        .edge(
            "HOSTED_IN",
            vec![Label::new("Talk")],
            vec![Label::new("Room")],
            Vec::<grust_core::Field>::new(),
        )
        .build();

    store.apply_schema(&schema).await.expect("apply_schema");
    store.put_graph(&sample_graph()).await.expect("put_graph");

    let person_table = store
        .open_table(&store.typed_node_table_name("Person").unwrap())
        .await
        .expect("typed person table");
    let edge_table = store
        .open_table(&store.typed_edge_table_name("PRESENTS").unwrap())
        .await
        .expect("typed presents table");

    assert_eq!(person_table.count_rows(None).await.expect("person rows"), 1);
    assert_eq!(edge_table.count_rows(None).await.expect("edge rows"), 1);
}

#[tokio::test]
async fn applied_schema_rejects_wrong_typed_property() {
    let store = store().await;
    let schema = GraphSchema::builder()
        .node(
            "Person",
            vec![grust_core::Field::required("age", FieldType::Int)],
        )
        .build();
    store.apply_schema(&schema).await.expect("apply_schema");

    let error = store
        .put_node(&Node::new("Person", "person-1", {
            let mut props = Props::new();
            props.insert("age".to_string(), Value::from("old"));
            props
        }))
        .await
        .expect_err("wrong field type should fail");

    assert!(error.to_string().contains("field 'age' expected Int"));
}

#[tokio::test]
async fn put_and_get_node() {
    let store = store().await;
    let node = Node::new("Person", "person-1", {
        let mut props = Props::new();
        props.insert("name".to_string(), Value::from("Ada"));
        props
    });

    let id = store.put_node(&node).await.expect("put_node");
    assert_eq!(id.as_str(), "person-1");

    let fetched = store
        .get_node(&NodeId::new("person-1"))
        .await
        .expect("get_node")
        .expect("node exists");
    assert_eq!(fetched.label.as_str(), "Person");
    assert_eq!(
        fetched.props.get("name").and_then(Value::as_str),
        Some("Ada")
    );
}

#[tokio::test]
async fn idempotent_put_node_updates_props() {
    let store = store().await;
    store
        .put_node(&Node::new("Person", "person-1", {
            let mut props = Props::new();
            props.insert("name".to_string(), Value::from("Ada v1"));
            props
        }))
        .await
        .expect("first put");

    store
        .put_node(&Node::new("Person", "person-1", {
            let mut props = Props::new();
            props.insert("name".to_string(), Value::from("Ada v2"));
            props
        }))
        .await
        .expect("second put");

    let fetched = store
        .get_node(&NodeId::new("person-1"))
        .await
        .expect("get_node")
        .expect("node exists");
    assert_eq!(
        fetched.props.get("name").and_then(Value::as_str),
        Some("Ada v2")
    );
}

#[tokio::test]
async fn put_graph_and_get_edges() {
    let store = store().await;
    let graph = sample_graph();
    let report = store.put_graph(&graph).await.expect("put_graph");
    assert_eq!(report.nodes, 3);
    assert_eq!(report.edges, 2);

    let edges = store
        .get_edges(EdgeQuery {
            from: Some(NodeId::new("person-1")),
            label: Some(Label::new("PRESENTS")),
            ..Default::default()
        })
        .await
        .expect("get_edges");

    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].to.as_str(), "talk-1");
    assert_eq!(edges[0].label.as_str(), "PRESENTS");
}

#[tokio::test]
async fn traverse_one_and_two_hops() {
    let store = store().await;
    store.put_graph(&sample_graph()).await.expect("put_graph");

    let talks = store
        .traverse(Traversal::from_node("person-1").out("PRESENTS").to("Talk"))
        .await
        .expect("one hop");
    assert_eq!(talks.len(), 1);
    assert_eq!(talks[0].id.as_str(), "talk-1");

    let rooms = store
        .traverse(
            Traversal::from_node("person-1")
                .out("PRESENTS")
                .to("Talk")
                .out("HOSTED_IN")
                .to("Room"),
        )
        .await
        .expect("two hops");
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0].id.as_str(), "room-1");
}

#[tokio::test]
async fn starts_by_label_and_property() {
    let store = store().await;
    store.put_graph(&sample_graph()).await.expect("put_graph");

    let people = store
        .traverse(Traversal {
            start: Start::NodesByLabel(Label::new("Person")),
            steps: Vec::new(),
            limit: None,
        })
        .await
        .expect("label start");
    assert_eq!(people.len(), 1);

    let ada = store
        .traverse(Traversal {
            start: Start::NodesByProperty {
                label: Label::new("Person"),
                key: "name".to_string(),
                value: Value::from("Ada"),
            },
            steps: Vec::new(),
            limit: None,
        })
        .await
        .expect("property start");
    assert_eq!(ada.len(), 1);
    assert_eq!(ada[0].id.as_str(), "person-1");
}

#[tokio::test]
async fn clear_removes_graph() {
    let store = store().await;
    store.put_graph(&sample_graph()).await.expect("put_graph");
    store.clear().await.expect("clear");

    let missing = store
        .get_node(&NodeId::new("person-1"))
        .await
        .expect("get_node");
    assert!(missing.is_none());
}

#[test]
fn edge_key_prefers_explicit_id() {
    let edge = Edge::new("KNOWS", "a", "b", Props::new()).with_id("edge-1");
    assert_eq!(edge_key(&edge), "edge-1");

    let edge = Edge::new("KNOWS", "a", "b", Props::new());
    assert_eq!(edge_key(&edge), "a\u{1f}KNOWS\u{1f}b");
}
