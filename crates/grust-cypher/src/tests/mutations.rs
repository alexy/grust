//! mutations tests (split verbatim from the former monolithic tests.rs).
use super::*;

#[test]
fn cypher_mutation_options_default_to_upsert_compatible_create() {
    assert_eq!(
        CypherMutationOptions::default(),
        CypherMutationOptions {
            create_mode: CypherCreateMode::UpsertCompatible,
            node_id_policy: CypherNodeIdPolicy::ExplicitOnly,
            relationship_id_policy: CypherRelationshipIdPolicy::ExplicitOnly,
            collect_written_node_identities: false,
            collect_written_edge_identities: false,
            null_assignment: CypherNullAssignment::StoreNull,
            parameters: CypherParameters::new(),
        }
    );
}

#[test]
fn cypher_parser_classifies_top_level_mutation_statements() {
    use super::cypher_parser::CypherStatement;

    assert_eq!(
        super::cypher_parser::classify_statement("MATCH (n) DELETE n").unwrap(),
        CypherStatement::Match("(n) DELETE n")
    );
    assert_eq!(
        super::cypher_parser::classify_statement("create (:Person {id: 'p'})").unwrap(),
        CypherStatement::Create("(:Person {id: 'p'})")
    );
    assert_eq!(
        super::cypher_parser::classify_statement("MERGE (:Person {id: 'p'})").unwrap(),
        CypherStatement::Merge("(:Person {id: 'p'})")
    );
    assert_eq!(
        super::cypher_parser::classify_statement("DELETE (:Person {id: 'p'})").unwrap(),
        CypherStatement::Delete("(:Person {id: 'p'})")
    );

    let error =
        super::cypher_parser::classify_statement("SET n.name = 'Ada'").expect_err("bare SET");
    assert!(matches!(error, GrustError::CypherSyntax(_)));

    let error = super::cypher_parser::classify_statement("RETURN 1").expect_err("read query");
    assert!(matches!(error, GrustError::CypherSyntax(_)));
}

#[test]
fn strict_create_edge_conflicts_on_sail_write_identity() {
    let structural = Edge::new("KNOWS", "person-1", "person-2", Props::new());
    let explicit = Edge::new("KNOWS", "person-1", "person-2", Props::new()).with_id("edge-1");
    let same_id_elsewhere =
        Edge::new("KNOWS", "person-3", "person-4", Props::new()).with_id("edge-1");
    let same_structural_different_id =
        Edge::new("KNOWS", "person-1", "person-2", Props::new()).with_id("edge-2");
    let unrelated = Edge::new("KNOWS", "person-2", "person-3", Props::new()).with_id("edge-3");

    assert!(strict_create_edge_conflicts(
        &structural,
        &[same_structural_different_id.clone()]
    ));
    assert!(strict_create_edge_conflicts(
        &explicit,
        &[same_id_elsewhere]
    ));
    assert!(strict_create_edge_conflicts(
        &explicit,
        &[same_structural_different_id]
    ));
    assert!(!strict_create_edge_conflicts(&explicit, &[unrelated]));
}

#[test]
fn strict_create_plan_conflicts_reject_duplicate_concrete_create_targets() {
    let duplicate_nodes = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new("Person", "ada", Props::new()),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new("Person", "ada", Props::new()),
        },
    ]);
    let error = check_strict_create_plan_conflicts(&duplicate_nodes)
        .expect_err("duplicate CREATE node should fail");
    assert!(error.to_string().contains("duplicate node 'ada'"));

    let duplicate_structural_edges = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Create,
            edge: Edge::new("KNOWS", "ada", "bob", Props::new()).with_id("edge-1"),
        },
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Create,
            edge: Edge::new("KNOWS", "ada", "bob", Props::new()).with_id("edge-2"),
        },
    ]);
    let error = check_strict_create_plan_conflicts(&duplicate_structural_edges)
        .expect_err("duplicate CREATE structural edge should fail");
    assert!(error.to_string().contains("duplicate edge 'edge-2'"));

    let duplicate_explicit_edges = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Create,
            edge: Edge::new("KNOWS", "ada", "bob", Props::new()).with_id("edge-1"),
        },
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Create,
            edge: Edge::new("LIKES", "ada", "carol", Props::new()).with_id("edge-1"),
        },
    ]);
    let error = check_strict_create_plan_conflicts(&duplicate_explicit_edges)
        .expect_err("duplicate CREATE explicit edge id should fail");
    assert!(error.to_string().contains("duplicate edge 'edge-1'"));
}

