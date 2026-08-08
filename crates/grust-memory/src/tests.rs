use super::*;

use grust_core::typed::{TypedGraphBuilder, TypedNode, garde};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, garde::Validate)]
#[garde(allow_unvalidated)]
struct Person {
    #[garde(length(min = 1))]
    id: String,
    #[garde(length(min = 1))]
    name: String,
    #[garde(length(min = 1))]
    skill: String,
}

impl TypedNode for Person {
    const LABEL: &'static str = "Person";

    fn node_id(&self) -> NodeId {
        format!("person:{}", self.id).into()
    }
}

fn assert_adjacency_consistent(store: &MemoryGraphStore) {
    let inner = store.inner.read().expect("memory graph lock poisoned");
    for (key, edge) in &inner.edges {
        assert_eq!(key, &MemoryEdgeKey::from_edge(edge));
        assert!(
            inner
                .outgoing_edges
                .get(&key.from)
                .is_some_and(|keys| keys.contains(key))
        );
        assert!(
            inner
                .incoming_edges
                .get(&key.to)
                .is_some_and(|keys| keys.contains(key))
        );
    }
    for (node, keys) in &inner.outgoing_edges {
        assert!(!keys.is_empty());
        assert!(
            keys.iter()
                .all(|key| &key.from == node && inner.edges.contains_key(key))
        );
    }
    for (node, keys) in &inner.incoming_edges {
        assert!(!keys.is_empty());
        assert!(
            keys.iter()
                .all(|key| &key.to == node && inner.edges.contains_key(key))
        );
    }
}

#[test]
fn adjacency_indexes_follow_edge_and_node_mutations() {
    let store = MemoryGraphStore::new();
    let graph = Graph::new(
        vec![
            Node::new("Person", "a", Props::new()),
            Node::new("Person", "b", Props::new()),
            Node::new("Person", "c", Props::new()),
        ],
        vec![
            Edge::new("KNOWS", "a", "b", Props::new()).with_id("edge-1"),
            Edge::new("KNOWS", "a", "b", Props::new()).with_id("edge-2"),
            Edge::new("KNOWS", "c", "a", Props::new()).with_id("edge-3"),
        ],
    );
    futures_executor::block_on(store.put_graph(&graph)).unwrap();
    assert_adjacency_consistent(&store);

    let outgoing = futures_executor::block_on(store.get_edges(EdgeQuery {
        from: Some(NodeId::new("a")),
        ..EdgeQuery::default()
    }))
    .unwrap();
    assert_eq!(outgoing.len(), 2);

    futures_executor::block_on(store.delete_edge(
        &NodeId::new("a"),
        &Label::new("KNOWS"),
        &NodeId::new("b"),
    ))
    .unwrap();
    assert_adjacency_consistent(&store);
    assert!(
        futures_executor::block_on(store.get_edges(EdgeQuery {
            from: Some(NodeId::new("a")),
            ..EdgeQuery::default()
        }))
        .unwrap()
        .is_empty()
    );

    futures_executor::block_on(store.delete_node(&NodeId::new("a"))).unwrap();
    assert_adjacency_consistent(&store);
    assert!(
        futures_executor::block_on(store.get_edges(EdgeQuery::default()))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn stores_graph_and_traverses_one_step() {
    let mut builder = GraphBuilder::new();
    let talk = builder.node("Talk", "talk-1").finish();
    let person = builder.node("Person", "person-1").finish();
    let _ = builder.edge("PRESENTED_BY", &talk, &person).finish();
    let graph = builder.build();

    let store = MemoryGraphStore::new();
    futures_executor::block_on(store.put_graph(&graph)).unwrap();
    let speakers = futures_executor::block_on(
        store.traverse(
            Traversal::from_node("talk-1")
                .out("PRESENTED_BY")
                .to("Person"),
        ),
    )
    .unwrap();

    assert_eq!(speakers.len(), 1);
    assert_eq!(speakers[0].id, NodeId::from("person-1"));
}

#[test]
fn typed_graph_round_trips_through_memory_store() {
    let mut builder = TypedGraphBuilder::new();
    builder
        .add_node(&Person {
            id: "ada".to_string(),
            name: "Ada".to_string(),
            skill: "math".to_string(),
        })
        .expect("typed person is valid");
    let graph = builder.build();
    let store = MemoryGraphStore::new();

    futures_executor::block_on(store.put_graph(&graph)).unwrap();
    let fetched = futures_executor::block_on(store.get_node(&NodeId::new("person:ada")))
        .unwrap()
        .expect("person node exists");
    let person = Person::from_node(&fetched).expect("typed person decodes");

    assert_eq!(person.id, "ada");
    assert_eq!(person.name, "Ada");
    assert_eq!(person.skill, "math");
}

#[test]
fn applied_schema_validates_memory_graph_writes() {
    let schema = GraphSchema::builder()
        .node("Person", vec![Field::required("name", FieldType::String)])
        .node("Project", vec![Field::required("name", FieldType::String)])
        .edge(
            "WORKS_ON",
            vec![Label::new("Person")],
            vec![Label::new("Project")],
            Vec::<Field>::new(),
        )
        .build();
    let store = MemoryGraphStore::new();
    futures_executor::block_on(store.apply_schema(&schema)).unwrap();

    let error =
        futures_executor::block_on(store.put_node(&Node::new("Person", "person-1", Props::new())))
            .expect_err("missing required field should fail");

    assert!(error.to_string().contains("missing required field 'name'"));
}

#[test]
fn node_label_updates_revalidate_incident_edge_endpoints() {
    let schema = GraphSchema::builder()
        .node("Person", Vec::<Field>::new())
        .node("Project", Vec::<Field>::new())
        .edge(
            "WORKS_ON",
            vec![Label::new("Person")],
            vec![Label::new("Project")],
            Vec::<Field>::new(),
        )
        .build();
    let store = MemoryGraphStore::new();
    futures_executor::block_on(store.apply_schema(&schema)).unwrap();
    futures_executor::block_on(store.put_graph(&Graph::new(
        vec![
            Node::new("Person", "person-1", Props::new()),
            Node::new("Project", "project-1", Props::new()),
        ],
        vec![Edge::new("WORKS_ON", "person-1", "project-1", Props::new())],
    )))
    .unwrap();

    let error =
        futures_executor::block_on(store.put_node(&Node::new("Project", "person-1", Props::new())))
            .expect_err("label update must preserve incident edge validity");
    assert!(error.to_string().contains("cannot start from node label"));
    assert_eq!(
        futures_executor::block_on(store.get_node(&NodeId::new("person-1")))
            .unwrap()
            .unwrap()
            .label,
        Label::new("Person")
    );
}

#[test]
fn memory_reports_constraint_capabilities_and_validates_constraints() {
    let required = GraphConstraint::NodePropertyRequired {
        label: Label::new("Person"),
        key: "email".to_string(),
    };
    let unique = GraphConstraint::NodePropertyUnique {
        label: Label::new("Person"),
        key: "email".to_string(),
    };
    let schema = GraphSchema::builder()
        .node("Person", Vec::<Field>::new())
        .required_node_property("Person", "email")
        .unique_node_property("Person", "email")
        .build();
    let store = MemoryGraphStore::new();

    assert_eq!(
        store.constraint_capability(&required),
        GraphConstraintCapability::ValidateBeforeWrite
    );
    assert_eq!(
        store.constraint_capability(&unique),
        GraphConstraintCapability::ValidateBeforeWrite
    );
    assert_eq!(
        store.native_constraint_capability(&required),
        GraphNativeConstraintCapability::NativeConstraint
    );
    assert_eq!(
        store.native_constraint_capability(&unique),
        GraphNativeConstraintCapability::NativeConstraint
    );

    let native_report =
        futures_executor::block_on(store.apply_native_constraint(GraphNativeConstraintRequest {
            constraint: unique.clone(),
            if_not_exists: false,
        }))
        .expect("memory applies native constraints");
    assert_eq!(
        native_report,
        GraphNativeConstraintReport {
            applied: 1,
            skipped: 0
        }
    );
    let duplicate_native =
        futures_executor::block_on(store.apply_native_constraint(GraphNativeConstraintRequest {
            constraint: unique.clone(),
            if_not_exists: true,
        }))
        .expect("if-not-exists skips duplicate native constraints");
    assert_eq!(
        duplicate_native,
        GraphNativeConstraintReport {
            applied: 0,
            skipped: 1
        }
    );
    let duplicate_error =
        futures_executor::block_on(store.apply_native_constraint(GraphNativeConstraintRequest {
            constraint: unique.clone(),
            if_not_exists: false,
        }))
        .expect_err("duplicate native constraint without if-not-exists should fail");
    assert!(
        matches!(duplicate_error, GrustError::Schema(message) if message.contains("already exists"))
    );

    futures_executor::block_on(store.apply_schema(&schema)).unwrap();
    let error =
        futures_executor::block_on(store.put_node(&Node::new("Person", "person-1", Props::new())))
            .expect_err("missing required constrained property should fail");

    assert!(
        error
            .to_string()
            .contains("missing required constrained property 'email'")
    );

    let first = Node::new(
        "Person",
        "person-1",
        Props::from([("email".to_string(), Value::from("ada@example.com"))]),
    );
    futures_executor::block_on(store.put_node(&first)).unwrap();
    assert_eq!(
        futures_executor::block_on(store.put_node(&first)).unwrap(),
        PutOutcome::Updated,
        "a unique value does not conflict with the record it replaces"
    );

    let mut duplicate_props = Props::new();
    duplicate_props.insert("email".to_string(), Value::from("ada@example.com"));
    let error = futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "person-2",
        duplicate_props,
    )))
    .expect_err("duplicate unique constrained property should fail");
    assert!(
        error
            .to_string()
            .contains("duplicates unique constrained property 'email'")
    );

    let moved = Node::new(
        "Person",
        "person-1",
        Props::from([("email".to_string(), Value::from("grace@example.com"))]),
    );
    futures_executor::block_on(store.put_node(&moved)).unwrap();
    futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "person-2",
        Props::from([("email".to_string(), Value::from("ada@example.com"))]),
    )))
    .expect("replacing a unique value releases its previous index entry");
}

