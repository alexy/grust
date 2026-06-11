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
fn helix_http_node_batch_omits_arrays() {
    let graph = sample_graph();
    let request = helix_add_nodes_request(std::slice::from_ref(&graph.nodes[0])).unwrap();
    let properties = &request["query"]["queries"][0]["Query"]["steps"][0]["AddN"]["properties"];
    assert!(
        !properties
            .as_array()
            .unwrap()
            .iter()
            .any(|prop| prop[0] == "tags")
    );
}

#[test]
fn helix_http_edge_batch_uses_target_variable() {
    let graph = sample_graph();
    let request = helix_add_edges_request(std::slice::from_ref(&graph.edges[0])).unwrap();
    assert_eq!(
        request["query"]["queries"][1]["Query"]["steps"][1]["AddE"]["to"]["Var"],
        "target_0"
    );
}

#[test]
fn helix_edge_writes_store_grust_edge_metadata() {
    let graph = sample_graph();
    let request = helix_add_edges_request(std::slice::from_ref(&graph.edges[0])).unwrap();
    let properties = &request["query"]["queries"][1]["Query"]["steps"][1]["AddE"]["properties"];

    assert!(
        properties
            .as_array()
            .unwrap()
            .iter()
            .any(|prop| { prop[0] == "relationship" && prop[1]["Value"]["String"] == "presents" })
    );
    assert!(
        properties
            .as_array()
            .unwrap()
            .iter()
            .any(|prop| { prop[0] == "from_id" && prop[1]["Value"]["String"] == "person-1" })
    );
    assert!(
        properties
            .as_array()
            .unwrap()
            .iter()
            .any(|prop| { prop[0] == "to_id" && prop[1]["Value"]["String"] == "talk-1" })
    );
}

#[test]
fn helix_get_edges_request_filters_by_grust_metadata() {
    let request = helix_get_edges_request(&EdgeQuery {
        from: Some(NodeId::new("person-1")),
        to: Some(NodeId::new("talk-1")),
        label: Some(Label::new("presents")),
    })
    .unwrap();
    let predicate = &request["query"]["queries"][0]["Query"]["steps"][0]["EWhere"];

    assert_eq!(request["request_type"], "read");
    assert!(
        predicate["And"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["Eq"][0] == "from_id" && item["Eq"][1]["String"] == "person-1" })
    );
    assert!(
        predicate["And"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["Eq"][0] == "to_id" && item["Eq"][1]["String"] == "talk-1" })
    );
    assert!(
        predicate["And"].as_array().unwrap().iter().any(|item| {
            item["Eq"][0] == "relationship" && item["Eq"][1]["String"] == "presents"
        })
    );
}

#[test]
fn helix_traversal_request_lowers_to_direction_steps() {
    let request = helix_traversal_request(
        &Traversal::from_node("person-1")
            .out("presents")
            .to("Talk")
            .limit(5),
    )
    .unwrap();
    let steps = request["query"]["queries"][0]["Query"]["steps"]
        .as_array()
        .unwrap();

    assert_eq!(request["request_type"], "read");
    assert_eq!(steps[0]["NWhere"]["Eq"][0], "id");
    assert_eq!(steps[0]["NWhere"]["Eq"][1]["String"], "person-1");
    assert_eq!(steps[1]["Out"], "PRESENTS");
    assert_eq!(steps[2]["HasLabel"], "Talk");
}

#[test]
fn helix_response_parsers_rebuild_grust_values() {
    let response = serde_json::json!({
        "nodes": [{"id": "person-1", "$label": "Person", "name": "Ada Lovelace"}],
        "edges": [{
            "from_id": "person-1",
            "to_id": "talk-1",
            "relationship": "presents",
            "source": "schedule"
        }]
    });

    let nodes = helix_nodes_from_response(&response, "nodes").unwrap();
    let edges = helix_edges_from_response(&response, "edges").unwrap();

    assert_eq!(nodes[0].id, NodeId::new("person-1"));
    assert_eq!(nodes[0].label, Label::new("Person"));
    assert_eq!(edges[0].from, NodeId::new("person-1"));
    assert_eq!(edges[0].to, NodeId::new("talk-1"));
    assert_eq!(edges[0].label, Label::new("presents"));
}

#[test]
fn strips_v1_query_for_sdk_base_url() {
    assert_eq!(
        helix_base_url("http://127.0.0.1:8080/v1/query"),
        "http://127.0.0.1:8080"
    );
}

#[test]
fn graph_schema_is_validated_for_dynamic_helix_names() {
    let schema = GraphSchema::builder()
        .node("Person", vec![Field::required("name", FieldType::String)])
        .edge(
            "presents",
            vec![Label::new("Person")],
            vec![Label::new("Talk")],
            Vec::<Field>::new(),
        )
        .build();

    validate_helix_schema(&schema).unwrap();

    let bad = GraphSchema::builder()
        .node(
            "Person",
            vec![Field::required("display-name", FieldType::String)],
        )
        .build();

    assert!(validate_helix_schema(&bad).is_err());
}
