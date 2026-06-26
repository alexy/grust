use std::collections::BTreeMap;

use grust_core::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CocoIndexGraphExport {
    pub nodes: Vec<CocoIndexNodeState>,
    pub relationships: Vec<CocoIndexRelationshipState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CocoIndexNodeState {
    pub label: String,
    pub key: JsonValue,
    pub properties: JsonMap<String, JsonValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CocoIndexRelationshipState {
    pub rel_type: String,
    pub source: CocoIndexEndpoint,
    pub target: CocoIndexEndpoint,
    pub key: JsonValue,
    pub properties: JsonMap<String, JsonValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CocoIndexEndpoint {
    pub label: String,
    pub key: JsonValue,
}

pub trait CocoIndexExport {
    fn to_cocoindex_export(&self) -> Result<CocoIndexGraphExport>;
}

impl CocoIndexExport for Graph {
    fn to_cocoindex_export(&self) -> Result<CocoIndexGraphExport> {
        graph_to_cocoindex_export(self)
    }
}

pub fn cocoindex_export_to_graph(export: CocoIndexGraphExport) -> Result<Graph> {
    let nodes = export
        .nodes
        .into_iter()
        .map(node_from_state)
        .collect::<Result<Vec<_>>>()?;
    let node_labels = nodes
        .iter()
        .map(|node| (node.id.clone(), node.label.clone()))
        .collect::<BTreeMap<_, _>>();
    let edges = export
        .relationships
        .into_iter()
        .map(|relationship| edge_from_state(relationship, &node_labels))
        .collect::<Result<Vec<_>>>()?;

    Ok(Graph::new(nodes, edges))
}

pub fn graph_to_cocoindex_export(graph: &Graph) -> Result<CocoIndexGraphExport> {
    let labels = graph
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node.label.clone()))
        .collect::<BTreeMap<_, _>>();

    let nodes = graph
        .nodes
        .iter()
        .map(node_to_state)
        .collect::<Result<Vec<_>>>()?;

    let relationships = graph
        .edges
        .iter()
        .map(|edge| edge_to_state(edge, &labels))
        .collect::<Result<Vec<_>>>()?;

    Ok(CocoIndexGraphExport {
        nodes,
        relationships,
    })
}

pub fn node_to_state(node: &Node) -> Result<CocoIndexNodeState> {
    Ok(CocoIndexNodeState {
        label: node.label.as_str().to_string(),
        key: id_key(node.id.as_str()),
        properties: props_to_json_object(&node.props)?,
    })
}

pub fn node_from_state(node: CocoIndexNodeState) -> Result<Node> {
    Ok(Node::new(
        node.label,
        id_from_key(&node.key, "node")?,
        props_from_json_object(node.properties),
    ))
}

pub fn edge_to_state(
    edge: &Edge,
    node_labels: &BTreeMap<NodeId, Label>,
) -> Result<CocoIndexRelationshipState> {
    let source_label = node_labels.get(&edge.from).ok_or_else(|| {
        GrustError::Schema(format!(
            "CocoIndex export edge '{}' references missing source node '{}'",
            edge.label, edge.from
        ))
    })?;
    let target_label = node_labels.get(&edge.to).ok_or_else(|| {
        GrustError::Schema(format!(
            "CocoIndex export edge '{}' references missing target node '{}'",
            edge.label, edge.to
        ))
    })?;

    Ok(CocoIndexRelationshipState {
        rel_type: edge.label.as_str().to_string(),
        source: CocoIndexEndpoint {
            label: source_label.as_str().to_string(),
            key: id_key(edge.from.as_str()),
        },
        target: CocoIndexEndpoint {
            label: target_label.as_str().to_string(),
            key: id_key(edge.to.as_str()),
        },
        key: id_key(&edge_key(edge)),
        properties: props_to_json_object(&edge.props)?,
    })
}

pub fn edge_from_state(
    relationship: CocoIndexRelationshipState,
    node_labels: &BTreeMap<NodeId, Label>,
) -> Result<Edge> {
    let from = id_from_key(&relationship.source.key, "relationship source")?;
    let to = id_from_key(&relationship.target.key, "relationship target")?;
    let label = Label::new(relationship.rel_type);
    validate_endpoint_label("source", &from, &relationship.source.label, node_labels)?;
    validate_endpoint_label("target", &to, &relationship.target.label, node_labels)?;

    let edge_key_id = id_from_key(&relationship.key, "relationship")?;
    let props = props_from_json_object(relationship.properties);
    let mut edge = Edge::new(label, from, to, props);
    let structural_key = edge_key(&edge);
    if edge_key_id.as_str() != structural_key {
        edge.id = Some(EdgeId::new(edge_key_id.into_string()));
    }
    Ok(edge)
}

fn id_key(id: &str) -> JsonValue {
    let mut key = JsonMap::new();
    key.insert("id".to_string(), JsonValue::String(id.to_string()));
    JsonValue::Object(key)
}

fn id_from_key(key: &JsonValue, kind: &str) -> Result<NodeId> {
    key.as_object()
        .and_then(|object| object.get("id"))
        .and_then(JsonValue::as_str)
        .map(NodeId::new)
        .ok_or_else(|| {
            GrustError::Serialization(format!(
                "CocoIndex {kind} key must be an object with a string id field"
            ))
        })
}

fn props_to_json_object(props: &Props) -> Result<JsonMap<String, JsonValue>> {
    props
        .iter()
        .map(|(key, value)| Ok((key.clone(), value_to_json(value)?)))
        .collect()
}

fn props_from_json_object(properties: JsonMap<String, JsonValue>) -> Props {
    properties
        .into_iter()
        .map(|(key, value)| (key, Value::from_json(value)))
        .collect()
}

fn validate_endpoint_label(
    endpoint: &str,
    id: &NodeId,
    label: &str,
    node_labels: &BTreeMap<NodeId, Label>,
) -> Result<()> {
    match node_labels.get(id) {
        Some(actual) if actual.as_str() == label => Ok(()),
        Some(actual) => Err(GrustError::Schema(format!(
            "CocoIndex relationship {endpoint} endpoint '{}' has label '{}' but imported node label is '{}'",
            id, label, actual
        ))),
        None => Err(GrustError::Schema(format!(
            "CocoIndex relationship {endpoint} endpoint '{}' references a missing imported node",
            id
        ))),
    }
}

fn value_to_json(value: &Value) -> Result<JsonValue> {
    Ok(match value {
        Value::Null => JsonValue::Null,
        Value::Bool(value) => JsonValue::Bool(*value),
        Value::Int(value) => JsonValue::Number((*value).into()),
        Value::Float(value) => serde_json::Number::from_f64(*value)
            .map(JsonValue::Number)
            .ok_or_else(|| {
                GrustError::Serialization(format!(
                    "cannot export non-finite float property value {value}"
                ))
            })?,
        Value::String(value) => JsonValue::String(value.clone()),
        Value::DateTime(value) => JsonValue::String(value.as_str().to_string()),
        Value::Decimal(value) => JsonValue::String(value.to_canonical_string()),
        Value::Duration(value) => JsonValue::String(value.to_iso_string()),
        Value::IntArray(values) => JsonValue::Array(
            values
                .iter()
                .map(|value| JsonValue::Number((*value).into()))
                .collect(),
        ),
        Value::FloatArray(values) => values
            .iter()
            .map(|value| {
                serde_json::Number::from_f64(*value)
                    .map(JsonValue::Number)
                    .ok_or_else(|| {
                        GrustError::Serialization(format!(
                            "cannot export non-finite float property value {value}"
                        ))
                    })
            })
            .collect::<Result<Vec<_>>>()
            .map(JsonValue::Array)?,
        Value::StringArray(values) => JsonValue::Array(
            values
                .iter()
                .map(|value| JsonValue::String(value.clone()))
                .collect(),
        ),
        Value::Json(value) => value.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meetup_graph() -> Graph {
        let mut builder = Graph::builder();
        let _ = builder
            .node("Group", "meetup:rust-sf")
            .prop("name", "Rust SF")
            .finish();
        let _ = builder
            .node("Event", "event:123")
            .prop("title", "Async Rust Night")
            .prop("capacity", 80i64)
            .finish();
        let _ = builder
            .node("Person", "person:ada")
            .prop("name", "Ada")
            .finish();
        let _ = builder
            .edge("HOSTED", "meetup:rust-sf", "event:123")
            .finish();
        let _ = builder
            .edge("RSVPED", "person:ada", "event:123")
            .prop("status", "yes")
            .finish();
        builder.build()
    }

    #[test]
    fn exports_nodes_and_relationships() {
        let export = meetup_graph().to_cocoindex_export().expect("export");

        assert_eq!(export.nodes.len(), 3);
        assert_eq!(export.relationships.len(), 2);
        let group = export
            .nodes
            .iter()
            .find(|node| node.label == "Group")
            .expect("group node");
        assert_eq!(group.key, id_key("meetup:rust-sf"));
        assert_eq!(
            group.properties.get("name"),
            Some(&JsonValue::String("Rust SF".to_string()))
        );

        let hosted = export
            .relationships
            .iter()
            .find(|relationship| relationship.rel_type == "HOSTED")
            .expect("hosted relationship");
        assert_eq!(hosted.rel_type, "HOSTED");
        assert_eq!(hosted.source.label, "Group");
        assert_eq!(hosted.target.label, "Event");
        assert_eq!(
            hosted.key,
            id_key("meetup:rust-sf\u{1f}HOSTED\u{1f}event:123")
        );
    }

    #[test]
    fn explicit_edge_id_becomes_relationship_key() {
        let edge = Edge::new("RSVPED", "person:ada", "event:123", Props::new()).with_id("rsvp:1");
        assert_eq!(edge_key(&edge), "rsvp:1");
    }

    #[test]
    fn export_preserves_explicit_edge_id_as_relationship_key() {
        let graph = Graph::new(
            vec![
                Node::new("Person", "person:ada", Props::new()),
                Node::new("Event", "event:123", Props::new()),
            ],
            vec![Edge::new("RSVPED", "person:ada", "event:123", Props::new()).with_id("rsvp:1")],
        );

        let export = graph.to_cocoindex_export().expect("export");

        assert_eq!(export.relationships[0].key, id_key("rsvp:1"));
    }

    #[test]
    fn export_allows_graph_with_zero_edges() {
        let graph = Graph::new(
            vec![Node::new("Person", "person:ada", Props::new())],
            Vec::new(),
        );

        let export = graph.to_cocoindex_export().expect("export");

        assert_eq!(export.nodes.len(), 1);
        assert!(export.relationships.is_empty());
    }

    #[test]
    fn imports_exported_graph_state() {
        let imported =
            cocoindex_export_to_graph(meetup_graph().to_cocoindex_export().expect("export"))
                .expect("import");

        assert_eq!(imported.nodes.len(), 3);
        assert_eq!(imported.edges.len(), 2);
        let event = imported
            .nodes
            .iter()
            .find(|node| node.id == NodeId::new("event:123"))
            .expect("event node");
        assert_eq!(event.label, Label::new("Event"));
        assert_eq!(event.props.get("capacity"), Some(&Value::Int(80)));

        let rsvp = imported
            .edges
            .iter()
            .find(|edge| edge.label == Label::new("RSVPED"))
            .expect("rsvp edge");
        assert_eq!(rsvp.from, NodeId::new("person:ada"));
        assert_eq!(rsvp.to, NodeId::new("event:123"));
        assert_eq!(rsvp.id, None);
        assert_eq!(
            rsvp.props.get("status"),
            Some(&Value::String("yes".to_string()))
        );
    }

    #[test]
    fn imports_explicit_relationship_keys_as_edge_ids() {
        let export = CocoIndexGraphExport {
            nodes: vec![
                CocoIndexNodeState {
                    label: "Person".to_string(),
                    key: id_key("person:ada"),
                    properties: JsonMap::new(),
                },
                CocoIndexNodeState {
                    label: "Event".to_string(),
                    key: id_key("event:123"),
                    properties: JsonMap::new(),
                },
            ],
            relationships: vec![CocoIndexRelationshipState {
                rel_type: "RSVPED".to_string(),
                source: CocoIndexEndpoint {
                    label: "Person".to_string(),
                    key: id_key("person:ada"),
                },
                target: CocoIndexEndpoint {
                    label: "Event".to_string(),
                    key: id_key("event:123"),
                },
                key: id_key("rsvp:1"),
                properties: JsonMap::new(),
            }],
        };

        let graph = cocoindex_export_to_graph(export).expect("import");

        assert_eq!(graph.edges[0].id, Some(EdgeId::new("rsvp:1")));
    }

    #[test]
    fn import_rejects_malformed_keys_and_endpoint_label_mismatches() {
        let err = cocoindex_export_to_graph(CocoIndexGraphExport {
            nodes: vec![CocoIndexNodeState {
                label: "Person".to_string(),
                key: JsonValue::String("person:ada".to_string()),
                properties: JsonMap::new(),
            }],
            relationships: Vec::new(),
        })
        .expect_err("node key must be an object");
        assert!(err.to_string().contains("node key"));

        let err = cocoindex_export_to_graph(CocoIndexGraphExport {
            nodes: vec![CocoIndexNodeState {
                label: "Person".to_string(),
                key: id_key("person:ada"),
                properties: JsonMap::new(),
            }],
            relationships: vec![CocoIndexRelationshipState {
                rel_type: "RSVPED".to_string(),
                source: CocoIndexEndpoint {
                    label: "Robot".to_string(),
                    key: id_key("person:ada"),
                },
                target: CocoIndexEndpoint {
                    label: "Event".to_string(),
                    key: id_key("event:missing"),
                },
                key: id_key("rsvp:1"),
                properties: JsonMap::new(),
            }],
        })
        .expect_err("source label should match imported node");
        assert!(err.to_string().contains("source endpoint"));
    }

    #[test]
    fn missing_endpoint_node_is_an_error() {
        let graph = Graph::new(
            vec![Node::new("Person", "person:ada", Props::new())],
            vec![Edge::new(
                "RSVPED",
                "person:ada",
                "event:missing",
                Props::new(),
            )],
        );

        let err = graph.to_cocoindex_export().expect_err("missing target");
        assert!(err.to_string().contains("missing target node"));
    }

    #[test]
    fn missing_source_node_is_an_error() {
        let graph = Graph::new(
            vec![Node::new("Event", "event:123", Props::new())],
            vec![Edge::new(
                "RSVPED",
                "person:missing",
                "event:123",
                Props::new(),
            )],
        );

        let err = graph.to_cocoindex_export().expect_err("missing source");

        assert!(err.to_string().contains("missing source node"));
    }

    #[test]
    fn non_finite_float_properties_are_export_errors() {
        let mut props = Props::new();
        props.insert("score".to_string(), Value::Float(f64::NAN));
        let graph = Graph::new(vec![Node::new("Person", "person:ada", props)], Vec::new());

        let err = graph
            .to_cocoindex_export()
            .expect_err("NaN should not serialize to JSON");

        assert!(err.to_string().contains("non-finite float"));
    }

    #[test]
    fn non_finite_float_array_properties_are_export_errors() {
        let mut props = Props::new();
        props.insert("scores".to_string(), Value::FloatArray(vec![1.0, f64::NAN]));
        let graph = Graph::new(vec![Node::new("Person", "person:ada", props)], Vec::new());

        let err = graph
            .to_cocoindex_export()
            .expect_err("NaN should not serialize to JSON");

        assert!(err.to_string().contains("non-finite float"));
    }

    #[test]
    fn exported_json_uses_plain_property_values() {
        let export = meetup_graph().to_cocoindex_export().expect("export");
        let json = serde_json::to_value(export).expect("json");

        let nodes = json["nodes"].as_array().expect("nodes");
        let event = nodes
            .iter()
            .find(|node| node["label"] == "Event")
            .expect("event node");
        assert_eq!(event["properties"]["capacity"], 80);

        let relationships = json["relationships"].as_array().expect("relationships");
        let rsvp = relationships
            .iter()
            .find(|relationship| relationship["rel_type"] == "RSVPED")
            .expect("rsvp relationship");
        assert_eq!(rsvp["properties"]["status"], "yes");
    }
}
