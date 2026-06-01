use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose};
use grust_core::prelude::*;
use surrealdb::{
    Surreal,
    engine::remote::ws::{Client as WsClient, Ws},
    opt::auth::Root,
};

#[derive(Clone, Debug)]
pub struct SurrealConfig {
    pub url: String,
    pub user: String,
    pub pass: String,
    pub namespace: String,
    pub database: String,
    pub batch_size: usize,
    pub labels: Vec<String>,
    pub relationships: Vec<String>,
}

impl Default for SurrealConfig {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:8000/sql".to_string(),
            user: "root".to_string(),
            pass: "root".to_string(),
            namespace: "test".to_string(),
            database: "graph".to_string(),
            batch_size: 100,
            labels: Vec::new(),
            relationships: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SurrealHttpGraphStore {
    config: SurrealConfig,
    client: reqwest::Client,
}

impl SurrealHttpGraphStore {
    pub fn connect(config: SurrealConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|err| {
                GrustError::Backend(format!("failed to build SurrealDB HTTP client: {err}"))
            })?;
        Ok(Self { config, client })
    }

    async fn post(&self, query: &str) -> Result<()> {
        let auth =
            general_purpose::STANDARD.encode(format!("{}:{}", self.config.user, self.config.pass));
        let response = self
            .client
            .post(&self.config.url)
            .header("Authorization", format!("Basic {auth}"))
            .header("Surreal-NS", &self.config.namespace)
            .header("Surreal-DB", &self.config.database)
            .header("Accept", "application/json")
            .header("Content-Type", "application/surrealql")
            .body(query.to_string())
            .send()
            .await
            .map_err(|err| {
                GrustError::Backend(format!(
                    "failed to POST SurrealQL to {}: {err}",
                    self.config.url
                ))
            })?;
        check_surreal_http_response(response, "SurrealDB query").await
    }

    async fn post_bootstrap(&self, query: &str) -> Result<()> {
        let auth =
            general_purpose::STANDARD.encode(format!("{}:{}", self.config.user, self.config.pass));
        let response = self
            .client
            .post(&self.config.url)
            .header("Authorization", format!("Basic {auth}"))
            .header("Accept", "application/json")
            .header("Content-Type", "application/surrealql")
            .body(query.to_string())
            .send()
            .await
            .map_err(|err| {
                GrustError::Backend(format!(
                    "failed to bootstrap SurrealDB at {}: {err}",
                    self.config.url
                ))
            })?;
        check_surreal_http_bootstrap_response(response).await
    }

    async fn post_clear(&self, query: &str) -> Result<()> {
        let auth =
            general_purpose::STANDARD.encode(format!("{}:{}", self.config.user, self.config.pass));
        let response = self
            .client
            .post(&self.config.url)
            .header("Authorization", format!("Basic {auth}"))
            .header("Surreal-NS", &self.config.namespace)
            .header("Surreal-DB", &self.config.database)
            .header("Accept", "application/json")
            .header("Content-Type", "application/surrealql")
            .body(query.to_string())
            .send()
            .await
            .map_err(|err| {
                GrustError::Backend(format!(
                    "failed to clear SurrealDB tables at {}: {err}",
                    self.config.url
                ))
            })?;
        check_surreal_http_clear_response(response).await
    }
}

#[async_trait]
impl GraphStore for SurrealHttpGraphStore {
    async fn put_node(&self, node: &Node) -> Result<NodeId> {
        self.post(&surreal_upsert_nodes_query(std::slice::from_ref(node))?)
            .await?;
        Ok(node.id.clone())
    }

    async fn put_edge(&self, edge: &Edge) -> Result<Option<EdgeId>> {
        let id_tables = edge_id_tables(edge);
        self.post(&surreal_relate_edges_query(
            std::slice::from_ref(edge),
            &id_tables,
        )?)
        .await?;
        Ok(edge.id.clone())
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        let id_tables = surreal_id_tables(&graph.nodes)?;
        let mut report = LoadReport::default();
        for chunk in graph.nodes.chunks(self.config.batch_size.max(1)) {
            self.post(&surreal_upsert_nodes_query(chunk)?).await?;
            report.nodes += chunk.len();
        }
        for chunk in graph.edges.chunks(self.config.batch_size.max(1)) {
            self.post(&surreal_relate_edges_query(chunk, &id_tables)?)
                .await?;
            report.edges += chunk.len();
        }
        Ok(report)
    }

    async fn get_node(&self, _id: &NodeId) -> Result<Option<Node>> {
        Err(GrustError::Unsupported(
            "SurrealHttpGraphStore does not implement reads yet".to_string(),
        ))
    }

    async fn get_edges(&self, _query: EdgeQuery) -> Result<Vec<Edge>> {
        Err(GrustError::Unsupported(
            "SurrealHttpGraphStore does not implement reads yet".to_string(),
        ))
    }

