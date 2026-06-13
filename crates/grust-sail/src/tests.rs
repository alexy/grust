use std::net::TcpStream;

use grust_core::prelude::*;

use super::*;

fn sail_available() -> bool {
    TcpStream::connect("127.0.0.1:50051").is_ok()
}

async fn store() -> SailGraphStore {
    assert!(
        sail_available(),
        "live Sail integration tests require a Sail server on 127.0.0.1:50051; run scripts/integration-test.sh --backend sail"
    );
    let store = SailGraphStore::connect(SailConfig::default())
        .await
        .expect("connect to Sail");
    store.bootstrap().await.expect("bootstrap Sail tables");
    store.clear().await.expect("clear Sail tables");
    store
}

fn sample_graph() -> Graph {
    let mut b = Graph::builder();
    let _ = b
        .node("Person", "person-1")
        .prop("name", "Ada Lovelace")
        .prop("age", 36i64)
        .finish();
    let _ = b
        .node("Talk", "talk-1")
        .prop("title", "Analytical Engine")
        .finish();
    let _ = b.edge("presents", "person-1", "talk-1").finish();
    b.build()
}

fn person_schema() -> GraphSchema {
    GraphSchema::builder()
        .node(
            "Person",
            vec![
                Field::required("name", FieldType::String),
                Field::optional("age", FieldType::Int),
            ],
        )
        .edge(
            "presents",
            vec![Label::new("Person")],
            vec![Label::new("Talk")],
            vec![Field::optional("source", FieldType::String)],
        )
        .build()
}

#[test]
fn schema_sql_creates_typed_delta_tables() {
    let sql = sail_schema_sql(&person_schema()).unwrap();

    assert!(sql.iter().any(|statement| statement.contains(
        "CREATE TABLE IF NOT EXISTS grust_node_person (id STRING NOT NULL, `name` STRING, `age` BIGINT) USING delta"
    )));
    assert!(sql.iter().any(|statement| {
        statement.contains("CREATE TABLE IF NOT EXISTS grust_edge_presents")
            && statement.contains("`source` STRING")
    }));
}

#[test]
fn typed_node_merge_extracts_fields_from_staged_json() {
    let schema = person_schema();
    let node_type = schema.node_type(&Label::new("Person")).unwrap();

    let sql = typed_node_merge_from_view_sql(node_type).unwrap();

    assert!(sql.contains("MERGE INTO grust_node_person"));
    assert!(sql.contains("FROM grust_stage_nodes s WHERE s.label = 'Person'"));
    assert!(sql.contains("GET_JSON_OBJECT(s.props, '$.name') AS `name`"));
    assert!(sql.contains("CAST(GET_JSON_OBJECT(s.props, '$.age') AS BIGINT) AS `age`"));
}

#[test]
fn typed_edge_merge_extracts_fields_from_staged_json() {
    let schema = person_schema();
    let edge_type = schema.edge_type(&Label::new("presents")).unwrap();

    let sql = typed_edge_merge_from_view_sql(edge_type).unwrap();

    assert!(sql.contains("MERGE INTO grust_edge_presents"));
    assert!(sql.contains("FROM grust_stage_edges s WHERE s.edge_type = 'presents'"));
    assert!(sql.contains("ON t.edge_key = s.edge_key"));
    assert!(sql.contains("GET_JSON_OBJECT(s.props, '$.source') AS `source`"));
}

#[test]
fn staged_node_batch_round_trips_through_arrow_ipc() {
    let graph = sample_graph();
    let batch = nodes_record_batch(&graph.nodes).unwrap();
    let bytes = ipc_bytes(&batch).unwrap();

    // The staging schema matches grust_nodes, so the read path parses it.
    let nodes = parse_nodes_from_arrow(&bytes).unwrap();

    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].id, NodeId::new("person-1"));
    assert_eq!(nodes[0].label, Label::new("Person"));
    assert_eq!(
        nodes[0].props.get("name"),
        Some(&Value::from("Ada Lovelace"))
    );
    assert_eq!(nodes[0].props.get("age"), Some(&Value::from(36i64)));
}

#[test]
fn staged_edge_batch_round_trips_through_arrow_ipc() {
    let graph = sample_graph();
    let node_labels: std::collections::BTreeMap<&NodeId, &Label> = graph
        .nodes
        .iter()
        .map(|node| (&node.id, &node.label))
        .collect();
    let batch = edges_record_batch(&graph.edges, &node_labels).unwrap();
    let bytes = ipc_bytes(&batch).unwrap();

    let edges = parse_edges_from_arrow(&bytes).unwrap();

    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].from, NodeId::new("person-1"));
    assert_eq!(edges[0].to, NodeId::new("talk-1"));
    assert_eq!(edges[0].label, Label::new("presents"));
}