#[test]
fn memory_native_constraints_validate_existing_and_future_writes() {
    let store = MemoryGraphStore::new();
    futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "person-1",
        Props::from([("email".to_string(), Value::from("ada@example.com"))]),
    )))
    .unwrap();
    futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "person-2",
        Props::from([("email".to_string(), Value::from("ada@example.com"))]),
    )))
    .unwrap();

    let duplicate_unique =
        futures_executor::block_on(store.apply_native_constraint(GraphNativeConstraintRequest {
            constraint: GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "email".to_string(),
            },
            if_not_exists: false,
        }))
        .expect_err("native unique constraint should validate existing graph");
    assert!(
        duplicate_unique
            .to_string()
            .contains("duplicates native unique constrained property 'email'")
    );

    let store = MemoryGraphStore::new();
    futures_executor::block_on(store.apply_native_constraint(GraphNativeConstraintRequest {
        constraint: GraphConstraint::NodePropertyRequired {
            label: Label::new("Person"),
            key: "email".to_string(),
        },
        if_not_exists: false,
    }))
    .unwrap();
    let missing_required =
        futures_executor::block_on(store.put_node(&Node::new("Person", "person-1", Props::new())))
            .expect_err("native required constraint should validate future writes");
    assert!(
        missing_required
            .to_string()
            .contains("missing native required constrained property 'email'")
    );
}

