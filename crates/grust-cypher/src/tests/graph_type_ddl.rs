use super::*;

#[test]
fn graph_type_ddl_parses_node_and_edge_types() {
    let statements = sail_cypher_ddl(
        "CREATE GRAPH TYPE social IF NOT EXISTS CLOSED AS \
         NODE Person (id STRING REQUIRED, age INT), \
         EDGE KNOWS FROM Person TO Person (since INT)",
    )
    .expect("parse graph type");

    let expected = GraphTypeDefinition {
        mode: GraphTypeMode::Closed,
        schema: GraphSchema::builder()
            .node(
                "Person",
                vec![
                    Field::required("id", FieldType::String),
                    Field::optional("age", FieldType::Int),
                ],
            )
            .edge(
                "KNOWS",
                vec![Label::new("Person")],
                vec![Label::new("Person")],
                vec![Field::optional("since", FieldType::Int)],
            )
            .build(),
    };

    assert_eq!(
        statements,
        vec![CypherDdlStatement::CreateGraphType {
            name: "social".to_string(),
            if_not_exists: true,
            graph_type: expected,
        }]
    );
}

#[test]
fn graph_type_ddl_parses_drop_graph_type() {
    let statements =
        sail_cypher_ddl("DROP GRAPH TYPE social IF EXISTS").expect("parse drop graph type");
    assert_eq!(
        statements,
        vec![CypherDdlStatement::DropGraphType {
            name: "social".to_string(),
            if_exists: true,
        }]
    );
}

#[test]
fn graph_type_registry_tracks_named_graph_types() {
    let mut registry = CypherConstraintRegistry::new();
    let created = registry
        .apply_cypher(
            "CREATE GRAPH TYPE social CLOSED AS \
             NODE Person (id STRING NOT NULL), \
             EDGE KNOWS FROM Person TO Person ()",
        )
        .expect("create graph type");
    assert_eq!(
        created,
        CypherDdlApplicationReport {
            created: 1,
            ..Default::default()
        }
    );
    let graph_types = registry.named_graph_types();
    assert_eq!(graph_types.len(), 1);
    assert_eq!(graph_types[0].name, "social");
    assert_eq!(graph_types[0].graph_type.mode, GraphTypeMode::Closed);
    assert_eq!(graph_types[0].graph_type.schema.nodes.len(), 1);
    assert_eq!(graph_types[0].graph_type.schema.edges.len(), 1);

    let skipped = registry
        .apply_cypher(
            "CREATE GRAPH TYPE social IF NOT EXISTS CLOSED AS \
             NODE Person (id STRING NOT NULL)",
        )
        .expect("skip existing graph type");
    assert_eq!(
        skipped,
        CypherDdlApplicationReport {
            skipped: 1,
            ..Default::default()
        }
    );

    let dropped = registry
        .apply_cypher("DROP GRAPH TYPE social")
        .expect("drop graph type");
    assert_eq!(
        dropped,
        CypherDdlApplicationReport {
            dropped: 1,
            ..Default::default()
        }
    );
    assert!(registry.named_graph_types().is_empty());
}

#[test]
fn graph_type_ddl_rejects_unknown_field_type() {
    let error = sail_cypher_ddl("CREATE GRAPH TYPE social AS NODE Person (age NUMBER)")
        .expect_err("unknown field type must be rejected");
    assert!(matches!(error, GrustError::CypherSyntax(_)), "{error:?}");
}

#[test]
fn graph_type_ddl_rejected_by_constraints_collector() {
    let error = sail_cypher_constraints("CREATE GRAPH TYPE social AS NODE Person (id STRING)")
        .expect_err("graph type DDL must be rejected by constraints collector");
    assert!(matches!(error, GrustError::CypherSyntax(_)), "{error:?}");
}