#[test]
fn props_json_is_plain_and_reads_legacy_tagged_form() {
    let mut props = Props::new();
    props.insert("name".to_string(), Value::from("Ada"));
    props.insert("age".to_string(), Value::from(36i64));

    let json = props_to_json(&props).unwrap();
    assert_eq!(json, r#"{"age":36,"name":"Ada"}"#);

    let parsed = props_from_json(&json).unwrap();
    assert_eq!(parsed, props);

    let legacy = r#"{"name":{"type":"string","value":"Ada"},"age":{"type":"int","value":36}}"#;
    let parsed = props_from_json(legacy).unwrap();
    assert_eq!(parsed, props);
}

#[test]
fn props_json_rejects_non_finite_floats() {
    let mut props = Props::new();
    props.insert("score".to_string(), Value::Float(f64::NAN));
    let err = props_to_json(&props).expect_err("NaN must be rejected");
    assert!(err.to_string().contains("non-finite"));
}

#[test]
fn traversal_sql_joins_edges_by_id_and_binds_args() {
    // Edges staged without the full graph in scope carry empty
    // src_label/dst_label, so traversal joins must match on ids alone.
    let (sql, args) =
        traversal_sql(&Traversal::from_node("person-1").out("presents").to("Talk")).unwrap();

    assert!(sql.contains("JOIN grust_edges e0 ON e0.src_id = n0.id AND e0.edge_type = ?"));
    assert!(sql.contains("JOIN grust_nodes n1 ON n1.id = e0.dst_id AND n1.label = ?"));
    assert!(sql.contains("WHERE n0.id = ?"));
    assert!(
        !sql.contains("src_label") && !sql.contains("dst_label"),
        "traversal must not join on edge labels: {sql}"
    );
    assert_eq!(
        args,
        vec![lit_str("presents"), lit_str("Talk"), lit_str("person-1")]
    );

    let (sql, _) = traversal_sql(&Traversal::from_node("person-1").in_("presents")).unwrap();
    assert!(sql.contains("JOIN grust_edges e0 ON e0.dst_id = n0.id"));
    assert!(sql.contains("JOIN grust_nodes n1 ON n1.id = e0.src_id"));

    let (sql, _) = traversal_sql(&Traversal::from_node("person-1").both("presents")).unwrap();
    assert!(sql.contains("(e0.src_id = n0.id OR e0.dst_id = n0.id)"));
    assert!(sql.contains("CASE WHEN e0.src_id = n0.id THEN e0.dst_id ELSE e0.src_id END"));
}

#[test]
fn start_clause_binds_values_and_rejects_unsafe_json_keys() {
    let start = Start::NodesByProperty {
        label: Label::new("Person"),
        key: "name') = '' OR ('1'='1".to_string(),
        value: Value::from("x"),
    };
    let err = start_clause(&start, "n0").expect_err("unsafe key must be rejected");
    assert!(err.to_string().contains("invalid JSON property key"));

    let start = Start::NodesByProperty {
        label: Label::new("Person"),
        key: "age".to_string(),
        value: Value::from(36i64),
    };
    let (clause, args) = start_clause(&start, "n0").unwrap();
    assert!(clause.contains("n0.label = ?"));
    assert!(clause.contains("CAST(GET_JSON_OBJECT(n0.props, '$.age') AS BIGINT) = ?"));
    assert_eq!(args, vec![lit_str("Person"), lit_long(36)]);
}

#[test]
fn sql_str_escapes_backslashes_and_quotes() {
    assert_eq!(sql_str("plain"), "'plain'");
    assert_eq!(sql_str("it's"), "'it''s'");
    assert_eq!(sql_str(r"back\slash"), r"'back\\slash'");
    assert_eq!(sql_str(r"trailing\"), r"'trailing\\'");
    assert_eq!(sql_str(r"a\'b"), r"'a\\''b'");
}

#[test]
fn inline_sql_args_replaces_placeholders_with_escaped_literals() {
    let sql = inline_sql_args(
        "SELECT * FROM grust_nodes WHERE id = ? AND label = ? AND props = ?",
        &[
            lit_str("person-1"),
            lit_str("Person"),
            lit_str(r#"{"name":"Ada's"}"#),
        ],
    )
    .unwrap();
    assert_eq!(
        sql,
        r#"SELECT * FROM grust_nodes WHERE id = 'person-1' AND label = 'Person' AND props = '{"name":"Ada''s"}'"#
    );
    assert!(inline_sql_args("SELECT ?", &[]).is_err());
    assert!(inline_sql_args("SELECT 1", &[lit_long(1)]).is_err());
}

#[test]
fn clear_sql_drops_delta_tables_for_robust_reset() {
    assert_eq!(DROP_NODES_SQL, "DROP TABLE IF EXISTS grust_nodes");
    assert_eq!(DROP_EDGES_SQL, "DROP TABLE IF EXISTS grust_edges");
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_put_and_get_node() {
    let store = store().await;

    let node = Node::new("Person", "person-1", {
        let mut p = Props::new();
        p.insert("name".into(), Value::from("Ada Lovelace"));
        p
    });
    let outcome = store.put_node(&node).await.expect("put_node");
    assert!(outcome.written());

    let fetched = store.get_node(&node.id).await.expect("get_node");
    assert!(fetched.is_some(), "node should exist after put");
    let fetched = fetched.unwrap();
    assert_eq!(fetched.id.as_str(), "person-1");
    assert_eq!(fetched.label.as_str(), "Person");
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_put_graph_and_traverse() {
    let store = store().await;

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
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_get_edges() {
    let store = store().await;

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
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_idempotent_put_node() {
    let store = store().await;

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

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_delete_node_and_edge() {
    let store = store().await;

    let graph = sample_graph();
    store.put_graph(&graph).await.expect("put_graph");

    store
        .delete_edge(
            &NodeId::new("person-1"),
            &Label::new("presents"),
            &NodeId::new("talk-1"),
        )
        .await
        .expect("delete_edge");
    let edges = store
        .get_edges(EdgeQuery::default())
        .await
        .expect("get_edges");
    assert!(edges.is_empty(), "edge should be deleted");

    store
        .delete_node(&NodeId::new("person-1"))
        .await
        .expect("delete_node");
    let fetched = store
        .get_node(&NodeId::new("person-1"))
        .await
        .expect("get_node");
    assert!(fetched.is_none(), "node should be deleted");
}
