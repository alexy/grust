use std::time::Duration;

use async_trait::async_trait;
use grust_core::prelude::*;
use helix_db::{
    Client as HelixClient, DynamicQueryRequest,
    dsl::prelude::{
        DateTime as HelixDateTime, NodeRef, PropertyInput, PropertyValue, SourcePredicate, g,
        write_batch,
    },
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
    client: reqwest::Client,
}

impl HelixHttpGraphStore {
    pub fn connect(config: HelixHttpConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|err| {
                GrustError::Backend(format!("failed to build Helix HTTP client: {err}"))
            })?;
        Ok(Self { config, client })
    }

    async fn post(&self, request: &serde_json::Value) -> Result<()> {
        let response = self
            .client
            .post(&self.config.query_url)
            .json(request)
            .send()
            .await
            .map_err(|err| {
                GrustError::Backend(format!(
                    "failed to POST Helix query to {}: {err}",
                    self.config.query_url
                ))
            })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| GrustError::Backend(format!("failed to read Helix response: {err}")))?;
        if !status.is_success() {
            return Err(GrustError::Backend(format!(
                "Helix query failed with status {status}: {body}"
            )));
        }
        Ok(())
    }

    async fn read(&self, request: &serde_json::Value) -> Result<serde_json::Value> {
        let response = self
            .client
            .post(&self.config.query_url)
            .json(request)
            .send()
            .await
            .map_err(|err| {
                GrustError::Backend(format!(
                    "failed to POST Helix query to {}: {err}",
                    self.config.query_url
                ))
            })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| GrustError::Backend(format!("failed to read Helix response: {err}")))?;
        if !status.is_success() {
            return Err(GrustError::Backend(format!(
                "Helix query failed with status {status}: {body}"
            )));
        }
        serde_json::from_str(&body)
            .map_err(|err| GrustError::Serialization(format!("invalid Helix response: {err}")))
    }
}

#[async_trait]
impl GraphStore for HelixHttpGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        validate_helix_schema(schema)
    }

    async fn put_node(&self, node: &Node) -> Result<PutOutcome> {
        self.post(&helix_add_nodes_request(std::slice::from_ref(node))?)
            .await?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        self.post(&helix_add_edges_request(std::slice::from_ref(edge))?)
            .await?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        let mut report = LoadReport::default();
        for chunk in graph.nodes.chunks(self.config.batch_size.max(1)) {
            self.post(&helix_add_nodes_request(chunk)?).await?;
            report.nodes += chunk.len();
        }
        for chunk in graph.edges.chunks(self.config.batch_size.max(1)) {
            self.post(&helix_add_edges_request(chunk)?).await?;
            report.edges += chunk.len();
        }
        Ok(report)
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        let request = helix_get_node_request(id);
        let response = self.read(&request).await?;
        Ok(helix_nodes_from_response(&response, "nodes")?
            .into_iter()
            .next())
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        let request = helix_get_edges_request(&query)?;
        let response = self.read(&request).await?;
        let mut edges = helix_edges_from_response(&response, "edges")?;
        helix_filter_edges(&mut edges, &query);
        Ok(edges)
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        let request = helix_traversal_request(&traversal)?;
        let response = self.read(&request).await?;
        let mut nodes = helix_nodes_from_response(&response, "nodes")?;
        if let Some(limit) = traversal.limit {
            nodes.truncate(limit as usize);
        }
        Ok(nodes)
    }
}

#[async_trait]
impl GraphAdminStore for HelixHttpGraphStore {
    async fn clear(&self) -> Result<()> {
        if self.config.labels.is_empty() {
            return Ok(());
        }
        self.post(&helix_drop_labels_request(&self.config.labels))
            .await
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
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        validate_helix_schema(schema)
    }

