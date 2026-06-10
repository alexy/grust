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
fn cypher_string_escapes_special_characters() {
    assert_eq!(cypher_string("a'b\\c\n"), "'a\\'b\\\\c\\n'");
}

#[test]
fn batch_queries_use_unwind_and_preserve_properties() {
    let graph = sample_graph();
    let config = FalkorConfig::default();
    let query = falkor_nodes_batch_query("Talk", &[&graph.nodes[0]], &config).unwrap();
    assert!(query.starts_with("UNWIND ["));
    assert!(query.contains("MERGE (n:Talk {id: row.id})"));
    assert!(query.contains("tags:['rust','graphs']"));

    let edge_query = falkor_edges_batch_query("PRESENTS", &[&graph.edges[0]], &config).unwrap();
    assert!(edge_query.contains("MERGE (a)-[r:PRESENTS]->(b)"));
}

#[test]
fn graph_schema_creates_falkor_indexes() {
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
            Vec::<Field>::new(),
        )
        .build();

    let queries = falkor_schema_queries(&schema, &FalkorConfig::default()).unwrap();

    assert!(queries.contains(&"CREATE INDEX ON :Person(id)".to_string()));
    assert!(queries.contains(&"CREATE INDEX ON :Person(name)".to_string()));
    assert!(queries.contains(&"CREATE INDEX ON :Person(age)".to_string()));
}
