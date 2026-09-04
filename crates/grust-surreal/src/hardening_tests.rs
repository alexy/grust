use std::collections::BTreeMap;

use super::*;

#[test]
fn property_keys_are_losslessly_quoted_in_writes_and_predicates() {
    let malicious = "name`; DELETE record; --";
    let node = Node::new(
        "Person",
        "person-1",
        Props::from([
            ("display-name".to_string(), Value::from("Ada")),
            ("1st".to_string(), Value::from(true)),
            ("select".to_string(), Value::from("reserved")),
            (malicious.to_string(), Value::from("literal")),
        ]),
    );
    let query = surreal_upsert_nodes_query(&[node]).unwrap();

    assert!(query.contains("`display-name` = \"Ada\""));
    assert!(query.contains("`1st` = true"));
    assert!(query.contains("`select` = \"reserved\""));
    assert!(query.contains("`name\\`; DELETE record; --` = \"literal\""));

    let predicate = surreal_start_nodes_query(
        &Start::NodesByProperty {
            label: Label::new("Person"),
            key: malicious.to_string(),
            value: Value::from("literal"),
        },
        &SurrealConfig::default(),
    )
    .unwrap();
    assert!(predicate.contains("`name\\`; DELETE record; --` = \"literal\""));
}

#[test]
fn runtime_writes_reject_reserved_storage_fields() {
    for key in ["id", "ID", "labels", "__grust_label"] {
        let node = Node::new(
            "Person",
            "person-1",
            Props::from([(key.to_string(), Value::from("claimed"))]),
        );
        let error = surreal_upsert_nodes_query(&[node])
            .expect_err("node storage fields must not be assignable as properties");
        assert!(error.to_string().contains("reserved storage field"));
    }

    for key in [
        "id",
        "in",
        "out",
        "relationship",
        "edge_id",
        "__grust_label",
    ] {
        let edge = Edge::new(
            "presents",
            "person-1",
            "talk-1",
            Props::from([(key.to_string(), Value::from("claimed"))]),
        );
        let error =
            surreal_relate_edges_query(&[edge], &BTreeMap::new(), &SurrealConfig::default())
                .expect_err("edge storage fields must not be assignable as properties");
        assert!(error.to_string().contains("reserved storage field"));
    }
}

#[test]
fn schema_rejects_table_collisions_and_fixed_or_duplicate_fields() {
    let table_collision = GraphSchema::builder()
        .node("Person-Role", Vec::new())
        .node("Person_Role", Vec::new())
        .build();
    let error = surreal_schema_query(&table_collision)
        .expect_err("normalized table aliases must be unique");
    assert!(error.to_string().contains("table 'person_role'"));

    let config = SurrealConfig {
        labels: vec!["Configured-Label".to_string()],
        ..SurrealConfig::default()
    };
    let cross_source_collision = GraphSchema::builder()
        .node("Configured_Label", Vec::new())
        .build();
    assert!(
        validate_schema_for_config(&config, &cross_source_collision).is_err(),
        "config and schema claims must share one physical table namespace"
    );

    let base_collision = GraphSchema::builder().node("record", Vec::new()).build();
    assert!(
        surreal_schema_query(&base_collision).is_err(),
        "schema node tables must not alias the fixed fallback table"
    );

    let cross_kind_collision = GraphSchema::builder()
        .node("Membership", Vec::new())
        .edge(
            "membership",
            Vec::<Label>::new(),
            Vec::<Label>::new(),
            Vec::new(),
        )
        .build();
    assert!(
        surreal_schema_query(&cross_kind_collision).is_err(),
        "node and relationship tables share one physical namespace"
    );

    let configured_relationship = SurrealConfig {
        relationships: vec!["membership".to_string()],
        ..SurrealConfig::default()
    };
    let schema_node = GraphSchema::builder()
        .node("Membership", Vec::new())
        .build();
    assert!(validate_schema_for_config(&configured_relationship, &schema_node).is_err());

    let node_fixed = GraphSchema::builder()
        .node("Person", vec![Field::required("id", FieldType::String)])
        .build();
    assert!(surreal_schema_query(&node_fixed).is_err());

    let labels_fixed = GraphSchema::builder()
        .node("Person", vec![Field::optional("labels", FieldType::String)])
        .build();
    assert!(surreal_schema_query(&labels_fixed).is_err());

    for field in [
        "id",
        "in",
        "out",
        "relationship",
        "edge_id",
        "__grust_label",
    ] {
        let edge_fixed = GraphSchema::builder()
            .edge(
                "presents",
                Vec::<Label>::new(),
                Vec::<Label>::new(),
                vec![Field::optional(field, FieldType::String)],
            )
            .build();
        assert!(surreal_schema_query(&edge_fixed).is_err(), "{field}");
    }

    let duplicate = GraphSchema::builder()
        .node(
            "Person",
            vec![
                Field::required("display-name", FieldType::String),
                Field::optional("display-name", FieldType::String),
            ],
        )
        .build();
    assert!(surreal_schema_query(&duplicate).is_err());
}

