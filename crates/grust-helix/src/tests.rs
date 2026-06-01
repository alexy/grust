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
fn strips_v1_query_for_sdk_base_url() {
    assert_eq!(
        helix_base_url("http://127.0.0.1:8080/v1/query"),
        "http://127.0.0.1:8080"
    );
}
