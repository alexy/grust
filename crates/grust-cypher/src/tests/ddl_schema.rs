//! ddl_schema tests (split verbatim from the former monolithic tests.rs).
use super::*;

#[test]
fn cypher_row_producing_edge_accepts_single_explicit_id() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'ada', status: 'active'});
                CREATE (:Team {id: 'eng'});
                MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
                CREATE (a)-[e:MEMBER_OF {id: 'membership-1', source: 'cypher'}]->(b)
                RETURN e.id, e.source;
                ",
            CypherMutationOptions {
                collect_written_edge_identities: true,
                ..CypherMutationOptions::default()
            },
        ))
        .expect("single row-producing edge can carry explicit id");

    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec!["e.id".to_string(), "e.source".to_string()],
            rows: vec![vec![Value::from("membership-1"), Value::from("cypher")]],
        }
    );
    assert_eq!(
        futures_executor::block_on(store.get_edges(EdgeQuery::default()))
            .expect("read edges")
            .into_iter()
            .map(|edge| edge.id.map(|id| id.as_str().to_string()))
            .collect::<Vec<_>>(),
        vec![Some("membership-1".to_string())]
    );
    assert_eq!(
        result.mutation.written_edge_identities,
        vec![CypherWrittenEdgeIdentity {
            kind: GraphMutationPlanKind::Create,
            from: NodeId::new("ada"),
            label: Label::new("MEMBER_OF"),
            to: NodeId::new("eng"),
            id: Some(EdgeId::new("membership-1")),
        }]
    );
}

#[test]
fn cypher_row_producing_edge_collects_structural_identity() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'ada', status: 'active'});
                CREATE (:Team {id: 'eng'});
                MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
                CREATE (a)-[e:MEMBER_OF {source: 'cypher'}]->(b)
                RETURN e.id, e.source;
                ",
            CypherMutationOptions {
                collect_written_edge_identities: true,
                ..CypherMutationOptions::default()
            },
        ))
        .expect("single row-producing edge can report structural identity");

    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec!["e.id".to_string(), "e.source".to_string()],
            rows: vec![vec![Value::Null, Value::from("cypher")]],
        }
    );
    assert_eq!(
        result.mutation.written_edge_identities,
        vec![CypherWrittenEdgeIdentity {
            kind: GraphMutationPlanKind::Create,
            from: NodeId::new("ada"),
            label: Label::new("MEMBER_OF"),
            to: NodeId::new("eng"),
            id: None,
        }]
    );
}

#[test]
fn cypher_row_producing_edge_generates_ids_for_create() {
    let store = MemoryGraphStore::new();
    let props = Props::from([("source".to_string(), Value::from("cypher"))]);
    let mut expected = vec![
        generated_row_edge_id(
            &NodeId::new("ada"),
            &Label::new("MEMBER_OF"),
            &NodeId::new("eng"),
            &props,
        ),
        generated_row_edge_id(
            &NodeId::new("bob"),
            &Label::new("MEMBER_OF"),
            &NodeId::new("eng"),
            &props,
        ),
    ];
    expected.sort();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'ada', status: 'active'});
                CREATE (:Person {id: 'bob', status: 'active'});
                CREATE (:Team {id: 'eng'});
                MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
                CREATE (a)-[e:MEMBER_OF {source: 'cypher'}]->(b)
                RETURN e.id ORDER BY e.id;
                ",
            CypherMutationOptions {
                relationship_id_policy: CypherRelationshipIdPolicy::GenerateForRowCreate,
                collect_written_edge_identities: true,
                ..CypherMutationOptions::default()
            },
        ))
        .expect("row-producing create can generate per-row edge ids");

    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec!["e.id".to_string()],
            rows: expected
                .iter()
                .map(|id| vec![Value::from(id.as_str())])
                .collect(),
        }
    );
    let mut written = result
        .mutation
        .written_edge_identities
        .iter()
        .map(|identity| identity.id.clone())
        .collect::<Vec<_>>();
    written.sort();
    assert_eq!(
        written,
        expected
            .iter()
            .cloned()
            .map(Some)
            .collect::<Vec<Option<EdgeId>>>()
    );
    let mut persisted = futures_executor::block_on(store.get_edges(EdgeQuery::default()))
        .expect("read generated edge ids")
        .into_iter()
        .filter_map(|edge| edge.id)
        .collect::<Vec<_>>();
    persisted.sort();
    assert_eq!(persisted, expected);
}

