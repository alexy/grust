use async_trait::async_trait;
use grust_core::prelude::*;
use tokio::task::JoinHandle;
use tokio_postgres::{Client, NoTls};

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
    client: Client,
    connection_task: JoinHandle<()>,
}

impl PgGraphStore {
    pub async fn connect(config: PgGraphConfig) -> Result<Self> {
        validate_identifier(&config.schema)?;
        validate_identifier(&config.table_prefix)?;
        let (client, connection) = tokio_postgres::connect(&config.connection_string, NoTls)
            .await
            .map_err(|err| {
                GrustError::Backend(format!(
                    "failed to connect to PostgreSQL for pgGraph backend: {err}"
                ))
            })?;
        let connection_task = tokio::spawn(async move {
            if let Err(err) = connection.await {
                eprintln!("grust-pggraph PostgreSQL connection task ended: {err}");
            }
        });
        Ok(Self {
            config,
            client,
            connection_task,
        })
    }

    pub fn config(&self) -> &PgGraphConfig {
        &self.config
    }

    pub async fn build_projection(&self) -> Result<()> {
        self.execute(&format!(
            "SELECT * FROM graph.build({})",
            sql_str(self.config.build_mode.as_pggraph_mode())
        ))
        .await
    }

    async fn execute(&self, sql: &str) -> Result<()> {
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

    fn nodes_table(&self) -> String {
        qualified_table(
            &self.config.schema,
            &format!("{}_nodes", self.config.table_prefix),
        )
    }

    fn edges_table(&self) -> String {
        qualified_table(
            &self.config.schema,
            &format!("{}_edges", self.config.table_prefix),
        )
    }
}

impl Drop for PgGraphStore {
    fn drop(&mut self) {
        self.connection_task.abort();
    }
}

#[async_trait]
impl GraphStore for PgGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        self.bootstrap().await?;
        self.execute(&pggraph_schema_sql(
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
        if self.config.auto_build {
            self.build_projection().await?;
        }
        Ok(PutOutcome::Upserted)
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        self.execute(&upsert_edges_sql(
            &self.edges_table(),
            std::slice::from_ref(edge),
        )?)
        .await?;
        if self.config.auto_build {
            self.build_projection().await?;
        }
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
        if self.config.auto_build {
            self.build_projection().await?;
        }
        Ok(report)
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        let sql = format!(
            "SELECT id, label, props::text AS props FROM {} WHERE id = {} LIMIT 1",
            self.nodes_table(),
            sql_str(id.as_str())
        );
        Ok(self.query_nodes(&sql).await?.into_iter().next())
    }

    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids = ids
            .iter()
            .map(|id| sql_str(id.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, label, props::text AS props FROM {} WHERE id IN ({ids})",
            self.nodes_table()
        );
        self.query_nodes(&sql).await
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        let mut conditions = Vec::new();
        if let Some(from) = query.from {
            conditions.push(format!("from_id = {}", sql_str(from.as_str())));
        }
        if let Some(to) = query.to {
            conditions.push(format!("to_id = {}", sql_str(to.as_str())));
        }
        if let Some(label) = query.label {
            conditions.push(format!("label = {}", sql_str(label.as_str())));
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };
        self.query_edges(&format!(
            "SELECT id, from_id, to_id, label, props::text AS props FROM {}{}",
            self.edges_table(),
            where_clause
        ))
        .await
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        let sql = traversal_sql(&self.nodes_table(), &self.edges_table(), &traversal)?;
        self.query_nodes(&sql).await
    }
}

#[async_trait]
impl GraphAdminStore for PgGraphStore {
    async fn bootstrap(&self) -> Result<()> {
        self.execute(&bootstrap_sql(
            &self.config,
            &self.nodes_table(),
            &self.edges_table(),
        )?)
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
impl GraphMutationStore for PgGraphStore {
    async fn delete_node(&self, id: &NodeId) -> Result<()> {
        self.execute(&delete_node_sql(&self.nodes_table(), id))
            .await?;
        if self.config.auto_build {
            self.build_projection().await?;
        }
        Ok(())
    }

    async fn delete_edge(&self, from: &NodeId, label: &Label, to: &NodeId) -> Result<()> {
        self.execute(&delete_edge_sql(&self.edges_table(), from, label, to))
            .await?;
        if self.config.auto_build {
            self.build_projection().await?;
        }
        Ok(())
    }

    async fn apply_mutations(&self, mutations: &[GraphMutation]) -> Result<()> {
        if mutations.is_empty() {
            return Ok(());
        }
        self.execute(&apply_mutations_sql(
            &self.config,
            &self.nodes_table(),
            &self.edges_table(),
            mutations,
        )?)
        .await
    }
}

fn bootstrap_sql(config: &PgGraphConfig, nodes_table: &str, edges_table: &str) -> Result<String> {
    let schema = quote_ident(&config.schema);
    Ok(format!(
        "CREATE EXTENSION IF NOT EXISTS graph;
         CREATE SCHEMA IF NOT EXISTS {schema};
         CREATE TABLE IF NOT EXISTS {nodes_table} (
            id text PRIMARY KEY,
            label text NOT NULL,
            props jsonb NOT NULL DEFAULT '{{}}'::jsonb
         );
         CREATE TABLE IF NOT EXISTS {edges_table} (
            id text,
            from_id text NOT NULL REFERENCES {nodes_table}(id) ON DELETE CASCADE,
            to_id text NOT NULL REFERENCES {nodes_table}(id) ON DELETE CASCADE,
            label text NOT NULL,
            props jsonb NOT NULL DEFAULT '{{}}'::jsonb,
            PRIMARY KEY (from_id, label, to_id)
         );
         CREATE INDEX IF NOT EXISTS {edge_from_idx} ON {edges_table}(from_id);
         CREATE INDEX IF NOT EXISTS {edge_to_idx} ON {edges_table}(to_id);
         CREATE INDEX IF NOT EXISTS {node_label_idx} ON {nodes_table}(label);
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
        edge_from_idx = quote_ident(&format!("{}_edges_from_idx", config.table_prefix)),
        edge_to_idx = quote_ident(&format!("{}_edges_to_idx", config.table_prefix)),
        node_label_idx = quote_ident(&format!("{}_nodes_label_idx", config.table_prefix)),
        nodes_regclass = sql_str(&unquoted_qualified_table(
            &config.schema,
            &format!("{}_nodes", config.table_prefix)
        )),
        edges_regclass = sql_str(&unquoted_qualified_table(
            &config.schema,
            &format!("{}_edges", config.table_prefix)
        )),
    ))
}

fn pggraph_schema_sql(
    config: &PgGraphConfig,
    nodes_table: &str,
    edges_table: &str,
    schema: &GraphSchema,
) -> Result<String> {
    let mut statements = Vec::new();

    for node_type in &schema.nodes {
        let view = qualified_table(
            &config.schema,
            &format!(
                "{}_node_{}",
                config.table_prefix,
                schema_identifier(node_type.label.as_str())?
            ),
        );
        let columns = node_type
            .fields
            .iter()
            .map(pggraph_typed_column)
            .collect::<Result<Vec<_>>>()?
            .join(",\n            ");
        let columns = if columns.is_empty() {
            String::new()
        } else {
            format!(",\n            {columns}")
        };
        statements.push(format!(
            "CREATE OR REPLACE VIEW {view} AS
             SELECT id{columns}
             FROM {nodes_table}
             WHERE label = {};",
            sql_str(node_type.label.as_str())
        ));

        for field in &node_type.fields {
            statements.push(format!(
                "CREATE INDEX IF NOT EXISTS {} ON {nodes_table} (({})) WHERE label = {};",
                quote_ident(&format!(
                    "{}_node_{}_{}_idx",
                    config.table_prefix,
                    schema_identifier(node_type.label.as_str())?,
                    schema_identifier(&field.name)?
                )),
                pggraph_prop_expr(field),
                sql_str(node_type.label.as_str())
            ));
        }
    }

    for edge_type in &schema.edges {
        let view = qualified_table(
            &config.schema,
            &format!(
                "{}_edge_{}",
                config.table_prefix,
                schema_identifier(edge_type.label.as_str())?
            ),
        );
        let columns = edge_type
            .fields
            .iter()
            .map(pggraph_typed_column)
            .collect::<Result<Vec<_>>>()?
            .join(",\n            ");
        let columns = if columns.is_empty() {
            String::new()
        } else {
            format!(",\n            {columns}")
        };
        statements.push(format!(
            "CREATE OR REPLACE VIEW {view} AS
             SELECT id, from_id, to_id{columns}
             FROM {edges_table}
             WHERE label = {};",
            sql_str(edge_type.label.as_str())
        ));

        for field in &edge_type.fields {
            statements.push(format!(
                "CREATE INDEX IF NOT EXISTS {} ON {edges_table} (({})) WHERE label = {};",
                quote_ident(&format!(
                    "{}_edge_{}_{}_idx",
                    config.table_prefix,
                    schema_identifier(edge_type.label.as_str())?,
                    schema_identifier(&field.name)?
                )),
                pggraph_prop_expr(field),
                sql_str(edge_type.label.as_str())
            ));
        }
    }

    Ok(statements.join("\n"))
}

fn pggraph_typed_column(field: &Field) -> Result<String> {
    Ok(format!(
        "{} AS {}",
        pggraph_prop_expr(field),
        quote_ident(&field.name)
    ))
}

fn pggraph_prop_expr(field: &Field) -> String {
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

fn upsert_nodes_sql(table: &str, nodes: &[Node]) -> Result<String> {
    if nodes.is_empty() {
        return Ok(String::new());
    }
    let rows = nodes
        .iter()
        .map(|node| {
            let props = props_to_json(&node.props)?;
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

fn upsert_edges_sql(table: &str, edges: &[Edge]) -> Result<String> {
    if edges.is_empty() {
        return Ok(String::new());
    }
    let rows = edges
        .iter()
        .map(|edge| {
            let props = props_to_json(&edge.props)?;
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

fn delete_node_sql(nodes_table: &str, id: &NodeId) -> String {
    format!(
        "DELETE FROM {nodes_table} WHERE id = {}",
        sql_str(id.as_str())
    )
}

fn patch_node_sql(nodes_table: &str, id: &NodeId, props: &Props) -> Result<String> {
    let props = props_to_json(props)?;
    Ok(format!(
        "UPDATE {nodes_table} SET props = props || {}::jsonb WHERE id = {}",
        sql_str(&props),
        sql_str(id.as_str())
    ))
}

fn delete_edge_sql(edges_table: &str, from: &NodeId, label: &Label, to: &NodeId) -> String {
    format!(
        "DELETE FROM {edges_table} WHERE from_id = {} AND label = {} AND to_id = {}",
        sql_str(from.as_str()),
        sql_str(label.as_str()),
        sql_str(to.as_str())
    )
}

fn mutation_sql(nodes_table: &str, edges_table: &str, mutation: &GraphMutation) -> Result<String> {
    Ok(match mutation {
        GraphMutation::UpsertNode(node) => {
            upsert_nodes_sql(nodes_table, std::slice::from_ref(node))?
        }
        GraphMutation::PatchNode { id, props } => patch_node_sql(nodes_table, id, props)?,
        GraphMutation::DeleteMatchingNodes { .. } => {
            return Err(GrustError::Unsupported(
                "pgGraph matched node deletes are not implemented yet".to_string(),
            ));
        }
        GraphMutation::DeleteNode(id) => delete_node_sql(nodes_table, id),
        GraphMutation::UpsertEdge(edge) => {
            upsert_edges_sql(edges_table, std::slice::from_ref(edge))?
        }
        GraphMutation::DeleteEdge { from, label, to } => {
            delete_edge_sql(edges_table, from, label, to)
        }
    })
}

fn apply_mutations_sql(
    config: &PgGraphConfig,
    nodes_table: &str,
    edges_table: &str,
    mutations: &[GraphMutation],
) -> Result<String> {
    let mut statements = vec!["BEGIN".to_string()];
    for mutation in mutations {
        statements.push(mutation_sql(nodes_table, edges_table, mutation)?);
    }
    if config.auto_build {
        statements.push(format!(
            "SELECT * FROM graph.build({})",
            sql_str(config.build_mode.as_pggraph_mode())
        ));
    }
    statements.push("COMMIT".to_string());
    Ok(statements
        .into_iter()
        .map(|statement| statement.trim().trim_end_matches(';').to_string())
        .filter(|statement| !statement.is_empty())
        .collect::<Vec<_>>()
        .join(";\n"))
}

fn traversal_sql(nodes_table: &str, edges_table: &str, traversal: &Traversal) -> Result<String> {
    if traversal.steps.is_empty() {
        let where_clause = start_where_clause(&traversal.start, "n0")?;
        return Ok(format!(
            "SELECT n0.id, n0.label, n0.props::text AS props FROM {nodes_table} n0{where_clause}{}",
            limit_clause(traversal.limit)
        ));
    }

    let mut joins = Vec::new();
    for (idx, step) in traversal.steps.iter().enumerate() {
        let prev = format!("n{idx}");
        let edge = format!("e{idx}");
        let next = format!("n{}", idx + 1);
        let edge_label = step
            .edge
            .as_ref()
            .map(|label| format!(" AND {edge}.label = {}", sql_str(label.as_str())))
            .unwrap_or_default();
        let node_label = step
            .node
            .as_ref()
            .map(|label| format!(" AND {next}.label = {}", sql_str(label.as_str())))
            .unwrap_or_default();

        match step.direction {
            Direction::Out => {
                joins.push(format!(
                    "JOIN {edges_table} {edge} ON {edge}.from_id = {prev}.id{edge_label}"
                ));
                joins.push(format!(
                    "JOIN {nodes_table} {next} ON {next}.id = {edge}.to_id{node_label}"
                ));
            }
            Direction::In => {
                joins.push(format!(
                    "JOIN {edges_table} {edge} ON {edge}.to_id = {prev}.id{edge_label}"
                ));
                joins.push(format!(
                    "JOIN {nodes_table} {next} ON {next}.id = {edge}.from_id{node_label}"
                ));
            }
            Direction::Both => {
                joins.push(format!(
                    "JOIN LATERAL (
                        SELECT to_id AS next_id
                        FROM {edges_table}
                        WHERE from_id = {prev}.id{edge_label}
                        UNION ALL
                        SELECT from_id AS next_id
                        FROM {edges_table}
                        WHERE to_id = {prev}.id{edge_label}
                    ) {edge} ON TRUE"
                ));
                joins.push(format!(
                    "JOIN {nodes_table} {next} ON {next}.id = {edge}.next_id{node_label}"
                ));
            }
        }
    }

    let last = format!("n{}", traversal.steps.len());
    Ok(format!(
        "SELECT {last}.id, {last}.label, {last}.props::text AS props
         FROM {nodes_table} n0
         {}
         {}{}",
        joins.join(" "),
        start_where_clause(&traversal.start, "n0")?,
        limit_clause(traversal.limit)
    ))
}

fn start_where_clause(start: &Start, alias: &str) -> Result<String> {
    match start {
        Start::Node(id) => Ok(format!(" WHERE {alias}.id = {}", sql_str(id.as_str()))),
        Start::NodesByLabel(label) => Ok(format!(
            " WHERE {alias}.label = {}",
            sql_str(label.as_str())
        )),
        Start::NodesByProperty { label, key, value } => Ok(format!(
            " WHERE {alias}.label = {} AND {}",
            sql_str(label.as_str()),
            jsonb_predicate(alias, key, value)?
        )),
    }
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

fn props_to_json(props: &Props) -> Result<String> {
    serde_json::to_string(props).map_err(|err| GrustError::Serialization(err.to_string()))
}

fn limit_clause(limit: Option<u32>) -> String {
    limit
        .map(|limit| format!(" LIMIT {limit}"))
        .unwrap_or_default()
}

fn qualified_table(schema: &str, table: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(table))
}

fn unquoted_qualified_table(schema: &str, table: &str) -> String {
    format!("{schema}.{table}")
}

fn quote_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn sql_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn validate_identifier(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        || value.chars().next().is_some_and(|ch| ch.is_ascii_digit())
    {
        return Err(GrustError::Schema(format!(
            "invalid PostgreSQL identifier '{value}'"
        )));
    }
    Ok(())
}

fn validate_json_key(value: &str) -> Result<()> {
    validate_identifier(value)
        .map_err(|_| GrustError::Schema(format!("invalid JSON property key '{value}'")))
}

#[cfg(test)]
mod tests;
