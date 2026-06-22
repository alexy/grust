use async_trait::async_trait;
use grust_core::prelude::*;
use grust_sql_core::{GraphSqlDialect, UniversalTableRefs};
use tokio::task::JoinHandle;
use tokio_postgres::{Client, NoTls};

#[derive(Clone, Debug)]
pub struct PostgresGraphConfig {
    pub connection_string: String,
    pub schema: String,
    pub table_prefix: String,
    pub batch_size: usize,
}

impl Default for PostgresGraphConfig {
    fn default() -> Self {
        Self {
            connection_string: "host=127.0.0.1 user=postgres dbname=graph".to_string(),
            schema: "public".to_string(),
            table_prefix: "grust".to_string(),
            batch_size: 500,
        }
    }
}

#[derive(Debug)]
pub struct PostgresGraphStore {
    config: PostgresGraphConfig,
    client: Client,
    connection_task: JoinHandle<()>,
}

impl PostgresGraphStore {
    pub async fn connect(config: PostgresGraphConfig) -> Result<Self> {
        validate_identifier(&config.schema)?;
        validate_identifier(&config.table_prefix)?;
        let (client, connection) = tokio_postgres::connect(&config.connection_string, NoTls)
            .await
            .map_err(|err| {
                GrustError::Backend(format!(
                    "failed to connect to PostgreSQL for PostgreSQL backend: {err}"
                ))
            })?;
        let connection_task = tokio::spawn(async move {
            if let Err(err) = connection.await {
                eprintln!("grust-postgres PostgreSQL connection task ended: {err}");
            }
        });
        Ok(Self {
            config,
            client,
            connection_task,
        })
    }

    pub fn config(&self) -> &PostgresGraphConfig {
        &self.config
    }

    pub async fn execute(&self, sql: &str) -> Result<()> {
        self.client
            .batch_execute(sql)
            .await
            .map_err(|err| GrustError::Backend(format!("PostgreSQL command failed: {err}: {sql}")))
    }

    async fn query_nodes(&self, sql: &str) -> Result<Vec<Node>> {
        let rows = self.client.query(sql, &[]).await.map_err(|err| {
            GrustError::Backend(format!("PostgreSQL node query failed: {err}: {sql}"))
        })?;
        rows.into_iter().map(row_to_node).collect()
    }

    async fn query_edges(&self, sql: &str) -> Result<Vec<Edge>> {
        let rows = self.client.query(sql, &[]).await.map_err(|err| {
            GrustError::Backend(format!("PostgreSQL edge query failed: {err}: {sql}"))
        })?;
        rows.into_iter().map(row_to_edge).collect()
    }

    fn tables(&self) -> UniversalTableRefs {
        UniversalTableRefs {
            nodes: self.nodes_table(),
            edges: self.edges_table(),
        }
    }

    pub fn nodes_table(&self) -> String {
        qualified_table(
            &self.config.schema,
            &format!("{}_nodes", self.config.table_prefix),
        )
    }

    pub fn edges_table(&self) -> String {
        qualified_table(
            &self.config.schema,
            &format!("{}_edges", self.config.table_prefix),
        )
    }
}

impl Drop for PostgresGraphStore {
    fn drop(&mut self) {
        self.connection_task.abort();
    }
}