#[test]
fn cypher_row_producing_edge_generates_ids_for_merge_when_requested() {
    let store = MemoryGraphStore::new();
    let props = Props::from([("source".to_string(), Value::from("merge"))]);
    let expected = generated_row_edge_id(
        &NodeId::new("ada"),
        &Label::new("MEMBER_OF"),
        &NodeId::new("eng"),
        &props,
    );

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'ada', status: 'active'});
                CREATE (:Team {id: 'eng'});
                MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
                MERGE (a)-[e:MEMBER_OF {source: 'merge'}]->(b)
                RETURN e.id;
                ",
            CypherMutationOptions {
                relationship_id_policy: CypherRelationshipIdPolicy::GenerateForRowCreateAndMerge,
                collect_written_edge_identities: true,
                ..CypherMutationOptions::default()
            },
        ))
        .expect("row-producing merge can generate edge ids when explicitly requested");

    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec!["e.id".to_string()],
            rows: vec![vec![Value::from(expected.as_str())]],
        }
    );
    assert_eq!(
        result.mutation.written_edge_identities,
        vec![CypherWrittenEdgeIdentity {
            kind: GraphMutationPlanKind::Merge,
            from: NodeId::new("ada"),
            label: Label::new("MEMBER_OF"),
            to: NodeId::new("eng"),
            id: Some(expected),
        }]
    );
}

#[test]
fn cypher_row_producing_edge_rejects_multirow_explicit_id() {
    let store = MemoryGraphStore::new();

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'ada', status: 'active'});
                CREATE (:Person {id: 'bob', status: 'active'});
                CREATE (:Team {id: 'eng'});
                MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
                CREATE (a)-[e:MEMBER_OF {id: 'membership-1'}]->(b)
                RETURN e.id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect_err("multi-row explicit relationship id should fail");

    assert!(
        matches!(error, GrustError::CypherUnsupportedCardinality(_)),
        "{error:?}"
    );
    assert!(
        futures_executor::block_on(store.get_edges(EdgeQuery::default()))
            .expect("read edges")
            .is_empty()
    );
}

#[test]
fn cypher_parser_polish_handles_comments_keyword_case_and_statement_splitting() {
    let plan = sail_cypher_mutation_plan(
        r#"
            // full-line comment before the batch
            create (:Person {id: 'person-1', note: 'semicolon; and // literal'});
            /* block comment with ; and MATCH (n) DELETE n */
            mErGe (:Person {id: 'person-2'});
            MaTcH (n:Person {status: 'inactive'}) DeLeTe n;
            "#,
    )
    .unwrap();

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            creates: 1,
            merges: 1,
            deletes: 1,
            changed_nodes: 2,
            node_upserts: 2,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(plan.operations.len(), 3);
    assert!(matches!(
        &plan.operations[2],
        GraphMutationPlanOp::DeleteMatchingNodes {
            label,
            cardinality: GraphMutationCardinality::BoundedMany,
            ..
        } if label.as_ref().is_some_and(|label| label.as_str() == "Person")
    ));

    let error = sail_cypher_mutation_plan("CREATE (:Person {id: 'person-1'}); /* nope")
        .expect_err("unterminated block comment should fail");
    assert!(error.to_string().contains("unterminated block comment"));
}