#[test]
fn memory_validates_unique_edge_constraints_before_writes() {
    let schema = GraphSchema::builder()
        .node("Person", Vec::<Field>::new())
        .node("Project", Vec::<Field>::new())
        .edge(
            "WORKS_ON",
            vec![Label::new("Person")],
            vec![Label::new("Project")],
            Vec::<Field>::new(),
        )
        .unique_edge_property("WORKS_ON", "role")
        .build();
    let store = MemoryGraphStore::new();

    futures_executor::block_on(store.apply_schema(&schema)).unwrap();
    futures_executor::block_on(store.put_node(&Node::new("Person", "person-1", Props::new())))
        .unwrap();
    futures_executor::block_on(store.put_node(&Node::new("Person", "person-2", Props::new())))
        .unwrap();
    futures_executor::block_on(store.put_node(&Node::new("Project", "project-1", Props::new())))
        .unwrap();
    futures_executor::block_on(store.put_node(&Node::new("Project", "project-2", Props::new())))
        .unwrap();

    let first = Edge::new(
        "WORKS_ON",
        "person-1",
        "project-1",
        Props::from([("role".to_string(), Value::from("maintainer"))]),
    );
    futures_executor::block_on(store.put_edge(&first)).unwrap();
    assert_eq!(
        futures_executor::block_on(store.put_edge(&first)).unwrap(),
        PutOutcome::Updated,
        "a unique value does not conflict with the edge it replaces"
    );

    let mut duplicate_props = Props::new();
    duplicate_props.insert("role".to_string(), Value::from("maintainer"));
    let error = futures_executor::block_on(store.put_edge(&Edge::new(
        "WORKS_ON",
        "person-2",
        "project-2",
        duplicate_props,
    )))
    .expect_err("duplicate unique constrained edge property should fail");
    assert!(
        error
            .to_string()
            .contains("duplicates unique constrained property 'role'")
    );

    let moved = Edge::new(
        "WORKS_ON",
        "person-1",
        "project-1",
        Props::from([("role".to_string(), Value::from("reviewer"))]),
    );
    futures_executor::block_on(store.put_edge(&moved)).unwrap();
    futures_executor::block_on(store.put_edge(&Edge::new(
        "WORKS_ON",
        "person-2",
        "project-2",
        Props::from([("role".to_string(), Value::from("maintainer"))]),
    )))
    .expect("replacing a unique edge value releases its previous index entry");
}

#[test]
fn memory_edge_uniqueness_uses_directed_and_undirected_endpoint_semantics() {
    let nodes = vec![
        Node::new("Person", "a", Props::new()),
        Node::new("Person", "b", Props::new()),
    ];

    let directed_schema = GraphSchema::builder()
        .node("Person", Vec::<Field>::new())
        .edge(
            "KNOWS",
            vec![Label::new("Person")],
            vec![Label::new("Person")],
            Vec::<Field>::new(),
        )
        .build();
    let directed = MemoryGraphStore::new();
    futures_executor::block_on(directed.apply_schema(&directed_schema)).unwrap();
    futures_executor::block_on(directed.put_graph(&Graph::new(nodes.clone(), Vec::new()))).unwrap();
    futures_executor::block_on(
        directed.put_edge(&Edge::new("KNOWS", "a", "b", Props::new()).with_id("first")),
    )
    .unwrap();
    futures_executor::block_on(
        directed.put_edge(&Edge::new("KNOWS", "b", "a", Props::new()).with_id("reverse")),
    )
    .expect("directed reverse endpoints are distinct");
    futures_executor::block_on(
        directed.put_edge(&Edge::new("KNOWS", "a", "b", Props::new()).with_id("duplicate")),
    )
    .expect_err("directed parallel endpoints must remain unique");

    let undirected_schema = GraphSchema::builder()
        .node("Person", Vec::<Field>::new())
        .edge_type(EdgeType {
            label: Label::new("KNOWS"),
            from: vec![Label::new("Person")],
            to: vec![Label::new("Person")],
            fields: Vec::new(),
            directed: false,
            uniqueness: EdgeUniqueness::FromLabelTo,
        })
        .build();
    let undirected = MemoryGraphStore::new();
    futures_executor::block_on(undirected.apply_schema(&undirected_schema)).unwrap();
    futures_executor::block_on(undirected.put_graph(&Graph::new(nodes, Vec::new()))).unwrap();
    futures_executor::block_on(
        undirected.put_edge(&Edge::new("KNOWS", "a", "b", Props::new()).with_id("first")),
    )
    .unwrap();
    futures_executor::block_on(
        undirected.put_edge(&Edge::new("KNOWS", "b", "a", Props::new()).with_id("reverse")),
    )
    .expect_err("undirected reverse endpoints must conflict");
}

#[test]
fn put_reports_insert_vs_update() {
    let store = MemoryGraphStore::new();
    let node = Node::new("Person", "a", Props::new());

    assert_eq!(
        futures_executor::block_on(store.put_node(&node)).unwrap(),
        PutOutcome::Inserted
    );
    assert_eq!(
        futures_executor::block_on(store.put_node(&node)).unwrap(),
        PutOutcome::Updated
    );
}

#[test]
fn preserves_parallel_edges_with_distinct_explicit_ids() {
    let store = MemoryGraphStore::new();
    futures_executor::block_on(store.put_node(&Node::new("Person", "a", Props::new()))).unwrap();
    futures_executor::block_on(store.put_node(&Node::new("Person", "b", Props::new()))).unwrap();

    let edge_1 = Edge::new(
        "KNOWS",
        "a",
        "b",
        Props::from([("since".to_string(), Value::Int(2020))]),
    )
    .with_id("edge-1");
    let edge_2 = Edge::new(
        "KNOWS",
        "a",
        "b",
        Props::from([("since".to_string(), Value::Int(2021))]),
    )
    .with_id("edge-2");

    assert_eq!(
        futures_executor::block_on(store.put_edge(&edge_1)).unwrap(),
        PutOutcome::Inserted
    );
    assert_eq!(
        futures_executor::block_on(store.put_edge(&edge_2)).unwrap(),
        PutOutcome::Inserted
    );

    let edges = futures_executor::block_on(store.get_edges(EdgeQuery {
        from: Some(NodeId::new("a")),
        to: Some(NodeId::new("b")),
        label: Some(Label::new("KNOWS")),
    }))
    .unwrap();
    assert_eq!(edges.len(), 2);
    assert!(
        edges
            .iter()
            .any(|edge| edge.id == Some(EdgeId::new("edge-1")))
    );
    assert!(
        edges
            .iter()
            .any(|edge| edge.id == Some(EdgeId::new("edge-2")))
    );

    futures_executor::block_on(store.apply_mutations(&[GraphMutation::PatchEdge {
        from: NodeId::new("a"),
        label: Label::new("KNOWS"),
        to: NodeId::new("b"),
        id: Some(EdgeId::new("edge-2")),
        props: Props::from([("seen".to_string(), Value::Bool(true))]),
    }]))
    .unwrap();

    let edges = futures_executor::block_on(store.get_edges(EdgeQuery {
        from: Some(NodeId::new("a")),
        to: Some(NodeId::new("b")),
        label: Some(Label::new("KNOWS")),
    }))
    .unwrap();
    let edge_1 = edges
        .iter()
        .find(|edge| edge.id == Some(EdgeId::new("edge-1")))
        .expect("first edge remains");
    let edge_2 = edges
        .iter()
        .find(|edge| edge.id == Some(EdgeId::new("edge-2")))
        .expect("second edge remains");
    assert_eq!(edge_1.props.get("seen"), None);
    assert_eq!(edge_2.props.get("seen"), Some(&Value::Bool(true)));

    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(
        &GraphMutationPlan::new(vec![GraphMutationPlanOp::DeleteMatchingEdges {
            relationship: GraphRelationshipMatch {
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
                id: Some(EdgeId::new("edge-2")),
                props: Props::new(),
                predicates: Vec::new(),
            },
            cardinality: GraphMutationCardinality::BoundedMany,
        }]),
    ))
    .unwrap();
    assert_eq!(
        report,
        GraphMutationReport {
            deletes: 1,
            matched_rows: 1,
            changed_edges: 1,
            edge_deletes: 1,
            ..GraphMutationReport::default()
        }
    );

    let edges = futures_executor::block_on(store.get_edges(EdgeQuery {
        from: Some(NodeId::new("a")),
        to: Some(NodeId::new("b")),
        label: Some(Label::new("KNOWS")),
    }))
    .unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].id, Some(EdgeId::new("edge-1")));
}

