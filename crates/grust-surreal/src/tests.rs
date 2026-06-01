use super::*;

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

#[test]
fn upsert_nodes_preserves_arrays() {
    let graph = sample_graph();
    let query = surreal_upsert_nodes_query(&graph.nodes).unwrap();
    assert!(query.contains("UPSERT type::record(\"talk\", \"talk-1\")"));
    assert!(query.contains("tags = [\"rust\",\"graphs\"]"));
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
