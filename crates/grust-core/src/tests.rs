use super::*;

#[test]
fn builder_dedupes_nodes_and_edges() {
    let mut builder = GraphBuilder::new();
    let talk = builder
        .node("Talk", "talk-1")
        .prop("title", "A Talk")
        .finish();
    let person = builder.node("Person", "person-1").finish();
    let _ = builder
        .node("Talk", "talk-1")
        .prop("description", "Updated")
        .finish();
    let _ = builder.edge("PRESENTS", &person, &talk).finish();
    let _ = builder.edge("PRESENTS", &person, &talk).finish();

    let graph = builder.build();

    assert_eq!(graph.nodes.len(), 2);
    assert_eq!(graph.edges.len(), 1);
    assert!(graph.nodes.iter().any(|node| {
        node.id == NodeId::from("talk-1")
            && node.props.get("id") == Some(&Value::from("talk-1"))
            && node.props.get("description") == Some(&Value::from("Updated"))
    }));
}

#[test]
fn graph_index_validates_and_indexes_adjacency() {
    let graph = Graph::new(
        vec![
            Node::new("Person", "a", Props::new()),
            Node::new("Person", "b", Props::new()),
            Node::new("Person", "c", Props::new()),
        ],
        vec![
            Edge::new("KNOWS", "a", "b", Props::new()),
            Edge::new("KNOWS", "a", "c", Props::new()),
            Edge::new("KNOWS", "c", "a", Props::new()),
        ],
    );

    let index = GraphIndex::new(&graph).expect("valid graph index");

    assert_eq!(index.vertex_index(&NodeId::new("a")), Some(0));
    assert_eq!(index.outgoing_edges(&NodeId::new("a")), &[0, 1]);
    assert_eq!(index.incoming_edges(&NodeId::new("a")), &[2]);
    assert_eq!(index.outgoing_by_vertex(0), &[0, 1]);
    assert_eq!(index.incoming_by_vertex(0), &[2]);
    assert_eq!(index.edge_endpoints(1), (0, 2));
    assert_eq!(index.out_degree(0), 2);
    assert_eq!(index.in_degree(0), 1);
    assert_eq!(index.degree(0), 3);
}

#[test]
fn graph_index_rejects_missing_edge_endpoints() {
    let graph = Graph::new(
        vec![Node::new("Person", "a", Props::new())],
        vec![Edge::new("KNOWS", "a", "missing", Props::new())],
    );

    let error = GraphIndex::new(&graph).expect_err("missing endpoint must fail");

    assert!(error.to_string().contains("edge destination 'missing'"));
}

#[test]
fn normalizes_relationship_types() {
    assert_eq!(relationship_type("presents"), "PRESENTS");
    assert_eq!(relationship_type("member-of"), "MEMBER_OF");
    assert_eq!(relationship_type(""), "RELATED_TO");
}

#[test]
fn normalizes_schema_identifiers() {
    assert_eq!(
        schema_identifier("Person Profile").unwrap(),
        "person_profile"
    );
    assert!(schema_identifier("1bad").is_err());
}

#[test]
fn edge_key_prefers_explicit_id_then_structural_identity() {
    let edge = Edge::new("KNOWS", "a", "b", Props::new()).with_id("edge-1");
    assert_eq!(edge_key(&edge), "edge-1");

    let edge = Edge::new("KNOWS", "a", "b", Props::new());
    assert_eq!(edge_key(&edge), "a\u{1f}KNOWS\u{1f}b");
}

