use super::*;

use std::sync::Mutex;

use async_trait::async_trait;

fn sample_graph() -> Graph {
    let mut talk_props = Props::new();
    talk_props.insert("title".to_string(), Value::from("A Talk"));
    talk_props.insert(
        "tags".to_string(),
        Value::StringArray(vec!["rust".to_string(), "graphs".to_string()]),
    );

    let mut person_props = Props::new();
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
    assert!(query.contains("`__grust_label` = \"Talk\""));
    assert!(query.contains("`__grust_label` = \"Person\""));
    assert!(query.contains("`tags` = [\"rust\",\"graphs\"]"));
    assert!(!query.contains("`id` ="));
}

#[test]
fn node_round_trip_preserves_logical_label_and_colons_inside_record_keys() {
    let node = Node::new("City", "City:4", Props::new());
    let query = surreal_upsert_nodes_query(std::slice::from_ref(&node)).unwrap();

    assert!(query.contains("type::record(\"city\", \"City:4\")"));
    assert!(query.contains("`__grust_label` = \"City\""));

    let decoded = surreal_node_from_value(serde_json::json!({
        "id": "city:`City:4`",
        "__grust_label": "City"
    }))
    .unwrap();
    assert_eq!(decoded, node);

    for record in [
        serde_json::json!("city:`City:4`"),
        serde_json::json!({"id": {"String": "City:4"}}),
        serde_json::json!({"id": "`City:4`"}),
    ] {
        assert_eq!(surreal_record_id(&record).unwrap(), NodeId::new("City:4"));
    }
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
    let mut graph = sample_graph();
    graph.edges[0].id = Some(EdgeId::new("edge-1"));
    let id_tables = surreal_id_tables(&graph.nodes).unwrap();
    let query =
        surreal_relate_edges_query(&graph.edges, &id_tables, &SurrealConfig::default()).unwrap();
    assert!(query.contains("DELETE `presents` WHERE in = type::record(\"person\", \"person-1\")"));
    assert!(query.contains("RELATE (type::record(\"person\", \"person-1\"))->`presents`->(type::record(\"talk\", \"talk-1\"))"));
    assert!(query.contains("`relationship` = \"presents\""));
    assert!(query.contains("`edge_id` = \"edge-1\""));
}

#[test]
fn mutation_batch_query_wraps_ordered_mutations_in_transaction() {
    let config = SurrealConfig {
        labels: vec!["Person".to_string(), "Talk".to_string()],
        relationships: vec!["presents".to_string()],
        ..SurrealConfig::default()
    };
    let query = surreal_apply_mutations_query(
        &[
            GraphMutation::UpsertNode(Node::new("Person", "person-1", Props::new())),
            GraphMutation::UpsertNode(Node::new("Talk", "talk-1", Props::new())),
            GraphMutation::UpsertEdge(Edge::new("presents", "person-1", "talk-1", Props::new())),
            GraphMutation::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([("name".to_string(), Value::from("Ada"))]),
            },
            GraphMutation::DeleteEdge {
                from: NodeId::new("person-1"),
                label: Label::new("presents"),
                to: NodeId::new("talk-1"),
            },
            GraphMutation::DeleteNode(NodeId::new("person-1")),
        ],
        &config,
    )
    .unwrap();

    assert!(query.starts_with("BEGIN TRANSACTION;\n"));
    assert!(query.ends_with("\nCOMMIT TRANSACTION;"));
    assert!(query.contains("UPSERT type::record(\"person\", \"person-1\")"));
    assert!(query.contains("RELATE"));
    assert!(query.contains("UPDATE type::record(\"person\", \"person-1\") SET `name` = \"Ada\";"));
    assert!(query.contains("->`presents`->"));
    assert!(query.contains("type::record(\"person\", \"person-1\")"));
    assert!(query.contains("type::record(\"talk\", \"talk-1\")"));
    assert!(query.contains("DELETE `presents` WHERE"));
    assert!(query.contains("DELETE type::record(\"person\", \"person-1\");"));
}

#[test]
fn http_store_reports_transactional_mutation_batches() {
    let store = SurrealHttpGraphStore::connect(SurrealConfig::default()).unwrap();

    assert_eq!(
        store.mutation_atomicity(),
        GraphMutationAtomicity::Transactional
    );
}

#[test]
fn delete_node_query_requires_relationship_config() {
    let err = surreal_delete_node_query(&NodeId::new("person-1"), &SurrealConfig::default())
        .expect_err("node deletes require configured relationships");

    assert!(
        err.to_string()
            .contains("SurrealConfig.relationships is empty")
    );
}

#[test]
fn delete_edge_query_uses_candidate_endpoint_tables() {
    let config = SurrealConfig {
        labels: vec!["Person".to_string(), "Talk".to_string()],
        ..SurrealConfig::default()
    };
    let query = surreal_delete_edge_query(
        &NodeId::new("person-1"),
        &Label::new("presents"),
        &NodeId::new("talk-1"),
        &config,
    )
    .unwrap();

    assert!(query.starts_with("DELETE `presents` WHERE"));
    assert!(query.contains("type::record(\"person\", \"person-1\")"));
    assert!(query.contains("type::record(\"talk\", \"talk-1\")"));
}

#[test]
fn get_node_query_scans_candidate_tables_in_one_statement() {
    let config = SurrealConfig {
        labels: vec!["Talk".to_string(), "Person".to_string()],
        ..SurrealConfig::default()
    };
    let query = surreal_get_node_query(&NodeId::new("talk-1"), &config).unwrap();

    assert!(query.starts_with("SELECT *, meta::tb(id) AS __grust_physical_label FROM "));
    assert!(query.contains("`person`, `record`, `talk`"));
    assert!(query.contains("id = type::record(\"talk\", \"talk-1\")"));
    assert_eq!(query.matches("SELECT").count(), 1);
}