#[test]
fn cypher_local_variables_resolve_edge_endpoints_and_deletes() {
    // Cross-statement local variables (`a`, `b`) resolve in later statements.
    // Unit 10a (decision B): edge DELETE by *pattern* (`DELETE (a)-[:KNOWS]->(b)`)
    // is non-standard and now rejected (see cypher_delete_lowers_resolved_node_and_edge_patterns);
    // node deletion via the bound variable is retained.
    let plan = sail_cypher_mutation_plan(
        "
            CREATE (a:Person {id: 'person-1', name: 'Ada'});
            MERGE (b:Person {id: 'person-2', name: 'Bob'});
            CREATE (a)-[:KNOWS]->(b);
            DELETE (a);
            ",
    )
    .unwrap();

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            creates: 2,
            merges: 1,
            deletes: 1,
            changed_nodes: 3,
            changed_edges: 1,
            node_upserts: 2,
            edge_upserts: 1,
            node_deletes: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        plan.into_mutations(),
        vec![
            GraphMutation::UpsertNode(Node::new(
                "Person",
                "person-1",
                Props::from([
                    ("id".to_string(), Value::String("person-1".to_string())),
                    ("name".to_string(), Value::String("Ada".to_string())),
                ]),
            )),
            GraphMutation::UpsertNode(Node::new(
                "Person",
                "person-2",
                Props::from([
                    ("id".to_string(), Value::String("person-2".to_string())),
                    ("name".to_string(), Value::String("Bob".to_string())),
                ]),
            )),
            GraphMutation::UpsertEdge(Edge::new("KNOWS", "person-1", "person-2", Props::new(),)),
            GraphMutation::DeleteNode(NodeId::new("person-1")),
        ]
    );
}

#[test]
fn cypher_local_variables_reject_rebinding_and_unbound_refs() {
    let error = sail_cypher_mutation_plan(
        "
            CREATE (a:Person {id: 'person-1'});
            CREATE (a:Person {id: 'person-2'});
            ",
    )
    .expect_err("rebinding a variable to a different id should fail");
    assert!(error.to_string().contains("already bound to node id"));

    let error = sail_cypher_mutation_plan("CREATE (a)-[:KNOWS]->(:Person {id: 'person-2'})")
        .expect_err("unbound edge endpoint should fail");
    assert!(error.to_string().contains("variable 'a' is not bound"));
}

#[test]
fn cypher_write_rejects_deferred_v1_semantics() {
    for cypher in [
        "CREATE (:Person {id: 'person-1'}) SET n.name = 'Ada'",
        "REMOVE n.name",
    ] {
        let error = sail_cypher_mutation_plan(cypher).expect_err("unsupported Cypher must fail");
        assert!(is_cypher_planning_error(&error));
    }
}

#[test]
fn cypher_errors_are_structured_for_callers() {
    let error = sail_cypher_mutation_plan("RETURN 1").expect_err("unsupported syntax");
    assert!(matches!(error, GrustError::CypherSyntax(_)));

    // A node write without a resolvable `id` is an unresolved-identity error.
    // (Previously `DELETE (:Person {name: 'Ada'})`; DELETE-by-pattern is now a
    // syntax error under the Unit 10a accept-set gate, so a CREATE is used to
    // exercise the unresolved-identity path specifically.)
    let error = sail_cypher_mutation_plan("CREATE (:Person {name: 'Ada'})")
        .expect_err("unresolved identity");
    assert!(matches!(error, GrustError::CypherUnresolvedIdentity(_)));

    let error = sail_cypher_mutation_plan(
        "MATCH (:Person {id: 'a'})-[e:KNOWS]->(n:Person {id: 'b'}) SET e.weight = n.weight + 1",
    )
    .expect_err("cross-variable edge expression cardinality");
    assert!(matches!(error, GrustError::CypherUnsupportedCardinality(_)));

    let error = cypher_execution_error(GrustError::Backend("boom".to_string()));
    assert!(matches!(error, GrustError::CypherExecution(_)));
}

#[test]
fn cypher_ddl_parses_node_unique_constraint() {
    let statements = sail_cypher_ddl(
        "CREATE CONSTRAINT person_id IF NOT EXISTS FOR (n:Person) REQUIRE n.id IS UNIQUE",
    )
    .expect("parse create constraint");
    assert_eq!(
        statements,
        vec![CypherDdlStatement::CreateConstraint {
            name: Some("person_id".to_string()),
            if_not_exists: true,
            constraint: GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "id".to_string(),
            },
        }]
    );
}

#[test]
fn cypher_ddl_parses_node_required_constraint_without_name() {
    let statements = sail_cypher_ddl("CREATE CONSTRAINT FOR (n:Person) REQUIRE n.name IS NOT NULL")
        .expect("parse create constraint");
    assert_eq!(
        statements,
        vec![CypherDdlStatement::CreateConstraint {
            name: None,
            if_not_exists: false,
            constraint: GraphConstraint::NodePropertyRequired {
                label: Label::new("Person"),
                key: "name".to_string(),
            },
        }]
    );
}

