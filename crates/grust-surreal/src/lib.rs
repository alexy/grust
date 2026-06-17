use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use async_trait::async_trait;
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
        let response = self
            .client
            .post(&self.config.url)
            .basic_auth(&self.config.user, Some(&self.config.pass))
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
        let response = self
            .client
            .post(&self.config.url)
            .basic_auth(&self.config.user, Some(&self.config.pass))
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
        let response = self
            .client
            .post(&self.config.url)
            .basic_auth(&self.config.user, Some(&self.config.pass))
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

    async fn read(&self, query: &str) -> Result<Vec<serde_json::Value>> {
        let response = self
            .client
            .post(&self.config.url)
            .basic_auth(&self.config.user, Some(&self.config.pass))
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
        read_surreal_http_response(response, "SurrealDB read").await
    }

    async fn read_nodes(&self, query: &str) -> Result<Vec<Node>> {
        self.read(query)
            .await?
            .into_iter()
            .map(surreal_node_from_value)
            .collect()
    }

    async fn read_edges(&self, query: &str) -> Result<Vec<Edge>> {
        self.read(query)
            .await?
            .into_iter()
            .map(surreal_edge_from_value)
            .collect()
    }
}

#[async_trait]
impl GraphStore for SurrealHttpGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        self.post_bootstrap(&surreal_bootstrap_query(&self.config))
            .await?;
        self.post(&surreal_schema_query(schema)?).await
    }

    async fn put_node(&self, node: &Node) -> Result<PutOutcome> {
        self.post(&surreal_upsert_nodes_query(std::slice::from_ref(node))?)
            .await?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        let id_tables = edge_id_tables(edge);
        self.post(&surreal_relate_edges_query(
            std::slice::from_ref(edge),
            &id_tables,
        )?)
        .await?;
        Ok(PutOutcome::Upserted)
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

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        Ok(self
            .read_nodes(&surreal_get_node_query(id, &self.config))
            .await?
            .into_iter()
            .next())
    }

    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>> {
        self.read_nodes(&surreal_get_nodes_query(ids, &self.config))
            .await
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        let mut edges = self
            .read_edges(&surreal_get_edges_query(&query, &self.config)?)
            .await?;
        filter_edges(&mut edges, &query);
        Ok(edges)
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        let current = self
            .read_nodes(&surreal_start_nodes_query(&traversal.start, &self.config))
            .await?;
        traverse_steps_with_store(self, current, traversal.steps, traversal.limit).await
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

#[async_trait]
impl GraphMutationStore for SurrealHttpGraphStore {
    fn mutation_atomicity(&self) -> GraphMutationAtomicity {
        GraphMutationAtomicity::Transactional
    }

    async fn delete_node(&self, id: &NodeId) -> Result<()> {
        self.post(&surreal_delete_node_query(id, &self.config)?)
            .await
    }

    async fn delete_edge(&self, from: &NodeId, label: &Label, to: &NodeId) -> Result<()> {
        self.post(&surreal_delete_edge_query(from, label, to, &self.config))
            .await
    }

    async fn apply_mutations(&self, mutations: &[GraphMutation]) -> Result<()> {
        if mutations.is_empty() {
            return Ok(());
        }
        self.post(&surreal_apply_mutations_query(mutations, &self.config)?)
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
        db.use_ns(&config.namespace)
            .use_db(&config.database)
            .await
            .map(|_| ())
            .map_err(|err| {
                GrustError::Backend(format!(
                    "failed to select SurrealDB namespace/database: {err}"
                ))
            })?;
        Ok(Self { config, db })
    }

    async fn query(&self, query: &str) -> Result<()> {
        self.db
            .query(query)
            .await
            .map(|_| ())
            .map_err(|err| GrustError::Backend(format!("SurrealDB SDK query failed: {err}")))
    }

    async fn read(&self, query: &str) -> Result<Vec<serde_json::Value>> {
        let mut response = self
            .db
            .query(query)
            .await
            .map_err(|err| GrustError::Backend(format!("SurrealDB SDK read failed: {err}")))?;
        let rows: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|err| GrustError::Backend(format!("SurrealDB SDK read failed: {err}")))?;
        Ok(rows)
    }

    async fn read_nodes(&self, query: &str) -> Result<Vec<Node>> {
        self.read(query)
            .await?
            .into_iter()
            .map(surreal_node_from_value)
            .collect()
    }

    async fn read_edges(&self, query: &str) -> Result<Vec<Edge>> {
        self.read(query)
            .await?
            .into_iter()
            .map(surreal_edge_from_value)
            .collect()
    }
}