#[test]
fn get_nodes_reads_multiple_ids() {
    let store = MemoryGraphStore::new();
    let nodes = [
        Node::new("Person", "a", Props::new()),
        Node::new("Person", "b", Props::new()),
    ];
    futures_executor::block_on(store.put_node(&nodes[0])).unwrap();
    futures_executor::block_on(store.put_node(&nodes[1])).unwrap();

    let fetched = futures_executor::block_on(store.get_nodes(&[
        NodeId::new("b"),
        NodeId::new("missing"),
        NodeId::new("a"),
    ]))
    .unwrap();

    assert_eq!(
        fetched
            .iter()
            .map(|node| node.id.clone())
            .collect::<Vec<_>>(),
        vec![NodeId::new("b"), NodeId::new("a")]
    );
}

#[test]
fn delete_node_cascades_to_incident_edges() {
    let store = MemoryGraphStore::new();
    let mut builder = Graph::builder();
    let _ = builder.node("Person", "a").finish();
    let _ = builder.node("Person", "b").finish();
    let _ = builder.edge("KNOWS", "a", "b").finish();
    futures_executor::block_on(store.put_graph(&builder.build())).unwrap();

    futures_executor::block_on(store.delete_node(&NodeId::new("a"))).unwrap();

    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("a")))
            .unwrap()
            .is_none()
    );
    assert!(
        futures_executor::block_on(store.get_edges(EdgeQuery::default()))
            .unwrap()
            .is_empty(),
        "incident edges should be deleted"
    );
    // Deleting again is idempotent.
    futures_executor::block_on(store.delete_node(&NodeId::new("a"))).unwrap();
}

#[test]
fn apply_mutations_upserts_and_deletes() {
    let store = MemoryGraphStore::new();
    let mutations = vec![
        GraphMutation::UpsertNode(Node::new("Person", "a", Props::new())),
        GraphMutation::UpsertNode(Node::new("Person", "b", Props::new())),
        GraphMutation::UpsertEdge(Edge::new("KNOWS", "a", "b", Props::new())),
        GraphMutation::PatchNode {
            id: NodeId::new("a"),
            props: Props::from([
                ("name".to_string(), Value::from("Ada")),
                ("nickname".to_string(), Value::Null),
            ]),
        },
        GraphMutation::DeleteEdge {
            from: NodeId::new("a"),
            label: Label::new("KNOWS"),
            to: NodeId::new("b"),
        },
        GraphMutation::DeleteNode(NodeId::new("b")),
    ];

    futures_executor::block_on(store.apply_mutations(&mutations)).unwrap();

    let node = futures_executor::block_on(store.get_node(&NodeId::new("a")))
        .unwrap()
        .expect("patched node remains");
    assert_eq!(node.props.get("name"), Some(&Value::from("Ada")));
    assert_eq!(node.props.get("nickname"), Some(&Value::Null));
    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("b")))
            .unwrap()
            .is_none()
    );
    assert!(
        futures_executor::block_on(store.get_edges(EdgeQuery::default()))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn default_mutation_batches_are_ordered_but_not_atomic() {
    let schema = GraphSchema::builder()
        .node("Person", vec![Field::required("name", FieldType::String)])
        .build();
    let store = MemoryGraphStore::new();
    futures_executor::block_on(store.apply_schema(&schema)).unwrap();

    let error = futures_executor::block_on(store.apply_mutations(&[
        GraphMutation::UpsertNode(Node::new(
            "Person",
            "person-1",
            Props::from([("name".to_string(), Value::from("Ada"))]),
        )),
        GraphMutation::UpsertNode(Node::new("Person", "person-2", Props::new())),
    ]))
    .expect_err("second mutation should fail schema validation");

    assert!(error.to_string().contains("missing required field 'name'"));
    assert_eq!(
        store.mutation_atomicity(),
        GraphMutationAtomicity::OrderedNonAtomic
    );
    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("person-1")))
            .unwrap()
            .is_some(),
        "first mutation remains applied after later failure"
    );
    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("person-2")))
            .unwrap()
            .is_none()
    );
}