#[async_trait]
impl GraphStore for PostgresGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        self.bootstrap().await?;
        self.execute(&postgres_schema_sql(
            &self.config,
            &self.nodes_table(),
            &self.edges_table(),
            schema,
        )?)
        .await
    }

    async fn put_node(&self, node: &Node) -> Result<PutOutcome> {
        self.execute(&upsert_nodes_sql(
            &self.nodes_table(),
            std::slice::from_ref(node),
        )?)
        .await?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        self.execute(&upsert_edges_sql(
            &self.edges_table(),
            std::slice::from_ref(edge),
        )?)
        .await?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        let batch_size = self.config.batch_size.max(1);
        let mut report = LoadReport::default();
        for chunk in graph.nodes.chunks(batch_size) {
            self.execute(&upsert_nodes_sql(&self.nodes_table(), chunk)?)
                .await?;
            report.nodes += chunk.len();
        }
        for chunk in graph.edges.chunks(batch_size) {
            self.execute(&upsert_edges_sql(&self.edges_table(), chunk)?)
                .await?;
            report.edges += chunk.len();
        }
        Ok(report)
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        let sql =
            grust_sql_core::select_node_sql(&PostgresDialect, &self.nodes_table(), id, sql_str);
        Ok(self.query_nodes(&sql).await?.into_iter().next())
    }

    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>> {
        match grust_sql_core::select_nodes_sql(&PostgresDialect, &self.nodes_table(), ids, sql_str)
        {
            Some(sql) => self.query_nodes(&sql).await,
            None => Ok(Vec::new()),
        }
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        let sql =
            grust_sql_core::select_edges_sql(&PostgresDialect, &self.edges_table(), query, sql_str);
        self.query_edges(&sql).await
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        let sql = traversal_sql(&self.nodes_table(), &self.edges_table(), &traversal)?;
        self.query_nodes(&sql).await
    }
}

#[async_trait]
impl GraphAdminStore for PostgresGraphStore {
    async fn bootstrap(&self) -> Result<()> {
        let tables = self.tables();
        self.execute(&bootstrap_sql(&self.config, &tables.nodes, &tables.edges)?)
            .await
    }

    async fn clear(&self) -> Result<()> {
        self.execute(&format!(
            "TRUNCATE TABLE {}, {}",
            self.edges_table(),
            self.nodes_table()
        ))
        .await
    }
}

#[async_trait]
impl GraphMutationStore for PostgresGraphStore {
    fn mutation_atomicity(&self) -> GraphMutationAtomicity {
        GraphMutationAtomicity::Transactional
    }

    async fn delete_node(&self, id: &NodeId) -> Result<()> {
        self.execute(&delete_node_sql(&self.nodes_table(), id))
            .await?;
        Ok(())
    }

    async fn delete_edge(&self, from: &NodeId, label: &Label, to: &NodeId) -> Result<()> {
        self.execute(&delete_edge_sql(&self.edges_table(), from, label, to))
            .await?;
        Ok(())
    }

    async fn apply_mutations(&self, mutations: &[GraphMutation]) -> Result<()> {
        if mutations.is_empty() {
            return Ok(());
        }
        self.execute(&apply_mutations_sql(
            &self.nodes_table(),
            &self.edges_table(),
            mutations,
        )?)
        .await
    }
}

pub fn bootstrap_sql(
    config: &PostgresGraphConfig,
    nodes_table: &str,
    edges_table: &str,
) -> Result<String> {
    let schema = quote_ident(&config.schema);
    Ok(grust_sql_core::universal_bootstrap_sql(
        &PostgresDialect,
        &config.table_prefix,
        &UniversalTableRefs {
            nodes: nodes_table.to_string(),
            edges: edges_table.to_string(),
        },
        Some(&format!("CREATE SCHEMA IF NOT EXISTS {schema}")),
        quote_ident,
    ))
}

pub fn postgres_schema_sql(
    config: &PostgresGraphConfig,
    nodes_table: &str,
    edges_table: &str,
    schema: &GraphSchema,
) -> Result<String> {
    grust_sql_core::schema_sql(
        &PostgresDialect,
        &config.table_prefix,
        nodes_table,
        edges_table,
        schema,
        |view| qualified_table(&config.schema, view),
        quote_ident,
        sql_str,
        postgres_prop_expr,
    )
}

pub fn postgres_typed_column(field: &Field) -> Result<String> {
    Ok(format!(
        "{} AS {}",
        postgres_prop_expr(field),
        quote_ident(&field.name)
    ))
}

pub fn postgres_prop_expr(field: &Field) -> String {
    let value = format!("props #>> ARRAY[{}, 'value']", sql_str(&field.name));
    match field.ty {
        FieldType::String
        | FieldType::DateTime
        | FieldType::StringArray
        | FieldType::IntArray
        | FieldType::FloatArray
        | FieldType::Json => value,
        FieldType::Int => format!("({value})::bigint"),
        FieldType::Float => format!("({value})::double precision"),
        FieldType::Bool => format!("({value})::boolean"),
    }
}

