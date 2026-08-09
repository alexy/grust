use async_trait::async_trait;
use grust_core::prelude::*;
use grust_cypher::pushdown::{NoTypeHints, SqlDialect, StrOp, combine_union, plan_read};
use grust_cypher::{CypherParameters, CypherResultTable};
use grust_sql_core::{GraphSqlDialect, UniversalTableRefs};

mod guarded_commit;

/// Journal/concurrency mode for a local Turso database.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TursoJournalMode {
    /// Write-ahead logging — Turso's default single-writer mode.
    #[default]
    Wal,
    /// Multi-version concurrency control (`PRAGMA journal_mode = mvcc`), which
    /// enables `BEGIN CONCURRENT` concurrent writers. MVCC is a database-*header*
    /// mode, so it only takes effect on a **fresh** database — an existing WAL
    /// database is not converted (`connect` errors if the mode cannot be applied).
    Mvcc,
}

impl TursoJournalMode {
    /// The `PRAGMA journal_mode` value the engine reports/accepts.
    fn pragma_value(self) -> &'static str {
        match self {
            TursoJournalMode::Wal => "wal",
            TursoJournalMode::Mvcc => "mvcc",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TursoConfig {
    pub path: String,
    pub table_prefix: String,
    pub batch_size: usize,
    /// Journal/concurrency mode for the local database (default `Wal`).
    pub journal_mode: TursoJournalMode,
}

impl Default for TursoConfig {
    fn default() -> Self {
        Self {
            path: ":memory:".to_string(),
            table_prefix: "grust".to_string(),
            batch_size: 500,
            journal_mode: TursoJournalMode::Wal,
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
    /// Serializes all operations on `conn` so an explicit guarded transaction
    /// cannot accidentally absorb a concurrent read or write on the same
    /// connection.
    connection_gate: tokio::sync::Mutex<()>,
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
        let store = Self {
            config,
            _db: TursoDatabase::Local(db),
            conn,
            connection_gate: tokio::sync::Mutex::new(()),
        };
        store.apply_journal_mode().await?;
        Ok(store)
    }

    /// Apply the configured journal mode on a fresh connection. MVCC is set via
    /// `PRAGMA journal_mode = mvcc` (a database-header mode) and verified by
    /// reading the mode back, so a silently-unconverted existing WAL database
    /// surfaces as an error rather than running in the wrong mode.
    async fn apply_journal_mode(&self) -> Result<()> {
        if self.config.journal_mode == TursoJournalMode::Wal {
            // WAL is the engine default; nothing to enforce.
            return Ok(());
        }
        let want = self.config.journal_mode.pragma_value();
        let got = self
            .query_scalar_text(&format!("PRAGMA journal_mode = {want}"))
            .await?;
        if got.as_deref() != Some(want) {
            return Err(GrustError::Backend(format!(
                "requested Turso journal_mode = {want} but the database reports {got:?}; \
                 MVCC must be set on a fresh database (an existing WAL database cannot be converted)"
            )));
        }
        Ok(())
    }

    /// Run a query expected to yield a single text cell in its first row.
    async fn query_scalar_text(&self, sql: &str) -> Result<Option<String>> {
        let _gate = self.connection_gate.lock().await;
        let mut rows = self
            .conn
            .query(sql, ())
            .await
            .map_err(|err| GrustError::Backend(format!("Turso query failed: {err}: {sql}")))?;
        match rows
            .next()
            .await
            .map_err(|err| GrustError::Backend(format!("Turso row read failed: {err}: {sql}")))?
        {
            Some(row) => row_optional_text(&row, 0, "pragma result"),
            None => Ok(None),
        }
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
                journal_mode: TursoJournalMode::Wal,
            },
            _db: TursoDatabase::Synced(db),
            conn,
            connection_gate: tokio::sync::Mutex::new(()),
        })
    }

    pub fn config(&self) -> &TursoConfig {
        &self.config
    }