#[test]
fn executes_mutation_plan_with_cardinality_aware_node_changes() {
    let store = MemoryGraphStore::new();
    let plan = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new(
                "Person",
                "a",
                Props::from([("status".to_string(), Value::from("inactive"))]),
            ),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new(
                "Person",
                "b",
                Props::from([("status".to_string(), Value::from("inactive"))]),
            ),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new(
                "Person",
                "c",
                Props::from([("status".to_string(), Value::from("active"))]),
            ),
        },
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Create,
            edge: Edge::new("KNOWS", "a", "b", Props::new()),
        },
        GraphMutationPlanOp::PatchMatchingNodes {
            label: Some(Label::new("Person")),
            props: Props::from([("status".to_string(), Value::from("inactive"))]),
            predicates: Vec::new(),
            patch: Props::from([("archived".to_string(), Value::Bool(true))]),
            cardinality: GraphMutationCardinality::BoundedMany,
        },
        GraphMutationPlanOp::DeleteMatchingNodes {
            label: Some(Label::new("Person")),
            props: Props::from([("archived".to_string(), Value::Bool(true))]),
            predicates: Vec::new(),
            cardinality: GraphMutationCardinality::BoundedMany,
        },
    ]);

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
        futures_executor::block_on(store.get_node(&NodeId::new("a")))
            .unwrap()
            .is_none()
    );
    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("b")))
            .unwrap()
            .is_none()
    );
    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("c")))
            .unwrap()
            .is_some()
    );
    assert!(
        futures_executor::block_on(store.get_edges(EdgeQuery::default()))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn executes_matching_node_property_removal() {
    let store = MemoryGraphStore::new();
    let plan = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new(
                "Person",
                "a",
                Props::from([
                    ("status".to_string(), Value::from("inactive")),
                    ("nickname".to_string(), Value::from("ada")),
                ]),
            ),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new(
                "Person",
                "b",
                Props::from([
                    ("status".to_string(), Value::from("inactive")),
                    ("nickname".to_string(), Value::from("bob")),
                ]),
            ),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new(
                "Person",
                "c",
                Props::from([
                    ("status".to_string(), Value::from("active")),
                    ("nickname".to_string(), Value::from("charlie")),
                ]),
            ),
        },
        GraphMutationPlanOp::RemoveMatchingNodeProps {
            label: Some(Label::new("Person")),
            props: Props::from([("status".to_string(), Value::from("inactive"))]),
            predicates: Vec::new(),
            keys: vec!["nickname".to_string()],
            cardinality: GraphMutationCardinality::BoundedMany,
        },
    ]);

    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&plan)).unwrap();

    assert_eq!(
        report,
        GraphMutationReport {
            creates: 3,
            property_removes: 1,
            matched_rows: 2,
            changed_nodes: 5,
            node_upserts: 3,
            node_property_removes: 2,
            node_inserts: 3,
            ..GraphMutationReport::default()
        }
    );
    for id in ["a", "b"] {
        let node = futures_executor::block_on(store.get_node(&NodeId::new(id)))
            .unwrap()
            .expect("matched node exists");
        assert_eq!(node.props.get("nickname"), None);
    }
    let active = futures_executor::block_on(store.get_node(&NodeId::new("c")))
        .unwrap()
        .expect("active node exists");
    assert_eq!(active.props.get("nickname"), Some(&Value::from("charlie")));

    let zero = GraphMutationPlan::new(vec![GraphMutationPlanOp::RemoveMatchingNodeProps {
        label: Some(Label::new("Person")),
        props: Props::from([("status".to_string(), Value::from("missing"))]),
        predicates: Vec::new(),
        keys: vec!["nickname".to_string()],
        cardinality: GraphMutationCardinality::BoundedMany,
    }]);
    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&zero)).unwrap();
    assert_eq!(
        report,
        GraphMutationReport {
            property_removes: 1,
            ..GraphMutationReport::default()
        }
    );
}

#[test]
fn matching_node_mutations_filter_property_predicates() {
    let store = MemoryGraphStore::new();
    let plan = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new(
                "Person",
                "a",
                Props::from([
                    ("status".to_string(), Value::from("inactive")),
                    ("score".to_string(), Value::Int(11)),
                    ("nickname".to_string(), Value::from("ada")),
                ]),
            ),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new(
                "Person",
                "b",
                Props::from([
                    ("status".to_string(), Value::from("inactive")),
                    ("score".to_string(), Value::Int(9)),
                    ("nickname".to_string(), Value::Null),
                ]),
            ),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new(
                "Person",
                "c",
                Props::from([("status".to_string(), Value::from("inactive"))]),
            ),
        },
        GraphMutationPlanOp::PatchMatchingNodes {
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
                    key: "nickname".to_string(),
                    op: GraphPredicateOp::NotEqual,
                    value: Value::Null,
                },
            ],
            patch: Props::from([("archived".to_string(), Value::Bool(true))]),
            cardinality: GraphMutationCardinality::BoundedMany,
        },
    ]);

    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&plan)).unwrap();

    assert_eq!(
        report,
        GraphMutationReport {
            creates: 3,
            patches: 1,
            matched_rows: 1,
            changed_nodes: 4,
            node_upserts: 3,
            node_patches: 1,
            node_inserts: 3,
            ..GraphMutationReport::default()
        }
    );
    let archived = futures_executor::block_on(store.get_node(&NodeId::new("a")))
        .unwrap()
        .expect("matched node exists");
    assert_eq!(archived.props.get("archived"), Some(&Value::Bool(true)));
    for id in ["b", "c"] {
        let node = futures_executor::block_on(store.get_node(&NodeId::new(id)))
            .unwrap()
            .expect("unmatched node exists");
        assert_eq!(node.props.get("archived"), None);
    }
}

