use super::*;

fn sample_graph() -> Graph {
    let mut builder = Graph::builder();
    let _ = builder
        .node("Person", "person-1")
        .prop("name", "Ada")
        .finish();
    let _ = builder
        .node("Talk", "talk-1")
        .prop("title", "Analytical Engine")
        .finish();
    let _ = builder
        .edge("PRESENTS", "person-1", "talk-1")
        .prop("source", "schedule")
        .finish();
    builder.build()
}

#[test]
fn bootstrap_defines_native_property_graph() {
    let config = PostgresPgqConfig::default();
    let sql = pgq_bootstrap_sql(
        &config,
        "\"public\".\"grust_nodes\"",
        "\"public\".\"grust_edges\"",
    )
    .unwrap();

    assert!(sql.contains("DROP PROPERTY GRAPH IF EXISTS \"public\".\"grust_graph\""));
    assert!(sql.contains("CREATE PROPERTY GRAPH \"public\".\"grust_graph\""));
    assert!(sql.contains("\"public\".\"grust_nodes\" AS grust_nodes"));
    assert!(sql.contains("LABEL grust_node"));
    assert!(sql.contains("PROPERTIES (id, label, props::text AS props)"));
    assert!(sql.contains("\"public\".\"grust_edges\" AS grust_edges"));
    assert!(sql.contains("SOURCE KEY (from_id) REFERENCES grust_nodes (id)"));
    assert!(sql.contains("DESTINATION KEY (to_id) REFERENCES grust_nodes (id)"));
    assert!(sql.contains("LABEL grust_edge"));
}

#[test]
fn pgq_traversal_builds_graph_table_query() {
    let traversal = Traversal::from_node("person-1")
        .out("PRESENTS")
        .to("Talk")
        .limit(10);

    let sql = pgq_traversal_sql(
        "\"public\".\"grust_graph\"",
        "\"public\".\"grust_nodes\"",
        &traversal,
    )
    .unwrap();

    assert!(sql.contains("FROM GRAPH_TABLE (\"public\".\"grust_graph\" MATCH"));
    assert!(sql.contains("(n0 IS grust_node WHERE n0.id = 'person-1')"));
    assert!(sql.contains("-[e0 IS grust_edge WHERE e0.label = 'PRESENTS']->"));
    assert!(sql.contains("(n1 IS grust_node WHERE n1.label = 'Talk')"));
    assert!(sql.contains("COLUMNS (n1.id AS target_id)"));
    assert!(sql.contains("JOIN \"public\".\"grust_nodes\" n ON n.id = gt.target_id"));
    assert!(sql.ends_with(" LIMIT 10"));
}

#[test]
fn pgq_traversal_builds_property_start() {
    let traversal = Traversal {
        start: Start::NodesByProperty {
            label: Label::new("Person"),
            key: "name".to_string(),
            value: Value::from("Ada"),
        },
        steps: Vec::new(),
        limit: None,
    };

    let sql = pgq_traversal_sql(
        "\"public\".\"grust_graph\"",
        "\"public\".\"grust_nodes\"",
        &traversal,
    )
    .unwrap();

    assert!(sql.contains("n0.label = 'Person'"));
    assert!(sql.contains("n0.props::jsonb #>> ARRAY['name', 'value'] = 'Ada'"));
}

#[test]
fn pgq_bootstrap_validates_graph_name_boundaries() {
    let mut config = PostgresPgqConfig {
        graph_name: "g".repeat(63),
        ..PostgresPgqConfig::default()
    };
    pgq_bootstrap_sql(
        &config,
        "\"public\".\"grust_nodes\"",
        "\"public\".\"grust_edges\"",
    )
    .expect("63-byte graph name should be accepted");

    config.graph_name = "g".repeat(64);
    let error = pgq_bootstrap_sql(
        &config,
        "\"public\".\"grust_nodes\"",
        "\"public\".\"grust_edges\"",
    )
    .expect_err("64-byte graph name must be rejected");
    assert!(error.to_string().contains("is 64 bytes"));
    assert!(error.to_string().contains("limit is 63 bytes"));
}