#[test]
fn inferred_endpoint_tables_share_the_configured_physical_namespace() {
    let config = SurrealConfig {
        relationships: vec!["membership".to_string()],
        ..SurrealConfig::default()
    };
    let edge = Edge::new("membership", "membership:1", "person:2", Props::new());

    let error = validate_edge_write(&config, &edge)
        .expect_err("an inferred node table must not alias a relationship table");
    assert!(error.to_string().contains("table 'membership'"));
    assert!(
        surreal_relate_edges_query(std::slice::from_ref(&edge), &BTreeMap::new(), &config).is_err(),
        "the renderer must preserve the endpoint preflight boundary"
    );

    let base_endpoint = Edge::new("membership", "record:1", "person:2", Props::new());
    assert!(
        validate_edge_write(&config, &base_endpoint).is_err(),
        "an explicit prefix must not alias the fixed fallback table"
    );

    let same_kind_alias = Edge::new("membership", "person-role:1", "Person_Role:2", Props::new());
    validate_edge_write(
        &SurrealConfig {
            labels: vec!["Person-Role".to_string()],
            relationships: vec!["membership".to_string()],
            ..SurrealConfig::default()
        },
        &same_kind_alias,
    )
    .expect("equivalent inferred node tables remain backward-compatible");
}

#[test]
fn graph_endpoints_use_known_node_tables_and_validate_unknown_prefixes() {
    let config = SurrealConfig {
        relationships: vec!["membership".to_string()],
        ..SurrealConfig::default()
    };
    let known_graph = Graph {
        nodes: vec![
            Node::new("Person", "membership:1", Props::new()),
            Node::new("Person", "person:2", Props::new()),
        ],
        edges: vec![Edge::new(
            "membership",
            "membership:1",
            "person:2",
            Props::new(),
        )],
    };
    validate_graph_write(&config, &known_graph)
        .expect("a graph-local node label, not its ID prefix, chooses its table");
    let id_tables = surreal_id_tables(&known_graph.nodes).unwrap();
    let query = surreal_relate_edges_query(&known_graph.edges, &id_tables, &config).unwrap();
    assert!(query.contains("type::record(\"person\", \"membership:1\")"));

    let unknown_endpoint_graph = Graph {
        nodes: Vec::new(),
        edges: vec![Edge::new(
            "membership",
            "membership:1",
            "person:2",
            Props::new(),
        )],
    };
    assert!(
        validate_graph_write(&config, &unknown_endpoint_graph).is_err(),
        "unknown endpoints fall back to their prefixes and must be claimed"
    );
}

#[test]
fn read_delete_and_mutation_helpers_reject_inferred_table_aliases() {
    let config = SurrealConfig {
        relationships: vec!["membership".to_string()],
        ..SurrealConfig::default()
    };
    let colliding = NodeId::new("membership:1");
    assert!(surreal_get_node_query(&colliding, &config).is_err());
    assert!(surreal_delete_node_query(&colliding, &config).is_err());
    assert!(
        surreal_patch_node_query(
            &colliding,
            &Props::from([("name".to_string(), Value::from("Ada"))]),
            &config,
        )
        .is_err()
    );
    assert!(
        surreal_delete_edge_query(
            &colliding,
            &Label::new("membership"),
            &NodeId::new("person:2"),
            &config,
        )
        .is_err()
    );
    assert!(
        surreal_apply_mutations_query(
            &[GraphMutation::UpsertEdge(Edge::new(
                "membership",
                "membership:1",
                "person:2",
                Props::new(),
            ))],
            &config,
        )
        .is_err()
    );

    surreal_get_nodes_query(
        &[NodeId::new("person-role:1"), NodeId::new("person_role:2")],
        &SurrealConfig::default(),
    )
    .expect("same-kind endpoint aliases do not create a table-type conflict");
    assert!(
        surreal_start_nodes_query(&Start::NodesByLabel(Label::new("membership")), &config).is_err()
    );
    assert!(
        surreal_get_edges_query(
            &EdgeQuery {
                label: Some(Label::new("membership")),
                ..EdgeQuery::default()
            },
            &SurrealConfig {
                labels: vec!["Membership".to_string()],
                ..SurrealConfig::default()
            },
        )
        .is_err()
    );
}

