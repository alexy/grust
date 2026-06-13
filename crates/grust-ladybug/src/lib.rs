use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

#[cfg(feature = "arrow")]
use std::io::Cursor;

#[cfg(feature = "arrow")]
use arrow::{
    ipc::{reader::StreamReader as ArrowStreamReader, writer::StreamWriter as ArrowStreamWriter},
    record_batch::RecordBatch as ArrowRecordBatch,
};
use async_trait::async_trait;
use grust_core::prelude::*;

#[derive(Clone, Debug)]
pub struct LadybugConfig {
    pub path: LadybugPath,
    pub table_prefix: String,
    /// Whether Grust may create Ladybug node/relationship tables from graph
    /// labels on write. `true` is the untyped/schema-later Grust mode; `false`
    /// is the strict typed mode where `apply_schema` must run first.
    pub dynamic_schema: bool,
    pub query_timeout_ms: Option<u64>,
}

impl Default for LadybugConfig {
    fn default() -> Self {
        Self {
            path: LadybugPath::InMemory,
            table_prefix: "grust".to_string(),
            dynamic_schema: true,
            query_timeout_ms: None,
        }
    }
}

impl LadybugConfig {
    pub fn untyped() -> Self {
        Self::default().with_graph_mode(LadybugGraphMode::Untyped)
    }

    pub fn typed() -> Self {
        Self::default().with_graph_mode(LadybugGraphMode::Typed)
    }

    pub fn graph_mode(&self) -> LadybugGraphMode {
        if self.dynamic_schema {
            LadybugGraphMode::Untyped
        } else {
            LadybugGraphMode::Typed
        }
    }

