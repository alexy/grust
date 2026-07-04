use crate::*;

#[test]
fn registry_builds_catalog_snapshot() {
    let mut registry = CypherConstraintRegistry::new();
    registry
        .apply_statements(
            cypher_ddl(
                "CREATE GRAPH TYPE social AS NODE Person (name STRING REQUIRED), \
             EDGE KNOWS FROM Person TO Person (since INT);
             CREATE INDEX person_name FOR (n:Person) ON (n.name);
             CREATE CONSTRAINT person_name_required FOR (n:Person) REQUIRE n.name IS NOT NULL",
            )
            .expect("parse catalog ddl"),
        )
        .expect("apply catalog ddl");

    let catalog = registry.catalog_snapshot("default");
    assert_eq!(
        catalog.graphs,
        vec![NamedGraphCatalog {
            name: "default".to_string(),
            graph_type: Some("social".to_string()),
        }]
    );
    assert_eq!(catalog.graph_types.len(), 1);
    assert_eq!(catalog.indexes.len(), 1);
    assert_eq!(catalog.constraints.len(), 1);
    assert_eq!(catalog.anonymous_constraint_count, 0);
}

#[test]
fn catalog_procedures_return_deterministic_metadata_tables() {
    let mut registry = CypherConstraintRegistry::new();
    registry
        .apply_statements(
            cypher_ddl(
                "CREATE GRAPH TYPE social CLOSED AS NODE Person (name STRING), \
             EDGE KNOWS FROM Person TO Person (since INT);
             CREATE INDEX person_name FOR (n:Person) ON (n.name);
             CREATE CONSTRAINT person_name_unique FOR (n:Person) REQUIRE n.name IS UNIQUE",
            )
            .expect("parse catalog ddl"),
        )
        .expect("apply catalog ddl");
    let catalog = registry.catalog_snapshot("default");

    let graphs = cypher_catalog_procedure(&catalog, "db.graphs").expect("graphs");
    assert_eq!(graphs.columns, vec!["graph", "graphType"]);
    assert_eq!(
        graphs.rows,
        vec![vec![Value::from("default"), Value::from("social"),]]
    );

    let graph_types = cypher_catalog_procedure(&catalog, "db.graphTypes").expect("graph types");
    assert_eq!(
        graph_types.columns,
        vec![
            "graphType",
            "elementKind",
            "label",
            "fromLabels",
            "toLabels",
            "fieldCount",
            "mode",
        ]
    );
    assert_eq!(graph_types.rows.len(), 2);
    assert!(graph_types.rows.contains(&vec![
        Value::from("social"),
        Value::from("node"),
        Value::from("Person"),
        Value::Null,
        Value::Null,
        Value::from(1usize),
        Value::from("closed"),
    ]));

    let indexes = cypher_catalog_procedure(&catalog, "db.indexes").expect("indexes");
    assert_eq!(
        indexes.rows,
        vec![vec![
            Value::from("person_name"),
            Value::from("node"),
            Value::from("Person"),
            Value::from("name"),
        ]]
    );

    let constraints = cypher_catalog_procedure(&catalog, "db.constraints").expect("constraints");
    assert_eq!(
        constraints.rows,
        vec![vec![
            Value::from("person_name_unique"),
            Value::from("node-property-unique"),
            Value::from("Person"),
            Value::from("name"),
        ]]
    );
}

#[test]
fn unknown_catalog_procedure_is_structured_non_support() {
    let catalog = CypherConstraintRegistry::new().catalog_snapshot("default");
    let error = cypher_catalog_procedure(&catalog, "db.schema.visualization")
        .expect_err("unsupported catalog procedure");
    assert!(matches!(error, GrustError::Unsupported(_)));
    assert!(error.to_string().contains("catalog-metadata"));
}