#[test]
fn cypher_node_create_requires_explicit_id_and_lowers_to_mutation() {
    let plan =
        sail_cypher_mutation_plan("CREATE (n:Person {id: 'person-1', name: 'Ada', age: 36})")
            .unwrap();

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            creates: 1,
            changed_nodes: 1,
            node_upserts: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        plan.into_mutations(),
        vec![GraphMutation::UpsertNode(Node::new(
            "Person",
            "person-1",
            Props::from([
                ("age".to_string(), Value::Int(36)),
                ("id".to_string(), Value::String("person-1".to_string())),
                ("name".to_string(), Value::String("Ada".to_string())),
            ]),
        ))]
    );

    let error = sail_cypher_mutation_plan("CREATE (:Person {name: 'Ada'})")
        .expect_err("missing id should fail");
    assert!(
        error
            .to_string()
            .contains("requires explicit string property 'id'")
    );
}

#[test]
fn cypher_edge_detection_ignores_arrow_inside_string_literals() {
    let create = sail_cypher_mutation_plan("CREATE (:Server {id: 'prod->primary'})").unwrap();
    assert_eq!(
        create.into_mutations(),
        vec![GraphMutation::UpsertNode(Node::new(
            "Server",
            "prod->primary",
            Props::from([("id".to_string(), Value::String("prod->primary".to_string()))]),
        ))]
    );

    let merge = sail_cypher_mutation_plan("MERGE (:Server {id: 'prod->primary'})").unwrap();
    assert_eq!(
        merge.into_mutations(),
        vec![GraphMutation::UpsertNode(Node::new(
            "Server",
            "prod->primary",
            Props::from([("id".to_string(), Value::String("prod->primary".to_string()))]),
        ))]
    );

    let delete = sail_cypher_mutation_plan("DELETE (:Server {id: 'prod->primary'})").unwrap();
    assert_eq!(
        delete.into_mutations(),
        vec![GraphMutation::DeleteNode(NodeId::new("prod->primary"))]
    );

    let edge = sail_cypher_mutation_plan(
        "CREATE (:Server {id: 'a'})-[:ROUTES {note: 'a->b'}]->(:Server {id: 'b'})",
    )
    .unwrap();
    assert_eq!(
        edge.into_mutations(),
        vec![GraphMutation::UpsertEdge(Edge::new(
            "ROUTES",
            "a",
            "b",
            Props::from([("note".to_string(), Value::from("a->b"))]),
        ))]
    );
}

#[test]
fn cypher_parameters_bind_literal_values_only() {
    let options = CypherMutationOptions {
        parameters: CypherParameters::from([
            ("id".to_string(), Value::from("person-1")),
            ("name".to_string(), Value::from("Ada")),
            ("age".to_string(), Value::Int(36)),
            ("active".to_string(), Value::Bool(true)),
            ("note".to_string(), Value::Null),
        ]),
        ..CypherMutationOptions::default()
    };
    let plan = sail_cypher_mutation_plan_with_options(
        "
            CREATE (:Person {id: $id, name: $name, age: $age, active: $active, note: $note});
            MATCH (n:Person {id: $id}) SET n.name = $name;
            MATCH (n:Person {id: $id}) SET n.quoted = '$name';
            ",
        options,
    )
    .unwrap()
    .0;

    assert_eq!(
        plan.into_mutations(),
        vec![
            GraphMutation::UpsertNode(Node::new(
                "Person",
                "person-1",
                Props::from([
                    ("active".to_string(), Value::Bool(true)),
                    ("age".to_string(), Value::Int(36)),
                    ("id".to_string(), Value::from("person-1")),
                    ("name".to_string(), Value::from("Ada")),
                    ("note".to_string(), Value::Null),
                ]),
            )),
            GraphMutation::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([("name".to_string(), Value::from("Ada"))]),
            },
            GraphMutation::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([("quoted".to_string(), Value::from("$name"))]),
            },
        ]
    );

    let missing = sail_cypher_mutation_plan_with_options(
        "CREATE (:Person {id: $missing})",
        CypherMutationOptions::default(),
    )
    .expect_err("missing parameter should fail");
    assert!(matches!(missing, GrustError::CypherUnresolvedIdentity(_)));

    let wrong_id_type = sail_cypher_mutation_plan_with_options(
        "CREATE (:Person {id: $id})",
        CypherMutationOptions {
            parameters: CypherParameters::from([("id".to_string(), Value::Int(1))]),
            ..CypherMutationOptions::default()
        },
    )
    .expect_err("non-string id parameter should fail");
    assert!(matches!(
        wrong_id_type,
        GrustError::CypherUnresolvedIdentity(_)
    ));
}