#[test]
fn mutation_plan_reports_and_lowers_to_graph_mutations() {
    let node = Node::new("Person", "person-1", Props::new());
    let edge = Edge::new("KNOWS", "person-1", "person-2", Props::new()).with_id("edge-1");
    let relationship = GraphRelationshipMatch {
        from: GraphNodeMatch {
            label: Some(Label::new("Person")),
            props: Props::from([("status".to_string(), Value::from("active"))]),
            predicates: Vec::new(),
        },
        label: Label::new("KNOWS"),
        to: GraphNodeMatch {
            label: Some(Label::new("Person")),
            props: Props::new(),
            predicates: Vec::new(),
        },
        id: None,
        props: Props::new(),
        predicates: Vec::new(),
    };
    let plan = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: node.clone(),
        },
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Merge,
            edge: edge.clone(),
        },
        GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
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
            props: Props::new(),
            cardinality: GraphMutationCardinality::BoundedMany,
        },
        GraphMutationPlanOp::PatchNode {
            id: NodeId::new("person-1"),
            props: Props::from([("name".to_string(), Value::from("Ada"))]),
        },
        GraphMutationPlanOp::PatchMatchingNodes {
            label: Some(Label::new("Person")),
            props: Props::from([("active".to_string(), Value::Bool(false))]),
            predicates: Vec::new(),
            patch: Props::from([("archived".to_string(), Value::Bool(true))]),
            cardinality: GraphMutationCardinality::BoundedMany,
        },
        GraphMutationPlanOp::UpdateMatchingNodeProperty {
            label: Some(Label::new("Counter")),
            props: Props::from([("active".to_string(), Value::Bool(true))]),
            predicates: Vec::new(),
            target_key: "count".to_string(),
            source_key: "count".to_string(),
            op: GraphNumericOp::Add,
            operand: Value::Int(1),
            cardinality: GraphMutationCardinality::BoundedMany,
        },
        GraphMutationPlanOp::PatchEdge {
            from: NodeId::new("person-1"),
            label: Label::new("KNOWS"),
            to: NodeId::new("person-2"),
            id: Some(EdgeId::new("edge-1")),
            props: Props::from([("since".to_string(), Value::Int(2026))]),
        },
        GraphMutationPlanOp::PatchMatchingEdges {
            relationship: relationship.clone(),
            patch: Props::from([("seen".to_string(), Value::Bool(true))]),
            cardinality: GraphMutationCardinality::BoundedMany,
        },
        GraphMutationPlanOp::RemoveNodeProps {
            id: NodeId::new("person-1"),
            keys: vec!["nickname".to_string()],
        },
        GraphMutationPlanOp::RemoveMatchingNodeProps {
            label: Some(Label::new("Person")),
            props: Props::from([("active".to_string(), Value::Bool(false))]),
            predicates: Vec::new(),
            keys: vec!["nickname".to_string()],
            cardinality: GraphMutationCardinality::BoundedMany,
        },
        GraphMutationPlanOp::RemoveEdgeProps {
            from: NodeId::new("person-1"),
            label: Label::new("KNOWS"),
            to: NodeId::new("person-2"),
            id: Some(EdgeId::new("edge-1")),
            keys: vec!["note".to_string()],
        },
        GraphMutationPlanOp::RemoveMatchingEdgeProps {
            relationship: relationship.clone(),
            keys: vec!["note".to_string()],
            cardinality: GraphMutationCardinality::BoundedMany,
        },
        GraphMutationPlanOp::DeleteMatchingNodes {
            label: Some(Label::new("Person")),
            props: Props::from([("active".to_string(), Value::Bool(false))]),
            predicates: Vec::new(),
            cardinality: GraphMutationCardinality::BoundedMany,
        },
        GraphMutationPlanOp::DeleteMatchingEdges {
            relationship: relationship.clone(),
            cardinality: GraphMutationCardinality::BoundedMany,
        },
        GraphMutationPlanOp::DeleteEdge {
            from: NodeId::new("person-1"),
            label: Label::new("KNOWS"),
            to: NodeId::new("person-2"),
        },
        GraphMutationPlanOp::DeleteNode(NodeId::new("person-3")),
    ]);

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            creates: 2,
            merges: 1,
            deletes: 4,
            patches: 5,
            property_removes: 4,
            matched_rows: 0,
            changed_nodes: 4,
            changed_edges: 4,
            node_upserts: 1,
            edge_upserts: 1,
            node_deletes: 1,
            edge_deletes: 1,
            node_patches: 1,
            edge_patches: 1,
            node_property_removes: 1,
            edge_property_removes: 1,
        }
    );
    assert_eq!(
        plan.into_mutations(),
        vec![
            GraphMutation::UpsertNode(node),
            GraphMutation::UpsertEdge(edge),
            GraphMutation::UpsertEdgesFromNodeMatches {
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
                props: Props::new(),
            },
            GraphMutation::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([("name".to_string(), Value::from("Ada"))]),
            },
            GraphMutation::PatchMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::from([("active".to_string(), Value::Bool(false))]),
                predicates: Vec::new(),
                patch: Props::from([("archived".to_string(), Value::Bool(true))]),
            },
            GraphMutation::UpdateMatchingNodeProperty {
                label: Some(Label::new("Counter")),
                props: Props::from([("active".to_string(), Value::Bool(true))]),
                predicates: Vec::new(),
                target_key: "count".to_string(),
                source_key: "count".to_string(),
                op: GraphNumericOp::Add,
                operand: Value::Int(1),
            },
            GraphMutation::PatchEdge {
                from: NodeId::new("person-1"),
                label: Label::new("KNOWS"),
                to: NodeId::new("person-2"),
                id: Some(EdgeId::new("edge-1")),
                props: Props::from([("since".to_string(), Value::Int(2026))]),
            },
            GraphMutation::PatchMatchingEdges {
                relationship: relationship.clone(),
                patch: Props::from([("seen".to_string(), Value::Bool(true))]),
            },
            GraphMutation::RemoveNodeProps {
                id: NodeId::new("person-1"),
                keys: vec!["nickname".to_string()],
            },
            GraphMutation::RemoveMatchingNodeProps {
                label: Some(Label::new("Person")),
                props: Props::from([("active".to_string(), Value::Bool(false))]),
                predicates: Vec::new(),
                keys: vec!["nickname".to_string()],
            },
            GraphMutation::RemoveEdgeProps {
                from: NodeId::new("person-1"),
                label: Label::new("KNOWS"),
                to: NodeId::new("person-2"),
                id: Some(EdgeId::new("edge-1")),
                keys: vec!["note".to_string()],
            },
            GraphMutation::RemoveMatchingEdgeProps {
                relationship: relationship.clone(),
                keys: vec!["note".to_string()],
            },
            GraphMutation::DeleteMatchingNodes {
                label: Some(Label::new("Person")),
                props: Props::from([("active".to_string(), Value::Bool(false))]),
                predicates: Vec::new(),
            },
            GraphMutation::DeleteMatchingEdges { relationship },
            GraphMutation::DeleteEdge {
                from: NodeId::new("person-1"),
                label: Label::new("KNOWS"),
                to: NodeId::new("person-2"),
            },
            GraphMutation::DeleteNode(NodeId::new("person-3")),
        ]
    );
}

#[test]
fn property_predicates_are_type_aware_and_missing_safe() {
    let active = GraphPropertyPredicate {
        key: "status".to_string(),
        op: GraphPredicateOp::Equal,
        value: Value::from("active"),
    };
    assert!(active.matches(Some(&Value::from("active"))));
    assert!(!active.matches(Some(&Value::from("inactive"))));
    assert!(!active.matches(None));

    let not_null = GraphPropertyPredicate {
        key: "nickname".to_string(),
        op: GraphPredicateOp::NotEqual,
        value: Value::Null,
    };
    assert!(not_null.matches(Some(&Value::from("ada"))));
    assert!(!not_null.matches(Some(&Value::Null)));
    assert!(!not_null.matches(None));

    let score_at_least = GraphPropertyPredicate {
        key: "score".to_string(),
        op: GraphPredicateOp::GreaterThanOrEqual,
        value: Value::Float(10.5),
    };
    assert!(score_at_least.matches(Some(&Value::Int(11))));
    assert!(score_at_least.matches(Some(&Value::Float(10.5))));
    assert!(!score_at_least.matches(Some(&Value::Int(10))));
    assert!(!score_at_least.matches(Some(&Value::from("11"))));

    let name_before = GraphPropertyPredicate {
        key: "name".to_string(),
        op: GraphPredicateOp::LessThan,
        value: Value::from("M"),
    };
    assert!(name_before.matches(Some(&Value::from("Ada"))));
    assert!(!name_before.matches(Some(&Value::from("Zoe"))));
}

