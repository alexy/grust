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

pub fn edge_key(edge: &Edge) -> String {
    edge.id
        .as_ref()
        .map(EdgeId::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            format!(
                "{}\u{1f}{}\u{1f}{}",
                edge.from.as_str(),
                edge.label.as_str(),
                edge.to.as_str()
            )
        })
}

fn id_key(id: &str) -> JsonValue {
    let mut key = JsonMap::new();
    key.insert("id".to_string(), JsonValue::String(id.to_string()));
    JsonValue::Object(key)
}

fn props_to_json_object(props: &Props) -> Result<JsonMap<String, JsonValue>> {
    props
        .iter()
        .map(|(key, value)| Ok((key.clone(), value_to_json(value)?)))
        .collect()
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
        builder
            .node("Group", "meetup:rust-sf")
            .prop("name", "Rust SF")
            .finish();
        builder
            .node("Event", "event:123")
            .prop("title", "Async Rust Night")
            .prop("capacity", 80i64)
            .finish();
        builder
            .node("Person", "person:ada")
            .prop("name", "Ada")
            .finish();
        builder
            .edge("HOSTED", "meetup:rust-sf", "event:123")
            .finish();
        builder
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