#[test]
fn cypher_ddl_parses_relationship_constraint() {
    let statements =
        sail_cypher_ddl("CREATE CONSTRAINT FOR ()-[r:KNOWS]-() REQUIRE r.since IS NOT NULL")
            .expect("parse relationship constraint");
    assert_eq!(
        statements,
        vec![CypherDdlStatement::CreateConstraint {
            name: None,
            if_not_exists: false,
            constraint: GraphConstraint::EdgePropertyRequired {
                label: Label::new("KNOWS"),
                key: "since".to_string(),
            },
        }]
    );
}

#[test]
fn cypher_ddl_accepts_legacy_on_assert_spelling() {
    let statements = sail_cypher_ddl("CREATE CONSTRAINT ON (n:Person) ASSERT n.email IS UNIQUE")
        .expect("parse legacy constraint");
    assert_eq!(
        statements,
        vec![CypherDdlStatement::CreateConstraint {
            name: None,
            if_not_exists: false,
            constraint: GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "email".to_string(),
            },
        }]
    );
}

#[test]
fn cypher_ddl_parses_drop_constraint() {
    let statements =
        sail_cypher_ddl("DROP CONSTRAINT person_id IF EXISTS").expect("parse drop constraint");
    assert_eq!(
        statements,
        vec![CypherDdlStatement::DropConstraint {
            name: "person_id".to_string(),
            if_exists: true,
        }]
    );
}

#[test]
fn cypher_constraints_collects_multiple_statements() {
    let constraints = sail_cypher_constraints(
        "CREATE CONSTRAINT FOR (n:Person) REQUIRE n.id IS UNIQUE; \
             CREATE CONSTRAINT FOR (n:Person) REQUIRE n.name IS NOT NULL",
    )
    .expect("collect constraints");
    assert_eq!(
        constraints,
        vec![
            GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "id".to_string(),
            },
            GraphConstraint::NodePropertyRequired {
                label: Label::new("Person"),
                key: "name".to_string(),
            },
        ]
    );
}

#[test]
fn cypher_ddl_rejects_predicate_variable_mismatch() {
    let error = sail_cypher_ddl("CREATE CONSTRAINT FOR (n:Person) REQUIRE m.id IS UNIQUE")
        .expect_err("variable mismatch must fail");
    assert!(matches!(error, GrustError::CypherSyntax(_)), "{error:?}");
}

#[test]
fn cypher_ddl_rejects_unknown_predicate() {
    let error = sail_cypher_ddl("CREATE CONSTRAINT FOR (n:Person) REQUIRE n.id IS NODE KEY")
        .expect_err("node key must be rejected");
    assert!(matches!(error, GrustError::CypherSyntax(_)), "{error:?}");
}

#[test]
fn cypher_constraints_rejects_drop() {
    let error = sail_cypher_constraints("DROP CONSTRAINT person_id")
        .expect_err("drop must be rejected by constraints collector");
    assert!(matches!(error, GrustError::CypherSyntax(_)), "{error:?}");
}

#[test]
fn cypher_constraint_registry_applies_create_and_drop() {
    let mut registry = CypherConstraintRegistry::new();

    let created = registry
        .apply_cypher(
            "CREATE CONSTRAINT person_id \
                 FOR (n:Person) REQUIRE n.id IS UNIQUE",
        )
        .expect("create constraint");
    assert_eq!(
        created,
        CypherDdlApplicationReport {
            created: 1,
            ..Default::default()
        }
    );
    assert_eq!(
        registry.named_constraints(),
        vec![NamedGraphConstraint {
            name: "person_id".to_string(),
            constraint: GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "id".to_string(),
            },
        }]
    );
    assert_eq!(
        registry.constraints(),
        vec![GraphConstraint::NodePropertyUnique {
            label: Label::new("Person"),
            key: "id".to_string(),
        }]
    );

    let dropped = registry
        .apply_cypher("DROP CONSTRAINT person_id")
        .expect("drop constraint");
    assert_eq!(
        dropped,
        CypherDdlApplicationReport {
            dropped: 1,
            ..Default::default()
        }
    );
    assert!(registry.constraints().is_empty());
}

