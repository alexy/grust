use async_trait::async_trait;
use grust_core::prelude::*;
use grust_sql_core::{GraphSqlDialect, UniversalTableRefs};

#[derive(Clone, Debug)]
pub struct TursoConfig {
    pub path: String,
    pub table_prefix: String,
    pub batch_size: usize,
}

impl Default for TursoConfig {
    fn default() -> Self {
        Self {
            path: ":memory:".to_string(),
            table_prefix: "grust".to_string(),
            batch_size: 500,
        }
    }
}

#[cfg(feature = "sync")]
#[derive(Clone, Debug)]
pub struct TursoSyncConfig {
    pub local_path: String,
    pub remote_url: String,
    pub auth_token: Option<String>,
    pub table_prefix: String,
    pub batch_size: usize,
}

#[allow(dead_code)]
enum TursoDatabase {
    Local(turso::Database),
    #[cfg(feature = "sync")]
    Synced(turso::sync::Database),
}

pub struct TursoGraphStore {
    config: TursoConfig,
    _db: TursoDatabase,
    conn: turso::Connection,
}

impl TursoGraphStore {
    pub async fn connect(config: TursoConfig) -> Result<Self> {
        validate_identifier(&config.table_prefix)?;
        let db = turso::Builder::new_local(&config.path)
            .build()
            .await
            .map_err(|err| {
                GrustError::Backend(format!(
                    "failed to open Turso database at {}: {err}",
                    config.path
                ))
            })?;
        let conn = db.connect().map_err(|err| {
            GrustError::Backend(format!("failed to connect to Turso database: {err}"))
        })?;
        Ok(Self {
            config,
            _db: TursoDatabase::Local(db),
            conn,
        })
    }

    pub async fn in_memory() -> Result<Self> {
        Self::connect(TursoConfig::default()).await
    }

    #[cfg(feature = "sync")]
    pub async fn connect_synced(config: TursoSyncConfig) -> Result<Self> {
        validate_identifier(&config.table_prefix)?;
        let mut builder = turso::sync::Builder::new_remote(&config.local_path)
            .with_remote_url(&config.remote_url);
        if let Some(token) = &config.auth_token {
            builder = builder.with_auth_token(token.clone());
        }
        let db = builder.build().await.map_err(|err| {
            GrustError::Backend(format!(
                "failed to open synced Turso database at {}: {err}",
                config.local_path
            ))
        })?;
        let conn = db.connect().await.map_err(|err| {
            GrustError::Backend(format!("failed to connect to synced Turso database: {err}"))
        })?;
        Ok(Self {
            config: TursoConfig {
                path: config.local_path,
                table_prefix: config.table_prefix,
                batch_size: config.batch_size,
            },
            _db: TursoDatabase::Synced(db),
            conn,
        })
    }

    pub fn config(&self) -> &TursoConfig {
        &self.config
    }

    #[cfg(feature = "sync")]
    pub async fn push(&self) -> Result<()> {
        match &self._db {
            TursoDatabase::Synced(db) => db
                .push()
                .await
                .map_err(|err| GrustError::Backend(format!("Turso push failed: {err}"))),
            _ => Err(GrustError::Unsupported(
                "Turso push is only available for synced stores".to_string(),
            )),
        }
    }

    #[cfg(feature = "sync")]
    pub async fn pull(&self) -> Result<bool> {
        match &self._db {
            TursoDatabase::Synced(db) => db
                .pull()
                .await
                .map_err(|err| GrustError::Backend(format!("Turso pull failed: {err}"))),
            _ => Err(GrustError::Unsupported(
                "Turso pull is only available for synced stores".to_string(),
            )),
        }
    }

    async fn execute(&self, sql: &str) -> Result<()> {
        self.conn
            .execute_batch(sql)
            .await
            .map_err(|err| GrustError::Backend(format!("Turso command failed: {err}: {sql}")))
    }

    async fn query_nodes(&self, sql: &str) -> Result<Vec<Node>> {
        let mut rows =
            self.conn.query(sql, ()).await.map_err(|err| {
                GrustError::Backend(format!("Turso node query failed: {err}: {sql}"))
            })?;
        let mut nodes = Vec::new();
        while let Some(row) = rows.next().await.map_err(|err| {
            GrustError::Backend(format!("Turso node row read failed: {err}: {sql}"))
        })? {
            nodes.push(row_to_node(&row)?);
        }
        Ok(nodes)
    }

    async fn query_edges(&self, sql: &str) -> Result<Vec<Edge>> {
        let mut rows =
            self.conn.query(sql, ()).await.map_err(|err| {
                GrustError::Backend(format!("Turso edge query failed: {err}: {sql}"))
            })?;
        let mut edges = Vec::new();
        while let Some(row) = rows.next().await.map_err(|err| {
            GrustError::Backend(format!("Turso edge row read failed: {err}: {sql}"))
        })? {
            edges.push(row_to_edge(&row)?);
        }
        Ok(edges)
    }

    fn nodes_table(&self) -> String {
        quote_ident(&format!("{}_nodes", self.config.table_prefix))
    }

