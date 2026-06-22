use async_trait::async_trait;
use grust_core::prelude::*;
use grust_postgres_core::{
    PostgresGraphConfig, PostgresGraphStore, bootstrap_sql, postgres_schema_sql, sql_str,
    unquoted_qualified_table, validate_identifier,
};

#[cfg(test)]
use grust_postgres_core::quote_ident;

#[derive(Clone, Debug)]
pub struct PgGraphConfig {
    pub connection_string: String,
    pub schema: String,
    pub table_prefix: String,
    pub batch_size: usize,
    pub auto_build: bool,
    pub build_mode: PgGraphBuildMode,
}

impl Default for PgGraphConfig {
    fn default() -> Self {
        Self {
            connection_string: "host=127.0.0.1 user=postgres dbname=graph".to_string(),
            schema: "public".to_string(),
            table_prefix: "grust".to_string(),
            batch_size: 500,
            auto_build: false,
            build_mode: PgGraphBuildMode::CsrReadonly,
        }
    }
}

impl From<&PgGraphConfig> for PostgresGraphConfig {
    fn from(config: &PgGraphConfig) -> Self {
        Self {
            connection_string: config.connection_string.clone(),
            schema: config.schema.clone(),
            table_prefix: config.table_prefix.clone(),
            batch_size: config.batch_size,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PgGraphBuildMode {
    CsrReadonly,
    MutableOverlay,
}

impl PgGraphBuildMode {
    fn as_pggraph_mode(self) -> &'static str {
        match self {
            Self::CsrReadonly => "csr_readonly",
            Self::MutableOverlay => "mutable_overlay",
        }
    }
}

#[derive(Debug)]
pub struct PgGraphStore {
    config: PgGraphConfig,
    postgres: PostgresGraphStore,
}

impl PgGraphStore {
    pub async fn connect(config: PgGraphConfig) -> Result<Self> {
        validate_identifier(&config.schema)?;
        validate_identifier(&config.table_prefix)?;
        let postgres = PostgresGraphStore::connect(PostgresGraphConfig::from(&config)).await?;
        Ok(Self { config, postgres })
    }

    pub fn config(&self) -> &PgGraphConfig {
        &self.config
    }

    pub async fn build_projection(&self) -> Result<()> {
        self.postgres
            .execute(&format!(
                "SELECT * FROM graph.build({})",
                sql_str(self.config.build_mode.as_pggraph_mode())
            ))
            .await
    }

    async fn maybe_build_projection(&self) -> Result<()> {
        if self.config.auto_build {
            self.build_projection().await?;
        }
        Ok(())
    }

    async fn execute(&self, sql: &str) -> Result<()> {
        self.postgres.execute(sql).await
    }

    fn nodes_table(&self) -> String {
        self.postgres.nodes_table()
    }

    fn edges_table(&self) -> String {
        self.postgres.edges_table()
    }
}

#[async_trait]
impl GraphStore for PgGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        self.bootstrap().await?;
        self.execute(&postgres_schema_sql(
            &PostgresGraphConfig::from(&self.config),
            &self.nodes_table(),
            &self.edges_table(),
            schema,
        )?)
        .await
    }

    async fn put_node(&self, node: &Node) -> Result<PutOutcome> {
        let outcome = self.postgres.put_node(node).await?;
        self.maybe_build_projection().await?;
        Ok(outcome)
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        let outcome = self.postgres.put_edge(edge).await?;
        self.maybe_build_projection().await?;
        Ok(outcome)
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        let report = self.postgres.put_graph(graph).await?;
        self.maybe_build_projection().await?;
        Ok(report)
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
        self.postgres.traverse(traversal).await
    }
}

#[async_trait]
impl GraphAdminStore for PgGraphStore {
    async fn bootstrap(&self) -> Result<()> {
        self.execute(&pggraph_bootstrap_sql(
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
impl GraphMutationStore for PgGraphStore {
    fn mutation_atomicity(&self) -> GraphMutationAtomicity {
        GraphMutationAtomicity::Transactional
    }

    async fn delete_node(&self, id: &NodeId) -> Result<()> {
        self.postgres.delete_node(id).await?;
        self.maybe_build_projection().await
    }

    async fn delete_edge(&self, from: &NodeId, label: &Label, to: &NodeId) -> Result<()> {
        self.postgres.delete_edge(from, label, to).await?;
        self.maybe_build_projection().await
    }

    async fn apply_mutations(&self, mutations: &[GraphMutation]) -> Result<()> {
        if mutations.is_empty() {
            return Ok(());
        }
        self.postgres.apply_mutations(mutations).await?;
        self.maybe_build_projection().await
    }
}

pub fn pggraph_bootstrap_sql(
    config: &PgGraphConfig,
    nodes_table: &str,
    edges_table: &str,
) -> Result<String> {
    let mut sql = bootstrap_sql(&PostgresGraphConfig::from(config), nodes_table, edges_table)?;
    sql.push_str(&format!(
        "
         CREATE EXTENSION IF NOT EXISTS graph;
         SELECT graph.add_table({nodes_regclass}::regclass, 'id', ARRAY['label', 'props']);
         SELECT graph.add_edge(
            {edges_regclass}::regclass,
            'from_id',
            {nodes_regclass}::regclass,
            'to_id',
            'grust_edge',
            false,
            NULL,
            'label'
         );",
        nodes_regclass = sql_str(&unquoted_qualified_table(
            &config.schema,
            &format!("{}_nodes", config.table_prefix)
        )),
        edges_regclass = sql_str(&unquoted_qualified_table(
            &config.schema,
            &format!("{}_edges", config.table_prefix)
        )),
    ));
    Ok(sql)
}

#[cfg(test)]
mod tests;
