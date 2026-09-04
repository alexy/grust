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
    person_props.insert("display name".to_string(), Value::from("Ada"));
    person_props.insert("sort-key".to_string(), Value::from(1_i64));

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
    let labels = falkor_labels(&graph.nodes[0], &config).unwrap();
    let query = falkor_nodes_batch_query(&labels, &[&graph.nodes[0]], &config).unwrap();
    assert!(query.starts_with("UNWIND ["));
    assert!(query.contains("MERGE (n:talk {`id`: row.id})"));
    assert!(query.contains("`tags`:['rust','graphs']"));
    assert!(!query.contains("props:{`id`:"));

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

    assert!(queries.contains(&"CREATE INDEX ON :person(`id`)".to_string()));
    assert!(queries.contains(&"CREATE INDEX ON :person(`name`)".to_string()));
    assert!(queries.contains(&"CREATE INDEX ON :person(`age`)".to_string()));
}

#[test]
fn graph_schema_rejects_falkor_identifier_collisions() {
    let label_collision = GraphSchema::builder()
        .node("a-b", Vec::new())
        .node("a_b", Vec::new())
        .build();
    let error = falkor_schema_queries(&label_collision, &FalkorConfig::default())
        .expect_err("colliding Falkor labels must fail");
    assert!(error.to_string().contains("node label identifier 'a_b'"));

    let reserved_field_collision = GraphSchema::builder()
        .node("Person", vec![Field::optional("id", FieldType::String)])
        .build();
    let error = falkor_schema_queries(&reserved_field_collision, &FalkorConfig::default())
        .expect_err("a field colliding with the structural id must fail");
    assert!(error.to_string().contains("`id`"));

    let relationship_collision = GraphSchema::builder()
        .edge("a-b", Vec::new(), Vec::new(), Vec::new())
        .edge("a_b", Vec::new(), Vec::new(), Vec::new())
        .build();
    let error = falkor_schema_queries(&relationship_collision, &FalkorConfig::default())
        .expect_err("colliding relationship types must fail");
    assert!(
        error
            .to_string()
            .contains("relationship type identifier 'A_B'")
    );

    let duplicate_field = GraphSchema::builder()
        .node(
            "Person",
            vec![
                Field::optional("name", FieldType::String),
                Field::optional("name", FieldType::String),
            ],
        )
        .build();
    assert!(falkor_schema_queries(&duplicate_field, &FalkorConfig::default()).is_err());
}

#[test]
fn property_identifiers_are_quoted_without_lossy_normalization() {
    assert_eq!(
        falkor_property_identifier("display name").unwrap(),
        "`display name`"
    );
    assert_eq!(
        falkor_property_identifier("external-id").unwrap(),
        "`external-id`"
    );

    for unsafe_name in ["", "tick`key", "line\nbreak", "nul\0key"] {
        assert!(falkor_property_identifier(unsafe_name).is_err());
    }

    let schema = GraphSchema::builder()
        .node(
            "Person",
            vec![
                Field::optional("given-name", FieldType::String),
                Field::optional("given_name", FieldType::String),
            ],
        )
        .build();
    let queries = falkor_schema_queries(&schema, &FalkorConfig::default()).unwrap();
    assert!(queries.contains(&"CREATE INDEX ON :person(`given-name`)".to_string()));
    assert!(queries.contains(&"CREATE INDEX ON :person(`given_name`)".to_string()));
}

#[test]
fn generated_maps_quote_keys_and_reject_identifier_injection() {
    let config = FalkorConfig::default();
    let mut props = Props::new();
    props.insert("display name".to_string(), Value::from("Ada"));
    props.insert("sort-key".to_string(), Value::from(7_i64));
    let node = Node::new("Person", "person-1", props);
    let query = falkor_node_query(&node, &config).unwrap();
    assert!(query.contains("`display name`:'Ada'"));
    assert!(query.contains("`sort-key`:7"));

    let mut hostile_props = Props::new();
    hostile_props.insert("x`:1}) DELETE n //".to_string(), Value::from("boom"));
    let hostile = Node::new("Person", "person-1", hostile_props);
    assert!(falkor_node_query(&hostile, &config).is_err());
}

#[test]
fn configured_id_is_quoted_and_cannot_be_overridden_by_props() {
    let config = FalkorConfig {
        id_property: "external id".to_string(),
        ..FalkorConfig::default()
    };
    let mut props = Props::new();
    props.insert("external id".to_string(), Value::from("attacker-value"));
    props.insert("name".to_string(), Value::from("Ada"));
    let node = Node::new("Person", "canonical-id", props);

    let query = falkor_node_query(&node, &config).unwrap();
    assert!(query.contains("{`external id`:'canonical-id'}"));
    assert!(query.contains("`name`:'Ada'"), "{query}");
    assert!(!query.contains("attacker-value"));

    let hostile = FalkorConfig {
        id_property: "id`}) MATCH (victim) DETACH DELETE victim //".to_string(),
        ..FalkorConfig::default()
    };
    assert!(falkor_node_query(&node, &hostile).is_err());
}