#[test]
fn row_producing_edge_create_matches_endpoint_nodes() {
    let store = MemoryGraphStore::new();
    let plan = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new(
                "Person",
                "ada",
                Props::from([
                    ("status".to_string(), Value::from("active")),
                    ("score".to_string(), Value::Int(11)),
                ]),
            ),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new(
                "Person",
                "bob",
                Props::from([
                    ("status".to_string(), Value::from("active")),
                    ("score".to_string(), Value::Int(9)),
                ]),
            ),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new("Team", "eng", Props::new()),
        },
        GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
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
                props: Props::from([("id".to_string(), Value::from("eng"))]),
                predicates: Vec::new(),
            },
            label: Label::new("MEMBER_OF"),
            props: Props::from([("source".to_string(), Value::from("cypher"))]),
            edge_id_policy: GraphRowEdgeIdPolicy::ExplicitOnly,
            cardinality: GraphMutationCardinality::BoundedMany,
        },
    ]);

    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&plan)).unwrap();

    assert_eq!(
        report,
        GraphMutationReport {
            creates: 4,
            matched_rows: 1,
            changed_nodes: 3,
            changed_edges: 1,
            node_upserts: 3,
            edge_upserts: 1,
            node_inserts: 3,
            edge_inserts: 1,
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
    assert_eq!(edges[0].props.get("source"), Some(&Value::from("cypher")));
    assert!(
        futures_executor::block_on(store.get_edges(EdgeQuery {
            from: Some(NodeId::new("bob")),
            to: Some(NodeId::new("eng")),
            label: Some(Label::new("MEMBER_OF")),
        }))
        .unwrap()
        .is_empty()
    );

    let merge = GraphMutationPlan::new(vec![GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
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
            props: Props::from([("id".to_string(), Value::from("eng"))]),
            predicates: Vec::new(),
        },
        label: Label::new("MEMBER_OF"),
        props: Props::from([("source".to_string(), Value::from("merge"))]),
        edge_id_policy: GraphRowEdgeIdPolicy::ExplicitOnly,
        cardinality: GraphMutationCardinality::BoundedMany,
    }]);
    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&merge)).unwrap();
    assert_eq!(
        report,
        GraphMutationReport {
            merges: 1,
            matched_rows: 1,
            changed_edges: 1,
            edge_upserts: 1,
            // The MEMBER_OF edge already exists from the earlier CREATE, so the
            // row-producing MERGE updates rather than inserts it.
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

    let generated_props = Props::from([("source".to_string(), Value::from("generated"))]);
    let generated = GraphMutationPlan::new(vec![GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
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
            props: Props::from([("id".to_string(), Value::from("eng"))]),
            predicates: Vec::new(),
        },
        label: Label::new("GENERATED_MEMBER_OF"),
        props: generated_props.clone(),
        edge_id_policy: GraphRowEdgeIdPolicy::GenerateForCreate,
        cardinality: GraphMutationCardinality::BoundedMany,
    }]);
    let report =
        futures_executor::block_on(store.execute_cypher_mutation_plan(&generated)).unwrap();
    assert_eq!(
        report,
        GraphMutationReport {
            creates: 1,
            matched_rows: 1,
            changed_edges: 1,
            edge_upserts: 1,
            edge_inserts: 1,
            ..GraphMutationReport::default()
        }
    );
    let edges = futures_executor::block_on(store.get_edges(EdgeQuery {
        from: Some(NodeId::new("ada")),
        to: Some(NodeId::new("eng")),
        label: Some(Label::new("GENERATED_MEMBER_OF")),
    }))
    .unwrap();
    assert_eq!(
        edges[0].id,
        Some(generated_row_edge_id(
            &NodeId::new("ada"),
            &Label::new("GENERATED_MEMBER_OF"),
            &NodeId::new("eng"),
            &generated_props
        ))
    );

    let generated_merge_props =
        Props::from([("source".to_string(), Value::from("generated-merge"))]);
    let generated_merge =
        GraphMutationPlan::new(vec![GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
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
                props: Props::from([("id".to_string(), Value::from("eng"))]),
                predicates: Vec::new(),
            },
            label: Label::new("GENERATED_MERGE_MEMBER_OF"),
            props: generated_merge_props.clone(),
            edge_id_policy: GraphRowEdgeIdPolicy::GenerateForCreateAndMerge,
            cardinality: GraphMutationCardinality::BoundedMany,
        }]);
    let report =
        futures_executor::block_on(store.execute_cypher_mutation_plan(&generated_merge)).unwrap();
    assert_eq!(
        report,
        GraphMutationReport {
            merges: 1,
            matched_rows: 1,
            changed_edges: 1,
            edge_upserts: 1,
            edge_inserts: 1,
            ..GraphMutationReport::default()
        }
    );
    let report =
        futures_executor::block_on(store.execute_cypher_mutation_plan(&generated_merge)).unwrap();
    assert_eq!(
        report,
        GraphMutationReport {
            merges: 1,
            matched_rows: 1,
            changed_edges: 1,
            edge_upserts: 1,
            edge_updates: 1,
            ..GraphMutationReport::default()
        }
    );
}

#[test]
fn executes_matching_node_numeric_property_updates() {
    let store = MemoryGraphStore::new();
    let plan = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new(
                "Counter",
                "a",
                Props::from([
                    ("active".to_string(), Value::Bool(true)),
                    ("count".to_string(), Value::Int(1)),
                ]),
            ),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new(
                "Counter",
                "b",
                Props::from([
                    ("active".to_string(), Value::Bool(true)),
                    ("count".to_string(), Value::Int(4)),
                ]),
            ),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new(
                "Counter",
                "c",
                Props::from([
                    ("active".to_string(), Value::Bool(false)),
                    ("count".to_string(), Value::Int(10)),
                ]),
            ),
        },
        GraphMutationPlanOp::UpdateMatchingNodeProperty {
            label: Some(Label::new("Counter")),
            props: Props::from([("active".to_string(), Value::Bool(true))]),
            predicates: Vec::new(),
            target_key: "count".to_string(),
            source_key: "count".to_string(),
            op: GraphNumericOp::Add,
            operand: Value::Int(2),
            cardinality: GraphMutationCardinality::BoundedMany,
        },
        GraphMutationPlanOp::UpdateMatchingNodeProperty {
            label: None,
            props: Props::from([("id".to_string(), Value::from("a"))]),
            predicates: Vec::new(),
            target_key: "half".to_string(),
            source_key: "count".to_string(),
            op: GraphNumericOp::Divide,
            operand: Value::Int(2),
            cardinality: GraphMutationCardinality::SingleIdentity,
        },
    ]);

    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&plan)).unwrap();

    assert_eq!(
        report,
        GraphMutationReport {
            creates: 3,
            patches: 2,
            matched_rows: 3,
            changed_nodes: 6,
            node_upserts: 3,
            node_patches: 3,
            node_inserts: 3,
            ..GraphMutationReport::default()
        }
    );
    let a = futures_executor::block_on(store.get_node(&NodeId::new("a")))
        .unwrap()
        .expect("counter a exists");
    assert_eq!(a.props.get("count"), Some(&Value::Int(3)));
    assert_eq!(a.props.get("half"), Some(&Value::Float(1.5)));
    let b = futures_executor::block_on(store.get_node(&NodeId::new("b")))
        .unwrap()
        .expect("counter b exists");
    assert_eq!(b.props.get("count"), Some(&Value::Int(6)));
    let c = futures_executor::block_on(store.get_node(&NodeId::new("c")))
        .unwrap()
        .expect("counter c exists");
    assert_eq!(c.props.get("count"), Some(&Value::Int(10)));

    let zero = GraphMutationPlan::new(vec![GraphMutationPlanOp::UpdateMatchingNodeProperty {
        label: Some(Label::new("Counter")),
        props: Props::from([("active".to_string(), Value::from("missing"))]),
        predicates: Vec::new(),
        target_key: "count".to_string(),
        source_key: "count".to_string(),
        op: GraphNumericOp::Add,
        operand: Value::Int(1),
        cardinality: GraphMutationCardinality::BoundedMany,
    }]);
    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&zero)).unwrap();
    assert_eq!(
        report,
        GraphMutationReport {
            patches: 1,
            ..GraphMutationReport::default()
        }
    );
}

