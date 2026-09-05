use std::sync::Barrier;

use futures_executor::block_on;

use super::*;

fn props(value: i64) -> Props {
    Props::from([("score".into(), Value::Int(value))])
}

fn store() -> MemoryGraphStore {
    let store = MemoryGraphStore::new();
    block_on(store.put_graph(&Graph::new(
        vec![
            Node::new("Person", "a", props(1)),
            Node::new("Person", "b", props(2)),
        ],
        vec![Edge::new("KNOWS", "a", "b", props(3))],
    )))
    .unwrap();
    store
}

fn rebuilt(store: &MemoryGraphStore, old: &Arc<TypedGraphIndex>) -> Arc<TypedGraphIndex> {
    let new = store.indexed_snapshot().unwrap();
    assert!(!Arc::ptr_eq(old, &new));
    assert_eq!(new.graph(), &store.graph());
    assert!(Arc::ptr_eq(&new, &store.indexed_snapshot().unwrap()));
    new
}

#[test]
fn cached_snapshot_is_shared_across_calls_and_cloned_stores() {
    let store = store();
    let first = store.indexed_snapshot().unwrap();
    assert_eq!(first.graph(), &store.graph());
    assert!(Arc::ptr_eq(&first, &store.indexed_snapshot().unwrap()));
    assert!(Arc::ptr_eq(
        &first,
        &store.clone().indexed_snapshot().unwrap()
    ));
    let a = first.vertex_index("a").unwrap();
    let b = first.vertex_index("b").unwrap();
    assert!(first.has_relationship(a, b, "KNOWS"));
    block_on(store.get_node(&NodeId::new("a"))).unwrap();
    block_on(store.get_edges(EdgeQuery::default())).unwrap();
    assert!(Arc::ptr_eq(&first, &store.indexed_snapshot().unwrap()));
}

#[test]
fn node_edge_and_batch_writes_leave_previous_snapshots_immutable() {
    let store = store();
    let original = store.indexed_snapshot().unwrap();
    let original_graph = original.graph().clone();
    block_on(store.clone().put_node(&Node::new("Person", "a", props(10)))).unwrap();
    let after_node = rebuilt(&store, &original);
    assert_eq!(after_node.graph().nodes[0].props["score"], Value::Int(10));
    block_on(store.put_edge(&Edge::new("KNOWS", "a", "b", props(20)))).unwrap();
    let after_edge = rebuilt(&store, &after_node);
    assert_eq!(after_edge.graph().edges[0].props["score"], Value::Int(20));
    block_on(store.put_graph(&Graph::new(
        vec![Node::new("Person", "c", props(30))],
        vec![Edge::new("KNOWS", "b", "c", Props::new())],
    )))
    .unwrap();
    let after_batch = rebuilt(&store, &after_edge);
    assert_eq!(after_batch.graph().nodes.len(), 3);
    assert_eq!(after_batch.graph().edges.len(), 2);
    assert_eq!(original.graph(), &original_graph);
    assert_eq!(after_node.graph().edges[0].props["score"], Value::Int(3));
}

#[test]
fn edge_node_deletes_and_delete_all_plan_invalidate_the_index() {
    let store = store();
    let before = store.indexed_snapshot().unwrap();
    block_on(store.delete_edge(&NodeId::new("a"), &Label::new("KNOWS"), &NodeId::new("b")))
        .unwrap();
    let without_edge = rebuilt(&store, &before);
    assert!(without_edge.graph().edges.is_empty());
    block_on(store.put_edge(&Edge::new("KNOWS", "a", "b", Props::new()))).unwrap();
    let with_edge = rebuilt(&store, &without_edge);
    block_on(store.delete_node(&NodeId::new("a"))).unwrap();
    let without_node = rebuilt(&store, &with_edge);
    assert!(without_node.vertex_index("a").is_none());
    assert!(without_node.graph().edges.is_empty());
    // Memory's existing API clears data through matched deletion, rather than
    // implementing GraphAdminStore::clear.
    block_on(
        store.execute_cypher_mutation_plan(&GraphMutationPlan::new(vec![
            GraphMutationPlanOp::DeleteMatchingNodes {
                label: None,
                props: Props::new(),
                predicates: vec![],
                cardinality: GraphMutationCardinality::BoundedMany,
            },
        ])),
    )
    .unwrap();
    let empty = rebuilt(&store, &without_node);
    assert!(empty.graph().nodes.is_empty());
    assert!(empty.graph().edges.is_empty());
    assert_eq!(before.graph().nodes.len(), 2);
    assert_eq!(before.graph().edges.len(), 1);
}