    pub fn with_graph_mode(mut self, mode: LadybugGraphMode) -> Self {
        self.dynamic_schema = matches!(mode, LadybugGraphMode::Untyped);
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LadybugPath {
    InMemory,
    Directory(PathBuf),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LadybugGraphMode {
    /// Accept ordinary Grust graphs without a predeclared `GraphSchema`.
    ///
    /// The backend creates the Ladybug node and relationship tables it needs
    /// from the graph's labels and endpoint labels.
    Untyped,
    /// Require `apply_schema` or `put_typed_graph` before writes.
    ///
    /// Writes are validated against the applied `GraphSchema`, and the backend
    /// does not create undeclared node or relationship tables during writes.
    Typed,
}

#[derive(Debug)]
pub struct LadybugGraphStore {
    config: LadybugConfig,
    db: Arc<lbug::Database>,
    lock: Mutex<()>,
    schema: RwLock<Option<GraphSchema>>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct NodeTable {
    label: Label,
    table: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RelTable {
    label: Label,
    from_label: Label,
    to_label: Label,
    table: String,
}

impl LadybugGraphStore {
    pub fn new(config: LadybugConfig) -> Result<Self> {
        let db = match &config.path {
            LadybugPath::InMemory => lbug::Database::in_memory(lbug::SystemConfig::default()),
            LadybugPath::Directory(path) => {
                lbug::Database::new(path, lbug::SystemConfig::default())
            }
        }
        .map_err(ladybug_error)?;
        Ok(Self {
            config,
            db: Arc::new(db),
            lock: Mutex::new(()),
            schema: RwLock::new(None),
        })
    }

    pub fn in_memory() -> Result<Self> {
        Self::new(LadybugConfig::default())
    }

    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        Self::new(LadybugConfig {
            path: LadybugPath::Directory(path.into()),
            ..LadybugConfig::default()
        })
    }

    /// Registers an Arrow IPC stream as a Ladybug node table.
    ///
    /// The stream is decoded into the Arrow version used by `lbug`, and the
    /// first column in the Arrow schema becomes Ladybug's primary key column.
    #[cfg(feature = "arrow")]
    pub fn register_arrow_ipc_node_table(&self, table_name: &str, ipc_stream: &[u8]) -> Result<()> {
        let batches = arrow_ipc_batches(ipc_stream)?;
        self.with_conn(|conn| {
            conn.create_arrow_table(table_name, &batches)
                .map(drop)
                .map_err(ladybug_error)
        })
    }

    /// Registers an Arrow IPC stream as a Ladybug relationship table.
    ///
    /// The relationship stream must include endpoint columns named `from` and
    /// `to`. The source and destination table names refer to Ladybug node
    /// tables registered separately.
    #[cfg(feature = "arrow")]
    pub fn register_arrow_ipc_rel_table(
        &self,
        table_name: &str,
        ipc_stream: &[u8],
        src_table_name: &str,
        dst_table_name: &str,
    ) -> Result<()> {
        let batches = arrow_ipc_batches(ipc_stream)?;
        self.with_conn(|conn| {
            conn.create_arrow_rel_table(table_name, &batches, src_table_name, dst_table_name)
                .map(drop)
                .map_err(ladybug_error)
        })
    }

    /// Registers CSR-shaped Arrow IPC streams as a Ladybug relationship table.
    ///
    /// `indices_ipc` must contain a destination column named by
    /// `dst_col_name`, while `indptr_ipc` contains the source offsets.
    #[cfg(feature = "arrow")]
    pub fn register_arrow_ipc_rel_table_csr(
        &self,
        table_name: &str,
        indices_ipc: &[u8],
        indptr_ipc: &[u8],
        src_table_name: &str,
        dst_table_name: &str,
        dst_col_name: &str,
    ) -> Result<()> {
        let indices_batches = arrow_ipc_batches(indices_ipc)?;
        let indptr_batches = arrow_ipc_batches(indptr_ipc)?;
        self.with_conn(|conn| {
            conn.create_arrow_rel_table_csr(
                table_name,
                &indices_batches,
                &indptr_batches,
                src_table_name,
                dst_table_name,
                dst_col_name,
            )
            .map(drop)
            .map_err(ladybug_error)
        })
    }

    /// Drops a Ladybug Arrow table registered through the embedded `lbug` API.
    #[cfg(feature = "arrow")]
    pub fn drop_arrow_table(&self, table_name: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.drop_arrow_table(table_name)
                .map(drop)
                .map_err(ladybug_error)
        })
    }

    /// Executes a Ladybug query and returns result chunks as Arrow IPC streams.
    ///
    /// Each returned item is a complete Arrow IPC stream for one result batch,
    /// so callers can decode the chunks with any compatible Arrow runtime.
    #[cfg(feature = "arrow")]
    pub fn query_arrow_ipc(&self, query: &str, chunk_size: usize) -> Result<Vec<Vec<u8>>> {
        let chunk_size = chunk_size.max(1);
        self.with_conn(|conn| {
            let mut result = conn
                .query_as_arrow(query, chunk_size)
                .map_err(ladybug_error)?;
            let mut chunks = Vec::new();
            for batch in result.iter_arrow(chunk_size).map_err(ladybug_error)? {
                chunks.push(arrow_batch_to_ipc(&batch)?);
            }
            Ok(chunks)
        })
    }

    fn with_conn<T>(&self, f: impl FnOnce(&lbug::Connection<'_>) -> Result<T>) -> Result<T> {
        let _guard = self.lock.lock().expect("ladybug store lock poisoned");
        let conn = lbug::Connection::new(&self.db).map_err(ladybug_error)?;
        if let Some(timeout_ms) = self.config.query_timeout_ms {
            conn.set_query_timeout(timeout_ms);
        }
        f(&conn)
    }

    fn current_schema(&self) -> Option<GraphSchema> {
        self.schema
            .read()
            .expect("Ladybug schema lock poisoned")
            .clone()
    }

    fn write_schema(&self, schema: &GraphSchema) {
        *self.schema.write().expect("Ladybug schema lock poisoned") = Some(schema.clone());
    }

    fn require_schema_for_typed_mode(&self) -> Result<Option<GraphSchema>> {
        let schema = self.current_schema();
        if schema.is_none() && self.config.graph_mode() == LadybugGraphMode::Typed {
            return Err(GrustError::Schema(
                "Ladybug typed mode requires apply_schema before writes".to_string(),
            ));
        }
        Ok(schema)
    }

    fn validate_edge_with_applied_schema(
        &self,
        conn: &lbug::Connection<'_>,
        schema: &GraphSchema,
        edge: &Edge,
    ) -> Result<()> {
        let from = self.node_table_for_id(conn, &edge.from)?.ok_or_else(|| {
            GrustError::Schema(format!(
                "edge '{}' references unknown from node '{}'",
                edge.label.as_str(),
                edge.from.as_str()
            ))
        })?;
        let to = self.node_table_for_id(conn, &edge.to)?.ok_or_else(|| {
            GrustError::Schema(format!(
                "edge '{}' references unknown to node '{}'",
                edge.label.as_str(),
                edge.to.as_str()
            ))
        })?;
        schema.validate_edge_with(edge, |id| {
            if id == &edge.from {
                Some(&from.label)
            } else if id == &edge.to {
                Some(&to.label)
            } else {
                None
            }
        })
    }

    fn exec(conn: &lbug::Connection<'_>, query: &str) -> Result<()> {
        conn.query(query).map(drop).map_err(ladybug_error)
    }

    fn exec_ignore_exists(conn: &lbug::Connection<'_>, query: &str) -> Result<()> {
        match Self::exec(conn, query) {
            Ok(()) => Ok(()),
            Err(GrustError::Backend(message)) if is_exists_error(&message) => Ok(()),
            Err(err) => Err(err),
        }
    }

    fn exec_ignore_missing(conn: &lbug::Connection<'_>, query: &str) -> Result<()> {
        match Self::exec(conn, query) {
            Ok(()) => Ok(()),
            Err(GrustError::Backend(message)) if is_missing_error(&message) => Ok(()),
            Err(err) => Err(err),
        }
    }

    fn execute(
        conn: &lbug::Connection<'_>,
        query: &str,
        params: Vec<(&str, lbug::Value)>,
    ) -> Result<()> {
        let mut statement = conn.prepare(query).map_err(ladybug_error)?;
        conn.execute(&mut statement, params)
            .map(drop)
            .map_err(ladybug_error)
    }

    fn metadata_tables(&self) -> Result<(String, String)> {
        let prefix = schema_identifier(&self.config.table_prefix)?;
        Ok((
            format!("{prefix}_node_index"),
            format!("{prefix}_rel_index"),
        ))
    }

    fn bootstrap_locked(&self, conn: &lbug::Connection<'_>) -> Result<()> {
        let (node_index, rel_index) = self.metadata_tables()?;
        Self::exec_ignore_exists(
            conn,
            &format!(
                "CREATE NODE TABLE {node_index}(id STRING, kind STRING, label STRING, table_name STRING, PRIMARY KEY(id));"
            ),
        )?;
        Self::exec_ignore_exists(
            conn,
            &format!(
                "CREATE NODE TABLE {rel_index}(id STRING, edge_label STRING, from_label STRING, to_label STRING, table_name STRING, PRIMARY KEY(id));"
            ),
        )
    }

    fn node_table_name(&self, label: &Label) -> Result<String> {
        let prefix = schema_identifier(&self.config.table_prefix)?;
        let label = schema_identifier(label.as_str())?;
        Ok(format!("{prefix}_node_{label}"))
    }

    fn rel_table_name(
        &self,
        label: &Label,
        from_label: &Label,
        to_label: &Label,
    ) -> Result<String> {
        let prefix = schema_identifier(&self.config.table_prefix)?;
        let label = schema_identifier(label.as_str())?;
        let from_label = schema_identifier(from_label.as_str())?;
        let to_label = schema_identifier(to_label.as_str())?;
        Ok(format!("{prefix}_rel_{label}_{from_label}_{to_label}"))
    }

    fn ensure_node_table(&self, conn: &lbug::Connection<'_>, label: &Label) -> Result<NodeTable> {
        let table = self.node_table_name(label)?;
        Self::exec_ignore_exists(
            conn,
            &format!("CREATE NODE TABLE {table}(id STRING, props STRING, PRIMARY KEY(id));"),
        )?;
        self.write_node_table_index(conn, label, &table)?;
        Ok(NodeTable {
            label: label.clone(),
            table,
        })
    }

    fn ensure_rel_table(
        &self,
        conn: &lbug::Connection<'_>,
        label: &Label,
        from_label: &Label,
        to_label: &Label,
    ) -> Result<RelTable> {
        let table = self.rel_table_name(label, from_label, to_label)?;
        let from_table = self.ensure_node_table(conn, from_label)?.table;
        let to_table = self.ensure_node_table(conn, to_label)?.table;
        Self::exec_ignore_exists(
            conn,
            &format!(
                "CREATE REL TABLE {table}(FROM {from_table} TO {to_table}, id STRING, props STRING);"
            ),
        )?;
        self.write_rel_index(conn, label, from_label, to_label, &table)?;
        Ok(RelTable {
            label: label.clone(),
            from_label: from_label.clone(),
            to_label: to_label.clone(),
            table,
        })
    }

    fn write_node_index(
        &self,
        conn: &lbug::Connection<'_>,
        node: &Node,
        table: &str,
    ) -> Result<()> {
        let (node_index, _) = self.metadata_tables()?;
        Self::execute(
            conn,
            &format!(
                "MERGE (n:{node_index} {{id: $id}}) SET n.kind = 'node', n.label = $label, n.table_name = $table_name;"
            ),
            vec![
                ("id", lbug::Value::String(node.id.as_str().to_string())),
                (
                    "label",
                    lbug::Value::String(node.label.as_str().to_string()),
                ),
                ("table_name", lbug::Value::String(table.to_string())),
            ],
        )
    }

    fn write_node_table_index(
        &self,
        conn: &lbug::Connection<'_>,
        label: &Label,
        table: &str,
    ) -> Result<()> {
        let (node_index, _) = self.metadata_tables()?;
        Self::execute(
            conn,
            &format!(
                "MERGE (n:{node_index} {{id: $id}}) SET n.kind = 'table', n.label = $label, n.table_name = $table_name;"
            ),
            vec![
                (
                    "id",
                    lbug::Value::String(format!("table\u{1f}{}", label.as_str())),
                ),
                ("label", lbug::Value::String(label.as_str().to_string())),
                ("table_name", lbug::Value::String(table.to_string())),
            ],
        )
    }

    fn write_rel_index(
        &self,
        conn: &lbug::Connection<'_>,
        label: &Label,
        from_label: &Label,
        to_label: &Label,
        table: &str,
    ) -> Result<()> {
        let (_, rel_index) = self.metadata_tables()?;
        let id = rel_index_id(label, from_label, to_label);
        Self::execute(
            conn,
            &format!(
                "MERGE (r:{rel_index} {{id: $id}}) SET r.edge_label = $edge_label, r.from_label = $from_label, r.to_label = $to_label, r.table_name = $table_name;"
            ),
            vec![
                ("id", lbug::Value::String(id)),
                (
                    "edge_label",
                    lbug::Value::String(label.as_str().to_string()),
                ),
                (
                    "from_label",
                    lbug::Value::String(from_label.as_str().to_string()),
                ),
                (
                    "to_label",
                    lbug::Value::String(to_label.as_str().to_string()),
                ),
                ("table_name", lbug::Value::String(table.to_string())),
            ],
        )
    }

    fn node_table_for_id(
        &self,
        conn: &lbug::Connection<'_>,
        id: &NodeId,
    ) -> Result<Option<NodeTable>> {
        let (node_index, _) = self.metadata_tables()?;
        let mut statement = conn
            .prepare(&format!(
                "MATCH (n:{node_index}) WHERE n.kind = 'node' AND n.id = $id RETURN n.label, n.table_name LIMIT 1;"
            ))
            .map_err(ladybug_error)?;
        let mut rows = conn
            .execute(
                &mut statement,
                vec![("id", lbug::Value::String(id.as_str().to_string()))],
            )
            .map_err(ladybug_error)?;
        rows.next().map(row_to_node_table).transpose()
    }

    fn node_tables(&self, conn: &lbug::Connection<'_>) -> Result<Vec<NodeTable>> {
        let (node_index, _) = self.metadata_tables()?;
        let rows = conn
            .query(&format!(
                "MATCH (n:{node_index}) WHERE n.kind = 'table' RETURN n.label, n.table_name ORDER BY n.label;"
            ))
            .map_err(ladybug_error)?;
        rows.map(row_to_node_table).collect()
    }

    fn rel_tables(&self, conn: &lbug::Connection<'_>) -> Result<Vec<RelTable>> {
        let (_, rel_index) = self.metadata_tables()?;
        let rows = conn
            .query(&format!(
                "MATCH (r:{rel_index}) RETURN r.edge_label, r.from_label, r.to_label, r.table_name ORDER BY r.id;"
            ))
            .map_err(ladybug_error)?;
        rows.map(row_to_rel_table).collect()
    }

    fn put_node_locked(&self, conn: &lbug::Connection<'_>, node: &Node) -> Result<PutOutcome> {
        self.bootstrap_locked(conn)?;
        let table = if self.config.dynamic_schema {
            self.ensure_node_table(conn, &node.label)?
        } else {
            NodeTable {
                label: node.label.clone(),
                table: self.node_table_name(&node.label)?,
            }
        };
        self.write_node_locked(conn, node, &table.table)?;
        Ok(PutOutcome::Upserted)
    }

    fn write_node_locked(
        &self,
        conn: &lbug::Connection<'_>,
        node: &Node,
        table: &str,
    ) -> Result<()> {
        let props = props_to_string(&node.props)?;
        Self::execute(
            conn,
            &format!("MERGE (n:{table} {{id: $id}}) SET n.props = $props;"),
            vec![
                ("id", lbug::Value::String(node.id.as_str().to_string())),
                ("props", lbug::Value::String(props)),
            ],
        )?;
        self.write_node_index(conn, node, table)
    }

    fn put_edge_locked(
        &self,
        conn: &lbug::Connection<'_>,
        edge: &Edge,
        labels: Option<(&Label, &Label)>,
    ) -> Result<PutOutcome> {
        self.bootstrap_locked(conn)?;
        let (from_label, to_label) = match labels {
            Some(labels) => labels,
            None => {
                let from = self.node_table_for_id(conn, &edge.from)?.ok_or_else(|| {
                    GrustError::Schema(format!(
                        "Ladybug edge '{}' references unknown from node '{}'",
                        edge.label.as_str(),
                        edge.from.as_str()
                    ))
                })?;
                let to = self.node_table_for_id(conn, &edge.to)?.ok_or_else(|| {
                    GrustError::Schema(format!(
                        "Ladybug edge '{}' references unknown to node '{}'",
                        edge.label.as_str(),
                        edge.to.as_str()
                    ))
                })?;
                return self.put_edge_locked(conn, edge, Some((&from.label, &to.label)));
            }
        };
        let rel = if self.config.dynamic_schema {
            self.ensure_rel_table(conn, &edge.label, from_label, to_label)?
        } else {
            RelTable {
                label: edge.label.clone(),
                from_label: from_label.clone(),
                to_label: to_label.clone(),
                table: self.rel_table_name(&edge.label, from_label, to_label)?,
            }
        };
        let from_table = self.node_table_name(from_label)?;
        let to_table = self.node_table_name(to_label)?;
        self.write_edge_locked(conn, edge, &rel.table, &from_table, &to_table)?;
        Ok(PutOutcome::Upserted)
    }

    fn write_edge_locked(
        &self,
        conn: &lbug::Connection<'_>,
        edge: &Edge,
        rel_table: &str,
        from_table: &str,
        to_table: &str,
    ) -> Result<()> {
        let id = edge_key(edge);
        let props = props_to_string(&edge.props)?;
        Self::execute(
            conn,
            &format!(
                "MATCH (a:{from_table}), (b:{to_table}) WHERE a.id = $from_id AND b.id = $to_id MERGE (a)-[r:{}]->(b) SET r.id = $id, r.props = $props;",
                rel_table
            ),
            vec![
                (
                    "from_id",
                    lbug::Value::String(edge.from.as_str().to_string()),
                ),
                ("to_id", lbug::Value::String(edge.to.as_str().to_string())),
                ("id", lbug::Value::String(id)),
                ("props", lbug::Value::String(props)),
            ],
        )?;
        Ok(())
    }

    fn get_node_from_table(
        &self,
        conn: &lbug::Connection<'_>,
        table: &NodeTable,
        id: &NodeId,
    ) -> Result<Option<Node>> {
        let mut statement = conn
            .prepare(&format!(
                "MATCH (n:{}) WHERE n.id = $id RETURN n.id, n.props LIMIT 1;",
                table.table
            ))
            .map_err(ladybug_error)?;
        let mut rows = conn
            .execute(
                &mut statement,
                vec![("id", lbug::Value::String(id.as_str().to_string()))],
            )
            .map_err(ladybug_error)?;
        rows.next()
            .map(|row| row_to_node(row, &table.label))
            .transpose()
    }

    fn get_edges_from_table(
        &self,
        conn: &lbug::Connection<'_>,
        table: &RelTable,
        query: &EdgeQuery,
    ) -> Result<Vec<Edge>> {
        let from_table = self.node_table_name(&table.from_label)?;
        let to_table = self.node_table_name(&table.to_label)?;
        let mut where_parts = Vec::new();
        let mut params = Vec::new();
        if let Some(from) = &query.from {
            where_parts.push("a.id = $from_id");
            params.push(("from_id", lbug::Value::String(from.as_str().to_string())));
        }
        if let Some(to) = &query.to {
            where_parts.push("b.id = $to_id");
            params.push(("to_id", lbug::Value::String(to.as_str().to_string())));
        }
        let where_clause = if where_parts.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", where_parts.join(" AND "))
        };
        let cypher = format!(
            "MATCH (a:{from_table})-[r:{}]->(b:{to_table}){where_clause} RETURN a.id, b.id, r.id, r.props;",
            table.table
        );
        let mut statement = conn.prepare(&cypher).map_err(ladybug_error)?;
        let rows = conn
            .execute(&mut statement, params)
            .map_err(ladybug_error)?;
        rows.map(|row| row_to_edge(row, &table.label)).collect()
    }

    fn get_edges_locked(&self, conn: &lbug::Connection<'_>, query: EdgeQuery) -> Result<Vec<Edge>> {
        self.bootstrap_locked(conn)?;
        let tables = self
            .rel_tables(conn)?
            .into_iter()
            .filter(|table| {
                query
                    .label
                    .as_ref()
                    .is_none_or(|label| label == &table.label)
            })
            .collect::<Vec<_>>();
        let mut edges = Vec::new();
        for table in tables {
            edges.extend(self.get_edges_from_table(conn, &table, &query)?);
        }
        Ok(edges)
    }

    fn start_nodes(&self, conn: &lbug::Connection<'_>, start: Start) -> Result<Vec<Node>> {
        match start {
            Start::Node(id) => Ok(self.get_node_locked(conn, &id)?.into_iter().collect()),
            Start::NodesByLabel(label) => {
                let table = NodeTable {
                    table: self.node_table_name(&label)?,
                    label,
                };
                self.get_nodes_from_label_table(conn, &table, None)
            }
            Start::NodesByProperty { label, key, value } => {
                let table = NodeTable {
                    table: self.node_table_name(&label)?,
                    label,
                };
                let expected = value;
                Ok(self
                    .get_nodes_from_label_table(conn, &table, None)?
                    .into_iter()
                    .filter(|node| node.props.get(&key) == Some(&expected))
                    .collect())
            }
        }
    }

    fn get_nodes_from_label_table(
        &self,
        conn: &lbug::Connection<'_>,
        table: &NodeTable,
        limit: Option<u32>,
    ) -> Result<Vec<Node>> {
        let limit_clause = limit
            .map(|limit| format!(" LIMIT {limit}"))
            .unwrap_or_default();
        let rows = conn
            .query(&format!(
                "MATCH (n:{}) RETURN n.id, n.props ORDER BY n.id{limit_clause};",
                table.table
            ))
            .map_err(ladybug_error)?;
        rows.map(|row| row_to_node(row, &table.label)).collect()
    }

    fn get_node_locked(&self, conn: &lbug::Connection<'_>, id: &NodeId) -> Result<Option<Node>> {
        self.bootstrap_locked(conn)?;
        if let Some(table) = self.node_table_for_id(conn, id)? {
            return self.get_node_from_table(conn, &table, id);
        }
        for table in self.node_tables(conn)? {
            if let Some(node) = self.get_node_from_table(conn, &table, id)? {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }
}

#[async_trait]
impl GraphStore for LadybugGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        self.with_conn(|conn| {
            self.bootstrap_locked(conn)?;
            for node in &schema.nodes {
                self.ensure_node_table(conn, &node.label)?;
            }
            for edge in &schema.edges {
                for from in &edge.from {
                    for to in &edge.to {
                        self.ensure_rel_table(conn, &edge.label, from, to)?;
                    }
                }
            }
            Ok(())
        })?;
        self.write_schema(schema);
        Ok(())
    }

    async fn put_node(&self, node: &Node) -> Result<PutOutcome> {
        if let Some(schema) = self.require_schema_for_typed_mode()? {
            schema.validate_node(node)?;
        }
        self.with_conn(|conn| self.put_node_locked(conn, node))
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        let schema = self.require_schema_for_typed_mode()?;
        self.with_conn(|conn| {
            if let Some(schema) = schema.as_ref() {
                self.validate_edge_with_applied_schema(conn, schema, edge)?;
            }
            self.put_edge_locked(conn, edge, None)
        })
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        if let Some(schema) = self.require_schema_for_typed_mode()? {
            schema.validate_graph(graph)?;
        }
        self.with_conn(|conn| {
            self.bootstrap_locked(conn)?;
            let labels = graph
                .nodes
                .iter()
                .map(|node| (node.id.clone(), node.label.clone()))
                .collect::<BTreeMap<_, _>>();
            let mut node_tables = BTreeMap::new();
            for node in &graph.nodes {
                let table = if self.config.dynamic_schema {
                    self.ensure_node_table(conn, &node.label)?.table
                } else {
                    self.node_table_name(&node.label)?
                };
                node_tables.insert(node.id.clone(), table);
            }
            let mut edge_tables = BTreeMap::new();
            for edge in &graph.edges {
                let from_label = labels.get(&edge.from).ok_or_else(|| {
                    GrustError::Schema(format!(
                        "Ladybug edge '{}' references unknown from node '{}'",
                        edge.label.as_str(),
                        edge.from.as_str()
                    ))
                })?;
                let to_label = labels.get(&edge.to).ok_or_else(|| {
                    GrustError::Schema(format!(
                        "Ladybug edge '{}' references unknown to node '{}'",
                        edge.label.as_str(),
                        edge.to.as_str()
                    ))
                })?;
                let rel_table = if self.config.dynamic_schema {
                    self.ensure_rel_table(conn, &edge.label, from_label, to_label)?
                        .table
                } else {
                    self.rel_table_name(&edge.label, from_label, to_label)?
                };
                let from_table = self.node_table_name(from_label)?;
                let to_table = self.node_table_name(to_label)?;
                edge_tables.insert(edge_key(edge), (rel_table, from_table, to_table));
            }
            Self::exec(conn, "BEGIN TRANSACTION;")?;
            let result = (|| {
                let mut report = LoadReport::default();
                for node in &graph.nodes {
                    let table = node_tables.get(&node.id).ok_or_else(|| {
                        GrustError::Schema(format!(
                            "Ladybug node table missing for '{}'",
                            node.id.as_str()
                        ))
                    })?;
                    self.write_node_locked(conn, node, table)?;
                    report.nodes += 1;
                }
                for edge in &graph.edges {
                    let edge_key = edge_key(edge);
                    let (rel_table, from_table, to_table) =
                        edge_tables.get(&edge_key).ok_or_else(|| {
                            GrustError::Schema(format!(
                                "Ladybug relationship table missing for edge '{}'",
                                edge_key
                            ))
                        })?;
                    self.write_edge_locked(conn, edge, rel_table, from_table, to_table)?;
                    report.edges += 1;
                }
                Ok(report)
            })();
            match result {
                Ok(report) => {
                    Self::exec(conn, "COMMIT;")?;
                    Ok(report)
                }
                Err(err) => {
                    let _ = Self::exec(conn, "ROLLBACK;");
                    Err(err)
                }
            }
        })
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        self.with_conn(|conn| self.get_node_locked(conn, id))
    }

    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>> {
        self.with_conn(|conn| {
            let mut nodes = Vec::new();
            for id in ids {
                if let Some(node) = self.get_node_locked(conn, id)? {
                    nodes.push(node);
                }
            }
            Ok(nodes)
        })
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        self.with_conn(|conn| self.get_edges_locked(conn, query))
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        self.with_conn(|conn| {
            self.bootstrap_locked(conn)?;
            let mut current = self.start_nodes(conn, traversal.start)?;
            for step in traversal.steps {
                let mut next_by_id = BTreeMap::<NodeId, Node>::new();
                for node in &current {
                    let mut out_query = EdgeQuery {
                        from: Some(node.id.clone()),
                        to: None,
                        label: step.edge.clone(),
                    };
                    let mut in_query = EdgeQuery {
                        from: None,
                        to: Some(node.id.clone()),
                        label: step.edge.clone(),
                    };
                    let edge_sets = match step.direction {
                        Direction::Out => {
                            vec![self.get_edges_locked(conn, shift_query(&mut out_query))?]
                        }
                        Direction::In => {
                            vec![self.get_edges_locked(conn, shift_query(&mut in_query))?]
                        }
                        Direction::Both => vec![
                            self.get_edges_locked(conn, shift_query(&mut out_query))?,
                            self.get_edges_locked(conn, shift_query(&mut in_query))?,
                        ],
                    };
                    for edge in edge_sets.into_iter().flatten() {
                        let target_id = if edge.from == node.id {
                            &edge.to
                        } else {
                            &edge.from
                        };
                        if let Some(target) = self.get_node_locked(conn, target_id)?
                            && step
                                .node
                                .as_ref()
                                .is_none_or(|label| label == &target.label)
                        {
                            next_by_id.insert(target.id.clone(), target);
                        }
                    }
                }
                current = next_by_id.into_values().collect();
            }
            if let Some(limit) = traversal.limit {
                current.truncate(limit as usize);
            }
            Ok(current)
        })
    }
}

#[async_trait]
impl GraphAdminStore for LadybugGraphStore {
    async fn bootstrap(&self) -> Result<()> {
        self.with_conn(|conn| self.bootstrap_locked(conn))
    }

    async fn clear(&self) -> Result<()> {
        self.with_conn(|conn| {
            self.bootstrap_locked(conn)?;
            let rel_tables = self.rel_tables(conn)?;
            let node_tables = self.node_tables(conn)?;
            for table in rel_tables.into_iter().map(|table| table.table) {
                Self::exec_ignore_missing(conn, &format!("DROP TABLE {table};"))?;
            }
            for table in node_tables.into_iter().map(|table| table.table) {
                Self::exec_ignore_missing(conn, &format!("DROP TABLE {table};"))?;
            }
            let (node_index, rel_index) = self.metadata_tables()?;
            Self::exec_ignore_missing(conn, &format!("DROP TABLE {rel_index};"))?;
            Self::exec_ignore_missing(conn, &format!("DROP TABLE {node_index};"))?;
            self.bootstrap_locked(conn)?;
            if let Some(schema) = self.current_schema() {
                for node in &schema.nodes {
                    self.ensure_node_table(conn, &node.label)?;
                }
                for edge in &schema.edges {
                    for from in &edge.from {
                        for to in &edge.to {
                            self.ensure_rel_table(conn, &edge.label, from, to)?;
                        }
                    }
                }
            }
            Ok(())
        })
    }
}

#[cfg(feature = "arrow")]
fn arrow_ipc_batches(ipc_stream: &[u8]) -> Result<Vec<ArrowRecordBatch>> {
    let reader = ArrowStreamReader::try_new(Cursor::new(ipc_stream), None)
        .map_err(|err| GrustError::Backend(format!("Arrow IPC read failed: {err}")))?;
    reader
        .map(|batch| {
            batch.map_err(|err| GrustError::Backend(format!("Arrow batch read failed: {err}")))
        })
        .collect()
}

#[cfg(feature = "arrow")]
fn arrow_batch_to_ipc(batch: &ArrowRecordBatch) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    {
        let cursor = Cursor::new(&mut data);
        let mut writer = ArrowStreamWriter::try_new(cursor, batch.schema().as_ref())
            .map_err(|err| GrustError::Backend(format!("Arrow IPC write failed: {err}")))?;
        writer
            .write(batch)
            .map_err(|err| GrustError::Backend(format!("Arrow IPC write failed: {err}")))?;
        writer
            .finish()
            .map_err(|err| GrustError::Backend(format!("Arrow IPC write failed: {err}")))?;
    }
    Ok(data)
}

fn shift_query(query: &mut EdgeQuery) -> EdgeQuery {
    std::mem::take(query)
}

fn props_to_string(props: &Props) -> Result<String> {
    serde_json::to_string(props)
        .map_err(|err| GrustError::Serialization(format!("Ladybug props encode error: {err}")))
}

fn props_from_string(value: &str) -> Result<Props> {
    serde_json::from_str(value)
        .map_err(|err| GrustError::Serialization(format!("Ladybug props decode error: {err}")))
}

fn ladybug_error(err: lbug::Error) -> GrustError {
    GrustError::Backend(format!("LadybugDB error: {err}"))
}

fn is_exists_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("already exists") || message.contains("conflict")
}