#[test]
fn matching_node_numeric_property_updates_fail_before_writing_invalid_values() {
    let store = MemoryGraphStore::new();
    futures_executor::block_on(store.put_node(&Node::new("Counter", "missing", Props::new())))
        .unwrap();
    futures_executor::block_on(store.put_node(&Node::new(
        "Counter",
        "null",
        Props::from([("count".to_string(), Value::Null)]),
    )))
    .unwrap();
    futures_executor::block_on(store.put_node(&Node::new(
        "Counter",
        "string",
        Props::from([("count".to_string(), Value::from("one"))]),
    )))
    .unwrap();
    futures_executor::block_on(store.put_node(&Node::new(
        "Counter",
        "overflow",
        Props::from([("count".to_string(), Value::Int(i64::MAX))]),
    )))
    .unwrap();

    for (id, expected) in [
        ("missing", "missing"),
        ("null", "null"),
        ("string", "integer or float"),
        ("overflow", "overflow"),
    ] {
        let plan = GraphMutationPlan::new(vec![GraphMutationPlanOp::UpdateMatchingNodeProperty {
            label: None,
            props: Props::from([("id".to_string(), Value::from(id))]),
            predicates: Vec::new(),
            target_key: "count".to_string(),
            source_key: "count".to_string(),
            op: GraphNumericOp::Add,
            operand: Value::Int(1),
            cardinality: GraphMutationCardinality::SingleIdentity,
        }]);
        let error = futures_executor::block_on(store.execute_cypher_mutation_plan(&plan))
            .expect_err("invalid numeric update should fail");
        assert!(
            error.to_string().contains(expected),
            "expected error containing {expected:?}, got {error}"
        );
    }

    let overflow = futures_executor::block_on(store.get_node(&NodeId::new("overflow")))
        .unwrap()
        .expect("overflow node remains");
    assert_eq!(overflow.props.get("count"), Some(&Value::Int(i64::MAX)));
}

#[test]
fn executes_matching_edge_mutations() {
    let store = MemoryGraphStore::new();
    let relationship = GraphRelationshipMatch {
        from: GraphNodeMatch {
            label: Some(Label::new("Person")),
            props: Props::from([("id".to_string(), Value::from("a"))]),
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
    let setup = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new("Person", "a", Props::new()),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new(
                "Person",
                "b",
                Props::from([("status".to_string(), Value::from("inactive"))]),
            ),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new(
                "Person",
                "c",
                Props::from([("status".to_string(), Value::from("active"))]),
            ),
        },
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Create,
            edge: Edge::new(
                "KNOWS",
                "a",
                "b",
                Props::from([("note".to_string(), Value::from("old"))]),
            ),
        },
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Create,
            edge: Edge::new(
                "KNOWS",
                "a",
                "c",
                Props::from([("note".to_string(), Value::from("keep"))]),
            ),
        },
        GraphMutationPlanOp::PatchMatchingEdges {
            relationship: relationship.clone(),
            patch: Props::from([("seen".to_string(), Value::Bool(true))]),
            cardinality: GraphMutationCardinality::BoundedMany,
        },
        GraphMutationPlanOp::RemoveMatchingEdgeProps {
            relationship: relationship.clone(),
            keys: vec!["note".to_string()],
            cardinality: GraphMutationCardinality::BoundedMany,
        },
    ]);

    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&setup)).unwrap();

    assert_eq!(
        report,
        GraphMutationReport {
            creates: 5,
            patches: 1,
            property_removes: 1,
            matched_rows: 2,
            changed_nodes: 3,
            changed_edges: 4,
            node_upserts: 3,
            edge_upserts: 2,
            edge_patches: 1,
            edge_property_removes: 1,
            node_inserts: 3,
            edge_inserts: 2,
            ..GraphMutationReport::default()
        }
    );
    let patched = futures_executor::block_on(store.get_edges(EdgeQuery {
        from: Some(NodeId::new("a")),
        to: Some(NodeId::new("b")),
        label: Some(Label::new("KNOWS")),
    }))
    .unwrap();
    assert_eq!(patched.len(), 1);
    assert_eq!(patched[0].props.get("seen"), Some(&Value::Bool(true)));
    assert_eq!(patched[0].props.get("note"), None);
    let untouched = futures_executor::block_on(store.get_edges(EdgeQuery {
        from: Some(NodeId::new("a")),
        to: Some(NodeId::new("c")),
        label: Some(Label::new("KNOWS")),
    }))
    .unwrap();
    assert_eq!(untouched[0].props.get("note"), Some(&Value::from("keep")));

    let delete = GraphMutationPlan::new(vec![GraphMutationPlanOp::DeleteMatchingEdges {
        relationship,
        cardinality: GraphMutationCardinality::BoundedMany,
    }]);
    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&delete)).unwrap();
    assert_eq!(
        report,
        GraphMutationReport {
            deletes: 1,
            matched_rows: 1,
            changed_edges: 1,
            edge_deletes: 1,
            ..GraphMutationReport::default()
        }
    );
    assert!(
        futures_executor::block_on(store.get_edges(EdgeQuery {
            from: Some(NodeId::new("a")),
            to: Some(NodeId::new("b")),
            label: Some(Label::new("KNOWS")),
        }))
        .unwrap()
        .is_empty()
    );
    assert_adjacency_consistent(&store);
}