#[test]
fn numeric_update_evaluator_is_type_checked_and_overflow_safe() {
    assert_eq!(
        evaluate_numeric_update(&Value::Int(2), GraphNumericOp::Add, &Value::Int(3)).unwrap(),
        Value::Int(5)
    );
    assert_eq!(
        evaluate_numeric_update(&Value::Int(5), GraphNumericOp::Subtract, &Value::Int(3)).unwrap(),
        Value::Int(2)
    );
    assert_eq!(
        evaluate_numeric_update(&Value::Int(4), GraphNumericOp::Multiply, &Value::Int(3)).unwrap(),
        Value::Int(12)
    );
    assert_eq!(
        evaluate_numeric_update(&Value::Int(5), GraphNumericOp::Divide, &Value::Int(2)).unwrap(),
        Value::Float(2.5)
    );
    assert_eq!(
        evaluate_numeric_update(&Value::Float(1.5), GraphNumericOp::Add, &Value::Int(2)).unwrap(),
        Value::Float(3.5)
    );

    let overflow =
        evaluate_numeric_update(&Value::Int(i64::MAX), GraphNumericOp::Add, &Value::Int(1))
            .expect_err("integer overflow should fail");
    assert!(matches!(overflow, GrustError::CypherExecution(_)));
    assert!(overflow.to_string().contains("overflow"));

    let division_by_zero =
        evaluate_numeric_update(&Value::Int(1), GraphNumericOp::Divide, &Value::Int(0))
            .expect_err("division by zero should fail");
    assert!(division_by_zero.to_string().contains("division by zero"));

    let null = evaluate_numeric_update(&Value::Null, GraphNumericOp::Add, &Value::Int(1))
        .expect_err("null should fail");
    assert!(null.to_string().contains("null"));

    let string = evaluate_numeric_update(&Value::from("1"), GraphNumericOp::Add, &Value::Int(1))
        .expect_err("string should fail");
    assert!(string.to_string().contains("integer or float"));
}

#[test]
fn graph_loads_from_yaml() {
    let graph = Graph::from_yaml(
        r#"
nodes:
  - id: employee:nia
    label: Employee
    props:
      level: IC4
      department: Engineering
  - id: employee:marco
    label: Employee
edges:
  - label: REPORTS_TO
    from: employee:nia
    to: employee:marco
    props:
      source: hris
"#,
    )
    .expect("graph yaml should load");

    assert_eq!(graph.nodes.len(), 2);
    assert_eq!(graph.edges.len(), 1);
    assert_eq!(
        graph.nodes[0].props.get("id"),
        Some(&Value::from("employee:nia"))
    );
    assert_eq!(graph.nodes[0].props.get("level"), Some(&Value::from("IC4")));
}

#[test]
fn graph_yaml_rejects_edges_with_unknown_nodes() {
    let error = Graph::from_yaml(
        r#"
nodes:
  - id: employee:nia
    label: Employee
edges:
  - label: REPORTS_TO
    from: employee:nia
    to: employee:missing
"#,
    )
    .expect_err("missing node should fail validation");

    assert!(
        error
            .to_string()
            .contains("unknown to node 'employee:missing'")
    );
}

#[test]
fn graph_loads_from_json() {
    let graph = Graph::from_json(
        r#"
{
  "nodes": [
    {
      "id": "employee:nia",
      "label": "Employee",
      "props": {
        "level": "IC4",
        "department": "Engineering"
      }
    },
    {
      "id": "employee:marco",
      "label": "Employee"
    }
  ],
  "edges": [
    {
      "label": "REPORTS_TO",
      "from": "employee:nia",
      "to": "employee:marco",
      "props": {
        "source": "hris"
      }
    }
  ]
}
"#,
    )
    .expect("graph json should load");

    assert_eq!(graph.nodes.len(), 2);
    assert_eq!(graph.edges.len(), 1);
    assert_eq!(
        graph.nodes[0].props.get("id"),
        Some(&Value::from("employee:nia"))
    );
    assert_eq!(graph.nodes[0].props.get("level"), Some(&Value::from("IC4")));
}

#[test]
fn graph_json_rejects_edges_with_unknown_nodes() {
    let error = Graph::from_json(
        r#"
{
  "nodes": [
    { "id": "employee:nia", "label": "Employee" }
  ],
  "edges": [
    { "label": "REPORTS_TO", "from": "employee:nia", "to": "employee:missing" }
  ]
}
"#,
    )
    .expect_err("missing node should fail validation");

    assert!(
        error
            .to_string()
            .contains("unknown to node 'employee:missing'")
    );
}

#[test]
fn graph_serializes_to_json() {
    let mut builder = GraphBuilder::new();
    let _ = builder
        .node("Employee", "employee:nia")
        .prop("level", "IC4")
        .finish();
    let _ = builder.node("Employee", "employee:marco").finish();
    let _ = builder
        .edge("REPORTS_TO", "employee:nia", "employee:marco")
        .finish();

    let json = builder.build().to_json().expect("graph should serialize");

    assert!(json.contains("\"employee:nia\""));
    assert!(json.contains("\"REPORTS_TO\""));
    assert!(!json.contains("\"value\": \"employee:nia\""));
}