#[test]
fn cypher_constraint_registry_honors_if_modifiers() {
    let mut registry = CypherConstraintRegistry::new();
    registry
        .apply_cypher(
            "CREATE CONSTRAINT person_id \
                 FOR (n:Person) REQUIRE n.id IS UNIQUE",
        )
        .expect("initial create");

    let skipped = registry
        .apply_cypher(
            "CREATE CONSTRAINT person_id IF NOT EXISTS \
                 FOR (n:Person) REQUIRE n.email IS UNIQUE",
        )
        .expect("duplicate create with IF NOT EXISTS should skip");
    assert_eq!(
        skipped,
        CypherDdlApplicationReport {
            skipped: 1,
            ..Default::default()
        }
    );
    assert_eq!(
        registry.constraints(),
        vec![GraphConstraint::NodePropertyUnique {
            label: Label::new("Person"),
            key: "id".to_string(),
        }]
    );

    let missing = registry
        .apply_cypher("DROP CONSTRAINT missing IF EXISTS")
        .expect("missing drop with IF EXISTS should not fail");
    assert_eq!(
        missing,
        CypherDdlApplicationReport {
            missing: 1,
            ..Default::default()
        }
    );
}

#[test]
fn cypher_constraint_registry_rejects_duplicate_or_missing_names() {
    let mut registry = CypherConstraintRegistry::new();
    registry
        .apply_cypher(
            "CREATE CONSTRAINT person_id \
                 FOR (n:Person) REQUIRE n.id IS UNIQUE",
        )
        .expect("initial create");

    let duplicate = registry
        .apply_cypher(
            "CREATE CONSTRAINT person_id \
                 FOR (n:Person) REQUIRE n.email IS UNIQUE",
        )
        .expect_err("duplicate create without IF NOT EXISTS should fail");
    assert!(
        matches!(duplicate, GrustError::CypherExecution(_)),
        "{duplicate:?}"
    );

    let missing = registry
        .apply_cypher("DROP CONSTRAINT missing")
        .expect_err("missing drop without IF EXISTS should fail");
    assert!(
        matches!(missing, GrustError::CypherExecution(_)),
        "{missing:?}"
    );
}

#[test]
fn cypher_constraint_registry_preserves_anonymous_constraints() {
    let mut registry = CypherConstraintRegistry::new();
    let report = registry
        .apply_cypher(
            "CREATE CONSTRAINT FOR (n:Person) REQUIRE n.name IS NOT NULL; \
                 CREATE CONSTRAINT person_email FOR (n:Person) REQUIRE n.email IS UNIQUE",
        )
        .expect("create anonymous and named constraints");
    assert_eq!(
        report,
        CypherDdlApplicationReport {
            created: 2,
            ..Default::default()
        }
    );
    assert_eq!(
        registry.anonymous_constraints(),
        &[GraphConstraint::NodePropertyRequired {
            label: Label::new("Person"),
            key: "name".to_string(),
        }]
    );
    assert_eq!(
        registry.constraints(),
        vec![
            GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "email".to_string(),
            },
            GraphConstraint::NodePropertyRequired {
                label: Label::new("Person"),
                key: "name".to_string(),
            },
        ]
    );
}

#[test]
fn cypher_constraint_registry_serializes_for_external_persistence() {
    let base = GraphSchema::builder()
        .required_node_property("Person", "id")
        .build();
    let mut registry = CypherConstraintRegistry::from_schema(&base);
    registry
        .apply_cypher(
            "CREATE CONSTRAINT person_email \
                 FOR (n:Person) REQUIRE n.email IS UNIQUE",
        )
        .expect("create named constraint");

    let json = registry.to_json().expect("serialize registry");
    assert!(json.contains("person_email"));
    let round_trip = CypherConstraintRegistry::from_json(&json).expect("deserialize registry");
    assert_eq!(round_trip, registry);
    assert_eq!(
        round_trip.constraints(),
        vec![
            GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "email".to_string(),
            },
            GraphConstraint::NodePropertyRequired {
                label: Label::new("Person"),
                key: "id".to_string(),
            },
        ]
    );

    let error = CypherConstraintRegistry::from_json("{not json}")
        .expect_err("invalid registry JSON should fail");
    assert!(matches!(error, GrustError::Serialization(_)), "{error:?}");
}

