use async_trait::async_trait;
use grust_core::prelude::*;
use grust_postgres_core::{
    PostgresGraphConfig, PostgresGraphStore, postgres_schema_sql, quote_ident, sql_str,
    validate_identifier, validate_postgres_config,
};
use tokio::task::JoinHandle;
use tokio_postgres::{Client, NoTls};

#[derive(Clone, Debug)]
pub struct PostgresPgqConfig {
    pub connection_string: String,
    pub schema: String,
    pub table_prefix: String,
    pub graph_name: String,
    pub batch_size: usize,
}

impl Default for PostgresPgqConfig {
    fn default() -> Self {
        Self {
            connection_string: "host=127.0.0.1 port=5419 user=postgres dbname=graph".to_string(),
            schema: "public".to_string(),
            table_prefix: "grust".to_string(),
            graph_name: "grust_graph".to_string(),
            batch_size: 500,
        }
    }
}

impl From<&PostgresPgqConfig> for PostgresGraphConfig {
    fn from(config: &PostgresPgqConfig) -> Self {
        Self {
            connection_string: config.connection_string.clone(),
            schema: config.schema.clone(),
            table_prefix: config.table_prefix.clone(),
            batch_size: config.batch_size,
        }
    }
}

#[derive(Debug)]
pub struct PostgresPgqStore {
    config: PostgresPgqConfig,
    postgres: PostgresGraphStore,
    client: Client,
    connection_task: JoinHandle<()>,
}

impl PostgresPgqStore {
    pub async fn connect(config: PostgresPgqConfig) -> Result<Self> {
        validate_pgq_config(&config)?;
        let postgres = PostgresGraphStore::connect(PostgresGraphConfig::from(&config)).await?;
        let (client, connection) = tokio_postgres::connect(&config.connection_string, NoTls)
            .await
            .map_err(|err| {
                GrustError::Backend(format!(
                    "failed to connect to PostgreSQL for PostgreSQL PGQ backend: {err}"
                ))
            })?;
        let connection_task = tokio::spawn(async move {
            if let Err(err) = connection.await {
                eprintln!("grust-postgres-pgq PostgreSQL connection task ended: {err}");
            }
        });
        Ok(Self {
            config,
            postgres,
            client,
            connection_task,
        })
    }

    pub fn config(&self) -> &PostgresPgqConfig {
        &self.config
    }

    /// Forward SQL through the PostgreSQL core store's autocommit-only guard.
    pub async fn execute(&self, sql: &str) -> Result<()> {
        self.postgres.execute(sql).await
    }

    fn nodes_table(&self) -> String {
        self.postgres.nodes_table()
    }

    fn edges_table(&self) -> String {
        self.postgres.edges_table()
    }

    fn graph_name(&self) -> String {
        qualified_name(&self.config.schema, &self.config.graph_name)
    }
}

#[async_trait]
impl GraphStore for PostgresPgqStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        let ddl = postgres_schema_sql(
            &PostgresGraphConfig::from(&self.config),
            &self.nodes_table(),
            &self.edges_table(),
            schema,
        )?;
        self.bootstrap().await?;
        self.execute(&ddl).await
    }

    async fn put_node(&self, node: &Node) -> Result<PutOutcome> {
        self.postgres.put_node(node).await
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        self.postgres.put_edge(edge).await
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        self.postgres.put_graph(graph).await
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        self.postgres.get_node(id).await
    }

    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>> {
        self.postgres.get_nodes(ids).await
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        self.postgres.get_edges(query).await
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        let sql = pgq_traversal_sql(&self.graph_name(), &self.nodes_table(), &traversal)?;
        let rows = self.client.query(&sql, &[]).await.map_err(|err| {
            GrustError::Backend(format!(
                "PostgreSQL PGQ traversal query failed: {err}: {sql}"
            ))
        })?;
        rows.into_iter()
            .map(|row| {
                let id: String = row.get("id");
                let label: String = row.get("label");
                let props_json: String = row.get("props");
                let props: Props = serde_json::from_str(&props_json).map_err(|err| {
                    GrustError::Serialization(format!("node props JSON parse failed: {err}"))
                })?;
                Ok(Node {
                    id: NodeId::new(id),
                    label: Label::new(label),
                    props,
                })
            })
            .collect()
    }
}

impl Drop for PostgresPgqStore {
    fn drop(&mut self) {
        self.connection_task.abort();
    }
}

#[async_trait]
impl GraphAdminStore for PostgresPgqStore {
    async fn bootstrap(&self) -> Result<()> {
        self.postgres.bootstrap().await?;
        self.execute(&pgq_bootstrap_sql(
            &self.config,
            &self.nodes_table(),
            &self.edges_table(),
        )?)
        .await
    }

    async fn clear(&self) -> Result<()> {
        self.postgres.clear().await
    }
}

#[async_trait]
impl GraphMutationStore for PostgresPgqStore {
    fn mutation_atomicity(&self) -> GraphMutationAtomicity {
        GraphMutationAtomicity::Transactional
    }

    async fn delete_node(&self, id: &NodeId) -> Result<()> {
        self.postgres.delete_node(id).await
    }