#[test]
fn schema_native_constraints_and_rejected_writes_invalidate_conservatively() {
    let store = store();
    let before = store.indexed_snapshot().unwrap();
    let schema = GraphSchema::builder()
        .node("Person", vec![Field::required("score", FieldType::Int)])
        .edge(
            "KNOWS",
            vec![Label::new("Person")],
            vec![Label::new("Person")],
            vec![Field::required("score", FieldType::Int)],
        )
        .build();
    block_on(store.apply_schema(&schema)).unwrap();
    let after_schema = rebuilt(&store, &before);
    assert_eq!(before.graph(), after_schema.graph());
    assert!(block_on(store.put_node(&Node::new("Person", "a", Props::new()))).is_err());
    let after_error = rebuilt(&store, &after_schema);
    assert_eq!(after_schema.graph(), after_error.graph());
    let invalid_schema = GraphSchema::builder().node("Other", vec![]).build();
    assert!(block_on(store.apply_schema(&invalid_schema)).is_err());
    let after_schema_error = rebuilt(&store, &after_error);
    let request = GraphNativeConstraintRequest {
        constraint: GraphConstraint::NodePropertyUnique {
            label: Label::new("Person"),
            key: "score".into(),
        },
        if_not_exists: false,
    };
    block_on(store.apply_native_constraint(request.clone())).unwrap();
    let after_constraint = rebuilt(&store, &after_schema_error);
    assert!(block_on(store.apply_native_constraint(request)).is_err());
    rebuilt(&store, &after_constraint);
}

fn relationship() -> GraphRelationshipMatch {
    GraphRelationshipMatch {
        from: GraphNodeMatch::default(),
        label: Label::new("KNOWS"),
        to: GraphNodeMatch::default(),
        id: None,
        props: Props::new(),
        predicates: vec![],
    }
}

#[test]
fn every_direct_mutation_plan_branch_rebuilds_the_snapshot() {
    let cardinality = GraphMutationCardinality::BoundedMany;
    let operations = vec![
        GraphMutationPlanOp::PatchMatchingNodes {
            label: None,
            props: Props::new(),
            predicates: vec![],
            patch: props(10),
            cardinality,
        },
        GraphMutationPlanOp::UpdateMatchingNodeProperty {
            label: None,
            props: Props::new(),
            predicates: vec![],
            target_key: "score".into(),
            source_key: "score".into(),
            op: GraphNumericOp::Add,
            operand: Value::Int(1),
            cardinality,
        },
        GraphMutationPlanOp::RemoveMatchingNodeProps {
            label: None,
            props: Props::new(),
            predicates: vec![],
            keys: vec!["score".into()],
            cardinality,
        },
        GraphMutationPlanOp::DeleteMatchingNodes {
            label: None,
            props: Props::new(),
            predicates: vec![],
            cardinality,
        },
        GraphMutationPlanOp::PatchMatchingEdges {
            relationship: relationship(),
            patch: props(10),
            cardinality,
        },
        GraphMutationPlanOp::UpdateMatchingEdgeProperty {
            relationship: relationship(),
            target_key: "score".into(),
            source_key: "score".into(),
            op: GraphNumericOp::Add,
            operand: Value::Int(1),
            cardinality,
        },
        GraphMutationPlanOp::RemoveMatchingEdgeProps {
            relationship: relationship(),
            keys: vec!["score".into()],
            cardinality,
        },
        GraphMutationPlanOp::DeleteMatchingEdges {
            relationship: relationship(),
            cardinality,
        },
        GraphMutationPlanOp::DeleteRelationshipRows {
            relationship: relationship(),
            delete_edges: true,
            endpoint_nodes: vec![GraphRelationshipEndpoint::From],
            target_count: 2,
            cardinality,
        },
        GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
            kind: GraphMutationPlanKind::Create,
            from: GraphNodeMatch::default(),
            to: GraphNodeMatch::default(),
            label: Label::new("OTHER"),
            props: Props::new(),
            edge_id_policy: GraphRowEdgeIdPolicy::GenerateForCreate,
            cardinality,
        },
        GraphMutationPlanOp::SetMatchingNodeFromNode {
            target_label: None,
            target_props: Props::new(),
            target_predicates: vec![],
            target_key: "copied".into(),
            source_label: None,
            source_props: Props::new(),
            source_predicates: vec![],
            source_key: "score".into(),
            op: None,
            operand: Value::Null,
            correlation: GraphWriteCorrelation::Cartesian,
            cardinality,
        },
    ];
    for operation in operations {
        let store = store();
        let before = store.indexed_snapshot().unwrap();
        block_on(
            store.execute_cypher_mutation_plan(&GraphMutationPlan::new(vec![operation.clone()])),
        )
        .unwrap_or_else(|error| panic!("{operation:?}: {error}"));
        let after = rebuilt(&store, &before);
        assert_ne!(before.graph(), after.graph(), "{operation:?}");
    }
}