#[test]
fn cypher_constraint_registry_projects_into_existing_schema() {
    let base = GraphSchema::builder()
        .node(
            "Person",
            vec![
                Field::required("id", FieldType::String),
                Field::optional("email", FieldType::String),
            ],
        )
        .edge(
            "KNOWS",
            vec![Label::new("Person")],
            vec![Label::new("Person")],
            vec![Field::optional("since", FieldType::Int)],
        )
        .build();
    let mut registry = CypherConstraintRegistry::new();
    registry
        .apply_cypher(
            "CREATE CONSTRAINT person_email \
                 FOR (n:Person) REQUIRE n.email IS UNIQUE",
        )
        .expect("create constraint");

    let schema = registry.apply_to_schema(&base);
    assert_eq!(schema.nodes, base.nodes);
    assert_eq!(schema.edges, base.edges);
    assert_eq!(
        schema.constraints,
        vec![GraphConstraint::NodePropertyUnique {
            label: Label::new("Person"),
            key: "email".to_string(),
        }]
    );
}

#[test]
fn cypher_constraint_registry_can_start_from_schema_constraints() {
    let base = GraphSchema::builder()
        .node("Person", vec![Field::required("id", FieldType::String)])
        .required_node_property("Person", "id")
        .build();
    let mut registry = CypherConstraintRegistry::from_schema(&base);
    registry
        .apply_cypher(
            "CREATE CONSTRAINT person_email \
                 FOR (n:Person) REQUIRE n.email IS UNIQUE",
        )
        .expect("create named constraint");

    let schema = registry.apply_to_schema(&base);
    assert_eq!(
        schema.constraints,
        vec![
            GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "email".to_string(),
            },
            GraphConstraint::NodePropertyRequired {
                label: Label::new("Person"),
                key: "id".to_string(),
            },
        ]
    );
}

#[test]
fn cypher_constraint_registry_batches_are_atomic() {
    let mut registry = CypherConstraintRegistry::new();
    let error = registry
        .apply_cypher(
            "CREATE CONSTRAINT person_id FOR (n:Person) REQUIRE n.id IS UNIQUE; \
                 DROP CONSTRAINT missing",
        )
        .expect_err("failing batch should reject");
    assert!(matches!(error, GrustError::CypherExecution(_)), "{error:?}");
    assert!(registry.constraints().is_empty());
}

#[test]
fn cypher_ddl_schema_helper_applies_schema_to_store() {
    let store = MemoryGraphStore::new();
    let schema = GraphSchema::builder()
        .node(
            "Person",
            vec![
                Field::required("id", FieldType::String),
                Field::optional("email", FieldType::String),
            ],
        )
        .build();
    let mut registry = CypherConstraintRegistry::from_schema(&schema);

    let applied = futures_executor::block_on(apply_cypher_ddl_to_schema(
        &store,
        &schema,
        &mut registry,
        "CREATE CONSTRAINT person_email FOR (n:Person) REQUIRE n.email IS UNIQUE",
    ))
    .expect("apply DDL to schema and store");

    assert_eq!(
        applied.report,
        CypherDdlApplicationReport {
            created: 1,
            ..Default::default()
        }
    );
    assert_eq!(
        applied.schema.constraints,
        vec![GraphConstraint::NodePropertyUnique {
            label: Label::new("Person"),
            key: "email".to_string(),
        }]
    );

    futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "p1",
        Props::from([("email".to_string(), Value::from("ada@example.test"))]),
    )))
    .expect("first unique email");
    let error = futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "p2",
        Props::from([("email".to_string(), Value::from("ada@example.test"))]),
    )))
    .expect_err("applied schema should reject duplicate unique property");
    assert!(matches!(error, GrustError::Schema(_)), "{error:?}");
}

