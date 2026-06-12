use super::*;

fn sample_graph() -> Graph {
    let mut builder = Graph::builder();
    builder
        .node("Person", "person-1")
        .prop("name", "Ada")
        .finish();
    builder
        .node("Talk", "talk-1")
        .prop("title", "Analytical Engine")
        .finish();
    builder
        .edge("PRESENTS", "person-1", "talk-1")
        .prop("source", "schedule")
        .finish();
    builder.build()
}

#[test]
fn bootstrap_registers_pggraph_projection() {
    let config = PgGraphConfig::default();
    let sql = bootstrap_sql(
        &config,
        "\"public\".\"grust_nodes\"",
        "\"public\".\"grust_edges\"",
    )
    .unwrap();

    assert!(sql.contains("CREATE EXTENSION IF NOT EXISTS graph"));
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"public\".\"grust_nodes\""));
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"public\".\"grust_edges\""));
    assert!(sql.contains("SELECT graph.add_table('public.grust_nodes'::regclass, 'id'"));
    assert!(sql.contains("SELECT graph.add_edge("));
    assert!(sql.contains("'from_id'"));
    assert!(sql.contains("'to_id'"));
    assert!(sql.contains("'label'"));
}

#[test]
fn graph_schema_creates_typed_views_and_indexes() {
    let config = PgGraphConfig::default();
    let schema = GraphSchema::builder()
        .node(
            "Person",
            vec![
                Field::required("name", FieldType::String),
                Field::optional("age", FieldType::Int),
            ],
        )
        .edge(
            "WORKS_ON",
            vec![Label::new("Person")],
            vec![Label::new("Project")],
            vec![Field::required("since", FieldType::Int)],
        )
        .build();

    let sql = pggraph_schema_sql(
        &config,
        "\"public\".\"grust_nodes\"",
        "\"public\".\"grust_edges\"",
        &schema,
    )
    .unwrap();

    assert!(sql.contains("CREATE OR REPLACE VIEW \"public\".\"grust_node_person\""));
    assert!(sql.contains("props #>> ARRAY['name', 'value'] AS \"name\""));
    assert!(sql.contains("(props #>> ARRAY['age', 'value'])::bigint AS \"age\""));
    assert!(sql.contains("CREATE OR REPLACE VIEW \"public\".\"grust_edge_works_on\""));
    assert!(sql.contains("\"grust_node_person_age_idx\""));
    assert!(sql.contains("\"grust_edge_works_on_since_idx\""));
}

#[test]
fn node_upsert_uses_jsonb_and_conflict_update() {
    let graph = sample_graph();
    let sql = upsert_nodes_sql("\"public\".\"grust_nodes\"", &graph.nodes).unwrap();

    assert!(sql.contains("INSERT INTO \"public\".\"grust_nodes\""));
    assert!(sql.contains("::jsonb"));
    assert!(sql.contains("ON CONFLICT (id) DO UPDATE"));
    assert!(sql.contains("'person-1'"));
    assert!(sql.contains("'Talk'"));
}

#[test]
fn edge_upsert_uses_grust_edge_identity() {
    let graph = sample_graph();
    let sql = upsert_edges_sql("\"public\".\"grust_edges\"", &graph.edges).unwrap();

    assert!(sql.contains("INSERT INTO \"public\".\"grust_edges\""));
    assert!(sql.contains("(id, from_id, to_id, label, props)"));
    assert!(sql.contains("ON CONFLICT (from_id, label, to_id) DO UPDATE"));
    assert!(sql.contains("'person-1'"));
    assert!(sql.contains("'talk-1'"));
    assert!(sql.contains("'PRESENTS'"));
}

#[test]
fn traversal_sql_builds_exact_out_step() {
    let traversal = Traversal::from_node("person-1")
        .out("PRESENTS")
        .to("Talk")
        .limit(10);

    let sql = traversal_sql(
        "\"public\".\"grust_nodes\"",
        "\"public\".\"grust_edges\"",
        &traversal,
    )
    .unwrap();

    assert!(sql.contains("JOIN \"public\".\"grust_edges\" e0 ON e0.from_id = n0.id"));
    assert!(sql.contains("AND e0.label = 'PRESENTS'"));
    assert!(sql.contains("JOIN \"public\".\"grust_nodes\" n1 ON n1.id = e0.to_id"));
    assert!(sql.contains("AND n1.label = 'Talk'"));
    assert!(sql.contains("WHERE n0.id = 'person-1'"));
    assert!(sql.contains("LIMIT 10"));
}

#[test]
fn traversal_sql_builds_property_start() {
    let traversal = Traversal {
        start: Start::NodesByProperty {
            label: Label::new("Person"),
            key: "name".to_string(),
            value: Value::from("Ada"),
        },
        steps: Vec::new(),
        limit: None,
    };

    let sql = traversal_sql(
        "\"public\".\"grust_nodes\"",
        "\"public\".\"grust_edges\"",
        &traversal,
    )
    .unwrap();

    assert!(sql.contains("WHERE n0.label = 'Person'"));
    assert!(sql.contains("n0.props #>> ARRAY['name', 'value'] = 'Ada'"));
}

#[test]
fn rejects_invalid_config_identifiers() {
    assert!(validate_identifier("grust_1").is_ok());
    assert!(validate_identifier("1grust").is_err());
    assert!(validate_identifier("grust-nodes").is_err());
}

#[tokio::test]
#[ignore = "requires PostgreSQL with the pgGraph graph extension"]
async fn live_postgres_put_read_traverse_and_schema() {
    let connection_string = std::env::var("PGGRAPH_TEST_CONNECTION_STRING")
        .unwrap_or_else(|_| "host=127.0.0.1 user=alexy dbname=postgres".to_string());
    let suffix = std::process::id();
    let store = PgGraphStore::connect(PgGraphConfig {
        connection_string,
        schema: format!("grust_integration_{suffix}"),
        table_prefix: "grust_live".to_string(),
        batch_size: 2,
        auto_build: false,
        ..PgGraphConfig::default()
    })
    .await
    .expect("connect pgGraph store");

    store.bootstrap().await.expect("bootstrap pgGraph tables");
    store.clear().await.expect("clear pgGraph tables");
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
        .expect("apply pgGraph schema");

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

    let edges = store
        .get_edges(EdgeQuery {
            from: Some(NodeId::new("person-1")),
            to: Some(NodeId::new("talk-1")),
            label: Some(Label::new("PRESENTS")),
        })
        .await
        .expect("read edges");
    assert_eq!(edges.len(), 1);

    let talks = store
        .traverse(Traversal::from_node("person-1").out("PRESENTS").to("Talk"))
        .await
        .expect("traverse");
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