#[test]
fn graph_loads_from_xml() {
    let graph = Graph::from_xml(
        r#"
<graph>
  <nodes>
    <node>
      <id>employee:nia</id>
      <label>Employee</label>
      <props>
        <prop>
          <key>level</key>
          <value>
            <type>string</type>
            <value>IC4</value>
          </value>
        </prop>
      </props>
    </node>
    <node>
      <id>employee:marco</id>
      <label>Employee</label>
    </node>
  </nodes>
  <edges>
    <edge>
      <label>REPORTS_TO</label>
      <from>employee:nia</from>
      <to>employee:marco</to>
    </edge>
  </edges>
</graph>
"#,
    )
    .expect("graph xml should load");

    assert_eq!(graph.nodes.len(), 2);
    assert_eq!(graph.edges.len(), 1);
    assert_eq!(
        graph.nodes[0].props.get("id"),
        Some(&Value::from("employee:nia"))
    );
    assert_eq!(graph.nodes[0].props.get("level"), Some(&Value::from("IC4")));
}

#[test]
fn graph_xml_rejects_edges_with_unknown_nodes() {
    let error = Graph::from_xml(
        r#"
<graph>
  <nodes>
    <node>
      <id>employee:nia</id>
      <label>Employee</label>
    </node>
  </nodes>
  <edges>
    <edge>
      <label>REPORTS_TO</label>
      <from>employee:nia</from>
      <to>employee:missing</to>
    </edge>
  </edges>
</graph>
"#,
    )
    .expect_err("missing node should fail validation");

    assert!(
        error
            .to_string()
            .contains("unknown to node 'employee:missing'")
    );
}

#[test]
fn graph_serializes_to_xml() {
    let mut builder = GraphBuilder::new();
    let _ = builder
        .node("Employee", "employee:nia")
        .prop("level", "IC4")
        .finish();
    let _ = builder.node("Employee", "employee:marco").finish();
    let _ = builder
        .edge("REPORTS_TO", "employee:nia", "employee:marco")
        .finish();

    let graph = builder.build();
    let xml = graph.to_xml().expect("graph should serialize");
    let reparsed = Graph::from_xml(&xml).expect("serialized xml should load");

    assert!(xml.contains("<id>employee:nia</id>"));
    assert!(xml.contains("<label>REPORTS_TO</label>"));
    assert_eq!(reparsed, graph);
}

#[test]
fn graph_schema_validates_labels_fields_and_edge_endpoints() {
    let schema = GraphSchema::builder()
        .node(
            "Person",
            vec![
                Field::required("name", FieldType::String),
                Field::optional("age", FieldType::Int),
            ],
        )
        .node("Project", vec![Field::required("name", FieldType::String)])
        .edge(
            "WORKS_ON",
            vec![Label::new("Person")],
            vec![Label::new("Project")],
            vec![Field::required("role", FieldType::String)],
        )
        .build();

    let mut builder = Graph::builder();
    let _ = builder
        .node("Person", "person:ada")
        .prop("name", "Ada")
        .prop("age", 36i64)
        .finish();
    let _ = builder
        .node("Project", "project:grust")
        .prop("name", "Grust")
        .finish();
    let _ = builder
        .edge("WORKS_ON", "person:ada", "project:grust")
        .prop("role", "maintainer")
        .finish();

    schema
        .validate_graph(&builder.build())
        .expect("graph should match schema");
}

#[test]
fn graph_schema_carries_and_validates_required_constraints() {
    let schema = GraphSchema::builder()
        .node("Person", Vec::<Field>::new())
        .node("Project", Vec::<Field>::new())
        .edge(
            "WORKS_ON",
            vec![Label::new("Person")],
            vec![Label::new("Project")],
            Vec::<Field>::new(),
        )
        .required_node_property("Person", "email")
        .required_edge_property("WORKS_ON", "role")
        .unique_node_property("Person", "email")
        .build();

    assert_eq!(
        schema.constraints_for_label(&Label::new("Person")),
        vec![
            &GraphConstraint::NodePropertyRequired {
                label: Label::new("Person"),
                key: "email".to_string(),
            },
            &GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "email".to_string(),
            },
        ]
    );

    let mut missing_node_prop = Graph::builder();
    let _ = missing_node_prop.node("Person", "person:ada").finish();
    let _ = missing_node_prop.node("Project", "project:grust").finish();
    let _ = missing_node_prop
        .edge("WORKS_ON", "person:ada", "project:grust")
        .prop("role", "maintainer")
        .finish();
    let error = schema
        .validate_graph(&missing_node_prop.build())
        .expect_err("required node constraint should fail");
    assert!(
        error
            .to_string()
            .contains("missing required constrained property 'email'")
    );

    let mut missing_edge_prop = Graph::builder();
    let _ = missing_edge_prop
        .node("Person", "person:ada")
        .prop("email", "ada@example.com")
        .finish();
    let _ = missing_edge_prop.node("Project", "project:grust").finish();
    let _ = missing_edge_prop
        .edge("WORKS_ON", "person:ada", "project:grust")
        .finish();
    let error = schema
        .validate_graph(&missing_edge_prop.build())
        .expect_err("required edge constraint should fail");
    assert!(
        error
            .to_string()
            .contains("missing required constrained property 'role'")
    );
}

