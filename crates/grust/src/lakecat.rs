use std::collections::BTreeMap;

use grust_core::{Edge, EdgeId, Graph, GraphIndex, GrustError, Node, Props, Result, Value};
use serde_json::Value as JsonValue;

#[derive(Clone, Debug, PartialEq)]
pub struct LakeCatCatalogGraph {
    pub graph: Graph,
    pub index: GraphIndex,
}

impl LakeCatCatalogGraph {
    pub fn from_json_value(value: &JsonValue) -> Result<Self> {
        let graph = lakecat_catalog_graph_from_json_value(value)?;
        let index = GraphIndex::new(&graph)?;
        Ok(Self { graph, index })
    }

    pub fn node_count(&self) -> usize {
        self.graph.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.graph.edges.len()
    }
}

pub fn lakecat_catalog_graph_from_json_value(value: &JsonValue) -> Result<Graph> {
    let object = value.as_object().ok_or_else(|| {
        GrustError::Serialization(
            "LakeCat catalog graph envelope must be a JSON object".to_string(),
        )
    })?;
    let nodes = object
        .get("nodes")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            GrustError::Serialization(
                "LakeCat catalog graph envelope is missing a nodes array".to_string(),
            )
        })?
        .iter()
        .map(lakecat_node_from_json_value)
        .collect::<Result<Vec<_>>>()?;
    let edges = object
        .get("edges")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            GrustError::Serialization(
                "LakeCat catalog graph envelope is missing an edges array".to_string(),
            )
        })?
        .iter()
        .map(lakecat_edge_from_json_value)
        .collect::<Result<Vec<_>>>()?;
    Ok(Graph::new(nodes, edges))
}

fn lakecat_node_from_json_value(value: &JsonValue) -> Result<Node> {
    let object = value.as_object().ok_or_else(|| {
        GrustError::Serialization("LakeCat catalog graph node must be a JSON object".to_string())
    })?;
    let id = string_field(object, "id", "LakeCat catalog graph node")?;
    let label = string_field(object, "label", "LakeCat catalog graph node")?;
    let props = props_field(object)?;
    Ok(Node::new(label, id, props))
}

fn lakecat_edge_from_json_value(value: &JsonValue) -> Result<Edge> {
    let object = value.as_object().ok_or_else(|| {
        GrustError::Serialization("LakeCat catalog graph edge must be a JSON object".to_string())
    })?;
    let label = string_field(object, "label", "LakeCat catalog graph edge")?;
    let from = string_field(object, "from", "LakeCat catalog graph edge")?;
    let to = string_field(object, "to", "LakeCat catalog graph edge")?;
    let props = props_field(object)?;
    let mut edge = Edge::new(label, from, to, props);
    if let Some(id) = object.get("id").and_then(JsonValue::as_str) {
        edge = edge.with_id(EdgeId::new(id));
    }
    Ok(edge)
}

fn props_field(object: &serde_json::Map<String, JsonValue>) -> Result<Props> {
    match object.get("properties").or_else(|| object.get("props")) {
        Some(JsonValue::Object(properties)) => Ok(properties
            .iter()
            .map(|(key, value)| (key.clone(), Value::from_json(value.clone())))
            .collect()),
        Some(_) => Err(GrustError::Serialization(
            "LakeCat catalog graph properties must be a JSON object".to_string(),
        )),
        None => Ok(BTreeMap::new()),
    }
}

fn string_field(
    object: &serde_json::Map<String, JsonValue>,
    field: &str,
    context: &str,
) -> Result<String> {
    object
        .get(field)
        .and_then(JsonValue::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| {
            GrustError::Serialization(format!("{context} is missing string field '{field}'"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn imports_lakecat_catalog_graph_envelope() {
        let envelope = json!({
            "nodes": [
                {
                    "id": "lakecat:catalog:local",
                    "label": "Catalog",
                    "properties": {
                        "warehouse": "local",
                        "standards": ["Croissant", "ODRL"],
                        "raw": {"nested": true}
                    }
                },
                {
                    "id": "lakecat:table:local:default:events",
                    "label": "Table",
                    "properties": {"version": 3}
                }
            ],
            "edges": [
                {
                    "id": "edge:catalog-table",
                    "from": "lakecat:catalog:local",
                    "to": "lakecat:table:local:default:events",
                    "label": "HAS_TABLE",
                    "properties": {"source": "lakecat"}
                }
            ]
        });

        let catalog_graph = LakeCatCatalogGraph::from_json_value(&envelope).unwrap();

        assert_eq!(catalog_graph.node_count(), 2);
        assert_eq!(catalog_graph.edge_count(), 1);
        assert_eq!(
            catalog_graph.graph.nodes[0].props.get("warehouse"),
            Some(&Value::String("local".to_string()))
        );
        assert_eq!(
            catalog_graph.graph.edges[0].id.as_ref().map(EdgeId::as_str),
            Some("edge:catalog-table")
        );
    }

    #[test]
    fn rejects_lakecat_catalog_graph_edges_with_unknown_endpoints() {
        let envelope = json!({
            "nodes": [
                {"id": "lakecat:catalog:local", "label": "Catalog"}
            ],
            "edges": [
                {
                    "from": "lakecat:catalog:local",
                    "to": "lakecat:table:missing",
                    "label": "HAS_TABLE"
                }
            ]
        });

        let err = LakeCatCatalogGraph::from_json_value(&envelope)
            .unwrap_err()
            .to_string();
        assert!(err.contains("edge destination"));
        assert!(err.contains("is not present in vertices"));
    }
}
