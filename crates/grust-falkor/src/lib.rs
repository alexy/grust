use std::collections::BTreeMap;

use async_trait::async_trait;
use grust_core::prelude::*;
use r2d2::{ManageConnection, Pool, PooledConnection};
use redis::{Client as RedisClient, ConnectionLike, Value as RedisValue};

#[derive(Clone, Debug)]
pub struct FalkorConfig {
    pub redis_url: String,
    pub graph: String,
    pub batch_size: usize,
    pub pool_size: u32,
    pub id_property: String,
    pub labels_property: String,
}

impl Default for FalkorConfig {
    fn default() -> Self {
        Self {
            redis_url: "redis://127.0.0.1:6379".to_string(),
            graph: "grust".to_string(),
            batch_size: 100,
            pool_size: 16,
            id_property: "id".to_string(),
            labels_property: "labels".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FalkorGraphStore {
    config: FalkorConfig,
    pool: Pool<RedisConnectionManager>,
}

impl FalkorGraphStore {
    pub fn new(config: FalkorConfig) -> Self {
        let manager = RedisConnectionManager {
            redis_url: config.redis_url.clone(),
        };
        let pool = Pool::builder()
            .max_size(config.pool_size.max(1))
            .build_unchecked(manager);
        Self { config, pool }
    }

    fn connection(&self) -> Result<PooledConnection<RedisConnectionManager>> {
        self.pool.get().map_err(|_| falkor_pool_error())
    }

    fn query<C>(&self, connection: &mut C, query: &str) -> Result<RedisValue>
    where
        C: ConnectionLike,
    {
        redis::cmd("GRAPH.QUERY")
            .arg(&self.config.graph)
            .arg(query)
            .query::<RedisValue>(connection)
            .map_err(|_| falkor_query_error())
    }

    /// Backend-native Cypher escape hatch (Full39075 F11): run `query` verbatim
    /// against the configured FalkorDB graph. This is deliberately **outside**
    /// Grust's portable conformance surface — the text is FalkorDB's own
    /// openCypher dialect and no portable semantics are claimed for it.
    pub fn run_native_cypher(&self, query: &str) -> Result<()> {
        let mut connection = self.connection()?;
        self.query(&mut connection, query).map(|_| ())
    }
}

#[derive(Clone, Debug)]
struct RedisConnectionManager {
    redis_url: String,
}

impl ManageConnection for RedisConnectionManager {
    type Connection = redis::Connection;
    type Error = redis::RedisError;

    fn connect(&self) -> std::result::Result<Self::Connection, Self::Error> {
        RedisClient::open(self.redis_url.as_str())?.get_connection()
    }

    fn is_valid(&self, connection: &mut Self::Connection) -> std::result::Result<(), Self::Error> {
        redis::cmd("PING").query::<String>(connection).map(|_| ())
    }

    fn has_broken(&self, connection: &mut Self::Connection) -> bool {
        !connection.is_open()
    }
}

#[async_trait]
impl GraphStore for FalkorGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        let queries = falkor_schema_queries(schema, &self.config)?;
        let mut connection = self.connection()?;
        for query in queries {
            self.query(&mut connection, &query)?;
        }
        Ok(())
    }

    async fn put_node(&self, node: &Node) -> Result<PutOutcome> {
        let query = falkor_node_query(node, &self.config)?;
        let mut connection = self.connection()?;
        self.query(&mut connection, &query)?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        let query = falkor_edge_query(edge, &self.config)?;
        let mut connection = self.connection()?;
        self.query(&mut connection, &query)?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        // Validate the entire graph before attempting a connection so a late
        // unsafe label or property key cannot leave a partially written graph.
        validate_falkor_graph(graph, &self.config)?;
        let mut connection = self.connection()?;
        let batch_size = self.config.batch_size.max(1);
        let mut report = LoadReport::default();

        let mut nodes_by_labels: BTreeMap<String, Vec<&Node>> = BTreeMap::new();
        for node in &graph.nodes {
            nodes_by_labels
                .entry(falkor_labels(node, &self.config)?)
                .or_default()
                .push(node);
        }

        for (labels, nodes) in nodes_by_labels {
            validate_label_path(&labels)?;
            for chunk in nodes.chunks(batch_size) {
                let query = falkor_nodes_batch_query(&labels, chunk, &self.config)?;
                self.query(&mut connection, &query)?;
                report.nodes += chunk.len();
            }
        }

        let mut edges_by_label: BTreeMap<String, Vec<&Edge>> = BTreeMap::new();
        for edge in &graph.edges {
            let relationship = relationship_type(edge.label.as_str());
            validate_label(&relationship)?;
            edges_by_label.entry(relationship).or_default().push(edge);
        }

        for (relationship, edges) in edges_by_label {
            for chunk in edges.chunks(batch_size) {
                let query = falkor_edges_batch_query(&relationship, chunk, &self.config)?;
                self.query(&mut connection, &query)?;
                report.edges += chunk.len();
            }
        }

        Ok(report)
    }

    async fn get_node(&self, _id: &NodeId) -> Result<Option<Node>> {
        Err(GrustError::Unsupported(
            "FalkorGraphStore does not implement reads yet".to_string(),
        ))
    }

    async fn get_edges(&self, _query: EdgeQuery) -> Result<Vec<Edge>> {
        Err(GrustError::Unsupported(
            "FalkorGraphStore does not implement reads yet".to_string(),
        ))
    }

    async fn traverse(&self, _traversal: Traversal) -> Result<Vec<Node>> {
        Err(GrustError::Unsupported(
            "FalkorGraphStore does not implement traversal yet".to_string(),
        ))
    }
}

#[async_trait]
impl GraphAdminStore for FalkorGraphStore {
    async fn clear(&self) -> Result<()> {
        let mut connection = self.connection()?;
        match redis::cmd("GRAPH.DELETE")
            .arg(&self.config.graph)
            .query::<RedisValue>(&mut connection)
        {
            Ok(_) => Ok(()),
            Err(err) if err.to_string().contains("Invalid graph operation") => Ok(()),
            Err(err) if err.to_string().contains("graph not found") => Ok(()),
            Err(err) if err.to_string().contains("does not exist") => Ok(()),
            Err(err) => Err(GrustError::Backend(format!(
                "failed to delete existing FalkorDB graph {}: {err}",
                self.config.graph
            ))),
        }
    }
}

fn falkor_node_query(node: &Node, config: &FalkorConfig) -> Result<String> {
    let labels = falkor_labels(node, config)?;
    validate_label_path(&labels)?;
    let id_property = falkor_property_identifier(&config.id_property)?;
    Ok(format!(
        "MERGE (n:{} {{{}:{}}}) SET n += {}",
        labels,
        id_property,
        cypher_string(node.id.as_str()),
        cypher_map(&node.props, config, true)?
    ))
}

fn falkor_edge_query(edge: &Edge, config: &FalkorConfig) -> Result<String> {
    let relationship = relationship_type(edge.label.as_str());
    validate_label(&relationship)?;
    let id_property = falkor_property_identifier(&config.id_property)?;
    if edge.props.is_empty() {
        Ok(format!(
            "MATCH (a {{{}:{}}}), (b {{{}:{}}}) MERGE (a)-[:{}]->(b)",
            id_property,
            cypher_string(edge.from.as_str()),
            id_property,
            cypher_string(edge.to.as_str()),
            relationship
        ))
    } else {
        Ok(format!(
            "MATCH (a {{{}:{}}}), (b {{{}:{}}}) MERGE (a)-[r:{}]->(b) SET r += {}",
            id_property,
            cypher_string(edge.from.as_str()),
            id_property,
            cypher_string(edge.to.as_str()),
            relationship,
            cypher_map(&edge.props, config, false)?
        ))
    }
}

fn falkor_nodes_batch_query(
    labels: &str,
    nodes: &[&Node],
    config: &FalkorConfig,
) -> Result<String> {
    let id_property = falkor_property_identifier(&config.id_property)?;
    let rows = nodes
        .iter()
        .map(|node| {
            Ok(format!(
                "{{id:{},props:{}}}",
                cypher_string(node.id.as_str()),
                cypher_map(&node.props, config, true)?
            ))
        })
        .collect::<Result<Vec<_>>>()?
        .join(",");
    Ok(format!(
        "UNWIND [{}] AS row MERGE (n:{} {{{}: row.id}}) SET n += row.props",
        rows, labels, id_property
    ))
}

fn falkor_edges_batch_query(
    relationship: &str,
    edges: &[&Edge],
    config: &FalkorConfig,
) -> Result<String> {
    let id_property = falkor_property_identifier(&config.id_property)?;
    let rows = edges
        .iter()
        .map(|edge| {
            Ok(format!(
                "{{from:{},to:{},props:{}}}",
                cypher_string(edge.from.as_str()),
                cypher_string(edge.to.as_str()),
                cypher_map(&edge.props, config, false)?
            ))
        })
        .collect::<Result<Vec<_>>>()?
        .join(",");
    Ok(format!(
        "UNWIND [{}] AS row MATCH (a {{{}: row.from}}), (b {{{}: row.to}}) MERGE (a)-[r:{}]->(b) SET r += row.props",
        rows, id_property, id_property, relationship
    ))
}

fn falkor_schema_queries(schema: &GraphSchema, config: &FalkorConfig) -> Result<Vec<String>> {
    let id_property = falkor_property_identifier(&config.id_property)?;
    let mut claims = Vec::new();
    for node_type in &schema.nodes {
        let label = schema_identifier(node_type.label.as_str())?;
        claims.push((
            "node label".to_string(),
            label.clone(),
            format!("node type '{}'", node_type.label.as_str()),
        ));
        let namespace = format!("property index on node label '{label}'");
        claims.push((
            namespace.clone(),
            id_property.clone(),
            format!("structural id property '{}'", config.id_property),
        ));
        for field in &node_type.fields {
            let property = falkor_property_identifier(&field.name)?;
            claims.push((
                namespace.clone(),
                property,
                format!("node field '{}.{}'", node_type.label.as_str(), field.name),
            ));
        }
    }
    for edge_type in &schema.edges {
        let relationship = relationship_type(edge_type.label.as_str());
        validate_label(&relationship)?;
        claims.push((
            "relationship type".to_string(),
            relationship,
            format!("edge type '{}'", edge_type.label.as_str()),
        ));
    }
    validate_physical_identifier_claims("FalkorDB", claims)?;

    let mut queries = Vec::new();
    for node_type in &schema.nodes {
        let label = schema_identifier(node_type.label.as_str())?;
        queries.push(format!("CREATE INDEX ON :{}({})", label, id_property));
        for field in &node_type.fields {
            queries.push(format!(
                "CREATE INDEX ON :{}({})",
                label,
                falkor_property_identifier(&field.name)?
            ));
        }
    }
    Ok(queries)
}

fn falkor_labels(node: &Node, config: &FalkorConfig) -> Result<String> {
    node.props
        .get(&config.labels_property)
        .and_then(Value::as_string_array)
        .map(|labels| labels.iter().map(String::as_str).collect::<Vec<_>>())
        .unwrap_or_else(|| vec![node.label.as_str()])
        .into_iter()
        .map(schema_identifier)
        .collect::<Result<Vec<_>>>()
        .map(|labels| labels.join(":"))
}

fn validate_falkor_graph(graph: &Graph, config: &FalkorConfig) -> Result<()> {
    falkor_property_identifier(&config.id_property)?;
    let mut claims = Vec::new();
    for node in &graph.nodes {
        let logical_labels = node
            .props
            .get(&config.labels_property)
            .and_then(Value::as_string_array)
            .map(|labels| labels.iter().map(String::as_str).collect::<Vec<_>>())
            .unwrap_or_else(|| vec![node.label.as_str()]);
        for logical in logical_labels {
            claims.push((
                "node label".to_string(),
                schema_identifier(logical)?,
                format!("node label '{logical}'"),
            ));
        }
        let labels = falkor_labels(node, config)?;
        validate_label_path(&labels)?;
        cypher_map(&node.props, config, true)?;
    }
    for edge in &graph.edges {
        let relationship = relationship_type(edge.label.as_str());
        validate_label(&relationship)?;
        claims.push((
            "relationship type".to_string(),
            relationship,
            format!("edge label '{}'", edge.label.as_str()),
        ));
        cypher_map(&edge.props, config, false)?;
    }
    // Repeated records may intentionally share one logical label; collapse
    // those identical claims while retaining lossy-normalization collisions.
    claims.sort();
    claims.dedup();
    validate_physical_identifier_claims("FalkorDB graph", claims)?;
    Ok(())
}

fn cypher_map(props: &Props, config: &FalkorConfig, reserve_id: bool) -> Result<String> {
    let body = props
        .iter()
        .filter(|(key, _)| {
            key.as_str() != config.labels_property
                && (!reserve_id || key.as_str() != config.id_property)
        })
        .map(|(key, value)| {
            let key = falkor_property_identifier(key)?;
            Ok(match value {
                Value::Null => format!("{key}:null"),
                Value::Bool(value) => format!("{key}:{value}"),
                Value::Int(value) => format!("{key}:{value}"),
                Value::Float(value) => format!("{key}:{value}"),
                Value::String(value) => format!("{key}:{}", cypher_string(value)),
                Value::DateTime(value) => format!("{key}:{}", cypher_string(value.as_str())),
                Value::Decimal(value) => {
                    format!("{key}:{}", cypher_string(&value.to_canonical_string()))
                }
                Value::Duration(value) => {
                    format!("{key}:{}", cypher_string(&value.to_iso_string()))
                }
                Value::IntArray(values) => format!(
                    "{key}:[{}]",
                    values
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                Value::FloatArray(values) => format!(
                    "{key}:[{}]",
                    values
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                Value::StringArray(values) => format!(
                    "{key}:[{}]",
                    values
                        .iter()
                        .map(|value| cypher_string(value))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                Value::Path(_) | Value::Graph(_) => {
                    format!("{key}:{}", cypher_string(&value.to_json().to_string()))
                }
                Value::Json(value) => format!("{key}:{}", cypher_string(&value.to_string())),
            })
        })
        .collect::<Result<Vec<_>>>()?
        .join(",");
    Ok(format!("{{{body}}}"))
}

/// Quotes a FalkorDB property identifier without changing its physical name.
///
/// FalkorDB 4.20 accepts backtick-delimited spaces and punctuation, but does
/// not accept openCypher's doubled-backtick escape. Rejecting backticks and
/// control characters is therefore the conservative, injection-safe contract.
fn falkor_property_identifier(value: &str) -> Result<String> {
    if value.is_empty() || value.chars().any(|ch| ch == '`' || ch.is_control()) {
        return Err(GrustError::Backend(format!(
            "unsafe FalkorDB property identifier: {value:?}"
        )));
    }
    Ok(format!("`{value}`"))
}

fn falkor_pool_error() -> GrustError {
    GrustError::Backend("failed to acquire FalkorDB Redis connection from pool".to_string())
}

fn falkor_query_error() -> GrustError {
    GrustError::Backend("FalkorDB query failed".to_string())
}

fn cypher_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('\'');
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '\'' => escaped.push_str("\\'"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(ch),
        }
    }
    escaped.push('\'');
    escaped
}

fn validate_label_path(labels: &str) -> Result<()> {
    for label in labels.split(':') {
        validate_label(label)?;
    }
    Ok(())
}

fn validate_label(label: &str) -> Result<()> {
    if !label.is_empty()
        && label
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        Ok(())
    } else {
        Err(GrustError::Backend(format!(
            "unsafe FalkorDB label or relationship: {label}"
        )))
    }
}

#[cfg(test)]
mod tests;