#[test]
fn graph_schema_validates_unique_property_constraints() {
    let schema = GraphSchema::builder()
        .node("Person", Vec::<Field>::new())
        .node("Project", Vec::<Field>::new())
        .edge(
            "WORKS_ON",
            vec![Label::new("Person")],
            vec![Label::new("Project")],
            Vec::<Field>::new(),
        )
        .unique_node_property("Person", "email")
        .unique_edge_property("WORKS_ON", "role")
        .build();

    let mut duplicate_nodes = Graph::builder();
    let _ = duplicate_nodes
        .node("Person", "person:ada")
        .prop("email", "ada@example.com")
        .finish();
    let _ = duplicate_nodes
        .node("Person", "person:grace")
        .prop("email", "ada@example.com")
        .finish();
    let error = schema
        .validate_graph(&duplicate_nodes.build())
        .expect_err("duplicate node property should violate unique constraint");
    assert!(
        error
            .to_string()
            .contains("duplicates unique constrained property 'email'")
    );

    let mut duplicate_edges = Graph::builder();
    let _ = duplicate_edges.node("Person", "person:ada").finish();
    let _ = duplicate_edges.node("Person", "person:grace").finish();
    let _ = duplicate_edges.node("Project", "project:grust").finish();
    let _ = duplicate_edges.node("Project", "project:sail").finish();
    let _ = duplicate_edges
        .edge("WORKS_ON", "person:ada", "project:grust")
        .prop("role", "maintainer")
        .finish();
    let _ = duplicate_edges
        .edge("WORKS_ON", "person:grace", "project:sail")
        .prop("role", "maintainer")
        .finish();
    let error = schema
        .validate_graph(&duplicate_edges.build())
        .expect_err("duplicate edge property should violate unique constraint");
    assert!(
        error
            .to_string()
            .contains("duplicates unique constrained property 'role'")
    );
}

#[test]
fn graph_schema_rejects_wrong_field_type() {
    let schema = GraphSchema::builder()
        .node("Person", vec![Field::required("age", FieldType::Int)])
        .build();

    let mut builder = Graph::builder();
    let _ = builder
        .node("Person", "person:ada")
        .prop("age", "old")
        .finish();

    let error = schema
        .validate_graph(&builder.build())
        .expect_err("string age should fail int field validation");

    assert!(error.to_string().contains("field 'age' expected Int"));
}

#[test]
fn graph_schema_rejects_wrong_edge_endpoint_label() {
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

    let mut builder = Graph::builder();
    let _ = builder.node("Project", "project:source").finish();
    let _ = builder.node("Project", "project:target").finish();
    let _ = builder
        .edge("WORKS_ON", "project:source", "project:target")
        .finish();

    let error = schema
        .validate_graph(&builder.build())
        .expect_err("wrong from label should fail");

    assert!(
        error
            .to_string()
            .contains("cannot start from node label 'Project'")
    );
}

#[cfg(feature = "typed-garde")]
mod typed_garde_tests {
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::typed::{TypedEdge, TypedGraphBuilder, TypedNode, garde};

    #[derive(Debug, Deserialize, Serialize, garde::Validate)]
    #[garde(allow_unvalidated)]
    struct Person {
        #[garde(length(min = 1))]
        id: String,
        #[garde(length(min = 1))]
        name: String,
        #[garde(length(min = 2))]
        skills: Vec<String>,
    }

    impl TypedNode for Person {
        const LABEL: &'static str = "Person";

        fn node_id(&self) -> NodeId {
            format!("person:{}", self.id).into()
        }
    }

    #[derive(Debug, Deserialize, Serialize, garde::Validate)]
    #[garde(allow_unvalidated)]
    struct Project {
        #[garde(length(min = 1))]
        id: String,
        #[garde(length(min = 1))]
        title: String,
    }

    impl TypedNode for Project {
        const LABEL: &'static str = "Project";

        fn node_id(&self) -> NodeId {
            format!("project:{}", self.id).into()
        }
    }

    #[derive(Debug, Deserialize, Serialize, garde::Validate)]
    #[garde(allow_unvalidated)]
    struct WorksOn {
        #[garde(length(min = 1))]
        person_id: String,
        #[garde(length(min = 1))]
        project_id: String,
        #[garde(range(min = 1, max = 100))]
        allocation: u8,
    }

    impl TypedEdge for WorksOn {
        const LABEL: &'static str = "WORKS_ON";

        fn source_node_id(&self) -> NodeId {
            format!("person:{}", self.person_id).into()
        }

        fn target_node_id(&self) -> NodeId {
            format!("project:{}", self.project_id).into()
        }
    }