#[test]
fn matching_edge_mutations_filter_relationship_properties() {
    let store = MemoryGraphStore::new();
    let setup = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new("Person", "a", Props::new()),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new("Person", "b", Props::new()),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new("Person", "c", Props::new()),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new("Person", "d", Props::new()),
        },
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Create,
            edge: Edge::new(
                "KNOWS",
                "a",
                "b",
                Props::from([
                    ("active".to_string(), Value::Bool(true)),
                    ("since".to_string(), Value::Int(2020)),
                ]),
            ),
        },
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Create,
            edge: Edge::new(
                "KNOWS",
                "a",
                "c",
                Props::from([
                    ("active".to_string(), Value::Bool(true)),
                    ("since".to_string(), Value::from("2020")),
                ]),
            )
            .with_id("edge-c"),
        },
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Create,
            edge: Edge::new(
                "KNOWS",
                "a",
                "d",
                Props::from([
                    ("active".to_string(), Value::Bool(false)),
                    ("since".to_string(), Value::Int(2020)),
                ]),
            ),
        },
    ]);
    futures_executor::block_on(store.execute_cypher_mutation_plan(&setup)).unwrap();

    let one_int_match = GraphRelationshipMatch {
        from: GraphNodeMatch {
            label: Some(Label::new("Person")),
            props: Props::from([("id".to_string(), Value::from("a"))]),
            predicates: Vec::new(),
        },
        label: Label::new("KNOWS"),
        to: GraphNodeMatch {
            label: Some(Label::new("Person")),
            props: Props::new(),
            predicates: Vec::new(),
        },
        id: None,
        props: Props::from([
            ("active".to_string(), Value::Bool(true)),
            ("since".to_string(), Value::Int(2020)),
        ]),
        predicates: Vec::new(),
    };
    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(
        &GraphMutationPlan::new(vec![GraphMutationPlanOp::PatchMatchingEdges {
            relationship: one_int_match,
            patch: Props::from([("seen".to_string(), Value::Bool(true))]),
            cardinality: GraphMutationCardinality::BoundedMany,
        }]),
    ))
    .unwrap();
    assert_eq!(
        report,
        GraphMutationReport {
            patches: 1,
            matched_rows: 1,
            changed_edges: 1,
            edge_patches: 1,
            ..GraphMutationReport::default()
        }
    );

    let b_edges = futures_executor::block_on(store.get_edges(EdgeQuery {
        from: Some(NodeId::new("a")),
        to: Some(NodeId::new("b")),
        label: Some(Label::new("KNOWS")),
    }))
    .unwrap();
    assert_eq!(b_edges[0].props.get("seen"), Some(&Value::Bool(true)));
    let c_edges = futures_executor::block_on(store.get_edges(EdgeQuery {
        from: Some(NodeId::new("a")),
        to: Some(NodeId::new("c")),
        label: Some(Label::new("KNOWS")),
    }))
    .unwrap();
    assert_eq!(c_edges[0].props.get("seen"), None);

    let explicit_id_and_string_prop = GraphRelationshipMatch {
        from: GraphNodeMatch {
            label: None,
            props: Props::from([("id".to_string(), Value::from("a"))]),
            predicates: Vec::new(),
        },
        label: Label::new("KNOWS"),
        to: GraphNodeMatch {
            label: None,
            props: Props::new(),
            predicates: Vec::new(),
        },
        id: Some(EdgeId::new("edge-c")),
        props: Props::from([("since".to_string(), Value::from("2020"))]),
        predicates: Vec::new(),
    };
    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(
        &GraphMutationPlan::new(vec![GraphMutationPlanOp::RemoveMatchingEdgeProps {
            relationship: explicit_id_and_string_prop,
            keys: vec!["active".to_string()],
            cardinality: GraphMutationCardinality::BoundedMany,
        }]),
    ))
    .unwrap();
    assert_eq!(
        report,
        GraphMutationReport {
            property_removes: 1,
            matched_rows: 1,
            changed_edges: 1,
            edge_property_removes: 1,
            ..GraphMutationReport::default()
        }
    );
    let c_edges = futures_executor::block_on(store.get_edges(EdgeQuery {
        from: Some(NodeId::new("a")),
        to: Some(NodeId::new("c")),
        label: Some(Label::new("KNOWS")),
    }))
    .unwrap();
    assert_eq!(c_edges[0].props.get("active"), None);

    let zero = GraphRelationshipMatch {
        from: GraphNodeMatch {
            label: None,
            props: Props::new(),
            predicates: Vec::new(),
        },
        label: Label::new("KNOWS"),
        to: GraphNodeMatch {
            label: None,
            props: Props::new(),
            predicates: Vec::new(),
        },
        id: Some(EdgeId::new("edge-c")),
        props: Props::from([("since".to_string(), Value::Int(2020))]),
        predicates: Vec::new(),
    };
    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(
        &GraphMutationPlan::new(vec![GraphMutationPlanOp::DeleteMatchingEdges {
            relationship: zero,
            cardinality: GraphMutationCardinality::BoundedMany,
        }]),
    ))
    .unwrap();
    assert_eq!(
        report,
        GraphMutationReport {
            deletes: 1,
            ..GraphMutationReport::default()
        }
    );
}

#[test]
fn executor_classifies_inserts_and_updates_precisely() {
    let store = MemoryGraphStore::new();
    // First write inserts the node and the edge.
    let insert = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new("Person", "p1", Props::new()),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new("Person", "p2", Props::new()),
        },
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Create,
            edge: Edge::new("KNOWS", "p1", "p2", Props::new()),
        },
    ]);
    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&insert)).unwrap();
    assert_eq!(report.node_inserts, 2);
    assert_eq!(report.node_updates, 0);
    assert_eq!(report.edge_inserts, 1);
    assert_eq!(report.edge_updates, 0);

    // A MERGE over the same identities updates rather than inserts.
    let merge = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Merge,
            node: Node::new(
                "Person",
                "p1",
                Props::from([("seen".to_string(), Value::Bool(true))]),
            ),
        },
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Merge,
            edge: Edge::new(
                "KNOWS",
                "p1",
                "p2",
                Props::from([("weight".to_string(), Value::Int(1))]),
            ),
        },
    ]);
    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&merge)).unwrap();
    assert_eq!(report.node_inserts, 0);
    assert_eq!(report.node_updates, 1);
    assert_eq!(report.edge_inserts, 0);
    assert_eq!(report.edge_updates, 1);
    assert_adjacency_consistent(&store);
}
