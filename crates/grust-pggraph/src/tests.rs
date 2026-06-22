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
fn bootstrap_registers_pggraph_projection() {
    let config = PgGraphConfig::default();
    let sql = pggraph_bootstrap_sql(
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
fn build_mode_maps_to_pggraph_projection_modes() {
    assert_eq!(
        PgGraphBuildMode::CsrReadonly.as_pggraph_mode(),
        "csr_readonly"
    );
    assert_eq!(
        PgGraphBuildMode::MutableOverlay.as_pggraph_mode(),
        "mutable_overlay"
    );
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