    async fn delete_edge(&self, from: &NodeId, label: &Label, to: &NodeId) -> Result<()> {
        self.postgres.delete_edge(from, label, to).await
    }

    async fn apply_mutations(&self, mutations: &[GraphMutation]) -> Result<()> {
        self.postgres.apply_mutations(mutations).await
    }
}

pub fn pgq_bootstrap_sql(
    config: &PostgresPgqConfig,
    nodes_table: &str,
    edges_table: &str,
) -> Result<String> {
    validate_pgq_config(config)?;
    let graph_name = qualified_name(&config.schema, &config.graph_name);
    Ok(format!(
        "DROP PROPERTY GRAPH IF EXISTS {graph_name};
         CREATE PROPERTY GRAPH {graph_name}
            VERTEX TABLES (
                {nodes_table} AS grust_nodes
                    KEY (id)
                    LABEL grust_node
                    PROPERTIES (id, label, props::text AS props)
            )
            EDGE TABLES (
                {edges_table} AS grust_edges
                    KEY (from_id, label, to_id)
                    SOURCE KEY (from_id) REFERENCES grust_nodes (id)
                    DESTINATION KEY (to_id) REFERENCES grust_nodes (id)
                    LABEL grust_edge
                    PROPERTIES (label, props::text AS props)
            );"
    ))
}

fn validate_pgq_config(config: &PostgresPgqConfig) -> Result<()> {
    validate_postgres_config(&PostgresGraphConfig::from(config))?;
    validate_identifier(&config.graph_name)
}

pub fn pgq_traversal_sql(
    graph_name: &str,
    nodes_table: &str,
    traversal: &Traversal,
) -> Result<String> {
    let pattern = pgq_pattern(traversal)?;
    let target = format!("n{}", traversal.steps.len());
    let limit = traversal
        .limit
        .map(|limit| format!(" LIMIT {limit}"))
        .unwrap_or_default();
    Ok(format!(
        "SELECT n.id, n.label, n.props::text AS props
         FROM GRAPH_TABLE ({graph_name} MATCH {pattern} COLUMNS ({target}.id AS target_id)) gt
         JOIN {nodes_table} n ON n.id = gt.target_id{limit}"
    ))
}

fn pgq_pattern(traversal: &Traversal) -> Result<String> {
    let mut pattern = start_pattern(&traversal.start)?;
    for (index, step) in traversal.steps.iter().enumerate() {
        let edge = edge_pattern(index, step)?;
        let node = node_pattern(index + 1, step.node.as_ref())?;
        pattern.push_str(&edge);
        pattern.push_str(&node);
    }
    Ok(pattern)
}

fn start_pattern(start: &Start) -> Result<String> {
    Ok(match start {
        Start::Node(id) => format!("(n0 IS grust_node WHERE n0.id = {})", sql_str(id.as_str())),
        Start::NodesByLabel(label) => format!(
            "(n0 IS grust_node WHERE n0.label = {})",
            sql_str(label.as_str())
        ),
        Start::NodesByProperty { label, key, value } => format!(
            "(n0 IS grust_node WHERE n0.label = {} AND {})",
            sql_str(label.as_str()),
            pgq_json_property_predicate("n0", key, value)?
        ),
    })
}

fn node_pattern(index: usize, label: Option<&Label>) -> Result<String> {
    Ok(match label {
        Some(label) => format!(
            "(n{index} IS grust_node WHERE n{index}.label = {})",
            sql_str(label.as_str())
        ),
        None => format!("(n{index} IS grust_node)"),
    })
}

fn edge_pattern(index: usize, step: &Step) -> Result<String> {
    let filler = match &step.edge {
        Some(label) => format!(
            "e{index} IS grust_edge WHERE e{index}.label = {}",
            sql_str(label.as_str())
        ),
        None => format!("e{index} IS grust_edge"),
    };
    Ok(match step.direction {
        Direction::Out => format!("-[{filler}]->"),
        Direction::In => format!("<-[{filler}]-"),
        Direction::Both => format!("-[{filler}]-"),
    })
}

fn pgq_json_property_predicate(alias: &str, key: &str, value: &Value) -> Result<String> {
    grust_postgres_core::validate_json_key(key)?;
    let prop = format!("{alias}.props::jsonb #>> ARRAY[{}, 'value']", sql_str(key));
    Ok(match value {
        Value::Null => format!(
            "{alias}.props::jsonb -> {} ->> 'type' = 'null'",
            sql_str(key)
        ),
        Value::Bool(value) => format!("({prop})::boolean = {value}"),
        Value::Int(value) => format!("({prop})::bigint = {value}"),
        Value::Float(value) => format!("({prop})::double precision = {value}"),
        Value::String(value) => format!("{prop} = {}", sql_str(value)),
        other => {
            let json = serde_json::to_string(other)
                .map_err(|err| GrustError::Serialization(err.to_string()))?;
            format!(
                "{alias}.props::jsonb -> {} = {}::jsonb",
                sql_str(key),
                sql_str(&json)
            )
        }
    })
}

fn qualified_name(schema: &str, name: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(name))
}

#[cfg(test)]
mod tests;