    fn edges_table(&self) -> String {
        quote_ident(&format!("{}_edges", self.config.table_prefix))
    }
}

#[async_trait]
impl GraphStore for TursoGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        self.bootstrap().await?;
        self.execute(&turso_schema_sql(
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
        let sql = grust_sql_core::select_node_sql(&TursoDialect, &self.nodes_table(), id, sql_str);
        Ok(self.query_nodes(&sql).await?.into_iter().next())
    }

    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>> {
        match grust_sql_core::select_nodes_sql(&TursoDialect, &self.nodes_table(), ids, sql_str) {
            Some(sql) => self.query_nodes(&sql).await,
            None => Ok(Vec::new()),
        }
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        let sql =
            grust_sql_core::select_edges_sql(&TursoDialect, &self.edges_table(), query, sql_str);
        self.query_edges(&sql).await
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        let sql = traversal_sql(&self.nodes_table(), &self.edges_table(), &traversal)?;
        self.query_nodes(&sql).await
    }
}

#[async_trait]
impl GraphAdminStore for TursoGraphStore {
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
            "DELETE FROM {};
             DELETE FROM {};",
            self.edges_table(),
            self.nodes_table()
        ))
        .await
    }
}

#[async_trait]
impl GraphMutationStore for TursoGraphStore {
    fn mutation_atomicity(&self) -> GraphMutationAtomicity {
        GraphMutationAtomicity::Transactional
    }

    async fn delete_node(&self, id: &NodeId) -> Result<()> {
        self.execute(&delete_node_sql(&self.nodes_table(), id))
            .await
    }

    async fn delete_edge(&self, from: &NodeId, label: &Label, to: &NodeId) -> Result<()> {
        self.execute(&delete_edge_sql(&self.edges_table(), from, label, to))
            .await
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

pub fn bootstrap_sql(config: &TursoConfig, nodes_table: &str, edges_table: &str) -> Result<String> {
    Ok(grust_sql_core::universal_bootstrap_sql(
        &TursoDialect,
        &config.table_prefix,
        &UniversalTableRefs {
            nodes: nodes_table.to_string(),
            edges: edges_table.to_string(),
        },
        Some("PRAGMA foreign_keys = ON"),
        quote_ident,
    ))
}

pub fn turso_schema_sql(
    config: &TursoConfig,
    nodes_table: &str,
    edges_table: &str,
    schema: &GraphSchema,
) -> Result<String> {
    grust_sql_core::schema_sql(
        &TursoDialect,
        &config.table_prefix,
        nodes_table,
        edges_table,
        schema,
        quote_ident,
        quote_ident,
        sql_str,
        turso_prop_expr,
    )
}

fn turso_prop_expr(field: &Field) -> String {
    let value = format!(
        "json_extract(props, '$.{}.value')",
        json_path_key(&field.name)
    );
    match field.ty {
        FieldType::String
        | FieldType::DateTime
        | FieldType::StringArray
        | FieldType::IntArray
        | FieldType::FloatArray
        | FieldType::Json => value,
        FieldType::Int => format!("CAST({value} AS INTEGER)"),
        FieldType::Float => format!("CAST({value} AS REAL)"),
        FieldType::Bool => format!("CAST({value} AS INTEGER)"),
    }
}

pub fn upsert_nodes_sql(table: &str, nodes: &[Node]) -> Result<String> {
    TursoDialect.upsert_nodes_sql(table, nodes)
}

pub fn upsert_edges_sql(table: &str, edges: &[Edge]) -> Result<String> {
    TursoDialect.upsert_edges_sql(table, edges)
}

pub fn delete_node_sql(nodes_table: &str, id: &NodeId) -> String {
    grust_sql_core::delete_node_sql(nodes_table, id, sql_str)
}

pub fn patch_node_sql(nodes_table: &str, id: &NodeId, props: &Props) -> Result<String> {
    TursoDialect.patch_node_sql(nodes_table, id, props)
}

pub fn delete_edge_sql(edges_table: &str, from: &NodeId, label: &Label, to: &NodeId) -> String {
    grust_sql_core::delete_edge_sql(edges_table, from, label, to, sql_str)
}

pub fn mutation_sql(
    nodes_table: &str,
    edges_table: &str,
    mutation: &GraphMutation,
) -> Result<String> {
    grust_sql_core::mutation_sql(&TursoDialect, nodes_table, edges_table, mutation, sql_str)
}

pub fn apply_mutations_sql(
    nodes_table: &str,
    edges_table: &str,
    mutations: &[GraphMutation],
) -> Result<String> {
    grust_sql_core::apply_mutations_sql(&TursoDialect, nodes_table, edges_table, mutations, sql_str)
}

pub fn traversal_sql(
    nodes_table: &str,
    edges_table: &str,
    traversal: &Traversal,
) -> Result<String> {
    grust_sql_core::traversal_sql(&TursoDialect, nodes_table, edges_table, traversal, sql_str)
}

fn json_predicate(alias: &str, key: &str, value: &Value) -> Result<String> {
    validate_identifier(key)?;
    let path = format!("$.{}.value", json_path_key(key));
    let prop = format!("json_extract({alias}.props, {})", sql_str(&path));
    Ok(match value {
        Value::Null => format!("json_type({alias}.props, {}) = 'null'", sql_str(&path)),
        Value::Bool(value) => format!("{prop} = {}", if *value { 1 } else { 0 }),
        Value::Int(value) => format!("CAST({prop} AS INTEGER) = {value}"),
        Value::Float(value) => format!("CAST({prop} AS REAL) = {value}"),
        Value::String(value) => format!("{prop} = {}", sql_str(value)),
        other => {
            let json = serde_json::to_string(other)
                .map_err(|err| GrustError::Serialization(err.to_string()))?;
            format!("{prop} = {}", sql_str(&json))
        }
    })
}

fn row_to_node(row: &turso::Row) -> Result<Node> {
    let id = row_text(row, 0, "node id")?;
    let label = row_text(row, 1, "node label")?;
    let props_json = row_text(row, 2, "node props")?;
    let props: Props = serde_json::from_str(&props_json)
        .map_err(|err| GrustError::Serialization(format!("node props JSON parse failed: {err}")))?;
    Ok(Node {
        id: NodeId::new(id),
        label: Label::new(label),
        props,
    })
}

fn row_to_edge(row: &turso::Row) -> Result<Edge> {
    let id = row_optional_text(row, 0, "edge id")?;
    let from_id = row_text(row, 1, "edge from_id")?;
    let to_id = row_text(row, 2, "edge to_id")?;
    let label = row_text(row, 3, "edge label")?;
    let props_json = row_text(row, 4, "edge props")?;
    let props: Props = serde_json::from_str(&props_json)
        .map_err(|err| GrustError::Serialization(format!("edge props JSON parse failed: {err}")))?;
    let mut edge = Edge::new(label, from_id, to_id, props);
    edge.id = id.map(EdgeId::new);
    Ok(edge)
}

fn row_text(row: &turso::Row, idx: usize, name: &str) -> Result<String> {
    row_optional_text(row, idx, name)?.ok_or_else(|| {
        GrustError::Backend(format!("Turso {name} column unexpectedly contained NULL"))
    })
}

fn row_optional_text(row: &turso::Row, idx: usize, name: &str) -> Result<Option<String>> {
    match row
        .get_value(idx)
        .map_err(|err| GrustError::Backend(format!("failed to read Turso {name}: {err}")))?
    {
        turso::Value::Text(value) => Ok(Some(value)),
        turso::Value::Null => Ok(None),
        other => Err(GrustError::Backend(format!(
            "Turso {name} column had unexpected value {other:?}"
        ))),
    }
}

fn quote_ident(value: &str) -> String {
    grust_sql_core::quote_ident(value)
}

fn sql_str(value: &str) -> String {
    grust_sql_core::sql_str(value)
}

fn validate_identifier(value: &str) -> Result<()> {
    grust_sql_core::validate_identifier("Turso", value)
}

fn json_path_key(value: &str) -> String {
    grust_sql_core::json_path_key(value)
}

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug)]
struct TursoDialect;