    #[test]
    fn typed_builder_validates_and_lowers_nodes_and_edges() {
        let mut builder = TypedGraphBuilder::new();
        builder
            .add_node(&Person {
                id: "nia".to_string(),
                name: "Nia".to_string(),
                skills: vec!["rust".to_string(), "graphs".to_string()],
            })
            .expect("person is valid");
        builder
            .add_node(&Project {
                id: "grust".to_string(),
                title: "Grust".to_string(),
            })
            .expect("project is valid");
        builder
            .add_edge(&WorksOn {
                person_id: "nia".to_string(),
                project_id: "grust".to_string(),
                allocation: 80,
            })
            .expect("edge is valid");

        let graph = builder.build();

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.nodes[0].label, Label::from("Person"));
        assert_eq!(
            graph.nodes[0].props.get("skills"),
            Some(&Value::from(vec!["rust".to_string(), "graphs".to_string()]))
        );
        assert_eq!(graph.edges[0].label, Label::from("WORKS_ON"));
        assert_eq!(graph.edges[0].from, NodeId::from("person:nia"));
        assert_eq!(graph.edges[0].to, NodeId::from("project:grust"));
    }

    #[test]
    fn typed_node_and_edge_read_back_from_graph_values() {
        let mut builder = TypedGraphBuilder::new();
        builder
            .add_node(&Person {
                id: "nia".to_string(),
                name: "Nia".to_string(),
                skills: vec!["rust".to_string(), "graphs".to_string()],
            })
            .expect("person is valid");
        builder
            .add_node(&Project {
                id: "grust".to_string(),
                title: "Grust".to_string(),
            })
            .expect("project is valid");
        builder
            .add_edge(&WorksOn {
                person_id: "nia".to_string(),
                project_id: "grust".to_string(),
                allocation: 80,
            })
            .expect("edge is valid");
        let graph = builder.build();

        let person = Person::from_node(
            graph
                .nodes
                .iter()
                .find(|node| node.label == Label::new("Person"))
                .expect("person node"),
        )
        .expect("typed person should decode");
        let works_on = WorksOn::from_edge(&graph.edges[0]).expect("typed edge should decode");

        assert_eq!(person.id, "nia");
        assert_eq!(
            person.skills,
            vec!["rust".to_string(), "graphs".to_string()]
        );
        assert_eq!(works_on.person_id, "nia");
        assert_eq!(works_on.project_id, "grust");
        assert_eq!(works_on.allocation, 80);
    }

    #[test]
    fn typed_readback_rejects_label_and_identity_mismatches() {
        let mut props = Props::new();
        props.insert("id".to_string(), Value::from("nia"));
        props.insert("name".to_string(), Value::from("Nia"));
        props.insert(
            "skills".to_string(),
            Value::StringArray(vec!["rust".to_string(), "graphs".to_string()]),
        );

        let wrong_label = Node::new("Project", "person:nia", props.clone());
        let wrong_id = Node::new("Person", "person:other", props);

        assert!(Person::from_node(&wrong_label).is_err());
        assert!(Person::from_node(&wrong_id).is_err());
    }

    #[test]
    fn typed_builder_rejects_invalid_values_before_building_graph() {
        let mut builder = TypedGraphBuilder::new();
        let error = builder
            .add_edge(&WorksOn {
                person_id: "nia".to_string(),
                project_id: "grust".to_string(),
                allocation: 0,
            })
            .expect_err("allocation must be in range");

        assert!(error.to_string().contains("WORKS_ON validation failed"));
    }

    #[test]
    fn typed_builder_coexists_with_raw_nodes_and_edges() {
        let mut builder = TypedGraphBuilder::new();
        builder.add_raw_node(Node::new("Document", "doc:proposal", Props::new()));
        builder
            .add_node(&Person {
                id: "nia".to_string(),
                name: "Nia".to_string(),
                skills: vec!["rust".to_string(), "graphs".to_string()],
            })
            .expect("person is valid");
        builder.add_raw_edge(Edge::new(
            "AUTHORED",
            "person:nia",
            "doc:proposal",
            Props::new(),
        ));

        let graph = builder.build();

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);
        assert!(graph.nodes.iter().any(|node| {
            node.id == NodeId::from("doc:proposal") && node.label == Label::from("Document")
        }));
        assert!(graph.nodes.iter().any(|node| {
            node.id == NodeId::from("person:nia") && node.label == Label::from("Person")
        }));
        assert_eq!(graph.edges[0].label, Label::from("AUTHORED"));
    }

    #[test]
    fn typed_builder_can_extend_an_existing_graph() {
        let existing = Graph::new(
            vec![Node::new("Document", "doc:proposal", Props::new())],
            Vec::new(),
        );
        let mut builder = TypedGraphBuilder::from_graph(existing);
        builder
            .add_node(&Person {
                id: "nia".to_string(),
                name: "Nia".to_string(),
                skills: vec!["rust".to_string(), "graphs".to_string()],
            })
            .expect("person is valid");

        let mut builder = builder.into_builder();
        let _ = builder
            .edge("AUTHORED", "person:nia", "doc:proposal")
            .finish();
        let graph = TypedGraphBuilder::from_builder(builder).build();

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].from, NodeId::from("person:nia"));
        assert_eq!(graph.edges[0].to, NodeId::from("doc:proposal"));
    }

    #[cfg(feature = "typed-zod-rs")]
    #[test]
    fn zod_rs_schema_can_feed_typed_garde_nodes_and_edges() {
        use serde_json::json;
        use zod_rs::prelude::{Schema, number, object, string};

        let person_schema = object()
            .field("id", string().min(1))
            .field("name", string().min(1))
            .field("skills", string().min(1).array())
            .strict();
        let project_schema = object()
            .field("id", string().min(1))
            .field("title", string().min(1))
            .strict();
        let works_on_schema = object()
            .field("person_id", string().min(1))
            .field("project_id", string().min(1))
            .field("allocation", number().int().min(1.0).max(100.0))
            .strict();

        let mut builder = TypedGraphBuilder::new();
        builder
            .add_node_from_json::<Person, _>(
                &person_schema,
                &json!({
                    "id": "nia",
                    "name": "Nia",
                    "skills": ["rust", "graphs"]
                }),
            )
            .expect("person JSON is valid");
        builder
            .add_node_from_json::<Project, _>(
                &project_schema,
                &json!({
                    "id": "grust",
                    "title": "Grust"
                }),
            )
            .expect("project JSON is valid");
        builder
            .add_edge_from_json::<WorksOn, _>(
                &works_on_schema,
                &json!({
                    "person_id": "nia",
                    "project_id": "grust",
                    "allocation": 80
                }),
            )
            .expect("edge JSON is valid");

        let graph = builder.build();

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].label, Label::from("WORKS_ON"));
    }

    #[cfg(feature = "typed-zod-rs")]
    #[test]
    fn zod_rs_schema_errors_before_deserialize_and_garde_errors_after() {
        use serde_json::json;
        use zod_rs::prelude::{Schema, object, string};

        let loose_person_schema = object()
            .field("id", string().min(1))
            .field("name", string().min(1))
            .field("skills", string().array())
            .strict();
        let shape_error = crate::typed::parse_typed_json::<Person, _>(
            &loose_person_schema,
            &json!({
                "id": "nia",
                "name": "Nia",
                "skills": "rust"
            }),
        )
        .expect_err("zod-rs should reject a non-array skills field");

        assert!(shape_error.to_string().contains("zod-rs validation failed"));

        let domain_error = crate::typed::parse_typed_json::<Person, _>(
            &loose_person_schema,
            &json!({
                "id": "nia",
                "name": "Nia",
                "skills": ["rust"]
            }),
        )
        .expect_err("garde should reject too few skills");

        assert!(domain_error.to_string().contains("typed validation failed"));
    }
}

