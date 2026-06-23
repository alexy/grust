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
fn bootstrap_creates_universal_turso_tables() {
    let config = TursoConfig::default();
    let sql = bootstrap_sql(&config, "\"grust_nodes\"", "\"grust_edges\"").unwrap();

    assert!(sql.contains("PRAGMA foreign_keys = ON"));
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"grust_nodes\""));
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"grust_edges\""));
    assert!(sql.contains("props TEXT NOT NULL"));
    assert!(sql.contains("PRIMARY KEY (from_id, label, to_id)"));
}

#[test]
fn schema_creates_json_views_and_indexes() {
    let config = TursoConfig::default();
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

    let sql = turso_schema_sql(&config, "\"grust_nodes\"", "\"grust_edges\"", &schema).unwrap();

    assert!(sql.contains("CREATE VIEW IF NOT EXISTS \"grust_node_person\""));
    assert!(sql.contains("json_extract(props, '$.name.value') AS \"name\""));
    assert!(sql.contains("CAST(json_extract(props, '$.age.value') AS INTEGER) AS \"age\""));
    assert!(sql.contains("CREATE VIEW IF NOT EXISTS \"grust_edge_works_on\""));
    assert!(sql.contains("\"grust_node_person_age_idx\""));
    assert!(sql.contains("\"grust_edge_works_on_since_idx\""));
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
    let sql = apply_mutations_sql("\"grust_nodes\"", "\"grust_edges\"", &mutations).unwrap();

    assert!(sql.starts_with("BEGIN;\n"));
    assert!(sql.ends_with(";\nCOMMIT"));
    assert!(sql.contains("INSERT INTO \"grust_nodes\""));
    assert!(sql.contains("INSERT INTO \"grust_edges\""));
    assert!(sql.contains("UPDATE \"grust_nodes\" SET props = json_patch(props,"));
    assert!(sql.contains("DELETE FROM \"grust_edges\" WHERE from_id = 'person-1'"));
    assert!(sql.contains("DELETE FROM \"grust_nodes\" WHERE id = 'person-1'"));
}

#[test]
fn traversal_sql_builds_exact_out_step() {
    let traversal = Traversal::from_node("person-1")
        .out("PRESENTS")
        .to("Talk")
        .limit(10);

    let sql = traversal_sql("\"grust_nodes\"", "\"grust_edges\"", &traversal).unwrap();

    assert!(sql.contains("JOIN \"grust_edges\" e0 ON e0.from_id = n0.id"));
    assert!(sql.contains("AND e0.label = 'PRESENTS'"));
    assert!(sql.contains("JOIN \"grust_nodes\" n1 ON n1.id = e0.to_id"));
    assert!(sql.contains("AND n1.label = 'Talk'"));
    assert!(sql.contains("WHERE n0.id = 'person-1'"));
    assert!(sql.contains("LIMIT 10"));
}

#[test]
fn rejects_invalid_table_prefix() {
    assert!(validate_identifier("grust_1").is_ok());
    assert!(validate_identifier("1grust").is_err());
    assert!(validate_identifier("grust-nodes").is_err());
}

#[tokio::test]
async fn in_memory_put_read_traverse_schema_and_mutations() {
    let store = TursoGraphStore::in_memory()
        .await
        .expect("open Turso store");
    store.bootstrap().await.expect("bootstrap Turso tables");
    store.clear().await.expect("clear Turso tables");
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
        .expect("apply Turso schema");

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

    let by_property = store
        .traverse(Traversal {
            start: Start::NodesByProperty {
                label: Label::new("Person"),
                key: "name".to_string(),
                value: Value::from("Ada"),
            },
            steps: Vec::new(),
            limit: None,
        })
        .await
        .expect("property start");
    assert_eq!(by_property.len(), 1);
    assert_eq!(by_property[0].id, NodeId::new("person-1"));

    let talks = store
        .traverse(Traversal::from_node("person-1").out("PRESENTS").to("Talk"))
        .await
        .expect("traverse");
    assert_eq!(talks.len(), 1);
    assert_eq!(talks[0].id, NodeId::new("talk-1"));

    store
        .apply_mutations(&[
            GraphMutation::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([("role".to_string(), Value::from("engineer"))]),
            },
            GraphMutation::DeleteEdge {
                from: NodeId::new("person-1"),
                label: Label::new("PRESENTS"),
                to: NodeId::new("talk-1"),
            },
        ])
        .await
        .expect("apply mutations");

    let person = store
        .get_node(&NodeId::new("person-1"))
        .await
        .expect("read patched node")
        .expect("person node missing");
    assert_eq!(person.props.get("role"), Some(&Value::from("engineer")));
    let edges = store
        .get_edges(EdgeQuery {
            from: Some(NodeId::new("person-1")),
            to: Some(NodeId::new("talk-1")),
            label: Some(Label::new("PRESENTS")),
        })
        .await
        .expect("read deleted edges");
    assert!(edges.is_empty());
}

#[tokio::test]
async fn cypher_mutation_executor_patches_matching_nodes() {
    let store = TursoGraphStore::in_memory()
        .await
        .expect("open Turso store");
    store.bootstrap().await.expect("bootstrap Turso tables");
    store.put_graph(&sample_graph()).await.expect("write graph");

    let report = store
        .execute_cypher_mutation_plan(&GraphMutationPlan::new(vec![
            GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::from([("id".to_string(), Value::from("person-1"))]),
                predicates: Vec::new(),
                patch: Props::from([("querygraph_ready".to_string(), Value::from(true))]),
                cardinality: GraphMutationCardinality::SingleIdentity,
            },
        ]))
        .await
        .expect("execute matched-node patch");

    assert_eq!(report.matched_rows, 1);
    assert_eq!(report.node_patches, 1);
    assert_eq!(report.changed_nodes, 1);
    let person = store
        .get_node(&NodeId::new("person-1"))
        .await
        .expect("read patched node")
        .expect("person node missing");
    assert_eq!(
        person.props.get("querygraph_ready"),
        Some(&Value::from(true))
    );
}