#[test]
fn cypher_ddl_schema_helper_does_not_mutate_registry_when_store_rejects_schema() {
    let store = MemoryGraphStore::new();
    futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "p1",
        Props::from([("email".to_string(), Value::from("same@example.test"))]),
    )))
    .expect("first node");
    futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "p2",
        Props::from([("email".to_string(), Value::from("same@example.test"))]),
    )))
    .expect("second node");

    let schema = GraphSchema::builder()
        .node("Person", vec![Field::optional("email", FieldType::String)])
        .build();
    let mut registry = CypherConstraintRegistry::from_schema(&schema);
    let error = futures_executor::block_on(apply_cypher_ddl_to_schema(
        &store,
        &schema,
        &mut registry,
        "CREATE CONSTRAINT person_email FOR (n:Person) REQUIRE n.email IS UNIQUE",
    ))
    .expect_err("schema validation should reject existing duplicates");

    assert!(matches!(error, GrustError::Schema(_)), "{error:?}");
    assert!(registry.constraints().is_empty());
}

#[test]
fn cypher_native_constraint_helper_applies_memory_constraints() {
    let store = MemoryGraphStore::new();

    let applied = futures_executor::block_on(apply_cypher_native_constraints(
        &store,
        "CREATE CONSTRAINT person_email IF NOT EXISTS FOR (n:Person) REQUIRE n.email IS UNIQUE; \
             CREATE CONSTRAINT FOR (n:Person) REQUIRE n.email IS NOT NULL",
    ))
    .expect("apply native constraints");

    assert_eq!(
        applied,
        GraphNativeConstraintReport {
            applied: 2,
            skipped: 0,
        }
    );

    let skipped = futures_executor::block_on(apply_cypher_native_constraints(
        &store,
        "CREATE CONSTRAINT person_email IF NOT EXISTS FOR (n:Person) REQUIRE n.email IS UNIQUE",
    ))
    .expect("skip duplicate native constraint");
    assert_eq!(
        skipped,
        GraphNativeConstraintReport {
            applied: 0,
            skipped: 1,
        }
    );

    let error = futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "missing-email",
        Props::new(),
    )))
    .expect_err("native required property should reject future writes");
    assert!(matches!(error, GrustError::Schema(_)), "{error:?}");
    assert!(
        error
            .to_string()
            .contains("missing native required constrained property 'email'")
    );

    futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "p1",
        Props::from([("email".to_string(), Value::from("ada@example.test"))]),
    )))
    .expect("first native unique value");
    let error = futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "p2",
        Props::from([("email".to_string(), Value::from("ada@example.test"))]),
    )))
    .expect_err("native unique property should reject future duplicates");
    assert!(matches!(error, GrustError::Schema(_)), "{error:?}");
    assert!(
        error
            .to_string()
            .contains("duplicates native unique constrained property 'email'")
    );
}

#[test]
fn cypher_native_constraint_helper_rejects_drop_constraint() {
    let store = MemoryGraphStore::new();

    let error = futures_executor::block_on(apply_cypher_native_constraints(
        &store,
        "DROP CONSTRAINT person_email",
    ))
    .expect_err("native constraint helper does not drop constraints");

    assert!(matches!(error, GrustError::CypherSyntax(_)), "{error:?}");
    assert!(
        error
            .to_string()
            .contains("does not support DROP CONSTRAINT")
    );
}

#[test]
fn cypher_schema_manager_applies_ddl_and_exports_registry() {
    let store = MemoryGraphStore::new();
    let schema = GraphSchema::builder()
        .node(
            "Person",
            vec![
                Field::required("id", FieldType::String),
                Field::optional("email", FieldType::String),
            ],
        )
        .build();
    let mut manager = CypherSchemaManager::new(schema);

    let applied = futures_executor::block_on(manager.apply_cypher_ddl(
        &store,
        "CREATE CONSTRAINT person_email FOR (n:Person) REQUIRE n.email IS UNIQUE",
    ))
    .expect("manager applies DDL");

    assert_eq!(
        applied.report,
        CypherDdlApplicationReport {
            created: 1,
            ..Default::default()
        }
    );
    assert_eq!(manager.schema, applied.schema);
    assert_eq!(
        manager.registry.named_constraints(),
        vec![NamedGraphConstraint {
            name: "person_email".to_string(),
            constraint: GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "email".to_string(),
            },
        }]
    );

    let registry_json = manager.registry_json().expect("export registry");
    let imported = CypherSchemaManager::from_registry_json(manager.schema.clone(), &registry_json)
        .expect("import registry");
    assert_eq!(imported, manager);
}