#[test]
fn graph_serializes_to_yaml() {
    let mut builder = GraphBuilder::new();
    let _ = builder
        .node("Employee", "employee:nia")
        .prop("level", "IC4")
        .finish();
    let _ = builder.node("Employee", "employee:marco").finish();
    let _ = builder
        .edge("REPORTS_TO", "employee:nia", "employee:marco")
        .finish();

    let yaml = builder.build().to_yaml().expect("graph should serialize");

    assert!(yaml.contains("employee:nia"));
    assert!(yaml.contains("REPORTS_TO"));
    assert!(!yaml.contains("value: employee:nia"));
}

#[test]
fn graph_builder_label_conflict_replaces_node() {
    let mut builder = GraphBuilder::new();
    let _ = builder
        .node("Person", "entity:1")
        .prop("name", "Ada")
        .finish();
    let _ = builder
        .node("Robot", "entity:1")
        .prop("model", "Mark I")
        .finish();

    let graph = builder.build();

    assert_eq!(graph.nodes.len(), 1);
    let node = &graph.nodes[0];
    assert_eq!(node.label, Label::from("Robot"));
    assert!(node.props.contains_key("model"));
    assert!(
        !node.props.contains_key("name"),
        "label conflict should replace the node, not merge props"
    );
}

#[test]
fn graph_builder_same_label_merges_props() {
    let mut builder = GraphBuilder::new();
    let _ = builder
        .node("Person", "person:ada")
        .prop("name", "Ada")
        .finish();
    let _ = builder
        .node("Person", "person:ada")
        .prop("age", 36i64)
        .finish();

    let graph = builder.build();

    assert_eq!(graph.nodes.len(), 1);
    let node = &graph.nodes[0];
    assert_eq!(node.props.get("name"), Some(&Value::from("Ada")));
    assert_eq!(node.props.get("age"), Some(&Value::from(36i64)));
}

#[test]
#[should_panic(expected = "Traversal::to() must follow")]
fn traversal_to_without_step_panics() {
    let _ = Traversal::from_node("person:ada").to("Person");
}

#[test]
fn validate_edge_with_uses_label_lookup() {
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

    let person = Label::new("Person");
    let project = Label::new("Project");
    let lookup = |id: &NodeId| match id.as_str() {
        "person:ada" => Some(&person),
        "project:grust" => Some(&project),
        _ => None,
    };

    let edge = Edge::new("WORKS_ON", "person:ada", "project:grust", Props::new());
    schema
        .validate_edge_with(&edge, lookup)
        .expect("edge should validate via lookup");

    let dangling = Edge::new("WORKS_ON", "person:ada", "project:missing", Props::new());
    let error = schema
        .validate_edge_with(&dangling, lookup)
        .expect_err("dangling edge should fail");
    assert!(error.to_string().contains("unknown to node"));

    let wrong = Edge::new("WORKS_ON", "project:grust", "project:grust", Props::new());
    let error = schema
        .validate_edge_with(&wrong, lookup)
        .expect_err("wrong from label should fail");
    assert!(error.to_string().contains("cannot start from node label"));
}

#[test]
fn validate_edge_props_checks_label_and_fields_only() {
    let schema = GraphSchema::builder()
        .edge(
            "WORKS_ON",
            vec![Label::new("Person")],
            vec![Label::new("Project")],
            vec![Field::required("role", FieldType::String)],
        )
        .build();

    // Endpoints are not checked, but required fields are.
    let edge = Edge::new("WORKS_ON", "anyone", "anything", Props::new());
    let error = schema
        .validate_edge_props(&edge)
        .expect_err("missing required field should fail");
    assert!(error.to_string().contains("missing required field 'role'"));

    let mut props = Props::new();
    props.insert("role".to_string(), Value::from("maintainer"));
    let edge = Edge::new("WORKS_ON", "anyone", "anything", props);
    schema
        .validate_edge_props(&edge)
        .expect("props-only validation should pass");

    let unknown = Edge::new("UNKNOWN", "a", "b", Props::new());
    let error = schema
        .validate_edge_props(&unknown)
        .expect_err("unknown edge label should fail");
    assert!(error.to_string().contains("schema has no edge type"));
}

#[test]
fn datetime_value_validates_rfc3339() {
    let date = Value::datetime("2026-06-12T09:30:00Z").unwrap();
    assert_eq!(date.as_datetime(), Some("2026-06-12T09:30:00Z"));
    assert!(Value::datetime("2026-06-12T09:30:00.123+02:00").is_ok());
    assert!(Value::datetime("2026-02-29T09:30:00Z").is_err());
    assert!(Value::datetime("2024-02-29T09:30:00Z").is_ok());
    assert!(Value::datetime("2026-06-12 09:30:00Z").is_err());
    assert!(Value::datetime("2026-06-12T09:30:00z").is_err());
    assert!(Value::datetime("not a date").is_err());
    assert!(Value::datetime("2026-06-12").is_err());
    assert!(Value::datetime("2026-13-12T09:30:00Z").is_err());
    assert!(Value::datetime("2026-04-31T09:30:00Z").is_err());
    assert!(Value::datetime("2026-06-12T25:30:00Z").is_err());
    assert!(Value::datetime("2026-06-12T09:30:00").is_err());
    assert!(Value::datetime("2026-06-12T09:30:00.Z").is_err());

    let valid: Value = serde_json::from_value(
        serde_json::json!({"type": "date_time", "value": "2026-06-12T09:30:00Z"}),
    )
    .expect("valid tagged DateTime should deserialize");
    assert_eq!(valid, date);

    let err = serde_json::from_value::<Value>(
        serde_json::json!({"type": "date_time", "value": "not a date"}),
    )
    .expect_err("invalid tagged DateTime should not deserialize");
    assert!(err.to_string().contains("not an RFC 3339 date-time"));
}