    async fn traverse(&self, _traversal: Traversal) -> Result<Vec<Node>> {
        Err(GrustError::Unsupported(
            "SurrealHttpGraphStore does not implement traversal yet".to_string(),
        ))
    }
}

#[async_trait]
impl GraphAdminStore for SurrealHttpGraphStore {
    async fn bootstrap(&self) -> Result<()> {
        self.post_bootstrap(&surreal_bootstrap_query(&self.config))
            .await
    }

    async fn clear(&self) -> Result<()> {
        self.post_clear(&surreal_delete_tables_query(&self.config))
            .await
    }
}

#[derive(Clone, Debug)]
pub struct SurrealSdkGraphStore {
    config: SurrealConfig,
    db: Surreal<WsClient>,
}

impl SurrealSdkGraphStore {
    pub async fn connect(config: SurrealConfig) -> Result<Self> {
        let address = surreal_ws_address(&config.url)?;
        let db = Surreal::new::<Ws>(&address).await.map_err(|err| {
            GrustError::Backend(format!(
                "failed to connect to SurrealDB at {address}: {err}"
            ))
        })?;
        db.signin(Root {
            username: config.user.clone(),
            password: config.pass.clone(),
        })
        .await
        .map_err(|err| {
            GrustError::Backend(format!("failed to authenticate with SurrealDB: {err}"))
        })?;
        Ok(Self { config, db })
    }

    async fn select_database(&self) -> Result<()> {
        self.db
            .use_ns(&self.config.namespace)
            .use_db(&self.config.database)
            .await
            .map(|_| ())
            .map_err(|err| {
                GrustError::Backend(format!(
                    "failed to select SurrealDB namespace/database: {err}"
                ))
            })
    }

    async fn query(&self, query: &str) -> Result<()> {
        self.db
            .query(query)
            .await
            .map(|_| ())
            .map_err(|err| GrustError::Backend(format!("SurrealDB SDK query failed: {err}")))
    }
}

#[async_trait]
impl GraphStore for SurrealSdkGraphStore {
    async fn put_node(&self, node: &Node) -> Result<NodeId> {
        self.select_database().await?;
        self.query(&surreal_upsert_nodes_query(std::slice::from_ref(node))?)
            .await?;
        Ok(node.id.clone())
    }

    async fn put_edge(&self, edge: &Edge) -> Result<Option<EdgeId>> {
        self.select_database().await?;
        let id_tables = edge_id_tables(edge);
        self.query(&surreal_relate_edges_query(
            std::slice::from_ref(edge),
            &id_tables,
        )?)
        .await?;
        Ok(edge.id.clone())
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        self.select_database().await?;
        let id_tables = surreal_id_tables(&graph.nodes)?;
        let mut report = LoadReport::default();
        for chunk in graph.nodes.chunks(self.config.batch_size.max(1)) {
            self.query(&surreal_upsert_nodes_query(chunk)?).await?;
            report.nodes += chunk.len();
        }
        for chunk in graph.edges.chunks(self.config.batch_size.max(1)) {
            self.query(&surreal_relate_edges_query(chunk, &id_tables)?)
                .await?;
            report.edges += chunk.len();
        }
        Ok(report)
    }

    async fn get_node(&self, _id: &NodeId) -> Result<Option<Node>> {
        Err(GrustError::Unsupported(
            "SurrealSdkGraphStore does not implement reads yet".to_string(),
        ))
    }

    async fn get_edges(&self, _query: EdgeQuery) -> Result<Vec<Edge>> {
        Err(GrustError::Unsupported(
            "SurrealSdkGraphStore does not implement reads yet".to_string(),
        ))
    }

    async fn traverse(&self, _traversal: Traversal) -> Result<Vec<Node>> {
        Err(GrustError::Unsupported(
            "SurrealSdkGraphStore does not implement traversal yet".to_string(),
        ))
    }
}

#[async_trait]
impl GraphAdminStore for SurrealSdkGraphStore {
    async fn bootstrap(&self) -> Result<()> {
        match self.db.query(surreal_bootstrap_query(&self.config)).await {
            Ok(_) => {
                self.select_database().await?;
                Ok(())
            }
            Err(err) if err.to_string().contains("already exists") => {
                self.select_database().await?;
                Ok(())
            }
            Err(err) => Err(GrustError::Backend(format!(
                "SurrealDB SDK bootstrap failed: {err}"
            ))),
        }
    }

    async fn clear(&self) -> Result<()> {
        self.select_database().await?;
        self.query(&surreal_delete_tables_query(&self.config)).await
    }
}

