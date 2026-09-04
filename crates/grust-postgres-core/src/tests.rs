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
fn bootstrap_creates_universal_postgres_tables() {
    let config = PostgresGraphConfig::default();
    let sql = bootstrap_sql(
        &config,
        "\"public\".\"grust_nodes\"",
        "\"public\".\"grust_edges\"",
    )
    .unwrap();

    assert!(!sql.contains("CREATE EXTENSION IF NOT EXISTS graph"));
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"public\".\"grust_nodes\""));
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"public\".\"grust_edges\""));
    assert!(sql.contains("CREATE INDEX IF NOT EXISTS \"grust_edges_from_idx\""));
    assert!(sql.contains("CREATE INDEX IF NOT EXISTS \"grust_edges_to_idx\""));
    assert!(sql.contains("CREATE INDEX IF NOT EXISTS \"grust_nodes_label_idx\""));
}

#[test]
fn postgres_recursive_walk_tokens_are_delimiter_free() {
    let dialect = PostgresReadDialect::new(&PostgresGraphConfig::default());
    assert_eq!(
        dialect.recursive_walk_id_token("ed.to_id"),
        Some("encode(convert_to(ed.to_id, 'UTF8'), 'hex')".to_string())
    );
}

#[test]
fn graph_schema_creates_typed_views_and_indexes() {
    let config = PostgresGraphConfig::default();
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

    let sql = postgres_schema_sql(
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
fn graph_schema_enforces_postgres_identifier_byte_limit() {
    let config = PostgresGraphConfig::default();
    let at_limit = GraphSchema::builder()
        .node("x".repeat(52), Vec::new())
        .build();
    postgres_schema_sql(
        &config,
        "\"public\".\"grust_nodes\"",
        "\"public\".\"grust_edges\"",
        &at_limit,
    )
    .expect("63-byte generated view identifier should be accepted");

    let over_limit = GraphSchema::builder()
        .node("x".repeat(53), Vec::new())
        .build();
    let error = postgres_schema_sql(
        &config,
        "\"public\".\"grust_nodes\"",
        "\"public\".\"grust_edges\"",
        &over_limit,
    )
    .expect_err("PostgreSQL silently truncates identifiers beyond 63 bytes");

    assert!(
        error
            .to_string()
            .contains("PostgreSQL generated identifier")
    );
    assert!(error.to_string().contains("is 64 bytes"));
    assert!(error.to_string().contains("limit is 63 bytes"));
}

#[test]
fn postgres_config_enforces_schema_and_derived_prefix_boundaries() {
    let mut config = PostgresGraphConfig {
        schema: "s".repeat(63),
        ..PostgresGraphConfig::default()
    };
    bootstrap_sql(
        &config,
        "\"schema\".\"grust_nodes\"",
        "\"schema\".\"grust_edges\"",
    )
    .expect("63-byte schema identifier should be accepted");

    config.schema = "s".repeat(64);
    let error = bootstrap_sql(
        &config,
        "\"schema\".\"grust_nodes\"",
        "\"schema\".\"grust_edges\"",
    )
    .expect_err("64-byte schema identifier must be rejected");
    assert!(error.to_string().contains("is 64 bytes"));
    assert!(error.to_string().contains("limit is 63 bytes"));

    config.schema = "public".to_string();
    config.table_prefix = "p".repeat(47);
    bootstrap_sql(&config, "\"public\".\"nodes\"", "\"public\".\"edges\"")
        .expect("47-byte prefix should keep every bootstrap identifier within 63 bytes");

    config.table_prefix = "p".repeat(48);
    let error = bootstrap_sql(&config, "\"public\".\"nodes\"", "\"public\".\"edges\"")
        .expect_err("48-byte prefix makes the node-label index 64 bytes");
    assert!(error.to_string().contains("_nodes_label_idx"));
    assert!(error.to_string().contains("is 64 bytes"));
}

#[test]
fn typed_field_aliases_use_postgres_utf8_byte_length() {
    let at_limit = format!("{}a", "é".repeat(31));
    assert_eq!(at_limit.len(), 63);
    let schema = GraphSchema::builder()
        .node(
            "Person",
            vec![Field::required(at_limit.clone(), FieldType::String)],
        )
        .build();
    let sql = postgres_schema_sql(
        &PostgresGraphConfig::default(),
        "\"public\".\"grust_nodes\"",
        "\"public\".\"grust_edges\"",
        &schema,
    )
    .expect("63-byte UTF-8 alias should be accepted");
    assert!(sql.contains(&quote_ident(&at_limit)));

    let over_limit = "é".repeat(32);
    assert_eq!(over_limit.len(), 64);
    let schema = GraphSchema::builder()
        .node(
            "Person",
            vec![Field::required(over_limit, FieldType::String)],
        )
        .build();
    let error = postgres_schema_sql(
        &PostgresGraphConfig::default(),
        "\"public\".\"grust_nodes\"",
        "\"public\".\"grust_edges\"",
        &schema,
    )
    .expect_err("64-byte UTF-8 alias must be rejected before DDL");
    assert!(error.to_string().contains("typed field alias"));
    assert!(error.to_string().contains("is 64 bytes"));
}

#[test]
fn typed_views_reject_fixed_and_duplicate_column_aliases() {
    {
        let fixed = "id";
        let schema = GraphSchema::builder()
            .node("Person", vec![Field::required(fixed, FieldType::String)])
            .build();
        let error = postgres_schema_sql(
            &PostgresGraphConfig::default(),
            "\"public\".\"grust_nodes\"",
            "\"public\".\"grust_edges\"",
            &schema,
        )
        .expect_err("node fields must not collide with the fixed id column");
        assert!(error.to_string().contains("fixed column 'id'"));
    }

    for fixed in ["id", "from_id", "to_id"] {
        let schema = GraphSchema::builder()
            .edge(
                "RELATES",
                Vec::<Label>::new(),
                Vec::<Label>::new(),
                vec![Field::required(fixed, FieldType::String)],
            )
            .build();
        let error = postgres_schema_sql(
            &PostgresGraphConfig::default(),
            "\"public\".\"grust_nodes\"",
            "\"public\".\"grust_edges\"",
            &schema,
        )
        .expect_err("edge fields must not collide with fixed columns");
        assert!(
            error
                .to_string()
                .contains(&format!("fixed column '{fixed}'"))
        );
    }

    let schema = GraphSchema::builder()
        .node(
            "Person",
            vec![
                Field::required("name", FieldType::String),
                Field::optional("name", FieldType::String),
            ],
        )
        .build();
    let error = postgres_schema_sql(
        &PostgresGraphConfig::default(),
        "\"public\".\"grust_nodes\"",
        "\"public\".\"grust_edges\"",
        &schema,
    )
    .expect_err("duplicate raw typed aliases must be rejected");
    assert!(error.to_string().contains("both use identifier 'name'"));
}

#[test]
fn postgres_typed_column_validates_its_public_input() {
    let at_limit = Field::required("x".repeat(63), FieldType::String);
    postgres_typed_column(&at_limit).expect("63-byte alias should be accepted");

    for invalid in [String::new(), "bad\0alias".to_string(), "x".repeat(64)] {
        let field = Field::required(invalid, FieldType::String);
        assert!(
            postgres_typed_column(&field).is_err(),
            "empty, NUL, and overlong aliases must be rejected"
        );
    }
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
fn mutation_batch_sql_wraps_ordered_mutations_in_transaction() {
    let mutations = vec![
        GraphMutation::UpsertNode(Node::new("Person", "person-1", Props::new())),
        GraphMutation::UpsertNode(Node::new("Talk", "talk-1", Props::new())),
        GraphMutation::UpsertEdge(Edge::new("PRESENTS", "person-1", "talk-1", Props::new())),
        GraphMutation::PatchNode {
            id: NodeId::new("person-1"),
            props: Props::from([("name".to_string(), Value::from("Ada"))]),
        },
        GraphMutation::DeleteEdge {
            from: NodeId::new("person-1"),
            label: Label::new("PRESENTS"),
            to: NodeId::new("talk-1"),
        },
        GraphMutation::DeleteNode(NodeId::new("person-1")),
    ];
    let sql = apply_mutations_sql(
        "\"public\".\"grust_nodes\"",
        "\"public\".\"grust_edges\"",
        &mutations,
    )
    .unwrap();

    assert!(sql.starts_with("BEGIN;\n"));
    assert!(sql.ends_with(";\nCOMMIT"));
    assert!(sql.contains("INSERT INTO \"public\".\"grust_nodes\""));
    assert!(sql.contains("INSERT INTO \"public\".\"grust_edges\""));
    assert!(sql.contains("UPDATE \"public\".\"grust_nodes\" SET props = props ||"));
    assert!(sql.contains("'person-1'"));
    assert!(sql.contains("DELETE FROM \"public\".\"grust_edges\" WHERE from_id = 'person-1' AND label = 'PRESENTS' AND to_id = 'talk-1'"));
    assert!(sql.contains("DELETE FROM \"public\".\"grust_nodes\" WHERE id = 'person-1'"));
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

#[test]
fn rejects_unsafe_json_property_keys() {
    assert!(validate_json_key("safe_key_1").is_ok());
    assert!(validate_json_key("display-name").is_err());
    assert!(validate_json_key("'} OR 1=1--").is_err());
    assert!(validate_json_key("").is_err());
}

#[tokio::test]
#[ignore = "requires PostgreSQL"]
async fn live_invalid_typed_schema_is_rejected_before_bootstrap_ddl() {
    let connection_string = std::env::var("POSTGRES_TEST_CONNECTION_STRING")
        .unwrap_or_else(|_| "host=127.0.0.1 user=alexy dbname=postgres".to_string());
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock should be after Unix epoch")
        .as_nanos();
    let schema_name = format!("grust_invalid_schema_{}_{nonce}", std::process::id());
    let store = PostgresGraphStore::connect(PostgresGraphConfig {
        connection_string,
        schema: schema_name.clone(),
        ..PostgresGraphConfig::default()
    })
    .await
    .expect("connect PostgreSQL store");

    let invalid = GraphSchema::builder()
        .node("Person", vec![Field::required("id", FieldType::String)])
        .build();
    store
        .apply_schema(&invalid)
        .await
        .expect_err("fixed-column collision must fail before bootstrap");

    let row = store
        .client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1)",
            &[&schema_name],
        )
        .await
        .expect("check whether invalid apply created a schema");
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
        "invalid typed schema must not execute bootstrap DDL"
    );
}

#[tokio::test]
#[ignore = "requires PostgreSQL"]
async fn live_postgres_put_read_traverse_and_schema() {
    let connection_string = std::env::var("POSTGRES_TEST_CONNECTION_STRING")
        .unwrap_or_else(|_| "host=127.0.0.1 user=alexy dbname=postgres".to_string());
    let suffix = std::process::id();
    let store = PostgresGraphStore::connect(PostgresGraphConfig {
        connection_string,
        schema: format!("grust_integration_{suffix}"),
        table_prefix: "grust_live".to_string(),
        batch_size: 2,
    })
    .await
    .expect("connect PostgreSQL store");

    store
        .bootstrap()
        .await
        .expect("bootstrap PostgreSQL tables");
    store.clear().await.expect("clear PostgreSQL tables");
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
        .expect("apply PostgreSQL schema");

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

    let ordered = Node::new("Person", "ordered-person", Props::new());
    let ordered_report = store
        .execute_cypher_mutation_plan(&GraphMutationPlan::new(vec![
            GraphMutationPlanOp::UpsertNode {
                kind: GraphMutationPlanKind::Create,
                node: ordered.clone(),
            },
            GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::from([("id".to_string(), Value::from("ordered-person"))]),
                predicates: Vec::new(),
                patch: Props::from([("ready".to_string(), Value::from(true))]),
                cardinality: GraphMutationCardinality::SingleIdentity,
            },
        ]))
        .await
        .expect("execute ordered PostgreSQL mutation plan");
    assert_eq!(ordered_report.matched_rows, 1);
    assert_eq!(
        store
            .get_node(&ordered.id)
            .await
            .expect("read ordered node")
            .expect("ordered node missing")
            .props
            .get("ready"),
        Some(&Value::from(true))
    );

    let rollback_node = Node::new("Person", "must-roll-back", Props::new());
    let failed = store
        .execute_cypher_mutation_plan(&GraphMutationPlan::new(vec![
            GraphMutationPlanOp::UpsertNode {
                kind: GraphMutationPlanKind::Create,
                node: rollback_node.clone(),
            },
            GraphMutationPlanOp::UpsertEdge {
                kind: GraphMutationPlanKind::Create,
                edge: Edge::new(
                    "PRESENTS",
                    rollback_node.id.clone(),
                    "missing-endpoint",
                    Props::new(),
                ),
            },
        ]))
        .await;
    assert!(failed.is_err(), "the missing endpoint must reject the plan");
    assert!(
        store
            .get_node(&rollback_node.id)
            .await
            .expect("read after rollback")
            .is_none(),
        "a failed plan must roll back its valid prefix"
    );

    let cancelled_node = Node::new("Person", "cancelled-plan", Props::new());
    {
        let _gate = store.lock_connection().await.expect("lock connection");
        store
            .transaction_needs_rollback
            .store(true, Ordering::Release);
        store
            .execute_unlocked("BEGIN")
            .await
            .expect("begin simulated cancelled plan");
        store
            .execute_unlocked(
                &upsert_nodes_sql(&store.nodes_table(), std::slice::from_ref(&cancelled_node))
                    .expect("lower simulated cancelled write"),
            )
            .await
            .expect("write simulated cancelled prefix");
        // Dropping the gate while the recovery marker remains set simulates a
        // future being aborted after a successful statement but before COMMIT.
    }
    assert!(
        store
            .get_node(&cancelled_node.id)
            .await
            .expect("read after cancellation recovery")
            .is_none(),
        "the next caller must roll back an abandoned transaction"
    );

    store
        .execute(&format!(
            "DROP SCHEMA {} CASCADE",
            quote_ident(&store.config().schema)
        ))
        .await
        .expect("drop integration schema");
}
