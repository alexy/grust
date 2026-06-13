use super::*;

use std::sync::Mutex;

use async_trait::async_trait;

fn sample_graph() -> Graph {
    let mut talk_props = Props::new();
    talk_props.insert("id".to_string(), Value::from("talk-1"));
    talk_props.insert("title".to_string(), Value::from("A Talk"));
    talk_props.insert(
        "tags".to_string(),
        Value::StringArray(vec!["rust".to_string(), "graphs".to_string()]),
    );

    let mut person_props = Props::new();
    person_props.insert("id".to_string(), Value::from("person-1"));
    person_props.insert("name".to_string(), Value::from("Ada Lovelace"));

    Graph {
        nodes: vec![
            Node::new("Talk", "talk-1", talk_props),
            Node::new("Person", "person-1", person_props),
        ],
        edges: vec![Edge::new("presents", "person-1", "talk-1", Props::new())],
    }
}

#[derive(Default)]
struct RecordingStore {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    edge_queries: Mutex<Vec<EdgeQuery>>,
    node_queries: Mutex<Vec<NodeId>>,
    node_batch_queries: Mutex<Vec<Vec<NodeId>>>,
}

#[async_trait]
impl GraphStore for RecordingStore {
    async fn put_node(&self, _node: &Node) -> Result<PutOutcome> {
        Ok(PutOutcome::Upserted)
    }

    async fn put_edge(&self, _edge: &Edge) -> Result<PutOutcome> {
        Ok(PutOutcome::Upserted)
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        self.node_queries.lock().unwrap().push(id.clone());
        Ok(self.nodes.iter().find(|node| &node.id == id).cloned())
    }

    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>> {
        self.node_batch_queries.lock().unwrap().push(ids.to_vec());
        Ok(ids
            .iter()
            .filter_map(|id| self.nodes.iter().find(|node| &node.id == id).cloned())
            .collect())
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        self.edge_queries.lock().unwrap().push(query.clone());
        Ok(self
            .edges
            .iter()
            .filter(|edge| {
                query.from.as_ref().is_none_or(|from| from == &edge.from)
                    && query.to.as_ref().is_none_or(|to| to == &edge.to)
                    && query
                        .label
                        .as_ref()
                        .is_none_or(|label| label == &edge.label)
            })
            .cloned()
            .collect())
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        traverse_steps_with_store(self, Vec::new(), traversal.steps, traversal.limit).await
    }
}

#[test]
fn upsert_nodes_preserves_arrays() {
    let graph = sample_graph();
    let query = surreal_upsert_nodes_query(&graph.nodes).unwrap();
    assert!(query.contains("UPSERT type::record(\"talk\", \"talk-1\")"));
    assert!(query.contains("tags = [\"rust\",\"graphs\"]"));
}

#[test]
fn traverse_steps_follow_edges_and_filter_target_label() {
    let graph = sample_graph();
    let store = RecordingStore {
        nodes: graph.nodes.clone(),
        edges: graph.edges.clone(),
        ..RecordingStore::default()
    };

    let nodes = futures_executor::block_on(traverse_steps_with_store(
        &store,
        vec![graph.nodes[1].clone()],
        vec![Step {
            direction: Direction::Out,
            edge: Some(Label::new("presents")),
            node: Some(Label::new("Talk")),
        }],
        Some(1),
    ))
    .unwrap();

    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].id, NodeId::new("talk-1"));

    let edge_queries = store.edge_queries.lock().unwrap();
    assert_eq!(edge_queries[0].from, Some(NodeId::new("person-1")));
    assert_eq!(edge_queries[0].label, Some(Label::new("presents")));

    let node_queries = store.node_queries.lock().unwrap();
    assert!(
        node_queries.is_empty(),
        "traversal should batch target node reads"
    );

    let node_batch_queries = store.node_batch_queries.lock().unwrap();
    assert_eq!(node_batch_queries[0], vec![NodeId::new("talk-1")]);
}

#[test]
fn relate_edges_are_idempotent_by_endpoints() {
    let graph = sample_graph();
    let id_tables = surreal_id_tables(&graph.nodes).unwrap();
    let query = surreal_relate_edges_query(&graph.edges, &id_tables).unwrap();
    assert!(query.contains("DELETE presents WHERE in = type::record(\"person\", \"person-1\")"));
    assert!(query.contains("RELATE (type::record(\"person\", \"person-1\"))->presents->(type::record(\"talk\", \"talk-1\"))"));
    assert!(query.contains("relationship = \"presents\""));
}

#[test]
fn get_node_query_scans_candidate_tables_in_one_statement() {
    let config = SurrealConfig {
        labels: vec!["Talk".to_string(), "Person".to_string()],
        ..SurrealConfig::default()
    };
    let query = surreal_get_node_query(&NodeId::new("talk-1"), &config);

    assert!(query.starts_with("SELECT *, meta::tb(id) AS __grust_label FROM "));
    assert!(query.contains("person, record, talk"));
    assert!(query.contains("id = type::record(\"talk\", \"talk-1\")"));
    assert_eq!(query.matches("SELECT").count(), 1);
}

