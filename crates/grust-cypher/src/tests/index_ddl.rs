use super::*;

#[test]
fn index_ddl_parses_node_index() {
    let statements =
        sail_cypher_ddl("CREATE INDEX person_email IF NOT EXISTS FOR (n:Person) ON (n.email)")
            .expect("parse create index");
    assert_eq!(
        statements,
        vec![CypherDdlStatement::CreateIndex {
            name: "person_email".to_string(),
            if_not_exists: true,
            index: GraphIndexDefinition {
                element: GraphIndexElement::Node,
                label: Label::new("Person"),
                key: "email".to_string(),
            },
        }]
    );
}

#[test]
fn index_ddl_parses_relationship_index_and_drop() {
    let statements = sail_cypher_ddl(
        "CREATE INDEX knows_since FOR ()-[r:KNOWS]-() ON (r.since); \
         DROP INDEX knows_since IF EXISTS",
    )
    .expect("parse relationship index and drop");
    assert_eq!(
        statements,
        vec![
            CypherDdlStatement::CreateIndex {
                name: "knows_since".to_string(),
                if_not_exists: false,
                index: GraphIndexDefinition {
                    element: GraphIndexElement::Edge,
                    label: Label::new("KNOWS"),
                    key: "since".to_string(),
                },
            },
            CypherDdlStatement::DropIndex {
                name: "knows_since".to_string(),
                if_exists: true,
            },
        ]
    );
}

#[test]
fn index_ddl_rejects_composite_index() {
    let error = sail_cypher_ddl("CREATE INDEX person_lookup FOR (n:Person) ON (n.email, n.name)")
        .expect_err("composite index must be rejected for the first portable index surface");
    assert!(matches!(error, GrustError::CypherSyntax(_)), "{error:?}");
}

#[test]
fn index_ddl_rejected_by_constraints_collector() {
    let error = sail_cypher_constraints("CREATE INDEX person_email FOR (n:Person) ON (n.email)")
        .expect_err("index DDL must be rejected by constraints collector");
    assert!(matches!(error, GrustError::CypherSyntax(_)), "{error:?}");
}

#[test]
fn index_registry_tracks_named_indexes() {
    let mut registry = CypherConstraintRegistry::new();

    let created = registry
        .apply_cypher(
            "CREATE INDEX person_email IF NOT EXISTS \
             FOR (n:Person) ON (n.email)",
        )
        .expect("create index");
    assert_eq!(
        created,
        CypherDdlApplicationReport {
            created: 1,
            ..Default::default()
        }
    );
    assert_eq!(
        registry.named_indexes(),
        vec![NamedGraphIndex {
            name: "person_email".to_string(),
            index: GraphIndexDefinition {
                element: GraphIndexElement::Node,
                label: Label::new("Person"),
                key: "email".to_string(),
            },
        }]
    );

    let skipped = registry
        .apply_cypher(
            "CREATE INDEX person_email IF NOT EXISTS \
             FOR (n:Person) ON (n.email)",
        )
        .expect("skip existing index");
    assert_eq!(
        skipped,
        CypherDdlApplicationReport {
            skipped: 1,
            ..Default::default()
        }
    );

    let dropped = registry
        .apply_cypher("DROP INDEX person_email")
        .expect("drop index");
    assert_eq!(
        dropped,
        CypherDdlApplicationReport {
            dropped: 1,
            ..Default::default()
        }
    );
    assert!(registry.named_indexes().is_empty());
}
