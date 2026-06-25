//! match_misc tests (split verbatim from the former monolithic tests.rs).
use super::*;

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
    let node = sail_cypher_mutation_plan("MATCH (n:Person {id: 'person-1'}) SET n.nickname = null")
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
    let node =
        sail_cypher_mutation_plan("MATCH (n:Person {id: 'person-1'}) REMOVE n.nickname").unwrap();
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
        let error = sail_cypher_mutation_plan(cypher).expect_err("unsupported MATCH SET must fail");
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
        assert!(["star-cara", "star-dana"].contains(&person["id"].as_str().expect("person id")));
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

    let err = CypherWriteResultRows::new(&row_node_values, &row_edge_values, &row_path_bindings)
        .row_count_for_return(&return_clause)
        .expect_err("path rows must validate endpoint and edge row counts");
    assert!(
        matches!(err, GrustError::CypherUnsupportedCardinality(_)),
        "{err:?}"
    );
}