    async fn put_node(&self, node: &Node) -> Result<PutOutcome> {
        post_helix_sdk_nodes(&self.client, std::slice::from_ref(node)).await?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        post_helix_sdk_edges(&self.client, std::slice::from_ref(edge)).await?;
        Ok(PutOutcome::Upserted)
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

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        let response = send_helix_sdk_read(&self.client, helix_get_node_request(id)).await?;
        Ok(helix_nodes_from_response(&response, "nodes")?
            .into_iter()
            .next())
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        let response = send_helix_sdk_read(&self.client, helix_get_edges_request(&query)?).await?;
        let mut edges = helix_edges_from_response(&response, "edges")?;
        helix_filter_edges(&mut edges, &query);
        Ok(edges)
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        let response =
            send_helix_sdk_read(&self.client, helix_traversal_request(&traversal)?).await?;
        let mut nodes = helix_nodes_from_response(&response, "nodes")?;
        if let Some(limit) = traversal.limit {
            nodes.truncate(limit as usize);
        }
        Ok(nodes)
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
    let mut queries = Vec::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        queries.push(json!({
                "Query": {
                    "name": format!("created_{index}"),
                    "steps": [{
                        "AddN": {
                            "label": node.label.as_str(),
                            "properties": helix_http_properties(node)?
                        }
                    }],
                    "condition": null
                }
        }));
    }
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

fn helix_http_properties(node: &Node) -> Result<Vec<serde_json::Value>> {
    node.props
        .iter()
        .filter_map(|(key, value)| {
            if key == "labels" {
                None
            } else {
                Some(helix_http_property(key, value))
            }
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
                            "properties": helix_http_edge_properties(edge)?
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

fn helix_http_edge_properties(edge: &Edge) -> Result<Vec<serde_json::Value>> {
    let mut properties = vec![
        json!(["relationship", {"Value": {"String": edge.label.as_str()}}]),
        json!(["from_id", {"Value": {"String": edge.from.as_str()}}]),
        json!(["to_id", {"Value": {"String": edge.to.as_str()}}]),
    ];
    if let Some(id) = &edge.id {
        properties.push(json!(["edge_id", {"Value": {"String": id.as_str()}}]));
    }
    for (key, value) in &edge.props {
        properties.push(helix_http_property(key, value)?);
    }
    Ok(properties)
}

fn helix_http_property(key: &str, value: &Value) -> Result<serde_json::Value> {
    Ok(json!([key, {"Value": helix_property_json(value)?}]))
}

fn helix_property_json(value: &Value) -> Result<serde_json::Value> {
    match value {
        Value::Null => Ok(json!({"Null": null})),
        Value::Bool(value) => Ok(json!({"Boolean": value})),
        Value::Int(value) => Ok(json!({"I64": value})),
        Value::Float(value) => Ok(json!({"F64": value})),
        Value::String(value) => Ok(json!({"String": value})),
        Value::DateTime(value) => {
            let datetime = HelixDateTime::parse_rfc3339(value.as_str()).map_err(|err| {
                GrustError::Serialization(format!("invalid Helix datetime '{value}': {err}"))
            })?;
            Ok(json!({"DateTime": datetime.millis()}))
        }
        Value::Decimal(value) => Ok(json!({"String": value.to_canonical_string()})),
        Value::Duration(value) => Ok(json!({"String": value.to_iso_string()})),
        Value::StringArray(values) => Ok(json!({"StringArray": values})),
        Value::IntArray(values) => Ok(json!({"I64Array": values})),
        Value::FloatArray(values) => Ok(json!({"F64Array": values})),
        Value::Json(_) => Err(GrustError::Unsupported(
            "Helix backend does not support JSON object properties".to_string(),
        )),
    }
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
            g().add_n(node.label.as_str().to_string(), helix_sdk_properties(node)?),
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

fn helix_sdk_properties(node: &Node) -> Result<Vec<(String, PropertyInput)>> {
    node.props
        .iter()
        .filter_map(|(key, value)| {
            if key == "labels" {
                None
            } else {
                Some(helix_property_input(value).map(|value| (key.clone(), value)))
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
                        helix_sdk_edge_properties(edge)?,
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

fn helix_sdk_edge_properties(edge: &Edge) -> Result<Vec<(String, PropertyInput)>> {
    let mut properties = vec![
        (
            "relationship".to_string(),
            PropertyInput::from(edge.label.as_str().to_string()),
        ),
        (
            "from_id".to_string(),
            PropertyInput::from(edge.from.as_str().to_string()),
        ),
        (
            "to_id".to_string(),
            PropertyInput::from(edge.to.as_str().to_string()),
        ),
    ];
    if let Some(id) = &edge.id {
        properties.push((
            "edge_id".to_string(),
            PropertyInput::from(id.as_str().to_string()),
        ));
    }
    for (key, value) in &edge.props {
        properties.push((key.clone(), helix_property_input(value)?));
    }
    Ok(properties)
}

fn helix_property_input(value: &Value) -> Result<PropertyInput> {
    let value = match value {
        Value::Null => PropertyValue::Null,
        Value::Bool(value) => PropertyValue::Bool(*value),
        Value::Int(value) => PropertyValue::I64(*value),
        Value::Float(value) => PropertyValue::F64(*value),
        Value::String(value) => PropertyValue::String(value.clone()),
        Value::DateTime(value) => {
            let datetime = HelixDateTime::parse_rfc3339(value.as_str()).map_err(|err| {
                GrustError::Serialization(format!("invalid Helix datetime '{value}': {err}"))
            })?;
            PropertyValue::DateTime(datetime.millis())
        }
        Value::Decimal(value) => PropertyValue::String(value.to_canonical_string()),
        Value::Duration(value) => PropertyValue::String(value.to_iso_string()),
        Value::StringArray(values) => PropertyValue::StringArray(values.clone()),
        Value::IntArray(values) => PropertyValue::I64Array(values.clone()),
        Value::FloatArray(values) => PropertyValue::F64Array(values.clone()),
        Value::Json(_) => {
            return Err(GrustError::Unsupported(
                "Helix backend does not support JSON object properties".to_string(),
            ));
        }
    };
    Ok(PropertyInput::from(value))
}

async fn send_helix_sdk_read(
    client: &HelixClient,
    request: serde_json::Value,
) -> Result<serde_json::Value> {
    let mut sdk_request = request;
    sdk_request["request_type"] = json!("Read");
    let sdk_request: DynamicQueryRequest = serde_json::from_value(sdk_request)
        .map_err(|err| GrustError::Serialization(format!("invalid Helix SDK read: {err}")))?;
    client
        .query::<serde_json::Value>()
        .dynamic_query(sdk_request)
        .send()
        .await
        .map_err(|err| GrustError::Backend(format!("Helix SDK read failed: {err}")))
}

fn helix_get_node_request(id: &NodeId) -> serde_json::Value {
    helix_read_request(
        "nodes",
        vec![json!({
            "Query": {
                "name": "nodes",
                "steps": [
                    {"NWhere": {"Eq": ["id", {"String": id.as_str()}]}},
                    {"ValueMap": null}
                ],
                "condition": null
            }
        })],
    )
}

fn helix_get_edges_request(query: &EdgeQuery) -> Result<serde_json::Value> {
    let mut predicates = Vec::new();
    if let Some(from) = &query.from {
        predicates.push(json!({"Eq": ["from_id", {"String": from.as_str()}]}));
    }
    if let Some(to) = &query.to {
        predicates.push(json!({"Eq": ["to_id", {"String": to.as_str()}]}));
    }
    if let Some(label) = &query.label {
        predicates.push(json!({"Eq": ["relationship", {"String": label.as_str()}]}));
    }
    Ok(helix_read_request(
        "edges",
        vec![json!({
            "Query": {
                "name": "edges",
                "steps": [
                    {"EWhere": helix_and_predicate(predicates)},
                    "EdgeProperties"
                ],
                "condition": null
            }
        })],
    ))
}

fn helix_traversal_request(traversal: &Traversal) -> Result<serde_json::Value> {
    let mut steps = match &traversal.start {
        Start::Node(id) => vec![json!({"NWhere": {"Eq": ["id", {"String": id.as_str()}]}})],
        Start::NodesByLabel(label) => {
            vec![json!({"NWhere": {"Eq": ["$label", {"String": label.as_str()}]}})]
        }
        Start::NodesByProperty { label, key, value } => vec![json!({
            "NWhere": helix_and_predicate(vec![
                json!({"Eq": ["$label", {"String": label.as_str()}]}),
                json!({"Eq": [key, helix_grust_value(value)?]}),
            ])
        })],
    };
    for step in &traversal.steps {
        let edge = step
            .edge
            .as_ref()
            .map(|label| relationship_type(label.as_str()));
        match step.direction {
            Direction::Out => steps.push(json!({"Out": edge})),
            Direction::In => steps.push(json!({"In": edge})),
            Direction::Both => steps.push(json!({"Both": edge})),
        }
        if let Some(label) = &step.node {
            steps.push(json!({"HasLabel": label.as_str()}));
        }
    }
    steps.push(json!({"ValueMap": null}));
    Ok(helix_read_request(
        "nodes",
        vec![json!({
            "Query": {
                "name": "nodes",
                "steps": steps,
                "condition": null
            }
        })],
    ))
}

fn helix_read_request(return_name: &str, queries: Vec<serde_json::Value>) -> serde_json::Value {
    json!({
        "request_type": "read",
        "query": {"queries": queries, "returns": [return_name]},
        "parameters": {},
        "parameter_types": {}
    })
}

fn helix_and_predicate(predicates: Vec<serde_json::Value>) -> serde_json::Value {
    match predicates.as_slice() {
        [] => json!({"HasKey": "relationship"}),
        [predicate] => predicate.clone(),
        _ => json!({"And": predicates}),
    }
}

fn helix_grust_value(value: &Value) -> Result<serde_json::Value> {
    match value {
        Value::String(value) => Ok(json!({"String": value})),
        Value::Int(value) => Ok(json!({"I64": value})),
        Value::Float(value) => Ok(json!({"F64": value})),
        Value::Bool(value) => Ok(json!({"Boolean": value})),
        _ => Err(GrustError::Unsupported(
            "Helix reads support scalar string, int, float, and bool predicates".to_string(),
        )),
    }
}

fn helix_nodes_from_response(response: &serde_json::Value, name: &str) -> Result<Vec<Node>> {
    helix_response_items(response, name)
        .into_iter()
        .map(helix_node_from_value)
        .collect()
}

fn helix_edges_from_response(response: &serde_json::Value, name: &str) -> Result<Vec<Edge>> {
    helix_response_items(response, name)
        .into_iter()
        .map(helix_edge_from_value)
        .collect()
}

fn helix_response_items(response: &serde_json::Value, name: &str) -> Vec<serde_json::Value> {
    if let Some(items) = response
        .get(name)
        .and_then(|value| value.get("properties"))
        .and_then(|value| value.as_array())
    {
        return items.clone();
    }
    if let Some(items) = response.get(name).and_then(|value| value.as_array()) {
        return items.clone();
    }
    if let Some(items) = response
        .get("results")
        .and_then(|results| results.get(name))
        .and_then(|value| value.get("properties").or(Some(value)))
        .and_then(|value| value.as_array())
    {
        return items.clone();
    }
    if let Some(items) = response
        .get("result")
        .and_then(|results| results.get(name))
        .and_then(|value| value.get("properties").or(Some(value)))
        .and_then(|value| value.as_array())
    {
        return items.clone();
    }
    response.as_array().cloned().unwrap_or_default()
}

fn helix_node_from_value(value: serde_json::Value) -> Result<Node> {
    let object = value
        .as_object()
        .ok_or_else(|| GrustError::Serialization("Helix node row is not an object".to_string()))?;
    let id = object
        .get("id")
        .or_else(|| object.get("$id"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| GrustError::Serialization("Helix node row has no id".to_string()))?;
    let label = object
        .get("$label")
        .or_else(|| object.get("label"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| GrustError::Serialization("Helix node row has no label".to_string()))?;
    let props = object
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "$id" | "id" | "$label" | "label"))
        .map(|(key, value)| (key.clone(), helix_value_from_json(value.clone())))
        .collect::<Props>();
    Ok(Node::new(label, id, props))
}

fn helix_edge_from_value(value: serde_json::Value) -> Result<Edge> {
    let object = value
        .as_object()
        .ok_or_else(|| GrustError::Serialization("Helix edge row is not an object".to_string()))?;
    let from = object
        .get("from_id")
        .or_else(|| object.get("$from"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| GrustError::Serialization("Helix edge row has no from_id".to_string()))?;
    let to = object
        .get("to_id")
        .or_else(|| object.get("$to"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| GrustError::Serialization("Helix edge row has no to_id".to_string()))?;
    let label = object
        .get("relationship")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            GrustError::Serialization("Helix edge row has no relationship".to_string())
        })?;
    let mut edge = Edge::new(
        label,
        from,
        to,
        object
            .iter()
            .filter(|(key, _)| {
                !matches!(
                    key.as_str(),
                    "from_id"
                        | "to_id"
                        | "$from"
                        | "$to"
                        | "relationship"
                        | "edge_id"
                        | "$id"
                        | "$label"
                        | "label"
                )
            })
            .map(|(key, value)| (key.clone(), helix_value_from_json(value.clone())))
            .collect::<Props>(),
    );
    edge.id = object
        .get("edge_id")
        .and_then(|value| value.as_str())
        .map(EdgeId::new);
    Ok(edge)
}

fn helix_value_from_json(value: serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(value) => Value::Bool(value),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Value::Int(value)
            } else {
                Value::Float(value.as_f64().unwrap_or_default())
            }
        }
        serde_json::Value::String(value) => Value::String(value),
        value => Value::Json(value),
    }
}

fn helix_filter_edges(edges: &mut Vec<Edge>, query: &EdgeQuery) {
    edges.retain(|edge| {
        query.from.as_ref().is_none_or(|from| from == &edge.from)
            && query.to.as_ref().is_none_or(|to| to == &edge.to)
            && query
                .label
                .as_ref()
                .is_none_or(|label| label == &edge.label)
    });
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

fn validate_helix_schema(schema: &GraphSchema) -> Result<()> {
    for node_type in &schema.nodes {
        validate_helix_name(node_type.label.as_str())?;
        for field in &node_type.fields {
            validate_helix_name(&field.name)?;
        }
    }
    for edge_type in &schema.edges {
        validate_helix_name(&relationship_type(edge_type.label.as_str()))?;
        for field in &edge_type.fields {
            validate_helix_name(&field.name)?;
        }
    }
    Ok(())
}

fn validate_helix_name(value: &str) -> Result<()> {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        Ok(())
    } else {
        Err(GrustError::Schema(format!(
            "unsafe Helix schema identifier '{value}'"
        )))
    }
}

#[cfg(test)]
mod tests;
