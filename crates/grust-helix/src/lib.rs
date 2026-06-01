use std::time::Duration;

use async_trait::async_trait;
use grust_core::prelude::*;
use helix_db::{
    Client as HelixClient, DynamicQueryRequest,
    dsl::prelude::{NodeRef, PropertyInput, SourcePredicate, g, write_batch},
};
use serde_json::json;

#[derive(Clone, Debug)]
pub struct HelixHttpConfig {
    pub query_url: String,
    pub batch_size: usize,
    pub labels: Vec<String>,
}

impl Default for HelixHttpConfig {
    fn default() -> Self {
        Self {
            query_url: "http://127.0.0.1:8080/v1/query".to_string(),
            batch_size: 100,
            labels: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct HelixSdkConfig {
    pub base_url: String,
    pub batch_size: usize,
    pub labels: Vec<String>,
}

impl Default for HelixSdkConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:8080".to_string(),
            batch_size: 100,
            labels: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct HelixHttpGraphStore {
    config: HelixHttpConfig,
    client: reqwest::blocking::Client,
}

impl HelixHttpGraphStore {
    pub fn connect(config: HelixHttpConfig) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|err| {
                GrustError::Backend(format!("failed to build Helix HTTP client: {err}"))
            })?;
        Ok(Self { config, client })
    }

    fn post(&self, request: &serde_json::Value) -> Result<()> {
        let response = self
            .client
            .post(&self.config.query_url)
            .json(request)
            .send()
            .map_err(|err| {
                GrustError::Backend(format!(
                    "failed to POST Helix query to {}: {err}",
                    self.config.query_url
                ))
            })?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|err| GrustError::Backend(format!("failed to read Helix response: {err}")))?;
        if !status.is_success() {
            return Err(GrustError::Backend(format!(
                "Helix query failed with status {status}: {body}"
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl GraphStore for HelixHttpGraphStore {
    async fn put_node(&self, node: &Node) -> Result<NodeId> {
        self.post(&helix_add_nodes_request(std::slice::from_ref(node))?)?;
        Ok(node.id.clone())
    }

    async fn put_edge(&self, edge: &Edge) -> Result<Option<EdgeId>> {
        self.post(&helix_add_edges_request(std::slice::from_ref(edge))?)?;
        Ok(edge.id.clone())
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        let mut report = LoadReport::default();
        for chunk in graph.nodes.chunks(self.config.batch_size.max(1)) {
            self.post(&helix_add_nodes_request(chunk)?)?;
            report.nodes += chunk.len();
        }
        for chunk in graph.edges.chunks(self.config.batch_size.max(1)) {
            self.post(&helix_add_edges_request(chunk)?)?;
            report.edges += chunk.len();
        }
        Ok(report)
    }

    async fn get_node(&self, _id: &NodeId) -> Result<Option<Node>> {
        Err(GrustError::Unsupported(
            "HelixHttpGraphStore does not implement reads yet".to_string(),
        ))
    }

    async fn get_edges(&self, _query: EdgeQuery) -> Result<Vec<Edge>> {
        Err(GrustError::Unsupported(
            "HelixHttpGraphStore does not implement reads yet".to_string(),
        ))
    }

    async fn traverse(&self, _traversal: Traversal) -> Result<Vec<Node>> {
        Err(GrustError::Unsupported(
            "HelixHttpGraphStore does not implement traversal yet".to_string(),
        ))
    }
}

#[async_trait]
impl GraphAdminStore for HelixHttpGraphStore {
    async fn clear(&self) -> Result<()> {
        if self.config.labels.is_empty() {
            return Ok(());
        }
        self.post(&helix_drop_labels_request(&self.config.labels))
    }
}

#[derive(Clone, Debug)]
pub struct HelixSdkGraphStore {
    config: HelixSdkConfig,
    client: HelixClient,
}

impl HelixSdkGraphStore {
    pub fn connect(config: HelixSdkConfig) -> Result<Self> {
        let base_url = helix_base_url(&config.base_url);
        let client = HelixClient::new(Some(&base_url)).map_err(|err| {
            GrustError::Backend(format!("failed to build Helix SDK client: {err}"))
        })?;
        Ok(Self {
            config: HelixSdkConfig { base_url, ..config },
            client,
        })
    }
}

#[async_trait]
impl GraphStore for HelixSdkGraphStore {
    async fn put_node(&self, node: &Node) -> Result<NodeId> {
        post_helix_sdk_nodes(&self.client, std::slice::from_ref(node)).await?;
        Ok(node.id.clone())
    }

    async fn put_edge(&self, edge: &Edge) -> Result<Option<EdgeId>> {
        post_helix_sdk_edges(&self.client, std::slice::from_ref(edge)).await?;
        Ok(edge.id.clone())
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        let mut report = LoadReport::default();
        for chunk in graph.nodes.chunks(self.config.batch_size.max(1)) {
            post_helix_sdk_nodes(&self.client, chunk).await?;
            report.nodes += chunk.len();
        }
        for chunk in graph.edges.chunks(self.config.batch_size.max(1)) {
            post_helix_sdk_edges(&self.client, chunk).await?;
            report.edges += chunk.len();
        }
        Ok(report)
    }

    async fn get_node(&self, _id: &NodeId) -> Result<Option<Node>> {
        Err(GrustError::Unsupported(
            "HelixSdkGraphStore does not implement reads yet".to_string(),
        ))
    }

    async fn get_edges(&self, _query: EdgeQuery) -> Result<Vec<Edge>> {
        Err(GrustError::Unsupported(
            "HelixSdkGraphStore does not implement reads yet".to_string(),
        ))
    }

    async fn traverse(&self, _traversal: Traversal) -> Result<Vec<Node>> {
        Err(GrustError::Unsupported(
            "HelixSdkGraphStore does not implement traversal yet".to_string(),
        ))
    }
}

#[async_trait]
impl GraphAdminStore for HelixSdkGraphStore {
    async fn clear(&self) -> Result<()> {
        if self.config.labels.is_empty() {
            return Ok(());
        }
        post_helix_sdk_drop_labels(&self.client, &self.config.labels).await
    }
}

fn helix_add_nodes_request(nodes: &[Node]) -> Result<serde_json::Value> {
    let queries = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            json!({
                "Query": {
                    "name": format!("created_{index}"),
                    "steps": [{
                        "AddN": {
                            "label": node.label.as_str(),
                            "properties": helix_http_properties(node)
                        }
                    }],
                    "condition": null
                }
            })
        })
        .collect::<Vec<_>>();
    let returns = (0..nodes.len())
        .map(|index| format!("created_{index}"))
        .collect::<Vec<_>>();
    Ok(json!({
        "request_type": "write",
        "query": {"queries": queries, "returns": returns},
        "parameters": {},
        "parameter_types": {}
    }))
}

fn helix_http_properties(node: &Node) -> Vec<serde_json::Value> {
    node.props
        .iter()
        .filter_map(|(key, value)| match (key.as_str(), value) {
            ("labels", _) => None,
            (_, Value::String(value)) => Some(json!([key, {"Value": {"String": value}}])),
            (_, _) => None,
        })
        .collect()
}

fn helix_add_edges_request(edges: &[Edge]) -> Result<serde_json::Value> {
    let mut queries = Vec::with_capacity(edges.len() * 2);
    let mut returns = Vec::with_capacity(edges.len());
    for (index, edge) in edges.iter().enumerate() {
        let target_name = format!("target_{index}");
        let linked_name = format!("linked_{index}");
        queries.push(json!({
            "Query": {
                "name": target_name,
                "steps": [{"NWhere": {"Eq": ["id", {"String": edge.to.as_str()}]}}],
                "condition": null
            }
        }));
        queries.push(json!({
            "Query": {
                "name": linked_name,
                "steps": [
                    {"NWhere": {"Eq": ["id", {"String": edge.from.as_str()}]}},
                    {
                        "AddE": {
                            "label": relationship_type(edge.label.as_str()),
                            "to": {"Var": target_name},
                            "properties": helix_http_edge_properties(edge)
                        }
                    }
                ],
                "condition": null
            }
        }));
        returns.push(linked_name);
    }
    Ok(json!({
        "request_type": "write",
        "query": {"queries": queries, "returns": returns},
        "parameters": {},
        "parameter_types": {}
    }))
}

fn helix_http_edge_properties(edge: &Edge) -> Vec<serde_json::Value> {
    edge.props
        .iter()
        .filter_map(|(key, value)| match value {
            Value::String(value) => Some(json!([key, {"Value": {"String": value}}])),
            _ => None,
        })
        .collect()
}

fn helix_drop_labels_request(labels: &[String]) -> serde_json::Value {
    let queries = labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            json!({
                "Query": {
                    "name": format!("drop_{index}"),
                    "steps": [{"NWhere": {"Eq": ["$label", {"String": label}]}}, "Drop"],
                    "condition": null
                }
            })
        })
        .collect::<Vec<_>>();
    json!({
        "request_type": "write",
        "query": {"queries": queries, "returns": []},
        "parameters": {},
        "parameter_types": {}
    })
}

async fn post_helix_sdk_nodes(client: &HelixClient, nodes: &[Node]) -> Result<()> {
    let mut batch = write_batch();
    let mut returns = Vec::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        let name = format!("created_{index}");
        batch = batch.var_as(
            &name,
            g().add_n(node.label.as_str().to_string(), helix_sdk_properties(node)),
        );
        returns.push(name);
    }
    let request = DynamicQueryRequest::write(batch.returning(returns));
    let _: serde_json::Value = client
        .query::<serde_json::Value>()
        .dynamic_query(request)
        .send()
        .await
        .map_err(|err| GrustError::Backend(format!("Helix SDK node write failed: {err}")))?;
    Ok(())
}

fn helix_sdk_properties(node: &Node) -> Vec<(String, PropertyInput)> {
    node.props
        .iter()
        .filter_map(|(key, value)| {
            if key == "labels" {
                return None;
            }
            match value {
                Value::String(value) => Some((key.clone(), PropertyInput::from(value.clone()))),
                _ => None,
            }
        })
        .collect()
}

async fn post_helix_sdk_edges(client: &HelixClient, edges: &[Edge]) -> Result<()> {
    let mut batch = write_batch();
    let mut returns = Vec::with_capacity(edges.len());
    for (index, edge) in edges.iter().enumerate() {
        let target_name = format!("target_{index}");
        let linked_name = format!("linked_{index}");
        batch = batch
            .var_as(
                &target_name,
                g().n_where(SourcePredicate::eq("id", edge.to.as_str().to_string())),
            )
            .var_as(
                &linked_name,
                g().n_where(SourcePredicate::eq("id", edge.from.as_str().to_string()))
                    .add_e(
                        relationship_type(edge.label.as_str()),
                        NodeRef::var(&target_name),
                        helix_sdk_edge_properties(edge),
                    ),
            );
        returns.push(linked_name);
    }
    let request = DynamicQueryRequest::write(batch.returning(returns));
    let _: serde_json::Value = client
        .query::<serde_json::Value>()
        .dynamic_query(request)
        .send()
        .await
        .map_err(|err| GrustError::Backend(format!("Helix SDK edge write failed: {err}")))?;
    Ok(())
}

fn helix_sdk_edge_properties(edge: &Edge) -> Vec<(String, String)> {
    edge.props
        .iter()
        .filter_map(|(key, value)| match value {
            Value::String(value) => Some((key.clone(), value.clone())),
            _ => None,
        })
        .collect()
}

async fn post_helix_sdk_drop_labels(client: &HelixClient, labels: &[String]) -> Result<()> {
    let mut batch = write_batch();
    for (index, label) in labels.iter().enumerate() {
        batch = batch.var_as(
            &format!("drop_{index}"),
            g().n_where(SourcePredicate::eq("$label", label.as_str()))
                .drop(),
        );
    }
    let request = DynamicQueryRequest::write(batch.returning(Vec::<String>::new()));
    let _: serde_json::Value = client
        .query::<serde_json::Value>()
        .dynamic_query(request)
        .send()
        .await
        .map_err(|err| GrustError::Backend(format!("Helix SDK replace/drop failed: {err}")))?;
    Ok(())
}

fn helix_base_url(helix_url: &str) -> String {
    helix_url
        .strip_suffix("/v1/query")
        .unwrap_or(helix_url)
        .trim_end_matches('/')
        .to_string()
}

fn relationship_type(value: &str) -> String {
    let relationship = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    if relationship.is_empty() {
        "RELATED_TO".to_string()
    } else {
        relationship
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_graph() -> Graph {
        let mut talk_props = Props::new();
        talk_props.insert("id".to_string(), Value::from("talk-1"));
        talk_props.insert("title".to_string(), Value::from("A Talk"));
        talk_props.insert(
            "tags".to_string(),
            Value::StringArray(vec!["rust".to_string(), "graphs".to_string()]),
        );

        let mut person_props = Props::new();
        person_props.insert("id".to_string(), Value::from("person-1"));
        person_props.insert("name".to_string(), Value::from("Ada Lovelace"));

        Graph {
            nodes: vec![
                Node::new("Talk", "talk-1", talk_props),
                Node::new("Person", "person-1", person_props),
            ],
            edges: vec![Edge::new("presents", "person-1", "talk-1", Props::new())],
        }
    }

    #[test]
    fn helix_http_node_batch_omits_arrays() {
        let graph = sample_graph();
        let request = helix_add_nodes_request(std::slice::from_ref(&graph.nodes[0])).unwrap();
        let properties = &request["query"]["queries"][0]["Query"]["steps"][0]["AddN"]["properties"];
        assert!(
            !properties
                .as_array()
                .unwrap()
                .iter()
                .any(|prop| prop[0] == "tags")
        );
    }

    #[test]
    fn helix_http_edge_batch_uses_target_variable() {
        let graph = sample_graph();
        let request = helix_add_edges_request(std::slice::from_ref(&graph.edges[0])).unwrap();
        assert_eq!(
            request["query"]["queries"][1]["Query"]["steps"][1]["AddE"]["to"]["Var"],
            "target_0"
        );
    }

    #[test]
    fn strips_v1_query_for_sdk_base_url() {
        assert_eq!(
            helix_base_url("http://127.0.0.1:8080/v1/query"),
            "http://127.0.0.1:8080"
        );
    }
}