#[test]
fn leading_digit_and_reserved_table_names_are_quoted() {
    let schema = GraphSchema::builder()
        .node("123", Vec::new())
        .node("select", Vec::new())
        .build();
    let query = surreal_schema_query(&schema).unwrap();
    assert!(query.contains("DEFINE TABLE `123` SCHEMAFULL"));
    assert!(query.contains("DEFINE TABLE `select` SCHEMAFULL"));
}

#[test]
fn namespace_and_database_identifiers_are_quoted_and_validated_on_connect() {
    let config = SurrealConfig {
        namespace: "select".to_string(),
        database: "1database; DROP DATABASE graph".to_string(),
        ..SurrealConfig::default()
    };
    SurrealHttpGraphStore::connect(config.clone()).expect("quoted scope names should be safe");
    let query = surreal_bootstrap_query(&config).unwrap();
    assert!(query.contains("DEFINE NAMESPACE `select`"));
    assert!(query.contains("DEFINE DATABASE `1database; DROP DATABASE graph`"));

    for (namespace, database) in [("", "graph"), ("test", "bad\nname")] {
        let error = SurrealHttpGraphStore::connect(SurrealConfig {
            namespace: namespace.to_string(),
            database: database.to_string(),
            ..SurrealConfig::default()
        })
        .expect_err("invalid scopes must fail during connect validation");
        assert!(matches!(error, GrustError::Schema(_)));
    }
}

#[tokio::test]
async fn invalid_schema_and_late_graph_property_fail_before_http_network_io() {
    let store = SurrealHttpGraphStore::connect(SurrealConfig {
        url: "http://127.0.0.1:1/sql".to_string(),
        batch_size: 1,
        ..SurrealConfig::default()
    })
    .unwrap();

    let invalid_schema = GraphSchema::builder()
        .node("Person", vec![Field::required("id", FieldType::String)])
        .build();
    let error = store
        .apply_schema(&invalid_schema)
        .await
        .expect_err("schema validation must precede bootstrap HTTP I/O");
    assert!(matches!(error, GrustError::Schema(_)));

    let graph = Graph {
        nodes: vec![
            Node::new(
                "Person",
                "person-1",
                Props::from([("name".to_string(), Value::from("Ada"))]),
            ),
            Node::new(
                "Person",
                "person-2",
                Props::from([("id".to_string(), Value::from("override"))]),
            ),
        ],
        edges: Vec::new(),
    };
    let error = store
        .put_graph(&graph)
        .await
        .expect_err("the entire graph must validate before its first HTTP batch");
    assert!(matches!(error, GrustError::Schema(_)));
}

#[tokio::test]
async fn inferred_endpoint_collisions_fail_before_http_network_io() {
    let store = SurrealHttpGraphStore::connect(SurrealConfig {
        url: "http://127.0.0.1:1/sql".to_string(),
        relationships: vec!["membership".to_string()],
        ..SurrealConfig::default()
    })
    .unwrap();
    let edge = Edge::new("membership", "membership:1", "person:2", Props::new());

    for error in [
        store.put_edge(&edge).await.expect_err("put_edge preflight"),
        store
            .put_graph(&Graph {
                nodes: Vec::new(),
                edges: vec![edge.clone()],
            })
            .await
            .expect_err("put_graph preflight"),
        store
            .get_node(&NodeId::new("membership:1"))
            .await
            .expect_err("get_node preflight"),
        store
            .apply_mutations(&[GraphMutation::UpsertEdge(edge)])
            .await
            .expect_err("mutation preflight"),
    ] {
        assert!(matches!(error, GrustError::Schema(_)), "{error}");
    }
}

#[tokio::test]
async fn sdk_connect_validates_config_before_network_io() {
    let error = SurrealSdkGraphStore::connect(SurrealConfig {
        url: "http://127.0.0.1:1/sql".to_string(),
        namespace: "bad\nnamespace".to_string(),
        ..SurrealConfig::default()
    })
    .await
    .expect_err("invalid namespace must be rejected before the WebSocket connection");
    assert!(matches!(error, GrustError::Schema(_)));
}

#[test]
fn invalid_url_errors_do_not_echo_embedded_secrets() {
    let error = surreal_ws_address("http://alice:supersecret@[invalid/?token=hunter2")
        .expect_err("malformed URL should fail validation");
    let message = error.to_string();
    for secret in ["alice", "supersecret", "hunter2"] {
        assert!(!message.contains(secret), "leaked {secret}: {message}");
    }
}
