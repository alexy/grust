use std::collections::BTreeMap;

use async_trait::async_trait;
use grust_core::prelude::*;
use redis::{Client as RedisClient, Value as RedisValue};

#[derive(Clone, Debug)]
pub struct FalkorConfig {
    pub redis_url: String,
    pub graph: String,
    pub batch_size: usize,
    pub id_property: String,
    pub labels_property: String,
}

impl Default for FalkorConfig {
    fn default() -> Self {
        Self {
            redis_url: "redis://127.0.0.1:6379".to_string(),
            graph: "grust".to_string(),
            batch_size: 100,
            id_property: "id".to_string(),
            labels_property: "labels".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FalkorGraphStore {
    config: FalkorConfig,
}

impl FalkorGraphStore {
    pub fn new(config: FalkorConfig) -> Self {
        Self { config }
    }

    fn connection(&self) -> Result<redis::Connection> {
        let client = RedisClient::open(self.config.redis_url.as_str()).map_err(|err| {
            GrustError::Backend(format!(
                "invalid Redis/FalkorDB URL {}: {err}",
                self.config.redis_url
            ))
        })?;
        client.get_connection().map_err(|err| {
            GrustError::Backend(format!(
                "failed to connect to {}: {err}",
                self.config.redis_url
            ))
        })
    }

    fn query(&self, connection: &mut redis::Connection, query: &str) -> Result<RedisValue> {
        redis::cmd("GRAPH.QUERY")
            .arg(&self.config.graph)
            .arg(query)
            .query::<RedisValue>(connection)
            .map_err(|err| GrustError::Backend(format!("FalkorDB query failed: {query}: {err}")))
    }
}

#[async_trait]
impl GraphStore for FalkorGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        let mut connection = self.connection()?;
        for query in falkor_schema_queries(schema, &self.config)? {
            self.query(&mut connection, &query)?;
        }
        Ok(())
    }

    async fn put_node(&self, node: &Node) -> Result<PutOutcome> {
        let mut connection = self.connection()?;
        let query = falkor_node_query(node, &self.config)?;
        self.query(&mut connection, &query)?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        let mut connection = self.connection()?;
        let query = falkor_edge_query(edge, &self.config)?;
        self.query(&mut connection, &query)?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
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
    Ok(format!(
        "MERGE (n:{} {{{}:{}}}) SET n += {}",
        labels,
        config.id_property,
        cypher_string(node.id.as_str()),
        cypher_map(&node.props, config)
    ))
}

fn falkor_edge_query(edge: &Edge, config: &FalkorConfig) -> Result<String> {
    let relationship = relationship_type(edge.label.as_str());
    validate_label(&relationship)?;
    if edge.props.is_empty() {
        Ok(format!(
            "MATCH (a {{{}:{}}}), (b {{{}:{}}}) MERGE (a)-[:{}]->(b)",
            config.id_property,
            cypher_string(edge.from.as_str()),
            config.id_property,
            cypher_string(edge.to.as_str()),
            relationship
        ))
    } else {
        Ok(format!(
            "MATCH (a {{{}:{}}}), (b {{{}:{}}}) MERGE (a)-[r:{}]->(b) SET r += {}",
            config.id_property,
            cypher_string(edge.from.as_str()),
            config.id_property,
            cypher_string(edge.to.as_str()),
            relationship,
            cypher_map(&edge.props, config)
        ))
    }
}

fn falkor_nodes_batch_query(
    labels: &str,
    nodes: &[&Node],
    config: &FalkorConfig,
) -> Result<String> {
    let rows = nodes
        .iter()
        .map(|node| {
            Ok(format!(
                "{{{}:{},props:{}}}",
                config.id_property,
                cypher_string(node.id.as_str()),
                cypher_map(&node.props, config)
            ))
        })
        .collect::<Result<Vec<_>>>()?
        .join(",");
    Ok(format!(
        "UNWIND [{}] AS row MERGE (n:{} {{{}: row.{}}}) SET n += row.props",
        rows, labels, config.id_property, config.id_property
    ))
}

fn falkor_edges_batch_query(
    relationship: &str,
    edges: &[&Edge],
    config: &FalkorConfig,
) -> Result<String> {
    let rows = edges
        .iter()
        .map(|edge| {
            Ok(format!(
                "{{from:{},to:{},props:{}}}",
                cypher_string(edge.from.as_str()),
                cypher_string(edge.to.as_str()),
                cypher_map(&edge.props, config)
            ))
        })
        .collect::<Result<Vec<_>>>()?
        .join(",");
    Ok(format!(
        "UNWIND [{}] AS row MATCH (a {{{}: row.from}}), (b {{{}: row.to}}) MERGE (a)-[r:{}]->(b) SET r += row.props",
        rows, config.id_property, config.id_property, relationship
    ))
}

fn falkor_schema_queries(schema: &GraphSchema, config: &FalkorConfig) -> Result<Vec<String>> {
    let mut queries = Vec::new();
    for node_type in &schema.nodes {
        let label = schema_identifier(node_type.label.as_str())?;
        queries.push(format!(
            "CREATE INDEX ON :{}({})",
            label, config.id_property
        ));
        for field in &node_type.fields {
            queries.push(format!(
                "CREATE INDEX ON :{}({})",
                label,
                schema_identifier(&field.name)?
            ));
        }
    }
    for edge_type in &schema.edges {
        let relationship = relationship_type(edge_type.label.as_str());
        validate_label(&relationship)?;
    }
    Ok(queries)
}

fn falkor_labels(node: &Node, config: &FalkorConfig) -> Result<String> {
    let labels = node
        .props
        .get(&config.labels_property)
        .and_then(Value::as_string_array)
        .map(|labels| labels.join(":"))
        .unwrap_or_else(|| node.label.as_str().to_string());
    validate_label_path(&labels)?;
    Ok(labels)
}

fn cypher_map(props: &Props, config: &FalkorConfig) -> String {
    let body = props
        .iter()
        .filter(|(key, _)| key.as_str() != config.labels_property)
        .map(|(key, value)| match value {
            Value::Null => format!("{key}:null"),
            Value::Bool(value) => format!("{key}:{value}"),
            Value::Int(value) => format!("{key}:{value}"),
            Value::Float(value) => format!("{key}:{value}"),
            Value::String(value) | Value::DateTime(value) => {
                format!("{key}:{}", cypher_string(value))
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
            Value::Json(value) => format!("{key}:{}", cypher_string(&value.to_string())),
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{body}}}")
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

fn schema_identifier(value: &str) -> Result<String> {
    let identifier = value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();
    validate_label(&identifier)?;
    Ok(identifier)
}

#[cfg(test)]
mod tests;
