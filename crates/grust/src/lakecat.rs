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

#[derive(Clone, Debug, PartialEq)]
pub struct LakeCatCatalogEvent {
    pub event_id: Option<String>,
    pub subject: String,
    pub label: String,
    pub action: String,
    pub emitted_at: String,
    pub properties: JsonValue,
    pub table: Option<LakeCatTableRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LakeCatTableRef {
    pub stable_id: String,
    pub warehouse: String,
    pub namespace: Vec<String>,
    pub name: String,
}

impl LakeCatTableRef {
    pub fn namespace_path(&self) -> String {
        self.namespace.join(".")
    }

    pub fn warehouse_id(&self) -> String {
        lakecat_warehouse_id(&self.warehouse)
    }

    pub fn namespace_id(&self) -> String {
        lakecat_namespace_id(&self.warehouse, &self.namespace)
    }
}

pub fn lakecat_catalog_event_graph(event: &LakeCatCatalogEvent) -> Graph {
    let mut builder = Graph::builder();
    let event_id = event
        .event_id
        .clone()
        .unwrap_or_else(|| lakecat_event_id(&event.subject, &event.action, &event.emitted_at));
    let _ = builder
        .node("CatalogEvent", event_id.clone())
        .prop("subject", event.subject.clone())
        .prop("label", event.label.clone())
        .prop("action", event.action.clone())
        .prop("emitted_at", event.emitted_at.clone())
        .prop("properties", event.properties.clone())
        .finish();

    if let Some(table) = &event.table {
        let warehouse_id = table.warehouse_id();
        let namespace_id = table.namespace_id();
        let namespace_path = table.namespace_path();
        let _ = builder
            .node("Warehouse", warehouse_id.clone())
            .prop("name", table.warehouse.clone())
            .finish();
        let _ = builder
            .node("Namespace", namespace_id.clone())
            .prop("warehouse", table.warehouse.clone())
            .prop("path", namespace_path.clone())
            .prop("segments", JsonValue::from(table.namespace.clone()))
            .finish();
        let _ = builder
            .node("Table", table.stable_id.clone())
            .prop("warehouse", table.warehouse.clone())
            .prop("namespace", namespace_path)
            .prop("name", table.name.clone())
            .finish();
        let _ = builder
            .edge(
                "CONTAINS_NAMESPACE",
                warehouse_id.clone(),
                namespace_id.clone(),
            )
            .finish();
        let _ = builder
            .edge("CONTAINS_TABLE", namespace_id, table.stable_id.clone())
            .finish();
        let _ = builder
            .edge("AFFECTS_TABLE", event_id, table.stable_id.clone())
            .prop("action", event.action.clone())
            .finish();
    }

    builder.build()
}

pub fn lakecat_warehouse_id(warehouse: &str) -> String {
    format!("lakecat:warehouse:{warehouse}")
}

pub fn lakecat_namespace_id(warehouse: &str, namespace: &[String]) -> String {
    format!(
        "{}:namespace:{}",
        lakecat_warehouse_id(warehouse),
        namespace.join(".")
    )
}

pub fn lakecat_event_id(subject: &str, action: &str, emitted_at: &str) -> String {
    format!("lakecat:event:{subject}:{action}:{emitted_at}")
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
    use grust_core::NodeId;
    use serde_json::json;

    fn has_edge(graph: &Graph, label: &str, from: &str, to: &str) -> bool {
        let from = NodeId::new(from);
        let to = NodeId::new(to);
        graph
            .edges
            .iter()
            .any(|edge| edge.label.as_str() == label && edge.from == from && edge.to == to)
    }

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

    #[test]
    fn projects_lakecat_catalog_event_taxonomy() {
        let event = LakeCatCatalogEvent {
            event_id: Some("lakecat:outbox:evt-1".to_string()),
            subject: "lakecat:table:local:default:events".to_string(),
            label: "Table".to_string(),
            action: "created".to_string(),
            emitted_at: "2026-06-17T12:00:00Z".to_string(),
            properties: json!({"metadata-location": "file:///tmp/events/metadata/00000.json"}),
            table: Some(LakeCatTableRef {
                stable_id: "lakecat:table:local:default:events".to_string(),
                warehouse: "local".to_string(),
                namespace: vec!["default".to_string()],
                name: "events".to_string(),
            }),
        };

        let graph = lakecat_catalog_event_graph(&event);
        let catalog_graph = LakeCatCatalogGraph {
            index: GraphIndex::new(&graph).unwrap(),
            graph,
        };

        assert_eq!(catalog_graph.node_count(), 4);
        assert_eq!(catalog_graph.edge_count(), 3);
        assert!(has_edge(
            &catalog_graph.graph,
            "CONTAINS_NAMESPACE",
            "lakecat:warehouse:local",
            "lakecat:warehouse:local:namespace:default"
        ));
        assert!(has_edge(
            &catalog_graph.graph,
            "CONTAINS_TABLE",
            "lakecat:warehouse:local:namespace:default",
            "lakecat:table:local:default:events"
        ));
        assert!(has_edge(
            &catalog_graph.graph,
            "AFFECTS_TABLE",
            "lakecat:outbox:evt-1",
            "lakecat:table:local:default:events"
        ));
    }
}
