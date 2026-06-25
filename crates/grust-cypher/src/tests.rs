//! Unit tests for grust-cypher (relocated from lib.rs in Unit 2a of docs/GQL_GOAL.md).
//!
//! Moved verbatim out of the 32,960-line lib.rs to shrink the crate root; the
//! `use super::*` below still resolves to the crate root, so every item these
//! tests reach (public or private) is unchanged. No test was added, removed, or
//! modified by the move.
#![cfg(test)]

    use super::*;
    use grust_memory::MemoryGraphStore;

    fn is_cypher_planning_error(error: &GrustError) -> bool {
        matches!(
            error,
            GrustError::CypherSyntax(_)
                | GrustError::CypherUnresolvedIdentity(_)
                | GrustError::CypherUnsupportedCardinality(_)
                | GrustError::Unsupported(_)
        )
    }

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

    #[test]
    fn cypher_match_delete_lowers_id_resolved_patterns() {
        let node_delete =
            sail_cypher_mutation_plan("MATCH (n:Person {id: 'person-1'}) DELETE n").unwrap();
        assert_eq!(
            node_delete.report(),
            GraphMutationReport {
                deletes: 1,
                changed_nodes: 1,
                node_deletes: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            node_delete.into_mutations(),
            vec![GraphMutation::DeleteNode(NodeId::new("person-1"))]
        );

        let edge_delete = sail_cypher_mutation_plan(
            "MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {id: 'person-2'}) DELETE e",
        )
        .unwrap();
        assert_eq!(
            edge_delete.report(),
            GraphMutationReport {
                deletes: 1,
                changed_edges: 1,
                edge_deletes: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            edge_delete.into_mutations(),
            vec![GraphMutation::DeleteEdge {
                from: NodeId::new("person-1"),
                label: Label::new("KNOWS"),
                to: NodeId::new("person-2"),
            }]
        );

        let broad_edge = sail_cypher_mutation_plan(
            "MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {status: 'inactive'}) DELETE e",
        )
        .unwrap();
        let relationship = GraphRelationshipMatch {
            from: GraphNodeMatch {
                label: Some(Label::new("Person")),
                props: Props::from([("id".to_string(), Value::from("person-1"))]),
                predicates: Vec::new(),
            },
            label: Label::new("KNOWS"),
            to: GraphNodeMatch {
                label: Some(Label::new("Person")),
                props: Props::from([("status".to_string(), Value::from("inactive"))]),
                predicates: Vec::new(),
            },
            id: None,
            props: Props::new(),
            predicates: Vec::new(),
        };
        assert_eq!(
            broad_edge.report(),
            GraphMutationReport {
                deletes: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            broad_edge.into_mutations(),
            vec![GraphMutation::DeleteMatchingEdges { relationship }]
        );
    }

    #[test]
    fn cypher_match_delete_lowers_multiple_relationship_pattern_targets() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (a:Person {id: 'person-1'})-[e:KNOWS]->(b:Person {id: 'person-2'}) DELETE e, a",
        )
        .unwrap();
        assert_eq!(
            plan.report(),
            GraphMutationReport {
                deletes: 2,
                changed_nodes: 1,
                changed_edges: 1,
                node_deletes: 1,
                edge_deletes: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            plan.into_mutations(),
            vec![
                GraphMutation::DeleteEdge {
                    from: NodeId::new("person-1"),
                    label: Label::new("KNOWS"),
                    to: NodeId::new("person-2"),
                },
                GraphMutation::DeleteNode(NodeId::new("person-1")),
            ]
        );
    }

    #[test]
    fn cypher_match_delete_lowers_relationship_row_endpoint_target() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (a:Person {status: 'inactive'})-[e:KNOWS]->(b:Person {id: 'person-2'}) DELETE a",
        )
        .unwrap();
        let relationship = GraphRelationshipMatch {
            from: GraphNodeMatch {
                label: Some(Label::new("Person")),
                props: Props::from([("status".to_string(), Value::from("inactive"))]),
                predicates: Vec::new(),
            },
            label: Label::new("KNOWS"),
            to: GraphNodeMatch {
                label: Some(Label::new("Person")),
                props: Props::from([("id".to_string(), Value::from("person-2"))]),
                predicates: Vec::new(),
            },
            id: None,
            props: Props::new(),
            predicates: Vec::new(),
        };

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::DeleteRelationshipRows {
                relationship,
                delete_edges: false,
                endpoint_nodes: vec![GraphRelationshipEndpoint::From],
                target_count: 1,
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_delete_lowers_mixed_relationship_row_targets() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (a:Person {status: 'inactive'})-[e:KNOWS]->(b:Person {id: 'person-2'}) DELETE e, a",
        )
        .unwrap();
        let relationship = GraphRelationshipMatch {
            from: GraphNodeMatch {
                label: Some(Label::new("Person")),
                props: Props::from([("status".to_string(), Value::from("inactive"))]),
                predicates: Vec::new(),
            },
            label: Label::new("KNOWS"),
            to: GraphNodeMatch {
                label: Some(Label::new("Person")),
                props: Props::from([("id".to_string(), Value::from("person-2"))]),
                predicates: Vec::new(),
            },
            id: None,
            props: Props::new(),
            predicates: Vec::new(),
        };

        assert_eq!(
            plan.report(),
            GraphMutationReport {
                deletes: 2,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::DeleteRelationshipRows {
                relationship,
                delete_edges: true,
                endpoint_nodes: vec![GraphRelationshipEndpoint::From],
                target_count: 2,
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_delete_lowers_broad_node_patterns_with_cardinality() {
        let bounded =
            sail_cypher_mutation_plan("MATCH (n:Person {active: false}) DELETE n").unwrap();
        assert_eq!(
            bounded.report(),
            GraphMutationReport {
                deletes: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            bounded.operations,
            vec![GraphMutationPlanOp::DeleteMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::from([("active".to_string(), Value::Bool(false))]),
                predicates: Vec::new(),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
        assert_eq!(
            bounded.into_mutations(),
            vec![GraphMutation::DeleteMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::from([("active".to_string(), Value::Bool(false))]),
                predicates: Vec::new(),
            }]
        );

        let unbounded = sail_cypher_mutation_plan("MATCH (n) DELETE n").unwrap();
        assert_eq!(
            unbounded.operations,
            vec![GraphMutationPlanOp::DeleteMatchingNodes {
                label: None,
                props: Props::new(),
                predicates: Vec::new(),
                cardinality: GraphMutationCardinality::UnboundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_delete_rejects_unresolved_or_mismatched_patterns() {
        for cypher in [
            "MATCH (n:Person {id: 'person-1'}) DELETE m",
            "MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {id: 'person-2'}) DELETE n",
            "MATCH (:Person {id: 'person-1'})-[:KNOWS]->(:Person {id: 'person-2'}) DELETE e",
            "MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {id: 'person-2'}) DELETE e,",
        ] {
            let error = sail_cypher_mutation_plan(cypher).expect_err("unsupported MATCH must fail");
            assert!(is_cypher_planning_error(&error));
        }
    }

    #[test]
    fn cypher_match_where_lowers_node_predicates() {
        let plan = sail_cypher_mutation_plan_with_options(
            "MATCH (n:Person) WHERE (n.status = 'inactive' AND n.score >= $min) AND NOT (n.active = true) AND (n.nickname IS NOT NULL) AND n.name STARTS WITH 'Ad' AND n.team IN $teams SET n.archived = true",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    ("min".to_string(), Value::Int(10)),
                    (
                        "teams".to_string(),
                        Value::from(vec!["eng".to_string(), "data".to_string()]),
                    ),
                ]),
                ..CypherMutationOptions::default()
            },
        )
        .unwrap()
        .0;

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("inactive"),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::Int(10),
                    },
                    GraphPropertyPredicate {
                        key: "active".to_string(),
                        op: GraphPredicateOp::NotEqual,
                        value: Value::Bool(true),
                    },
                    GraphPropertyPredicate {
                        key: "nickname".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::StartsWith,
                        value: Value::from("Ad"),
                    },
                    GraphPropertyPredicate {
                        key: "team".to_string(),
                        op: GraphPredicateOp::In,
                        value: Value::from(vec!["eng".to_string(), "data".to_string()]),
                    },
                ],
                patch: Props::from([("archived".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_folds_same_property_or_to_in_predicate() {
        let plan = sail_cypher_mutation_plan_with_options(
            "MATCH (n:Person) WHERE (n.status = 'active' OR n.status = $status) AND n.kind = 'person' SET n.reviewed = true",
            CypherMutationOptions {
                parameters: CypherParameters::from([("status".to_string(), Value::from("pending"))]),
                ..CypherMutationOptions::default()
            },
        )
        .unwrap()
        .0;

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::In,
                        value: Value::Json(serde_json::json!(["active", "pending"])),
                    },
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    }
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_folds_negated_same_property_or_to_not_in_predicate() {
        let plan = sail_cypher_mutation_plan_with_options(
            "MATCH (n:Person) WHERE NOT (n.status = 'blocked' OR n.status = $status) AND n.kind = 'person' SET n.reviewed = true",
            CypherMutationOptions {
                parameters: CypherParameters::from([("status".to_string(), Value::from("archived"))]),
                ..CypherMutationOptions::default()
            },
        )
        .unwrap()
        .0;

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!(["blocked", "archived"])),
                    },
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    }
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_folds_same_property_in_or_equal_predicates() {
        let plan = sail_cypher_mutation_plan_with_options(
            "MATCH (n:Person) WHERE (n.status IN ['active', $status] OR n.status = 'review') SET n.reviewed = true",
            CypherMutationOptions {
                parameters: CypherParameters::from([("status".to_string(), Value::from("pending"))]),
                ..CypherMutationOptions::default()
            },
        )
        .unwrap()
        .0;

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::Json(serde_json::json!(["active", "pending", "review"])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_flattens_nested_foldable_or_predicates() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.status = 'active' OR n.status = 'pending') OR n.status = 'review' SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::Json(serde_json::json!(["active", "pending", "review"])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );

        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.status = 'blocked' OR n.status = 'archived') OR n.status = 'deleted') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::NotIn,
                    value: Value::Json(serde_json::json!(["blocked", "archived", "deleted"])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_deduplicates_folded_membership_values() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.status = 'active' OR n.status = 'active' OR n.status IN ['pending', 'pending'] OR n.status = 'active') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::Json(serde_json::json!(["active", "pending"])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_intersects_same_property_in_predicates() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.status IN ['active', 'pending'] AND n.status IN ['pending', 'review'] SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::Json(serde_json::json!(["pending"])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_empty_in_intersection_to_no_match() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.status IN ['active'] AND n.status IN ['pending'] SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::Json(serde_json::json!([])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_equality_membership_matches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.status = 'pending' AND n.status IN ['active', 'pending'] SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::Equal,
                    value: Value::from("pending"),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_equality_membership_contradictions() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.status = 'blocked' AND n.status IN ['active', 'pending'] SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::Json(serde_json::json!([])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_in_minus_not_in_predicates() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.status IN ['active', 'pending', 'review'] AND NOT n.status IN ['blocked', 'pending'] SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::Json(serde_json::json!(["active", "review"])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_equality_inequality_contradictions() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.status = 'active' AND n.status <> 'active' SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::Json(serde_json::json!([])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_in_minus_not_equal_predicates() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.status IN ['active', 'pending', 'review'] AND n.status <> 'pending' SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::Json(serde_json::json!(["active", "review"])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_repeated_not_equal_predicates() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.status <> 'blocked' AND n.status <> 'archived' SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::NotIn,
                    value: Value::Json(serde_json::json!(["blocked", "archived"])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_stricter_lower_bounds() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.score >= 10 AND n.score > 12 SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "score".to_string(),
                    op: GraphPredicateOp::GreaterThan,
                    value: Value::Int(12),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_stricter_upper_bounds() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.score <= 20 AND n.score < 18 SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "score".to_string(),
                    op: GraphPredicateOp::LessThan,
                    value: Value::Int(18),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_impossible_order_ranges() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.score > 20 AND n.score <= 20 SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "score".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::Json(serde_json::json!([])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_equality_inside_order_range() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.score = 13 AND n.score >= 10 AND n.score < 20 SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "score".to_string(),
                    op: GraphPredicateOp::Equal,
                    value: Value::Int(13),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_equality_outside_order_range() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.score = 9 AND n.score >= 10 SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "score".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::Json(serde_json::json!([])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_in_values_inside_order_range() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.score IN [9, 13, 19] AND n.score > 10 AND n.score < 18 SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "score".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::Json(serde_json::json!([13])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_trailing_in_values_inside_order_range() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.score > 10 AND n.score < 18 AND n.score IN [9, 13, 19] SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "score".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::Json(serde_json::json!([13])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_in_values_outside_order_range() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.score IN [9, 10] AND n.score > 12 SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "score".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::Json(serde_json::json!([])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_folds_negated_same_property_in_or_equal_predicates() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.status IN ['blocked', 'archived'] OR n.status = 'deleted') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::NotIn,
                    value: Value::Json(serde_json::json!(["blocked", "archived", "deleted"])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_double_negation() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT NOT n.active = true SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "active".to_string(),
                    op: GraphPredicateOp::Equal,
                    value: Value::Bool(true),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_folds_same_property_string_or_predicates() {
        let plan = sail_cypher_mutation_plan_with_options(
            "MATCH (n:Person) WHERE (n.name STARTS WITH 'Ad' OR n.name STARTS WITH $prefix) SET n.reviewed = true",
            CypherMutationOptions {
                parameters: CypherParameters::from([("prefix".to_string(), Value::from("Gr"))]),
                ..CypherMutationOptions::default()
            },
        )
        .unwrap()
        .0;

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "name".to_string(),
                    op: GraphPredicateOp::StartsWithAny,
                    value: Value::from(vec!["Ad".to_string(), "Gr".to_string()]),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_unions_same_property_not_in_predicates() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT n.status IN ['blocked'] AND NOT n.status IN ['archived', 'blocked'] SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::NotIn,
                    value: Value::Json(serde_json::json!(["blocked", "archived"])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_deduplicates_folded_string_predicate_needles() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "name".to_string(),
                    op: GraphPredicateOp::StartsWithAny,
                    value: Value::from(vec!["Ad".to_string(), "Gr".to_string()]),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_factors_or_of_and_groups() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.status = 'active') OR (n.kind = 'person' AND n.status = 'pending') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::In,
                        value: Value::Json(serde_json::json!(["active", "pending"])),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_deduplicates_factored_or_branches_before_folding() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.kind = 'person' AND n.status = 'active') OR (n.kind = 'person' AND n.status = 'pending' AND n.status = 'pending') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::In,
                        value: Value::Json(serde_json::json!(["active", "pending"])),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_factors_nested_or_terms_inside_and_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.region = 'us' AND (n.status = 'active' OR n.status = 'pending')) OR (n.region = 'eu' AND (n.status = 'active' OR n.status = 'pending')) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::In,
                        value: Value::Json(serde_json::json!(["active", "pending"])),
                    },
                    GraphPropertyPredicate {
                        key: "region".to_string(),
                        op: GraphPredicateOp::In,
                        value: Value::Json(serde_json::json!(["us", "eu"])),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_factored_or_branch_predicates() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.status = 'active' AND n.status IN ['active', 'pending']) OR (n.kind = 'person' AND n.status = 'review' AND n.status IN ['review', 'archived']) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::In,
                        value: Value::Json(serde_json::json!(["active", "review"])),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_boolean_ast_lowers_unparenthesized_factored_or() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.kind = 'person' AND n.status = 'active' OR n.kind = 'person' AND n.status = 'pending' SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::In,
                        value: Value::Json(serde_json::json!(["active", "pending"])),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_canonicalizes_range_factored_or_branch_predicates() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.score > 10 AND n.score < 18 AND n.score IN [9, 13, 19]) OR (n.kind = 'person' AND n.score = 21) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::In,
                        value: Value::Json(serde_json::json!([13, 21])),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_impossible_factored_or_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.status = 'active') OR (n.kind = 'person' AND n.status = 'blocked' AND n.status = 'active') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("active"),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_collapses_all_impossible_factored_or_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.status = 'active' AND n.status = 'blocked') OR (n.score IN [1] AND n.score > 5) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::Json(serde_json::json!([])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_subsumed_factored_or_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.status = 'active') OR (n.kind = 'person') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "kind".to_string(),
                    op: GraphPredicateOp::Equal,
                    value: Value::from("person"),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_leading_subsuming_factored_or_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person') OR (n.kind = 'person' AND n.status = 'active') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "kind".to_string(),
                    op: GraphPredicateOp::Equal,
                    value: Value::from("person"),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_semantically_subsumed_factored_or_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.status = 'active' AND n.region = 'us') OR (n.kind = 'person' AND n.status IN ['active', 'pending']) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::In,
                        value: Value::Json(serde_json::json!(["active", "pending"])),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_string_subsumed_factored_or_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.name STARTS WITH 'Ad') OR (n.kind = 'person' AND (n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr')) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::StartsWithAny,
                        value: Value::from(vec!["Ad".to_string(), "Gr".to_string()]),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_negated_string_subsumed_factored_or_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND NOT (n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr' OR n.name STARTS WITH 'Al')) OR (n.kind = 'person' AND NOT (n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr')) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotStartsWithAny,
                        value: Value::from(vec!["Ad".to_string(), "Gr".to_string()]),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_null_check_subsumed_factored_or_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.name STARTS WITH 'Ad') OR (n.kind = 'person' AND n.name IS NOT NULL) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );

        let null_plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.nickname = null) OR (n.kind = 'person' AND n.nickname IS NULL) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            null_plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "nickname".to_string(),
                        op: GraphPredicateOp::IsNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_null_check_subsumed_simple_or_terms() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.name STARTS WITH 'Ad' OR n.name IS NOT NULL SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "name".to_string(),
                    op: GraphPredicateOp::IsNotNull,
                    value: Value::Null,
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_negated_equality_subsumed_simple_or_terms() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT n.status = 'active' OR n.status = 'pending' SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::NotEqual,
                    value: Value::from("active"),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_negated_null_check_subsumed_simple_or_terms() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.name STARTS WITH 'Ad' OR n.name IS NOT NULL) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "name".to_string(),
                    op: GraphPredicateOp::IsNull,
                    value: Value::Null,
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_negated_null_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.status = 'inactive' OR n.status = null) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::NotEqual,
                        value: Value::from("inactive"),
                    },
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_negated_null_membership_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.status IN ['inactive', 'paused'] OR n.status = null) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!(["inactive", "paused"])),
                    },
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_nested_negated_null_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.status = 'inactive' OR n.status = 'paused') OR n.status = null) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!(["inactive", "paused"])),
                    },
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_nested_negated_null_membership_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.status IN ['inactive', 'paused'] OR n.status = 'blocked') OR n.status = null) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!(["inactive", "paused", "blocked"])),
                    },
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_negated_null_string_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.name STARTS WITH 'Ad' OR n.name = null) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotStartsWith,
                        value: Value::from("Ad"),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_rejects_negated_null_mixed_property_string_or_terms() {
        let err = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.name STARTS WITH 'Ad' OR n.nickname = null) SET n.reviewed = true",
        )
        .expect_err("mixed-property negated null string OR terms should stay rejected");

        assert!(
            err.to_string().contains("same-property string predicate"),
            "{err}"
        );
    }

    #[test]
    fn cypher_match_where_lowers_nested_negated_null_string_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name = null) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotStartsWithAny,
                        value: Value::from(vec!["Ad".to_string(), "Gr".to_string()]),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_negated_null_string_order_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.name STARTS WITH 'Ad' OR n.name < 'M' OR n.name = null) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotStartsWith,
                        value: Value::from("Ad"),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::from("M"),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_nested_negated_null_string_order_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR n.name = null) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotStartsWithAny,
                        value: Value::from(vec!["Ad".to_string(), "Gr".to_string()]),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::from("M"),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_rejects_negated_null_string_numeric_order_or_terms() {
        let err = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.name STARTS WITH 'Ad' OR n.name < 10 OR n.name = null) SET n.reviewed = true",
        )
        .expect_err("numeric ordered bound mixed with string predicate should stay rejected");

        assert!(
            err.to_string()
                .contains("matching same-property string predicate"),
            "{err}"
        );
    }

    #[test]
    fn cypher_match_where_lowers_negated_null_mixed_string_order_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR n.name IN ['Alan'] OR n.name = null) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotStartsWithAny,
                        value: Value::from(vec!["Ad".to_string(), "Gr".to_string()]),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::from("M"),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!(["Alan"])),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_nested_negated_null_mixed_string_order_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR (n.name IN ['Alan'] OR n.name = 'Bob') OR n.name = null) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotStartsWithAny,
                        value: Value::from(vec!["Ad".to_string(), "Gr".to_string()]),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::from("M"),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!(["Alan", "Bob"])),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_rejects_negated_null_mixed_string_order_numeric_membership() {
        let err = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.name STARTS WITH 'Ad' OR n.name < 'M' OR n.name IN [7] OR n.name = null) SET n.reviewed = true",
        )
        .expect_err("numeric membership mixed with string-domain OR terms should stay rejected");

        assert!(
            err.to_string()
                .contains("matching same-property string predicate"),
            "{err}"
        );
    }

    #[test]
    fn cypher_match_where_lowers_negated_mixed_string_order_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR n.name IN ['Alan']) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotStartsWithAny,
                        value: Value::from(vec!["Ad".to_string(), "Gr".to_string()]),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::from("M"),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!(["Alan"])),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_nested_negated_mixed_string_order_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR (n.name IN ['Alan'] OR n.name = 'Bob')) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotStartsWithAny,
                        value: Value::from(vec!["Ad".to_string(), "Gr".to_string()]),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::from("M"),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!(["Alan", "Bob"])),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_negated_string_order_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.name STARTS WITH 'Ad' OR n.name < 'M') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotStartsWith,
                        value: Value::from("Ad"),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::from("M"),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_nested_negated_string_order_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotStartsWithAny,
                        value: Value::from(vec!["Ad".to_string(), "Gr".to_string()]),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::from("M"),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_negated_mixed_string_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name IN ['Alan']) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotStartsWithAny,
                        value: Value::from(vec!["Ad".to_string(), "Gr".to_string()]),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!(["Alan"])),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_nested_negated_mixed_string_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR (n.name IN ['Alan'] OR n.name = 'Bob')) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotStartsWithAny,
                        value: Value::from(vec!["Ad".to_string(), "Gr".to_string()]),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!(["Alan", "Bob"])),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_negated_string_equality_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.name STARTS WITH 'Ad' OR n.name = 'Alan') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotStartsWith,
                        value: Value::from("Ad"),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotEqual,
                        value: Value::from("Alan"),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_rejects_negated_string_numeric_order_or_terms() {
        let err = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.name STARTS WITH 'Ad' OR n.name < 10) SET n.reviewed = true",
        )
        .expect_err("numeric ordered bound mixed with string predicate should stay rejected");

        assert!(
            err.to_string()
                .contains("matching same-property string predicate"),
            "{err}"
        );
    }

    #[test]
    fn cypher_match_where_rejects_negated_mixed_string_numeric_membership() {
        let err = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.name STARTS WITH 'Ad' OR n.name IN [7]) SET n.reviewed = true",
        )
        .expect_err("numeric membership mixed with string-domain OR terms should stay rejected");

        assert!(
            err.to_string()
                .contains("matching same-property string predicate"),
            "{err}"
        );
    }

    #[test]
    fn cypher_match_where_rejects_negated_mixed_string_order_numeric_membership() {
        let err = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.name STARTS WITH 'Ad' OR n.name < 'M' OR n.name IN [7]) SET n.reviewed = true",
        )
        .expect_err("numeric membership mixed with string-domain OR terms should stay rejected");

        assert!(
            err.to_string()
                .contains("matching same-property string predicate"),
            "{err}"
        );
    }

    #[test]
    fn cypher_match_where_rejects_negated_null_mixed_string_family_or_terms() {
        let err = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.name STARTS WITH 'Ad' OR n.name CONTAINS 'ra' OR n.name = null) SET n.reviewed = true",
        )
        .expect_err("mixed string-family negated null OR terms should stay rejected");

        assert!(
            err.to_string()
                .contains("matching same-property string predicate"),
            "{err}"
        );
    }

    #[test]
    fn cypher_match_where_lowers_negated_null_mixed_string_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name IN ['Alan'] OR n.name = null) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotStartsWithAny,
                        value: Value::from(vec!["Ad".to_string(), "Gr".to_string()]),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!(["Alan"])),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_nested_negated_null_mixed_string_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR (n.name IN ['Alan'] OR n.name = 'Bob') OR n.name = null) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotStartsWithAny,
                        value: Value::from(vec!["Ad".to_string(), "Gr".to_string()]),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!(["Alan", "Bob"])),
                    },
                    GraphPropertyPredicate {
                        key: "name".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_rejects_negated_null_mixed_string_membership_families() {
        let err = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.name STARTS WITH 'Ad' OR n.name CONTAINS 'ra' OR n.name IN ['Alan'] OR n.name = null) SET n.reviewed = true",
        )
        .expect_err("mixed string-family negated null string/membership OR terms should stay rejected");

        assert!(
            err.to_string()
                .contains("matching same-property string predicate"),
            "{err}"
        );
    }

    #[test]
    fn cypher_match_where_lowers_negated_null_order_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.score < 10 OR n.score = null) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::Int(10),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_nested_negated_null_order_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.score < 10 OR n.score > 20) OR n.score = null) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::Int(10),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::LessThanOrEqual,
                        value: Value::Int(20),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_negated_null_mixed_order_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.score < 10 OR n.score IN [15, 18] OR n.score = null) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::Int(10),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!([15, 18])),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_nested_negated_null_mixed_order_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.score < 10 OR n.score > 20) OR n.score IN [15, 18] OR n.score = null) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::Int(10),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::LessThanOrEqual,
                        value: Value::Int(20),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!([15, 18])),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_rejects_incomparable_negated_null_order_or_terms() {
        let err = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.score < 10 OR n.score > 'z' OR n.score = null) SET n.reviewed = true",
        )
        .expect_err("incomparable negated null ordered OR terms should stay rejected");

        assert!(
            err.to_string()
                .contains("MATCH WHERE OR only supports same-property"),
            "{err}"
        );
    }

    #[test]
    fn cypher_match_where_rejects_null_membership_negated_null_mixed_order_or_terms() {
        let err = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.score < 10 OR n.score IN [15, null] OR n.score = null) SET n.reviewed = true",
        )
        .expect_err("null membership in mixed negated ordered OR terms should stay rejected");

        assert!(
            err.to_string()
                .contains("MATCH WHERE IN predicates only support scalar"),
            "{err}"
        );
    }

    #[test]
    fn cypher_match_where_lowers_negated_order_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.score < 10 OR n.score > 20) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::Int(10),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::LessThanOrEqual,
                        value: Value::Int(20),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_nested_negated_order_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.score < 10 OR n.score > 20) OR n.score <= 5) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::Int(10),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::LessThanOrEqual,
                        value: Value::Int(20),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_rejects_mixed_type_negated_order_or_terms() {
        let err = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.score < 10 OR n.score > 'z') SET n.reviewed = true",
        )
        .expect_err("mixed-type negated order OR terms should stay rejected");

        assert!(
            err.to_string()
                .contains("MATCH WHERE OR only supports same-property"),
            "{err}"
        );
    }

    #[test]
    fn cypher_match_where_lowers_negated_mixed_order_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.score < 10 OR n.score = 15) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::Int(10),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::NotEqual,
                        value: Value::Int(15),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_negated_mixed_order_membership_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.score < 10 OR n.score IN [15, 18]) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::Int(10),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!([15, 18])),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_nested_negated_mixed_order_or_terms_to_bounded_and() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.score < 10 OR n.score > 20) OR n.score IN [15, 18]) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::Int(10),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::LessThanOrEqual,
                        value: Value::Int(20),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!([15, 18])),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_rejects_incomparable_negated_mixed_order_or_terms() {
        let err = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.score < 10 OR n.score = 'mid') SET n.reviewed = true",
        )
        .expect_err("incomparable negated mixed order OR terms should stay rejected");

        assert!(
            err.to_string()
                .contains("MATCH WHERE OR only supports same-property"),
            "{err}"
        );
    }

    #[test]
    fn cypher_match_where_prunes_negated_subsumed_factored_or_to_single_predicate() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((n.kind = 'person' AND n.status = 'active') OR n.kind = 'person') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "kind".to_string(),
                    op: GraphPredicateOp::NotEqual,
                    value: Value::from("person"),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_singleton_not_in_subsumed_factored_or_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.status <> 'blocked') OR (n.kind = 'person' AND NOT n.status IN ['blocked']) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!(["blocked"])),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_singleton_in_subsumed_factored_or_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.status IN ['active'] AND n.region = 'us') OR (n.kind = 'person' AND n.status = 'active') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("active"),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prefers_equality_over_singleton_in_factored_or_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.status IN ['active']) OR (n.kind = 'person' AND n.status = 'active') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("active"),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_order_inequality_subsumed_factored_or_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.score > 20 AND n.region = 'us') OR (n.kind = 'person' AND n.score <> 5) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::NotEqual,
                        value: Value::Int(5),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_order_not_in_subsumed_factored_or_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.score > 20 AND n.region = 'us') OR (n.kind = 'person' AND NOT n.score IN [5, 10]) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::NotIn,
                        value: Value::Json(serde_json::json!([5, 10])),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_rejects_incomparable_order_not_in_subsumption() {
        let error = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.score > 20 AND n.region = 'us') OR (n.kind = 'person' AND NOT n.score IN [5, 'ten']) SET n.reviewed = true",
        )
        .expect_err("mixed-type ordered NOT IN subsumption should stay rejected");

        assert!(is_cypher_planning_error(&error) || matches!(error, GrustError::CypherSyntax(_)));
    }

    #[test]
    fn cypher_match_where_prunes_order_subsumed_factored_or_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.score = 13 AND n.region = 'us') OR (n.kind = 'person' AND n.score > 10) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::GreaterThan,
                        value: Value::Int(10),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_stricter_lower_bound_factored_or_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.score > 20 AND n.region = 'us') OR (n.kind = 'person' AND n.score > 10) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::GreaterThan,
                        value: Value::Int(10),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_stricter_upper_bound_factored_or_branches() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.score < 5 AND n.region = 'us') OR (n.kind = 'person' AND n.score <= 10) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::LessThanOrEqual,
                        value: Value::Int(10),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_deduplicates_identical_bounded_predicates() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE n.kind = 'person' AND (n.status = 'active' OR n.status = 'pending') AND n.kind = 'person' AND (n.status = 'active' OR n.status = 'pending') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "kind".to_string(),
                        op: GraphPredicateOp::Equal,
                        value: Value::from("person"),
                    },
                    GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::In,
                        value: Value::Json(serde_json::json!(["active", "pending"])),
                    },
                ],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_folds_negated_same_property_string_or_predicates() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.name CONTAINS 'bot' OR n.name CONTAINS 'test') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "name".to_string(),
                    op: GraphPredicateOp::NotContainsAny,
                    value: Value::from(vec!["bot".to_string(), "test".to_string()]),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_folds_negated_same_property_and_predicates() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.status <> 'active' AND n.status <> 'pending') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::Json(serde_json::json!(["active", "pending"])),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_prunes_negated_and_terms_to_single_predicate() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.status = 'active' AND n.status IN ['active', 'pending']) SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::NotEqual,
                    value: Value::from("active"),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_folds_negated_same_property_string_and_predicates() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (NOT n.name STARTS WITH 'Ad' AND NOT n.name STARTS WITH 'Gr') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "name".to_string(),
                    op: GraphPredicateOp::StartsWithAny,
                    value: Value::from(vec!["Ad".to_string(), "Gr".to_string()]),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_folds_nested_negated_string_and_predicates() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT ((NOT n.name STARTS WITH 'Ad' AND NOT n.name STARTS WITH 'Gr') AND NOT n.name STARTS WITH 'Al') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "name".to_string(),
                    op: GraphPredicateOp::StartsWithAny,
                    value: Value::from(vec!["Ad".to_string(), "Gr".to_string(), "Al".to_string()]),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_collapses_duplicate_negated_and_predicates() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person) WHERE NOT (n.status = 'blocked' AND n.status = 'blocked') SET n.reviewed = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::NotEqual,
                    value: Value::from("blocked"),
                }],
                patch: Props::from([("reviewed".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_keeps_predicated_identity_matches_on_matching_path() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (n:Person {id: 'person-1'}) WHERE n.status <> 'deleted' REMOVE n.nickname",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::RemoveMatchingNodeProps {
                label: Some(Label::new("Person")),
                props: Props::from([("id".to_string(), Value::from("person-1"))]),
                predicates: vec![GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::NotEqual,
                    value: Value::from("deleted"),
                }],
                keys: vec!["nickname".to_string()],
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_lowers_edge_and_endpoint_predicates() {
        let plan = sail_cypher_mutation_plan(
            "MATCH (a:Person {id: 'a'})-[e:KNOWS]->(b:Person) WHERE (e.since >= 2020 AND e.source IS NOT NULL AND e.note CONTAINS 'work') AND NOT (b.status ENDS WITH 'blocked') AND NOT b.team IN ['ops'] SET e.seen = true",
        )
        .unwrap();

        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::PatchMatchingEdges {
                relationship: GraphRelationshipMatch {
                    from: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("id".to_string(), Value::from("a"))]),
                        predicates: Vec::new(),
                    },
                    label: Label::new("KNOWS"),
                    to: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::new(),
                        predicates: vec![
                            GraphPropertyPredicate {
                                key: "status".to_string(),
                                op: GraphPredicateOp::NotEndsWith,
                                value: Value::from("blocked"),
                            },
                            GraphPropertyPredicate {
                                key: "team".to_string(),
                                op: GraphPredicateOp::NotIn,
                                value: Value::Json(serde_json::json!(["ops"])),
                            },
                        ],
                    },
                    id: None,
                    props: Props::new(),
                    predicates: vec![
                        GraphPropertyPredicate {
                            key: "since".to_string(),
                            op: GraphPredicateOp::GreaterThanOrEqual,
                            value: Value::Int(2020),
                        },
                        GraphPropertyPredicate {
                            key: "source".to_string(),
                            op: GraphPredicateOp::IsNotNull,
                            value: Value::Null,
                        },
                        GraphPropertyPredicate {
                            key: "note".to_string(),
                            op: GraphPredicateOp::Contains,
                            value: Value::from("work"),
                        },
                    ],
                },
                patch: Props::from([("seen".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_where_rejects_deferred_predicate_forms() {
        for cypher in [
            "MATCH (n:Person) WHERE n.status = 'inactive' OR n.score >= 10 SET n.archived = true",
            "MATCH (n:Person) WHERE (n.status = 'inactive' OR n.score >= 10) SET n.archived = true",
            "MATCH (n:Person) WHERE n.status <> 'inactive' OR n.status <> 'deleted' SET n.archived = true",
            "MATCH (n:Person) WHERE n.status = 'inactive' OR n.active = true SET n.archived = true",
            "MATCH (n:Person) WHERE n.status = 'inactive' OR n.status = null SET n.archived = true",
            "MATCH (n:Person) WHERE n.status = 'active' OR n.status = 'pending' AND n.kind = 'person' SET n.archived = true",
            "MATCH (n:Person) WHERE NOT (n.status <> 'inactive' OR n.status <> 'deleted') SET n.archived = true",
            "MATCH (n:Person) WHERE NOT (n.status NOT IN ['inactive'] OR n.status = 'deleted') SET n.archived = true",
            "MATCH (n:Person) WHERE NOT (n.status = 'inactive' OR n.active = true) SET n.archived = true",
            "MATCH (n:Person) WHERE NOT (n.status <> 'inactive' AND n.active <> true) SET n.archived = true",
            "MATCH (n:Person) WHERE NOT (n.status = 'inactive' AND n.status = 'archived') SET n.archived = true",
            "MATCH (n:Person) WHERE n.name STARTS WITH 'A' OR n.name CONTAINS 'a' SET n.archived = true",
            "MATCH (n:Person) WHERE n.name STARTS WITH 'A' OR n.alias STARTS WITH 'A' SET n.archived = true",
            "MATCH (n:Person) WHERE n.name STARTS WITH 'A' OR n.name STARTS WITH 1 SET n.archived = true",
            "MATCH (n:Person) WHERE (n.kind = 'person' AND n.status = 'active') OR (n.kind = 'system' AND n.score >= 10) SET n.archived = true",
            "MATCH (n:Person) WHERE NOT ((n.kind = 'person' AND n.status = 'blocked') OR (n.kind = 'person' AND n.status = 'archived')) SET n.archived = true",
            "MATCH (n:Person) WHERE (n.status = 'inactive' SET n.archived = true",
            "MATCH (n:Person) WHERE size(n.tags) = 2 SET n.archived = true",
            "MATCH (n:Person) WHERE n.active > true SET n.archived = true",
            "MATCH (n:Person) WHERE n.name STARTS WITH 1 SET n.archived = true",
            "MATCH (n:Person) WHERE n.team IN null SET n.archived = true",
            "MATCH (n:Person) WHERE n.team IN [null] SET n.archived = true",
            "MATCH (n:Person) WHERE n.team IN [['eng']] SET n.archived = true",
            "MATCH (n:Person) WHERE m.status = 'inactive' SET n.archived = true",
        ] {
            let error = sail_cypher_mutation_plan(cypher)
                .expect_err("unsupported WHERE predicate should fail");
            assert!(
                is_cypher_planning_error(&error) || matches!(error, GrustError::CypherSyntax(_))
            );
        }
    }

    #[test]
    fn cypher_match_where_string_or_predicates_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-string-or-ada', name: 'Ada Lovelace'});
                CREATE (:Person {id: 'where-string-or-grace', name: 'Grace Hopper'});
                CREATE (:Person {id: 'where-string-or-alan', name: 'Alan Turing'});
                CREATE (:Person {id: 'where-string-or-missing'});
                MATCH (n:Person)
                WHERE n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr'
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted string OR WHERE predicates should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-string-or-ada"), Value::Bool(true)],
                vec![Value::from("where-string-or-grace"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_negated_string_or_predicates_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result = futures_executor::block_on(
            execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-string-or-ada', name: 'Ada Lovelace'});
                CREATE (:Person {id: 'where-not-string-or-bot', name: 'Ada Bot'});
                CREATE (:Person {id: 'where-not-string-or-test', name: 'Test User'});
                CREATE (:Person {id: 'where-not-string-or-missing'});
                MATCH (n:Person)
                WHERE NOT (n.name CONTAINS 'Bot' OR n.name CONTAINS 'Test')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ),
        )
        .expect("restricted negated string OR WHERE predicates should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![
                Value::from("where-not-string-or-ada"),
                Value::Bool(true)
            ]]
        );
    }

    #[test]
    fn cypher_match_where_in_or_equal_predicates_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-in-or-ada', status: 'active'});
                CREATE (:Person {id: 'where-in-or-bob', status: 'pending'});
                CREATE (:Person {id: 'where-in-or-cara', status: 'review'});
                CREATE (:Person {id: 'where-in-or-dan', status: 'blocked'});
                CREATE (:Person {id: 'where-in-or-missing'});
                MATCH (n:Person)
                WHERE n.status IN ['active', 'pending'] OR n.status = 'review'
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted IN OR equality WHERE predicates should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-in-or-ada"), Value::Bool(true)],
                vec![Value::from("where-in-or-bob"), Value::Bool(true)],
                vec![Value::from("where-in-or-cara"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_duplicate_folded_values_execute_once_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-fold-dedup-ada', status: 'active'});
                CREATE (:Person {id: 'where-fold-dedup-bob', status: 'pending'});
                CREATE (:Person {id: 'where-fold-dedup-cara', status: 'blocked'});
                MATCH (n:Person)
                WHERE n.status = 'active'
                   OR n.status = 'active'
                   OR n.status IN ['pending', 'pending']
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("duplicate folded WHERE values should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-fold-dedup-ada"), Value::Bool(true)],
                vec![Value::from("where-fold-dedup-bob"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_nested_or_predicates_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-nested-or-ada', status: 'active'});
                CREATE (:Person {id: 'where-nested-or-bob', status: 'pending'});
                CREATE (:Person {id: 'where-nested-or-cara', status: 'review'});
                CREATE (:Person {id: 'where-nested-or-dan', status: 'blocked'});
                CREATE (:Person {id: 'where-nested-or-missing'});
                MATCH (n:Person)
                WHERE (n.status = 'active' OR n.status = 'pending')
                   OR n.status = 'review'
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested folded OR WHERE predicates should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-nested-or-ada"), Value::Bool(true)],
                vec![Value::from("where-nested-or-bob"), Value::Bool(true)],
                vec![Value::from("where-nested-or-cara"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_intersected_in_predicates_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-in-intersect-ada', status: 'active'});
                CREATE (:Person {id: 'where-in-intersect-bob', status: 'pending'});
                CREATE (:Person {id: 'where-in-intersect-cara', status: 'review'});
                MATCH (n:Person)
                WHERE n.status IN ['active', 'pending']
                  AND n.status IN ['pending', 'review']
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("intersected IN WHERE predicates should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![
                Value::from("where-in-intersect-bob"),
                Value::Bool(true)
            ]]
        );
    }

    #[test]
    fn cypher_match_where_empty_in_intersection_matches_no_memory_rows() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-empty-in-ada', status: 'active'});
                CREATE (:Person {id: 'where-empty-in-bob', status: 'pending'});
                MATCH (n:Person)
                WHERE n.status IN ['active']
                  AND n.status IN ['pending']
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("empty IN intersections should execute on memory facade");

        assert!(result.table.rows.is_empty());
    }

    #[test]
    fn cypher_match_where_equality_membership_canonicalization_executes_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-eq-membership-ada', status: 'active'});
                CREATE (:Person {id: 'where-eq-membership-bob', status: 'pending'});
                CREATE (:Person {id: 'where-eq-membership-cara', status: 'review'});
                CREATE (:Person {id: 'where-eq-membership-dan', status: 'blocked'});
                MATCH (n:Person)
                WHERE n.status IN ['active', 'pending', 'review']
                  AND NOT n.status IN ['blocked', 'pending']
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("equality/membership WHERE canonicalization should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-eq-membership-ada"), Value::Bool(true)],
                vec![Value::from("where-eq-membership-cara"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_inequality_canonicalization_executes_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-neq-canonical-ada', status: 'active'});
                CREATE (:Person {id: 'where-neq-canonical-bob', status: 'pending'});
                CREATE (:Person {id: 'where-neq-canonical-cara', status: 'review'});
                MATCH (n:Person)
                WHERE n.status IN ['active', 'pending', 'review']
                  AND n.status <> 'pending'
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("inequality WHERE canonicalization should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-neq-canonical-ada"), Value::Bool(true)],
                vec![Value::from("where-neq-canonical-cara"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_order_canonicalization_executes_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-order-canonical-ada', score: 9});
                CREATE (:Person {id: 'where-order-canonical-bob', score: 13});
                CREATE (:Person {id: 'where-order-canonical-cara', score: 19});
                MATCH (n:Person)
                WHERE n.score >= 10
                  AND n.score > 12
                  AND n.score <= 20
                  AND n.score < 18
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("order WHERE canonicalization should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![
                Value::from("where-order-canonical-bob"),
                Value::Bool(true)
            ]]
        );
    }

    #[test]
    fn cypher_match_where_equality_order_canonicalization_executes_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-eq-order-ada', score: 9});
                CREATE (:Person {id: 'where-eq-order-bob', score: 13});
                CREATE (:Person {id: 'where-eq-order-cara', score: 19});
                MATCH (n:Person)
                WHERE n.score = 13
                  AND n.score >= 10
                  AND n.score < 20
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("equality/order WHERE canonicalization should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![Value::from("where-eq-order-bob"), Value::Bool(true)]]
        );
    }

    #[test]
    fn cypher_match_where_membership_order_canonicalization_executes_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-in-order-ada', score: 9});
                CREATE (:Person {id: 'where-in-order-bob', score: 13});
                CREATE (:Person {id: 'where-in-order-cara', score: 19});
                MATCH (n:Person)
                WHERE n.score IN [9, 13, 19]
                  AND n.score > 10
                  AND n.score < 18
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("membership/order WHERE canonicalization should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![Value::from("where-in-order-bob"), Value::Bool(true)]]
        );
    }

    #[test]
    fn cypher_match_where_negated_or_predicates_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-or-ada', status: 'active'});
                CREATE (:Person {id: 'where-not-or-bob', status: 'pending'});
                CREATE (:Person {id: 'where-not-or-cara', status: 'blocked'});
                CREATE (:Person {id: 'where-not-or-dan', status: 'archived'});
                CREATE (:Person {id: 'where-not-or-missing'});
                MATCH (n:Person)
                WHERE NOT (n.status = 'blocked' OR n.status = 'archived')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted negated OR WHERE predicates should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-not-or-ada"), Value::Bool(true)],
                vec![Value::from("where-not-or-bob"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_negated_null_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-null-or-ada', status: 'active'});
                CREATE (:Person {id: 'where-not-null-or-bob', status: 'inactive'});
                CREATE (:Person {id: 'where-not-null-or-cara', status: null});
                CREATE (:Person {id: 'where-not-null-or-dan'});
                MATCH (n:Person)
                WHERE NOT (n.status = 'inactive' OR n.status = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated null OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![
                Value::from("where-not-null-or-ada"),
                Value::Bool(true)
            ]]
        );
    }

    #[test]
    fn cypher_match_where_nested_negated_null_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-null-or-ada', status: 'active'});
                CREATE (:Person {id: 'where-not-nested-null-or-bob', status: 'inactive'});
                CREATE (:Person {id: 'where-not-nested-null-or-cara', status: 'paused'});
                CREATE (:Person {id: 'where-not-nested-null-or-dan', status: null});
                CREATE (:Person {id: 'where-not-nested-null-or-eve'});
                MATCH (n:Person)
                WHERE NOT ((n.status = 'inactive' OR n.status = 'paused') OR n.status = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated null OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![
                Value::from("where-not-nested-null-or-ada"),
                Value::Bool(true)
            ]]
        );
    }

    #[test]
    fn cypher_match_where_negated_null_membership_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-null-membership-or-ada', status: 'active'});
                CREATE (:Person {id: 'where-not-null-membership-or-bob', status: 'inactive'});
                CREATE (:Person {id: 'where-not-null-membership-or-cara', status: 'paused'});
                CREATE (:Person {id: 'where-not-null-membership-or-dan', status: null});
                CREATE (:Person {id: 'where-not-null-membership-or-eve'});
                MATCH (n:Person)
                WHERE NOT (n.status IN ['inactive', 'paused'] OR n.status = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated null membership OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![
                Value::from("where-not-null-membership-or-ada"),
                Value::Bool(true)
            ]]
        );
    }

    #[test]
    fn cypher_match_where_negated_null_string_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-null-string-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-null-string-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-null-string-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-null-string-or-cara', name: null});
                CREATE (:Person {id: 'where-not-null-string-or-dan'});
                MATCH (n:Person)
                WHERE NOT (n.name STARTS WITH 'Ad' OR n.name = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated null string OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-null-string-or-alan"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-null-string-or-bob"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_nested_negated_null_string_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-null-nested-string-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-null-nested-string-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-null-nested-string-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-null-nested-string-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-null-nested-string-or-cara', name: null});
                CREATE (:Person {id: 'where-not-null-nested-string-or-dan'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated null string OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-null-nested-string-or-alan"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-null-nested-string-or-bob"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_negated_null_string_order_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-null-string-order-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-null-string-order-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-null-string-order-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-null-string-order-or-zoe', name: 'Zoe'});
                CREATE (:Person {id: 'where-not-null-string-order-or-null', name: null});
                CREATE (:Person {id: 'where-not-null-string-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT (n.name STARTS WITH 'Ad' OR n.name < 'M' OR n.name = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated null string ordered OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-null-string-order-or-mira"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-null-string-order-or-zoe"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_nested_negated_null_string_order_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-null-string-order-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-nested-null-string-order-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-nested-null-string-order-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-nested-null-string-order-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-nested-null-string-order-or-zoe', name: 'Zoe'});
                CREATE (:Person {id: 'where-not-nested-null-string-order-or-null', name: null});
                CREATE (:Person {id: 'where-not-nested-null-string-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR n.name = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated null string ordered OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-nested-null-string-order-or-mira"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-nested-null-string-order-or-zoe"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_negated_null_mixed_string_order_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-null-mixed-string-order-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-null-mixed-string-order-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-null-mixed-string-order-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-null-mixed-string-order-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-null-mixed-string-order-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-null-mixed-string-order-or-zoe', name: 'Zoe'});
                CREATE (:Person {id: 'where-not-null-mixed-string-order-or-null', name: null});
                CREATE (:Person {id: 'where-not-null-mixed-string-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR n.name IN ['Alan'] OR n.name = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated null mixed string/order OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-null-mixed-string-order-or-mira"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-null-mixed-string-order-or-zoe"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_nested_negated_null_mixed_string_order_or_terms_execute_on_memory_facade()
    {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-order-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-order-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-order-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-order-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-order-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-order-or-zoe', name: 'Zoe'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-order-or-null', name: null});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR (n.name IN ['Alan'] OR n.name = 'Bob') OR n.name = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated null mixed string/order OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-nested-null-mixed-string-order-or-mira"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-nested-null-mixed-string-order-or-zoe"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_negated_mixed_string_order_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-mixed-string-order-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-mixed-string-order-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-mixed-string-order-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-mixed-string-order-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-mixed-string-order-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-mixed-string-order-or-zoe', name: 'Zoe'});
                CREATE (:Person {id: 'where-not-mixed-string-order-or-null', name: null});
                CREATE (:Person {id: 'where-not-mixed-string-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR n.name IN ['Alan'])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated mixed string/order OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-mixed-string-order-or-mira"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-mixed-string-order-or-zoe"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_nested_negated_mixed_string_order_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-mixed-string-order-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-order-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-order-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-order-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-order-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-order-or-zoe', name: 'Zoe'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-order-or-null', name: null});
                CREATE (:Person {id: 'where-not-nested-mixed-string-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR (n.name IN ['Alan'] OR n.name = 'Bob'))
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated mixed string/order OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-nested-mixed-string-order-or-mira"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-nested-mixed-string-order-or-zoe"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_negated_string_order_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-string-order-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-string-order-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-string-order-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-string-order-or-zoe', name: 'Zoe'});
                CREATE (:Person {id: 'where-not-string-order-or-null', name: null});
                CREATE (:Person {id: 'where-not-string-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT (n.name STARTS WITH 'Ad' OR n.name < 'M')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated string/order OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-string-order-or-mira"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-string-order-or-zoe"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_nested_negated_string_order_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-string-order-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-nested-string-order-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-nested-string-order-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-nested-string-order-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-nested-string-order-or-zoe', name: 'Zoe'});
                CREATE (:Person {id: 'where-not-nested-string-order-or-null', name: null});
                CREATE (:Person {id: 'where-not-nested-string-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated string/order OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-nested-string-order-or-mira"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-nested-string-order-or-zoe"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_negated_mixed_string_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-mixed-string-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-mixed-string-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-mixed-string-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-mixed-string-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-mixed-string-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-mixed-string-or-null', name: null});
                CREATE (:Person {id: 'where-not-mixed-string-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name IN ['Alan'])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated mixed string OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-mixed-string-or-bob"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-mixed-string-or-mira"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_nested_negated_mixed_string_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-mixed-string-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-or-null', name: null});
                CREATE (:Person {id: 'where-not-nested-mixed-string-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR (n.name IN ['Alan'] OR n.name = 'Bob'))
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated mixed string OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![
                Value::from("where-not-nested-mixed-string-or-mira"),
                Value::Bool(true)
            ]]
        );
    }

    #[test]
    fn cypher_match_where_negated_null_mixed_string_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-null-mixed-string-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-null-mixed-string-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-null-mixed-string-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-null-mixed-string-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-null-mixed-string-or-cara', name: null});
                CREATE (:Person {id: 'where-not-null-mixed-string-or-dan'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name IN ['Alan'] OR n.name = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated null mixed string OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![
                Value::from("where-not-null-mixed-string-or-bob"),
                Value::Bool(true)
            ]]
        );
    }

    #[test]
    fn cypher_match_where_nested_negated_null_mixed_string_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-or-null', name: null});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR (n.name IN ['Alan'] OR n.name = 'Bob') OR n.name = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated null mixed string OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![
                Value::from("where-not-nested-null-mixed-string-or-mira"),
                Value::Bool(true)
            ]]
        );
    }

    #[test]
    fn cypher_match_where_negated_null_order_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-null-order-or-ada', score: 9});
                CREATE (:Person {id: 'where-not-null-order-or-bob', score: 10});
                CREATE (:Person {id: 'where-not-null-order-or-cara', score: 15});
                CREATE (:Person {id: 'where-not-null-order-or-dan', score: null});
                CREATE (:Person {id: 'where-not-null-order-or-eve'});
                MATCH (n:Person)
                WHERE NOT (n.score < 10 OR n.score = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated null ordered OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-null-order-or-bob"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-null-order-or-cara"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_nested_negated_null_order_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-null-order-or-ada', score: 9});
                CREATE (:Person {id: 'where-not-nested-null-order-or-bob', score: 10});
                CREATE (:Person {id: 'where-not-nested-null-order-or-cara', score: 15});
                CREATE (:Person {id: 'where-not-nested-null-order-or-dan', score: 20});
                CREATE (:Person {id: 'where-not-nested-null-order-or-eve', score: 21});
                CREATE (:Person {id: 'where-not-nested-null-order-or-null', score: null});
                CREATE (:Person {id: 'where-not-nested-null-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.score < 10 OR n.score > 20) OR n.score = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated null ordered OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-nested-null-order-or-bob"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-nested-null-order-or-cara"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-nested-null-order-or-dan"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_nested_negated_null_mixed_order_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-null-mixed-order-or-ada', score: 9});
                CREATE (:Person {id: 'where-not-nested-null-mixed-order-or-bob', score: 10});
                CREATE (:Person {id: 'where-not-nested-null-mixed-order-or-cara', score: 15});
                CREATE (:Person {id: 'where-not-nested-null-mixed-order-or-dan', score: 18});
                CREATE (:Person {id: 'where-not-nested-null-mixed-order-or-eve', score: 20});
                CREATE (:Person {id: 'where-not-nested-null-mixed-order-or-fay', score: 21});
                CREATE (:Person {id: 'where-not-nested-null-mixed-order-or-null', score: null});
                CREATE (:Person {id: 'where-not-nested-null-mixed-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.score < 10 OR n.score > 20) OR n.score IN [15, 18] OR n.score = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated null mixed ordered OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-nested-null-mixed-order-or-bob"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-nested-null-mixed-order-or-eve"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_negated_null_mixed_order_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-null-mixed-order-or-ada', score: 9});
                CREATE (:Person {id: 'where-not-null-mixed-order-or-bob', score: 10});
                CREATE (:Person {id: 'where-not-null-mixed-order-or-cara', score: 15});
                CREATE (:Person {id: 'where-not-null-mixed-order-or-dan', score: 18});
                CREATE (:Person {id: 'where-not-null-mixed-order-or-eve', score: 21});
                CREATE (:Person {id: 'where-not-null-mixed-order-or-fay', score: null});
                CREATE (:Person {id: 'where-not-null-mixed-order-or-gus'});
                MATCH (n:Person)
                WHERE NOT (n.score < 10 OR n.score IN [15, 18] OR n.score = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated null mixed ordered OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-null-mixed-order-or-bob"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-null-mixed-order-or-eve"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_negated_order_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-order-or-ada', score: 9});
                CREATE (:Person {id: 'where-not-order-or-bob', score: 10});
                CREATE (:Person {id: 'where-not-order-or-cara', score: 15});
                CREATE (:Person {id: 'where-not-order-or-dan', score: 20});
                CREATE (:Person {id: 'where-not-order-or-eve', score: 21});
                CREATE (:Person {id: 'where-not-order-or-fay', score: null});
                CREATE (:Person {id: 'where-not-order-or-gus'});
                MATCH (n:Person)
                WHERE NOT (n.score < 10 OR n.score > 20)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated ordered OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-not-order-or-bob"), Value::Bool(true)],
                vec![Value::from("where-not-order-or-cara"), Value::Bool(true)],
                vec![Value::from("where-not-order-or-dan"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_nested_negated_order_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-order-or-ada', score: 5});
                CREATE (:Person {id: 'where-not-nested-order-or-bob', score: 10});
                CREATE (:Person {id: 'where-not-nested-order-or-cara', score: 15});
                CREATE (:Person {id: 'where-not-nested-order-or-dan', score: 20});
                CREATE (:Person {id: 'where-not-nested-order-or-eve', score: 21});
                CREATE (:Person {id: 'where-not-nested-order-or-fay', score: null});
                CREATE (:Person {id: 'where-not-nested-order-or-gus'});
                MATCH (n:Person)
                WHERE NOT ((n.score < 10 OR n.score > 20) OR n.score <= 5)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated ordered OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-nested-order-or-bob"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-nested-order-or-cara"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-nested-order-or-dan"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_nested_negated_mixed_order_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-mixed-order-or-ada', score: 9});
                CREATE (:Person {id: 'where-not-nested-mixed-order-or-bob', score: 10});
                CREATE (:Person {id: 'where-not-nested-mixed-order-or-cara', score: 15});
                CREATE (:Person {id: 'where-not-nested-mixed-order-or-dan', score: 18});
                CREATE (:Person {id: 'where-not-nested-mixed-order-or-eve', score: 20});
                CREATE (:Person {id: 'where-not-nested-mixed-order-or-fay', score: 21});
                CREATE (:Person {id: 'where-not-nested-mixed-order-or-null', score: null});
                CREATE (:Person {id: 'where-not-nested-mixed-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.score < 10 OR n.score > 20) OR n.score IN [15, 18])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated mixed ordered OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-nested-mixed-order-or-bob"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-nested-mixed-order-or-eve"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_negated_mixed_order_or_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-mixed-order-or-ada', score: 9});
                CREATE (:Person {id: 'where-not-mixed-order-or-bob', score: 10});
                CREATE (:Person {id: 'where-not-mixed-order-or-cara', score: 15});
                CREATE (:Person {id: 'where-not-mixed-order-or-dan', score: 18});
                CREATE (:Person {id: 'where-not-mixed-order-or-eve', score: 21});
                CREATE (:Person {id: 'where-not-mixed-order-or-fay', score: null});
                CREATE (:Person {id: 'where-not-mixed-order-or-gus'});
                MATCH (n:Person)
                WHERE NOT (n.score < 10 OR n.score IN [15, 18])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated mixed ordered OR terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-mixed-order-or-bob"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-mixed-order-or-eve"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_negated_and_predicates_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-and-ada', status: 'active'});
                CREATE (:Person {id: 'where-not-and-bob', status: 'pending'});
                CREATE (:Person {id: 'where-not-and-cara', status: 'blocked'});
                CREATE (:Person {id: 'where-not-and-missing'});
                MATCH (n:Person)
                WHERE NOT (n.status <> 'active' AND n.status <> 'pending')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted negated AND WHERE predicates should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-not-and-ada"), Value::Bool(true)],
                vec![Value::from("where-not-and-bob"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_negated_and_subsumed_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-and-subsumed-ada', status: 'active'});
                CREATE (:Person {id: 'where-not-and-subsumed-bob', status: 'pending'});
                CREATE (:Person {id: 'where-not-and-subsumed-cara', status: 'blocked'});
                CREATE (:Person {id: 'where-not-and-subsumed-missing'});
                MATCH (n:Person)
                WHERE NOT (n.status = 'active' AND n.status IN ['active', 'pending'])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated AND subsumed terms should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-not-and-subsumed-bob"), Value::Bool(true)],
                vec![
                    Value::from("where-not-and-subsumed-cara"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_negated_string_and_predicates_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-string-and-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-string-and-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-string-and-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-string-and-missing'});
                MATCH (n:Person)
                WHERE NOT (NOT n.name STARTS WITH 'Ad' AND NOT n.name STARTS WITH 'Gr')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted negated string AND WHERE predicates should execute");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-not-string-and-ada"), Value::Bool(true)],
                vec![Value::from("where-not-string-and-grace"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_nested_negated_string_and_predicates_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-string-and-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-nested-string-and-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-nested-string-and-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-nested-string-and-bob', name: 'Bob'});
                MATCH (n:Person)
                WHERE NOT ((NOT n.name STARTS WITH 'Ad' AND NOT n.name STARTS WITH 'Gr') AND NOT n.name STARTS WITH 'Al')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated string AND WHERE predicates should execute");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-not-nested-string-and-ada"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-nested-string-and-alan"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-not-nested-string-and-grace"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_duplicate_negated_and_predicates_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-dup-and-ada', status: 'active'});
                CREATE (:Person {id: 'where-not-dup-and-bob', status: 'blocked'});
                CREATE (:Person {id: 'where-not-dup-and-missing'});
                MATCH (n:Person)
                WHERE NOT (n.status = 'blocked' AND n.status = 'blocked')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("duplicate negated AND WHERE predicates should execute");

        assert_eq!(
            result.table.rows,
            vec![vec![
                Value::from("where-not-dup-and-ada"),
                Value::Bool(true)
            ]]
        );
    }

    #[test]
    fn cypher_match_where_or_predicates_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-ada', status: 'active'});
                CREATE (:Person {id: 'where-or-bob', status: 'pending'});
                CREATE (:Person {id: 'where-or-cara', status: 'blocked'});
                CREATE (:Person {id: 'where-or-missing'});
                MATCH (n:Person)
                WHERE n.status = 'active' OR n.status = 'pending'
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted OR WHERE predicates should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-or-ada"), Value::Bool(true)],
                vec![Value::from("where-or-bob"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_or_of_and_groups_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-and-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-or-and-bob', kind: 'person', status: 'pending'});
                CREATE (:Person {id: 'where-or-and-cara', kind: 'system', status: 'active'});
                CREATE (:Person {id: 'where-or-and-dan', kind: 'person', status: 'blocked'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.status = 'active')
                   OR (n.kind = 'person' AND n.status = 'pending')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("factored OR of AND WHERE predicates should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-or-and-ada"), Value::Bool(true)],
                vec![Value::from("where-or-and-bob"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_boolean_ast_factored_or_executes_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-ast-or-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-ast-or-bob', kind: 'person', status: 'pending'});
                CREATE (:Person {id: 'where-ast-or-cara', kind: 'system', status: 'active'});
                CREATE (:Person {id: 'where-ast-or-dan', kind: 'person', status: 'blocked'});
                MATCH (n:Person)
                WHERE n.kind = 'person' AND n.status = 'active'
                   OR n.kind = 'person' AND n.status = 'pending'
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("boolean AST factored OR WHERE predicates should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-ast-or-ada"), Value::Bool(true)],
                vec![Value::from("where-ast-or-bob"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_negated_subsumed_factored_or_executes_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-subsumed-or-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-not-subsumed-or-bob', kind: 'person', status: 'blocked'});
                CREATE (:Person {id: 'where-not-subsumed-or-bot', kind: 'bot', status: 'active'});
                CREATE (:Person {id: 'where-not-subsumed-or-missing', status: 'active'});
                MATCH (n:Person)
                WHERE NOT ((n.kind = 'person' AND n.status = 'active') OR n.kind = 'person')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated subsumed factored OR should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![
                Value::from("where-not-subsumed-or-bot"),
                Value::Bool(true)
            ],]
        );
    }

    #[test]
    fn cypher_match_where_canonicalized_or_branch_predicates_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-branch-canon-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-or-branch-canon-bob', kind: 'person', status: 'review'});
                CREATE (:Person {id: 'where-or-branch-canon-cara', kind: 'person', status: 'pending'});
                CREATE (:Person {id: 'where-or-branch-canon-dan', kind: 'bot', status: 'active'});
                MATCH (n:Person)
                WHERE (n.kind = 'person'
                       AND n.status = 'active'
                       AND n.status IN ['active', 'pending'])
                   OR (n.kind = 'person'
                       AND n.status = 'review'
                       AND n.status IN ['review', 'archived'])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("canonicalized OR branch predicates should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-or-branch-canon-ada"), Value::Bool(true),],
                vec![Value::from("where-or-branch-canon-bob"), Value::Bool(true),],
            ]
        );
    }

    #[test]
    fn cypher_match_where_pruned_or_branches_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-prune-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-or-prune-bob', kind: 'person', status: 'blocked'});
                CREATE (:Person {id: 'where-or-prune-cara', kind: 'bot', status: 'active'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.status = 'active')
                   OR (n.kind = 'person' AND n.status = 'blocked' AND n.status = 'active')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("pruned impossible OR branches should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![Value::from("where-or-prune-ada"), Value::Bool(true)]]
        );
    }

    #[test]
    fn cypher_match_where_subsumed_or_branches_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-subsumed-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-or-subsumed-bob', kind: 'person', status: 'blocked'});
                CREATE (:Person {id: 'where-or-subsumed-cara', kind: 'bot', status: 'active'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.status = 'active')
                   OR (n.kind = 'person')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("subsumed OR branches should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-or-subsumed-ada"), Value::Bool(true)],
                vec![Value::from("where-or-subsumed-bob"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_semantically_subsumed_or_branches_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-semantic-subsumed-ada', kind: 'person', status: 'active', region: 'us'});
                CREATE (:Person {id: 'where-or-semantic-subsumed-bob', kind: 'person', status: 'pending', region: 'eu'});
                CREATE (:Person {id: 'where-or-semantic-subsumed-cara', kind: 'person', status: 'blocked', region: 'us'});
                CREATE (:Person {id: 'where-or-semantic-subsumed-dan', kind: 'bot', status: 'active', region: 'us'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.status = 'active' AND n.region = 'us')
                   OR (n.kind = 'person' AND n.status IN ['active', 'pending'])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("semantically subsumed OR branches should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-or-semantic-subsumed-ada"),
                    Value::Bool(true),
                ],
                vec![
                    Value::from("where-or-semantic-subsumed-bob"),
                    Value::Bool(true),
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_string_subsumed_or_branches_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-string-subsumed-ada', kind: 'person', name: 'Ada'});
                CREATE (:Person {id: 'where-or-string-subsumed-grace', kind: 'person', name: 'Grace'});
                CREATE (:Person {id: 'where-or-string-subsumed-bob', kind: 'person', name: 'Bob'});
                CREATE (:Person {id: 'where-or-string-subsumed-adbot', kind: 'bot', name: 'AdaBot'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.name STARTS WITH 'Ad')
                   OR (n.kind = 'person' AND (n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr'))
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("string-subsumed OR branches should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-or-string-subsumed-ada"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-or-string-subsumed-grace"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_negated_string_subsumed_or_branches_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-neg-string-subsumed-ada', kind: 'person', name: 'Ada'});
                CREATE (:Person {id: 'where-or-neg-string-subsumed-grace', kind: 'person', name: 'Grace'});
                CREATE (:Person {id: 'where-or-neg-string-subsumed-bob', kind: 'person', name: 'Bob'});
                CREATE (:Person {id: 'where-or-neg-string-subsumed-adbot', kind: 'bot', name: 'AdaBot'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND NOT (n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr'))
                   OR (n.kind = 'person' AND NOT n.name STARTS WITH 'Ad')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated string-subsumed OR branches should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-or-neg-string-subsumed-bob"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-or-neg-string-subsumed-grace"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_null_check_subsumed_or_branches_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let non_null =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-null-subsumed-ada', kind: 'person', name: 'Ada'});
                CREATE (:Person {id: 'where-or-null-subsumed-bob', kind: 'person', name: 'Bob'});
                CREATE (:Person {id: 'where-or-null-subsumed-null', kind: 'person', name: null});
                CREATE (:Person {id: 'where-or-null-subsumed-missing', kind: 'person'});
                CREATE (:Person {id: 'where-or-null-subsumed-bot', kind: 'bot', name: 'AdaBot'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.name STARTS WITH 'Ad')
                   OR (n.kind = 'person' AND n.name IS NOT NULL)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("null-check subsumed OR branches should execute on memory facade");

        assert_eq!(
            non_null.table.rows,
            vec![
                vec![Value::from("where-or-null-subsumed-ada"), Value::Bool(true)],
                vec![Value::from("where-or-null-subsumed-bob"), Value::Bool(true)],
            ]
        );

        let simple =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person)
                WHERE n.name STARTS WITH 'Ad' OR n.name IS NOT NULL
                SET n.simple_selected = true
                RETURN n.id AS id, n.simple_selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("simple null-check subsumed OR terms should execute on memory facade");

        assert_eq!(
            simple.table.rows,
            vec![
                vec![Value::from("where-or-null-subsumed-ada"), Value::Bool(true)],
                vec![Value::from("where-or-null-subsumed-bob"), Value::Bool(true)],
                vec![Value::from("where-or-null-subsumed-bot"), Value::Bool(true)],
            ]
        );

        let negated_simple =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person)
                WHERE NOT (n.name STARTS WITH 'Ad' OR n.name IS NOT NULL)
                SET n.negated_simple_selected = true
                RETURN n.id AS id, n.negated_simple_selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated simple null-check subsumed OR terms should execute on memory facade");

        assert_eq!(
            negated_simple.table.rows,
            vec![
                vec![
                    Value::from("where-or-null-subsumed-missing"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-or-null-subsumed-null"),
                    Value::Bool(true)
                ],
            ]
        );

        let null =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.nickname = null)
                   OR (n.kind = 'person' AND n.nickname IS NULL)
                SET n.needs_nickname = true
                RETURN n.id AS id, n.needs_nickname AS needs_nickname
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("IS NULL subsumed OR branches should execute on memory facade");

        assert_eq!(
            null.table.rows,
            vec![
                vec![Value::from("where-or-null-subsumed-ada"), Value::Bool(true)],
                vec![Value::from("where-or-null-subsumed-bob"), Value::Bool(true)],
                vec![
                    Value::from("where-or-null-subsumed-missing"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-or-null-subsumed-null"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_singleton_not_in_subsumed_or_branches_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-not-in-subsumed-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-or-not-in-subsumed-bob', kind: 'person', status: 'pending'});
                CREATE (:Person {id: 'where-or-not-in-subsumed-cara', kind: 'person', status: 'blocked'});
                CREATE (:Person {id: 'where-or-not-in-subsumed-dan', kind: 'bot', status: 'active'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.status <> 'blocked')
                   OR (n.kind = 'person' AND NOT n.status IN ['blocked'])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("singleton NOT IN subsumed OR branches should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-or-not-in-subsumed-ada"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-or-not-in-subsumed-bob"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_singleton_in_subsumed_or_branches_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-in-subsumed-ada', kind: 'person', status: 'active', region: 'us'});
                CREATE (:Person {id: 'where-or-in-subsumed-bob', kind: 'person', status: 'active', region: 'eu'});
                CREATE (:Person {id: 'where-or-in-subsumed-cara', kind: 'person', status: 'pending', region: 'us'});
                CREATE (:Person {id: 'where-or-in-subsumed-dan', kind: 'bot', status: 'active', region: 'us'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.status IN ['active'] AND n.region = 'us')
                   OR (n.kind = 'person' AND n.status = 'active')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("singleton IN subsumed OR branches should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-or-in-subsumed-ada"), Value::Bool(true)],
                vec![Value::from("where-or-in-subsumed-bob"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_equality_preferred_over_singleton_in_executes_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-in-prefer-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-or-in-prefer-bob', kind: 'person', status: 'pending'});
                CREATE (:Person {id: 'where-or-in-prefer-bot', kind: 'bot', status: 'active'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.status IN ['active'])
                   OR (n.kind = 'person' AND n.status = 'active')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("equality-preferred singleton IN OR branches should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![
                Value::from("where-or-in-prefer-ada"),
                Value::Bool(true)
            ]]
        );
    }

    #[test]
    fn cypher_match_where_order_inequality_subsumed_or_branches_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-order-neq-subsumed-ada', kind: 'person', score: 25, region: 'us'});
                CREATE (:Person {id: 'where-or-order-neq-subsumed-bob', kind: 'person', score: 13, region: 'eu'});
                CREATE (:Person {id: 'where-or-order-neq-subsumed-cara', kind: 'person', score: 5, region: 'us'});
                CREATE (:Person {id: 'where-or-order-neq-subsumed-dan', kind: 'bot', score: 25, region: 'us'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.score > 20 AND n.region = 'us')
                   OR (n.kind = 'person' AND n.score <> 5)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("ordered-bound inequality-subsumed OR branches should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-or-order-neq-subsumed-ada"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-or-order-neq-subsumed-bob"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_order_not_in_subsumed_or_branches_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-order-not-in-subsumed-ada', kind: 'person', score: 25, region: 'us'});
                CREATE (:Person {id: 'where-or-order-not-in-subsumed-bob', kind: 'person', score: 13, region: 'eu'});
                CREATE (:Person {id: 'where-or-order-not-in-subsumed-cara', kind: 'person', score: 5, region: 'us'});
                CREATE (:Person {id: 'where-or-order-not-in-subsumed-dan', kind: 'person', score: 10, region: 'eu'});
                CREATE (:Person {id: 'where-or-order-not-in-subsumed-bot', kind: 'bot', score: 25, region: 'us'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.score > 20 AND n.region = 'us')
                   OR (n.kind = 'person' AND NOT n.score IN [5, 10])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("ordered-bound NOT IN subsumed OR branches should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-or-order-not-in-subsumed-ada"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("where-or-order-not-in-subsumed-bob"),
                    Value::Bool(true)
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_order_subsumed_or_branches_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-order-subsumed-ada', kind: 'person', score: 9, region: 'us'});
                CREATE (:Person {id: 'where-or-order-subsumed-bob', kind: 'person', score: 13, region: 'eu'});
                CREATE (:Person {id: 'where-or-order-subsumed-cara', kind: 'person', score: 21, region: 'us'});
                CREATE (:Person {id: 'where-or-order-subsumed-dan', kind: 'bot', score: 30, region: 'us'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.score > 20 AND n.region = 'us')
                   OR (n.kind = 'person' AND n.score > 10)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("ordered-bound subsumed OR branches should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![
                    Value::from("where-or-order-subsumed-bob"),
                    Value::Bool(true),
                ],
                vec![
                    Value::from("where-or-order-subsumed-cara"),
                    Value::Bool(true),
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_nested_or_terms_inside_and_branches_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-nested-or-ada', region: 'us', status: 'active'});
                CREATE (:Person {id: 'where-nested-or-bob', region: 'us', status: 'pending'});
                CREATE (:Person {id: 'where-nested-or-cara', region: 'eu', status: 'active'});
                CREATE (:Person {id: 'where-nested-or-dan', region: 'eu', status: 'blocked'});
                CREATE (:Person {id: 'where-nested-or-eve', region: 'apac', status: 'active'});
                MATCH (n:Person)
                WHERE (n.region = 'us' AND (n.status = 'active' OR n.status = 'pending'))
                   OR (n.region = 'eu' AND (n.status = 'active' OR n.status = 'pending'))
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested OR terms inside factored AND branches should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-nested-or-ada"), Value::Bool(true)],
                vec![Value::from("where-nested-or-bob"), Value::Bool(true)],
                vec![Value::from("where-nested-or-cara"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_duplicate_factored_branch_predicates_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-branch-dedup-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-branch-dedup-bob', kind: 'person', status: 'pending'});
                CREATE (:Person {id: 'where-branch-dedup-cara', kind: 'system', status: 'active'});
                CREATE (:Person {id: 'where-branch-dedup-dan', kind: 'person', status: 'blocked'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.kind = 'person' AND n.status = 'active')
                   OR (n.kind = 'person' AND n.status = 'pending' AND n.status = 'pending')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("duplicate predicates inside factored OR branches should execute");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-branch-dedup-ada"), Value::Bool(true)],
                vec![Value::from("where-branch-dedup-bob"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_duplicate_predicates_execute_once_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-dedup-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-dedup-bob', kind: 'person', status: 'pending'});
                CREATE (:Person {id: 'where-dedup-cara', kind: 'person', status: 'blocked'});
                MATCH (n:Person)
                WHERE n.kind = 'person'
                  AND (n.status = 'active' OR n.status = 'pending')
                  AND n.kind = 'person'
                  AND (n.status = 'active' OR n.status = 'pending')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("duplicate WHERE predicates should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-dedup-ada"), Value::Bool(true)],
                vec![Value::from("where-dedup-bob"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_in_predicates_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-in-ada', team: 'eng', status: 'active'});
                CREATE (:Person {id: 'where-in-bob', team: 'ops', status: 'active'});
                CREATE (:Person {id: 'where-in-cara', team: 'data', status: 'blocked'});
                CREATE (:Person {id: 'where-in-missing', status: 'active'});
                MATCH (n:Person)
                WHERE n.team IN ['eng', 'data'] AND NOT n.status IN ['blocked']
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted IN WHERE predicates should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![Value::from("where-in-ada"), Value::Bool(true)]]
        );
    }

    #[test]
    fn cypher_match_where_parenthesized_terms_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-paren-ada', status: 'inactive', score: 12, active: false});
                CREATE (:Person {id: 'where-paren-bob', status: 'inactive', score: 5, active: false});
                CREATE (:Person {id: 'where-paren-cara', status: 'inactive', score: 14, active: true});
                MATCH (n:Person) WHERE (n.status = 'inactive' AND n.score >= 10) AND NOT (n.active = true)
                SET n.archived = true
                RETURN n.id AS id, n.archived AS archived
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("parenthesized WHERE predicates should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![Value::from("where-paren-ada"), Value::Bool(true)]]
        );
    }

    #[test]
    fn cypher_match_where_string_predicates_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-string-ada', name: 'Ada Lovelace', status: 'active'});
                CREATE (:Person {id: 'where-string-grace', name: 'Grace Hopper', status: 'inactive'});
                CREATE (:Person {id: 'where-string-alan', name: 'Alan Turing', status: 'active'});
                CREATE (:Person {id: 'where-string-missing', status: 'active'});
                MATCH (n:Person)
                WHERE n.name STARTS WITH 'A' AND n.name CONTAINS 'a' AND NOT n.name ENDS WITH 'ing'
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted string WHERE predicates should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![Value::from("where-string-ada"), Value::Bool(true)]]
        );
    }

    #[test]
    fn cypher_match_where_not_executes_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-ada', active: true});
                CREATE (:Person {id: 'where-not-bob', active: false});
                CREATE (:Person {id: 'where-not-cara'});
                MATCH (n:Person) WHERE NOT n.active = true SET n.archived = true
                RETURN n.id AS id, n.archived AS archived
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted NOT WHERE should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![Value::from("where-not-bob"), Value::Bool(true)]]
        );
    }

    #[test]
    fn cypher_match_where_double_negation_executes_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-not-ada', active: true});
                CREATE (:Person {id: 'where-not-not-bob', active: false});
                CREATE (:Person {id: 'where-not-not-cara'});
                MATCH (n:Person) WHERE NOT NOT n.active = true SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted double-negated WHERE should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![Value::from("where-not-not-ada"), Value::Bool(true)]]
        );
    }

    #[test]
    fn cypher_match_where_is_null_executes_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-null-ada', nickname: 'Ada'});
                CREATE (:Person {id: 'where-null-bob', nickname: null});
                CREATE (:Person {id: 'where-null-cara'});
                MATCH (n:Person) WHERE n.nickname IS NULL SET n.unset = true
                RETURN n.id AS id, n.unset AS unset
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted IS NULL WHERE should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-null-bob"), Value::Bool(true)],
                vec![Value::from("where-null-cara"), Value::Bool(true)],
            ]
        );
    }

    #[test]
    fn cypher_match_where_negated_null_checks_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-null-negated-ada', nickname: 'Ada'});
                CREATE (:Person {id: 'where-not-null-negated-bob', nickname: null});
                CREATE (:Person {id: 'where-not-null-negated-cara'});
                MATCH (n:Person) WHERE NOT n.nickname IS NOT NULL SET n.unset = true
                RETURN n.id AS id, n.unset AS unset
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated restricted IS NOT NULL WHERE should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![
                vec![Value::from("where-not-null-negated-bob"), Value::Bool(true),],
                vec![
                    Value::from("where-not-null-negated-cara"),
                    Value::Bool(true),
                ],
            ]
        );
    }

    #[test]
    fn cypher_match_where_is_not_null_executes_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-null-ada', nickname: 'Ada'});
                CREATE (:Person {id: 'where-not-null-bob', nickname: null});
                CREATE (:Person {id: 'where-not-null-cara'});
                MATCH (n:Person) WHERE n.nickname IS NOT NULL SET n.seen = true
                RETURN n.id AS id, n.seen AS seen
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted IS NOT NULL WHERE should execute on memory facade");

        assert_eq!(
            result.table.rows,
            vec![vec![Value::from("where-not-null-ada"), Value::Bool(true)]]
        );
    }

    #[test]
    fn cypher_match_merge_lowers_id_resolved_edge_pattern() {
        let plan = sail_cypher_mutation_plan(
            "
            MATCH (a:Person {id: 'person-1', note: 'contains, comma'}), (b:Person {id: 'person-2'})
            MERGE (a)-[:KNOWS {since: 2026}]->(b)
            ",
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
            vec![GraphMutation::UpsertEdge(Edge::new(
                "KNOWS",
                "person-1",
                "person-2",
                Props::from([("since".to_string(), Value::Int(2026))]),
            ))]
        );
    }

    #[test]
    fn cypher_match_create_lowers_id_resolved_edge_pattern() {
        let plan = sail_cypher_mutation_plan(
            "
            MATCH (a:Person {id: 'person-1'}), (b:Person {id: 'person-2'})
            CREATE (a)-[:KNOWS {since: 2026}]->(b)
            ",
        )
        .unwrap();

        assert_eq!(
            plan.report(),
            GraphMutationReport {
                creates: 1,
                changed_edges: 1,
                edge_upserts: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            plan.into_mutations(),
            vec![GraphMutation::UpsertEdge(Edge::new(
                "KNOWS",
                "person-1",
                "person-2",
                Props::from([("since".to_string(), Value::Int(2026))]),
            ))]
        );
    }

    #[test]
    fn cypher_match_create_lowers_row_producing_edge_pattern() {
        let plan = sail_cypher_mutation_plan_with_options(
            "
            MATCH (a:Person {status: 'active'}), (b:Team {id: $team})
            WHERE a.score >= 10
            CREATE (a)-[:MEMBER_OF {source: 'cypher'}]->(b)
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([("team".to_string(), Value::from("team-1"))]),
                ..CypherMutationOptions::default()
            },
        )
        .unwrap()
        .0;

        assert_eq!(
            plan.report(),
            GraphMutationReport {
                creates: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
                kind: GraphMutationPlanKind::Create,
                from: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("status".to_string(), Value::from("active"))]),
                    predicates: vec![GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::Int(10),
                    }],
                },
                to: GraphNodeMatch {
                    label: Some(Label::new("Team")),
                    props: Props::from([("id".to_string(), Value::from("team-1"))]),
                    predicates: Vec::new(),
                },
                label: Label::new("MEMBER_OF"),
                props: Props::from([("source".to_string(), Value::from("cypher"))]),
                edge_id_policy: GraphRowEdgeIdPolicy::ExplicitOnly,
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_create_lowers_row_producing_edge_variable() {
        let planned = sail_cypher_mutation_plan_with_return_options(
            "
            MATCH (a:Person {status: 'active'}), (b:Team {id: 'team-1'})
            CREATE (a)-[e:MEMBER_OF {source: 'cypher'}]->(b)
            RETURN e.label;
            ",
            CypherMutationOptions::default(),
        )
        .unwrap();

        assert_eq!(
            planned.plan.operations,
            vec![GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
                kind: GraphMutationPlanKind::Create,
                from: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("status".to_string(), Value::from("active"))]),
                    predicates: Vec::new(),
                },
                to: GraphNodeMatch {
                    label: Some(Label::new("Team")),
                    props: Props::from([("id".to_string(), Value::from("team-1"))]),
                    predicates: Vec::new(),
                },
                label: Label::new("MEMBER_OF"),
                props: Props::from([("source".to_string(), Value::from("cypher"))]),
                edge_id_policy: GraphRowEdgeIdPolicy::ExplicitOnly,
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
        assert_eq!(
            planned.row_edge_bindings.get("e"),
            Some(&CypherRowProducedEdgeBinding {
                kind: GraphMutationPlanKind::Create,
                from_variable: "a".to_string(),
                from: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("status".to_string(), Value::from("active"))]),
                    predicates: Vec::new(),
                },
                to_variable: "b".to_string(),
                to: GraphNodeMatch {
                    label: Some(Label::new("Team")),
                    props: Props::from([("id".to_string(), Value::from("team-1"))]),
                    predicates: Vec::new(),
                },
                label: Label::new("MEMBER_OF"),
                props: Props::from([("source".to_string(), Value::from("cypher"))]),
                edge_id_policy: GraphRowEdgeIdPolicy::ExplicitOnly,
            })
        );
    }

    #[test]
    fn cypher_match_merge_lowers_row_producing_edge_pattern() {
        let plan = sail_cypher_mutation_plan_with_options(
            "
            MATCH (a:Person {status: 'active'}), (b:Team {id: $team})
            WHERE a.score >= 10
            MERGE (a)-[:MEMBER_OF {source: 'cypher'}]->(b)
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([("team".to_string(), Value::from("team-1"))]),
                ..CypherMutationOptions::default()
            },
        )
        .unwrap()
        .0;

        assert_eq!(
            plan.report(),
            GraphMutationReport {
                merges: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            plan.operations,
            vec![GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
                kind: GraphMutationPlanKind::Merge,
                from: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("status".to_string(), Value::from("active"))]),
                    predicates: vec![GraphPropertyPredicate {
                        key: "score".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::Int(10),
                    }],
                },
                to: GraphNodeMatch {
                    label: Some(Label::new("Team")),
                    props: Props::from([("id".to_string(), Value::from("team-1"))]),
                    predicates: Vec::new(),
                },
                label: Label::new("MEMBER_OF"),
                props: Props::from([("source".to_string(), Value::from("cypher"))]),
                edge_id_policy: GraphRowEdgeIdPolicy::ExplicitOnly,
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_merge_rejects_unresolved_or_broad_forms() {
        for cypher in [
            "MATCH (:Person {id: 'person-1'}), (b:Person {id: 'person-2'}) MERGE (:Person {id: 'person-1'})-[:KNOWS]->(b)",
            "MATCH (a:Person {id: 'person-1'}) MERGE (a)-[:KNOWS]->(b)",
            "MATCH (a:Person {id: 'person-1'}) MERGE (:Person {id: 'person-3'})",
            "MATCH (a:Person {name: 'Ada'}), (b:Person {id: 'person-2'}) MERGE (a)-[:KNOWS {id: 1}]->(b)",
        ] {
            let error =
                sail_cypher_mutation_plan(cypher).expect_err("unsupported MATCH MERGE must fail");
            assert!(is_cypher_planning_error(&error));
        }
    }

    #[test]
    fn cypher_match_create_rejects_unresolved_or_broad_forms() {
        for cypher in [
            "MATCH (:Person {id: 'person-1'}), (b:Person {id: 'person-2'}) CREATE (:Person {id: 'person-1'})-[:KNOWS]->(b)",
            "MATCH (a:Person {id: 'person-1'}) CREATE (a)-[:KNOWS]->(b)",
            "MATCH (a:Person {id: 'person-1'}) CREATE (:Person {id: 'person-3'})",
            "MATCH (a:Person {id: 'person-1'}) CREATE (a)-[:KNOWS]->(:Person {id: 'person-2'})",
            "MATCH (a:Person {name: 'Ada'}), (b:Person {id: 'person-2'}) CREATE (a)-[:KNOWS {id: 1}]->(b)",
        ] {
            let error =
                sail_cypher_mutation_plan(cypher).expect_err("unsupported MATCH CREATE must fail");
            assert!(is_cypher_planning_error(&error));
        }
    }

    #[test]
    fn cypher_match_set_map_patch_lowers_id_resolved_node() {
        let plan = sail_cypher_mutation_plan(
            "
            MATCH (n:Person {id: 'person-1'})
            SET n += {name: 'Ada', nickname: null, note: 'literal += stays literal'}
            ",
        )
        .unwrap();

        assert_eq!(
            plan.report(),
            GraphMutationReport {
                patches: 1,
                changed_nodes: 1,
                node_patches: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            plan.into_mutations(),
            vec![GraphMutation::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([
                    ("name".to_string(), Value::from("Ada")),
                    ("nickname".to_string(), Value::Null),
                    (
                        "note".to_string(),
                        Value::String("literal += stays literal".to_string())
                    ),
                ]),
            }]
        );
    }

    #[test]
    fn cypher_match_set_map_patch_lowers_broad_nodes_with_cardinality() {
        let bounded = sail_cypher_mutation_plan(
            "
            MATCH (n:Person {status: 'inactive'})
            SET n += {archived: true, note: null}
            ",
        )
        .unwrap();

        assert_eq!(
            bounded.report(),
            GraphMutationReport {
                patches: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            bounded.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::from([("status".to_string(), Value::from("inactive"))]),
                predicates: Vec::new(),
                patch: Props::from([
                    ("archived".to_string(), Value::Bool(true)),
                    ("note".to_string(), Value::Null),
                ]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
        assert_eq!(
            bounded.into_mutations(),
            vec![GraphMutation::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::from([("status".to_string(), Value::from("inactive"))]),
                predicates: Vec::new(),
                patch: Props::from([
                    ("archived".to_string(), Value::Bool(true)),
                    ("note".to_string(), Value::Null),
                ]),
            }]
        );

        let unbounded = sail_cypher_mutation_plan("MATCH (n) SET n += {touched: true}").unwrap();
        assert_eq!(
            unbounded.operations,
            vec![GraphMutationPlanOp::PatchMatchingNodes {
                label: None,
                props: Props::new(),
                predicates: Vec::new(),
                patch: Props::from([("touched".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::UnboundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_set_map_patch_lowers_id_resolved_edge() {
        let plan = sail_cypher_mutation_plan(
            "
            MATCH (:Person {id: 'person-1'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'person-2'})
            SET e += {since: 2026, note: null}
            ",
        )
        .unwrap();

        assert_eq!(
            plan.report(),
            GraphMutationReport {
                patches: 1,
                changed_edges: 1,
                edge_patches: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            plan.into_mutations(),
            vec![GraphMutation::PatchEdge {
                from: NodeId::new("person-1"),
                label: Label::new("KNOWS"),
                to: NodeId::new("person-2"),
                id: Some(EdgeId::new("edge-1")),
                props: Props::from([
                    ("note".to_string(), Value::Null),
                    ("since".to_string(), Value::Int(2026)),
                ]),
            }]
        );

        let structural = sail_cypher_mutation_plan(
            "MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {id: 'person-2'}) SET e += {since: 2026}",
        )
        .unwrap();
        assert_eq!(
            structural.into_mutations(),
            vec![GraphMutation::PatchEdge {
                from: NodeId::new("person-1"),
                label: Label::new("KNOWS"),
                to: NodeId::new("person-2"),
                id: None,
                props: Props::from([("since".to_string(), Value::Int(2026))]),
            }]
        );

        let broad = sail_cypher_mutation_plan(
            "MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {status: 'inactive'}) SET e += {seen: true}",
        )
        .unwrap();
        assert_eq!(
            broad.into_mutations(),
            vec![GraphMutation::PatchMatchingEdges {
                relationship: GraphRelationshipMatch {
                    from: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("id".to_string(), Value::from("person-1"))]),
                        predicates: Vec::new(),
                    },
                    label: Label::new("KNOWS"),
                    to: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("status".to_string(), Value::from("inactive"))]),
                        predicates: Vec::new(),
                    },
                    id: None,
                    props: Props::new(),
                    predicates: Vec::new(),
                },
                patch: Props::from([("seen".to_string(), Value::Bool(true))]),
            }]
        );
    }

    #[test]
    fn cypher_match_edge_mutations_accept_relationship_property_predicates() {
        let patch = sail_cypher_mutation_plan(
            "MATCH (:Person {id: 'person-1'})-[e:KNOWS {since: 2020, active: true}]->(:Person {id: 'person-2'}) SET e.seen = true",
        )
        .unwrap();
        assert_eq!(
            patch.operations,
            vec![GraphMutationPlanOp::PatchMatchingEdges {
                relationship: GraphRelationshipMatch {
                    from: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("id".to_string(), Value::from("person-1"))]),
                        predicates: Vec::new(),
                    },
                    label: Label::new("KNOWS"),
                    to: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("id".to_string(), Value::from("person-2"))]),
                        predicates: Vec::new(),
                    },
                    id: None,
                    props: Props::from([
                        ("active".to_string(), Value::Bool(true)),
                        ("since".to_string(), Value::Int(2020)),
                    ]),
                    predicates: Vec::new(),
                },
                patch: Props::from([("seen".to_string(), Value::Bool(true))]),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );

        let remove = sail_cypher_mutation_plan(
            "MATCH (:Person {id: 'person-1'})-[e:KNOWS {id: 'edge-1', since: 2020}]->(:Person {id: 'person-2'}) REMOVE e.note",
        )
        .unwrap();
        assert_eq!(
            remove.operations,
            vec![GraphMutationPlanOp::RemoveMatchingEdgeProps {
                relationship: GraphRelationshipMatch {
                    from: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("id".to_string(), Value::from("person-1"))]),
                        predicates: Vec::new(),
                    },
                    label: Label::new("KNOWS"),
                    to: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("id".to_string(), Value::from("person-2"))]),
                        predicates: Vec::new(),
                    },
                    id: Some(EdgeId::new("edge-1")),
                    props: Props::from([("since".to_string(), Value::Int(2020))]),
                    predicates: Vec::new(),
                },
                keys: vec!["note".to_string()],
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );

        let delete = sail_cypher_mutation_plan(
            "MATCH (:Person {id: 'person-1'})-[e:KNOWS {active: false}]->(:Person {status: 'inactive'}) DELETE e",
        )
        .unwrap();
        assert_eq!(
            delete.operations,
            vec![GraphMutationPlanOp::DeleteMatchingEdges {
                relationship: GraphRelationshipMatch {
                    from: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("id".to_string(), Value::from("person-1"))]),
                        predicates: Vec::new(),
                    },
                    label: Label::new("KNOWS"),
                    to: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("status".to_string(), Value::from("inactive"))]),
                        predicates: Vec::new(),
                    },
                    id: None,
                    props: Props::from([("active".to_string(), Value::Bool(false))]),
                    predicates: Vec::new(),
                },
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_set_property_assignment_lowers_resolved_node_and_edge() {
        let node =
            sail_cypher_mutation_plan("MATCH (n:Person {id: 'person-1'}) SET n.name = 'Ada'")
                .unwrap();
        assert_eq!(
            node.into_mutations(),
            vec![GraphMutation::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([("name".to_string(), Value::from("Ada"))]),
            }]
        );

        let edge = sail_cypher_mutation_plan(
            "MATCH (:Person {id: 'person-1'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'person-2'}) SET e.since = 2026",
        )
        .unwrap();
        assert_eq!(
            edge.report(),
            GraphMutationReport {
                patches: 1,
                changed_edges: 1,
                edge_patches: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            edge.into_mutations(),
            vec![GraphMutation::PatchEdge {
                from: NodeId::new("person-1"),
                label: Label::new("KNOWS"),
                to: NodeId::new("person-2"),
                id: Some(EdgeId::new("edge-1")),
                props: Props::from([("since".to_string(), Value::Int(2026))]),
            }]
        );

        let broad_edge = sail_cypher_mutation_plan(
            "MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {status: 'inactive'}) SET e.seen = true",
        )
        .unwrap();
        assert_eq!(
            broad_edge.into_mutations(),
            vec![GraphMutation::PatchMatchingEdges {
                relationship: GraphRelationshipMatch {
                    from: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("id".to_string(), Value::from("person-1"))]),
                        predicates: Vec::new(),
                    },
                    label: Label::new("KNOWS"),
                    to: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("status".to_string(), Value::from("inactive"))]),
                        predicates: Vec::new(),
                    },
                    id: None,
                    props: Props::new(),
                    predicates: Vec::new(),
                },
                patch: Props::from([("seen".to_string(), Value::Bool(true))]),
            }]
        );

        let broad = sail_cypher_mutation_plan(
            "MATCH (n:Person {status: 'inactive'}) SET n.archived = true",
        )
        .unwrap();
        assert_eq!(
            broad.into_mutations(),
            vec![GraphMutation::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::from([("status".to_string(), Value::from("inactive"))]),
                predicates: Vec::new(),
                patch: Props::from([("archived".to_string(), Value::Bool(true))]),
            }]
        );
    }

    #[test]
    fn cypher_match_set_multiple_assignments_lowers_in_order() {
        let plan = sail_cypher_mutation_plan_with_options(
            "MATCH (n:Person {id: 'person-1'}) SET n.name = $name, n.updated_at = $ts, n.count = n.count + 1, n.name = 'Ada final'",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    ("name".to_string(), Value::from("Ada")),
                    ("ts".to_string(), Value::from("2026-06-16T00:00:00Z")),
                ]),
                ..CypherMutationOptions::default()
            },
        )
        .unwrap()
        .0;

        assert_eq!(
            plan.report(),
            GraphMutationReport {
                patches: 4,
                changed_nodes: 3,
                node_patches: 3,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            plan.operations,
            vec![
                GraphMutationPlanOp::PatchNode {
                    id: NodeId::new("person-1"),
                    props: Props::from([("name".to_string(), Value::from("Ada"))]),
                },
                GraphMutationPlanOp::PatchNode {
                    id: NodeId::new("person-1"),
                    props: Props::from([(
                        "updated_at".to_string(),
                        Value::from("2026-06-16T00:00:00Z")
                    )]),
                },
                GraphMutationPlanOp::UpdateMatchingNodeProperty {
                    label: None,
                    props: Props::from([("id".to_string(), Value::from("person-1"))]),
                    predicates: Vec::new(),
                    target_key: "count".to_string(),
                    source_key: "count".to_string(),
                    op: GraphNumericOp::Add,
                    operand: Value::Int(1),
                    cardinality: GraphMutationCardinality::SingleIdentity,
                },
                GraphMutationPlanOp::PatchNode {
                    id: NodeId::new("person-1"),
                    props: Props::from([("name".to_string(), Value::from("Ada final"))]),
                },
            ]
        );
    }

    #[test]
    fn cypher_match_set_multiple_assignments_preserves_nested_commas() {
        let node = sail_cypher_mutation_plan(
            "MATCH (n:Person {id: 'person-1'}) SET n += {name: 'Ada, Countess', note: 'x,y'}, n.flag = true",
        )
        .unwrap();
        assert_eq!(
            node.operations,
            vec![
                GraphMutationPlanOp::PatchNode {
                    id: NodeId::new("person-1"),
                    props: Props::from([
                        ("name".to_string(), Value::from("Ada, Countess")),
                        ("note".to_string(), Value::from("x,y")),
                    ]),
                },
                GraphMutationPlanOp::PatchNode {
                    id: NodeId::new("person-1"),
                    props: Props::from([("flag".to_string(), Value::Bool(true))]),
                },
            ]
        );

        let edge = sail_cypher_mutation_plan(
            "MATCH (:Person {id: 'person-1'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'person-2'}) SET e.since = 2026, e.note = 'a,b'",
        )
        .unwrap();
        assert_eq!(
            edge.operations,
            vec![
                GraphMutationPlanOp::PatchEdge {
                    from: NodeId::new("person-1"),
                    label: Label::new("KNOWS"),
                    to: NodeId::new("person-2"),
                    id: Some(EdgeId::new("edge-1")),
                    props: Props::from([("since".to_string(), Value::Int(2026))]),
                },
                GraphMutationPlanOp::PatchEdge {
                    from: NodeId::new("person-1"),
                    label: Label::new("KNOWS"),
                    to: NodeId::new("person-2"),
                    id: Some(EdgeId::new("edge-1")),
                    props: Props::from([("note".to_string(), Value::from("a,b"))]),
                },
            ]
        );
    }

    #[test]
    fn cypher_match_set_multiple_assignments_supports_null_removal() {
        let plan = sail_cypher_mutation_plan_with_options(
            "MATCH (n:Person {id: 'person-1'}) SET n.nickname = null, n.name = 'Ada'",
            CypherMutationOptions {
                null_assignment: CypherNullAssignment::RemoveProperty,
                ..CypherMutationOptions::default()
            },
        )
        .unwrap()
        .0;

        assert_eq!(
            plan.operations,
            vec![
                GraphMutationPlanOp::RemoveNodeProps {
                    id: NodeId::new("person-1"),
                    keys: vec!["nickname".to_string()],
                },
                GraphMutationPlanOp::PatchNode {
                    id: NodeId::new("person-1"),
                    props: Props::from([("name".to_string(), Value::from("Ada"))]),
                },
            ]
        );
    }

    #[test]
    fn cypher_match_set_multiple_assignments_rejects_invalid_items() {
        for cypher in [
            "MATCH (n:Person {id: 'person-1'}) SET n.name = 'Ada', m.name = 'Bob'",
            "MATCH (:Person {id: 'a'})-[e:KNOWS]->(n:Person {id: 'b'}) SET e.weight = n.weight + 1, e.note = 'x'",
            "MATCH (n:Person {id: 'person-1'}) SET n.name = 'Ada',",
        ] {
            let error =
                sail_cypher_mutation_plan(cypher).expect_err("invalid assignment list should fail");
            assert!(is_cypher_planning_error(&error));
        }
    }

    #[test]
    fn cypher_null_assignment_option_removes_properties() {
        let options = CypherMutationOptions {
            null_assignment: CypherNullAssignment::RemoveProperty,
            ..CypherMutationOptions::default()
        };

        let resolved_node = sail_cypher_mutation_plan_with_options(
            "MATCH (n:Person {id: 'person-1'}) SET n.nickname = null",
            options.clone(),
        )
        .unwrap()
        .0;
        assert_eq!(
            resolved_node.operations,
            vec![GraphMutationPlanOp::RemoveNodeProps {
                id: NodeId::new("person-1"),
                keys: vec!["nickname".to_string()],
            }]
        );

        let broad_node = sail_cypher_mutation_plan_with_options(
            "MATCH (n:Person {status: 'inactive'}) SET n.nickname = null",
            options.clone(),
        )
        .unwrap()
        .0;
        assert_eq!(
            broad_node.operations,
            vec![GraphMutationPlanOp::RemoveMatchingNodeProps {
                label: Some(Label::new("Person")),
                props: Props::from([("status".to_string(), Value::from("inactive"))]),
                predicates: Vec::new(),
                keys: vec!["nickname".to_string()],
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );

        let resolved_edge = sail_cypher_mutation_plan_with_options(
            "MATCH (:Person {id: 'person-1'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'person-2'}) SET e.note = null",
            options.clone(),
        )
        .unwrap()
        .0;
        assert_eq!(
            resolved_edge.operations,
            vec![GraphMutationPlanOp::RemoveEdgeProps {
                from: NodeId::new("person-1"),
                label: Label::new("KNOWS"),
                to: NodeId::new("person-2"),
                id: Some(EdgeId::new("edge-1")),
                keys: vec!["note".to_string()],
            }]
        );

        let broad_edge = sail_cypher_mutation_plan_with_options(
            "MATCH (:Person {id: 'person-1'})-[e:KNOWS {active: true}]->(:Person {status: 'inactive'}) SET e.note = null",
            options,
        )
        .unwrap()
        .0;
        assert_eq!(
            broad_edge.operations,
            vec![GraphMutationPlanOp::RemoveMatchingEdgeProps {
                relationship: GraphRelationshipMatch {
                    from: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("id".to_string(), Value::from("person-1"))]),
                        predicates: Vec::new(),
                    },
                    label: Label::new("KNOWS"),
                    to: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("status".to_string(), Value::from("inactive"))]),
                        predicates: Vec::new(),
                    },
                    id: None,
                    props: Props::from([("active".to_string(), Value::Bool(true))]),
                    predicates: Vec::new(),
                },
                keys: vec!["note".to_string()],
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_null_assignment_defaults_to_storing_null() {
        let node =
            sail_cypher_mutation_plan("MATCH (n:Person {id: 'person-1'}) SET n.nickname = null")
                .unwrap();
        assert_eq!(
            node.operations,
            vec![GraphMutationPlanOp::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([("nickname".to_string(), Value::Null)]),
            }]
        );

        let map_patch = sail_cypher_mutation_plan_with_options(
            "MATCH (n:Person {id: 'person-1'}) SET n += {nickname: null}",
            CypherMutationOptions {
                null_assignment: CypherNullAssignment::RemoveProperty,
                ..CypherMutationOptions::default()
            },
        )
        .unwrap()
        .0;
        assert_eq!(
            map_patch.operations,
            vec![GraphMutationPlanOp::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([("nickname".to_string(), Value::Null)]),
            }]
        );
    }

    #[test]
    fn cypher_match_set_numeric_expression_lowers_node_updates() {
        let resolved =
            sail_cypher_mutation_plan("MATCH (n:Counter {id: 'c1'}) SET n.count = n.count + 1")
                .unwrap();
        assert_eq!(
            resolved.report(),
            GraphMutationReport {
                patches: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            resolved.operations,
            vec![GraphMutationPlanOp::UpdateMatchingNodeProperty {
                label: None,
                props: Props::from([("id".to_string(), Value::from("c1"))]),
                predicates: Vec::new(),
                target_key: "count".to_string(),
                source_key: "count".to_string(),
                op: GraphNumericOp::Add,
                operand: Value::Int(1),
                cardinality: GraphMutationCardinality::SingleIdentity,
            }]
        );
        assert_eq!(
            resolved.into_mutations(),
            vec![GraphMutation::UpdateMatchingNodeProperty {
                label: None,
                props: Props::from([("id".to_string(), Value::from("c1"))]),
                predicates: Vec::new(),
                target_key: "count".to_string(),
                source_key: "count".to_string(),
                op: GraphNumericOp::Add,
                operand: Value::Int(1),
            }]
        );

        let broad = sail_cypher_mutation_plan_with_options(
            "MATCH (n:Counter {active: true}) SET n.count = n.count + $delta",
            CypherMutationOptions {
                parameters: CypherParameters::from([("delta".to_string(), Value::Int(2))]),
                ..CypherMutationOptions::default()
            },
        )
        .unwrap()
        .0;
        assert_eq!(
            broad.operations,
            vec![GraphMutationPlanOp::UpdateMatchingNodeProperty {
                label: Some(Label::new("Counter")),
                props: Props::from([("active".to_string(), Value::Bool(true))]),
                predicates: Vec::new(),
                target_key: "count".to_string(),
                source_key: "count".to_string(),
                op: GraphNumericOp::Add,
                operand: Value::Int(2),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );

        let unbounded = sail_cypher_mutation_plan("MATCH (n) SET n.score = n.score / 2").unwrap();
        assert_eq!(
            unbounded.operations,
            vec![GraphMutationPlanOp::UpdateMatchingNodeProperty {
                label: None,
                props: Props::new(),
                predicates: Vec::new(),
                target_key: "score".to_string(),
                source_key: "score".to_string(),
                op: GraphNumericOp::Divide,
                operand: Value::Int(2),
                cardinality: GraphMutationCardinality::UnboundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_set_numeric_expression_lowers_edge_updates() {
        let resolved = sail_cypher_mutation_plan(
            "MATCH (:Person {id: 'a'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'b'})
             SET e.weight = e.weight + 1",
        )
        .unwrap();
        let relationship = GraphRelationshipMatch {
            from: GraphNodeMatch {
                label: None,
                props: Props::from([("id".to_string(), Value::from("a"))]),
                predicates: Vec::new(),
            },
            label: Label::new("KNOWS"),
            to: GraphNodeMatch {
                label: None,
                props: Props::from([("id".to_string(), Value::from("b"))]),
                predicates: Vec::new(),
            },
            id: Some(EdgeId::new("edge-1")),
            props: Props::new(),
            predicates: Vec::new(),
        };
        assert_eq!(
            resolved.report(),
            GraphMutationReport {
                patches: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            resolved.operations,
            vec![GraphMutationPlanOp::UpdateMatchingEdgeProperty {
                relationship: relationship.clone(),
                target_key: "weight".to_string(),
                source_key: "weight".to_string(),
                op: GraphNumericOp::Add,
                operand: Value::Int(1),
                cardinality: GraphMutationCardinality::SingleIdentity,
            }]
        );
        assert_eq!(
            resolved.into_mutations(),
            vec![GraphMutation::UpdateMatchingEdgeProperty {
                relationship,
                target_key: "weight".to_string(),
                source_key: "weight".to_string(),
                op: GraphNumericOp::Add,
                operand: Value::Int(1),
            }]
        );

        let broad = sail_cypher_mutation_plan_with_options(
            "MATCH (:Person {status: 'active'})-[e:KNOWS {active: true}]->(:Person {status: 'active'})
             SET e.weight = e.weight * $factor",
            CypherMutationOptions {
                parameters: CypherParameters::from([("factor".to_string(), Value::Int(2))]),
                ..CypherMutationOptions::default()
            },
        )
        .unwrap()
        .0;
        assert_eq!(
            broad.operations,
            vec![GraphMutationPlanOp::UpdateMatchingEdgeProperty {
                relationship: GraphRelationshipMatch {
                    from: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("status".to_string(), Value::from("active"))]),
                        predicates: Vec::new(),
                    },
                    label: Label::new("KNOWS"),
                    to: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("status".to_string(), Value::from("active"))]),
                        predicates: Vec::new(),
                    },
                    id: None,
                    props: Props::from([("active".to_string(), Value::Bool(true))]),
                    predicates: Vec::new(),
                },
                target_key: "weight".to_string(),
                source_key: "weight".to_string(),
                op: GraphNumericOp::Multiply,
                operand: Value::Int(2),
                cardinality: GraphMutationCardinality::BoundedMany,
            }]
        );
    }

    #[test]
    fn cypher_match_set_numeric_expression_rejects_unsupported_forms() {
        for cypher in [
            "MATCH (n:Counter {id: 'c1'}) SET n.count = m.count + 1",
            "MATCH (n:Counter {id: 'c1'}) SET n.count = n.count + m.delta",
            "MATCH (n:Counter {id: 'c1'}) SET n.count = size([])",
            "MATCH (n:Counter {id: 'c1'}) SET n.count = CASE n.count WHEN 1 THEN 2 END",
            "MATCH (:Person {id: 'a'})-[e:KNOWS]->(:Person {id: 'b'}) SET e.weight = n.weight + 1",
        ] {
            let error =
                sail_cypher_mutation_plan(cypher).expect_err("unsupported expression should fail");
            assert!(is_cypher_planning_error(&error));
        }

        let non_numeric_parameter = sail_cypher_mutation_plan_with_options(
            "MATCH (n:Counter {id: 'c1'}) SET n.count = n.count + $delta",
            CypherMutationOptions {
                parameters: CypherParameters::from([("delta".to_string(), Value::from("one"))]),
                ..CypherMutationOptions::default()
            },
        )
        .expect_err("non-numeric expression parameter should fail");
        assert!(matches!(non_numeric_parameter, GrustError::CypherSyntax(_)));
    }

    #[test]
    fn cypher_match_remove_lowers_resolved_node_and_edge_properties() {
        let node = sail_cypher_mutation_plan("MATCH (n:Person {id: 'person-1'}) REMOVE n.nickname")
            .unwrap();
        assert_eq!(
            node.report(),
            GraphMutationReport {
                property_removes: 1,
                changed_nodes: 1,
                node_property_removes: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            node.into_mutations(),
            vec![GraphMutation::RemoveNodeProps {
                id: NodeId::new("person-1"),
                keys: vec!["nickname".to_string()],
            }]
        );

        let edge = sail_cypher_mutation_plan(
            "MATCH (:Person {id: 'person-1'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'person-2'}) REMOVE e.note",
        )
        .unwrap();
        assert_eq!(
            edge.into_mutations(),
            vec![GraphMutation::RemoveEdgeProps {
                from: NodeId::new("person-1"),
                label: Label::new("KNOWS"),
                to: NodeId::new("person-2"),
                id: Some(EdgeId::new("edge-1")),
                keys: vec!["note".to_string()],
            }]
        );

        let broad_edge = sail_cypher_mutation_plan(
            "MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {status: 'inactive'}) REMOVE e.note",
        )
        .unwrap();
        assert_eq!(
            broad_edge.into_mutations(),
            vec![GraphMutation::RemoveMatchingEdgeProps {
                relationship: GraphRelationshipMatch {
                    from: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("id".to_string(), Value::from("person-1"))]),
                        predicates: Vec::new(),
                    },
                    label: Label::new("KNOWS"),
                    to: GraphNodeMatch {
                        label: Some(Label::new("Person")),
                        props: Props::from([("status".to_string(), Value::from("inactive"))]),
                        predicates: Vec::new(),
                    },
                    id: None,
                    props: Props::new(),
                    predicates: Vec::new(),
                },
                keys: vec!["note".to_string()],
            }]
        );

        let broad =
            sail_cypher_mutation_plan("MATCH (n:Person {status: 'inactive'}) REMOVE n.nickname")
                .unwrap();
        assert_eq!(
            broad.report(),
            GraphMutationReport {
                property_removes: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            broad.into_mutations(),
            vec![GraphMutation::RemoveMatchingNodeProps {
                label: Some(Label::new("Person")),
                props: Props::from([("status".to_string(), Value::from("inactive"))]),
                predicates: Vec::new(),
                keys: vec!["nickname".to_string()],
            }]
        );
    }

    #[test]
    fn cypher_match_set_rejects_deferred_patch_forms() {
        for cypher in ["MATCH (n:Person {id: 'person-1'}) SET m += {name: 'Ada'}"] {
            let error =
                sail_cypher_mutation_plan(cypher).expect_err("unsupported MATCH SET must fail");
            assert!(is_cypher_planning_error(&error));
        }
    }

    #[test]
    fn cypher_multi_statement_batch_preserves_order_and_aggregates_report() {
        let plan = sail_cypher_mutation_plan(
            "
            CREATE (:Person {id: 'person-1', name: 'Ada; still one literal'});
            MERGE (:Person {id: 'person-2', name: 'Bob'});
            CREATE (:Person {id: 'person-1'})-[:KNOWS {since: 2026}]->(:Person {id: 'person-2'});
            DELETE (:Person {id: 'person-2'});
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
                        (
                            "name".to_string(),
                            Value::String("Ada; still one literal".to_string())
                        ),
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
                GraphMutation::UpsertEdge(Edge::new(
                    "KNOWS",
                    "person-1",
                    "person-2",
                    Props::from([("since".to_string(), Value::Int(2026))]),
                )),
                GraphMutation::DeleteNode(NodeId::new("person-2")),
            ]
        );
    }

    #[test]
    fn cypher_plan_executes_on_memory_facade() {
        let plan = sail_cypher_mutation_plan(
            "
            CREATE (:Person {id: 'person-1', status: 'inactive', score: 11});
            CREATE (:Person {id: 'person-2', status: 'inactive', score: 12});
            CREATE (:Person {id: 'person-3', status: 'active', score: 20});
            MATCH (a:Person {id: 'person-1'}), (b:Person {id: 'person-2'})
            CREATE (a)-[:KNOWS]->(b);
            MATCH (n:Person) WHERE n.status = 'inactive' AND n.score >= 10 SET n += {archived: true};
            MATCH (n:Person) WHERE n.archived = true DELETE n;
            ",
        )
        .unwrap();
        let store = MemoryGraphStore::new();

        let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&plan)).unwrap();

        assert_eq!(
            report,
            GraphMutationReport {
                creates: 4,
                deletes: 1,
                patches: 1,
                matched_rows: 4,
                changed_nodes: 7,
                changed_edges: 2,
                node_upserts: 3,
                edge_upserts: 1,
                node_deletes: 2,
                edge_deletes: 1,
                node_patches: 2,
                node_inserts: 3,
                edge_inserts: 1,
                ..GraphMutationReport::default()
            }
        );
        assert!(
            futures_executor::block_on(store.get_node(&NodeId::new("person-1")))
                .unwrap()
                .is_none()
        );
        assert!(
            futures_executor::block_on(store.get_node(&NodeId::new("person-2")))
                .unwrap()
                .is_none()
        );
        assert!(
            futures_executor::block_on(store.get_node(&NodeId::new("person-3")))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn cypher_multi_target_delete_executes_on_memory_facade() {
        let store = MemoryGraphStore::new();
        futures_executor::block_on(store.put_graph(&Graph::new(
            vec![
                Node::new("Person", "delete-a", Props::new()),
                Node::new("Person", "delete-b", Props::new()),
            ],
            vec![Edge::new("KNOWS", "delete-a", "delete-b", Props::new())],
        )))
        .unwrap();

        let plan = sail_cypher_mutation_plan(
            "
            MATCH (a:Person {id: 'delete-a'})-[e:KNOWS]->(b:Person {id: 'delete-b'})
            DELETE e, a;
            ",
        )
        .unwrap();
        let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&plan)).unwrap();

        assert_eq!(
            report,
            GraphMutationReport {
                deletes: 2,
                changed_nodes: 1,
                changed_edges: 1,
                node_deletes: 1,
                edge_deletes: 1,
                ..GraphMutationReport::default()
            }
        );
        assert!(
            futures_executor::block_on(store.get_node(&NodeId::new("delete-a")))
                .unwrap()
                .is_none()
        );
        assert!(
            futures_executor::block_on(store.get_node(&NodeId::new("delete-b")))
                .unwrap()
                .is_some()
        );
        assert!(
            futures_executor::block_on(store.get_edges(EdgeQuery {
                from: Some(NodeId::new("delete-a")),
                to: Some(NodeId::new("delete-b")),
                label: Some(Label::new("KNOWS")),
            }))
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn cypher_relationship_row_endpoint_delete_executes_on_memory_facade() {
        let store = MemoryGraphStore::new();
        futures_executor::block_on(store.put_graph(&Graph::new(
            vec![
                Node::new(
                    "Person",
                    "delete-row-a",
                    Props::from([("status".to_string(), Value::from("inactive"))]),
                ),
                Node::new(
                    "Person",
                    "delete-row-b",
                    Props::from([("status".to_string(), Value::from("inactive"))]),
                ),
                Node::new("Person", "delete-row-c", Props::new()),
                Node::new("Person", "keep-row-d", Props::new()),
            ],
            vec![
                Edge::new("KNOWS", "delete-row-a", "delete-row-c", Props::new()),
                Edge::new("KNOWS", "delete-row-b", "delete-row-c", Props::new()),
                Edge::new("KNOWS", "keep-row-d", "delete-row-c", Props::new()),
            ],
        )))
        .unwrap();

        let plan = sail_cypher_mutation_plan(
            "
            MATCH (a:Person {status: 'inactive'})-[e:KNOWS]->(b:Person {id: 'delete-row-c'})
            DELETE a;
            ",
        )
        .unwrap();
        let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&plan)).unwrap();

        assert_eq!(
            report,
            GraphMutationReport {
                deletes: 1,
                matched_rows: 2,
                changed_nodes: 2,
                changed_edges: 2,
                node_deletes: 2,
                edge_deletes: 2,
                ..GraphMutationReport::default()
            }
        );
        for id in ["delete-row-a", "delete-row-b"] {
            assert!(
                futures_executor::block_on(store.get_node(&NodeId::new(id)))
                    .unwrap()
                    .is_none()
            );
        }
        for id in ["delete-row-c", "keep-row-d"] {
            assert!(
                futures_executor::block_on(store.get_node(&NodeId::new(id)))
                    .unwrap()
                    .is_some()
            );
        }
        assert_eq!(
            futures_executor::block_on(store.get_edges(EdgeQuery::default())).unwrap(),
            vec![Edge::new(
                "KNOWS",
                "keep-row-d",
                "delete-row-c",
                Props::new()
            )]
        );
    }

    #[test]
    fn cypher_mixed_relationship_row_delete_executes_on_memory_facade() {
        let store = MemoryGraphStore::new();
        futures_executor::block_on(store.put_graph(&Graph::new(
            vec![
                Node::new(
                    "Person",
                    "delete-mixed-a",
                    Props::from([("status".to_string(), Value::from("inactive"))]),
                ),
                Node::new("Person", "delete-mixed-b", Props::new()),
                Node::new("Person", "keep-mixed-c", Props::new()),
            ],
            vec![
                Edge::new("KNOWS", "delete-mixed-a", "delete-mixed-b", Props::new()),
                Edge::new("KNOWS", "keep-mixed-c", "delete-mixed-b", Props::new()),
            ],
        )))
        .unwrap();

        let plan = sail_cypher_mutation_plan(
            "
            MATCH (a:Person {status: 'inactive'})-[e:KNOWS]->(b:Person {id: 'delete-mixed-b'})
            DELETE e, a;
            ",
        )
        .unwrap();
        let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&plan)).unwrap();

        assert_eq!(
            report,
            GraphMutationReport {
                deletes: 2,
                matched_rows: 1,
                changed_nodes: 1,
                changed_edges: 1,
                node_deletes: 1,
                edge_deletes: 1,
                ..GraphMutationReport::default()
            }
        );
        assert!(
            futures_executor::block_on(store.get_node(&NodeId::new("delete-mixed-a")))
                .unwrap()
                .is_none()
        );
        assert!(
            futures_executor::block_on(store.get_node(&NodeId::new("delete-mixed-b")))
                .unwrap()
                .is_some()
        );
        assert_eq!(
            futures_executor::block_on(store.get_edges(EdgeQuery::default())).unwrap(),
            vec![Edge::new(
                "KNOWS",
                "keep-mixed-c",
                "delete-mixed-b",
                Props::new()
            )]
        );
    }

    #[test]
    fn cypher_multiple_set_assignments_execute_in_order_on_memory_facade() {
        let plan = sail_cypher_mutation_plan(
            "
            CREATE (:Counter {id: 'c1', count: 1});
            MATCH (n:Counter {id: 'c1'}) SET n.count = n.count + 1, n.count = n.count * 2;
            ",
        )
        .unwrap();
        let store = MemoryGraphStore::new();

        let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&plan)).unwrap();

        assert_eq!(
            report,
            GraphMutationReport {
                creates: 1,
                patches: 2,
                matched_rows: 2,
                changed_nodes: 3,
                node_upserts: 1,
                node_patches: 2,
                node_inserts: 1,
                ..GraphMutationReport::default()
            }
        );
        let node = futures_executor::block_on(store.get_node(&NodeId::new("c1")))
            .unwrap()
            .expect("counter node");
        assert_eq!(node.props.get("count"), Some(&Value::Int(4)));
    }

    #[test]
    fn cypher_returning_executes_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'ada', name: 'Ada', order: 'first', limit: 3});
                MATCH (n:Person {id: 'ada'})
                SET n.seen = true, n.count = 1
                RETURN n.id, n.label, n.seen AS seen, n.order, n.limit, n.missing;
                ",
                CypherMutationOptions {
                    collect_written_node_identities: true,
                    ..CypherMutationOptions::default()
                },
            ))
            .unwrap();

        assert_eq!(
            result.mutation.report,
            GraphMutationReport {
                creates: 1,
                patches: 2,
                changed_nodes: 3,
                node_upserts: 1,
                node_patches: 2,
                node_inserts: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            result.mutation.written_node_identities,
            vec![CypherWrittenNodeIdentity {
                kind: GraphMutationPlanKind::Create,
                label: Label::new("Person"),
                id: NodeId::new("ada"),
            }]
        );
        assert_eq!(
            result.table,
            CypherResultTable {
                columns: vec![
                    "n.id".to_string(),
                    "n.label".to_string(),
                    "seen".to_string(),
                    "n.order".to_string(),
                    "n.limit".to_string(),
                    "n.missing".to_string()
                ],
                rows: vec![vec![
                    Value::from("ada"),
                    Value::from("Person"),
                    Value::Bool(true),
                    Value::from("first"),
                    Value::Int(3),
                    Value::Null,
                ]],
            }
        );
    }

    #[test]
    fn cypher_returning_projects_bound_edge_properties_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'ada'});
                CREATE (:Person {id: 'bob'});
                CREATE (:Person {id: 'ada'})-[:KNOWS {id: 'edge-1'}]->(:Person {id: 'bob'});
                MATCH (:Person {id: 'ada'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'bob'})
                SET e.weight = 2
                RETURN e.id, e.label, e.weight, e.missing;
                ",
                CypherMutationOptions::default(),
            ))
            .unwrap();

        assert_eq!(
            result.mutation.report,
            GraphMutationReport {
                creates: 3,
                patches: 1,
                changed_nodes: 2,
                changed_edges: 2,
                node_upserts: 2,
                edge_upserts: 1,
                edge_patches: 1,
                node_inserts: 2,
                edge_inserts: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            result.table,
            CypherResultTable {
                columns: vec![
                    "e.id".to_string(),
                    "e.label".to_string(),
                    "e.weight".to_string(),
                    "e.missing".to_string()
                ],
                rows: vec![vec![
                    Value::from("edge-1"),
                    Value::from("KNOWS"),
                    Value::Int(2),
                    Value::Null
                ]],
            }
        );
    }

    #[test]
    fn cypher_numeric_edge_updates_execute_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let resolved =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'edge-num-a'});
                CREATE (b:Person {id: 'edge-num-b'});
                CREATE (a)-[e:KNOWS {id: 'edge-num-1', weight: 2}]->(b);
                MATCH (a:Person {id: 'edge-num-a'})-[e:KNOWS {id: 'edge-num-1'}]->(b:Person {id: 'edge-num-b'})
                SET e.weight = e.weight + 3
                RETURN e.weight AS weight;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("resolved edge numeric update");
        assert_eq!(
            resolved.table,
            CypherResultTable {
                columns: vec!["weight".to_string()],
                rows: vec![vec![Value::Int(5)]],
            }
        );
        assert_eq!(resolved.mutation.report.edge_patches, 1);
        assert_eq!(resolved.mutation.report.changed_edges, 2);

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'edge-num-c', status: 'edge-num'});
                CREATE (:Person {id: 'edge-num-d', status: 'edge-num'});
                CREATE (:Person {id: 'edge-num-e', status: 'edge-num'});
                CREATE (:Person {id: 'edge-num-c'})-[:LIKES {active: true, weight: 2}]->(:Person {id: 'edge-num-e'});
                CREATE (:Person {id: 'edge-num-d'})-[:LIKES {active: true, weight: 4}]->(:Person {id: 'edge-num-e'});
                MATCH (n:Person {status: 'edge-num'})-[e:LIKES {active: true}]->(t:Person {id: 'edge-num-e'})
                SET e.weight = e.weight * $factor
                RETURN e.weight AS weight
                ORDER BY weight;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([("factor".to_string(), Value::Int(2))]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("broad edge numeric update");
        assert_eq!(
            broad.table,
            CypherResultTable {
                columns: vec!["weight".to_string()],
                rows: vec![vec![Value::Int(4)], vec![Value::Int(8)]],
            }
        );
        assert_eq!(broad.mutation.report.matched_rows, 2);
        assert_eq!(broad.mutation.report.edge_patches, 2);
        assert_eq!(broad.mutation.report.changed_edges, 4);
    }

    #[test]
    fn cypher_returning_projects_new_concrete_edge_properties_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let top_level =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'ada'});
                CREATE (b:Person {id: 'bob'});
                CREATE (a)-[e:KNOWS {id: 'edge-1', since: 2026}]->(b)
                RETURN e.id, e.since;
                ",
                CypherMutationOptions::default(),
            ))
            .unwrap();

        assert_eq!(
            top_level.table,
            CypherResultTable {
                columns: vec!["e.id".to_string(), "e.since".to_string()],
                rows: vec![vec![Value::from("edge-1"), Value::Int(2026)]],
            }
        );

        let match_create =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (a:Person {id: 'ada'}), (b:Person {id: 'bob'})
                CREATE (a)-[e:WORKS_WITH {id: 'edge-2', weight: 4}]->(b)
                RETURN e.id, e.weight;
                ",
                CypherMutationOptions::default(),
            ))
            .unwrap();

        assert_eq!(
            match_create.table,
            CypherResultTable {
                columns: vec!["e.id".to_string(), "e.weight".to_string()],
                rows: vec![vec![Value::from("edge-2"), Value::Int(4)]],
            }
        );
    }

    #[test]
    fn cypher_returning_projects_bound_elements_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'ada', name: 'Ada'});
                CREATE (b:Person {id: 'bob'});
                CREATE (a)-[e:KNOWS {id: 'edge-1', since: 2026}]->(b)
                RETURN a AS node, e AS relationship;
                ",
                CypherMutationOptions::default(),
            ))
            .unwrap();

        assert_eq!(
            result.table,
            CypherResultTable {
                columns: vec!["node".to_string(), "relationship".to_string()],
                rows: vec![vec![
                    Value::from(serde_json::json!({
                        "id": "ada",
                        "label": "Person",
                        "props": {
                            "id": {"type": "string", "value": "ada"},
                            "name": {"type": "string", "value": "Ada"}
                        }
                    })),
                    Value::from(serde_json::json!({
                        "id": "edge-1",
                        "from": "ada",
                        "to": "bob",
                        "label": "KNOWS",
                        "props": {
                            "id": {"type": "string", "value": "edge-1"},
                            "since": {"type": "int", "value": 2026}
                        }
                    }))
                ]],
            }
        );
    }

    #[test]
    fn cypher_returning_projects_star_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'star-ada', name: 'Ada'});
                CREATE (b:Person {id: 'star-bob'});
                CREATE (a)-[e:KNOWS {id: 'star-edge', since: 2026}]->(b)
                RETURN *;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("concrete RETURN *");
        assert_eq!(
            concrete.table.columns,
            vec!["a".to_string(), "b".to_string(), "e".to_string()]
        );
        assert_eq!(concrete.table.rows.len(), 1);
        assert_eq!(concrete.table.rows[0].len(), 3);
        let Value::Json(a) = &concrete.table.rows[0][0] else {
            panic!("RETURN * should project concrete node a");
        };
        let Value::Json(b) = &concrete.table.rows[0][1] else {
            panic!("RETURN * should project concrete node b");
        };
        let Value::Json(e) = &concrete.table.rows[0][2] else {
            panic!("RETURN * should project concrete relationship e");
        };
        assert_eq!(a["id"], serde_json::json!("star-ada"));
        assert_eq!(b["id"], serde_json::json!("star-bob"));
        assert_eq!(e["id"], serde_json::json!("star-edge"));

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'star-cara', status: 'active'});
                CREATE (:Person {id: 'star-dana', status: 'active'});
                MATCH (n:Person {status: 'active'}) SET n.seen = true
                RETURN *, n.id AS id ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad node RETURN *");
        assert_eq!(broad.table.columns, vec!["n".to_string(), "id".to_string()]);
        assert_eq!(broad.table.rows.len(), 2);
        assert_eq!(broad.table.rows[0][1], Value::from("star-cara"));
        assert_eq!(broad.table.rows[1][1], Value::from("star-dana"));
        let Value::Json(n) = &broad.table.rows[0][0] else {
            panic!("RETURN * should project broad node n");
        };
        assert_eq!(n["id"], serde_json::json!("star-cara"));

        let row_edge =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'star-team'});
                MATCH (n:Person {status: 'active'}), (t:Team {id: 'star-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'star'}]->(t)
                RETURN *, r.source AS source;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship RETURN *");
        assert_eq!(
            row_edge.table.columns,
            vec![
                "n".to_string(),
                "r".to_string(),
                "t".to_string(),
                "source".to_string()
            ]
        );
        assert_eq!(row_edge.table.rows.len(), 2);
        for row in &row_edge.table.rows {
            let Value::Json(person) = &row[0] else {
                panic!("RETURN * should project matched source n");
            };
            let Value::Json(edge) = &row[1] else {
                panic!("RETURN * should project row-producing relationship r");
            };
            let Value::Json(team) = &row[2] else {
                panic!("RETURN * should project matched endpoint t");
            };
            assert!(
                ["star-cara", "star-dana"].contains(&person["id"].as_str().expect("person id"))
            );
            assert_eq!(edge["label"], serde_json::json!("MEMBER_OF"));
            assert_eq!(team["id"], serde_json::json!("star-team"));
            assert_eq!(row[3], Value::from("star"));
        }

        let explicit_source =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'active'}), (t:Team {id: 'star-team'})
                MERGE (n)-[r:WORKS_ON {source: 'explicit'}]->(t)
                RETURN n.id AS person, r.source AS source ORDER BY person;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship source endpoint RETURN");
        assert_eq!(
            explicit_source.table,
            CypherResultTable {
                columns: vec!["person".to_string(), "source".to_string()],
                rows: vec![
                    vec![Value::from("star-cara"), Value::from("explicit")],
                    vec![Value::from("star-dana"), Value::from("explicit")]
                ],
            }
        );
    }

    #[test]
    fn cypher_returning_row_model_preserves_alignment_and_star_order() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'row-model-ada', status: 'row-model'});
                CREATE (:Person {id: 'row-model-bob', status: 'row-model'});
                CREATE (:Team {id: 'row-model-eng', kind: 'row-model'});
                CREATE (:Team {id: 'row-model-ops', kind: 'row-model'});
                MATCH (n:Person {status: 'row-model'}), (t:Team {kind: 'row-model'})
                CREATE (n)-[r:ASSIGNED {source: 'row-model'}]->(t)
                RETURN *, n.id AS person, t.id AS team
                ORDER BY person, team;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing RETURN should preserve row alignment");

        assert_eq!(
            result.table.columns,
            vec![
                "n".to_string(),
                "r".to_string(),
                "t".to_string(),
                "person".to_string(),
                "team".to_string()
            ]
        );
        assert_eq!(result.table.rows.len(), 4);
        for row in &result.table.rows {
            let Value::Json(person) = &row[0] else {
                panic!("RETURN * should include source endpoint node");
            };
            let Value::Json(edge) = &row[1] else {
                panic!("RETURN * should include produced relationship");
            };
            let Value::Json(team) = &row[2] else {
                panic!("RETURN * should include target endpoint node");
            };
            assert_eq!(person["id"], row[3].to_json());
            assert_eq!(team["id"], row[4].to_json());
            assert_eq!(edge["from"], row[3].to_json());
            assert_eq!(edge["to"], row[4].to_json());
        }

        let collected =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'row-model'}), (t:Team {kind: 'row-model'})
                MERGE (n)-[r:ASSIGNED_AGAIN {source: 'row-model'}]->(t)
                RETURN collect(*) AS rows;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("collect(*) should use the same row model");
        let Value::Json(rows) = &collected.table.rows[0][0] else {
            panic!("collect(*) should return JSON rows");
        };
        let rows = rows.as_array().expect("collect(*) array");
        assert_eq!(rows.len(), 4);
        let first = rows[0].as_object().expect("row object");
        assert_eq!(
            first.keys().cloned().collect::<Vec<_>>(),
            vec!["n".to_string(), "r".to_string(), "t".to_string()]
        );
    }

    #[test]
    fn cypher_write_result_rows_validate_path_endpoint_alignment() {
        let mut row_node_values = HashMap::new();
        row_node_values.insert(
            "n".to_string(),
            vec![Node::new("Person", "row-path-a", Props::new())],
        );
        row_node_values.insert(
            "t".to_string(),
            vec![
                Node::new("Team", "row-path-eng", Props::new()),
                Node::new("Team", "row-path-ops", Props::new()),
            ],
        );
        let mut row_edge_values = HashMap::new();
        row_edge_values.insert(
            "r".to_string(),
            vec![
                Edge::new("ASSIGNED", "row-path-a", "row-path-eng", Props::new()),
                Edge::new("ASSIGNED", "row-path-a", "row-path-ops", Props::new()),
            ],
        );
        let mut row_path_bindings = HashMap::new();
        row_path_bindings.insert(
            "p".to_string(),
            CypherRowProducedPathBinding {
                from_variable: "n".to_string(),
                edge_variable: "r".to_string(),
                to_variable: "t".to_string(),
            },
        );
        let return_clause = CypherReturnClause {
            projections: vec![CypherReturnProjection {
                variable: "p".to_string(),
                target: CypherReturnTarget::Element,
                column: "p".to_string(),
                expression: "p".to_string(),
                element: CypherReturnElement::RowPath,
                aggregate: None,
                distinct: false,
            }],
            order_by: Vec::new(),
            skip: None,
            limit: None,
            distinct: false,
        };

        let err =
            CypherWriteResultRows::new(&row_node_values, &row_edge_values, &row_path_bindings)
                .row_count_for_return(&return_clause)
                .expect_err("path rows must validate endpoint and edge row counts");
        assert!(
            matches!(err, GrustError::CypherUnsupportedCardinality(_)),
            "{err:?}"
        );
    }

    #[test]
    fn cypher_returning_projects_row_producing_paths_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'path-ada', status: 'path'});
                CREATE (:Person {id: 'path-bob', status: 'path'});
                CREATE (:Team {id: 'path-team'});
                MATCH (n:Person {status: 'path'}), (t:Team {id: 'path-team'})
                CREATE p = (n)-[r:MEMBER_OF {source: 'path'}]->(t)
                RETURN p,
                       length(p) AS hops,
                       nodes(p) AS path_nodes,
                       relationships(p) AS path_relationships,
                       n.id AS person,
                       r.source AS source
                ORDER BY person;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing path RETURN");

        assert_eq!(
            result.table.columns,
            vec![
                "p".to_string(),
                "hops".to_string(),
                "path_nodes".to_string(),
                "path_relationships".to_string(),
                "person".to_string(),
                "source".to_string()
            ]
        );
        assert_eq!(result.table.rows.len(), 2);
        assert_eq!(result.table.rows[0][4], Value::from("path-ada"));
        assert_eq!(result.table.rows[1][4], Value::from("path-bob"));
        for row in &result.table.rows {
            let Value::Json(path) = &row[0] else {
                panic!("RETURN p should project a JSON path");
            };
            assert_eq!(row[1], Value::Int(1));
            assert_eq!(path["nodes"][0]["id"], row[4].to_json());
            assert_eq!(path["nodes"][1]["id"], serde_json::json!("path-team"));
            assert_eq!(path["relationships"][0]["from"], row[4].to_json());
            assert_eq!(
                path["relationships"][0]["to"],
                serde_json::json!("path-team")
            );
            assert_eq!(
                path["relationships"][0]["label"],
                serde_json::json!("MEMBER_OF")
            );
            assert_eq!(row[2].to_json(), path["nodes"]);
            assert_eq!(row[3].to_json(), path["relationships"]);
            assert_eq!(row[5], Value::from("path"));
        }

        let star =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'path'}), (t:Team {id: 'path-team'})
                MERGE q = (n)-[r:WORKS_ON {source: 'path-star'}]->(t)
                RETURN *
                LIMIT 1;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing path RETURN *");
        assert_eq!(
            star.table.columns,
            vec![
                "n".to_string(),
                "q".to_string(),
                "r".to_string(),
                "t".to_string()
            ]
        );
        let Value::Json(path) = &star.table.rows[0][1] else {
            panic!("RETURN * should include the path variable");
        };
        assert_eq!(
            path["relationships"][0]["label"],
            serde_json::json!("WORKS_ON")
        );

        let resolved_path =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'path-resolved-a'});
                CREATE (b:Person {id: 'path-resolved-b'});
                MATCH (a:Person {id: 'path-resolved-a'}), (b:Person {id: 'path-resolved-b'})
                CREATE p = (a)-[r:KNOWS {id: 'path-resolved-r'}]->(b)
                RETURN p,
                       length(p) AS hops,
                       nodes(p) AS path_nodes,
                       relationships(p) AS path_relationships,
                       count(p) AS path_count,
                       collect(p) AS paths;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("resolved relationship path variables should project");
        assert_eq!(
            resolved_path.table.columns,
            vec![
                "p".to_string(),
                "hops".to_string(),
                "path_nodes".to_string(),
                "path_relationships".to_string(),
                "path_count".to_string(),
                "paths".to_string()
            ]
        );
        assert_eq!(resolved_path.table.rows.len(), 1);
        assert_eq!(resolved_path.table.rows[0][1], Value::Int(1));
        assert_eq!(resolved_path.table.rows[0][4], Value::Int(1));
        let Value::Json(path) = &resolved_path.table.rows[0][0] else {
            panic!("resolved RETURN p should project a JSON path");
        };
        assert_eq!(path["nodes"][0]["id"], serde_json::json!("path-resolved-a"));
        assert_eq!(path["nodes"][1]["id"], serde_json::json!("path-resolved-b"));
        assert_eq!(
            path["relationships"][0]["id"],
            serde_json::json!("path-resolved-r")
        );
        assert_eq!(resolved_path.table.rows[0][2].to_json(), path["nodes"]);
        assert_eq!(
            resolved_path.table.rows[0][3].to_json(),
            path["relationships"]
        );
        let Value::Json(paths) = &resolved_path.table.rows[0][5] else {
            panic!("resolved collect(p) should return JSON paths");
        };
        assert_eq!(paths.as_array().expect("resolved paths array").len(), 1);

        let path_function_on_node =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'path'}) SET n.path_function_checked = true
                RETURN nodes(n);
                ",
                CypherMutationOptions::default(),
            ))
            .expect_err("path functions should require path variables");
        assert!(
            matches!(
                path_function_on_node,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{path_function_on_node:?}"
        );

        let missing_relationship_variable =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'path'}), (t:Team {id: 'path-team'})
                CREATE p = (n)-[:MISSING_VAR]->(t)
                RETURN p;
                ",
                CypherMutationOptions::default(),
            ))
            .expect_err("path variables should require a relationship variable");
        assert!(
            matches!(missing_relationship_variable, GrustError::CypherSyntax(_)),
            "{missing_relationship_variable:?}"
        );

        let path_property =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'path'}), (t:Team {id: 'path-team'})
                CREATE p = (n)-[r:PATH_PROPERTY]->(t)
                RETURN p.id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect_err("path properties should stay deferred");
        assert!(
            matches!(path_property, GrustError::CypherUnsupportedCardinality(_)),
            "{path_property:?}"
        );

        let path_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'path'}), (t:Team {id: 'path-team'})
                CREATE p = (n)-[r:PATH_AGGREGATE]->(t)
                RETURN count(p) AS path_count,
                       count(DISTINCT p) AS distinct_path_count,
                       collect(p) AS paths;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted path aggregates should be supported");
        assert_eq!(
            path_aggregates.table.columns,
            vec![
                "path_count".to_string(),
                "distinct_path_count".to_string(),
                "paths".to_string()
            ]
        );
        assert_eq!(path_aggregates.table.rows[0][0], Value::Int(2));
        assert_eq!(path_aggregates.table.rows[0][1], Value::Int(2));
        let Value::Json(paths) = &path_aggregates.table.rows[0][2] else {
            panic!("collect(p) should return JSON paths");
        };
        let paths = paths.as_array().expect("path collection");
        assert_eq!(paths.len(), 2);
        assert_eq!(
            paths[0]["relationships"][0]["label"],
            serde_json::json!("PATH_AGGREGATE")
        );

        let path_function_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'path'}), (t:Team {id: 'path-team'})
                CREATE p = (n)-[r:PATH_FUNCTION_AGGREGATE]->(t)
                RETURN sum(length(p)) AS total_hops,
                       avg(length(p)) AS average_hops,
                       count(DISTINCT length(p)) AS distinct_lengths,
                       collect(nodes(p)) AS node_paths,
                       collect(relationships(p)) AS relationship_paths;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted path function aggregates should be supported");
        assert_eq!(
            path_function_aggregates.table.columns,
            vec![
                "total_hops".to_string(),
                "average_hops".to_string(),
                "distinct_lengths".to_string(),
                "node_paths".to_string(),
                "relationship_paths".to_string()
            ]
        );
        assert_eq!(path_function_aggregates.table.rows[0][0], Value::Int(2));
        assert_eq!(path_function_aggregates.table.rows[0][1], Value::Float(1.0));
        assert_eq!(path_function_aggregates.table.rows[0][2], Value::Int(1));
        let Value::Json(node_paths) = &path_function_aggregates.table.rows[0][3] else {
            panic!("collect(nodes(p)) should return JSON arrays");
        };
        let node_paths = node_paths.as_array().expect("node path collection");
        assert_eq!(node_paths.len(), 2);
        assert_eq!(node_paths[0].as_array().expect("node array").len(), 2);
        let Value::Json(relationship_paths) = &path_function_aggregates.table.rows[0][4] else {
            panic!("collect(relationships(p)) should return JSON arrays");
        };
        let relationship_paths = relationship_paths
            .as_array()
            .expect("relationship path collection");
        assert_eq!(relationship_paths.len(), 2);
        assert_eq!(
            relationship_paths[0][0]["label"],
            serde_json::json!("PATH_FUNCTION_AGGREGATE")
        );

        let path_function_aggregate_on_node =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'path'}) SET n.path_function_aggregate_checked = true
                RETURN count(length(n));
                ",
                CypherMutationOptions::default(),
            ))
            .expect_err("path function aggregates should require path variables");
        assert!(
            matches!(
                path_function_aggregate_on_node,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{path_function_aggregate_on_node:?}"
        );

        let grouped_path_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'path'}), (t:Team {id: 'path-team'})
                CREATE p = (n)-[r:PATH_GROUP]->(t)
                RETURN n.id AS person, count(p) AS path_count, collect(p) AS paths
                ORDER BY person;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("grouped path aggregates should be supported");
        assert_eq!(
            grouped_path_aggregates.table.columns,
            vec![
                "person".to_string(),
                "path_count".to_string(),
                "paths".to_string()
            ]
        );
        assert_eq!(grouped_path_aggregates.table.rows.len(), 2);
        for row in &grouped_path_aggregates.table.rows {
            assert_eq!(row[1], Value::Int(1));
            let Value::Json(paths) = &row[2] else {
                panic!("grouped collect(p) should return JSON paths");
            };
            let paths = paths.as_array().expect("grouped path collection");
            assert_eq!(paths.len(), 1);
            assert_eq!(paths[0]["nodes"][0]["id"], row[0].to_json());
            assert_eq!(
                paths[0]["relationships"][0]["label"],
                serde_json::json!("PATH_GROUP")
            );
        }
    }

    #[test]
    fn cypher_returning_projects_matched_relationship_paths_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'matched-path-ada', status: 'matched-path'});
                CREATE (:Person {id: 'matched-path-bob', status: 'matched-path'});
                CREATE (:Team {id: 'matched-path-team'});
                MATCH (n:Person {status: 'matched-path'}), (t:Team {id: 'matched-path-team'})
                CREATE (n)-[:MEMBER_OF {source: 'matched-path'}]->(t);
                MATCH p = (n:Person)-[r:MEMBER_OF {source: 'matched-path'}]->(t:Team)
                SET r.checked = true
                RETURN p,
                       length(p) AS hops,
                       nodes(p) AS path_nodes,
                       relationships(p) AS path_relationships,
                       n.id AS person,
                       t.id AS team,
                       r.checked AS checked
                ORDER BY person;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("matched relationship path RETURN");

        assert_eq!(
            result.table.columns,
            vec![
                "p".to_string(),
                "hops".to_string(),
                "path_nodes".to_string(),
                "path_relationships".to_string(),
                "person".to_string(),
                "team".to_string(),
                "checked".to_string()
            ]
        );
        assert_eq!(result.table.rows.len(), 2);
        assert_eq!(result.table.rows[0][4], Value::from("matched-path-ada"));
        assert_eq!(result.table.rows[1][4], Value::from("matched-path-bob"));
        for row in &result.table.rows {
            let Value::Json(path) = &row[0] else {
                panic!("RETURN p should project a JSON path");
            };
            assert_eq!(row[1], Value::Int(1));
            assert_eq!(row[5], Value::from("matched-path-team"));
            assert_eq!(row[6], Value::Bool(true));
            assert_eq!(path["nodes"][0]["id"], row[4].to_json());
            assert_eq!(path["nodes"][1]["id"], row[5].to_json());
            assert_eq!(path["relationships"][0]["from"], row[4].to_json());
            assert_eq!(path["relationships"][0]["to"], row[5].to_json());
            assert_eq!(
                path["relationships"][0]["label"],
                serde_json::json!("MEMBER_OF")
            );
            assert_eq!(
                path["relationships"][0]["props"]["checked"],
                serde_json::json!({"type": "bool", "value": true})
            );
            assert_eq!(row[2].to_json(), path["nodes"]);
            assert_eq!(row[3].to_json(), path["relationships"]);
        }

        let removed =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH p = (n:Person)-[r:MEMBER_OF {source: 'matched-path'}]->(t:Team)
                REMOVE r.checked
                RETURN p, r.checked AS checked, n.id AS person
                ORDER BY n.id
                LIMIT 1;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("matched relationship REMOVE path RETURN");

        assert_eq!(removed.table.rows[0][1], Value::Null);
        let Value::Json(path) = &removed.table.rows[0][0] else {
            panic!("RETURN p after REMOVE should project a JSON path");
        };
        assert!(
            path["relationships"][0]["props"].get("checked").is_none(),
            "removed relationship property should be absent in path JSON"
        );
    }

    #[test]
    fn cypher_returning_projects_deleted_relationship_paths_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'deleted-path-ada', status: 'deleted-path'});
                CREATE (:Person {id: 'deleted-path-bob', status: 'deleted-path'});
                CREATE (:Team {id: 'deleted-path-team'});
                MATCH (n:Person {status: 'deleted-path'}), (t:Team {id: 'deleted-path-team'})
                CREATE (n)-[:MEMBER_OF {source: 'deleted-path'}]->(t);
                MATCH p = (n:Person)-[r:MEMBER_OF {source: 'deleted-path'}]->(t:Team)
                DELETE r
                RETURN p,
                       length(p) AS hops,
                       nodes(p) AS path_nodes,
                       relationships(p) AS path_relationships,
                       n.id AS person,
                       t.id AS team,
                       r.source AS source
                ORDER BY person;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("deleted relationship path RETURN");

        assert_eq!(
            result.table.columns,
            vec![
                "p".to_string(),
                "hops".to_string(),
                "path_nodes".to_string(),
                "path_relationships".to_string(),
                "person".to_string(),
                "team".to_string(),
                "source".to_string()
            ]
        );
        assert_eq!(result.table.rows.len(), 2);
        assert_eq!(result.mutation.report.edge_deletes, 2);
        assert_eq!(result.table.rows[0][4], Value::from("deleted-path-ada"));
        assert_eq!(result.table.rows[1][4], Value::from("deleted-path-bob"));
        for row in &result.table.rows {
            let Value::Json(path) = &row[0] else {
                panic!("RETURN p should project the deleted relationship as a JSON path");
            };
            assert_eq!(row[1], Value::Int(1));
            assert_eq!(row[5], Value::from("deleted-path-team"));
            assert_eq!(row[6], Value::from("deleted-path"));
            assert_eq!(path["nodes"][0]["id"], row[4].to_json());
            assert_eq!(path["nodes"][1]["id"], row[5].to_json());
            assert_eq!(path["relationships"][0]["from"], row[4].to_json());
            assert_eq!(path["relationships"][0]["to"], row[5].to_json());
            assert_eq!(
                path["relationships"][0]["label"],
                serde_json::json!("MEMBER_OF")
            );
            assert_eq!(row[2].to_json(), path["nodes"]);
            assert_eq!(row[3].to_json(), path["relationships"]);
        }

        let remaining = futures_executor::block_on(store.get_edges(EdgeQuery {
            from: None,
            to: None,
            label: Some(Label::new("MEMBER_OF")),
        }))
        .expect("remaining relationship scan");
        assert!(
            remaining
                .iter()
                .all(|edge| edge.props.get("source") != Some(&Value::from("deleted-path"))),
            "MATCH DELETE should remove the relationships whose paths were returned"
        );

        let endpoint_delete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'deleted-path-endpoint-a'});
                CREATE (:Team {id: 'deleted-path-endpoint-t'});
                CREATE (:Person {id: 'deleted-path-endpoint-a'})-[:MEMBER_OF {source: 'deleted-path-endpoint'}]->(:Team {id: 'deleted-path-endpoint-t'});
                MATCH p = (n:Person)-[r:MEMBER_OF {source: 'deleted-path-endpoint'}]->(t:Team)
                DELETE r, n
                RETURN p, n.id AS person, t.id AS team, r.source AS source;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("endpoint-deleting relationship path RETURN");
        assert_eq!(endpoint_delete.table.rows.len(), 1);
        assert_eq!(
            endpoint_delete.table.rows[0][1],
            Value::from("deleted-path-endpoint-a")
        );
        assert_eq!(
            endpoint_delete.table.rows[0][2],
            Value::from("deleted-path-endpoint-t")
        );
        assert_eq!(
            endpoint_delete.table.rows[0][3],
            Value::from("deleted-path-endpoint")
        );
        let Value::Json(endpoint_path) = &endpoint_delete.table.rows[0][0] else {
            panic!("RETURN p should project the endpoint-deleting relationship as a JSON path");
        };
        assert_eq!(
            endpoint_path["nodes"][0]["id"],
            serde_json::json!("deleted-path-endpoint-a")
        );
        assert_eq!(
            endpoint_path["nodes"][1]["id"],
            serde_json::json!("deleted-path-endpoint-t")
        );
        assert_eq!(
            endpoint_path["relationships"][0]["from"],
            serde_json::json!("deleted-path-endpoint-a")
        );
        assert!(
            futures_executor::block_on(store.get_node(&NodeId::new("deleted-path-endpoint-a")))
                .expect("deleted endpoint lookup")
                .is_none(),
            "DELETE r, n should remove the endpoint node after snapshotting the path"
        );

        let resolved_endpoint_delete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'deleted-path-resolved-a'});
                CREATE (:Team {id: 'deleted-path-resolved-t'});
                CREATE (:Person {id: 'deleted-path-resolved-a'})-[:MEMBER_OF {source: 'deleted-path-resolved'}]->(:Team {id: 'deleted-path-resolved-t'});
                MATCH p = (n:Person {id: 'deleted-path-resolved-a'})-[r:MEMBER_OF {source: 'deleted-path-resolved'}]->(t:Team {id: 'deleted-path-resolved-t'})
                DELETE r, n
                RETURN p, n.id AS person, t.id AS team, r.source AS source;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("resolved endpoint-deleting relationship path RETURN");
        assert_eq!(resolved_endpoint_delete.table.rows.len(), 1);
        assert_eq!(
            resolved_endpoint_delete.table.rows[0][1],
            Value::from("deleted-path-resolved-a")
        );
        assert_eq!(
            resolved_endpoint_delete.table.rows[0][2],
            Value::from("deleted-path-resolved-t")
        );
        assert_eq!(
            resolved_endpoint_delete.table.rows[0][3],
            Value::from("deleted-path-resolved")
        );
        let Value::Json(resolved_path) = &resolved_endpoint_delete.table.rows[0][0] else {
            panic!("RETURN p should project the resolved endpoint-deleting path");
        };
        assert_eq!(
            resolved_path["nodes"][0]["id"],
            serde_json::json!("deleted-path-resolved-a")
        );
        assert_eq!(
            resolved_path["nodes"][1]["id"],
            serde_json::json!("deleted-path-resolved-t")
        );
        assert!(
            futures_executor::block_on(store.get_node(&NodeId::new("deleted-path-resolved-a")))
                .expect("resolved deleted endpoint lookup")
                .is_none(),
            "resolved endpoint delete should remove the endpoint node after snapshotting the path"
        );

        let node_path_delete_err = execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'deleted-node-path'});
                MATCH p = (n:Person {id: 'deleted-node-path'})
                DELETE n
                RETURN p;
                ",
            CypherMutationOptions::default(),
        );
        let node_path_delete_err = futures_executor::block_on(node_path_delete_err)
            .expect_err("node DELETE path variables should stay rejected");
        assert!(
            matches!(node_path_delete_err, GrustError::CypherSyntax(_)),
            "{node_path_delete_err:?}"
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_maps_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'map-ada', name: 'Ada'})
                RETURN n {
                    .id,
                    .label,
                    display: n.name,
                    lower: toLower(n.name),
                    upper: toUpper(n.name),
                    name_size: size(n.name),
                    nickname: coalesce(n.nickname, n.name, 'unknown'),
                    marker: 'seen',
                    rank: 1,
                    active: true,
                    fallback: $fallback,
                    empty: null,
                    .missing
                } AS person;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "fallback".to_string(),
                        Value::from("provided"),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete map projection");
        assert_eq!(concrete.table.columns, vec!["person"]);
        assert_eq!(
            concrete.table.rows,
            vec![vec![Value::Json(serde_json::json!({
                "id": "map-ada",
                "label": "Person",
                "display": "Ada",
                "lower": "ada",
                "upper": "ADA",
                "name_size": 3,
                "nickname": "Ada",
                "marker": "seen",
                "rank": 1,
                "active": true,
                "fallback": "provided",
                "empty": null,
                "missing": null
            }))]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'map-bob', status: 'active', team: 'eng'});
                CREATE (:Person {id: 'map-cara', status: 'active', team: 'ops'});
                MATCH (n:Person {status: 'active'}) SET n.seen = true
                RETURN n.id AS id, n { .id, kind: 'person', team: n.team, .seen } AS person ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad row map projection");
        assert_eq!(broad.table.columns, vec!["id", "person"]);
        assert_eq!(
            broad.table.rows,
            vec![
                vec![
                    Value::from("map-bob"),
                    Value::Json(serde_json::json!({
                        "id": "map-bob",
                        "kind": "person",
                        "team": "eng",
                        "seen": true
                    }))
                ],
                vec![
                    Value::from("map-cara"),
                    Value::Json(serde_json::json!({
                        "id": "map-cara",
                        "kind": "person",
                        "team": "ops",
                        "seen": true
                    }))
                ]
            ]
        );

        let row_edge =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'map-team'});
                MATCH (n:Person {status: 'active'}), (t:Team {id: 'map-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'map'}]->(t)
                RETURN n.id AS id,
                       n { .id, kind: 'person', team: n.team } AS person,
                       r { .label, source: r.source, static: 'map-entry' } AS membership
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing endpoint and relationship map projections");
        assert_eq!(
            row_edge.table.columns,
            vec![
                "id".to_string(),
                "person".to_string(),
                "membership".to_string()
            ]
        );
        assert_eq!(row_edge.table.rows.len(), 2);
        assert_eq!(
            row_edge.table.rows[0],
            vec![
                Value::from("map-bob"),
                Value::Json(serde_json::json!({"id": "map-bob", "kind": "person", "team": "eng"})),
                Value::Json(
                    serde_json::json!({"label": "MEMBER_OF", "source": "map", "static": "map-entry"})
                )
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'active'}) SET n.map_aggregated = true
                RETURN count(n { team: toLower(n.team), marker: 'seen' }) AS mapped_rows,
                       count(DISTINCT n { team: toLower(n.team), marker: 'seen' }) AS distinct_maps,
                       collect(n { .id, kind: 'person', team: toUpper(n.team), team_size: size(n.team) }) AS people;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("map projection aggregates");
        assert_eq!(
            aggregates.table.columns,
            vec![
                "mapped_rows".to_string(),
                "distinct_maps".to_string(),
                "people".to_string()
            ]
        );
        assert_eq!(aggregates.table.rows[0][0], Value::Int(2));
        assert_eq!(aggregates.table.rows[0][1], Value::Int(2));
        let Value::Json(people) = &aggregates.table.rows[0][2] else {
            panic!("collect(map projection) should return JSON array");
        };
        assert_eq!(people.as_array().expect("people maps").len(), 2);

        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'map-invalid'}) RETURN n { id };",
                CypherMutationOptions::default(),
            ))
            .expect_err("map projection expressions should stay restricted");
        assert!(
            matches!(
                error,
                GrustError::CypherUnsupportedCardinality(_) | GrustError::CypherSyntax(_)
            ),
            "{error:?}"
        );

        let cross_variable =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'map-cross-a'});
                CREATE (b:Person {id: 'map-cross-b'})
                RETURN a { other: b.id };
                ",
                CypherMutationOptions::default(),
            ))
            .expect_err("map projection entries should reject cross-variable properties");
        assert!(
            matches!(cross_variable, GrustError::CypherUnsupportedCardinality(_)),
            "{cross_variable:?}"
        );

        let duplicate_key =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'map-duplicate', team: 'eng'}) RETURN n { .team, team: 'dup' };",
                CypherMutationOptions::default(),
            ))
            .expect_err("map projection entries should reject duplicate output keys");
        assert!(
            matches!(duplicate_key, GrustError::CypherUnsupportedCardinality(_)),
            "{duplicate_key:?}"
        );

        let nested =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'map-nested'}) RETURN n { nested: {value: 1} };",
                CypherMutationOptions::default(),
            ))
            .expect_err("map projection entries should reject nested maps");
        assert!(
            matches!(nested, GrustError::CypherUnsupportedCardinality(_)),
            "{nested:?}"
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_lists_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'list-ada', name: 'Ada'})
                RETURN [
                    n.id,
                    n.label,
                    n.name,
                    toLower(n.name),
                    toUpper(n.name),
                    size(n.name),
                    coalesce(n.nickname, n.name, 'unknown'),
                    'seen',
                    1,
                    true,
                    null,
                    $marker,
                    n.missing
                ] AS person;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "marker".to_string(),
                        Value::from("param"),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete list projection");
        assert_eq!(concrete.table.columns, vec!["person"]);
        assert_eq!(
            concrete.table.rows,
            vec![vec![Value::Json(serde_json::json!([
                "list-ada", "Person", "Ada", "ada", "ADA", 3, "Ada", "seen", 1, true, null,
                "param", null
            ]))]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'list-bob', status: 'active', team: 'eng'});
                CREATE (:Person {id: 'list-cara', status: 'active', team: 'ops'});
                MATCH (n:Person {status: 'active'}) SET n.seen = true
                RETURN n.id AS id, [n.id, 'team', n.team, n.seen] AS person ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad row list projection");
        assert_eq!(broad.table.columns, vec!["id", "person"]);
        assert_eq!(
            broad.table.rows,
            vec![
                vec![
                    Value::from("list-bob"),
                    Value::Json(serde_json::json!(["list-bob", "team", "eng", true]))
                ],
                vec![
                    Value::from("list-cara"),
                    Value::Json(serde_json::json!(["list-cara", "team", "ops", true]))
                ]
            ]
        );

        let row_edge =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'list-team'});
                MATCH (n:Person {status: 'active'}), (t:Team {id: 'list-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'list'}]->(t)
                RETURN n.id AS id, [n.id, 'team', n.team] AS person, [r.label, 'source', r.source] AS membership
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing endpoint and relationship list projections");
        assert_eq!(
            row_edge.table.columns,
            vec![
                "id".to_string(),
                "person".to_string(),
                "membership".to_string()
            ]
        );
        assert_eq!(row_edge.table.rows.len(), 2);
        assert_eq!(
            row_edge.table.rows[0],
            vec![
                Value::from("list-bob"),
                Value::Json(serde_json::json!(["list-bob", "team", "eng"])),
                Value::Json(serde_json::json!(["MEMBER_OF", "source", "list"]))
            ]
        );

        let literal_only =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'list-literal-only'})
                RETURN ['literal', 1, false, null] AS values;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("literal-only list projection");
        assert_eq!(
            literal_only.table.rows,
            vec![vec![Value::Json(serde_json::json!([
                "literal", 1, false, null
            ]))]]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'active'}) SET n.list_aggregated = true
                RETURN count([toLower(n.team), 'seen']) AS listed_rows,
                       count(DISTINCT [toLower(n.team), 'seen']) AS distinct_lists,
                       collect([toUpper(n.id), 'team', size(n.team)]) AS people;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("list projection aggregates");
        assert_eq!(
            aggregates.table.columns,
            vec![
                "listed_rows".to_string(),
                "distinct_lists".to_string(),
                "people".to_string()
            ]
        );
        assert_eq!(aggregates.table.rows[0][0], Value::Int(2));
        assert_eq!(aggregates.table.rows[0][1], Value::Int(2));
        let Value::Json(people) = &aggregates.table.rows[0][2] else {
            panic!("collect(list projection) should return JSON array");
        };
        assert_eq!(people.as_array().expect("people lists").len(), 2);

        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'list-a'});
                CREATE (b:Person {id: 'list-b'})
                RETURN [a.id, b.id];
                ",
                CypherMutationOptions::default(),
            ))
            .expect_err("cross-variable list projections should stay restricted");
        assert!(
            matches!(
                error,
                GrustError::CypherUnsupportedCardinality(_) | GrustError::CypherSyntax(_)
            ),
            "{error:?}"
        );

        let nested =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'list-nested'})
                RETURN [n.id, [1, 2]];
                ",
                CypherMutationOptions::default(),
            ))
            .expect_err("nested list projections should stay restricted");
        assert!(
            matches!(nested, GrustError::CypherUnsupportedCardinality(_)),
            "{nested:?}"
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_literals_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'literal-ada', team: 'eng'})
                RETURN 'created' AS status, 1 AS one, true AS ok, null AS empty, n.id AS id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("concrete literal projections");
        assert_eq!(
            concrete.table,
            CypherResultTable {
                columns: vec![
                    "status".to_string(),
                    "one".to_string(),
                    "ok".to_string(),
                    "empty".to_string(),
                    "id".to_string(),
                ],
                rows: vec![vec![
                    Value::from("created"),
                    Value::Int(1),
                    Value::Bool(true),
                    Value::Null,
                    Value::from("literal-ada"),
                ]],
            }
        );

        let parameterized =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'literal-param', team: 'ops'})
                RETURN $status AS status, $score AS score, n.team AS team;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        ("status".to_string(), Value::from("accepted")),
                        ("score".to_string(), Value::Int(7)),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("parameterized literal projections");
        assert_eq!(
            parameterized.table.rows,
            vec![vec![
                Value::from("accepted"),
                Value::Int(7),
                Value::from("ops")
            ]]
        );

        let ranges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:RangeProbe {id: 'literal-range'})
                RETURN range(1, 4) AS ascending,
                       range($start, $end, $step) AS descending,
                       range(4, 1) AS empty_range;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        ("start".to_string(), Value::Int(5)),
                        ("end".to_string(), Value::Int(1)),
                        ("step".to_string(), Value::Int(-2)),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("range literal projections");
        assert_eq!(
            ranges.table.rows,
            vec![vec![
                Value::IntArray(vec![1, 2, 3, 4]),
                Value::IntArray(vec![5, 3, 1]),
                Value::IntArray(vec![]),
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person) SET n.literal_seen = true
                RETURN n.team AS team, 'seen' AS status, count(1) AS rows
                ORDER BY team;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("grouped literal projection");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("eng"), Value::from("seen"), Value::Int(1)],
                vec![Value::from("ops"), Value::from("seen"), Value::Int(1)],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person) SET n.literal_counted = true
                RETURN count(1) AS counted,
                       count(DISTINCT 'x') AS distinct_literal,
                       count(null) AS non_null,
                       sum(1) AS summed,
                       avg(2) AS averaged,
                       collect('x') AS collected,
                       count(range(1, 2)) AS range_count,
                       collect(range(1, 2)) AS ranges;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("literal aggregate projections");
        assert_eq!(aggregates.table.rows[0][0], Value::Int(2));
        assert_eq!(aggregates.table.rows[0][1], Value::Int(1));
        assert_eq!(aggregates.table.rows[0][2], Value::Int(0));
        assert_eq!(aggregates.table.rows[0][3], Value::Int(2));
        assert_eq!(aggregates.table.rows[0][4], Value::Float(2.0));
        assert_eq!(
            aggregates.table.rows[0][5],
            Value::Json(serde_json::json!(["x", "x"]))
        );
        assert_eq!(aggregates.table.rows[0][6], Value::Int(2));
        assert_eq!(
            aggregates.table.rows[0][7],
            Value::Json(serde_json::json!([[1, 2], [1, 2]]))
        );

        let zero_step =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:RangeProbe {id: 'literal-range-zero'}) RETURN range(1, 3, 0);",
                CypherMutationOptions::default(),
            ))
            .expect_err("range zero step should stay rejected");
        assert!(
            matches!(zero_step, GrustError::CypherUnsupportedCardinality(_)),
            "{zero_step:?}"
        );

        let non_integer =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:RangeProbe {id: 'literal-range-float'}) RETURN range(1.5, 3);",
                CypherMutationOptions::default(),
            ))
            .expect_err("range float arguments should stay rejected");
        assert!(
            matches!(non_integer, GrustError::CypherUnsupportedCardinality(_)),
            "{non_integer:?}"
        );

        let numeric_range_aggregate =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "MATCH (n:Person) SET n.literal_range_sum = true RETURN sum(range(1, 2));",
                CypherMutationOptions::default(),
            ))
            .expect_err("numeric aggregates over range arrays should stay rejected");
        assert!(
            matches!(
                numeric_range_aggregate,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{numeric_range_aggregate:?}"
        );

        let missing_parameter =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'literal-missing'}) RETURN $missing;",
                CypherMutationOptions::default(),
            ))
            .expect_err("missing RETURN parameter should fail");
        assert!(
            matches!(missing_parameter, GrustError::CypherUnresolvedIdentity(_)),
            "{missing_parameter:?}"
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_coalesce_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'coalesce-ada', name: 'Ada'})
                RETURN coalesce(n.nickname, n.name, 'unknown') AS display,
                       coalesce(null, $fallback) AS fallback;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "fallback".to_string(),
                        Value::from("provided"),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete coalesce projection");
        assert_eq!(
            concrete.table,
            CypherResultTable {
                columns: vec!["display".to_string(), "fallback".to_string()],
                rows: vec![vec![Value::from("Ada"), Value::from("provided")]],
            }
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'coalesce-bob', status: 'coalesce', name: 'Bob'});
                CREATE (:Person {id: 'coalesce-cara', status: 'coalesce', nickname: 'C'});
                MATCH (n:Person {status: 'coalesce'}) SET n.seen = true
                RETURN n.id AS id, coalesce(n.nickname, n.name, 'unknown') AS display
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad coalesce projection");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("coalesce-bob"), Value::from("Bob")],
                vec![Value::from("coalesce-cara"), Value::from("C")],
            ]
        );

        let nested =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'coalesce-nested-ada', name: 'Ada', status: 'coalesce-nested'});
                CREATE (:Person {id: 'coalesce-nested-bob', nickname: 'B', status: 'coalesce-nested'});
                MATCH (n:Person {status: 'coalesce-nested'}) SET n.seen = true
                RETURN n.id AS id,
                       coalesce(toLower(n.nickname), toUpper(n.name), 'unknown') AS display,
                       coalesce(size(n.nickname), size(n.name), 0) AS name_size
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested restricted coalesce projections should execute");
        assert_eq!(
            nested.table.rows,
            vec![
                vec![
                    Value::from("coalesce-nested-ada"),
                    Value::from("ADA"),
                    Value::Int(3)
                ],
                vec![
                    Value::from("coalesce-nested-bob"),
                    Value::from("b"),
                    Value::Int(1)
                ],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'coalesce'}) SET n.coalesced = true
                RETURN count(coalesce(n.nickname, n.name)) AS named,
                       count(DISTINCT coalesce(n.nickname, n.name)) AS distinct_names,
                       min(coalesce(n.nickname, n.name)) AS first_name,
                       max(coalesce(n.nickname, n.name)) AS last_name,
                       collect(coalesce(n.nickname, n.name)) AS names;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("coalesce aggregate projections");
        assert_eq!(aggregates.table.rows[0][0], Value::Int(2));
        assert_eq!(aggregates.table.rows[0][1], Value::Int(2));
        assert_eq!(aggregates.table.rows[0][2], Value::from("Bob"));
        assert_eq!(aggregates.table.rows[0][3], Value::from("C"));
        assert_eq!(
            aggregates.table.rows[0][4],
            Value::Json(serde_json::json!(["Bob", "C"]))
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'coalesce-nested'}) SET n.nested_coalesced = true
                RETURN count(coalesce(toLower(n.nickname), toUpper(n.name))) AS named,
                       collect(coalesce(toLower(n.nickname), toUpper(n.name))) AS names;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested coalesce aggregate projections should execute");
        assert_eq!(nested_aggregates.table.rows[0][0], Value::Int(2));
        assert_eq!(
            nested_aggregates.table.rows[0][1],
            Value::Json(serde_json::json!(["ADA", "b"]))
        );

        let cross_variable =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'coalesce-a'});
                CREATE (b:Person {id: 'coalesce-b'})
                RETURN coalesce(a.name, b.name, 'unknown');
                ",
                CypherMutationOptions::default(),
            ))
            .expect_err("cross-variable coalesce should stay restricted");
        assert!(
            matches!(cross_variable, GrustError::CypherUnsupportedCardinality(_)),
            "{cross_variable:?}"
        );

        let nested_list =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'coalesce-list', name: 'Ada'}) RETURN coalesce([n.name], 'unknown');",
                CypherMutationOptions::default(),
            ))
            .expect_err("coalesce arguments should reject nested list composites");
        assert!(
            matches!(nested_list, GrustError::CypherUnsupportedCardinality(_)),
            "{nested_list:?}"
        );

        let nested_map =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'coalesce-map', name: 'Ada'}) RETURN coalesce(n { name: n.name }, 'unknown');",
                CypherMutationOptions::default(),
            ))
            .expect_err("coalesce arguments should reject nested map composites");
        assert!(
            matches!(nested_map, GrustError::CypherUnsupportedCardinality(_)),
            "{nested_map:?}"
        );

        let path_function_on_node =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'coalesce-nested'}) RETURN coalesce(length(n), 'unknown');",
                CypherMutationOptions::default(),
            ))
            .expect_err("nested path functions should still require path variables");
        assert!(
            matches!(
                path_function_on_node,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{path_function_on_node:?}"
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_element_functions_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'function-ada'});
                CREATE (b:Person {id: 'function-bob'});
                MATCH (a:Person {id: 'function-ada'}), (b:Person {id: 'function-bob'})
                CREATE (a)-[e:KNOWS {id: 'function-knows'}]->(b)
                RETURN labels(a) AS node_labels, type(e) AS relationship_type;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("concrete element function projections");
        assert_eq!(
            concrete.table,
            CypherResultTable {
                columns: vec!["node_labels".to_string(), "relationship_type".to_string()],
                rows: vec![vec![
                    Value::Json(serde_json::json!(["Person"])),
                    Value::from("KNOWS"),
                ]],
            }
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'function-cara', status: 'element-functions'});
                CREATE (:Person {id: 'function-dan', status: 'element-functions'});
                MATCH (n:Person {status: 'element-functions'}) SET n.seen = true
                RETURN n.id AS id, labels(n) AS labels
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad node labels projection");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![
                    Value::from("function-cara"),
                    Value::Json(serde_json::json!(["Person"]))
                ],
                vec![
                    Value::from("function-dan"),
                    Value::Json(serde_json::json!(["Person"]))
                ],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'function-team'});
                MATCH (n:Person {status: 'element-functions'}), (t:Team {id: 'function-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'function'}]->(t)
                RETURN n.id AS id, type(r) AS relationship_type
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship type projection");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("function-cara"), Value::from("MEMBER_OF")],
                vec![Value::from("function-dan"), Value::from("MEMBER_OF")],
            ]
        );

        let node_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'element-functions'}) SET n.label_counted = true
                RETURN count(labels(n)) AS labelled_nodes,
                       collect(labels(n)) AS node_labels;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("node labels aggregate projections");
        assert_eq!(node_aggregates.table.rows[0][0], Value::Int(2));

        let relationship_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (:Person {status: 'element-functions'})-[r:MEMBER_OF]->(:Team {id: 'function-team'})
                SET r.checked = true
                RETURN
                       count(DISTINCT type(r)) AS relationship_types,
                       collect(type(r)) AS relationships;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("relationship type aggregate projections");
        assert_eq!(relationship_aggregates.table.rows[0][0], Value::Int(1));
        assert_eq!(
            relationship_aggregates.table.rows[0][1],
            Value::Json(serde_json::json!(["MEMBER_OF", "MEMBER_OF"]))
        );

        let labels_on_edge =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'element-functions'}), (t:Team {id: 'function-team'})
                CREATE (n)-[r:REJECTED_FUNCTION_TARGET]->(t)
                RETURN labels(r);
                ",
                CypherMutationOptions::default(),
            ))
            .expect_err("labels on relationship variables should stay rejected");
        assert!(
            matches!(labels_on_edge, GrustError::CypherUnsupportedCardinality(_)),
            "{labels_on_edge:?}"
        );

        let type_on_node =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'element-functions'}) SET n.rejected = true
                RETURN type(n);
                ",
                CypherMutationOptions::default(),
            ))
            .expect_err("type on node variables should stay rejected");
        assert!(
            matches!(type_on_node, GrustError::CypherUnsupportedCardinality(_)),
            "{type_on_node:?}"
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_properties_and_keys_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'props-ada', name: 'Ada', team: 'eng'});
                CREATE (b:Person {id: 'props-bob'});
                MATCH (a:Person {id: 'props-ada'}), (b:Person {id: 'props-bob'})
                CREATE (a)-[e:KNOWS {id: 'props-knows', since: 2026}]->(b)
                RETURN properties(a) AS node_props,
                       keys(a) AS node_keys,
                       properties(e) AS edge_props,
                       keys(e) AS edge_keys;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("concrete properties/keys projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::Json(serde_json::json!({
                    "id": "props-ada",
                    "name": "Ada",
                    "team": "eng"
                })),
                Value::Json(serde_json::json!(["id", "name", "team"])),
                Value::Json(serde_json::json!({
                    "id": "props-knows",
                    "since": 2026
                })),
                Value::Json(serde_json::json!(["id", "since"])),
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'props-cara', status: 'props', team: 'ops'});
                CREATE (:Person {id: 'props-dan', status: 'props', team: 'eng'});
                MATCH (n:Person {status: 'props'}) SET n.seen = true
                RETURN n.id AS id, properties(n) AS props, keys(n) AS keys
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad properties/keys projections");
        assert_eq!(broad.table.rows.len(), 2);
        assert_eq!(broad.table.rows[0][0], Value::from("props-cara"));
        assert_eq!(
            broad.table.rows[0][2],
            Value::Json(serde_json::json!(["id", "seen", "status", "team"]))
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'props-team'});
                MATCH (n:Person {status: 'props'}), (t:Team {id: 'props-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'props'}]->(t)
                RETURN n.id AS id, properties(r) AS props, keys(r) AS keys
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship properties/keys projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![
                    Value::from("props-cara"),
                    Value::Json(serde_json::json!({"source": "props"})),
                    Value::Json(serde_json::json!(["source"]))
                ],
                vec![
                    Value::from("props-dan"),
                    Value::Json(serde_json::json!({"source": "props"})),
                    Value::Json(serde_json::json!(["source"]))
                ],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'props'}) SET n.props_counted = true
                RETURN count(properties(n)) AS prop_rows,
                       count(DISTINCT keys(n)) AS distinct_key_sets,
                       collect(keys(n)) AS key_sets;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("properties/keys aggregate projections");
        assert_eq!(aggregates.table.rows[0][0], Value::Int(2));
        assert_eq!(aggregates.table.rows[0][1], Value::Int(1));
        let Value::Json(key_sets) = &aggregates.table.rows[0][2] else {
            panic!("collect(keys(n)) should return JSON arrays");
        };
        assert_eq!(key_sets.as_array().expect("key sets").len(), 2);

        let properties_on_path =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'props'}), (t:Team {id: 'props-team'})
                CREATE p = (n)-[r:REJECTED_PROPS_PATH]->(t)
                RETURN properties(p);
                ",
                CypherMutationOptions::default(),
            ))
            .expect_err("properties on path variables should stay rejected");
        assert!(
            matches!(
                properties_on_path,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{properties_on_path:?}"
        );
    }

    #[test]
    fn cypher_returning_projects_relationship_endpoints_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'endpoint-ada', name: 'Ada'});
                CREATE (b:Person {id: 'endpoint-bob', name: 'Bob'});
                MATCH (a:Person {id: 'endpoint-ada'}), (b:Person {id: 'endpoint-bob'})
                CREATE (a)-[e:KNOWS {id: 'endpoint-knows'}]->(b)
                RETURN startNode(e) AS source, endNode(e) AS target;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("concrete relationship endpoint projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::from(serde_json::json!({
                    "id": "endpoint-ada",
                    "label": "Person",
                    "props": {
                        "id": {"type": "string", "value": "endpoint-ada"},
                        "name": {"type": "string", "value": "Ada"}
                    }
                })),
                Value::from(serde_json::json!({
                    "id": "endpoint-bob",
                    "label": "Person",
                    "props": {
                        "id": {"type": "string", "value": "endpoint-bob"},
                        "name": {"type": "string", "value": "Bob"}
                    }
                })),
            ]]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'endpoint-team'});
                MATCH (n:Person), (t:Team {id: 'endpoint-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'endpoint'}]->(t)
                RETURN n.id AS id, startNode(r) AS source, endNode(r) AS target
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship endpoint projections");
        assert_eq!(row_edges.table.rows.len(), 2);
        assert_eq!(row_edges.table.rows[0][0], Value::from("endpoint-ada"));
        let Value::Json(source) = &row_edges.table.rows[0][1] else {
            panic!("startNode(r) should return a JSON node");
        };
        assert_eq!(source["id"], serde_json::json!("endpoint-ada"));
        let Value::Json(target) = &row_edges.table.rows[0][2] else {
            panic!("endNode(r) should return a JSON node");
        };
        assert_eq!(target["id"], serde_json::json!("endpoint-team"));

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (:Person)-[r:MEMBER_OF {source: 'endpoint'}]->(:Team {id: 'endpoint-team'})
                SET r.endpoint_checked = true
                RETURN count(startNode(r)) AS sources,
                       count(DISTINCT endNode(r)) AS target_count,
                       collect(endNode(r)) AS targets;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("relationship endpoint aggregate projections");
        assert_eq!(aggregates.table.rows[0][0], Value::Int(2));
        assert_eq!(aggregates.table.rows[0][1], Value::Int(1));
        let Value::Json(targets) = &aggregates.table.rows[0][2] else {
            panic!("collect(endNode(r)) should return JSON nodes");
        };
        assert_eq!(targets.as_array().expect("target nodes").len(), 2);

        let start_node_on_node =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person) SET n.endpoint_rejected = true
                RETURN startNode(n);
                ",
                CypherMutationOptions::default(),
            ))
            .expect_err("startNode on node variables should stay rejected");
        assert!(
            matches!(
                start_node_on_node,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{start_node_on_node:?}"
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_identity_functions_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'identity-ada'});
                CREATE (b:Person {id: 'identity-bob'});
                MATCH (a:Person {id: 'identity-ada'}), (b:Person {id: 'identity-bob'})
                CREATE (a)-[e:KNOWS {id: 'identity-knows'}]->(b)
                RETURN id(a) AS node_id,
                       elementId(a) AS node_element_id,
                       id(e) AS edge_id,
                       elementId(e) AS edge_element_id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("concrete identity function projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::from("identity-ada"),
                Value::from("identity-ada"),
                Value::from("identity-knows"),
                Value::from("identity-knows"),
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'identity-cara', status: 'identity'});
                CREATE (:Person {id: 'identity-dan', status: 'identity'});
                MATCH (n:Person {status: 'identity'}) SET n.seen = true
                RETURN n.id AS raw, id(n) AS id, elementId(n) AS element_id
                ORDER BY raw;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad node identity function projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![
                    Value::from("identity-cara"),
                    Value::from("identity-cara"),
                    Value::from("identity-cara")
                ],
                vec![
                    Value::from("identity-dan"),
                    Value::from("identity-dan"),
                    Value::from("identity-dan")
                ],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'identity-team'});
                MATCH (n:Person {status: 'identity'}), (t:Team {id: 'identity-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'identity'}]->(t)
                RETURN n.id AS id, id(r) AS relationship_id
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship identity function projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("identity-cara"), Value::Null],
                vec![Value::from("identity-dan"), Value::Null],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'identity'}) SET n.identity_counted = true
                RETURN count(id(n)) AS ids,
                       count(DISTINCT elementId(n)) AS distinct_ids,
                       collect(id(n)) AS collected_ids;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("identity function aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!(["identity-cara", "identity-dan"])),
            ]]
        );

        let relationship_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (:Person {status: 'identity'})-[r:MEMBER_OF {source: 'identity'}]->(:Team {id: 'identity-team'})
                SET r.identity_checked = true
                RETURN count(id(r)) AS relationship_ids,
                       collect(elementId(r)) AS collected_relationship_ids;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("relationship identity function aggregate projections");
        assert_eq!(
            relationship_aggregates.table.rows,
            vec![vec![Value::Int(0), Value::Json(serde_json::json!([]))]]
        );

        let identity_on_path =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'identity'}), (t:Team {id: 'identity-team'})
                CREATE p = (n)-[r:REJECTED_ID_PATH]->(t)
                RETURN id(p);
                ",
                CypherMutationOptions::default(),
            ))
            .expect_err("identity functions on path variables should stay rejected");
        assert!(
            matches!(
                identity_on_path,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{identity_on_path:?}"
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_exists_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'exists-ada', name: 'Ada'});
                CREATE (b:Person {id: 'exists-bob'});
                MATCH (a:Person {id: 'exists-ada'}), (b:Person {id: 'exists-bob'})
                CREATE (a)-[e:KNOWS {id: 'exists-knows', since: 2026}]->(b)
                RETURN exists(a.name) AS has_name,
                       exists(a.nickname) AS has_nickname,
                       exists(e.since) AS has_since,
                       exists(e.weight) AS has_weight;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("concrete exists projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::Bool(true),
                Value::Bool(false),
                Value::Bool(true),
                Value::Bool(false),
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'exists-cara', status: 'exists', nickname: 'C'});
                CREATE (:Person {id: 'exists-dan', status: 'exists'});
                MATCH (n:Person {status: 'exists'}) SET n.seen = true
                RETURN n.id AS id, exists(n.nickname) AS has_nickname
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad exists projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("exists-cara"), Value::Bool(true)],
                vec![Value::from("exists-dan"), Value::Bool(false)],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'exists-team'});
                MATCH (n:Person {status: 'exists'}), (t:Team {id: 'exists-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'exists'}]->(t)
                RETURN n.id AS id, exists(r.source) AS has_source, exists(r.id) AS has_id
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship exists projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![
                    Value::from("exists-cara"),
                    Value::Bool(true),
                    Value::Bool(false)
                ],
                vec![
                    Value::from("exists-dan"),
                    Value::Bool(true),
                    Value::Bool(false)
                ],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'exists'}) SET n.exists_counted = true
                RETURN count(exists(n.nickname)) AS rows,
                       count(DISTINCT exists(n.nickname)) AS distinct_states,
                       collect(exists(n.nickname)) AS states;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("exists aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!([true, false])),
            ]]
        );

        let non_property =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'exists-rejected'}) RETURN exists(n);",
                CypherMutationOptions::default(),
            ))
            .expect_err("exists over whole elements should stay rejected");
        assert!(
            matches!(non_property, GrustError::CypherUnsupportedCardinality(_)),
            "{non_property:?}"
        );

        let traversal_exists =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'exists'}), (t:Team {id: 'exists-team'})
                CREATE (n)-[r:REJECTED_EXISTS_PATH]->(t)
                RETURN exists((n)-[:REJECTED_EXISTS_PATH]->(t));
                ",
                CypherMutationOptions::default(),
            ))
            .expect_err("exists traversal predicates should stay rejected");
        assert!(
            matches!(
                traversal_exists,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{traversal_exists:?}"
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_size_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'size-ada', name: 'Ada', tags: $tags})
                RETURN size(n.name) AS name_size,
                       size(n.tags) AS tag_count,
                       size(n.nickname) AS missing_size;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "tags".to_string(),
                        Value::StringArray(vec!["engineer".to_string(), "speaker".to_string()]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete size projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![Value::Int(3), Value::Int(2), Value::Null]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'size-bob', status: 'size', nickname: 'B'});
                CREATE (:Person {id: 'size-cara', status: 'size', nickname: 'Cara'});
                MATCH (n:Person {status: 'size'}) SET n.seen = true
                RETURN n.id AS id, size(n.nickname) AS nickname_size
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad size projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("size-bob"), Value::Int(1)],
                vec![Value::from("size-cara"), Value::Int(4)],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'size-team'});
                MATCH (n:Person {status: 'size'}), (t:Team {id: 'size-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'size'}]->(t)
                RETURN n.id AS id, size(r.source) AS source_size
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship size projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("size-bob"), Value::Int(4)],
                vec![Value::from("size-cara"), Value::Int(4)],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'size'}) SET n.size_counted = true
                RETURN count(size(n.nickname)) AS rows,
                       sum(size(n.nickname)) AS total_size,
                       collect(size(n.nickname)) AS sizes;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("size aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(5),
                Value::Json(serde_json::json!([1, 4])),
            ]]
        );

        let numeric_size =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'size-number', score: 3}) RETURN size(n.score);",
                CypherMutationOptions::default(),
            ))
            .expect_err("size over numeric values should stay rejected");
        assert!(
            matches!(numeric_size, GrustError::CypherUnsupportedCardinality(_)),
            "{numeric_size:?}"
        );

        let traversal_size =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'size'}), (t:Team {id: 'size-team'})
                CREATE (n)-[r:REJECTED_SIZE_PATH]->(t)
                RETURN size((n)-[:REJECTED_SIZE_PATH]->(t));
                ",
                CypherMutationOptions::default(),
            ))
            .expect_err("size traversal expressions should stay rejected");
        assert!(
            matches!(traversal_size, GrustError::CypherUnsupportedCardinality(_)),
            "{traversal_size:?}"
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_list_slices_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'slice-ada', tags: $tags, scores: $scores});
                CREATE (b:Person {id: 'slice-bob'});
                MATCH (a:Person {id: 'slice-ada'}), (b:Person {id: 'slice-bob'})
                CREATE (a)-[e:KNOWS {id: 'slice-knows', weights: $weights}]->(b)
                RETURN a.tags[0..2] AS first_tags,
                       a.scores[$start..$end] AS middle_scores,
                       e.weights[1..] AS trailing_weights,
                       a.tags[..1] AS leading_tag,
                       a.tags[9..12] AS empty_tags,
                       a.nickname[0..1] AS missing_name;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        (
                            "tags".to_string(),
                            Value::StringArray(vec![
                                "engineer".to_string(),
                                "speaker".to_string(),
                                "writer".to_string(),
                            ]),
                        ),
                        ("scores".to_string(), Value::IntArray(vec![5, 7, 11, 13])),
                        (
                            "weights".to_string(),
                            Value::FloatArray(vec![2.5, 4.5, 6.5]),
                        ),
                        ("start".to_string(), Value::Int(1)),
                        ("end".to_string(), Value::Int(3)),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete list slice projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::StringArray(vec!["engineer".to_string(), "speaker".to_string()]),
                Value::IntArray(vec![7, 11]),
                Value::FloatArray(vec![4.5, 6.5]),
                Value::StringArray(vec!["engineer".to_string()]),
                Value::StringArray(vec![]),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'slice-cara', status: 'slice', scores: $scores_a});
                CREATE (:Person {id: 'slice-dan', status: 'slice', scores: $scores_b});
                MATCH (n:Person {status: 'slice'}) SET n.sliced = true
                RETURN n.id AS id, n.scores[1..3] AS scores
                ORDER BY id;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        ("scores_a".to_string(), Value::IntArray(vec![3, 5, 8])),
                        ("scores_b".to_string(), Value::IntArray(vec![7, 9, 13])),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("broad list slice projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("slice-cara"), Value::IntArray(vec![5, 8])],
                vec![Value::from("slice-dan"), Value::IntArray(vec![9, 13])],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'slice-team'});
                MATCH (n:Person {status: 'slice'}), (t:Team {id: 'slice-team'})
                CREATE (n)-[r:MEMBER_OF {rankings: $rankings}]->(t)
                RETURN n.id AS id, r.rankings[..2] AS ranks
                ORDER BY id;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "rankings".to_string(),
                        Value::IntArray(vec![1, 2, 3]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("row-producing relationship list slice projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("slice-cara"), Value::IntArray(vec![1, 2])],
                vec![Value::from("slice-dan"), Value::IntArray(vec![1, 2])],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'slice'}) SET n.slice_counted = true
                RETURN count(n.scores[1..3]) AS rows,
                       collect(n.scores[1..3]) AS score_slices;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("list slice aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Json(serde_json::json!([[5, 8], [9, 13]])),
            ]]
        );

        let numeric_slice_aggregate =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "MATCH (n:Person {status: 'slice'}) SET n.slice_summed = true RETURN sum(n.scores[1..3]);",
                CypherMutationOptions::default(),
            ))
            .expect_err("numeric aggregates over list slices should stay rejected");
        assert!(
            matches!(
                numeric_slice_aggregate,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{numeric_slice_aggregate:?}"
        );

        let non_array =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'slice-string', name: 'Ada'}) RETURN n.name[0..1];",
                CypherMutationOptions::default(),
            ))
            .expect_err("list slices over strings should stay rejected");
        assert!(
            matches!(non_array, GrustError::CypherUnsupportedCardinality(_)),
            "{non_array:?}"
        );

        let negative_bound =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'slice-negative', scores: $scores}) RETURN n.scores[-1..2];",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "scores".to_string(),
                        Value::IntArray(vec![1, 2]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect_err("negative list slice bounds should stay rejected");
        assert!(
            matches!(negative_bound, GrustError::CypherUnsupportedCardinality(_)),
            "{negative_bound:?}"
        );

        let nested_bound =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'slice-nested', scores: $scores}) RETURN n.scores[0..head(n.scores)];",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "scores".to_string(),
                        Value::IntArray(vec![1, 2]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("nested list slice bounds should execute");
        assert_eq!(
            nested_bound.table.rows,
            vec![vec![Value::IntArray(vec![1])]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'slice'}) SET n.nested_slice_counted = true
                RETURN count(n.scores[0..head(n.scores)]) AS rows,
                       collect(n.scores[0..head(n.scores)]) AS slices;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested list slice aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Json(serde_json::json!([[3, 5, 8], [7, 9, 13]])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_list_membership_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'membership-ada', tags: $tags, scores: $scores});
                CREATE (b:Person {id: 'membership-bob'});
                MATCH (a:Person {id: 'membership-ada'}), (b:Person {id: 'membership-bob'})
                CREATE (a)-[e:KNOWS {id: 'membership-knows', weights: $weights}]->(b)
                RETURN 'speaker' IN a.tags AS has_speaker,
                       $needle_score IN a.scores AS has_score,
                       4.5 IN e.weights AS has_weight,
                       'missing' IN a.tags AS missing_tag,
                       null IN a.tags AS null_needle,
                       'speaker' IN a.nickname AS missing_name;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        (
                            "tags".to_string(),
                            Value::StringArray(vec!["engineer".to_string(), "speaker".to_string()]),
                        ),
                        ("scores".to_string(), Value::IntArray(vec![7, 11])),
                        ("weights".to_string(), Value::FloatArray(vec![2.5, 4.5])),
                        ("needle_score".to_string(), Value::Int(11)),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete list membership projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(false),
                Value::Null,
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'membership-cara', status: 'membership', tags: $tags_a});
                CREATE (:Person {id: 'membership-dan', status: 'membership', tags: $tags_b});
                MATCH (n:Person {status: 'membership'}) SET n.membership_checked = true
                RETURN n.id AS id, 'speaker' IN n.tags AS has_speaker
                ORDER BY id;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        (
                            "tags_a".to_string(),
                            Value::StringArray(vec!["speaker".to_string(), "mentor".to_string()]),
                        ),
                        (
                            "tags_b".to_string(),
                            Value::StringArray(vec!["writer".to_string(), "mentor".to_string()]),
                        ),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("broad list membership projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("membership-cara"), Value::Bool(true)],
                vec![Value::from("membership-dan"), Value::Bool(false)],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'membership-team'});
                MATCH (n:Person {status: 'membership'}), (t:Team {id: 'membership-team'})
                CREATE (n)-[r:MEMBER_OF {rankings: $rankings}]->(t)
                RETURN n.id AS id, 2 IN r.rankings AS has_rank
                ORDER BY id;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "rankings".to_string(),
                        Value::IntArray(vec![1, 2, 3]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("row-producing relationship list membership projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("membership-cara"), Value::Bool(true)],
                vec![Value::from("membership-dan"), Value::Bool(true)],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'membership'}) SET n.membership_counted = true
                RETURN count('speaker' IN n.tags) AS rows,
                       count(DISTINCT 'speaker' IN n.tags) AS states,
                       collect('speaker' IN n.tags) AS memberships;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("list membership aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!([true, false])),
            ]]
        );

        let numeric_membership_aggregate =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "MATCH (n:Person {status: 'membership'}) SET n.membership_summed = true RETURN sum('speaker' IN n.tags);",
                CypherMutationOptions::default(),
            ))
            .expect_err("numeric aggregates over membership booleans should stay rejected");
        assert!(
            matches!(
                numeric_membership_aggregate,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{numeric_membership_aggregate:?}"
        );

        let type_mismatch =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'membership-type', scores: $scores}) RETURN '11' IN n.scores;",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "scores".to_string(),
                        Value::IntArray(vec![11]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("type mismatched list membership should evaluate false");
        assert_eq!(type_mismatch.table.rows, vec![vec![Value::Bool(false)]]);

        let non_array =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'membership-string', name: 'Ada'}) RETURN 'A' IN n.name;",
                CypherMutationOptions::default(),
            ))
            .expect_err("list membership over strings should stay rejected");
        assert!(
            matches!(non_array, GrustError::CypherUnsupportedCardinality(_)),
            "{non_array:?}"
        );

        let computed_needle =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'membership-computed', tags: $tags}) RETURN toLower('SPEAKER') IN n.tags;",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "tags".to_string(),
                        Value::StringArray(vec!["speaker".to_string()]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect_err("computed list membership needles should stay rejected");
        assert!(
            matches!(computed_needle, GrustError::CypherUnsupportedCardinality(_)),
            "{computed_needle:?}"
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_list_predicates_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'predicate-list-ada', tags: $tags, scores: $scores, marker: 'SPEAKER'});
                CREATE (b:Person {id: 'predicate-list-bob'});
                MATCH (a:Person {id: 'predicate-list-ada'}), (b:Person {id: 'predicate-list-bob'})
                CREATE (a)-[e:KNOWS {id: 'predicate-list-knows', weights: $weights}]->(b)
                RETURN any(t IN a.tags WHERE t = 'speaker') AS any_speaker,
                       any(t IN a.tags WHERE t = toLower(a.marker)) AS nested_any_speaker,
                       all(t IN a.tags WHERE t = 'speaker') AS all_speaker,
                       none(t IN a.tags WHERE t = 'missing') AS none_missing,
                       single(s IN a.scores WHERE s = $needle_score) AS single_score,
                       any(w IN e.weights WHERE w = 4.5) AS any_weight,
                       any(t IN a.nickname WHERE t = 'speaker') AS missing_name,
                       any(t IN a.tags WHERE t = null) AS null_needle;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        (
                            "tags".to_string(),
                            Value::StringArray(vec![
                                "engineer".to_string(),
                                "speaker".to_string(),
                                "speaker".to_string(),
                            ]),
                        ),
                        ("scores".to_string(), Value::IntArray(vec![7, 11])),
                        ("weights".to_string(), Value::FloatArray(vec![2.5, 4.5])),
                        ("needle_score".to_string(), Value::Int(11)),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete list predicate projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(false),
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(true),
                Value::Null,
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'predicate-list-cara', status: 'list-predicate', tags: $tags_a, marker: 'SPEAKER'});
                CREATE (:Person {id: 'predicate-list-dan', status: 'list-predicate', tags: $tags_b, marker: 'SPEAKER'});
                MATCH (n:Person {status: 'list-predicate'}) SET n.predicate_checked = true
                RETURN n.id AS id,
                       any(t IN n.tags WHERE t = 'speaker') AS any_speaker,
                       any(t IN n.tags WHERE t = toLower(n.marker)) AS nested_any_speaker
                ORDER BY id;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        (
                            "tags_a".to_string(),
                            Value::StringArray(vec!["speaker".to_string(), "mentor".to_string()]),
                        ),
                        (
                            "tags_b".to_string(),
                            Value::StringArray(vec!["writer".to_string(), "mentor".to_string()]),
                        ),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("broad list predicate projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![
                    Value::from("predicate-list-cara"),
                    Value::Bool(true),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("predicate-list-dan"),
                    Value::Bool(false),
                    Value::Bool(false)
                ],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'predicate-list-team'});
                MATCH (n:Person {status: 'list-predicate'}), (t:Team {id: 'predicate-list-team'})
                CREATE (n)-[r:MEMBER_OF {rankings: $rankings}]->(t)
                RETURN n.id AS id, single(rank IN r.rankings WHERE rank = 2) AS single_rank
                ORDER BY id;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "rankings".to_string(),
                        Value::IntArray(vec![1, 2, 3]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("row-producing relationship list predicate projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("predicate-list-cara"), Value::Bool(true)],
                vec![Value::from("predicate-list-dan"), Value::Bool(true)],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'list-predicate'}) SET n.predicate_counted = true
                RETURN count(any(t IN n.tags WHERE t = 'speaker')) AS rows,
                       count(DISTINCT any(t IN n.tags WHERE t = 'speaker')) AS states,
                       collect(any(t IN n.tags WHERE t = 'speaker')) AS predicates,
                       collect(any(t IN n.tags WHERE t = toLower(n.marker))) AS nested_predicates;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("list predicate aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!([true, false])),
                Value::Json(serde_json::json!([true, false])),
            ]]
        );

        let empty =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'predicate-list-empty', tags: $tags})
                RETURN any(t IN n.tags WHERE t = 'speaker') AS any_speaker,
                       all(t IN n.tags WHERE t = 'speaker') AS all_speaker,
                       none(t IN n.tags WHERE t = 'speaker') AS none_speaker,
                       single(t IN n.tags WHERE t = 'speaker') AS single_speaker;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "tags".to_string(),
                        Value::StringArray(Vec::new()),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("empty list predicate projections");
        assert_eq!(
            empty.table.rows,
            vec![vec![
                Value::Bool(false),
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(false),
            ]]
        );

        let numeric_predicate_aggregate =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "MATCH (n:Person {status: 'list-predicate'}) SET n.predicate_summed = true RETURN sum(any(t IN n.tags WHERE t = 'speaker'));",
                CypherMutationOptions::default(),
            ))
            .expect_err("numeric aggregates over list predicate booleans should stay rejected");
        assert!(
            matches!(
                numeric_predicate_aggregate,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{numeric_predicate_aggregate:?}"
        );

        let type_mismatch =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'predicate-list-type', scores: $scores}) RETURN any(s IN n.scores WHERE s = '11');",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "scores".to_string(),
                        Value::IntArray(vec![11]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("type mismatched list predicates should evaluate false");
        assert_eq!(type_mismatch.table.rows, vec![vec![Value::Bool(false)]]);

        let non_array =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'predicate-list-string', name: 'Ada'}) RETURN any(ch IN n.name WHERE ch = 'A');",
                CypherMutationOptions::default(),
            ))
            .expect_err("list predicates over strings should stay rejected");
        assert!(
            matches!(non_array, GrustError::CypherUnsupportedCardinality(_)),
            "{non_array:?}"
        );

        let wrong_item =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'predicate-list-wrong-item', tags: $tags}) RETURN any(t IN n.tags WHERE other = 'speaker');",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "tags".to_string(),
                        Value::StringArray(vec!["speaker".to_string()]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect_err("list predicates should require the same WHERE item variable");
        assert!(
            matches!(wrong_item, GrustError::CypherUnsupportedCardinality(_)),
            "{wrong_item:?}"
        );

        let computed_predicate =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'predicate-list-computed', tags: $tags}) RETURN any(t IN n.tags WHERE toLower(t) = 'speaker');",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "tags".to_string(),
                        Value::StringArray(vec!["speaker".to_string()]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect_err("computed list predicate expressions should stay rejected");
        assert!(
            matches!(
                computed_predicate,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{computed_predicate:?}"
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_list_indexes_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'index-ada', tags: $tags, scores: $scores});
                CREATE (b:Person {id: 'index-bob'});
                MATCH (a:Person {id: 'index-ada'}), (b:Person {id: 'index-bob'})
                CREATE (a)-[e:KNOWS {id: 'index-knows', weights: $weights}]->(b)
                RETURN a.tags[0] AS first_tag,
                       a.scores[$score_index] AS second_score,
                       e.weights[1] AS second_weight,
                       a.tags[9] AS missing_tag,
                       a.nickname[0] AS missing_name;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        (
                            "tags".to_string(),
                            Value::StringArray(vec!["engineer".to_string(), "speaker".to_string()]),
                        ),
                        ("scores".to_string(), Value::IntArray(vec![7, 11])),
                        ("weights".to_string(), Value::FloatArray(vec![2.5, 4.5])),
                        ("score_index".to_string(), Value::Int(1)),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete list index projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::from("engineer"),
                Value::Int(11),
                Value::Float(4.5),
                Value::Null,
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'index-cara', status: 'index', scores: $scores_a, indexes: $indexes_a});
                CREATE (:Person {id: 'index-dan', status: 'index', scores: $scores_b, indexes: $indexes_b});
                MATCH (n:Person {status: 'index'}) SET n.indexed = true
                RETURN n.id AS id, n.scores[0] AS score
                ORDER BY id;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        ("scores_a".to_string(), Value::IntArray(vec![3, 5])),
                        ("scores_b".to_string(), Value::IntArray(vec![7, 9])),
                        ("indexes_a".to_string(), Value::IntArray(vec![0])),
                        ("indexes_b".to_string(), Value::IntArray(vec![0])),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("broad list index projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("index-cara"), Value::Int(3)],
                vec![Value::from("index-dan"), Value::Int(7)],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'index-team'});
                MATCH (n:Person {status: 'index'}), (t:Team {id: 'index-team'})
                CREATE (n)-[r:MEMBER_OF {rankings: $rankings}]->(t)
                RETURN n.id AS id, r.rankings[1] AS rank
                ORDER BY id;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "rankings".to_string(),
                        Value::IntArray(vec![1, 2]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("row-producing relationship list index projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("index-cara"), Value::Int(2)],
                vec![Value::from("index-dan"), Value::Int(2)],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'index'}) SET n.index_counted = true
                RETURN count(n.scores[0]) AS rows,
                       sum(n.scores[0]) AS total_scores,
                       collect(n.scores[0]) AS scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("list index aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(10),
                Value::Json(serde_json::json!([3, 7])),
            ]]
        );

        let non_array =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'index-string', name: 'Ada'}) RETURN n.name[0];",
                CypherMutationOptions::default(),
            ))
            .expect_err("list indexes over strings should stay rejected");
        assert!(
            matches!(non_array, GrustError::CypherUnsupportedCardinality(_)),
            "{non_array:?}"
        );

        let negative_index =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'index-negative', scores: $scores}) RETURN n.scores[-1];",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "scores".to_string(),
                        Value::IntArray(vec![1]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect_err("negative list indexes should stay rejected");
        assert!(
            matches!(negative_index, GrustError::CypherUnsupportedCardinality(_)),
            "{negative_index:?}"
        );

        let nested_index =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'index-nested', scores: $scores}) RETURN n.scores[head(n.scores)];",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "scores".to_string(),
                        Value::IntArray(vec![0, 7]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("nested list index expressions should execute");
        assert_eq!(nested_index.table.rows, vec![vec![Value::Int(0)]]);

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'index'}) SET n.nested_index_counted = true
                RETURN count(n.scores[head(n.indexes)]) AS rows,
                       sum(n.scores[head(n.indexes)]) AS total_scores,
                       collect(n.scores[head(n.indexes)]) AS scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested list index aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(10),
                Value::Json(serde_json::json!([3, 7])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_list_elements_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'list-ada', tags: $tags, scores: $scores, empty: $empty});
                CREATE (b:Person {id: 'list-bob'});
                MATCH (a:Person {id: 'list-ada'}), (b:Person {id: 'list-bob'})
                CREATE (a)-[e:KNOWS {id: 'list-knows', weights: $weights}]->(b)
                RETURN head(a.tags) AS first_tag,
                       last(a.scores) AS last_score,
                       head(a.empty) AS empty_head,
                       last(e.weights) AS last_weight,
                       head(a.nickname) AS missing_name;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        (
                            "tags".to_string(),
                            Value::StringArray(vec!["engineer".to_string(), "speaker".to_string()]),
                        ),
                        ("scores".to_string(), Value::IntArray(vec![7, 11])),
                        ("empty".to_string(), Value::StringArray(Vec::new())),
                        ("weights".to_string(), Value::FloatArray(vec![2.5, 4.5])),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete list element projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::from("engineer"),
                Value::Int(11),
                Value::Null,
                Value::Float(4.5),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'list-cara', status: 'list', scores: $scores_a});
                CREATE (:Person {id: 'list-dan', status: 'list', scores: $scores_b});
                MATCH (n:Person {status: 'list'}) SET n.seen = true
                RETURN n.id AS id, head(n.scores) AS score
                ORDER BY id;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        ("scores_a".to_string(), Value::IntArray(vec![3, 5])),
                        ("scores_b".to_string(), Value::IntArray(vec![7, 9])),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("broad list element projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("list-cara"), Value::Int(3)],
                vec![Value::from("list-dan"), Value::Int(7)],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'list-team'});
                MATCH (n:Person {status: 'list'}), (t:Team {id: 'list-team'})
                CREATE (n)-[r:MEMBER_OF {rankings: $rankings}]->(t)
                RETURN n.id AS id, last(r.rankings) AS rank
                ORDER BY id;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "rankings".to_string(),
                        Value::IntArray(vec![1, 2]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("row-producing relationship list element projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("list-cara"), Value::Int(2)],
                vec![Value::from("list-dan"), Value::Int(2)],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'list'}) SET n.list_counted = true
                RETURN count(head(n.scores)) AS rows,
                       sum(head(n.scores)) AS total_scores,
                       collect(head(n.scores)) AS scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("list element aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(10),
                Value::Json(serde_json::json!([3, 7])),
            ]]
        );

        let string_head =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'list-string', name: 'Ada'}) RETURN head(n.name);",
                CypherMutationOptions::default(),
            ))
            .expect_err("head over string values should stay rejected");
        assert!(
            matches!(string_head, GrustError::CypherUnsupportedCardinality(_)),
            "{string_head:?}"
        );

        let nested_head =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'list-nested', path: 'a/b'}) RETURN head(split(n.path, '/'));",
                CypherMutationOptions::default(),
            ))
            .expect("nested head arguments should execute");
        assert_eq!(nested_head.table.rows, vec![vec![Value::from("a")]]);

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'list'}) SET n.nested_head_counted = true
                RETURN count(head(tail(n.scores))) AS rows,
                       sum(head(tail(n.scores))) AS total_scores,
                       collect(head(tail(n.scores))) AS scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested head aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(14),
                Value::Json(serde_json::json!([5, 9])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_list_tail_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'tail-ada', tags: $tags, scores: $scores, empty: $empty});
                CREATE (b:Person {id: 'tail-bob'});
                MATCH (a:Person {id: 'tail-ada'}), (b:Person {id: 'tail-bob'})
                CREATE (a)-[e:KNOWS {id: 'tail-knows', weights: $weights}]->(b)
                RETURN tail(a.tags) AS tag_tail,
                       tail(a.scores) AS score_tail,
                       tail(a.empty) AS empty_tail,
                       tail(e.weights) AS weight_tail,
                       tail(a.nickname) AS missing_name;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        (
                            "tags".to_string(),
                            Value::StringArray(vec!["engineer".to_string(), "speaker".to_string()]),
                        ),
                        ("scores".to_string(), Value::IntArray(vec![7, 11])),
                        ("empty".to_string(), Value::StringArray(Vec::new())),
                        ("weights".to_string(), Value::FloatArray(vec![2.5, 4.5])),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete list tail projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::StringArray(vec!["speaker".to_string()]),
                Value::IntArray(vec![11]),
                Value::StringArray(Vec::new()),
                Value::FloatArray(vec![4.5]),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'tail-cara', status: 'tail', scores: $scores_a});
                CREATE (:Person {id: 'tail-dan', status: 'tail', scores: $scores_b});
                MATCH (n:Person {status: 'tail'}) SET n.seen = true
                RETURN n.id AS id, tail(n.scores) AS scores
                ORDER BY id;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        ("scores_a".to_string(), Value::IntArray(vec![3, 5])),
                        ("scores_b".to_string(), Value::IntArray(vec![7, 9])),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("broad list tail projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("tail-cara"), Value::IntArray(vec![5])],
                vec![Value::from("tail-dan"), Value::IntArray(vec![9])],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'tail-team'});
                MATCH (n:Person {status: 'tail'}), (t:Team {id: 'tail-team'})
                CREATE (n)-[r:MEMBER_OF {rankings: $rankings}]->(t)
                RETURN n.id AS id, tail(r.rankings) AS ranks
                ORDER BY id;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "rankings".to_string(),
                        Value::IntArray(vec![1, 2]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("row-producing relationship list tail projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("tail-cara"), Value::IntArray(vec![2])],
                vec![Value::from("tail-dan"), Value::IntArray(vec![2])],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'tail'}) SET n.tail_counted = true
                RETURN count(tail(n.scores)) AS rows,
                       collect(tail(n.scores)) AS score_tails;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("list tail aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Json(serde_json::json!([[5], [9]])),
            ]]
        );

        let string_tail =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'tail-string', name: 'Ada'}) RETURN tail(n.name);",
                CypherMutationOptions::default(),
            ))
            .expect_err("tail over string values should stay rejected");
        assert!(
            matches!(string_tail, GrustError::CypherUnsupportedCardinality(_)),
            "{string_tail:?}"
        );

        let nested_tail =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'tail-nested', path: 'a/b'}) RETURN tail(split(n.path, '/'));",
                CypherMutationOptions::default(),
            ))
            .expect("nested tail arguments should execute");
        assert_eq!(
            nested_tail.table.rows,
            vec![vec![Value::StringArray(vec!["b".to_string()])]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'tail-path-a', status: 'tail-path', path: 'a/b'});
                CREATE (:Person {id: 'tail-path-b', status: 'tail-path', path: 'c/d/e'});
                MATCH (n:Person {status: 'tail-path'}) SET n.nested_tail_counted = true
                RETURN count(tail(split(n.path, '/'))) AS rows,
                       collect(tail(split(n.path, '/'))) AS tails;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested tail aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Json(serde_json::json!([["b"], ["d", "e"]])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_is_empty_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'empty-ada', name: '', tags: $tags, codes: $codes})
                RETURN isEmpty(n.name) AS empty_name,
                       isEmpty(n.tags) AS empty_tags,
                       isEmpty(n.codes) AS empty_codes,
                       isEmpty(n.nickname) AS missing_name;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        ("tags".to_string(), Value::StringArray(Vec::new())),
                        (
                            "codes".to_string(),
                            Value::StringArray(vec!["A".to_string()]),
                        ),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete isEmpty projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(false),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'empty-bob', status: 'empty', nickname: ''});
                CREATE (:Person {id: 'empty-cara', status: 'empty', nickname: 'Cara'});
                MATCH (n:Person {status: 'empty'}) SET n.seen = true
                RETURN n.id AS id, isEmpty(n.nickname) AS empty_nickname
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad isEmpty projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("empty-bob"), Value::Bool(true)],
                vec![Value::from("empty-cara"), Value::Bool(false)],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'empty-team'});
                MATCH (n:Person {status: 'empty'}), (t:Team {id: 'empty-team'})
                CREATE (n)-[r:MEMBER_OF {source: ''}]->(t)
                RETURN n.id AS id, isEmpty(r.source) AS empty_source
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship isEmpty projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("empty-bob"), Value::Bool(true)],
                vec![Value::from("empty-cara"), Value::Bool(true)],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'empty'}) SET n.empty_counted = true
                RETURN count(isEmpty(n.nickname)) AS rows,
                       count(DISTINCT isEmpty(n.nickname)) AS distinct_states,
                       collect(isEmpty(n.nickname)) AS states;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("isEmpty aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!([true, false])),
            ]]
        );

        let numeric_empty =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'empty-number', score: 3}) RETURN isEmpty(n.score);",
                CypherMutationOptions::default(),
            ))
            .expect_err("isEmpty over numeric values should stay rejected");
        assert!(
            matches!(numeric_empty, GrustError::CypherUnsupportedCardinality(_)),
            "{numeric_empty:?}"
        );

        let nested_empty =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'empty-nested', name: '', tags: $tags})
                RETURN isEmpty(toLower(n.name)) AS empty_lower,
                       isEmpty(coalesce(n.nickname, '')) AS fallback_empty,
                       isEmpty(range(1, 0)) AS empty_range,
                       isEmpty('static') AS literal_empty;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "tags".to_string(),
                        Value::StringArray(Vec::new()),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("nested isEmpty arguments should execute");
        assert_eq!(
            nested_empty.table.rows,
            vec![vec![
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(false),
            ]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'empty'}) SET n.nested_empty_counted = true
                RETURN count(isEmpty(toLower(n.nickname))) AS rows,
                       collect(isEmpty(coalesce(n.nickname, ''))) AS states;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested isEmpty aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Json(serde_json::json!([true, false])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_to_string_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'to-string-ada', name: 'Ada', score: 3, active: true});
                CREATE (b:Person {id: 'to-string-bob'});
                MATCH (a:Person {id: 'to-string-ada'}), (b:Person {id: 'to-string-bob'})
                CREATE (a)-[e:KNOWS {id: 'to-string-knows', weight: 2.5}]->(b)
                RETURN toString(a.name) AS name,
                       toString(a.score) AS score,
                       toString(a.active) AS active,
                       toString(e.weight) AS weight,
                       toString(a.nickname) AS missing_name;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("concrete toString projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::from("Ada"),
                Value::from("3"),
                Value::from("true"),
                Value::from("2.5"),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'to-string-cara', status: 'to-string', score: 7});
                CREATE (:Person {id: 'to-string-dan', status: 'to-string', score: 11});
                MATCH (n:Person {status: 'to-string'}) SET n.seen = true
                RETURN n.id AS id, toString(n.score) AS score
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad toString projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("to-string-cara"), Value::from("7")],
                vec![Value::from("to-string-dan"), Value::from("11")],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'to-string-team'});
                MATCH (n:Person {status: 'to-string'}), (t:Team {id: 'to-string-team'})
                CREATE (n)-[r:MEMBER_OF {rank: 5}]->(t)
                RETURN n.id AS id, toString(r.rank) AS rank
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship toString projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("to-string-cara"), Value::from("5")],
                vec![Value::from("to-string-dan"), Value::from("5")],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'to-string'}) SET n.to_string_counted = true
                RETURN count(toString(n.score)) AS rows,
                       count(DISTINCT toString(n.score)) AS distinct_scores,
                       collect(toString(n.score)) AS scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("toString aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!(["7", "11"])),
            ]]
        );

        let array_to_string =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'to-string-array', tags: $tags})
                RETURN toString(n.tags);
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "tags".to_string(),
                        Value::StringArray(vec!["a".to_string()]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect_err("toString over arrays should stay rejected");
        assert!(
            matches!(array_to_string, GrustError::CypherUnsupportedCardinality(_)),
            "{array_to_string:?}"
        );

        let nested_to_string =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'to-string-nested', name: 'Ada'})
                RETURN toString(toLower(n.name)) AS lowered,
                       toString(coalesce(n.nickname, 'Fallback')) AS fallback,
                       toString(42) AS literal_number;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested toString arguments should execute");
        assert_eq!(
            nested_to_string.table.rows,
            vec![vec![
                Value::from("ada"),
                Value::from("Fallback"),
                Value::from("42"),
            ]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'to-string'}) SET n.nested_to_string_counted = true
                RETURN count(toString(toLower(n.id))) AS rows,
                       collect(toString(coalesce(n.nickname, n.score))) AS scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested toString aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Json(serde_json::json!(["7", "11"])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_abs_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'abs-ada', debt: -3, ratio: -2.5});
                CREATE (b:Person {id: 'abs-bob'});
                MATCH (a:Person {id: 'abs-ada'}), (b:Person {id: 'abs-bob'})
                CREATE (a)-[e:KNOWS {id: 'abs-knows', weight: -4}]->(b)
                RETURN abs(a.debt) AS debt,
                       abs(a.ratio) AS ratio,
                       abs(e.weight) AS weight,
                       abs(a.nickname) AS missing_name;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("concrete abs projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::Int(3),
                Value::Float(2.5),
                Value::Int(4),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'abs-cara', status: 'abs', score: -7});
                CREATE (:Person {id: 'abs-dan', status: 'abs', score: -11});
                MATCH (n:Person {status: 'abs'}) SET n.seen = true
                RETURN n.id AS id, abs(n.score) AS score
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad abs projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("abs-cara"), Value::Int(7)],
                vec![Value::from("abs-dan"), Value::Int(11)],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'abs-team'});
                MATCH (n:Person {status: 'abs'}), (t:Team {id: 'abs-team'})
                CREATE (n)-[r:MEMBER_OF {rank: -5}]->(t)
                RETURN n.id AS id, abs(r.rank) AS rank
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship abs projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("abs-cara"), Value::Int(5)],
                vec![Value::from("abs-dan"), Value::Int(5)],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'abs'}) SET n.abs_counted = true
                RETURN count(abs(n.score)) AS rows,
                       sum(abs(n.score)) AS total_scores,
                       collect(abs(n.score)) AS scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("abs aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(18),
                Value::Json(serde_json::json!([7, 11])),
            ]]
        );

        let string_abs =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'abs-string', score: '3'}) RETURN abs(n.score);",
                CypherMutationOptions::default(),
            ))
            .expect_err("abs over string values should stay rejected");
        assert!(
            matches!(string_abs, GrustError::CypherUnsupportedCardinality(_)),
            "{string_abs:?}"
        );

        let nested_abs =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'abs-nested', score: -3, nickname: 'Ada'})
                RETURN abs(abs(n.score)) AS nested_abs,
                       abs(size(n.nickname)) AS nickname_size,
                       abs(-42) AS literal_abs;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested abs arguments should execute");
        assert_eq!(
            nested_abs.table.rows,
            vec![vec![Value::Int(3), Value::Int(3), Value::Int(42)]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'abs'}) SET n.nested_abs_counted = true
                RETURN count(abs(abs(n.score))) AS rows,
                       sum(abs(abs(n.score))) AS total_scores,
                       collect(abs(abs(n.score))) AS scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested abs aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(18),
                Value::Json(serde_json::json!([7, 11])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_numeric_rounds_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'round-ada', debt: -3.2, ratio: 2.1});
                CREATE (b:Person {id: 'round-bob'});
                MATCH (a:Person {id: 'round-ada'}), (b:Person {id: 'round-bob'})
                CREATE (a)-[e:KNOWS {id: 'round-knows', weight: -4.8}]->(b)
                RETURN ceil(a.debt) AS debt_ceiling,
                       floor(a.ratio) AS ratio_floor,
                       floor(e.weight) AS weight_floor,
                       ceil(a.nickname) AS missing_name;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("concrete numeric rounding projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::Float(-3.0),
                Value::Float(2.0),
                Value::Float(-5.0),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'round-cara', status: 'round', score: 7.2});
                CREATE (:Person {id: 'round-dan', status: 'round', score: 11.8});
                MATCH (n:Person {status: 'round'}) SET n.seen = true
                RETURN n.id AS id, ceil(n.score) AS score
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad numeric rounding projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("round-cara"), Value::Float(8.0)],
                vec![Value::from("round-dan"), Value::Float(12.0)],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'round-team'});
                MATCH (n:Person {status: 'round'}), (t:Team {id: 'round-team'})
                CREATE (n)-[r:MEMBER_OF {rank: -5.3}]->(t)
                RETURN n.id AS id, floor(r.rank) AS rank
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship numeric rounding projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("round-cara"), Value::Float(-6.0)],
                vec![Value::from("round-dan"), Value::Float(-6.0)],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'round'}) SET n.round_counted = true
                RETURN count(ceil(n.score)) AS rows,
                       sum(ceil(n.score)) AS total_scores,
                       collect(ceil(n.score)) AS scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("numeric rounding aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Float(20.0),
                Value::Json(serde_json::json!([8.0, 12.0])),
            ]]
        );

        let string_round =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'round-string', score: '3'}) RETURN ceil(n.score);",
                CypherMutationOptions::default(),
            ))
            .expect_err("ceil over string values should stay rejected");
        assert!(
            matches!(string_round, GrustError::CypherUnsupportedCardinality(_)),
            "{string_round:?}"
        );

        let nested_rounds =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'round-nested', score: -3.2, nickname: 'Ada'})
                RETURN ceil(abs(n.score)) AS debt_ceiling,
                       floor(abs(n.score)) AS debt_floor,
                       ceil(size(n.nickname)) AS nickname_ceiling,
                       floor(2.9) AS literal_floor;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested numeric rounding projections should execute");
        assert_eq!(
            nested_rounds.table.rows,
            vec![vec![
                Value::Float(4.0),
                Value::Float(3.0),
                Value::Int(3),
                Value::Float(2.0),
            ]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'round'}) SET n.nested_round_counted = true
                RETURN count(ceil(abs(n.score))) AS rows,
                       sum(ceil(abs(n.score))) AS total_scores,
                       collect(ceil(abs(n.score))) AS scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested numeric rounding aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Float(20.0),
                Value::Json(serde_json::json!([8.0, 12.0])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_numeric_sign_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'sign-ada', debt: -3, ratio: 2.1, zero: 0});
                CREATE (b:Person {id: 'sign-bob'});
                MATCH (a:Person {id: 'sign-ada'}), (b:Person {id: 'sign-bob'})
                CREATE (a)-[e:KNOWS {id: 'sign-knows', weight: -4.8}]->(b)
                RETURN sign(a.debt) AS debt_sign,
                       sign(a.ratio) AS ratio_sign,
                       sign(a.zero) AS zero_sign,
                       sign(e.weight) AS weight_sign,
                       sign(a.nickname) AS missing_name;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("concrete numeric sign projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::Int(-1),
                Value::Float(1.0),
                Value::Int(0),
                Value::Float(-1.0),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'sign-cara', status: 'sign', score: -7});
                CREATE (:Person {id: 'sign-dan', status: 'sign', score: 11});
                MATCH (n:Person {status: 'sign'}) SET n.seen = true
                RETURN n.id AS id, sign(n.score) AS score
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad numeric sign projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("sign-cara"), Value::Int(-1)],
                vec![Value::from("sign-dan"), Value::Int(1)],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'sign-team'});
                MATCH (n:Person {status: 'sign'}), (t:Team {id: 'sign-team'})
                CREATE (n)-[r:MEMBER_OF {rank: -5.3}]->(t)
                RETURN n.id AS id, sign(r.rank) AS rank
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship numeric sign projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("sign-cara"), Value::Float(-1.0)],
                vec![Value::from("sign-dan"), Value::Float(-1.0)],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'sign'}) SET n.sign_counted = true
                RETURN count(sign(n.score)) AS rows,
                       sum(sign(n.score)) AS total_scores,
                       collect(sign(n.score)) AS scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("numeric sign aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(0),
                Value::Json(serde_json::json!([-1, 1])),
            ]]
        );

        let string_sign =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'sign-string', score: '3'}) RETURN sign(n.score);",
                CypherMutationOptions::default(),
            ))
            .expect_err("sign over string values should stay rejected");
        assert!(
            matches!(string_sign, GrustError::CypherUnsupportedCardinality(_)),
            "{string_sign:?}"
        );

        let nested_sign =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'sign-nested', score: -3, nickname: 'Ada'})
                RETURN sign(abs(n.score)) AS positive_sign,
                       sign(size(n.nickname)) AS nickname_sign,
                       sign(-42) AS literal_sign;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested numeric sign projections should execute");
        assert_eq!(
            nested_sign.table.rows,
            vec![vec![Value::Int(1), Value::Int(1), Value::Int(-1)]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'sign'}) SET n.nested_sign_counted = true
                RETURN count(sign(abs(n.score))) AS rows,
                       sum(sign(abs(n.score))) AS total_scores,
                       collect(sign(abs(n.score))) AS scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested numeric sign aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!([1, 1])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_numeric_casts_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'cast-ada', score: 7, ratio: 2.9, text_score: '42'});
                CREATE (b:Person {id: 'cast-bob'});
                MATCH (a:Person {id: 'cast-ada'}), (b:Person {id: 'cast-bob'})
                CREATE (a)-[e:KNOWS {id: 'cast-knows', weight: '4.5'}]->(b)
                RETURN toFloat(a.score) AS score_float,
                       toInteger(a.ratio) AS ratio_int,
                       toInteger(a.text_score) AS text_score_int,
                       toFloat(e.weight) AS weight_float,
                       toInteger(a.nickname) AS missing_name;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("concrete numeric cast projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::Float(7.0),
                Value::Int(2),
                Value::Int(42),
                Value::Float(4.5),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'cast-cara', status: 'cast', score: 7.2});
                CREATE (:Person {id: 'cast-dan', status: 'cast', score: 11.8});
                MATCH (n:Person {status: 'cast'}) SET n.seen = true
                RETURN n.id AS id, toInteger(n.score) AS score
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad numeric cast projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("cast-cara"), Value::Int(7)],
                vec![Value::from("cast-dan"), Value::Int(11)],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'cast-team'});
                MATCH (n:Person {status: 'cast'}), (t:Team {id: 'cast-team'})
                CREATE (n)-[r:MEMBER_OF {rank: 5}]->(t)
                RETURN n.id AS id, toFloat(r.rank) AS rank
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship numeric cast projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("cast-cara"), Value::Float(5.0)],
                vec![Value::from("cast-dan"), Value::Float(5.0)],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'cast'}) SET n.cast_counted = true
                RETURN count(toInteger(n.score)) AS rows,
                       sum(toInteger(n.score)) AS total_scores,
                       collect(toInteger(n.score)) AS scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("numeric cast aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(18),
                Value::Json(serde_json::json!([7, 11])),
            ]]
        );

        let boolean_cast =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'cast-bool', score: true}) RETURN toInteger(n.score);",
                CypherMutationOptions::default(),
            ))
            .expect_err("toInteger over boolean values should stay rejected");
        assert!(
            matches!(boolean_cast, GrustError::CypherUnsupportedCardinality(_)),
            "{boolean_cast:?}"
        );

        let non_integer_string =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'cast-string', score: '3.5'}) RETURN toInteger(n.score);",
                CypherMutationOptions::default(),
            ))
            .expect_err("toInteger over non-integer strings should stay rejected");
        assert!(
            matches!(
                non_integer_string,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{non_integer_string:?}"
        );

        let nested_cast =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'cast-nested', score: -3, nickname: 'Ada'})
                RETURN toFloat(abs(n.score)) AS score_float,
                       toInteger(size(n.nickname)) AS nickname_size,
                       toInteger('42') AS literal_int;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested numeric cast projections should execute");
        assert_eq!(
            nested_cast.table.rows,
            vec![vec![Value::Float(3.0), Value::Int(3), Value::Int(42)]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'cast'}) SET n.nested_cast_counted = true
                RETURN count(toFloat(abs(n.score))) AS rows,
                       sum(toFloat(abs(n.score))) AS total_scores,
                       collect(toFloat(abs(n.score))) AS scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested numeric cast aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Float(19.0),
                Value::Json(serde_json::json!([7.2, 11.8])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_boolean_cast_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'bool-ada', active: true, enabled: 'FALSE'});
                CREATE (b:Person {id: 'bool-bob'});
                MATCH (a:Person {id: 'bool-ada'}), (b:Person {id: 'bool-bob'})
                CREATE (a)-[e:KNOWS {id: 'bool-knows', trusted: 'true'}]->(b)
                RETURN toBoolean(a.active) AS active,
                       toBoolean(a.enabled) AS enabled,
                       toBoolean(e.trusted) AS trusted,
                       toBoolean(a.nickname) AS missing_name;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("concrete boolean cast projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::Bool(true),
                Value::Bool(false),
                Value::Bool(true),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'bool-cara', status: 'bool', active: 'true'});
                CREATE (:Person {id: 'bool-dan', status: 'bool', active: 'false'});
                MATCH (n:Person {status: 'bool'}) SET n.seen = true
                RETURN n.id AS id, toBoolean(n.active) AS active
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad boolean cast projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("bool-cara"), Value::Bool(true)],
                vec![Value::from("bool-dan"), Value::Bool(false)],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'bool-team'});
                MATCH (n:Person {status: 'bool'}), (t:Team {id: 'bool-team'})
                CREATE (n)-[r:MEMBER_OF {trusted: false}]->(t)
                RETURN n.id AS id, toBoolean(r.trusted) AS trusted
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship boolean cast projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("bool-cara"), Value::Bool(false)],
                vec![Value::from("bool-dan"), Value::Bool(false)],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'bool'}) SET n.bool_counted = true
                RETURN count(toBoolean(n.active)) AS rows,
                       count(DISTINCT toBoolean(n.active)) AS distinct_states,
                       collect(toBoolean(n.active)) AS states;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("boolean cast aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!([true, false])),
            ]]
        );

        let numeric_cast =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'bool-number', active: 1}) RETURN toBoolean(n.active);",
                CypherMutationOptions::default(),
            ))
            .expect_err("toBoolean over numeric values should stay rejected");
        assert!(
            matches!(numeric_cast, GrustError::CypherUnsupportedCardinality(_)),
            "{numeric_cast:?}"
        );

        let invalid_string =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'bool-string', active: 'yes'}) RETURN toBoolean(n.active);",
                CypherMutationOptions::default(),
            ))
            .expect_err("toBoolean over non-boolean strings should stay rejected");
        assert!(
            matches!(invalid_string, GrustError::CypherUnsupportedCardinality(_)),
            "{invalid_string:?}"
        );

        let nested_cast =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'bool-nested', active: true, active_text: 'false'})
                RETURN toBoolean(toString(n.active)) AS active_string,
                       toBoolean(coalesce(n.missing, n.active_text)) AS fallback_text,
                       toBoolean('true') AS literal_bool;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested boolean cast projections should execute");
        assert_eq!(
            nested_cast.table.rows,
            vec![vec![
                Value::Bool(true),
                Value::Bool(false),
                Value::Bool(true),
            ]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'bool'}) SET n.nested_bool_counted = true
                RETURN count(toBoolean(toString(n.active))) AS rows,
                       count(DISTINCT toBoolean(toString(n.active))) AS distinct_states,
                       collect(toBoolean(toString(n.active))) AS states;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested boolean cast aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!([true, false])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_list_casts_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {
                    id: 'list-cast-ada',
                    scores: $scores,
                    text_scores: $text_scores,
                    ratios: $ratios,
                    flags: $flags,
                    json_numbers: $json_numbers
                });
                CREATE (b:Person {id: 'list-cast-bob'});
                MATCH (a:Person {id: 'list-cast-ada'}), (b:Person {id: 'list-cast-bob'})
                CREATE (a)-[e:KNOWS {id: 'list-cast-knows', ranks: $ranks}]->(b)
                RETURN toStringList(a.scores) AS score_strings,
                       toIntegerList(a.text_scores) AS score_ints,
                       toFloatList(a.ratios) AS ratio_floats,
                       toBooleanList(a.flags) AS flag_bools,
                       toIntegerList(a.json_numbers) AS json_ints,
                       toIntegerList(e.ranks) AS edge_ranks,
                       toStringList(a.nickname) AS missing_name;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        ("scores".to_string(), Value::IntArray(vec![7, 11])),
                        (
                            "text_scores".to_string(),
                            Value::StringArray(vec!["3".to_string(), "5".to_string()]),
                        ),
                        ("ratios".to_string(), Value::FloatArray(vec![2.5, 4.0])),
                        (
                            "flags".to_string(),
                            Value::StringArray(vec!["true".to_string(), "FALSE".to_string()]),
                        ),
                        (
                            "json_numbers".to_string(),
                            Value::Json(serde_json::json!(["8", 13])),
                        ),
                        (
                            "ranks".to_string(),
                            Value::StringArray(vec!["1".to_string(), "2".to_string()]),
                        ),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete list cast projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::StringArray(vec!["7".to_string(), "11".to_string()]),
                Value::IntArray(vec![3, 5]),
                Value::FloatArray(vec![2.5, 4.0]),
                Value::Json(serde_json::json!([true, false])),
                Value::IntArray(vec![8, 13]),
                Value::IntArray(vec![1, 2]),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'list-cast-cara', status: 'list-cast', scores: $scores_a});
                CREATE (:Person {id: 'list-cast-dan', status: 'list-cast', scores: $scores_b});
                MATCH (n:Person {status: 'list-cast'}) SET n.cast_seen = true
                RETURN n.id AS id, toFloatList(n.scores) AS scores
                ORDER BY id;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        ("scores_a".to_string(), Value::IntArray(vec![3, 5])),
                        ("scores_b".to_string(), Value::IntArray(vec![7, 9])),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("broad list cast projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![
                    Value::from("list-cast-cara"),
                    Value::FloatArray(vec![3.0, 5.0]),
                ],
                vec![
                    Value::from("list-cast-dan"),
                    Value::FloatArray(vec![7.0, 9.0]),
                ],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'list-cast'}) SET n.cast_counted = true
                RETURN count(toStringList(n.scores)) AS rows,
                       collect(toStringList(n.scores)) AS scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("list cast aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Json(serde_json::json!([["3", "5"], ["7", "9"]])),
            ]]
        );

        let scalar_cast =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'list-cast-scalar', score: 3}) RETURN toStringList(n.score);",
                CypherMutationOptions::default(),
            ))
            .expect_err("list casts over scalar values should stay rejected");
        assert!(
            matches!(scalar_cast, GrustError::CypherUnsupportedCardinality(_)),
            "{scalar_cast:?}"
        );

        let invalid_element =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'list-cast-invalid', scores: $scores}) RETURN toIntegerList(n.scores);",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "scores".to_string(),
                        Value::StringArray(vec!["3.5".to_string()]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect_err("invalid list cast elements should stay rejected");
        assert!(
            matches!(invalid_element, GrustError::CypherUnsupportedCardinality(_)),
            "{invalid_element:?}"
        );

        let nested_cast =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'list-cast-nested', tags: $tags}) RETURN toStringList(tail(n.tags));",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "tags".to_string(),
                        Value::StringArray(vec!["a".to_string(), "b".to_string()]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("nested list cast arguments should execute");
        assert_eq!(
            nested_cast.table.rows,
            vec![vec![Value::StringArray(vec!["b".to_string()])]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'list-cast'}) SET n.nested_cast_counted = true
                RETURN count(toStringList(tail(n.scores))) AS rows,
                       collect(toStringList(tail(n.scores))) AS score_tails;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested list cast aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Json(serde_json::json!([["5"], ["9"]])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_string_transforms_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'string-ada', name: 'Ada Lovelace'});
                CREATE (b:Person {id: 'string-bob'});
                MATCH (a:Person {id: 'string-ada'}), (b:Person {id: 'string-bob'})
                CREATE (a)-[e:KNOWS {id: 'string-knows', source: 'Conference'}]->(b)
                RETURN toLower(a.name) AS lower_name,
                       toUpper(a.name) AS upper_name,
                       toLower(e.source) AS lower_source,
                       toUpper(a.nickname) AS missing_name;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("concrete string transform projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::from("ada lovelace"),
                Value::from("ADA LOVELACE"),
                Value::from("conference"),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'string-cara', status: 'string', team: 'Eng'});
                CREATE (:Person {id: 'string-dan', status: 'string', team: 'Ops'});
                MATCH (n:Person {status: 'string'}) SET n.seen = true
                RETURN n.id AS id, toLower(n.team) AS team
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad string transform projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("string-cara"), Value::from("eng")],
                vec![Value::from("string-dan"), Value::from("ops")],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'string-team'});
                MATCH (n:Person {status: 'string'}), (t:Team {id: 'string-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'StringSlice'}]->(t)
                RETURN n.id AS id, toUpper(r.source) AS source
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship string transform projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("string-cara"), Value::from("STRINGSLICE")],
                vec![Value::from("string-dan"), Value::from("STRINGSLICE")],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'string'}) SET n.string_counted = true
                RETURN count(toLower(n.team)) AS rows,
                       count(DISTINCT toLower(n.team)) AS distinct_teams,
                       collect(toUpper(n.team)) AS teams;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("string transform aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!(["ENG", "OPS"])),
            ]]
        );

        let numeric_transform =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'string-number', score: 3}) RETURN toLower(n.score);",
                CypherMutationOptions::default(),
            ))
            .expect_err("string transforms over numeric values should stay rejected");
        assert!(
            matches!(
                numeric_transform,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{numeric_transform:?}"
        );

        let nested_transform =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'string-nested', name: 'Ada'})
                RETURN toLower(coalesce(n.name, 'unknown')) AS lower_name,
                       toUpper(toLower(n.name)) AS nested_name,
                       toLower('STATIC') AS literal_name;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested string transforms should execute");
        assert_eq!(
            nested_transform.table.rows,
            vec![vec![
                Value::from("ada"),
                Value::from("ADA"),
                Value::from("static"),
            ]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'string'}) SET n.nested_string_counted = true
                RETURN count(toLower(coalesce(n.nickname, n.team))) AS rows,
                       collect(toUpper(toLower(n.team))) AS teams;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested string transform aggregates should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Json(serde_json::json!(["ENG", "OPS"])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_string_trims_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'trim-ada', name: '  Ada  '});
                CREATE (b:Person {id: 'trim-bob'});
                MATCH (a:Person {id: 'trim-ada'}), (b:Person {id: 'trim-bob'})
                CREATE (a)-[e:KNOWS {id: 'trim-knows', source: '  Conference  '}]->(b)
                RETURN trim(a.name) AS trimmed_name,
                       lTrim(a.name) AS left_trimmed_name,
                       rTrim(e.source) AS right_trimmed_source,
                       trim(a.nickname) AS missing_name;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("concrete string trim projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::from("Ada"),
                Value::from("Ada  "),
                Value::from("  Conference"),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'trim-cara', status: 'trim', team: ' Eng '});
                CREATE (:Person {id: 'trim-dan', status: 'trim', team: ' Ops '});
                MATCH (n:Person {status: 'trim'}) SET n.seen = true
                RETURN n.id AS id, trim(n.team) AS team
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad string trim projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("trim-cara"), Value::from("Eng")],
                vec![Value::from("trim-dan"), Value::from("Ops")],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'trim-team'});
                MATCH (n:Person {status: 'trim'}), (t:Team {id: 'trim-team'})
                CREATE (n)-[r:MEMBER_OF {source: ' TrimSlice '}]->(t)
                RETURN n.id AS id, lTrim(r.source) AS source
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship string trim projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("trim-cara"), Value::from("TrimSlice ")],
                vec![Value::from("trim-dan"), Value::from("TrimSlice ")],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'trim'}) SET n.trim_counted = true
                RETURN count(trim(n.team)) AS rows,
                       count(DISTINCT trim(n.team)) AS distinct_teams,
                       collect(trim(n.team)) AS teams;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("string trim aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!(["Eng", "Ops"])),
            ]]
        );

        let numeric_trim =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'trim-number', score: 3}) RETURN trim(n.score);",
                CypherMutationOptions::default(),
            ))
            .expect_err("string trims over numeric values should stay rejected");
        assert!(
            matches!(numeric_trim, GrustError::CypherUnsupportedCardinality(_)),
            "{numeric_trim:?}"
        );

        let nested_trim =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'trim-nested', name: ' Ada '})
                RETURN trim(toLower(n.name)) AS trimmed_lower,
                       lTrim(coalesce(n.nickname, ' fallback ')) AS fallback,
                       rTrim(' static ') AS literal_trim;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested string trims should execute");
        assert_eq!(
            nested_trim.table.rows,
            vec![vec![
                Value::from("ada"),
                Value::from("fallback "),
                Value::from(" static"),
            ]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'trim'}) SET n.nested_trim_counted = true
                RETURN count(trim(toLower(n.team))) AS rows,
                       collect(rTrim(toUpper(n.team))) AS teams;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested string trim aggregates should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Json(serde_json::json!([" ENG", " OPS"])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_substring_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'substring-ada', name: 'Ada Lovelace'});
                CREATE (b:Person {id: 'substring-bob'});
                MATCH (a:Person {id: 'substring-ada'}), (b:Person {id: 'substring-bob'})
                CREATE (a)-[e:KNOWS {id: 'substring-knows', source: 'Conference'}]->(b)
                RETURN substring(a.name, 0, 3) AS first_name,
                       substring(a.name, 4) AS last_name,
                       substring(e.source, $start, $length) AS source_part,
                       substring(a.nickname, 0, 2) AS missing_name;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        ("start".to_string(), Value::Int(3)),
                        ("length".to_string(), Value::Int(4)),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete substring projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::from("Ada"),
                Value::from("Lovelace"),
                Value::from("fere"),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'substring-cara', status: 'substring', team: 'Engineering'});
                CREATE (:Person {id: 'substring-dan', status: 'substring', team: 'Operations'});
                MATCH (n:Person {status: 'substring'}) SET n.seen = true
                RETURN n.id AS id, substring(n.team, 0, 3) AS team
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad substring projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("substring-cara"), Value::from("Eng")],
                vec![Value::from("substring-dan"), Value::from("Ope")],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'substring-team'});
                MATCH (n:Person {status: 'substring'}), (t:Team {id: 'substring-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'SubstringSlice'}]->(t)
                RETURN n.id AS id, substring(r.source, 9, 5) AS source
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship substring projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("substring-cara"), Value::from("Slice")],
                vec![Value::from("substring-dan"), Value::from("Slice")],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'substring'}) SET n.substring_counted = true
                RETURN count(substring(n.team, 0, 3)) AS rows,
                       count(DISTINCT substring(n.team, 0, 3)) AS distinct_prefixes,
                       collect(substring(n.team, 0, 3)) AS prefixes;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("substring aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!(["Eng", "Ope"])),
            ]]
        );

        let numeric_substring =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'substring-number', score: 3}) RETURN substring(n.score, 0, 1);",
                CypherMutationOptions::default(),
            ))
            .expect_err("substring over numeric values should stay rejected");
        assert!(
            matches!(
                numeric_substring,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{numeric_substring:?}"
        );

        let negative_start =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'substring-negative', name: 'Ada'}) RETURN substring(n.name, -1, 1);",
                CypherMutationOptions::default(),
            ))
            .expect_err("negative substring offsets should stay rejected");
        assert!(
            matches!(negative_start, GrustError::CypherUnsupportedCardinality(_)),
            "{negative_start:?}"
        );

        let nested_substring =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'substring-nested', name: 'Ada'})
                RETURN substring(toLower(n.name), 0, 1) AS lowered_initial,
                       substring(coalesce(n.nickname, 'Fallback'), 0, 4) AS fallback_prefix,
                       substring('static', 1, 3) AS literal_slice;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested substring arguments should execute");
        assert_eq!(
            nested_substring.table.rows,
            vec![vec![
                Value::from("a"),
                Value::from("Fall"),
                Value::from("tat"),
            ]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'substring'}) SET n.nested_substring_counted = true
                RETURN count(substring(toLower(n.team), 0, 3)) AS rows,
                       collect(substring(toLower(n.team), 0, 3)) AS prefixes;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested substring aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Json(serde_json::json!(["eng", "ope"])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_replace_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'replace-ada', name: 'Ada Lovelace'});
                CREATE (b:Person {id: 'replace-bob'});
                MATCH (a:Person {id: 'replace-ada'}), (b:Person {id: 'replace-bob'})
                CREATE (a)-[e:KNOWS {id: 'replace-knows', source: 'Conference'}]->(b)
                RETURN replace(a.name, 'Ada', 'Augusta') AS renamed,
                       replace(e.source, $search, $replacement) AS source,
                       replace(a.nickname, 'x', 'y') AS missing_name;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        ("search".to_string(), Value::from("ference")),
                        ("replacement".to_string(), Value::from("gress")),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete replace projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::from("Augusta Lovelace"),
                Value::from("Congress"),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'replace-cara', status: 'replace', team: 'eng-team'});
                CREATE (:Person {id: 'replace-dan', status: 'replace', team: 'ops-team'});
                MATCH (n:Person {status: 'replace'}) SET n.seen = true
                RETURN n.id AS id, replace(n.team, '-team', '') AS team
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad replace projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("replace-cara"), Value::from("eng")],
                vec![Value::from("replace-dan"), Value::from("ops")],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'replace-team'});
                MATCH (n:Person {status: 'replace'}), (t:Team {id: 'replace-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'ReplaceSlice'}]->(t)
                RETURN n.id AS id, replace(r.source, 'Replace', 'Row') AS source
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship replace projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("replace-cara"), Value::from("RowSlice")],
                vec![Value::from("replace-dan"), Value::from("RowSlice")],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'replace'}) SET n.replace_counted = true
                RETURN count(replace(n.team, '-team', '')) AS rows,
                       count(DISTINCT replace(n.team, '-team', '')) AS distinct_teams,
                       collect(replace(n.team, '-team', '')) AS teams;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("replace aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!(["eng", "ops"])),
            ]]
        );

        let numeric_replace =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'replace-number', score: 3}) RETURN replace(n.score, '3', '4');",
                CypherMutationOptions::default(),
            ))
            .expect_err("replace over numeric values should stay rejected");
        assert!(
            matches!(numeric_replace, GrustError::CypherUnsupportedCardinality(_)),
            "{numeric_replace:?}"
        );

        let non_string_search =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'replace-search', name: 'Ada'}) RETURN replace(n.name, 1, 'A');",
                CypherMutationOptions::default(),
            ))
            .expect_err("replace search argument should stay string-only");
        assert!(
            matches!(
                non_string_search,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{non_string_search:?}"
        );

        let nested_replace =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'replace-nested', name: 'Ada'})
                RETURN replace(toLower(n.name), 'a', 'A') AS rewritten,
                       replace(coalesce(n.nickname, 'Fallback'), 'Fall', 'Call') AS fallback,
                       replace('static', 'sta', 'plas') AS literal_rewrite;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested replace arguments should execute");
        assert_eq!(
            nested_replace.table.rows,
            vec![vec![
                Value::from("AdA"),
                Value::from("Callback"),
                Value::from("plastic"),
            ]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'replace'}) SET n.nested_replace_counted = true
                RETURN count(replace(toLower(n.team), '-team', '')) AS rows,
                       collect(replace(toLower(n.team), '-team', '')) AS teams;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested replace aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Json(serde_json::json!(["eng", "ops"])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_string_predicates_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'predicate-ada', name: 'Ada Lovelace'});
                CREATE (b:Person {id: 'predicate-bob'});
                MATCH (a:Person {id: 'predicate-ada'}), (b:Person {id: 'predicate-bob'})
                CREATE (a)-[e:KNOWS {id: 'predicate-knows', source: 'Conference'}]->(b)
                RETURN startsWith(a.name, 'Ada') AS starts,
                       endsWith(a.name, $suffix) AS ends,
                       contains(e.source, 'fer') AS contains_source,
                       contains(a.nickname, 'x') AS missing_name;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "suffix".to_string(),
                        Value::from("lace"),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete string predicate projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(true),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'predicate-cara', status: 'predicate', team: 'engineering'});
                CREATE (:Person {id: 'predicate-dan', status: 'predicate', team: 'operations'});
                MATCH (n:Person {status: 'predicate'}) SET n.seen = true
                RETURN n.id AS id, startsWith(n.team, 'eng') AS engineering
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad string predicate projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("predicate-cara"), Value::Bool(true)],
                vec![Value::from("predicate-dan"), Value::Bool(false)],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'predicate-team'});
                MATCH (n:Person {status: 'predicate'}), (t:Team {id: 'predicate-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'PredicateSlice'}]->(t)
                RETURN n.id AS id, endsWith(r.source, 'Slice') AS source
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship string predicate projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("predicate-cara"), Value::Bool(true)],
                vec![Value::from("predicate-dan"), Value::Bool(true)],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'predicate'}) SET n.predicate_counted = true
                RETURN count(startsWith(n.team, 'eng')) AS rows,
                       count(DISTINCT startsWith(n.team, 'eng')) AS distinct_states,
                       collect(startsWith(n.team, 'eng')) AS states;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("string predicate aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!([true, false])),
            ]]
        );

        let numeric_predicate =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'predicate-number', score: 3}) RETURN contains(n.score, '3');",
                CypherMutationOptions::default(),
            ))
            .expect_err("string predicates over numeric values should stay rejected");
        assert!(
            matches!(
                numeric_predicate,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{numeric_predicate:?}"
        );

        let non_string_needle =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'predicate-needle', name: 'Ada'}) RETURN contains(n.name, 1);",
                CypherMutationOptions::default(),
            ))
            .expect_err("string predicate needle should stay string-only");
        assert!(
            matches!(
                non_string_needle,
                GrustError::CypherUnsupportedCardinality(_)
            ),
            "{non_string_needle:?}"
        );

        let nested_predicate =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'predicate-nested', name: 'Ada'})
                RETURN contains(toLower(n.name), 'a') AS contains_a,
                       startsWith(coalesce(n.nickname, 'Fallback'), 'Fall') AS fallback_starts,
                       endsWith('static', 'tic') AS literal_ends;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested string predicate arguments should execute");
        assert_eq!(
            nested_predicate.table.rows,
            vec![vec![
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(true),
            ]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'predicate'}) SET n.nested_predicate_counted = true
                RETURN count(startsWith(toLower(n.team), 'eng')) AS rows,
                       collect(contains(toUpper(n.team), 'A')) AS states;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested string predicate aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Json(serde_json::json!([false, true])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_string_slices_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'slice-ada', name: 'Ada Lovelace'});
                CREATE (b:Person {id: 'slice-bob'});
                MATCH (a:Person {id: 'slice-ada'}), (b:Person {id: 'slice-bob'})
                CREATE (a)-[e:KNOWS {id: 'slice-knows', source: 'Conference'}]->(b)
                RETURN left(a.name, 3) AS first,
                       right(a.name, 8) AS last,
                       left(e.source, $len) AS source_prefix,
                       right(a.nickname, 2) AS missing_name;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([("len".to_string(), Value::Int(4))]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete string slice projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::from("Ada"),
                Value::from("Lovelace"),
                Value::from("Conf"),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'slice-cara', status: 'slice', team: 'engineering'});
                CREATE (:Person {id: 'slice-dan', status: 'slice', team: 'operations'});
                MATCH (n:Person {status: 'slice'}) SET n.seen = true
                RETURN n.id AS id, left(n.team, 3) AS team
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad string slice projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("slice-cara"), Value::from("eng")],
                vec![Value::from("slice-dan"), Value::from("ope")],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'slice-team'});
                MATCH (n:Person {status: 'slice'}), (t:Team {id: 'slice-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'SliceSource'}]->(t)
                RETURN n.id AS id, right(r.source, 6) AS source
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship string slice projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("slice-cara"), Value::from("Source")],
                vec![Value::from("slice-dan"), Value::from("Source")],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'slice'}) SET n.slice_counted = true
                RETURN count(left(n.team, 3)) AS rows,
                       count(DISTINCT left(n.team, 3)) AS distinct_teams,
                       collect(left(n.team, 3)) AS teams;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("string slice aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!(["eng", "ope"])),
            ]]
        );

        let numeric_slice =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'slice-number', score: 3}) RETURN left(n.score, 1);",
                CypherMutationOptions::default(),
            ))
            .expect_err("string slices over numeric values should stay rejected");
        assert!(
            matches!(numeric_slice, GrustError::CypherUnsupportedCardinality(_)),
            "{numeric_slice:?}"
        );

        let negative_length =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'slice-negative', name: 'Ada'}) RETURN left(n.name, -1);",
                CypherMutationOptions::default(),
            ))
            .expect_err("string slice length should stay non-negative");
        assert!(
            matches!(negative_length, GrustError::CypherUnsupportedCardinality(_)),
            "{negative_length:?}"
        );

        let nested_slice =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'slice-nested', name: 'Ada'})
                RETURN left(toLower(n.name), 1) AS lowered_initial,
                       right(coalesce(n.nickname, 'Fallback'), 4) AS fallback_suffix,
                       left('static', 3) AS literal_prefix;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested string slice arguments should execute");
        assert_eq!(
            nested_slice.table.rows,
            vec![vec![
                Value::from("a"),
                Value::from("back"),
                Value::from("sta"),
            ]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'slice'}) SET n.nested_slice_counted = true
                RETURN count(left(toLower(n.team), 3)) AS rows,
                       collect(right(toUpper(n.team), 3)) AS suffixes;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested string slice aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Json(serde_json::json!(["ING", "ONS"])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_string_reverse_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'reverse-ada', name: 'Ada Lovelace', tags: $tags, scores: $scores});
                CREATE (b:Person {id: 'reverse-bob'});
                MATCH (a:Person {id: 'reverse-ada'}), (b:Person {id: 'reverse-bob'})
                CREATE (a)-[e:KNOWS {id: 'reverse-knows', source: 'Conference', weights: $weights}]->(b)
                RETURN reverse(a.name) AS reversed_name,
                       reverse(e.source) AS reversed_source,
                       reverse(a.tags) AS reversed_tags,
                       reverse(a.scores) AS reversed_scores,
                       reverse(e.weights) AS reversed_weights,
                       reverse(a.nickname) AS missing_name;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        (
                            "tags".to_string(),
                            Value::StringArray(vec!["engineer".to_string(), "speaker".to_string()]),
                        ),
                        ("scores".to_string(), Value::IntArray(vec![7, 11])),
                        ("weights".to_string(), Value::FloatArray(vec![2.5, 4.5])),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete string and array reverse projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::from("ecalevoL adA"),
                Value::from("ecnerefnoC"),
                Value::StringArray(vec!["speaker".to_string(), "engineer".to_string()]),
                Value::IntArray(vec![11, 7]),
                Value::FloatArray(vec![4.5, 2.5]),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'reverse-cara', status: 'reverse', team: 'engineering'});
                CREATE (:Person {id: 'reverse-dan', status: 'reverse', team: 'operations'});
                MATCH (n:Person {status: 'reverse'}) SET n.seen = true
                RETURN n.id AS id, reverse(n.team) AS team
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad string reverse projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("reverse-cara"), Value::from("gnireenigne")],
                vec![Value::from("reverse-dan"), Value::from("snoitarepo")],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'reverse-team'});
                MATCH (n:Person {status: 'reverse'}), (t:Team {id: 'reverse-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'ReverseSource'}]->(t)
                RETURN n.id AS id, reverse(r.source) AS source
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship string reverse projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![Value::from("reverse-cara"), Value::from("ecruoSesreveR")],
                vec![Value::from("reverse-dan"), Value::from("ecruoSesreveR")],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'reverse'}) SET n.reverse_counted = true
                RETURN count(reverse(n.team)) AS rows,
                       count(DISTINCT reverse(n.team)) AS distinct_teams,
                       collect(reverse(n.team)) AS teams;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("string reverse aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!(["gnireenigne", "snoitarepo"])),
            ]]
        );

        let numeric_reverse =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'reverse-number', score: 3}) RETURN reverse(n.score);",
                CypherMutationOptions::default(),
            ))
            .expect_err("reverse over numeric values should stay rejected");
        assert!(
            matches!(numeric_reverse, GrustError::CypherUnsupportedCardinality(_)),
            "{numeric_reverse:?}"
        );

        let nested_reverse =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'reverse-nested', name: 'Ada'})
                RETURN reverse(toLower(n.name)) AS reversed_lower,
                       reverse(coalesce(n.nickname, n.name, 'unknown')) AS display,
                       reverse(range(1, 3)) AS reversed_range,
                       reverse('static') AS literal_reverse;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested string reverse arguments should execute");
        assert_eq!(
            nested_reverse.table.rows,
            vec![vec![
                Value::from("ada"),
                Value::from("adA"),
                Value::IntArray(vec![3, 2, 1]),
                Value::from("citats"),
            ]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'reverse'}) SET n.nested_reverse_counted = true
                RETURN count(reverse(toLower(n.team))) AS rows,
                       collect(reverse(toUpper(n.team))) AS teams;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested string reverse aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Json(serde_json::json!(["GNIREENIGNE", "SNOITAREPO"])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_string_split_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'split-ada', path: 'people/ada/lovelace'});
                CREATE (b:Person {id: 'split-bob'});
                MATCH (a:Person {id: 'split-ada'}), (b:Person {id: 'split-bob'})
                CREATE (a)-[e:KNOWS {id: 'split-knows', source: 'Conference:Talk'}]->(b)
                RETURN split(a.path, '/') AS path_parts,
                       split(e.source, $delimiter) AS source_parts,
                       split(a.nickname, '/') AS missing_name;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([(
                        "delimiter".to_string(),
                        Value::from(":"),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("concrete string split projections");
        assert_eq!(
            concrete.table.rows,
            vec![vec![
                Value::Json(serde_json::json!(["people", "ada", "lovelace"])),
                Value::Json(serde_json::json!(["Conference", "Talk"])),
                Value::Null,
            ]]
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'split-cara', status: 'split', team: 'engineering/platform'});
                CREATE (:Person {id: 'split-dan', status: 'split', team: 'operations/support'});
                MATCH (n:Person {status: 'split'}) SET n.seen = true
                RETURN n.id AS id, split(n.team, '/') AS team
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad string split projections");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![
                    Value::from("split-cara"),
                    Value::Json(serde_json::json!(["engineering", "platform"])),
                ],
                vec![
                    Value::from("split-dan"),
                    Value::Json(serde_json::json!(["operations", "support"])),
                ],
            ]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'split-team'});
                MATCH (n:Person {status: 'split'}), (t:Team {id: 'split-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'Split|Source'}]->(t)
                RETURN n.id AS id, split(r.source, '|') AS source
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing relationship string split projections");
        assert_eq!(
            row_edges.table.rows,
            vec![
                vec![
                    Value::from("split-cara"),
                    Value::Json(serde_json::json!(["Split", "Source"])),
                ],
                vec![
                    Value::from("split-dan"),
                    Value::Json(serde_json::json!(["Split", "Source"])),
                ],
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'split'}) SET n.split_counted = true
                RETURN count(split(n.team, '/')) AS rows,
                       count(DISTINCT split(n.team, '/')) AS distinct_teams,
                       collect(split(n.team, '/')) AS teams;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("string split aggregate projections");
        assert_eq!(
            aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Int(2),
                Value::Json(serde_json::json!([
                    ["engineering", "platform"],
                    ["operations", "support"]
                ])),
            ]]
        );

        let numeric_split =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'split-number', score: 3}) RETURN split(n.score, '/');",
                CypherMutationOptions::default(),
            ))
            .expect_err("string split over numeric values should stay rejected");
        assert!(
            matches!(numeric_split, GrustError::CypherUnsupportedCardinality(_)),
            "{numeric_split:?}"
        );

        let empty_delimiter =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'split-empty', path: 'abc'}) RETURN split(n.path, '');",
                CypherMutationOptions::default(),
            ))
            .expect_err("string split delimiter should stay non-empty");
        assert!(
            matches!(empty_delimiter, GrustError::CypherUnsupportedCardinality(_)),
            "{empty_delimiter:?}"
        );

        let nested_split =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'split-nested', path: 'A/B'})
                RETURN split(toLower(n.path), '/') AS lowered_parts,
                       split(coalesce(n.nickname, 'fallback/name'), '/') AS fallback_parts,
                       split('static/value', '/') AS literal_parts;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested string split arguments should execute");
        assert_eq!(
            nested_split.table.rows,
            vec![vec![
                Value::Json(serde_json::json!(["a", "b"])),
                Value::Json(serde_json::json!(["fallback", "name"])),
                Value::Json(serde_json::json!(["static", "value"])),
            ]]
        );

        let nested_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'split'}) SET n.nested_split_counted = true
                RETURN count(split(toLower(n.team), '/')) AS rows,
                       collect(split(toLower(n.team), '/')) AS teams;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested string split aggregate projections should execute");
        assert_eq!(
            nested_aggregates.table.rows,
            vec![vec![
                Value::Int(2),
                Value::Json(serde_json::json!([
                    ["engineering", "platform"],
                    ["operations", "support"]
                ])),
            ]]
        );
    }

    #[test]
    fn cypher_returning_projects_restricted_case_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'case-ada', team: 'eng'})
                RETURN CASE WHEN n.team = 'eng' THEN 'engineering' ELSE 'other' END AS group;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("concrete CASE projection");
        assert_eq!(
            concrete.table,
            CypherResultTable {
                columns: vec!["group".to_string()],
                rows: vec![vec![Value::from("engineering")]],
            }
        );

        let broad =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'case-bob', status: 'case', team: 'eng'});
                CREATE (:Person {id: 'case-cara', status: 'case', team: 'ops'});
                MATCH (n:Person {status: 'case'}) SET n.seen = true
                RETURN n.id AS id,
                       CASE WHEN n.team = 'eng' THEN 'engineering' ELSE 'other' END AS group
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad row CASE projection");
        assert_eq!(
            broad.table.rows,
            vec![
                vec![Value::from("case-bob"), Value::from("engineering")],
                vec![Value::from("case-cara"), Value::from("other")]
            ]
        );

        let row_edge =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'case-team'});
                MATCH (n:Person {status: 'case'}), (t:Team {id: 'case-team'})
                CREATE (n)-[r:MEMBER_OF {source: 'case'}]->(t)
                RETURN n.id AS id,
                       CASE WHEN r.source = 'case' THEN 'matched' ELSE 'missed' END AS edge_case,
                       CASE WHEN t.id = 'case-team' THEN true ELSE false END AS endpoint_case
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("row-producing CASE projection");
        assert_eq!(
            row_edge.table.rows,
            vec![
                vec![
                    Value::from("case-bob"),
                    Value::from("matched"),
                    Value::Bool(true)
                ],
                vec![
                    Value::from("case-cara"),
                    Value::from("matched"),
                    Value::Bool(true)
                ],
            ]
        );

        let grouped =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'case'}) SET n.counted = true
                RETURN CASE WHEN n.team = 'eng' THEN 'engineering' ELSE 'other' END AS group,
                       count(*) AS people
                ORDER BY group;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("grouped CASE projection");
        assert_eq!(
            grouped.table.rows,
            vec![
                vec![Value::from("engineering"), Value::Int(1)],
                vec![Value::from("other"), Value::Int(1)]
            ]
        );

        let parameterized =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'case'}) SET n.parameterized = true
                RETURN n.id AS id,
                       CASE WHEN n.team = $team THEN $matched ELSE $unmatched END AS group
                ORDER BY id;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        ("team".to_string(), Value::from("eng")),
                        ("matched".to_string(), Value::from("engineering")),
                        ("unmatched".to_string(), Value::from("other")),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("parameterized CASE projection");
        assert_eq!(
            parameterized.table.rows,
            vec![
                vec![Value::from("case-bob"), Value::from("engineering")],
                vec![Value::from("case-cara"), Value::from("other")]
            ]
        );

        let nested =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'case'}) SET n.nested_case = true
                RETURN n.id AS id,
                       CASE WHEN n.team = 'eng' THEN toUpper(n.id) ELSE coalesce(n.nickname, 'other') END AS group
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested restricted CASE branch projections");
        assert_eq!(
            nested.table.rows,
            vec![
                vec![Value::from("case-bob"), Value::from("CASE-BOB")],
                vec![Value::from("case-cara"), Value::from("other")]
            ]
        );

        let aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'case'}) SET n.aggregated = true
                RETURN count(CASE WHEN n.team = 'eng' THEN 1 ELSE null END) AS eng_count,
                       count(DISTINCT CASE WHEN n.team = 'eng' THEN 'eng' ELSE null END) AS eng_teams,
                       sum(CASE WHEN n.team = 'eng' THEN 1 ELSE 0 END) AS eng_sum,
                       avg(CASE WHEN n.team = 'eng' THEN 10 ELSE 2 END) AS score_avg,
                       min(CASE WHEN n.team = 'eng' THEN 'a' ELSE 'z' END) AS first_bucket,
                       max(CASE WHEN n.team = 'eng' THEN 'a' ELSE 'z' END) AS last_bucket,
                       collect(CASE WHEN n.team = 'eng' THEN 'eng' ELSE null END) AS eng_ids;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted CASE aggregate projections");
        assert_eq!(
            aggregates.table.columns,
            vec![
                "eng_count".to_string(),
                "eng_teams".to_string(),
                "eng_sum".to_string(),
                "score_avg".to_string(),
                "first_bucket".to_string(),
                "last_bucket".to_string(),
                "eng_ids".to_string(),
            ]
        );
        assert_eq!(aggregates.table.rows[0][0], Value::Int(1));
        assert_eq!(aggregates.table.rows[0][1], Value::Int(1));
        assert_eq!(aggregates.table.rows[0][2], Value::Int(1));
        assert_eq!(aggregates.table.rows[0][3], Value::Float(6.0));
        assert_eq!(aggregates.table.rows[0][4], Value::from("a"));
        assert_eq!(aggregates.table.rows[0][5], Value::from("z"));
        assert_eq!(
            aggregates.table.rows[0][6],
            Value::Json(serde_json::json!(["eng"]))
        );

        let grouped_aggregates =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'case'}) SET n.group_aggregated = true
                RETURN CASE WHEN n.team = 'eng' THEN 'engineering' ELSE 'other' END AS group,
                       sum(CASE WHEN n.team = 'eng' THEN 1 ELSE 0 END) AS eng_sum,
                       collect(CASE WHEN n.team = 'eng' THEN 'eng' ELSE null END) AS eng_ids
                ORDER BY group;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("grouped restricted CASE aggregate projections");
        assert_eq!(
            grouped_aggregates.table.rows,
            vec![
                vec![
                    Value::from("engineering"),
                    Value::Int(1),
                    Value::Json(serde_json::json!(["eng"]))
                ],
                vec![
                    Value::from("other"),
                    Value::Int(0),
                    Value::Json(serde_json::json!([]))
                ]
            ]
        );

        let parameterized_aggregate =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'case'}) SET n.parameterized_aggregate = true
                RETURN sum(CASE WHEN n.team = $team THEN $matched ELSE $unmatched END) AS score;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        ("team".to_string(), Value::from("eng")),
                        ("matched".to_string(), Value::Int(3)),
                        ("unmatched".to_string(), Value::Int(1)),
                    ]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("parameterized CASE aggregate projection");
        assert_eq!(
            parameterized_aggregate.table.rows,
            vec![vec![Value::Int(4)]]
        );

        let nested_aggregate =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'case'}) SET n.nested_case_aggregate = true
                RETURN collect(CASE WHEN n.team = 'eng' THEN toUpper(n.id) ELSE coalesce(n.nickname, 'other') END) AS buckets;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested restricted CASE aggregate projections");
        assert_eq!(
            nested_aggregate.table.rows,
            vec![vec![Value::Json(serde_json::json!(["CASE-BOB", "other"]))]]
        );

        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "MATCH (n:Person {status: 'case'}) SET n.flag = true
                 RETURN sum(CASE WHEN lower(n.team) = 'eng' THEN 1 ELSE 0 END);",
                CypherMutationOptions::default(),
            ))
            .expect_err("aggregate CASE functions should stay rejected");
        assert!(
            matches!(error, GrustError::CypherUnsupportedCardinality(_)),
            "{error:?}"
        );

        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "MATCH (n:Person {status: 'case'}) SET n.flag = true
                 RETURN CASE WHEN n.team = $missing THEN 'match' ELSE 'miss' END;",
                CypherMutationOptions::default(),
            ))
            .expect_err("missing CASE parameter should be rejected");
        assert!(
            matches!(error, GrustError::CypherUnresolvedIdentity(_)),
            "{error:?}"
        );

        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "MATCH (n:Person {status: 'case'}) SET n.flag = true
                 RETURN CASE WHEN n.team > 'eng' THEN 'other' ELSE 'engineering' END;",
                CypherMutationOptions::default(),
            ))
            .expect_err("unsupported CASE predicate operator should be rejected");
        assert!(
            matches!(error, GrustError::CypherUnsupportedCardinality(_)),
            "{error:?}"
        );

        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "MATCH (n:Person {status: 'case'}) SET n.flag = true
                 RETURN CASE WHEN n.team = 'eng' THEN [n.id] ELSE 'other' END;",
                CypherMutationOptions::default(),
            ))
            .expect_err("CASE branches should still reject nested composites");
        assert!(
            matches!(
                error,
                GrustError::CypherUnsupportedCardinality(_) | GrustError::CypherSyntax(_)
            ),
            "{error:?}"
        );

        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'case'}) SET n.flag = true
                RETURN CASE WHEN n.team = 'eng' THEN m.id ELSE 'other' END;
                ",
                CypherMutationOptions::default(),
            ))
            .expect_err("CASE branches should reject cross-variable values");
        assert!(
            matches!(error, GrustError::CypherUnsupportedCardinality(_)),
            "{error:?}"
        );
    }

    #[test]
    fn cypher_returning_projects_broad_node_rows_on_memory_facade() {
        let store = MemoryGraphStore::new();

        futures_executor::block_on(store.put_graph(&Graph::new(
            vec![
                Node::new(
                    "Person",
                    "ada",
                    Props::from([
                        ("name".to_string(), Value::from("Ada")),
                        ("status".to_string(), Value::from("active")),
                        ("nickname".to_string(), Value::from("ada")),
                    ]),
                ),
                Node::new(
                    "Person",
                    "bob",
                    Props::from([
                        ("name".to_string(), Value::from("Bob")),
                        ("status".to_string(), Value::from("active")),
                        ("nickname".to_string(), Value::from("bob")),
                    ]),
                ),
                Node::new(
                    "Person",
                    "eve",
                    Props::from([
                        ("name".to_string(), Value::from("Eve")),
                        ("status".to_string(), Value::from("inactive")),
                    ]),
                ),
            ],
            vec![],
        )))
        .unwrap();

        let set_result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'active'})
                SET n.seen = true
                RETURN n.id, n.name, n.seen, n.label;
                ",
                CypherMutationOptions::default(),
            ))
            .unwrap();

        assert_eq!(
            set_result.mutation.report,
            GraphMutationReport {
                patches: 1,
                matched_rows: 2,
                changed_nodes: 2,
                node_patches: 2,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            set_result.table,
            CypherResultTable {
                columns: vec![
                    "n.id".to_string(),
                    "n.name".to_string(),
                    "n.seen".to_string(),
                    "n.label".to_string()
                ],
                rows: vec![
                    vec![
                        Value::from("ada"),
                        Value::from("Ada"),
                        Value::Bool(true),
                        Value::from("Person")
                    ],
                    vec![
                        Value::from("bob"),
                        Value::from("Bob"),
                        Value::Bool(true),
                        Value::from("Person")
                    ],
                ],
            }
        );

        let remove_result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'active'})
                REMOVE n.nickname
                RETURN n.id, n.nickname;
                ",
                CypherMutationOptions::default(),
            ))
            .unwrap();

        assert_eq!(
            remove_result.table,
            CypherResultTable {
                columns: vec!["n.id".to_string(), "n.nickname".to_string()],
                rows: vec![
                    vec![Value::from("ada"), Value::Null],
                    vec![Value::from("bob"), Value::Null],
                ],
            }
        );

        let ordered_store = MemoryGraphStore::new();
        futures_executor::block_on(ordered_store.put_node(&Node::new(
            "Person",
            "grace",
            Props::from([("status".to_string(), Value::from("inactive"))]),
        )))
        .unwrap();
        let ordered_result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &ordered_store,
                "
                MATCH (m:Person {status: 'inactive'})
                SET m.status = 'active';
                MATCH (n:Person {status: 'active'})
                SET n.seen = true
                RETURN n.id, n.status, n.seen;
                ",
                CypherMutationOptions::default(),
            ))
            .unwrap();

        assert_eq!(
            ordered_result.table,
            CypherResultTable {
                columns: vec![
                    "n.id".to_string(),
                    "n.status".to_string(),
                    "n.seen".to_string()
                ],
                rows: vec![vec![
                    Value::from("grace"),
                    Value::from("active"),
                    Value::Bool(true)
                ]],
            }
        );
    }

    #[test]
    fn cypher_returning_projects_deleted_broad_node_rows_on_memory_facade() {
        let store = MemoryGraphStore::new();

        futures_executor::block_on(store.put_graph(&Graph::new(
            vec![
                Node::new(
                    "Person",
                    "ada",
                    Props::from([
                        ("name".to_string(), Value::from("Ada")),
                        ("status".to_string(), Value::from("inactive")),
                    ]),
                ),
                Node::new(
                    "Person",
                    "bob",
                    Props::from([
                        ("name".to_string(), Value::from("Bob")),
                        ("status".to_string(), Value::from("inactive")),
                    ]),
                ),
                Node::new(
                    "Person",
                    "cara",
                    Props::from([("status".to_string(), Value::from("active"))]),
                ),
            ],
            vec![],
        )))
        .unwrap();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'inactive'})
                DELETE n
                RETURN n.id, n.name ORDER BY n.id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad node delete can return deleted matched rows");

        assert_eq!(
            result.mutation.report,
            GraphMutationReport {
                deletes: 1,
                matched_rows: 2,
                changed_nodes: 2,
                node_deletes: 2,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            result.table,
            CypherResultTable {
                columns: vec!["n.id".to_string(), "n.name".to_string()],
                rows: vec![
                    vec![Value::from("ada"), Value::from("Ada")],
                    vec![Value::from("bob"), Value::from("Bob")],
                ],
            }
        );
        assert!(
            futures_executor::block_on(store.get_node(&NodeId::new("ada")))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn cypher_returning_projects_broad_edge_rows_on_memory_facade() {
        let store = MemoryGraphStore::new();

        futures_executor::block_on(store.put_graph(&Graph::new(
            vec![
                Node::new(
                    "Person",
                    "ada",
                    Props::from([("status".to_string(), Value::from("active"))]),
                ),
                Node::new(
                    "Person",
                    "bob",
                    Props::from([("status".to_string(), Value::from("active"))]),
                ),
                Node::new(
                    "Person",
                    "eve",
                    Props::from([("status".to_string(), Value::from("inactive"))]),
                ),
            ],
            vec![
                Edge::new(
                    "KNOWS",
                    "ada",
                    "bob",
                    Props::from([("weight".to_string(), Value::Int(3))]),
                )
                .with_id("edge-1"),
                Edge::new(
                    "KNOWS",
                    "ada",
                    "eve",
                    Props::from([("weight".to_string(), Value::Int(7))]),
                )
                .with_id("edge-2"),
            ],
        )))
        .unwrap();

        let set_result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (a:Person {status: 'active'})-[e:KNOWS]->(b:Person {status: 'active'})
                SET e.seen = true
                RETURN e.id, e.label, e.weight, e.seen;
                ",
                CypherMutationOptions::default(),
            ))
            .unwrap();

        assert_eq!(
            set_result.mutation.report,
            GraphMutationReport {
                patches: 1,
                matched_rows: 1,
                changed_edges: 1,
                edge_patches: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            set_result.table,
            CypherResultTable {
                columns: vec![
                    "e.id".to_string(),
                    "e.label".to_string(),
                    "e.weight".to_string(),
                    "e.seen".to_string()
                ],
                rows: vec![vec![
                    Value::from("edge-1"),
                    Value::from("KNOWS"),
                    Value::Int(3),
                    Value::Bool(true)
                ]],
            }
        );

        let remove_result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (a:Person {status: 'active'})-[e:KNOWS]->(b:Person {status: 'active'})
                REMOVE e.weight
                RETURN e.id, e.weight;
                ",
                CypherMutationOptions::default(),
            ))
            .unwrap();

        assert_eq!(
            remove_result.table,
            CypherResultTable {
                columns: vec!["e.id".to_string(), "e.weight".to_string()],
                rows: vec![vec![Value::from("edge-1"), Value::Null]],
            }
        );

        let ordered_store = MemoryGraphStore::new();
        futures_executor::block_on(ordered_store.put_graph(&Graph::new(
            vec![
                Node::new("Person", "ada", Props::new()),
                Node::new("Person", "bob", Props::new()),
            ],
            vec![
                Edge::new(
                    "KNOWS",
                    "ada",
                    "bob",
                    Props::from([("status".to_string(), Value::from("inactive"))]),
                )
                .with_id("edge-3"),
            ],
        )))
        .unwrap();
        let ordered_result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &ordered_store,
                "
                MATCH (:Person {id: 'ada'})-[f:KNOWS {status: 'inactive'}]->(:Person {id: 'bob'})
                SET f.status = 'active';
                MATCH (:Person {id: 'ada'})-[e:KNOWS {status: 'active'}]->(:Person {id: 'bob'})
                SET e.seen = true
                RETURN e.id, e.status, e.seen;
                ",
                CypherMutationOptions::default(),
            ))
            .unwrap();

        assert_eq!(
            ordered_result.table,
            CypherResultTable {
                columns: vec![
                    "e.id".to_string(),
                    "e.status".to_string(),
                    "e.seen".to_string()
                ],
                rows: vec![vec![
                    Value::from("edge-3"),
                    Value::from("active"),
                    Value::Bool(true)
                ]],
            }
        );
    }

    #[test]
    fn cypher_returning_projects_deleted_broad_edge_rows_on_memory_facade() {
        let store = MemoryGraphStore::new();

        futures_executor::block_on(store.put_graph(&Graph::new(
            vec![
                Node::new(
                    "Person",
                    "ada",
                    Props::from([("status".to_string(), Value::from("active"))]),
                ),
                Node::new(
                    "Person",
                    "bob",
                    Props::from([("status".to_string(), Value::from("active"))]),
                ),
                Node::new(
                    "Person",
                    "eve",
                    Props::from([("status".to_string(), Value::from("inactive"))]),
                ),
            ],
            vec![
                Edge::new(
                    "KNOWS",
                    "ada",
                    "bob",
                    Props::from([("weight".to_string(), Value::Int(3))]),
                )
                .with_id("edge-1"),
                Edge::new(
                    "KNOWS",
                    "ada",
                    "eve",
                    Props::from([("weight".to_string(), Value::Int(7))]),
                )
                .with_id("edge-2"),
            ],
        )))
        .unwrap();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (a:Person {status: 'active'})-[e:KNOWS]->(b:Person {status: 'active'})
                DELETE e
                RETURN e.id, e.label, e.weight;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("broad edge delete can return deleted matched rows");

        assert_eq!(
            result.mutation.report,
            GraphMutationReport {
                deletes: 1,
                matched_rows: 1,
                changed_edges: 1,
                edge_deletes: 1,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            result.table,
            CypherResultTable {
                columns: vec![
                    "e.id".to_string(),
                    "e.label".to_string(),
                    "e.weight".to_string()
                ],
                rows: vec![vec![
                    Value::from("edge-1"),
                    Value::from("KNOWS"),
                    Value::Int(3)
                ]],
            }
        );
        assert_eq!(
            futures_executor::block_on(store.get_edges(EdgeQuery::default()))
                .unwrap()
                .into_iter()
                .map(|edge| edge.id.map(|id| id.as_str().to_string()))
                .collect::<Vec<_>>(),
            vec![Some("edge-2".to_string())]
        );
    }

    #[test]
    fn cypher_returning_evaluates_row_produced_edge_values() {
        let planned = sail_cypher_mutation_plan_with_return_options(
            "
            MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
            CREATE (a)-[e:MEMBER_OF {source: 'cypher'}]->(b)
            RETURN e.label, e.source, e.id;
            ",
            CypherMutationOptions::default(),
        )
        .unwrap();
        let mut row_edge_values = HashMap::new();
        row_edge_values.insert(
            "e".to_string(),
            vec![
                Edge::new(
                    "MEMBER_OF",
                    "ada",
                    "eng",
                    Props::from([("source".to_string(), Value::from("cypher"))]),
                ),
                Edge::new(
                    "MEMBER_OF",
                    "bob",
                    "eng",
                    Props::from([("source".to_string(), Value::from("cypher"))]),
                ),
            ],
        );

        let table = futures_executor::block_on(evaluate_cypher_return_table(
            &MemoryGraphStore::new(),
            &planned.node_bindings,
            &planned.edge_bindings,
            &HashMap::new(),
            &row_edge_values,
            &planned.row_path_bindings,
            &planned.return_clause,
        ))
        .unwrap();

        assert_eq!(
            table,
            CypherResultTable {
                columns: vec![
                    "e.label".to_string(),
                    "e.source".to_string(),
                    "e.id".to_string()
                ],
                rows: vec![
                    vec![Value::from("MEMBER_OF"), Value::from("cypher"), Value::Null],
                    vec![Value::from("MEMBER_OF"), Value::from("cypher"), Value::Null]
                ],
            }
        );
    }

    #[test]
    fn cypher_returning_allows_control_words_as_aliases() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'ada', name: 'Ada'})
                RETURN n.id AS limit, n.name AS skip;
                ",
                CypherMutationOptions::default(),
            ))
            .unwrap();

        assert_eq!(
            result.table,
            CypherResultTable {
                columns: vec!["limit".to_string(), "skip".to_string()],
                rows: vec![vec![Value::from("ada"), Value::from("Ada")]],
            }
        );
    }

    #[test]
    fn cypher_returning_generic_strict_create_checks_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'ada', name: 'Ada'}) RETURN n.id, n.name;",
                CypherMutationOptions {
                    create_mode: CypherCreateMode::ErrorIfExists,
                    ..CypherMutationOptions::default()
                },
            ))
            .unwrap();
        assert_eq!(
            result.table,
            CypherResultTable {
                columns: vec!["n.id".to_string(), "n.name".to_string()],
                rows: vec![vec![Value::from("ada"), Value::from("Ada")]],
            }
        );

        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'ada', name: 'Ada again'}) RETURN n.id;",
                CypherMutationOptions {
                    create_mode: CypherCreateMode::ErrorIfExists,
                    ..CypherMutationOptions::default()
                },
            ))
            .expect_err("strict CREATE should reject existing node");
        assert!(matches!(error, GrustError::CypherExecution(_)));
        assert!(error.to_string().contains("would overwrite existing node"));

        let fresh = MemoryGraphStore::new();
        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &fresh,
                "
                CREATE (n:Person {id: 'ada', name: 'Ada'});
                CREATE (n:Person {id: 'ada', name: 'Ada again'})
                RETURN n.id;
                ",
                CypherMutationOptions {
                    create_mode: CypherCreateMode::ErrorIfExists,
                    ..CypherMutationOptions::default()
                },
            ))
            .expect_err("strict CREATE should reject duplicate node target in the same batch");
        assert!(matches!(error, GrustError::CypherExecution(_)));
        assert!(error.to_string().contains("duplicate node 'ada'"));
        assert!(
            futures_executor::block_on(fresh.get_node(&NodeId::new("ada")))
                .unwrap()
                .is_none(),
            "failed strict preflight must not partially write the first CREATE"
        );

        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (b:Person {id: 'bob'});
            CREATE (a:Person {id: 'ada'})-[e:KNOWS {id: 'edge-1'}]->(b)
            RETURN e.id;
            ",
            CypherMutationOptions::default(),
        ))
        .unwrap();
        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'ada'})-[e:LIKES {id: 'edge-1'}]->(b:Person {id: 'bob'})
                RETURN e.id;
                ",
                CypherMutationOptions {
                    create_mode: CypherCreateMode::ErrorIfExists,
                    ..CypherMutationOptions::default()
                },
            ))
            .expect_err("strict CREATE should reject reused explicit edge id");
        assert!(matches!(error, GrustError::CypherExecution(_)));
        assert!(error.to_string().contains("would overwrite existing edge"));

        let fresh = MemoryGraphStore::new();
        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &fresh,
                "
                CREATE (a:Person {id: 'ada'});
                CREATE (b:Person {id: 'bob'});
                CREATE (a)-[:KNOWS {id: 'edge-1'}]->(b);
                CREATE (a)-[e:LIKES {id: 'edge-1'}]->(b)
                RETURN e.id;
                ",
                CypherMutationOptions {
                    create_mode: CypherCreateMode::ErrorIfExists,
                    ..CypherMutationOptions::default()
                },
            ))
            .expect_err("strict CREATE should reject duplicate edge id in the same batch");
        assert!(matches!(error, GrustError::CypherExecution(_)));
        assert!(error.to_string().contains("duplicate edge 'edge-1'"));
        assert!(
            futures_executor::block_on(fresh.get_edges(EdgeQuery::default()))
                .unwrap()
                .is_empty(),
            "failed strict preflight must not partially write earlier CREATE operations"
        );
    }

    #[test]
    fn cypher_returning_rejects_deferred_result_forms() {
        let store = MemoryGraphStore::new();

        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (:Person {id: 'ada'}) RETURN n.id;",
                CypherMutationOptions::default(),
            ))
            .expect_err("unbound variable should be rejected");
        assert!(matches!(error, GrustError::CypherUnresolvedIdentity(_)));

        // ORDER BY a column that was not returned is still rejected.
        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "MATCH (n:Person {id: 'ada'}) SET n.seen = true RETURN n.id ORDER BY n.missing;",
                CypherMutationOptions::default(),
            ))
            .expect_err("ORDER BY on a non-projected column should be rejected");
        assert!(matches!(error, GrustError::CypherUnsupportedCardinality(_)));
    }

    #[test]
    fn cypher_returning_classifies_materialization_targets() {
        assert_eq!(
            classify_return_target_materialization(&CypherReturnTarget::All),
            CypherReturnTargetMaterialization::Star
        );
        assert_eq!(
            classify_return_target_materialization(&CypherReturnTarget::Element),
            CypherReturnTargetMaterialization::Element
        );
        assert_eq!(
            classify_return_target_materialization(&CypherReturnTarget::Property("id".into())),
            CypherReturnTargetMaterialization::DirectProperty
        );
        assert_eq!(
            classify_return_target_materialization(&CypherReturnTarget::Literal(Value::Int(1))),
            CypherReturnTargetMaterialization::ScalarProjection
        );
        assert_eq!(
            classify_return_target_materialization(&CypherReturnTarget::ElementId),
            CypherReturnTargetMaterialization::ElementFunction
        );
        assert_eq!(
            classify_return_target_materialization(&CypherReturnTarget::PathLength),
            CypherReturnTargetMaterialization::PathFunction
        );
    }

    #[test]
    fn cypher_returning_classifies_scalar_projection_kinds() {
        assert_eq!(
            classify_return_scalar_projection(&CypherReturnTarget::All),
            CypherReturnScalarProjectionKind::Star
        );
        assert_eq!(
            classify_return_scalar_projection(&CypherReturnTarget::Element),
            CypherReturnScalarProjectionKind::Element
        );
        assert_eq!(
            classify_return_scalar_projection(&CypherReturnTarget::Property("id".into())),
            CypherReturnScalarProjectionKind::DirectProperty
        );
        assert_eq!(
            classify_return_scalar_projection(&CypherReturnTarget::Literal(Value::Bool(true))),
            CypherReturnScalarProjectionKind::Literal
        );
        assert_eq!(
            classify_return_scalar_projection(&CypherReturnTarget::MapProjection(
                CypherReturnMapProjection {
                    variable: "n".into(),
                    entries: vec![CypherReturnMapProjectionEntry {
                        output_key: "id".into(),
                        value: CypherReturnTarget::Property("id".into()),
                    }],
                },
            )),
            CypherReturnScalarProjectionKind::Map
        );
        assert_eq!(
            classify_return_scalar_projection(&CypherReturnTarget::ListProjection(
                CypherReturnListProjection {
                    variable: Some("n".into()),
                    terms: vec![CypherReturnTarget::Property("id".into())],
                },
            )),
            CypherReturnScalarProjectionKind::List
        );
        assert_eq!(
            classify_return_scalar_projection(&CypherReturnTarget::Case(CypherReturnCase {
                key: "status".into(),
                equals: Value::from("active"),
                then_target: Box::new(CypherReturnTarget::Literal(Value::Bool(true))),
                else_target: Box::new(CypherReturnTarget::Literal(Value::Bool(false))),
            })),
            CypherReturnScalarProjectionKind::Conditional
        );
        assert_eq!(
            classify_return_scalar_projection(&CypherReturnTarget::Coalesce(
                CypherReturnCoalesce {
                    variable: Some("n".into()),
                    terms: vec![CypherReturnTarget::Property("name".into())],
                },
            )),
            CypherReturnScalarProjectionKind::Coalesce
        );
        assert_eq!(
            classify_return_scalar_projection(&CypherReturnTarget::PropertyExists("name".into())),
            CypherReturnScalarProjectionKind::Introspection
        );
        assert_eq!(
            classify_return_scalar_projection(&CypherReturnTarget::PropertyListIndex(
                CypherReturnListIndexProjection {
                    key: "tags".into(),
                    index: CypherReturnListBound {
                        variable: None,
                        target: Box::new(CypherReturnTarget::Literal(Value::Int(0))),
                    },
                },
            )),
            CypherReturnScalarProjectionKind::ListAccess
        );
        assert_eq!(
            classify_return_scalar_projection(&CypherReturnTarget::PropertyListPredicate(
                CypherReturnListPredicateProjection {
                    key: "tags".into(),
                    predicate: CypherReturnListPredicate::Any,
                    item_variable: "tag".into(),
                    equals_variable: None,
                    equals: Box::new(CypherReturnTarget::Literal(Value::from("speaker"))),
                },
            )),
            CypherReturnScalarProjectionKind::ListPredicate
        );
        assert_eq!(
            classify_return_scalar_projection(&CypherReturnTarget::PropertyAbs(
                CypherReturnAbsProjection {
                    variable: Some("n".into()),
                    target: Box::new(CypherReturnTarget::Property("score".into())),
                },
            )),
            CypherReturnScalarProjectionKind::Numeric
        );
        assert_eq!(
            classify_return_scalar_projection(&CypherReturnTarget::PropertyToString(
                CypherReturnToStringProjection {
                    variable: Some("n".into()),
                    target: Box::new(CypherReturnTarget::Property("id".into())),
                },
            )),
            CypherReturnScalarProjectionKind::Conversion
        );
        assert_eq!(
            classify_return_scalar_projection(&CypherReturnTarget::PropertyStringSplit(
                CypherReturnStringSplit {
                    variable: Some("n".into()),
                    target: Box::new(CypherReturnTarget::Property("path".into())),
                    delimiter: "/".into(),
                },
            )),
            CypherReturnScalarProjectionKind::String
        );
        assert_eq!(
            classify_return_scalar_projection(&CypherReturnTarget::ElementId),
            CypherReturnScalarProjectionKind::ElementFunction
        );
        assert_eq!(
            classify_return_scalar_projection(&CypherReturnTarget::PathNodes),
            CypherReturnScalarProjectionKind::PathFunction
        );
    }

    #[test]
    fn cypher_returning_builds_scalar_ast() {
        assert!(matches!(
            scalar_return_ast(&CypherReturnTarget::All),
            CypherReturnScalarAst::Star
        ));
        assert!(matches!(
            scalar_return_ast(&CypherReturnTarget::Element),
            CypherReturnScalarAst::Element
        ));
        assert!(matches!(
            scalar_return_ast(&CypherReturnTarget::Property("id".into())),
            CypherReturnScalarAst::DirectProperty("id")
        ));
        assert!(matches!(
            scalar_return_ast(&CypherReturnTarget::Literal(Value::Int(1))),
            CypherReturnScalarAst::Literal(Value::Int(1))
        ));
        assert!(matches!(
            scalar_return_ast(&CypherReturnTarget::PropertyListIndex(
                CypherReturnListIndexProjection {
                    key: "tags".into(),
                    index: CypherReturnListBound {
                        variable: None,
                        target: Box::new(CypherReturnTarget::Literal(Value::Int(1))),
                    },
                },
            )),
            CypherReturnScalarAst::PropertyListIndex(CypherReturnListIndexProjection {
                key,
                index,
            }) if key == "tags"
                && index.variable.is_none()
                && *index.target == CypherReturnTarget::Literal(Value::Int(1))
        ));
        assert!(matches!(
            scalar_return_ast(&CypherReturnTarget::PropertyNumericRound(
                CypherReturnNumericRoundProjection {
                    variable: Some("n".into()),
                    target: Box::new(CypherReturnTarget::Property("score".into())),
                    round: CypherReturnNumericRound::Ceil,
                }
            )),
            CypherReturnScalarAst::PropertyNumericRound(CypherReturnNumericRoundProjection {
                variable: Some(variable),
                target,
                round: CypherReturnNumericRound::Ceil,
            }) if variable == "n" && **target == CypherReturnTarget::Property("score".into())
        ));
        assert!(matches!(
            scalar_return_ast(&CypherReturnTarget::PropertyStringTransform(
                CypherReturnStringTransformProjection {
                    variable: Some("n".into()),
                    target: Box::new(CypherReturnTarget::Property("name".into())),
                    transform: CypherReturnStringTransform::Upper,
                }
            )),
            CypherReturnScalarAst::PropertyStringTransform(
                CypherReturnStringTransformProjection {
                    variable: Some(variable),
                    target,
                    transform: CypherReturnStringTransform::Upper,
                }
            ) if variable == "n" && matches!(target.as_ref(), CypherReturnTarget::Property(key) if key == "name")
        ));
        assert!(matches!(
            scalar_return_ast(&CypherReturnTarget::PropertyStringTransform(
                CypherReturnStringTransformProjection {
                    variable: None,
                    target: Box::new(CypherReturnTarget::Literal(Value::from("ADA"))),
                    transform: CypherReturnStringTransform::Lower,
                }
            )),
            CypherReturnScalarAst::PropertyStringTransform(
                CypherReturnStringTransformProjection {
                    variable: None,
                    target,
                    transform: CypherReturnStringTransform::Lower,
                }
            ) if matches!(target.as_ref(), CypherReturnTarget::Literal(Value::String(value)) if value == "ADA")
        ));
        assert!(matches!(
            scalar_return_ast(&CypherReturnTarget::ElementId),
            CypherReturnScalarAst::ElementFunction
        ));
        assert!(matches!(
            scalar_return_ast(&CypherReturnTarget::PathRelationships),
            CypherReturnScalarAst::PathFunction
        ));
    }

    #[test]
    fn cypher_returning_classifies_scalar_ast_families() {
        assert_eq!(
            classify_return_scalar_ast_family(&scalar_return_ast(&CypherReturnTarget::All)),
            CypherReturnScalarAstFamily::Binding
        );
        assert_eq!(
            classify_return_scalar_ast_family(&scalar_return_ast(&CypherReturnTarget::PathNodes)),
            CypherReturnScalarAstFamily::Wrapper
        );
        assert_eq!(
            classify_return_scalar_ast_family(&scalar_return_ast(
                &CypherReturnTarget::ListProjection(CypherReturnListProjection {
                    variable: Some("n".into()),
                    terms: vec![CypherReturnTarget::Property("id".into())],
                })
            )),
            CypherReturnScalarAstFamily::Value
        );
        assert_eq!(
            classify_return_scalar_ast_family(&scalar_return_ast(&CypherReturnTarget::Case(
                CypherReturnCase {
                    key: "status".into(),
                    equals: Value::from("active"),
                    then_target: Box::new(CypherReturnTarget::Literal(Value::Bool(true))),
                    else_target: Box::new(CypherReturnTarget::Literal(Value::Bool(false))),
                }
            ))),
            CypherReturnScalarAstFamily::Control
        );
        assert_eq!(
            classify_return_scalar_ast_family(&scalar_return_ast(
                &CypherReturnTarget::PropertySize("tags".into())
            )),
            CypherReturnScalarAstFamily::Introspection
        );
        assert_eq!(
            classify_return_scalar_ast_family(&scalar_return_ast(
                &CypherReturnTarget::PropertyListContains(CypherReturnListContains {
                    key: "tags".into(),
                    needle: Value::from("speaker"),
                })
            )),
            CypherReturnScalarAstFamily::List
        );
        assert_eq!(
            classify_return_scalar_ast_family(&scalar_return_ast(
                &CypherReturnTarget::PropertyNumericSign(CypherReturnNumericSignProjection {
                    variable: Some("n".into()),
                    target: Box::new(CypherReturnTarget::Property("score".into())),
                })
            )),
            CypherReturnScalarAstFamily::Numeric
        );
        assert_eq!(
            classify_return_scalar_ast_family(&scalar_return_ast(
                &CypherReturnTarget::PropertyToBoolean(CypherReturnToBooleanProjection {
                    variable: Some("n".into()),
                    target: Box::new(CypherReturnTarget::Property("active".into())),
                })
            )),
            CypherReturnScalarAstFamily::Conversion
        );
        assert_eq!(
            classify_return_scalar_ast_family(&scalar_return_ast(
                &CypherReturnTarget::PropertyIsEmpty(CypherReturnIsEmptyProjection {
                    variable: Some("n".into()),
                    target: Box::new(CypherReturnTarget::Property("name".into())),
                })
            )),
            CypherReturnScalarAstFamily::String
        );
    }

    #[test]
    fn cypher_returning_groups_mixed_aggregate_rows() {
        let store = MemoryGraphStore::new();

        let grouped =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'ada', status: 'active', team: 'eng', score: 10, code: 'eng/ada'});
                CREATE (:Person {id: 'bob', status: 'active', team: 'eng', score: 20, code: 'eng/bob'});
                CREATE (:Person {id: 'cara', status: 'active', team: 'ops', score: 7, code: 'ops/cara'});
                MATCH (n:Person {status: 'active'}) SET n.seen = true
                RETURN n.team AS team,
                       count(*) AS people,
                       sum(n.score) AS total,
                       collect(n.id) AS ids,
                       collect(split(n.code, '/')) AS code_parts,
                       collect(*) AS rows
                ORDER BY total DESC;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("mixed aggregate/scalar RETURN should group by scalar projections");

        assert_eq!(
            grouped.table.columns,
            vec![
                "team".to_string(),
                "people".to_string(),
                "total".to_string(),
                "ids".to_string(),
                "code_parts".to_string(),
                "rows".to_string()
            ]
        );
        assert_eq!(grouped.table.rows.len(), 2);
        assert_eq!(
            &grouped.table.rows[0][..5],
            &[
                Value::from("eng"),
                Value::Int(2),
                Value::Int(30),
                Value::Json(serde_json::json!(["ada", "bob"])),
                Value::Json(serde_json::json!([["eng", "ada"], ["eng", "bob"]]))
            ]
        );
        let Value::Json(eng_rows) = &grouped.table.rows[0][5] else {
            panic!("collect(*) should return JSON rows");
        };
        assert_eq!(eng_rows.as_array().expect("array").len(), 2);
        assert_eq!(eng_rows[0]["n"]["id"], serde_json::json!("ada"));
        assert_eq!(eng_rows[1]["n"]["id"], serde_json::json!("bob"));
        assert_eq!(
            &grouped.table.rows[1][..5],
            &[
                Value::from("ops"),
                Value::Int(1),
                Value::Int(7),
                Value::Json(serde_json::json!(["cara"])),
                Value::Json(serde_json::json!([["ops", "cara"]]))
            ]
        );
        let Value::Json(ops_rows) = &grouped.table.rows[1][5] else {
            panic!("collect(*) should return JSON rows");
        };
        assert_eq!(ops_rows.as_array().expect("array").len(), 1);
        assert_eq!(ops_rows[0]["n"]["id"], serde_json::json!("cara"));

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Audit {id: 'grouped-concrete', kind: 'write'})
                 RETURN n.kind, count(*) AS writes;",
                CypherMutationOptions::default(),
            ))
            .expect("concrete row can mix scalar and aggregate projections");
        assert_eq!(
            concrete.table,
            CypherResultTable {
                columns: vec!["n.kind".to_string(), "writes".to_string()],
                rows: vec![vec![Value::from("write"), Value::Int(1)]],
            }
        );
    }

    #[test]
    fn cypher_returning_counts_materialized_rows_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'ada'}) RETURN count(n) AS writes;",
                CypherMutationOptions::default(),
            ))
            .expect("count concrete write row");
        assert_eq!(
            concrete.table,
            CypherResultTable {
                columns: vec!["writes".to_string()],
                rows: vec![vec![Value::Int(1)]],
            }
        );

        let concrete_props =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'dana', email: 'dana@example.test'})
                 RETURN count(n.email) AS emails, count(n.missing) AS missing;",
                CypherMutationOptions::default(),
            ))
            .expect("count concrete properties");
        assert_eq!(
            concrete_props.table,
            CypherResultTable {
                columns: vec!["emails".to_string(), "missing".to_string()],
                rows: vec![vec![Value::Int(1), Value::Int(0)]],
            }
        );

        let row_producing =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'bob', status: 'active'});
                CREATE (:Person {id: 'cara', status: 'active'});
                CREATE (:Team {id: 'eng'});
                MATCH (n:Person {status: 'active'}), (t:Team {id: 'eng'})
                CREATE (n)-[e:MEMBER_OF {source: 'cypher'}]->(t)
                RETURN count(e) AS relationships, count(e.source) AS sourced, count(e.id) AS explicit_ids
                LIMIT ALL;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("count row-producing write rows");
        assert_eq!(
            row_producing.table,
            CypherResultTable {
                columns: vec![
                    "relationships".to_string(),
                    "sourced".to_string(),
                    "explicit_ids".to_string()
                ],
                rows: vec![vec![Value::Int(2), Value::Int(2), Value::Int(0)]],
            }
        );

        let star =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (:Audit {id: 'a1'}) RETURN COUNT ( * ) AS rows;",
                CypherMutationOptions::default(),
            ))
            .expect("count star with spaces");
        assert_eq!(
            star.table,
            CypherResultTable {
                columns: vec!["rows".to_string()],
                rows: vec![vec![Value::Int(1)]],
            }
        );

        let shared_projection_targets =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'count-projection-a', code: 'a/b', score: 7});
                CREATE (:Person {id: 'count-projection-b', code: 'c/d', score: 7});
                MATCH (n:Person) WHERE n.id STARTS WITH 'count-projection-'
                SET n.counted = true
                RETURN count(split(n.code, '/')) AS split_codes,
                       count(DISTINCT toString(n.score)) AS distinct_scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted count projections should share scalar materialization");
        assert_eq!(
            shared_projection_targets.table,
            CypherResultTable {
                columns: vec!["split_codes".to_string(), "distinct_scores".to_string()],
                rows: vec![vec![Value::Int(2), Value::Int(1)]],
            }
        );
    }

    #[test]
    fn cypher_returning_counts_distinct_materialized_values() {
        let store = MemoryGraphStore::new();

        let row_nodes =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'ada', status: 'active', department: 'eng'});
                CREATE (:Person {id: 'bob', status: 'active', department: 'eng'});
                CREATE (:Person {id: 'cara', status: 'active', department: 'ops'});
                MATCH (n:Person {status: 'active'}) SET n.seen = true
                RETURN count(n.department) AS departments,
                       count(DISTINCT n.department) AS distinct_departments,
                       count(DISTINCT n.missing) AS missing;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("count distinct row node properties");
        assert_eq!(
            row_nodes.table,
            CypherResultTable {
                columns: vec![
                    "departments".to_string(),
                    "distinct_departments".to_string(),
                    "missing".to_string()
                ],
                rows: vec![vec![Value::Int(3), Value::Int(2), Value::Int(0)]],
            }
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'eng'});
                MATCH (n:Person {status: 'active'}), (t:Team {id: 'eng'})
                CREATE (n)-[e:MEMBER_OF {source: 'cypher'}]->(t)
                RETURN count(e) AS relationships,
                       count(DISTINCT e.label) AS distinct_labels,
                       count(DISTINCT e.source) AS distinct_sources;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("count distinct row edge properties");
        assert_eq!(
            row_edges.table,
            CypherResultTable {
                columns: vec![
                    "relationships".to_string(),
                    "distinct_labels".to_string(),
                    "distinct_sources".to_string()
                ],
                rows: vec![vec![Value::Int(3), Value::Int(1), Value::Int(1)]],
            }
        );
    }

    #[test]
    fn cypher_returning_evaluates_restricted_numeric_aggregates() {
        let store = MemoryGraphStore::new();

        let row_nodes =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'ada', status: 'active', score: 10, team: 'eng'});
                CREATE (:Person {id: 'bob', status: 'active', score: 20, team: 'eng'});
                CREATE (:Person {id: 'cara', status: 'active', score: 20, team: 'ops'});
                CREATE (:Person {id: 'dana', status: 'active'});
                MATCH (n:Person {status: 'active'}) SET n.seen = true
                RETURN sum(n.score) AS total,
                       avg(n.score) AS average,
                       min(n.score) AS low,
                       max(n.score) AS high,
                       sum(DISTINCT n.score) AS distinct_total;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted numeric aggregates over broad node rows");
        assert_eq!(
            row_nodes.table,
            CypherResultTable {
                columns: vec![
                    "total".to_string(),
                    "average".to_string(),
                    "low".to_string(),
                    "high".to_string(),
                    "distinct_total".to_string()
                ],
                rows: vec![vec![
                    Value::Int(50),
                    Value::Float(50.0 / 3.0),
                    Value::Int(10),
                    Value::Int(20),
                    Value::Int(30),
                ]],
            }
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'eng'});
                MATCH (n:Person {status: 'active'}), (t:Team {id: 'eng'})
                CREATE (n)-[e:MEMBER_OF {weight: 1.5, source: 'cypher'}]->(t)
                RETURN sum(e.weight) AS total_weight,
                       avg(e.weight) AS average_weight,
                       min(e.source) AS first_source,
                       max(e.source) AS last_source;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted aggregates over row-producing edges");
        assert_eq!(
            row_edges.table,
            CypherResultTable {
                columns: vec![
                    "total_weight".to_string(),
                    "average_weight".to_string(),
                    "first_source".to_string(),
                    "last_source".to_string()
                ],
                rows: vec![vec![
                    Value::Float(6.0),
                    Value::Float(1.5),
                    Value::from("cypher"),
                    Value::from("cypher"),
                ]],
            }
        );

        let missing =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (:Audit {id: 'aggregate-missing'}) RETURN sum(a.missing) AS missing;",
                CypherMutationOptions::default(),
            ))
            .expect_err("unbound aggregate variable should fail");
        assert!(matches!(missing, GrustError::CypherUnresolvedIdentity(_)));
    }

    #[test]
    fn cypher_returning_rejects_unsupported_aggregate_forms() {
        let store = MemoryGraphStore::new();

        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'ada', name: 'Ada'}) RETURN sum(n.name);",
                CypherMutationOptions::default(),
            ))
            .expect_err("SUM over strings should fail");
        assert!(
            matches!(error, GrustError::CypherUnsupportedCardinality(_)),
            "{error:?}"
        );

        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'cara', score: 1}) RETURN avg(n);",
                CypherMutationOptions::default(),
            ))
            .expect_err("non-count aggregate over element should fail");
        assert!(
            matches!(error, GrustError::CypherUnsupportedCardinality(_)),
            "{error:?}"
        );

        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (:Person {id: 'dana', score: 1}) RETURN sum(*);",
                CypherMutationOptions::default(),
            ))
            .expect_err("non-count aggregate star should fail");
        assert!(
            matches!(error, GrustError::CypherUnsupportedCardinality(_)),
            "{error:?}"
        );
    }

    #[test]
    fn cypher_returning_collects_restricted_materialized_values() {
        let store = MemoryGraphStore::new();

        let row_nodes =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'ada', status: 'active', team: 'eng'});
                CREATE (:Person {id: 'bob', status: 'active', team: 'eng'});
                CREATE (:Person {id: 'cara', status: 'active', team: 'ops'});
                CREATE (:Person {id: 'dana', status: 'active'});
                MATCH (n:Person {status: 'active'}) SET n.seen = true
                RETURN collect(n.team) AS teams,
                       collect(DISTINCT n.team) AS distinct_teams,
                       collect(n.missing) AS missing,
                       collect(*) AS rows;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted collect over broad node rows");
        assert_eq!(
            row_nodes.table.columns,
            vec![
                "teams".to_string(),
                "distinct_teams".to_string(),
                "missing".to_string(),
                "rows".to_string(),
            ]
        );
        assert_eq!(
            &row_nodes.table.rows[0][..3],
            &[
                Value::Json(serde_json::json!(["eng", "eng", "ops"])),
                Value::Json(serde_json::json!(["eng", "ops"])),
                Value::Json(serde_json::json!([]))
            ]
        );
        let Value::Json(rows) = &row_nodes.table.rows[0][3] else {
            panic!("collect(*) should return JSON rows");
        };
        let rows = rows.as_array().expect("array");
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0]["n"]["id"], serde_json::json!("ada"));
        assert_eq!(rows[1]["n"]["id"], serde_json::json!("bob"));
        assert_eq!(rows[2]["n"]["id"], serde_json::json!("cara"));
        assert_eq!(rows[3]["n"]["id"], serde_json::json!("dana"));

        let shared_projection_targets =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'aggregate-projection-a', code: 'a/b', score: 7});
                CREATE (:Person {id: 'aggregate-projection-b', code: 'c/d', score: 11});
                MATCH (n:Person) WHERE n.id STARTS WITH 'aggregate-projection-'
                SET n.seen = true
                RETURN collect(split(n.code, '/')) AS split_codes,
                       collect(toString(n.score)) AS string_scores;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted aggregate projections should share scalar materialization");
        assert_eq!(
            shared_projection_targets.table.rows,
            vec![vec![
                Value::Json(serde_json::json!([["a", "b"], ["c", "d"]])),
                Value::Json(serde_json::json!(["7", "11"])),
            ]]
        );

        let row_edges =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Team {id: 'eng'});
                MATCH (n:Person {status: 'active'}), (t:Team {id: 'eng'})
                CREATE (n)-[e:MEMBER_OF {source: 'cypher'}]->(t)
                RETURN collect(e.source) AS sources,
                       collect(DISTINCT e.label) AS labels;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted collect over row-producing edge rows");
        assert_eq!(
            row_edges.table,
            CypherResultTable {
                columns: vec!["sources".to_string(), "labels".to_string()],
                rows: vec![vec![
                    Value::Json(serde_json::json!(["cypher", "cypher", "cypher", "cypher"])),
                    Value::Json(serde_json::json!(["MEMBER_OF"])),
                ]],
            }
        );
    }

    #[test]
    fn cypher_returning_collects_bound_elements() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (n:Person {id: 'ada', status: 'active'})
                RETURN collect(n) AS nodes, collect(DISTINCT n) AS distinct_nodes;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted collect over concrete node element");
        assert_eq!(result.table.columns, vec!["nodes", "distinct_nodes"]);
        assert_eq!(result.table.rows.len(), 1);
        let Value::Json(nodes) = &result.table.rows[0][0] else {
            panic!("collect(n) should return JSON array");
        };
        assert_eq!(nodes.as_array().expect("array").len(), 1);
        assert_eq!(nodes[0]["id"], serde_json::Value::String("ada".to_string()));
        let Value::Json(distinct_nodes) = &result.table.rows[0][1] else {
            panic!("collect(DISTINCT n) should return JSON array");
        };
        assert_eq!(distinct_nodes.as_array().expect("array").len(), 1);

        let star =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (a:Audit {id: 'collect-star'}) RETURN collect(*) AS rows;",
                CypherMutationOptions::default(),
            ))
            .expect("collect star over concrete bound variable");
        assert_eq!(star.table.columns, vec!["rows"]);
        let Value::Json(rows) = &star.table.rows[0][0] else {
            panic!("collect(*) should return JSON rows");
        };
        let rows = rows.as_array().expect("array");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["a"]["id"], serde_json::json!("collect-star"));
        assert_eq!(rows[0]["a"]["label"], serde_json::json!("Audit"));
    }

    #[test]
    fn cypher_returning_count_rejects_unbound_variables() {
        let store = MemoryGraphStore::new();

        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (:Person {id: 'ada'}) RETURN count(n);",
                CypherMutationOptions::default(),
            ))
            .expect_err("count over unbound variable should fail");
        assert!(matches!(error, GrustError::CypherUnresolvedIdentity(_)));

        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (:Person {id: 'bob'}) RETURN count(DISTINCT *);",
                CypherMutationOptions::default(),
            ))
            .expect_err("COUNT DISTINCT star should stay deferred");
        assert!(matches!(error, GrustError::CypherUnsupportedCardinality(_)));
    }

    #[test]
    fn cypher_returning_accepts_limit_all_on_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'a', age: 30});
                CREATE (:Person {id: 'b', age: 20});
                MATCH (n:Person) SET n.seen = true
                RETURN n.id AS id ORDER BY id LIMIT ALL;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("LIMIT ALL should preserve all rows");
        assert_eq!(
            result.table,
            CypherResultTable {
                columns: vec!["id".to_string()],
                rows: vec![vec![Value::from("a")], vec![Value::from("b")]],
            }
        );
    }

    #[test]
    fn cypher_returning_accepts_offset_control() {
        let store = MemoryGraphStore::new();

        let rows =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
            CREATE (:Person {id: 'a', age: 30});
            CREATE (:Person {id: 'b', age: 20});
            CREATE (:Person {id: 'c', age: 40});
            MATCH (n:Person) SET n.seen = true
            RETURN n.id AS id, n.age AS age ORDER BY age DESC OFFSET 1 LIMIT 1;
            ",
                CypherMutationOptions::default(),
            ))
            .expect("OFFSET should behave like SKIP");
        assert_eq!(
            rows.table,
            CypherResultTable {
                columns: vec!["id".to_string(), "age".to_string()],
                rows: vec![vec![Value::from("a"), Value::Int(30)]],
            }
        );

        let aggregate =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (:Audit {id: 'offset-count'}) RETURN count(*) AS writes OFFSET 0 LIMIT ALL;",
                CypherMutationOptions::default(),
            ))
            .expect("OFFSET should work on aggregate table");
        assert_eq!(
            aggregate.table,
            CypherResultTable {
                columns: vec!["writes".to_string()],
                rows: vec![vec![Value::Int(1)]],
            }
        );
    }

    #[test]
    fn cypher_returning_distinct_dedupes_materialized_rows() {
        let store = MemoryGraphStore::new();

        let rows =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
            CREATE (:Person {id: 'ada', status: 'active', department: 'eng'});
            CREATE (:Person {id: 'bob', status: 'active', department: 'eng'});
            CREATE (:Person {id: 'cara', status: 'active', department: 'ops'});
            MATCH (n:Person {status: 'active'}) SET n.seen = true
            RETURN DISTINCT n.department AS department ORDER BY department;
            ",
                CypherMutationOptions::default(),
            ))
            .expect("restricted RETURN DISTINCT over broad rows");

        assert_eq!(
            rows.table,
            CypherResultTable {
                columns: vec!["department".to_string()],
                rows: vec![vec![Value::from("eng")], vec![Value::from("ops")]],
            }
        );

        let aggregate =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (:Audit {id: 'distinct-row'}) RETURN DISTINCT count(*) AS rows;",
                CypherMutationOptions::default(),
            ))
            .expect("RETURN DISTINCT over aggregate result row");
        assert_eq!(
            aggregate.table,
            CypherResultTable {
                columns: vec!["rows".to_string()],
                rows: vec![vec![Value::Int(1)]],
            }
        );
    }

    #[test]
    fn cypher_returning_distinct_requires_projection() {
        let store = MemoryGraphStore::new();

        let error =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (:Person {id: 'ada'}) RETURN DISTINCT;",
                CypherMutationOptions::default(),
            ))
            .expect_err("RETURN DISTINCT without projection should fail");
        assert!(matches!(error, GrustError::CypherSyntax(_)), "{error:?}");
    }

    #[test]
    fn cypher_returning_orders_by_projection_expression() {
        let store = MemoryGraphStore::new();

        let rows =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
            CREATE (:Person {id: 'ada', status: 'active', department: 'eng'});
            CREATE (:Person {id: 'bob', status: 'active', department: 'ops'});
            MATCH (n:Person {status: 'active'}) SET n.seen = true
            RETURN n.department AS department ORDER BY n.department DESC;
            ",
                CypherMutationOptions::default(),
            ))
            .expect("ORDER BY returned projection expression");

        assert_eq!(
            rows.table,
            CypherResultTable {
                columns: vec!["department".to_string()],
                rows: vec![vec![Value::from("ops")], vec![Value::from("eng")]],
            }
        );

        let count =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (:Audit {id: 'order-count'}) RETURN count(*) AS writes ORDER BY count(*);",
                CypherMutationOptions::default(),
            ))
            .expect("ORDER BY returned aggregate expression");
        assert_eq!(
            count.table,
            CypherResultTable {
                columns: vec!["writes".to_string()],
                rows: vec![vec![Value::Int(1)]],
            }
        );
    }

    #[test]
    fn cypher_returning_generic_row_producing_edges_memory_facade() {
        let store = MemoryGraphStore::new();

        let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'ada', status: 'active'});
                CREATE (:Person {id: 'bob', status: 'active'});
                CREATE (:Team {id: 'eng'});
                MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
                CREATE (a)-[e:MEMBER_OF {source: 'generic'}]->(b)
                RETURN e.label, e.source, e.id;
                ",
                CypherMutationOptions::default(),
            ))
            .unwrap();

        assert_eq!(
            result.mutation.report,
            GraphMutationReport {
                creates: 4,
                matched_rows: 2,
                changed_nodes: 3,
                changed_edges: 2,
                node_upserts: 3,
                edge_upserts: 2,
                node_inserts: 3,
                edge_inserts: 2,
                ..GraphMutationReport::default()
            }
        );
        assert_eq!(
            result.table,
            CypherResultTable {
                columns: vec![
                    "e.label".to_string(),
                    "e.source".to_string(),
                    "e.id".to_string()
                ],
                rows: vec![
                    vec![
                        Value::from("MEMBER_OF"),
                        Value::from("generic"),
                        Value::Null
                    ],
                    vec![
                        Value::from("MEMBER_OF"),
                        Value::from("generic"),
                        Value::Null
                    ],
                ],
            }
        );
    }

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
                    relationship_id_policy:
                        CypherRelationshipIdPolicy::GenerateForRowCreateAndMerge,
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
        let plan = sail_cypher_mutation_plan(
            "
            CREATE (a:Person {id: 'person-1', name: 'Ada'});
            MERGE (b:Person {id: 'person-2', name: 'Bob'});
            CREATE (a)-[:KNOWS]->(b);
            DELETE (a)-[:KNOWS]->(b);
            DELETE (a);
            ",
        )
        .unwrap();

        assert_eq!(
            plan.report(),
            GraphMutationReport {
                creates: 2,
                merges: 1,
                deletes: 2,
                changed_nodes: 3,
                changed_edges: 2,
                node_upserts: 2,
                edge_upserts: 1,
                node_deletes: 1,
                edge_deletes: 1,
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
                GraphMutation::UpsertEdge(
                    Edge::new("KNOWS", "person-1", "person-2", Props::new(),)
                ),
                GraphMutation::DeleteEdge {
                    from: NodeId::new("person-1"),
                    label: Label::new("KNOWS"),
                    to: NodeId::new("person-2"),
                },
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
            let error =
                sail_cypher_mutation_plan(cypher).expect_err("unsupported Cypher must fail");
            assert!(is_cypher_planning_error(&error));
        }
    }

    #[test]
    fn cypher_errors_are_structured_for_callers() {
        let error = sail_cypher_mutation_plan("RETURN 1").expect_err("unsupported syntax");
        assert!(matches!(error, GrustError::CypherSyntax(_)));

        let error = sail_cypher_mutation_plan("DELETE (:Person {name: 'Ada'})")
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
        let statements =
            sail_cypher_ddl("CREATE CONSTRAINT FOR (n:Person) REQUIRE n.name IS NOT NULL")
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
        let statements =
            sail_cypher_ddl("CREATE CONSTRAINT ON (n:Person) ASSERT n.email IS UNIQUE")
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
        let imported =
            CypherSchemaManager::from_registry_json(manager.schema.clone(), &registry_json)
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