pub fn upsert_nodes_sql(table: &str, nodes: &[Node]) -> Result<String> {
    PostgresDialect.upsert_nodes_sql(table, nodes)
}

pub fn upsert_edges_sql(table: &str, edges: &[Edge]) -> Result<String> {
    PostgresDialect.upsert_edges_sql(table, edges)
}

pub fn delete_node_sql(nodes_table: &str, id: &NodeId) -> String {
    grust_sql_core::delete_node_sql(nodes_table, id, sql_str)
}

pub fn patch_node_sql(nodes_table: &str, id: &NodeId, props: &Props) -> Result<String> {
    PostgresDialect.patch_node_sql(nodes_table, id, props)
}

pub fn delete_edge_sql(edges_table: &str, from: &NodeId, label: &Label, to: &NodeId) -> String {
    grust_sql_core::delete_edge_sql(edges_table, from, label, to, sql_str)
}

pub fn mutation_sql(
    nodes_table: &str,
    edges_table: &str,
    mutation: &GraphMutation,
) -> Result<String> {
    grust_sql_core::mutation_sql(
        &PostgresDialect,
        nodes_table,
        edges_table,
        mutation,
        sql_str,
    )
}

pub fn apply_mutations_sql(
    nodes_table: &str,
    edges_table: &str,
    mutations: &[GraphMutation],
) -> Result<String> {
    grust_sql_core::apply_mutations_sql(
        &PostgresDialect,
        nodes_table,
        edges_table,
        mutations,
        sql_str,
    )
}

pub fn traversal_sql(
    nodes_table: &str,
    edges_table: &str,
    traversal: &Traversal,
) -> Result<String> {
    grust_sql_core::traversal_sql(
        &PostgresDialect,
        nodes_table,
        edges_table,
        traversal,
        sql_str,
    )
}

fn jsonb_predicate(alias: &str, key: &str, value: &Value) -> Result<String> {
    validate_json_key(key)?;
    let prop = format!("{alias}.props #>> ARRAY[{}, 'value']", sql_str(key));
    Ok(match value {
        Value::Null => format!("{alias}.props -> {} ->> 'type' = 'null'", sql_str(key)),
        Value::Bool(value) => format!("({prop})::boolean = {value}"),
        Value::Int(value) => format!("({prop})::bigint = {value}"),
        Value::Float(value) => format!("({prop})::double precision = {value}"),
        Value::String(value) => format!("{prop} = {}", sql_str(value)),
        other => {
            let json = serde_json::to_string(other)
                .map_err(|err| GrustError::Serialization(err.to_string()))?;
            format!(
                "{alias}.props -> {} = {}::jsonb",
                sql_str(key),
                sql_str(&json)
            )
        }
    })
}

fn row_to_node(row: tokio_postgres::Row) -> Result<Node> {
    let id: String = row.get("id");
    let label: String = row.get("label");
    let props_json: String = row.get("props");
    let props: Props = serde_json::from_str(&props_json)
        .map_err(|err| GrustError::Serialization(format!("node props JSON parse failed: {err}")))?;
    Ok(Node {
        id: NodeId::new(id),
        label: Label::new(label),
        props,
    })
}

fn row_to_edge(row: tokio_postgres::Row) -> Result<Edge> {
    let id: Option<String> = row.get("id");
    let from_id: String = row.get("from_id");
    let to_id: String = row.get("to_id");
    let label: String = row.get("label");
    let props_json: String = row.get("props");
    let props: Props = serde_json::from_str(&props_json)
        .map_err(|err| GrustError::Serialization(format!("edge props JSON parse failed: {err}")))?;
    let mut edge = Edge::new(label, from_id, to_id, props);
    edge.id = id.map(EdgeId::new);
    Ok(edge)
}

pub fn qualified_table(schema: &str, table: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(table))
}

pub fn unquoted_qualified_table(schema: &str, table: &str) -> String {
    format!("{schema}.{table}")
}