impl GraphSqlDialect for TursoDialect {
    fn name(&self) -> &'static str {
        "Turso"
    }

    fn props_column_type(&self) -> &'static str {
        "TEXT"
    }

    fn empty_props_default(&self) -> &'static str {
        "'{}'"
    }

    fn node_props_select(&self, alias: &str) -> String {
        if alias.is_empty() {
            "props".to_string()
        } else {
            format!("{alias}.props")
        }
    }

    fn edge_props_select(&self, alias: &str) -> String {
        self.node_props_select(alias)
    }

    fn json_property_predicate(&self, alias: &str, key: &str, value: &Value) -> Result<String> {
        json_predicate(alias, key, value)
    }

    fn both_direction_join(
        &self,
        edges_table: &str,
        edge_alias: &str,
        prev_alias: &str,
        edge_label: &str,
    ) -> String {
        format!(
            "JOIN (
                SELECT to_id AS next_id, from_id AS current_id, label FROM {edges_table}
                UNION ALL
                SELECT from_id AS next_id, to_id AS current_id, label FROM {edges_table}
            ) {edge_alias} ON {edge_alias}.current_id = {prev_alias}.id{edge_label}"
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
                    "({}, {}, {})",
                    sql_str(node.id.as_str()),
                    sql_str(node.label.as_str()),
                    sql_str(&props)
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        Ok(format!(
            "INSERT INTO {table} (id, label, props) VALUES {rows}
             ON CONFLICT(id) DO UPDATE SET
                label = excluded.label,
                props = excluded.props"
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
                    "({}, {}, {}, {}, {})",
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
             ON CONFLICT(from_id, label, to_id) DO UPDATE SET
                id = excluded.id,
                props = excluded.props"
        ))
    }

    fn patch_node_sql(&self, nodes_table: &str, id: &NodeId, props: &Props) -> Result<String> {
        let props = grust_sql_core::props_to_json(props)?;
        Ok(format!(
            "UPDATE {nodes_table} SET props = json_patch(props, {}) WHERE id = {}",
            sql_str(&props),
            sql_str(id.as_str())
        ))
    }
}