#[test]
fn schema_validates_datetime_and_array_fields() {
    let schema = GraphSchema::builder()
        .node(
            "Event",
            vec![
                Field::required("when", FieldType::DateTime),
                Field::optional("counts", FieldType::IntArray),
                Field::optional("scores", FieldType::FloatArray),
            ],
        )
        .build();

    let mut builder = Graph::builder();
    let _ = builder
        .node("Event", "event:1")
        .prop("when", Value::datetime("2026-06-12T09:30:00Z").unwrap())
        .prop("counts", vec![1i64, 2, 3])
        .prop("scores", vec![0.5f64, 0.9])
        .finish();
    schema
        .validate_graph(&builder.build())
        .expect("typed values should validate");

    // Plain strings stay valid for DateTime fields, but must parse.
    let mut builder = Graph::builder();
    let _ = builder
        .node("Event", "event:2")
        .prop("when", "2026-06-12T09:30:00Z")
        .finish();
    schema
        .validate_graph(&builder.build())
        .expect("RFC 3339 string should validate as DateTime");

    let mut builder = Graph::builder();
    let _ = builder
        .node("Event", "event:3")
        .prop("when", "soon")
        .finish();
    let error = schema
        .validate_graph(&builder.build())
        .expect_err("non-RFC 3339 string should fail DateTime field");
    assert!(error.to_string().contains("expected DateTime"));
}

#[test]
fn value_json_round_trips_plain_and_tagged() {
    assert_eq!(
        Value::from(vec![1i64, 2]).to_json(),
        serde_json::json!([1, 2])
    );
    assert_eq!(
        Value::from_json(serde_json::json!([1, 2])),
        Value::IntArray(vec![1, 2])
    );
    assert_eq!(
        Value::from_json(serde_json::json!([1.5, 2.0])),
        Value::FloatArray(vec![1.5, 2.0])
    );
    assert_eq!(
        Value::from_json(serde_json::json!(["a", "b"])),
        Value::StringArray(vec!["a".to_string(), "b".to_string()])
    );
    // DateTime flattens to a string in plain JSON.
    let dt = Value::datetime("2026-06-12T09:30:00Z").unwrap();
    assert_eq!(dt.to_json(), serde_json::json!("2026-06-12T09:30:00Z"));
    // Tagged form round-trips exactly.
    let tagged = serde_json::to_value(&dt).unwrap();
    assert_eq!(Value::from_json(tagged), dt);
}

#[test]
fn builder_add_edge_reports_outcome() {
    let mut builder = GraphBuilder::new();
    let _ = builder.node("Person", "a").finish();
    let _ = builder.node("Person", "b").finish();

    let first = builder.edge("KNOWS", "a", "b").finish();
    let second = builder.edge("KNOWS", "a", "b").finish();

    assert_eq!(first, PutOutcome::Inserted);
    assert_eq!(second, PutOutcome::Deduped);
}

#[test]
fn schema_enforces_edge_uniqueness() {
    let schema = GraphSchema::builder()
        .node("Person", Vec::<Field>::new())
        .edge(
            "KNOWS",
            vec![Label::new("Person")],
            vec![Label::new("Person")],
            Vec::<Field>::new(),
        )
        .build();

    let mut builder = GraphBuilder::new().edge_policy(EdgePolicy::AllowDuplicates);
    let _ = builder.node("Person", "a").finish();
    let _ = builder.node("Person", "b").finish();
    let _ = builder.edge("KNOWS", "a", "b").finish();
    let _ = builder.edge("KNOWS", "a", "b").finish();

    let error = schema
        .validate_graph(&builder.build())
        .expect_err("duplicate edge should violate uniqueness");
    assert!(error.to_string().contains("violates"));
}

#[test]
fn undirected_edge_type_accepts_reversed_endpoints_and_unordered_uniqueness() {
    let schema = GraphSchema::builder()
        .node("Person", Vec::<Field>::new())
        .node("Project", Vec::<Field>::new())
        .edge_type(EdgeType {
            label: Label::new("LINKED"),
            from: vec![Label::new("Person")],
            to: vec![Label::new("Project")],
            fields: Vec::new(),
            directed: false,
            uniqueness: EdgeUniqueness::FromLabelTo,
        })
        .build();

    // Reversed orientation is accepted for undirected edges.
    let mut builder = GraphBuilder::new();
    let _ = builder.node("Person", "person:ada").finish();
    let _ = builder.node("Project", "project:grust").finish();
    let _ = builder
        .edge("LINKED", "project:grust", "person:ada")
        .finish();
    schema
        .validate_graph(&builder.build())
        .expect("reversed undirected edge should validate");

    // The same pair in both orientations violates unordered uniqueness.
    let mut builder = GraphBuilder::new().edge_policy(EdgePolicy::AllowDuplicates);
    let _ = builder.node("Person", "person:ada").finish();
    let _ = builder.node("Project", "project:grust").finish();
    let _ = builder
        .edge("LINKED", "person:ada", "project:grust")
        .finish();
    let _ = builder
        .edge("LINKED", "project:grust", "person:ada")
        .finish();
    let error = schema
        .validate_graph(&builder.build())
        .expect_err("reversed duplicate should violate uniqueness");
    assert!(error.to_string().contains("violates"));

    // A directed type accepts the same pair in both orientations.
    let directed = GraphSchema::builder()
        .node("Person", Vec::<Field>::new())
        .node("Project", Vec::<Field>::new())
        .edge(
            "LINKED",
            vec![Label::new("Person"), Label::new("Project")],
            vec![Label::new("Person"), Label::new("Project")],
            Vec::<Field>::new(),
        )
        .build();
    let mut builder = GraphBuilder::new().edge_policy(EdgePolicy::AllowDuplicates);
    let _ = builder.node("Person", "person:ada").finish();
    let _ = builder.node("Project", "project:grust").finish();
    let _ = builder
        .edge("LINKED", "person:ada", "project:grust")
        .finish();
    let _ = builder
        .edge("LINKED", "project:grust", "person:ada")
        .finish();
    directed
        .validate_graph(&builder.build())
        .expect("opposite directed edges are distinct");
}