#[test]
fn pgq_bootstrap_validates_schema_and_derived_prefix_names() {
    let mut config = PostgresPgqConfig {
        schema: "s".repeat(63),
        table_prefix: "p".repeat(47),
        ..PostgresPgqConfig::default()
    };
    pgq_bootstrap_sql(&config, "\"schema\".\"nodes\"", "\"schema\".\"edges\"")
        .expect("valid PostgreSQL config should reach PGQ rendering");

    config.schema = "s".repeat(64);
    assert!(
        pgq_bootstrap_sql(&config, "\"schema\".\"nodes\"", "\"schema\".\"edges\"",).is_err(),
        "PGQ bootstrap must apply the PostgreSQL schema-name limit"
    );

    config.schema = "public".to_string();
    config.table_prefix = "p".repeat(48);
    let error = pgq_bootstrap_sql(&config, "\"public\".\"nodes\"", "\"public\".\"edges\"")
        .expect_err("PGQ bootstrap must validate derived universal identifiers");
    assert!(error.to_string().contains("_nodes_label_idx"));
    assert!(error.to_string().contains("is 64 bytes"));
}

#[tokio::test]
#[ignore = "requires PostgreSQL 19 with SQL/PGQ"]
async fn live_invalid_typed_schema_is_rejected_before_pgq_bootstrap_ddl() {
    let connection_string = std::env::var("POSTGRES_PGQ_TEST_CONNECTION_STRING")
        .unwrap_or_else(|_| "host=127.0.0.1 port=5419 user=alexy dbname=postgres".to_string());
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock should be after Unix epoch")
        .as_nanos();
    let schema_name = format!("grust_pgq_invalid_{}_{nonce}", std::process::id());
    let store = PostgresPgqStore::connect(PostgresPgqConfig {
        connection_string,
        schema: schema_name.clone(),
        graph_name: "grust_invalid_graph".to_string(),
        ..PostgresPgqConfig::default()
    })
    .await
    .expect("connect PostgreSQL PGQ store");

    let invalid = GraphSchema::builder()
        .edge(
            "RELATES",
            Vec::<Label>::new(),
            Vec::<Label>::new(),
            vec![Field::required("from_id", FieldType::String)],
        )
        .build();
    store
        .apply_schema(&invalid)
        .await
        .expect_err("fixed-column collision must fail before PGQ bootstrap");

    let row = store
        .client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1)",
            &[&schema_name],
        )
        .await
        .expect("check whether invalid PGQ apply created a schema");
    let schema_exists: bool = row.get(0);
    if schema_exists {
        store
            .execute(&format!(
                "DROP SCHEMA {} CASCADE",
                quote_ident(&schema_name)
            ))
            .await
            .expect("clean up unexpectedly created schema");
    }
    assert!(
        !schema_exists,
        "invalid typed schema must not execute PGQ bootstrap DDL"
    );
}

#[tokio::test]
#[ignore = "requires PostgreSQL 19 with SQL/PGQ"]
async fn live_postgres_pgq_put_read_traverse_and_schema() {
    let connection_string = std::env::var("POSTGRES_PGQ_TEST_CONNECTION_STRING")
        .unwrap_or_else(|_| "host=127.0.0.1 port=5419 user=alexy dbname=postgres".to_string());
    let suffix = std::process::id();
    let store = PostgresPgqStore::connect(PostgresPgqConfig {
        connection_string,
        schema: format!("grust_pgq_integration_{suffix}"),
        table_prefix: "grust_live".to_string(),
        graph_name: "grust_live_graph".to_string(),
        batch_size: 2,
    })
    .await
    .expect("connect PostgreSQL PGQ store");

    store.bootstrap().await.expect("bootstrap PGQ tables");
    store.clear().await.expect("clear PGQ tables");
    store
        .apply_schema(
            &GraphSchema::builder()
                .node("Person", vec![Field::required("name", FieldType::String)])
                .node("Talk", vec![Field::required("title", FieldType::String)])
                .edge(
                    "PRESENTS",
                    vec![Label::new("Person")],
                    vec![Label::new("Talk")],
                    vec![Field::required("source", FieldType::String)],
                )
                .build(),
        )
        .await
        .expect("apply PGQ schema");

    let graph = sample_graph();
    let report = store.put_graph(&graph).await.expect("write graph");
    assert_eq!(report.nodes, 2);
    assert_eq!(report.edges, 1);

    let talks = store
        .traverse(Traversal::from_node("person-1").out("PRESENTS").to("Talk"))
        .await
        .expect("PGQ traverse");
    assert_eq!(talks.len(), 1);
    assert_eq!(talks[0].id, NodeId::new("talk-1"));

    store
        .execute(&format!(
            "DROP SCHEMA {} CASCADE",
            quote_ident(&store.config.schema)
        ))
        .await
        .expect("drop integration schema");
}
