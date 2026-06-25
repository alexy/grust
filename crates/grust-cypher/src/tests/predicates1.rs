//! predicates1 tests (split verbatim from the former monolithic tests.rs).
use super::*;

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
    let bounded = sail_cypher_mutation_plan("MATCH (n:Person {active: false}) DELETE n").unwrap();
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
        let error =
            sail_cypher_mutation_plan(cypher).expect_err("unsupported WHERE predicate should fail");
        assert!(is_cypher_planning_error(&error) || matches!(error, GrustError::CypherSyntax(_)));
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

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
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
        ))
        .expect("restricted negated string OR WHERE predicates should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![
            Value::from("where-not-string-or-ada"),
            Value::Bool(true)
        ]]
    );
}