#[async_trait]
impl GraphStore for SurrealSdkGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        self.bootstrap().await?;
        self.query(&surreal_schema_query(schema)?).await
    }

    async fn put_node(&self, node: &Node) -> Result<PutOutcome> {
        self.query(&surreal_upsert_nodes_query(std::slice::from_ref(node))?)
            .await?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        let id_tables = edge_id_tables(edge);
        self.query(&surreal_relate_edges_query(
            std::slice::from_ref(edge),
            &id_tables,
        )?)
        .await?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
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

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        Ok(self
            .read_nodes(&surreal_get_node_query(id, &self.config))
            .await?
            .into_iter()
            .next())
    }

    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>> {
        self.read_nodes(&surreal_get_nodes_query(ids, &self.config))
            .await
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        let mut edges = self
            .read_edges(&surreal_get_edges_query(&query, &self.config)?)
            .await?;
        filter_edges(&mut edges, &query);
        Ok(edges)
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        let current = self
            .read_nodes(&surreal_start_nodes_query(&traversal.start, &self.config))
            .await?;
        traverse_steps_with_store(self, current, traversal.steps, traversal.limit).await
    }
}

#[async_trait]
impl GraphAdminStore for SurrealSdkGraphStore {
    async fn bootstrap(&self) -> Result<()> {
        match self.db.query(surreal_bootstrap_query(&self.config)).await {
            Ok(_) => Ok(()),
            Err(err) if err.to_string().contains("already exists") => Ok(()),
            Err(err) => Err(GrustError::Backend(format!(
                "SurrealDB SDK bootstrap failed: {err}"
            ))),
        }
    }

    async fn clear(&self) -> Result<()> {
        self.query(&surreal_delete_tables_query(&self.config)).await
    }
}

#[async_trait]
impl GraphMutationStore for SurrealSdkGraphStore {
    fn mutation_atomicity(&self) -> GraphMutationAtomicity {
        GraphMutationAtomicity::Transactional
    }

    async fn delete_node(&self, id: &NodeId) -> Result<()> {
        self.query(&surreal_delete_node_query(id, &self.config)?)
            .await
    }

    async fn delete_edge(&self, from: &NodeId, label: &Label, to: &NodeId) -> Result<()> {
        self.query(&surreal_delete_edge_query(from, label, to, &self.config))
            .await
    }

