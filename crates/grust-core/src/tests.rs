use super::*;

#[test]
fn builder_dedupes_nodes_and_edges() {
    let mut builder = GraphBuilder::new();
    let talk = builder
        .node("Talk", "talk-1")
        .prop("title", "A Talk")
        .finish();
    let person = builder.node("Person", "person-1").finish();
    builder
        .node("Talk", "talk-1")
        .prop("description", "Updated")
        .finish();
    builder.edge("PRESENTS", &person, &talk).finish();
    builder.edge("PRESENTS", &person, &talk).finish();

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
    builder
        .node("Employee", "employee:nia")
        .prop("level", "IC4")
        .finish();
    builder.node("Employee", "employee:marco").finish();
    builder
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
    builder
        .node("Employee", "employee:nia")
        .prop("level", "IC4")
        .finish();
    builder.node("Employee", "employee:marco").finish();
    builder
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
    builder
        .node("Person", "person:ada")
        .prop("name", "Ada")
        .prop("age", 36i64)
        .finish();
    builder
        .node("Project", "project:grust")
        .prop("name", "Grust")
        .finish();
    builder
        .edge("WORKS_ON", "person:ada", "project:grust")
        .prop("role", "maintainer")
        .finish();

    schema
        .validate_graph(&builder.build())
        .expect("graph should match schema");
}

#[test]
fn graph_schema_rejects_wrong_field_type() {
    let schema = GraphSchema::builder()
        .node("Person", vec![Field::required("age", FieldType::Int)])
        .build();

    let mut builder = Graph::builder();
    builder
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
    builder.node("Project", "project:source").finish();
    builder.node("Project", "project:target").finish();
    builder
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
        builder
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
    builder
        .node("Employee", "employee:nia")
        .prop("level", "IC4")
        .finish();
    builder.node("Employee", "employee:marco").finish();
    builder
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
    builder
        .node("Person", "entity:1")
        .prop("name", "Ada")
        .finish();
    builder
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
    builder
        .node("Person", "person:ada")
        .prop("name", "Ada")
        .finish();
    builder
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
    assert!(Value::datetime("2026-06-12T09:30:00Z").is_ok());
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
    builder
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
    builder
        .node("Event", "event:2")
        .prop("when", "2026-06-12T09:30:00Z")
        .finish();
    schema
        .validate_graph(&builder.build())
        .expect("RFC 3339 string should validate as DateTime");

    let mut builder = Graph::builder();
    builder
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
    builder.node("Person", "a").finish();
    builder.node("Person", "b").finish();

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
    builder.node("Person", "a").finish();
    builder.node("Person", "b").finish();
    builder.edge("KNOWS", "a", "b").finish();
    builder.edge("KNOWS", "a", "b").finish();

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
    builder.node("Person", "person:ada").finish();
    builder.node("Project", "project:grust").finish();
    builder
        .edge("LINKED", "project:grust", "person:ada")
        .finish();
    schema
        .validate_graph(&builder.build())
        .expect("reversed undirected edge should validate");

    // The same pair in both orientations violates unordered uniqueness.
    let mut builder = GraphBuilder::new().edge_policy(EdgePolicy::AllowDuplicates);
    builder.node("Person", "person:ada").finish();
    builder.node("Project", "project:grust").finish();
    builder
        .edge("LINKED", "person:ada", "project:grust")
        .finish();
    builder
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
    builder.node("Person", "person:ada").finish();
    builder.node("Project", "project:grust").finish();
    builder
        .edge("LINKED", "person:ada", "project:grust")
        .finish();
    builder
        .edge("LINKED", "project:grust", "person:ada")
        .finish();
    directed
        .validate_graph(&builder.build())
        .expect("opposite directed edges are distinct");
}