async fn check_surreal_http_response(response: reqwest::Response, context: &str) -> Result<()> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| GrustError::Backend(format!("failed to read SurrealDB response: {err}")))?;
    if !status.is_success() {
        return Err(GrustError::Backend(format!(
            "{context} failed with status {status}: {body}"
        )));
    }
    if let Ok(results) = serde_json::from_str::<serde_json::Value>(&body) {
        if surreal_response_has_error(&results) {
            return Err(GrustError::Backend(format!(
                "{context} returned an error: {body}"
            )));
        }
    }
    Ok(())
}

async fn check_surreal_http_bootstrap_response(response: reqwest::Response) -> Result<()> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| GrustError::Backend(format!("failed to read SurrealDB response: {err}")))?;
    if !status.is_success() {
        return Err(GrustError::Backend(format!(
            "SurrealDB bootstrap failed with status {status}: {body}"
        )));
    }
    if let Ok(results) = serde_json::from_str::<serde_json::Value>(&body) {
        if surreal_response_has_non_idempotent_error(&results) {
            return Err(GrustError::Backend(format!(
                "SurrealDB bootstrap returned an error: {body}"
            )));
        }
    }
    Ok(())
}

async fn check_surreal_http_clear_response(response: reqwest::Response) -> Result<()> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| GrustError::Backend(format!("failed to read SurrealDB response: {err}")))?;
    if !status.is_success() {
        return Err(GrustError::Backend(format!(
            "SurrealDB clear failed with status {status}: {body}"
        )));
    }
    if let Ok(results) = serde_json::from_str::<serde_json::Value>(&body) {
        if surreal_response_has_non_idempotent_clear_error(&results) {
            return Err(GrustError::Backend(format!(
                "SurrealDB clear returned an error: {body}"
            )));
        }
    }
    Ok(())
}

fn surreal_response_has_error(value: &serde_json::Value) -> bool {
    value.as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.get("status").and_then(|status| status.as_str()) == Some("ERR"))
    })
}

fn surreal_response_has_non_idempotent_error(value: &serde_json::Value) -> bool {
    value.as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item.get("status").and_then(|status| status.as_str()) == Some("ERR")
                && item.get("kind").and_then(|kind| kind.as_str()) != Some("AlreadyExists")
        })
    })
}

fn surreal_response_has_non_idempotent_clear_error(value: &serde_json::Value) -> bool {
    value.as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item.get("status").and_then(|status| status.as_str()) == Some("ERR")
                && !surreal_error_is_missing_table(item)
        })
    })
}

fn surreal_error_is_missing_table(item: &serde_json::Value) -> bool {
    item.get("kind").and_then(|kind| kind.as_str()) == Some("NotFound")
        && item
            .get("details")
            .and_then(|details| details.get("kind"))
            .and_then(|kind| kind.as_str())
            == Some("Table")
}

fn surreal_bootstrap_query(config: &SurrealConfig) -> String {
    format!(
        "DEFINE NAMESPACE {}; USE NS {}; DEFINE DATABASE {};",
        surreal_identifier(&config.namespace),
        surreal_identifier(&config.namespace),
        surreal_identifier(&config.database)
    )
}