#[test]
fn trait_mutations_and_partially_failing_plans_cannot_leave_a_stale_cache() {
    let store = store();
    let before = store.indexed_snapshot().unwrap();
    block_on(store.apply_mutations(&[GraphMutation::PatchNode {
        id: NodeId::new("a"),
        props: props(99),
    }]))
    .unwrap();
    let patched = rebuilt(&store, &before);
    assert_eq!(patched.graph().nodes[0].props["score"], Value::Int(99));
    let plan = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new("Person", "c", props(3)),
        },
        GraphMutationPlanOp::UpdateMatchingNodeProperty {
            label: None,
            props: Props::new(),
            predicates: vec![],
            target_key: "score".into(),
            source_key: "missing".into(),
            op: GraphNumericOp::Add,
            operand: Value::Int(1),
            cardinality: GraphMutationCardinality::BoundedMany,
        },
    ]);
    assert!(block_on(store.execute_cypher_mutation_plan(&plan)).is_err());
    let partial = rebuilt(&store, &patched);
    assert!(partial.vertex_index("c").is_some());
    assert!(patched.vertex_index("c").is_none());
}

#[test]
fn invalid_index_build_does_not_cache_a_failure_or_change_store_semantics() {
    let store = store();
    let before = store.indexed_snapshot().unwrap();
    // Unconstrained Memory accepts dangling edges; only the new indexed view
    // requires endpoints to exist. Ordinary graph reads must still work.
    block_on(store.put_edge(&Edge::new("KNOWS", "a", "missing", Props::new()))).unwrap();
    assert_eq!(store.graph().edges.len(), 2);
    assert!(store.indexed_snapshot().is_err());
    assert!(store.indexed_snapshot().is_err());
    block_on(store.put_node(&Node::new("Person", "missing", Props::new()))).unwrap();
    let repaired = rebuilt(&store, &before);
    assert!(repaired.has_relationship(
        repaired.vertex_index("a").unwrap(),
        repaired.vertex_index("missing").unwrap(),
        "KNOWS"
    ));
}

#[test]
fn concurrent_first_readers_share_one_snapshot_and_observe_coherent_updates() {
    let store = store();
    let barrier = Arc::new(Barrier::new(8));
    let threads = (0..8)
        .map(|_| {
            let store = store.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                store.indexed_snapshot().unwrap()
            })
        })
        .collect::<Vec<_>>();
    let snapshots = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert!(
        snapshots
            .iter()
            .all(|snapshot| Arc::ptr_eq(&snapshots[0], snapshot))
    );
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let store = &store;
            scope.spawn(move || {
                for _ in 0..32 {
                    let snapshot = store.indexed_snapshot().unwrap();
                    let nodes = &snapshot.graph().nodes;
                    // The initial values differ by one; each atomic batch
                    // update preserves this invariant.
                    let Value::Int(a) = nodes[0].props["score"] else {
                        panic!()
                    };
                    let Value::Int(b) = nodes[1].props["score"] else {
                        panic!()
                    };
                    assert_eq!(b, a + 1);
                }
            });
        }
        for value in 10..26 {
            block_on(store.put_graph(&Graph::new(
                vec![
                    Node::new("Person", "a", props(value)),
                    Node::new("Person", "b", props(value + 1)),
                ],
                vec![],
            )))
            .unwrap();
        }
    });
    assert_eq!(snapshots[0].graph().nodes[0].props["score"], Value::Int(1));
    assert_eq!(
        store.indexed_snapshot().unwrap().graph().nodes[0].props["score"],
        Value::Int(25)
    );
}