#[test]
fn pool_error_does_not_render_connection_credentials() {
    let rendered = falkor_pool_error().to_string();
    assert!(!rendered.contains("redis://"));
    assert!(!rendered.contains("password"));
    assert_eq!(
        rendered,
        "backend error: failed to acquire FalkorDB Redis connection from pool"
    );
}

#[test]
fn put_graph_preflights_every_property_before_connecting() {
    let config = FalkorConfig {
        redis_url: "redis://user:supersecret@127.0.0.1:1".to_string(),
        batch_size: 1,
        ..FalkorConfig::default()
    };
    let store = FalkorGraphStore::new(config);
    let valid = Node::new("Person", "person-1", Props::new());
    let mut invalid_props = Props::new();
    invalid_props.insert("late`injection".to_string(), Value::from("boom"));
    let invalid = Node::new("Person", "person-2", invalid_props);
    let graph = Graph::new(vec![valid, invalid], Vec::new());

    let error = futures_executor::block_on(store.put_graph(&graph))
        .expect_err("late unsafe property must fail during preflight");
    let rendered = error.to_string();
    assert!(rendered.contains("unsafe FalkorDB property identifier"));
    assert!(!rendered.contains("supersecret"));
    assert!(!rendered.contains("connection"));
}

#[test]
fn graph_preflight_rejects_lossy_label_collisions_but_allows_repeats() {
    let config = FalkorConfig::default();
    let repeated = Graph::new(
        vec![
            Node::new("Person", "person-1", Props::new()),
            Node::new("Person", "person-2", Props::new()),
        ],
        vec![
            Edge::new("knows", "person-1", "person-2", Props::new()),
            Edge::new("knows", "person-2", "person-1", Props::new()),
        ],
    );
    validate_falkor_graph(&repeated, &config).unwrap();

    let node_collision = Graph::new(
        vec![
            Node::new("a-b", "node-1", Props::new()),
            Node::new("a_b", "node-2", Props::new()),
        ],
        Vec::new(),
    );
    assert!(validate_falkor_graph(&node_collision, &config).is_err());

    let edge_collision = Graph::new(
        repeated.nodes,
        vec![
            Edge::new("a-b", "person-1", "person-2", Props::new()),
            Edge::new("a_b", "person-2", "person-1", Props::new()),
        ],
    );
    assert!(validate_falkor_graph(&edge_collision, &config).is_err());
}

#[test]
fn query_errors_do_not_echo_cypher_or_property_values() {
    let rendered = falkor_query_error().to_string();
    for marker in ["MATCH", "secret-value", "DELETE", "redis://"] {
        assert!(!rendered.contains(marker));
    }
    assert_eq!(rendered, "backend error: FalkorDB query failed");
}

#[test]
fn store_uses_configured_connection_pool_size() {
    let store = FalkorGraphStore::new(FalkorConfig {
        pool_size: 3,
        ..FalkorConfig::default()
    });

    assert_eq!(store.pool.max_size(), 3);
}

#[test]
#[ignore = "requires a live FalkorDB server on 127.0.0.1:6379"]
fn live_put_graph_and_schema() {
    futures_executor::block_on(async {
        let store = FalkorGraphStore::new(FalkorConfig {
            graph: "grust_integration".to_string(),
            ..FalkorConfig::default()
        });
        store
            .clear()
            .await
            .expect("clear FalkorDB integration graph");
        store
            .apply_schema(
                &GraphSchema::builder()
                    .node(
                        "Person",
                        vec![
                            Field::required("name", FieldType::String),
                            Field::optional("display name", FieldType::String),
                            Field::optional("sort-key", FieldType::Int),
                        ],
                    )
                    .node("Talk", vec![Field::required("title", FieldType::String)])
                    .edge(
                        "presents",
                        vec![Label::new("Person")],
                        vec![Label::new("Talk")],
                        Vec::<Field>::new(),
                    )
                    .build(),
            )
            .await
            .expect("apply FalkorDB schema");
        let report = store
            .put_graph(&sample_graph())
            .await
            .expect("write graph to FalkorDB");
        assert_eq!(report.nodes, 2);
        assert_eq!(report.edges, 1);
    });
}