#[test]
fn get_nodes_query_batches_candidate_records_in_one_statement() {
    let config = SurrealConfig {
        labels: vec!["Talk".to_string(), "Person".to_string()],
        ..SurrealConfig::default()
    };
    let query = surreal_get_nodes_query(&[NodeId::new("person-1"), NodeId::new("talk-1")], &config)
        .unwrap();

    assert!(query.starts_with("SELECT *, meta::tb(id) AS __grust_physical_label FROM "));
    assert!(query.contains("`person`, `record`, `talk`"));
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
    let query = surreal_get_edges_query(&EdgeQuery::default(), &config).unwrap();

    assert!(query.contains("FROM `member_of`, `presents`"));
    assert_eq!(query.matches("SELECT").count(), 1);
}

#[test]
fn get_edges_query_requires_relationship_config_for_generic_scan() {
    let err = surreal_get_edges_query(&EdgeQuery::default(), &SurrealConfig::default())
        .expect_err("generic Surreal edge reads require configured relationships");

    assert!(
        err.to_string()
            .contains("SurrealConfig.relationships is empty")
    );
}

#[test]
fn get_edges_query_accepts_explicit_label_without_relationship_config() {
    let query = surreal_get_edges_query(
        &EdgeQuery {
            label: Some(Label::new("presents")),
            ..EdgeQuery::default()
        },
        &SurrealConfig::default(),
    )
    .unwrap();

    assert!(query.contains("FROM `presents`"));
}

#[test]
fn surreal_response_parsers_rebuild_grust_values() {
    let string_id_node = surreal_node_from_value(serde_json::json!({
        "id": "city:`City:4`",
        "__grust_label": "City",
        "name": "Ada Lovelace"
    }))
    .unwrap();
    let typed_id_node = surreal_node_from_value(serde_json::json!({
        "id": {"id": {"String": "person-2"}},
        "__grust_label": "person",
        "score": 42,
        "active": true
    }))
    .unwrap();
    let sdk_string_id_node = surreal_node_from_value(serde_json::json!({
        "id": {"id": "`person-3`"},
        "__grust_label": "person",
        "name": "Grace Hopper"
    }))
    .unwrap();
    let legacy_node = surreal_node_from_value(serde_json::json!({
        "id": "city:`City:5`",
        "__grust_physical_label": "city"
    }))
    .unwrap();
    let edge = surreal_edge_from_value(serde_json::json!({
        "id": "presents:abc",
        "__grust_label": "presents",
        "in": "city:`City:4`",
        "out": {"id": "`talk-1`"},
        "relationship": "presents",
        "edge_id": "edge-1",
        "source": "schedule"
    }))
    .unwrap();

    assert_eq!(string_id_node.id, NodeId::new("City:4"));
    assert_eq!(string_id_node.label, Label::new("City"));
    assert_eq!(
        string_id_node.props.get("name"),
        Some(&Value::String("Ada Lovelace".to_string()))
    );
    assert_eq!(typed_id_node.id, NodeId::new("person-2"));
    assert_eq!(typed_id_node.props.get("score"), Some(&Value::Int(42)));
    assert_eq!(typed_id_node.props.get("active"), Some(&Value::Bool(true)));
    assert_eq!(sdk_string_id_node.id, NodeId::new("person-3"));
    assert_eq!(legacy_node.id, NodeId::new("City:5"));
    assert_eq!(legacy_node.label, Label::new("city"));
    assert!(!legacy_node.props.contains_key("__grust_physical_label"));
    assert_eq!(edge.from, NodeId::new("City:4"));
    assert_eq!(edge.to, NodeId::new("talk-1"));
    assert_eq!(edge.label, Label::new("presents"));
    assert_eq!(edge.id, Some(EdgeId::new("edge-1")));
    assert_eq!(
        edge.props.get("source"),
        Some(&Value::String("schedule".to_string()))
    );
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

    assert!(query.contains("DEFINE TABLE `person` SCHEMAFULL"));
    assert!(query.contains("DEFINE FIELD `__grust_label` ON TABLE `person` TYPE string"));
    assert!(query.contains("DEFINE FIELD `name` ON TABLE `person` TYPE string"));
    assert!(query.contains("DEFINE FIELD `age` ON TABLE `person` TYPE int"));
    assert!(query.contains("DEFINE TABLE `presents` TYPE RELATION SCHEMAFULL"));
    assert!(query.contains("DEFINE FIELD `relationship` ON TABLE `presents` TYPE string"));
    assert!(query.contains("DEFINE FIELD `edge_id` ON TABLE `presents` TYPE option<string>"));
    assert!(query.contains("DEFINE FIELD `source` ON TABLE `presents` TYPE string"));
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
    assert_eq!(fetched.label, Label::new("Talk"));
    assert_eq!(fetched.id, NodeId::new("talk-1"));

    let colon_id_node = Node::new("Talk", "Talk:4", Props::new());
    store
        .put_node(&colon_id_node)
        .await
        .expect("write colon-id node");
    let colon_id_fetched = store
        .get_node(&NodeId::new("Talk:4"))
        .await
        .expect("read colon-id node")
        .expect("colon-id node missing");
    assert_eq!(colon_id_fetched, colon_id_node);

    let result = store
        .traverse(Traversal::from_node("person-1").out("presents").to("Talk"))
        .await
        .expect("traverse");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id, NodeId::new("talk-1"));
}
