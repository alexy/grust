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