    #[cfg(feature = "sync")]
    pub async fn push(&self) -> Result<()> {
        let _gate = self.connection_gate.lock().await;
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
        let _gate = self.connection_gate.lock().await;
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
        let _gate = self.connection_gate.lock().await;
        self.execute_unlocked(sql).await
    }

    async fn execute_unlocked(&self, sql: &str) -> Result<()> {
        self.conn
            .execute_batch(sql)
            .await
            .map_err(|err| GrustError::Backend(format!("Turso command failed: {err}: {sql}")))
    }

    /// Execute a single data-write statement. In WAL mode this is a plain
    /// auto-commit statement (unchanged); in MVCC mode it runs inside a
    /// `BEGIN CONCURRENT` transaction with conflict retry.
    async fn execute_data(&self, sql: &str) -> Result<()> {
        match self.config.journal_mode {
            TursoJournalMode::Wal => self.execute(sql).await,
            TursoJournalMode::Mvcc => {
                self.execute_concurrent(std::slice::from_ref(&sql.to_string()))
                    .await
            }
        }
    }

    /// Run `statements` as one MVCC `BEGIN CONCURRENT … COMMIT` transaction,
    /// retrying the whole transaction on a write-write / busy conflict (bounded).
    /// Only used when `journal_mode == Mvcc`.
    async fn execute_concurrent(&self, statements: &[String]) -> Result<()> {
        const MAX_ATTEMPTS: usize = 8;
        let body = statements
            .iter()
            .map(|s| s.trim().trim_end_matches(';'))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(";\n");
        if body.is_empty() {
            return Ok(());
        }
        let txn = format!("BEGIN CONCURRENT;\n{body};\nCOMMIT;");
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.execute(&txn).await {
                Ok(()) => return Ok(()),
                Err(err) => {
                    // A conflict aborts the transaction; clear any residual state
                    // (best-effort) before retrying or surfacing the error.
                    let _ = self.execute("ROLLBACK").await;
                    if is_mvcc_conflict(&err) && attempt < MAX_ATTEMPTS {
                        continue;
                    }
                    return Err(err);
                }
            }
        }
    }

    async fn query_nodes(&self, sql: &str) -> Result<Vec<Node>> {
        let _gate = self.connection_gate.lock().await;
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
        let _gate = self.connection_gate.lock().await;
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

    fn commits_table(&self) -> String {
        quote_ident(&format!("{}_commits", self.config.table_prefix))
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
        self.execute_data(&upsert_nodes_sql(
            &self.nodes_table(),
            std::slice::from_ref(node),
        )?)
        .await?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        self.execute_data(&upsert_edges_sql(
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
            self.execute_data(&upsert_nodes_sql(&self.nodes_table(), chunk)?)
                .await?;
            report.nodes += chunk.len();
        }
        for chunk in graph.edges.chunks(batch_size) {
            self.execute_data(&upsert_edges_sql(&self.edges_table(), chunk)?)
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
             DELETE FROM {};
             DELETE FROM {};",
            self.edges_table(),
            self.nodes_table(),
            self.commits_table()
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
        self.execute_data(&delete_node_sql(&self.nodes_table(), id))
            .await
    }

    async fn delete_edge(&self, from: &NodeId, label: &Label, to: &NodeId) -> Result<()> {
        self.execute_data(&delete_edge_sql(&self.edges_table(), from, label, to))
            .await
    }

    async fn apply_mutations(&self, mutations: &[GraphMutation]) -> Result<()> {
        if mutations.is_empty() {
            return Ok(());
        }
        match self.config.journal_mode {
            TursoJournalMode::Wal => {
                self.execute(&apply_mutations_sql(
                    &self.nodes_table(),
                    &self.edges_table(),
                    mutations,
                )?)
                .await
            }
            TursoJournalMode::Mvcc => {
                let nodes = self.nodes_table();
                let edges = self.edges_table();
                let statements = mutations
                    .iter()
                    .map(|m| {
                        grust_sql_core::mutation_sql(&TursoDialect, &nodes, &edges, m, sql_str)
                    })
                    .collect::<Result<Vec<_>>>()?;
                self.execute_concurrent(&statements).await
            }
        }
    }
}

#[async_trait]
impl CypherMutationExecutor for TursoGraphStore {
    async fn execute_cypher_mutation_plan(
        &self,
        plan: &GraphMutationPlan,
    ) -> Result<GraphMutationReport> {
        let mut report = plan.report();
        for operation in &plan.operations {
            match operation {
                GraphMutationPlanOp::PatchMatchingNodes {
                    label,
                    props,
                    predicates,
                    patch,
                    ..
                } => {
                    let nodes = self
                        .matching_nodes(label.as_ref(), props, predicates)
                        .await?;
                    report.matched_rows += nodes.len();
                    report.node_patches += nodes.len();
                    report.changed_nodes += nodes.len();

                    for mut node in nodes {
                        for (key, value) in patch {
                            node.props.insert(key.clone(), value.clone());
                        }
                        self.put_node(&node).await?;
                    }
                }
                GraphMutationPlanOp::UpsertNode { node, .. } => {
                    classify_node_upsert(self.put_node(node).await?, &mut report);
                }
                GraphMutationPlanOp::UpsertEdge { edge, .. } => {
                    classify_edge_upsert(self.put_edge(edge).await?, &mut report);
                }
                GraphMutationPlanOp::DeleteMatchingNodes { .. } => {
                    return Err(GrustError::CypherExecution(
                        "matched node deletes require Turso delete query support".to_string(),
                    ));
                }
                GraphMutationPlanOp::UpdateMatchingNodeProperty { .. } => {
                    return Err(GrustError::CypherExecution(
                        "matched node expression updates require Turso expression update support"
                            .to_string(),
                    ));
                }
                GraphMutationPlanOp::RemoveMatchingNodeProps { .. } => {
                    return Err(GrustError::CypherExecution(
                        "matched node property removals require Turso property removal support"
                            .to_string(),
                    ));
                }
                GraphMutationPlanOp::PatchMatchingEdges { .. } => {
                    return Err(GrustError::CypherExecution(
                        "matched edge patches require Turso edge query support".to_string(),
                    ));
                }
                GraphMutationPlanOp::UpdateMatchingEdgeProperty { .. } => {
                    return Err(GrustError::CypherExecution(
                        "matched edge expression updates require Turso expression update support"
                            .to_string(),
                    ));
                }
                GraphMutationPlanOp::RemoveMatchingEdgeProps { .. } => {
                    return Err(GrustError::CypherExecution(
                        "matched edge property removals require Turso property removal support"
                            .to_string(),
                    ));
                }
                GraphMutationPlanOp::DeleteMatchingEdges { .. } => {
                    return Err(GrustError::CypherExecution(
                        "matched edge deletes require Turso edge query support".to_string(),
                    ));
                }
                GraphMutationPlanOp::UpsertEdgesFromNodeMatches { .. } => {
                    return Err(GrustError::CypherExecution(
                        "row-producing edge upserts require Turso node-pair query support"
                            .to_string(),
                    ));
                }
                _ => {
                    let mutation = GraphMutation::from(operation.clone());
                    self.apply_mutations(std::slice::from_ref(&mutation))
                        .await?;
                }
            }
        }
        Ok(report)
    }
}

pub fn bootstrap_sql(config: &TursoConfig, nodes_table: &str, edges_table: &str) -> Result<String> {
    let graph_sql = grust_sql_core::universal_bootstrap_sql(
        &TursoDialect,
        &config.table_prefix,
        &UniversalTableRefs {
            nodes: nodes_table.to_string(),
            edges: edges_table.to_string(),
        },
        Some("PRAGMA foreign_keys = ON"),
        quote_ident,
    );
    Ok(format!(
        "{graph_sql}\n{}",
        guarded_commit::ledger_bootstrap_sql(config)
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
        grust_sql_core::GraphSqlSchemaLayout {
            table_prefix: &config.table_prefix,
            nodes_table,
            edges_table,
        },
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

impl TursoGraphStore {
    async fn matching_nodes(
        &self,
        label: Option<&Label>,
        props: &Props,
        predicates: &[GraphPropertyPredicate],
    ) -> Result<Vec<Node>> {
        let mut clauses = Vec::new();
        if let Some(label) = label {
            clauses.push(format!("n.label = {}", sql_str(label.as_str())));
        }
        for (key, value) in props {
            if key == "id" {
                let Some(id) = value.as_str() else {
                    return Ok(Vec::new());
                };
                clauses.push(format!("n.id = {}", sql_str(id)));
            } else {
                clauses.push(json_predicate("n", key, value)?);
            }
        }
        let where_clause = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };
        let sql = format!(
            "SELECT n.id, n.label, {} AS props FROM {} n{where_clause}",
            TursoDialect.node_props_select("n"),
            self.nodes_table()
        );
        let mut nodes = self.query_nodes(&sql).await?;
        if !predicates.is_empty() {
            nodes.retain(|node| {
                predicates
                    .iter()
                    .all(|predicate| predicate.matches(node.props.get(&predicate.key)))
            });
        }
        Ok(nodes)
    }
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

/// Whether a backend error is a retryable MVCC conflict (write-write conflict,
/// busy / busy-snapshot, or a generic conflict) — the engine aborts the
/// `BEGIN CONCURRENT` transaction in these cases and the write can be retried.
fn is_mvcc_conflict(err: &GrustError) -> bool {
    let msg = err.to_string().to_ascii_lowercase();
    msg.contains("conflict") || msg.contains("busy") || msg.contains("snapshot")
}

// ---------------------------------------------------------------------------
// Portable read pushdown (PUSHDOWN2 follow-on: Turso joins the consumers)
// ---------------------------------------------------------------------------

/// The pushdown [`SqlDialect`] for Turso's universal tables: **tagged-JSON**
/// props (each property is stored as `{"type": t, "value": v}`, so scalar
/// extraction is `$.key.value`), `from_id`/`to_id`/`label` edge columns, and
/// no recursive CTEs or JSON table functions in the embedded engine — those
/// leaves report unsupported and the store falls back to the reference.
#[derive(Clone, Debug)]
pub struct TursoReadDialect {
    nodes: String,
    edges: String,
}

impl TursoReadDialect {
    pub fn new(table_prefix: &str) -> Self {
        Self {
            nodes: format!("{table_prefix}_nodes"),
            edges: format!("{table_prefix}_edges"),
        }
    }
}

impl SqlDialect for TursoReadDialect {
    fn nodes_table(&self) -> &str {
        &self.nodes
    }
    fn edges_table(&self) -> &str {
        &self.edges
    }
    fn quote_ident(&self, ident: &str) -> String {
        quote_ident(ident)
    }
    fn json_property(&self, props_column: &str, key: &str) -> String {
        // Tagged storage: extract the scalar payload under the type tag.
        format!("json_extract({props_column}, '$.{key}.value')")
    }
    fn cast_int(&self, expr: &str) -> String {
        format!("CAST({expr} AS INTEGER)")
    }
    fn cast_float(&self, expr: &str) -> String {
        format!("CAST({expr} AS REAL)")
    }
    fn string_literal(&self, value: &str) -> String {
        sql_str(value)
    }
    fn string_predicate(&self, expr: &str, op: StrOp, needle: &str) -> String {
        // Mirrors `SqliteDialect`: literal (non-LIKE) matching, NULL-propagating.
        let n = self.string_literal(needle);
        match op {
            StrOp::StartsWith => format!("instr({expr}, {n}) = 1"),
            StrOp::Contains => format!("instr({expr}, {n}) > 0"),
            StrOp::EndsWith => format!("substr({expr}, -{}) = {n}", needle.chars().count()),
        }
    }
    fn bool_literal_sql(&self, value: bool) -> String {
        // json_extract returns the tagged JSON boolean as integer 1/0.
        if value {
            "1".to_string()
        } else {
            "0".to_string()
        }
    }
    fn orders_json_typed(&self) -> bool {
        // json_extract of the tagged `.value` yields INTEGER/REAL/TEXT.
        true
    }
    fn recursive_cte_supported(&self) -> bool {
        // The embedded turso (limbo) engine does not execute WITH RECURSIVE.
        false
    }
    fn edge_src_col(&self) -> &str {
        "from_id"
    }
    fn edge_dst_col(&self) -> &str {
        "to_id"
    }
    fn edge_type_col(&self) -> &str {
        "label"
    }
}

impl TursoGraphStore {
    fn read_dialect(&self) -> TursoReadDialect {
        TursoReadDialect::new(&self.config.table_prefix)
    }

    /// Materialize the full graph — the reference-executor fallback input.
    pub async fn read_graph(&self) -> Result<Graph> {
        let nodes = self
            .query_nodes(&format!(
                "SELECT id, label, props FROM {}",
                self.nodes_table()
            ))
            .await?;
        let edges = self
            .query_edges(&format!(
                "SELECT id, from_id, to_id, label, props FROM {}",
                self.edges_table()
            ))
            .await?;
        Ok(Graph::new(nodes, edges))
    }

    /// Execute pushdown SQL whose result cells decode as optional text.
    async fn run_text_rows(&self, sql: &str, columns: usize) -> Result<Vec<Vec<Option<String>>>> {
        let _gate = self.connection_gate.lock().await;
        let mut rows = self.conn.query(sql, ()).await.map_err(|err| {
            GrustError::Backend(format!("Turso read pushdown query failed: {err}: {sql}"))
        })?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(|err| {
            GrustError::Backend(format!("Turso read pushdown row failed: {err}: {sql}"))
        })? {
            let mut cells = Vec::with_capacity(columns);
            for i in 0..columns {
                let cell = match row.get_value(i).map_err(|err| {
                    GrustError::Backend(format!("Turso read pushdown cell failed: {err}"))
                })? {
                    turso::Value::Null => None,
                    turso::Value::Text(v) => Some(v),
                    turso::Value::Integer(v) => Some(v.to_string()),
                    turso::Value::Real(v) => Some(v.to_string()),
                    turso::Value::Blob(_) => None,
                };
                cells.push(cell);
            }
            out.push(cells);
        }
        Ok(out)
    }

    /// Portable read entrypoint: the query's `MATCH`/`WHERE`/row-source part
    /// is pushed into SQL over the universal tables where the plan supports
    /// this dialect (node scans, fixed segments, `OPTIONAL MATCH`,
    /// multi-pattern, `UNION`, `WITH` pipelines, subqueries, and the
    /// non-recursive catalog procedures); everything else falls back to the
    /// Memory reference over [`Self::read_graph`]. Results are identical to
    /// [`grust_cypher::read::run_read_query`] by construction.
    pub async fn run_read_query(
        &self,
        cypher: &str,
        params: &CypherParameters,
    ) -> Result<CypherResultTable> {
        let dialect = self.read_dialect();
        if let Some(plan) = plan_read(cypher, params, &NoTypeHints)?
            && plan.supported_by(&dialect)
        {
            if let Some((arms, distinct)) = plan.union_arms() {
                let mut tables = Vec::with_capacity(arms.len());
                for arm in arms {
                    let rows = self
                        .run_text_rows(&arm.to_sql(&dialect), arm.column_count())
                        .await?;
                    tables.push(arm.project_text_rows(&dialect, rows, params)?);
                }
                return combine_union(tables, distinct);
            }
            let rows = self
                .run_text_rows(&plan.to_sql(&dialect), plan.column_count())
                .await?;
            return plan.project_text_rows(&dialect, rows, params);
        }
        let graph = self.read_graph().await?;
        grust_cypher::read::run_read_query(&graph, cypher, params)
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