#[test]
fn cypher_generated_node_id_policy_is_opt_in_for_create_only() {
    let (plan, generated) = sail_cypher_mutation_plan_with_options(
        "CREATE (n:Person {name: 'Ada'})",
        CypherMutationOptions {
            node_id_policy: CypherNodeIdPolicy::GenerateForCreate,
            ..CypherMutationOptions::default()
        },
    )
    .unwrap();

    assert_eq!(generated.len(), 1);
    assert_eq!(generated[0].variable.as_deref(), Some("n"));
    assert!(generated[0].id.as_str().starts_with("node-"));
    assert_eq!(
        plan.report(),
        GraphMutationReport {
            creates: 1,
            changed_nodes: 1,
            node_upserts: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(plan.operations.len(), 1);
    let GraphMutationPlanOp::UpsertNode { kind, node } = &plan.operations[0] else {
        panic!("generated node CREATE should lower to node upsert");
    };
    assert_eq!(*kind, GraphMutationPlanKind::Create);
    assert_eq!(node.id, generated[0].id);
    assert_eq!(node.props.get("id"), Some(&Value::from(node.id.as_str())));
    assert_eq!(node.props.get("name"), Some(&Value::from("Ada")));

    let error = sail_cypher_mutation_plan_with_options(
        "MERGE (:Person {name: 'Ada'})",
        CypherMutationOptions {
            node_id_policy: CypherNodeIdPolicy::GenerateForCreate,
            ..CypherMutationOptions::default()
        },
    )
    .expect_err("MERGE must still require a stable explicit id");
    assert!(matches!(error, GrustError::CypherUnresolvedIdentity(_)));

    let error = sail_cypher_mutation_plan_with_options(
        "CREATE (:Person {name: 'Ada'})-[:KNOWS]->(:Person {id: 'person-2'})",
        CypherMutationOptions {
            node_id_policy: CypherNodeIdPolicy::GenerateForCreate,
            ..CypherMutationOptions::default()
        },
    )
    .expect_err("edge endpoints must still resolve before writing");
    assert!(matches!(error, GrustError::CypherUnresolvedIdentity(_)));
}

#[test]
fn cypher_generated_node_id_can_bind_local_create_variable() {
    let (plan, generated) = sail_cypher_mutation_plan_with_options(
        "
            CREATE (a:Person {name: 'Ada'});
            CREATE (:Person {id: 'person-2'});
            CREATE (a)-[:KNOWS]->(:Person {id: 'person-2'});
            ",
        CypherMutationOptions {
            node_id_policy: CypherNodeIdPolicy::GenerateForCreate,
            ..CypherMutationOptions::default()
        },
    )
    .unwrap();

    assert_eq!(generated.len(), 1);
    assert_eq!(generated[0].variable.as_deref(), Some("a"));
    assert_eq!(plan.operations.len(), 3);
    let GraphMutationPlanOp::UpsertEdge { edge, .. } = &plan.operations[2] else {
        panic!("third operation should be an edge create");
    };
    assert_eq!(edge.from, generated[0].id);
    assert_eq!(edge.to, NodeId::new("person-2"));
}

#[test]
fn cypher_merge_edge_requires_resolved_endpoint_ids() {
    let plan = sail_cypher_mutation_plan(
            "MERGE (:Person {id: 'person-1'})-[e:KNOWS {id: 'edge-1', since: 2020}]->(:Person {id: 'person-2'})",
        )
        .unwrap();

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            merges: 1,
            changed_edges: 1,
            edge_upserts: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        plan.into_mutations(),
        vec![GraphMutation::UpsertEdge(
            Edge::new(
                "KNOWS",
                "person-1",
                "person-2",
                Props::from([
                    ("id".to_string(), Value::String("edge-1".to_string())),
                    ("since".to_string(), Value::Int(2020)),
                ]),
            )
            .with_id("edge-1")
        )]
    );

    let error = sail_cypher_mutation_plan(
        "CREATE (:Person {name: 'Ada'})-[:KNOWS]->(:Person {id: 'person-2'})",
    )
    .expect_err("unresolved source id should fail");
    assert!(error.to_string().contains("edge mutation source node"));
}

#[test]
fn cypher_delete_lowers_resolved_node_and_edge_patterns() {
    let node_delete = sail_cypher_mutation_plan("DELETE (:Person {id: 'person-1'})").unwrap();
    assert_eq!(
        node_delete.into_mutations(),
        vec![GraphMutation::DeleteNode(NodeId::new("person-1"))]
    );

    let edge_delete = sail_cypher_mutation_plan(
        "DELETE (:Person {id: 'person-1'})-[:KNOWS]->(:Person {id: 'person-2'})",
    )
    .unwrap();
    assert_eq!(
        edge_delete.into_mutations(),
        vec![GraphMutation::DeleteEdge {
            from: NodeId::new("person-1"),
            label: Label::new("KNOWS"),
            to: NodeId::new("person-2"),
        }]
    );
}