#[test]
fn get_nodes_query_batches_candidate_records_in_one_statement() {
    let config = SurrealConfig {
        labels: vec!["Talk".to_string(), "Person".to_string()],
        ..SurrealConfig::default()
    };
    let query = surreal_get_nodes_query(&[NodeId::new("person-1"), NodeId::new("talk-1")], &config);

    assert!(query.starts_with("SELECT *, meta::tb(id) AS __grust_label FROM "));
    assert!(query.contains("person, record, talk"));
    assert!(query.contains("id = type::record(\"person\", \"person-1\")"));
    assert!(query.contains("id = type::record(\"talk\", \"talk-1\")"));
    assert_eq!(query.matches("SELECT").count(), 1);
}

#[test]
fn get_edges_query_uses_relationship_tables_in_one_statement() {
    let config = SurrealConfig {
        relationships: vec!["presents".to_string(), "member_of".to_string()],
        ..SurrealConfig::default()
    };
    let query = surreal_get_edges_query(&EdgeQuery::default(), &config);

    assert!(query.contains("FROM member_of, presents"));
    assert_eq!(query.matches("SELECT").count(), 1);
}

#[test]
fn surreal_response_parsers_rebuild_grust_values() {
    let node = surreal_node_from_value(serde_json::json!({
        "id": "person:`person-1`",
        "__grust_label": "person",
        "name": "Ada Lovelace"
    }))
    .unwrap();
    let edge = surreal_edge_from_value(serde_json::json!({
        "id": "presents:abc",
        "__grust_label": "presents",
        "in": "person:`person-1`",
        "out": "talk:`talk-1`",
        "relationship": "presents",
        "source": "schedule"
    }))
    .unwrap();

    assert_eq!(node.id, NodeId::new("person-1"));
    assert_eq!(node.label, Label::new("person"));
    assert_eq!(edge.from, NodeId::new("person-1"));
    assert_eq!(edge.to, NodeId::new("talk-1"));
    assert_eq!(edge.label, Label::new("presents"));
}

#[test]
fn graph_schema_defines_schemafull_tables_and_fields() {
    let schema = GraphSchema::builder()
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
        .build();

    let query = surreal_schema_query(&schema).unwrap();

    assert!(query.contains("DEFINE TABLE person SCHEMAFULL"));
    assert!(query.contains("DEFINE FIELD name ON TABLE person TYPE string"));
    assert!(query.contains("DEFINE FIELD age ON TABLE person TYPE int"));
    assert!(query.contains("DEFINE TABLE presents TYPE RELATION SCHEMAFULL"));
    assert!(query.contains("DEFINE FIELD source ON TABLE presents TYPE string"));
}

#[test]
fn sdk_uses_ws_address_from_http_sql_url() {
    assert_eq!(
        surreal_ws_address("http://127.0.0.1:8000/sql").unwrap(),
        "127.0.0.1:8000"
    );
}

#[test]
fn clear_response_ignores_missing_tables() {
    let response = serde_json::json!([
        {
            "status": "ERR",
            "kind": "NotFound",
            "details": {"kind": "Table", "details": {"name": "announcement"}},
            "result": "The table 'announcement' does not exist"
        },
        {"status": "OK", "result": []}
    ]);

    assert!(!surreal_response_has_non_idempotent_clear_error(&response));
}

#[test]
fn clear_response_keeps_non_missing_table_errors() {
    let response = serde_json::json!([
        {
            "status": "ERR",
            "kind": "Other",
            "result": "permission denied"
        }
    ]);

    assert!(surreal_response_has_non_idempotent_clear_error(&response));
}

#[tokio::test]
#[ignore = "requires a live SurrealDB server on 127.0.0.1:8000"]
async fn live_http_put_read_and_traverse() {
    let store = SurrealHttpGraphStore::connect(SurrealConfig {
        labels: vec!["Talk".to_string(), "Person".to_string()],
        relationships: vec!["presents".to_string()],
        ..SurrealConfig::default()
    })
    .expect("connect SurrealDB HTTP store");
    store.bootstrap().await.expect("bootstrap SurrealDB");
    store.clear().await.expect("clear SurrealDB");

    let graph = sample_graph();
    let report = store.put_graph(&graph).await.expect("write graph");
    assert_eq!(report.nodes, 2);
    assert_eq!(report.edges, 1);

    let fetched = store
        .get_node(&NodeId::new("talk-1"))
        .await
        .expect("read node")
        .expect("talk node missing");
    assert_eq!(fetched.label, Label::new("talk"));
    assert_eq!(fetched.id, NodeId::new("talk-1"));

    let result = store
        .traverse(Traversal::from_node("person-1").out("presents").to("talk"))
        .await
        .expect("traverse");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id, NodeId::new("talk-1"));
}