fn surreal_delete_tables_query(config: &SurrealConfig) -> String {
    let mut tables = config
        .labels
        .iter()
        .map(|label| surreal_table_name(label))
        .collect::<BTreeSet<_>>();
    tables.extend(
        config
            .relationships
            .iter()
            .map(|relationship| surreal_table_name(&relationship_type(relationship))),
    );
    tables.insert("record".to_string());
    tables
        .into_iter()
        .map(|table| format!("DELETE {table};"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn surreal_upsert_nodes_query(nodes: &[Node]) -> Result<String> {
    nodes
        .iter()
        .map(|node| {
            Ok(format!(
                "UPSERT type::record({}, {}) SET {};",
                surreal_string(&surreal_table_name(node.label.as_str())),
                surreal_string(node.id.as_str()),
                surreal_node_props(node)?
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map(|statements| statements.join("\n"))
}

fn surreal_node_props(node: &Node) -> Result<String> {
    Ok(node
        .props
        .iter()
        .filter(|(key, _)| key.as_str() != "labels")
        .map(|(key, value)| Ok(format!("{key} = {}", surreal_value(value)?)))
        .collect::<Result<Vec<_>>>()?
        .join(", "))
}

fn surreal_relate_edges_query(
    edges: &[Edge],
    id_tables: &BTreeMap<String, String>,
) -> Result<String> {
    edges
        .iter()
        .map(|edge| {
            let from_table = id_tables
                .get(edge.from.as_str())
                .cloned()
                .unwrap_or_else(|| node_id_table(edge.from.as_str()));
            let to_table = id_tables
                .get(edge.to.as_str())
                .cloned()
                .unwrap_or_else(|| node_id_table(edge.to.as_str()));
            let from = format!(
                "type::record({}, {})",
                surreal_string(&from_table),
                surreal_string(edge.from.as_str())
            );
            let to = format!(
                "type::record({}, {})",
                surreal_string(&to_table),
                surreal_string(edge.to.as_str())
            );
            let table = surreal_table_name(&relationship_type(edge.label.as_str()));
            Ok(format!(
                "DELETE {table} WHERE in = {from} AND out = {to};\nRELATE ({from})->{table}->({to}) SET {};",
                surreal_edge_props(edge)?
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map(|statements| statements.join("\n"))
}

fn surreal_edge_props(edge: &Edge) -> Result<String> {
    let mut props = vec![format!(
        "relationship = {}",
        surreal_string(edge.label.as_str())
    )];
    props.extend(
        edge.props
            .iter()
            .map(|(key, value)| Ok(format!("{key} = {}", surreal_value(value)?)))
            .collect::<Result<Vec<_>>>()?,
    );
    Ok(props.join(", "))
}

fn surreal_id_tables(nodes: &[Node]) -> Result<BTreeMap<String, String>> {
    nodes
        .iter()
        .map(|node| {
            Ok((
                node.id.as_str().to_string(),
                surreal_table_name(node.label.as_str()),
            ))
        })
        .collect()
}

fn edge_id_tables(edge: &Edge) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            edge.from.as_str().to_string(),
            node_id_table(edge.from.as_str()),
        ),
        (
            edge.to.as_str().to_string(),
            node_id_table(edge.to.as_str()),
        ),
    ])
}

fn node_id_table(id: &str) -> String {
    id.split_once(':')
        .map(|(prefix, _)| surreal_table_name(prefix))
        .unwrap_or_else(|| "record".to_string())
}

fn surreal_value(value: &Value) -> Result<String> {
    match value {
        Value::Null => Ok("NONE".to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Int(value) => Ok(value.to_string()),
        Value::Float(value) => Ok(value.to_string()),
        Value::String(value) => Ok(surreal_string(value)),
        Value::StringArray(values) => {
            serde_json::to_string(values).map_err(|err| GrustError::Serialization(err.to_string()))
        }
        Value::Json(value) => {
            serde_json::to_string(value).map_err(|err| GrustError::Serialization(err.to_string()))
        }
    }
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

fn surreal_table_name(value: &str) -> String {
    let table = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    if table.is_empty() {
        "related_to".to_string()
    } else {
        table
    }
}

fn surreal_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization cannot fail")
}

fn surreal_identifier(value: &str) -> String {
    let identifier = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    if identifier.is_empty() {
        "default".to_string()
    } else {
        identifier
    }
}

fn surreal_ws_address(surreal_url: &str) -> Result<String> {
    let parsed = url::Url::parse(surreal_url).map_err(|err| {
        GrustError::Backend(format!("invalid SurrealDB URL {surreal_url}: {err}"))
    })?;
    let host = parsed
        .host_str()
        .ok_or_else(|| GrustError::Backend(format!("SurrealDB URL has no host: {surreal_url}")))?;
    Ok(match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
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
    fn upsert_nodes_preserves_arrays() {
        let graph = sample_graph();
        let query = surreal_upsert_nodes_query(&graph.nodes).unwrap();
        assert!(query.contains("UPSERT type::record(\"talk\", \"talk-1\")"));
        assert!(query.contains("tags = [\"rust\",\"graphs\"]"));
    }

    #[test]
    fn relate_edges_are_idempotent_by_endpoints() {
        let graph = sample_graph();
        let id_tables = surreal_id_tables(&graph.nodes).unwrap();
        let query = surreal_relate_edges_query(&graph.edges, &id_tables).unwrap();
        assert!(
            query.contains("DELETE presents WHERE in = type::record(\"person\", \"person-1\")")
        );
        assert!(query.contains("RELATE (type::record(\"person\", \"person-1\"))->presents->(type::record(\"talk\", \"talk-1\"))"));
        assert!(query.contains("relationship = \"presents\""));
    }

    #[test]
    fn sdk_uses_ws_address_from_http_sql_url() {
        assert_eq!(
            surreal_ws_address("http://127.0.0.1:8000/sql").unwrap(),
            "127.0.0.1:8000"
        );
    }

    #[test]
    fn clear_response_ignores_missing_tables() {
        let response = serde_json::json!([
            {
                "status": "ERR",
                "kind": "NotFound",
                "details": {"kind": "Table", "details": {"name": "announcement"}},
                "result": "The table 'announcement' does not exist"
            },
            {"status": "OK", "result": []}
        ]);

        assert!(!surreal_response_has_non_idempotent_clear_error(&response));
    }

    #[test]
    fn clear_response_keeps_non_missing_table_errors() {
        let response = serde_json::json!([
            {
                "status": "ERR",
                "kind": "Other",
                "result": "permission denied"
            }
        ]);

        assert!(surreal_response_has_non_idempotent_clear_error(&response));
    }
}