    async fn apply_mutations(&self, mutations: &[GraphMutation]) -> Result<()> {
        if mutations.is_empty() {
            return Ok(());
        }
        self.query(&surreal_apply_mutations_query(mutations, &self.config)?)
            .await
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
    if let Ok(results) = serde_json::from_str::<serde_json::Value>(&body)
        && surreal_response_has_error(&results)
    {
        return Err(GrustError::Backend(format!(
            "{context} returned an error: {body}"
        )));
    }
    Ok(())
}

async fn read_surreal_http_response(
    response: reqwest::Response,
    context: &str,
) -> Result<Vec<serde_json::Value>> {
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
    let results = serde_json::from_str::<serde_json::Value>(&body)
        .map_err(|err| GrustError::Serialization(format!("invalid SurrealDB response: {err}")))?;
    if surreal_response_has_error(&results) {
        return Err(GrustError::Backend(format!(
            "{context} returned an error: {body}"
        )));
    }
    Ok(surreal_response_rows(&results))
}

fn surreal_response_rows(value: &serde_json::Value) -> Vec<serde_json::Value> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|item| item.get("result").and_then(|result| result.as_array()))
        .flatten()
        .cloned()
        .collect()
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
    if let Ok(results) = serde_json::from_str::<serde_json::Value>(&body)
        && surreal_response_has_non_idempotent_error(&results)
    {
        return Err(GrustError::Backend(format!(
            "SurrealDB bootstrap returned an error: {body}"
        )));
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
    if let Ok(results) = serde_json::from_str::<serde_json::Value>(&body)
        && surreal_response_has_non_idempotent_clear_error(&results)
    {
        return Err(GrustError::Backend(format!(
            "SurrealDB clear returned an error: {body}"
        )));
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
        "DEFINE NAMESPACE {}; USE NS {}; DEFINE DATABASE {}; USE DB {}; DEFINE TABLE IF NOT EXISTS record;",
        surreal_identifier(&config.namespace),
        surreal_identifier(&config.namespace),
        surreal_identifier(&config.database),
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

fn surreal_get_node_query(id: &NodeId, config: &SurrealConfig) -> String {
    let tables = surreal_node_tables_for_id(id, config);
    let where_clause = tables
        .iter()
        .map(|table| {
            format!(
                "id = type::record({}, {})",
                surreal_string(table),
                surreal_string(id.as_str())
            )
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    format!(
        "SELECT *, meta::tb(id) AS __grust_label FROM {} WHERE {where_clause};",
        tables.join(", ")
    )
}

fn surreal_get_nodes_query(ids: &[NodeId], config: &SurrealConfig) -> String {
    if ids.is_empty() {
        return "RETURN [];".to_string();
    }
    let tables = ids
        .iter()
        .flat_map(|id| surreal_node_tables_for_id(id, config))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let where_clause = ids
        .iter()
        .flat_map(|id| {
            surreal_node_tables_for_id(id, config)
                .into_iter()
                .map(move |table| {
                    format!(
                        "id = type::record({}, {})",
                        surreal_string(&table),
                        surreal_string(id.as_str())
                    )
                })
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    format!(
        "SELECT *, meta::tb(id) AS __grust_label FROM {} WHERE {where_clause};",
        tables.join(", ")
    )
}

fn surreal_get_edges_query(query: &EdgeQuery, config: &SurrealConfig) -> Result<String> {
    let tables = surreal_edge_tables(query.label.as_ref(), config);
    if tables.is_empty() {
        return Err(GrustError::Backend(
            "SurrealConfig.relationships is empty; generic edge reads need configured relationship labels or an EdgeQuery label".to_string(),
        ));
    }
    Ok(format!(
        "SELECT *, meta::tb(id) AS __grust_label FROM {};",
        tables.join(", ")
    ))
}

fn surreal_start_nodes_query(start: &Start, config: &SurrealConfig) -> String {
    match start {
        Start::Node(id) => surreal_get_node_query(id, config),
        Start::NodesByLabel(label) => {
            let table = surreal_table_name(label.as_str());
            format!("SELECT *, meta::tb(id) AS __grust_label FROM {table};")
        }
        Start::NodesByProperty { label, key, value } => {
            let table = surreal_table_name(label.as_str());
            format!(
                "SELECT *, meta::tb(id) AS __grust_label FROM {table} WHERE {} = {};",
                surreal_identifier(key),
                surreal_value(value).unwrap_or_else(|_| "NONE".to_string())
            )
        }
    }
}

fn surreal_node_tables_for_id(id: &NodeId, config: &SurrealConfig) -> Vec<String> {
    let mut tables = config
        .labels
        .iter()
        .map(|label| surreal_table_name(label))
        .collect::<BTreeSet<_>>();
    tables.insert(node_id_table(id.as_str()));
    tables.insert("record".to_string());
    tables.into_iter().collect()
}

fn surreal_edge_tables(label: Option<&Label>, config: &SurrealConfig) -> Vec<String> {
    let mut tables = BTreeSet::new();
    if let Some(label) = label {
        tables.insert(surreal_table_name(&relationship_type(label.as_str())));
    } else {
        tables.extend(
            config
                .relationships
                .iter()
                .map(|relationship| surreal_table_name(&relationship_type(relationship))),
        );
    }
    tables.into_iter().collect()
}

fn surreal_schema_query(schema: &GraphSchema) -> Result<String> {
    let mut statements = Vec::new();
    for node_type in &schema.nodes {
        let table = surreal_table_name(node_type.label.as_str());
        statements.push(format!("DEFINE TABLE {table} SCHEMAFULL;"));
        for field in &node_type.fields {
            statements.push(format!(
                "DEFINE FIELD {} ON TABLE {table} TYPE {};",
                surreal_identifier(&field.name),
                surreal_field_type(&field.ty)
            ));
        }
    }
    for edge_type in &schema.edges {
        let table = surreal_table_name(&relationship_type(edge_type.label.as_str()));
        statements.push(format!("DEFINE TABLE {table} TYPE RELATION SCHEMAFULL;"));
        statements.push(format!(
            "DEFINE FIELD relationship ON TABLE {table} TYPE string;"
        ));
        for field in &edge_type.fields {
            statements.push(format!(
                "DEFINE FIELD {} ON TABLE {table} TYPE {};",
                surreal_identifier(&field.name),
                surreal_field_type(&field.ty)
            ));
        }
    }
    Ok(statements.join("\n"))
}

fn surreal_field_type(ty: &FieldType) -> &'static str {
    match ty {
        FieldType::String | FieldType::DateTime => "string",
        FieldType::Int => "int",
        FieldType::Float => "float",
        FieldType::Bool => "bool",
        FieldType::StringArray => "array<string>",
        FieldType::IntArray => "array<int>",
        FieldType::FloatArray => "array<float>",
        FieldType::Json => "any",
    }
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
    let mut relation_tables = BTreeSet::new();
    let mut statements = Vec::new();
    for edge in edges {
        relation_tables.insert(surreal_table_name(&relationship_type(edge.label.as_str())));
    }
    statements.extend(
        relation_tables
            .into_iter()
            .map(|table| format!("DEFINE TABLE IF NOT EXISTS {table} TYPE RELATION;")),
    );
    statements.extend(
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
        .collect::<Result<Vec<_>>>()?,
    );
    Ok(statements.join("\n"))
}

fn surreal_delete_node_query(id: &NodeId, config: &SurrealConfig) -> Result<String> {
    if config.relationships.is_empty() {
        return Err(GrustError::Backend(
            "SurrealConfig.relationships is empty; node deletes need configured relationship labels to remove incident edges".to_string(),
        ));
    }
    let records = surreal_node_tables_for_id(id, config)
        .into_iter()
        .map(|table| {
            format!(
                "type::record({}, {})",
                surreal_string(&table),
                surreal_string(id.as_str())
            )
        })
        .collect::<Vec<_>>();
    let incident_clause = records
        .iter()
        .flat_map(|record| [format!("in = {record}"), format!("out = {record}")])
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut statements = surreal_edge_tables(None, config)
        .into_iter()
        .map(|table| format!("DELETE {table} WHERE {incident_clause};"))
        .collect::<Vec<_>>();
    statements.extend(
        records
            .into_iter()
            .map(|record| format!("DELETE {record};")),
    );
    Ok(statements.join("\n"))
}

fn surreal_patch_node_query(id: &NodeId, props: &Props, config: &SurrealConfig) -> Result<String> {
    let assignments = props
        .iter()
        .filter(|(key, _)| key.as_str() != "labels")
        .map(|(key, value)| Ok(format!("{key} = {}", surreal_value(value)?)))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    if assignments.is_empty() {
        return Ok(String::new());
    }
    Ok(surreal_node_tables_for_id(id, config)
        .into_iter()
        .map(|table| {
            format!(
                "UPDATE type::record({}, {}) SET {};",
                surreal_string(&table),
                surreal_string(id.as_str()),
                assignments
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

fn surreal_delete_edge_query(
    from: &NodeId,
    label: &Label,
    to: &NodeId,
    config: &SurrealConfig,
) -> String {
    let table = surreal_table_name(&relationship_type(label.as_str()));
    let from_records = surreal_node_tables_for_id(from, config);
    let to_records = surreal_node_tables_for_id(to, config);
    let where_clause = from_records
        .iter()
        .flat_map(|from_table| {
            to_records.iter().map(move |to_table| {
                format!(
                    "(in = type::record({}, {}) AND out = type::record({}, {}))",
                    surreal_string(from_table),
                    surreal_string(from.as_str()),
                    surreal_string(to_table),
                    surreal_string(to.as_str())
                )
            })
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    format!("DELETE {table} WHERE {where_clause};")
}

fn surreal_mutation_query(mutation: &GraphMutation, config: &SurrealConfig) -> Result<String> {
    match mutation {
        GraphMutation::UpsertNode(node) => surreal_upsert_nodes_query(std::slice::from_ref(node)),
        GraphMutation::PatchNode { id, props } => surreal_patch_node_query(id, props, config),
        GraphMutation::PatchMatchingNodes { .. } => Err(GrustError::Unsupported(
            "SurrealDB matched node patches are not implemented yet".to_string(),
        )),
        GraphMutation::UpdateMatchingNodeProperty { .. } => Err(GrustError::Unsupported(
            "SurrealDB matched node expression updates are not implemented yet".to_string(),
        )),
        GraphMutation::PatchEdge { .. } => Err(GrustError::Unsupported(
            "SurrealDB edge patches are not implemented yet".to_string(),
        )),
        GraphMutation::PatchMatchingEdges { .. } => Err(GrustError::Unsupported(
            "SurrealDB matched edge patches are not implemented yet".to_string(),
        )),
        GraphMutation::RemoveNodeProps { .. } => Err(GrustError::Unsupported(
            "SurrealDB node property removals are not implemented yet".to_string(),
        )),
        GraphMutation::RemoveMatchingNodeProps { .. } => Err(GrustError::Unsupported(
            "SurrealDB matched node property removals are not implemented yet".to_string(),
        )),
        GraphMutation::RemoveEdgeProps { .. } => Err(GrustError::Unsupported(
            "SurrealDB edge property removals are not implemented yet".to_string(),
        )),
        GraphMutation::UpdateMatchingEdgeProperty { .. } => Err(GrustError::Unsupported(
            "SurrealDB matched edge property updates are not implemented yet".to_string(),
        )),
        GraphMutation::RemoveMatchingEdgeProps { .. } => Err(GrustError::Unsupported(
            "SurrealDB matched edge property removals are not implemented yet".to_string(),
        )),
        GraphMutation::DeleteMatchingNodes { .. } => Err(GrustError::Unsupported(
            "SurrealDB matched node deletes are not implemented yet".to_string(),
        )),
        GraphMutation::DeleteNode(id) => surreal_delete_node_query(id, config),
        GraphMutation::UpsertEdge(edge) => {
            surreal_relate_edges_query(std::slice::from_ref(edge), &edge_id_tables(edge))
        }
        GraphMutation::UpsertEdgesFromNodeMatches { .. } => Err(GrustError::Unsupported(
            "SurrealDB row-producing edge upserts are not implemented yet".to_string(),
        )),
        GraphMutation::DeleteEdge { from, label, to } => {
            Ok(surreal_delete_edge_query(from, label, to, config))
        }
        GraphMutation::DeleteMatchingEdges { .. } => Err(GrustError::Unsupported(
            "SurrealDB matched edge deletes are not implemented yet".to_string(),
        )),
    }
}

fn surreal_apply_mutations_query(
    mutations: &[GraphMutation],
    config: &SurrealConfig,
) -> Result<String> {
    let mut statements = vec!["BEGIN TRANSACTION;".to_string()];
    for mutation in mutations {
        statements.push(surreal_mutation_query(mutation, config)?);
    }
    statements.push("COMMIT TRANSACTION;".to_string());
    Ok(statements.join("\n"))
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

fn surreal_node_from_value(mut value: serde_json::Value) -> Result<Node> {
    let object = value.as_object_mut().ok_or_else(|| {
        GrustError::Serialization("SurrealDB node row is not an object".to_string())
    })?;
    let label = object
        .remove("__grust_label")
        .and_then(|value| value.as_str().map(Label::new))
        .ok_or_else(|| GrustError::Serialization("SurrealDB node row has no label".to_string()))?;
    let id =
        surreal_record_id(object.get("id").ok_or_else(|| {
            GrustError::Serialization("SurrealDB node row has no id".to_string())
        })?)?;
    object.remove("__grust_label");
    object.remove("id");
    let props = object
        .iter()
        .map(|(key, value)| Ok((key.clone(), value_from_json(value.clone()))))
        .collect::<Result<Props>>()?;
    Ok(Node::new(label, id, props))
}

fn surreal_edge_from_value(mut value: serde_json::Value) -> Result<Edge> {
    let object = value.as_object_mut().ok_or_else(|| {
        GrustError::Serialization("SurrealDB edge row is not an object".to_string())
    })?;
    let label = object
        .remove("relationship")
        .and_then(|value| value.as_str().map(Label::new))
        .or_else(|| {
            object
                .get("__grust_label")
                .and_then(|value| value.as_str())
                .map(Label::new)
        })
        .ok_or_else(|| GrustError::Serialization("SurrealDB edge row has no label".to_string()))?;
    let from =
        surreal_record_id(object.get("in").ok_or_else(|| {
            GrustError::Serialization("SurrealDB edge row has no in".to_string())
        })?)?;
    let to =
        surreal_record_id(object.get("out").ok_or_else(|| {
            GrustError::Serialization("SurrealDB edge row has no out".to_string())
        })?)?;
    let id = object
        .get("edge_id")
        .and_then(|value| value.as_str())
        .map(EdgeId::new);
    object.remove("__grust_label");
    object.remove("id");
    object.remove("in");
    object.remove("out");
    object.remove("edge_id");
    let props = object
        .iter()
        .map(|(key, value)| Ok((key.clone(), value_from_json(value.clone()))))
        .collect::<Result<Props>>()?;
    let mut edge = Edge::new(label, from, to, props);
    edge.id = id;
    Ok(edge)
}

fn surreal_record_id(value: &serde_json::Value) -> Result<NodeId> {
    if let Some(id) = value.as_str() {
        return Ok(NodeId::new(surreal_record_key(
            id.rsplit_once(':').map(|(_, id)| id).unwrap_or(id),
        )));
    }
    if let Some(object) = value.as_object()
        && let Some(id) = object.get("id").and_then(surreal_record_id_value)
    {
        return Ok(NodeId::new(surreal_record_key(&id)));
    }
    Err(GrustError::Serialization(format!(
        "could not read SurrealDB record id from {value}"
    )))
}

fn surreal_record_key(value: &str) -> &str {
    value
        .strip_prefix('`')
        .and_then(|value| value.strip_suffix('`'))
        .unwrap_or(value)
}

fn surreal_record_id_value(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(ToString::to_string).or_else(|| {
        value
            .as_object()
            .and_then(|object| object.values().find_map(|value| value.as_str()))
            .map(ToString::to_string)
    })
}

fn value_from_json(value: serde_json::Value) -> Value {
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
        serde_json::Value::Array(values) if values.iter().all(|value| value.as_str().is_some()) => {
            Value::StringArray(
                values
                    .into_iter()
                    .filter_map(|value| match value {
                        serde_json::Value::String(value) => Some(value),
                        _ => None,
                    })
                    .collect(),
            )
        }
        value => Value::Json(value),
    }
}

fn filter_edges(edges: &mut Vec<Edge>, query: &EdgeQuery) {
    edges.retain(|edge| {
        query.from.as_ref().is_none_or(|from| from == &edge.from)
            && query.to.as_ref().is_none_or(|to| to == &edge.to)
            && query
                .label
                .as_ref()
                .is_none_or(|label| label == &edge.label)
    });
}

async fn traverse_steps_with_store<S>(
    store: &S,
    mut current: Vec<Node>,
    steps: Vec<Step>,
    limit: Option<u32>,
) -> Result<Vec<Node>>
where
    S: GraphStore,
{
    for step in steps {
        let mut target_ids = BTreeSet::new();
        for node in &current {
            let edge_query = EdgeQuery {
                from: match step.direction {
                    Direction::Out => Some(node.id.clone()),
                    Direction::In | Direction::Both => None,
                },
                to: match step.direction {
                    Direction::In => Some(node.id.clone()),
                    Direction::Out | Direction::Both => None,
                },
                label: step.edge.clone(),
            };
            for edge in store.get_edges(edge_query).await? {
                let out_matches = matches!(step.direction, Direction::Out | Direction::Both)
                    && edge.from == node.id;
                let in_matches =
                    matches!(step.direction, Direction::In | Direction::Both) && edge.to == node.id;
                if !out_matches && !in_matches {
                    continue;
                }
                let target_id = if out_matches { &edge.to } else { &edge.from };
                target_ids.insert(target_id.clone());
            }
        }
        let target_ids = target_ids.into_iter().collect::<Vec<_>>();
        let mut next = store.get_nodes(&target_ids).await?;
        next.retain(|node| step.node.as_ref().is_none_or(|label| label == &node.label));
        current = next;
    }

    if let Some(limit) = limit {
        current.truncate(limit as usize);
    }
    Ok(current)
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
        Value::DateTime(value) => Ok(surreal_string(value.as_str())),
        Value::IntArray(values) => {
            serde_json::to_string(values).map_err(|err| GrustError::Serialization(err.to_string()))
        }
        Value::FloatArray(values) => {
            serde_json::to_string(values).map_err(|err| GrustError::Serialization(err.to_string()))
        }
        Value::StringArray(values) => {
            serde_json::to_string(values).map_err(|err| GrustError::Serialization(err.to_string()))
        }
        Value::Json(value) => {
            serde_json::to_string(value).map_err(|err| GrustError::Serialization(err.to_string()))
        }
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
mod tests;