pub fn quote_ident(value: &str) -> String {
    grust_sql_core::quote_ident(value)
}

pub fn sql_str(value: &str) -> String {
    grust_sql_core::sql_str(value)
}

pub fn validate_identifier(value: &str) -> Result<()> {
    grust_sql_core::validate_identifier("PostgreSQL", value)
}

pub fn validate_json_key(value: &str) -> Result<()> {
    validate_identifier(value)
        .map_err(|_| GrustError::Schema(format!("invalid JSON property key '{value}'")))
}

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug)]
struct PostgresDialect;

impl GraphSqlDialect for PostgresDialect {
    fn name(&self) -> &'static str {
        "PostgreSQL"
    }

    fn props_column_type(&self) -> &'static str {
        "jsonb"
    }

    fn empty_props_default(&self) -> &'static str {
        "'{}'::jsonb"
    }

    fn create_view_prefix(&self) -> &'static str {
        "CREATE OR REPLACE VIEW"
    }

    fn node_props_select(&self, alias: &str) -> String {
        if alias.is_empty() {
            "props::text".to_string()
        } else {
            format!("{alias}.props::text")
        }
    }

    fn edge_props_select(&self, alias: &str) -> String {
        self.node_props_select(alias)
    }

    fn json_property_predicate(&self, alias: &str, key: &str, value: &Value) -> Result<String> {
        jsonb_predicate(alias, key, value)
    }

    fn both_direction_join(
        &self,
        edges_table: &str,
        edge_alias: &str,
        prev_alias: &str,
        edge_label: &str,
    ) -> String {
        format!(
            "JOIN LATERAL (
                SELECT to_id AS next_id
                FROM {edges_table}
                WHERE from_id = {prev_alias}.id{edge_label}
                UNION ALL
                SELECT from_id AS next_id
                FROM {edges_table}
                WHERE to_id = {prev_alias}.id{edge_label}
            ) {edge_alias} ON TRUE"
        )
    }

    fn upsert_nodes_sql(&self, table: &str, nodes: &[Node]) -> Result<String> {
        if nodes.is_empty() {
            return Ok(String::new());
        }
        let rows = nodes
            .iter()
            .map(|node| {
                let props = grust_sql_core::props_to_json(&node.props)?;
                Ok(format!(
                    "({}, {}, {}::jsonb)",
                    sql_str(node.id.as_str()),
                    sql_str(node.label.as_str()),
                    sql_str(&props)
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        Ok(format!(
            "INSERT INTO {table} (id, label, props) VALUES {rows}
             ON CONFLICT (id) DO UPDATE SET
                label = EXCLUDED.label,
                props = EXCLUDED.props"
        ))
    }

    fn upsert_edges_sql(&self, table: &str, edges: &[Edge]) -> Result<String> {
        if edges.is_empty() {
            return Ok(String::new());
        }
        let rows = edges
            .iter()
            .map(|edge| {
                let props = grust_sql_core::props_to_json(&edge.props)?;
                Ok(format!(
                    "({}, {}, {}, {}, {}::jsonb)",
                    edge.id
                        .as_ref()
                        .map(|id| sql_str(id.as_str()))
                        .unwrap_or_else(|| "NULL".to_string()),
                    sql_str(edge.from.as_str()),
                    sql_str(edge.to.as_str()),
                    sql_str(edge.label.as_str()),
                    sql_str(&props)
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        Ok(format!(
            "INSERT INTO {table} (id, from_id, to_id, label, props) VALUES {rows}
             ON CONFLICT (from_id, label, to_id) DO UPDATE SET
                id = EXCLUDED.id,
                props = EXCLUDED.props"
        ))
    }

    fn patch_node_sql(&self, nodes_table: &str, id: &NodeId, props: &Props) -> Result<String> {
        let props = grust_sql_core::props_to_json(props)?;
        Ok(format!(
            "UPDATE {nodes_table} SET props = props || {}::jsonb WHERE id = {}",
            sql_str(&props),
            sql_str(id.as_str())
        ))
    }
}