fn is_missing_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("does not exist")
        || message.contains("not found")
        || message.contains("cannot find")
        || message.contains("not in catalog")
}

fn row_string(row: &[lbug::Value], index: usize, context: &str) -> Result<String> {
    match row.get(index) {
        Some(lbug::Value::String(value)) => Ok(value.clone()),
        Some(value) => Err(GrustError::Serialization(format!(
            "Ladybug {context} column {index} was {value:?}, expected string"
        ))),
        None => Err(GrustError::Serialization(format!(
            "Ladybug {context} row missing column {index}"
        ))),
    }
}

fn row_to_node_table(row: Vec<lbug::Value>) -> Result<NodeTable> {
    Ok(NodeTable {
        label: Label::from(row_string(&row, 0, "node metadata")?),
        table: row_string(&row, 1, "node metadata")?,
    })
}

fn row_to_rel_table(row: Vec<lbug::Value>) -> Result<RelTable> {
    Ok(RelTable {
        label: Label::from(row_string(&row, 0, "relationship metadata")?),
        from_label: Label::from(row_string(&row, 1, "relationship metadata")?),
        to_label: Label::from(row_string(&row, 2, "relationship metadata")?),
        table: row_string(&row, 3, "relationship metadata")?,
    })
}

fn row_to_node(row: Vec<lbug::Value>, label: &Label) -> Result<Node> {
    let id = row_string(&row, 0, "node")?;
    let props = props_from_string(&row_string(&row, 1, "node")?)?;
    Ok(Node {
        id: NodeId::from(id),
        label: label.clone(),
        props,
    })
}

fn row_to_edge(row: Vec<lbug::Value>, label: &Label) -> Result<Edge> {
    let from = row_string(&row, 0, "edge")?;
    let to = row_string(&row, 1, "edge")?;
    let id = row_string(&row, 2, "edge")?;
    let props = props_from_string(&row_string(&row, 3, "edge")?)?;
    let mut edge = Edge::new(label.clone(), from, to, props);
    edge.id = Some(EdgeId::from(id));
    Ok(edge)
}

fn rel_index_id(label: &Label, from_label: &Label, to_label: &Label) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        label.as_str(),
        from_label.as_str(),
        to_label.as_str()
    )
}

#[cfg(test)]
mod tests;