#[test]
fn cypher_schema_manager_keeps_state_when_schema_apply_fails() {
    let store = MemoryGraphStore::new();
    futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "p1",
        Props::from([("email".to_string(), Value::from("same@example.test"))]),
    )))
    .expect("first node");
    futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "p2",
        Props::from([("email".to_string(), Value::from("same@example.test"))]),
    )))
    .expect("second node");

    let schema = GraphSchema::builder()
        .node("Person", vec![Field::optional("email", FieldType::String)])
        .build();
    let mut manager = CypherSchemaManager::new(schema);
    let before = manager.clone();

    let error = futures_executor::block_on(manager.apply_cypher_ddl(
        &store,
        "CREATE CONSTRAINT person_email FOR (n:Person) REQUIRE n.email IS UNIQUE",
    ))
    .expect_err("manager should surface backend schema validation failure");

    assert!(matches!(error, GrustError::Schema(_)), "{error:?}");
    assert_eq!(manager, before);
}

#[test]
fn cypher_returning_orders_skips_and_limits_rows() {
    let store = MemoryGraphStore::new();
    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'a', age: 30});
                CREATE (:Person {id: 'b', age: 20});
                CREATE (:Person {id: 'c', age: 40});
                MATCH (n:Person) SET n.seen = true
                RETURN n.id, n.age ORDER BY n.age DESC SKIP 1 LIMIT 1;
                ",
            CypherMutationOptions::default(),
        ))
        .unwrap();
    assert_eq!(
        result.table.columns,
        vec!["n.id".to_string(), "n.age".to_string()]
    );
    // ages descending: c(40), a(30), b(20); SKIP 1 drops c; LIMIT 1 keeps a.
    assert_eq!(
        result.table.rows,
        vec![vec![Value::from("a"), Value::Int(30)]]
    );
}

#[test]
fn cypher_returning_orders_ascending_with_alias() {
    let store = MemoryGraphStore::new();
    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'a', age: 30});
                CREATE (:Person {id: 'b', age: 20});
                CREATE (:Person {id: 'c', age: 40});
                MATCH (n:Person) SET n.seen = true
                RETURN n.id AS id, n.age AS age ORDER BY age;
                ",
            CypherMutationOptions::default(),
        ))
        .unwrap();
    assert_eq!(
        result.table.columns,
        vec!["id".to_string(), "age".to_string()]
    );
    // Ascending by age: b(20), a(30), c(40).
    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("b"), Value::Int(20)],
            vec![Value::from("a"), Value::Int(30)],
            vec![Value::from("c"), Value::Int(40)],
        ]
    );
}

#[test]
fn cypher_row_producing_match_create_and_merge_execute_on_memory_facade() {
    let plan = sail_cypher_mutation_plan(
        "
            CREATE (:Person {id: 'ada', status: 'active', score: 11});
            CREATE (:Person {id: 'bob', status: 'active', score: 9});
            CREATE (:Team {id: 'eng'});
            MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
            WHERE a.score >= 10
            CREATE (a)-[:MEMBER_OF {source: 'cypher'}]->(b);
            MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
            WHERE a.score >= 10
            MERGE (a)-[:MEMBER_OF {source: 'merge'}]->(b);
            ",
    )
    .unwrap();
    let store = MemoryGraphStore::new();

    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&plan)).unwrap();

    assert_eq!(
        report,
        GraphMutationReport {
            creates: 4,
            merges: 1,
            matched_rows: 2,
            changed_nodes: 3,
            changed_edges: 2,
            node_upserts: 3,
            edge_upserts: 2,
            node_inserts: 3,
            edge_inserts: 1,
            edge_updates: 1,
            ..GraphMutationReport::default()
        }
    );
    let edges = futures_executor::block_on(store.get_edges(EdgeQuery {
        from: Some(NodeId::new("ada")),
        to: Some(NodeId::new("eng")),
        label: Some(Label::new("MEMBER_OF")),
    }))
    .unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].props.get("source"), Some(&Value::from("merge")));
}
