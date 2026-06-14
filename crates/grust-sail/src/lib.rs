use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::sync::{Arc, RwLock};

use arrow::array::{Array as _, Int64Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field as ArrowField, Schema as ArrowSchema};
use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use async_trait::async_trait;
use grust_core::prelude::*;
use tonic::transport::Channel;

#[allow(clippy::all, unused_imports, dead_code)]
mod spark_connect;
use spark_connect as sc;

use sc::spark_connect_service_client::SparkConnectServiceClient;
use sc::{
    Command, CreateDataFrameViewCommand, ExecutePlanRequest, Expression, LocalRelation, Plan,
    ReattachOptions, Relation, Sql, UserContext, command, execute_plan_request,
    execute_plan_response, expression, plan, relation,
};

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct SailConfig {
    pub endpoint: String,
    pub user_id: String,
    pub session_id: String,
    pub batch_size: usize,
}

impl Default for SailConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:50051".to_string(),
            user_id: "grust".to_string(),
            session_id: uuid::Uuid::new_v4().to_string(),
            batch_size: 1000,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SailDegreeRow {
    pub id: NodeId,
    pub degree: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SailDegreePairRow {
    pub id: NodeId,
    pub in_degree: usize,
    pub out_degree: usize,
}

// ── Store ─────────────────────────────────────────────────────────────────────

/// Session-scoped temp views used to stage Arrow batches before MERGE.
const NODE_STAGE_VIEW: &str = "grust_stage_nodes";
const EDGE_STAGE_VIEW: &str = "grust_stage_edges";
const DELETE_NODE_STAGE_VIEW: &str = "grust_delete_node_ids";
const DELETE_EDGE_STAGE_VIEW: &str = "grust_delete_edges";
const DROP_NODES_SQL: &str = "DROP TABLE IF EXISTS grust_nodes";
const DROP_EDGES_SQL: &str = "DROP TABLE IF EXISTS grust_edges";

pub struct SailGraphStore {
    config: SailConfig,
    client: SparkConnectServiceClient<Channel>,
    schema: RwLock<Option<GraphSchema>>,
}

impl SailGraphStore {
    pub async fn connect(config: SailConfig) -> Result<Self> {
        let client = SparkConnectServiceClient::connect(config.endpoint.clone())
            .await
            .map_err(|e| {
                GrustError::Backend(format!("connect to Sail at {}: {e}", config.endpoint))
            })?;
        Ok(Self {
            config,
            client,
            schema: RwLock::new(None),
        })
    }

    /// Stages an Arrow IPC stream as a replaceable Sail session temp view.
    ///
    /// The view name must already be a safe Grust/Sail SQL identifier such as
    /// `people_arrow`. Query it from the same `SailGraphStore` session.
    pub async fn stage_arrow_ipc_view(&self, name: &str, ipc_stream: &[u8]) -> Result<()> {
        validate_arrow_view_name(name)?;
        self.run_plan(self.stage_view_request(name, ipc_stream.to_vec()), |_| {
            Ok(())
        })
        .await
    }

    /// Executes Spark SQL through Sail and returns result batches as Arrow IPC streams.
    ///
    /// Each item in the returned vector is the complete IPC stream emitted by
    /// one Spark Connect `ArrowBatch` response.
    pub async fn query_arrow_ipc(&self, sql: &str) -> Result<Vec<Vec<u8>>> {
        let mut chunks = Vec::new();
        self.run_plan(self.query_request(sql, vec![])?, |data| {
            chunks.push(data.to_vec());
            Ok(())
        })
        .await?;
        Ok(chunks)
    }

    /// Loads Grust-shaped Arrow IPC node and edge streams into Sail tables.
    ///
    /// Node streams must provide `id`, `label`, and `props` string columns.
    /// Edge streams must provide `src_id`, `dst_id`, `edge_type`, and `props`
    /// string columns, and may include an optional string `id` column.
    pub async fn load_graph_arrow_ipc(
        &self,
        nodes_ipc: &[u8],
        edges_ipc: &[u8],
    ) -> Result<LoadReport> {
        self.bootstrap().await?;
        let graph = Graph::new(
            parse_nodes_from_arrow(nodes_ipc)?,
            parse_edges_from_arrow(edges_ipc)?,
        );
        self.put_graph(&graph).await
    }

    /// Reads the generic persisted `grust_nodes` and `grust_edges` tables into
    /// a portable Grust graph.
    pub async fn read_graph(&self) -> Result<Graph> {
        let nodes = self
            .run_query("SELECT id, label, props FROM grust_nodes", vec![])
            .await?;
        let edges = self
            .run_edge_query(
                "SELECT src_id, src_label, dst_id, dst_label, edge_type, props FROM grust_edges",
                vec![],
            )
            .await?;
        Ok(Graph::new(nodes, edges))
    }

    /// Computes out-degrees over the generic persisted Sail edge table.
    pub async fn out_degrees(&self) -> Result<Vec<SailDegreeRow>> {
        self.run_degree_query(&sail_out_degrees_sql()).await
    }

    /// Computes in-degrees over the generic persisted Sail edge table.
    pub async fn in_degrees(&self) -> Result<Vec<SailDegreeRow>> {
        self.run_degree_query(&sail_in_degrees_sql()).await
    }

    /// Computes total degree for each non-isolated vertex over the generic
    /// persisted Sail edge table.
    pub async fn degrees(&self) -> Result<Vec<SailDegreeRow>> {
        self.run_degree_query(&sail_degrees_sql()).await
    }

    /// Computes both directed degree components for every persisted vertex.
    pub async fn degree_pairs(&self) -> Result<Vec<SailDegreePairRow>> {
        let mut rows = Vec::new();
        self.run_plan(
            self.query_request(sail_degree_pairs_sql(), vec![])?,
            |data| {
                rows.extend(parse_degree_pairs_from_arrow(data)?);
                Ok(())
            },
        )
        .await?;
        Ok(rows)
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    fn user_context(&self) -> UserContext {
        UserContext {
            user_id: self.config.user_id.clone(),
            user_name: self.config.user_id.clone(),
            extensions: vec![],
        }
    }

    fn request_with_plan(&self, plan: Plan) -> ExecutePlanRequest {
        ExecutePlanRequest {
            session_id: self.config.session_id.clone(),
            user_context: Some(self.user_context()),
            operation_id: Some(uuid::Uuid::new_v4().to_string()),
            plan: Some(plan),
            client_type: Some("grust-sail/0.1.0".to_string()),
            request_options: vec![execute_plan_request::RequestOption {
                request_option: Some(
                    execute_plan_request::request_option::RequestOption::ReattachOptions(
                        ReattachOptions { reattachable: true },
                    ),
                ),
            }],
            ..Default::default()
        }
    }

    fn query_request(
        &self,
        sql: impl Into<String>,
        args: Vec<expression::Literal>,
    ) -> Result<ExecutePlanRequest> {
        let sql = sql.into();
        let (query, named_arguments) = bind_sql_arguments(&sql, args)?;
        Ok(self.request_with_plan(Plan {
            op_type: Some(plan::OpType::Root(Relation {
                common: None,
                rel_type: Some(relation::RelType::Sql(Sql {
                    query,
                    named_arguments,
                    ..Default::default()
                })),
            })),
        }))
    }

    async fn stage_record_batch(&self, name: &str, batch: RecordBatch) -> Result<()> {
        self.run_plan(
            self.stage_view_request(name, ipc_bytes(&batch)?),
            |_| Ok(()),
        )
        .await
    }

    /// Stages an Arrow record batch as a replaceable session temp view by
    /// shipping it as a Spark Connect `LocalRelation` (Arrow IPC bytes).
    fn stage_view_request(&self, name: &str, data: Vec<u8>) -> ExecutePlanRequest {
        self.request_with_plan(Plan {
            op_type: Some(plan::OpType::Command(Command {
                command_type: Some(command::CommandType::CreateDataframeView(
                    CreateDataFrameViewCommand {
                        input: Some(Relation {
                            common: None,
                            rel_type: Some(relation::RelType::LocalRelation(LocalRelation {
                                data: Some(data),
                                schema: None,
                            })),
                        }),
                        name: name.to_string(),
                        is_global: false,
                        replace: true,
                    },
                )),
            })),
        })
    }

    async fn run_plan(
        &self,
        req: ExecutePlanRequest,
        mut on_batch: impl FnMut(&[u8]) -> Result<()> + Send,
    ) -> Result<()> {
        let mut client = self.client.clone();
        let mut stream = client
            .execute_plan(req)
            .await
            .map_err(|e| GrustError::Backend(format!("execute_plan failed: {e}")))?
            .into_inner();
        loop {
            match stream.message().await {
                Ok(None) => break,
                Ok(Some(resp)) => {
                    if let Some(execute_plan_response::ResponseType::ArrowBatch(batch)) =
                        resp.response_type
                        && batch.row_count > 0
                    {
                        on_batch(&batch.data)?;
                    }
                }
                Err(e) => return Err(GrustError::Backend(format!("Sail stream error: {e}"))),
            }
        }
        Ok(())
    }

    async fn run_command(&self, sql: &str, args: Vec<expression::Literal>) -> Result<()> {
        if !args.is_empty() {
            return Err(GrustError::Backend(
                "Sail SQL commands do not support Spark Connect arguments yet".to_string(),
            ));
        }
        self.run_plan(self.query_request(sql, args)?, |_| Ok(()))
            .await
    }

    async fn run_query(&self, sql: &str, args: Vec<expression::Literal>) -> Result<Vec<Node>> {
        let mut nodes = Vec::new();
        self.run_plan(self.query_request(sql, args)?, |data| {
            nodes.extend(parse_nodes_from_arrow(data)?);
            Ok(())
        })
        .await?;
        Ok(nodes)
    }

    async fn run_edge_query(&self, sql: &str, args: Vec<expression::Literal>) -> Result<Vec<Edge>> {
        let mut edges = Vec::new();
        self.run_plan(self.query_request(sql, args)?, |data| {
            edges.extend(parse_edges_from_arrow(data)?);
            Ok(())
        })
        .await?;
        Ok(edges)
    }

    async fn run_degree_query(&self, sql: &str) -> Result<Vec<SailDegreeRow>> {
        let mut rows = Vec::new();
        self.run_plan(self.query_request(sql, vec![])?, |data| {
            rows.extend(parse_degrees_from_arrow(data)?);
            Ok(())
        })
        .await?;
        Ok(rows)
    }

    fn current_schema(&self) -> Option<GraphSchema> {
        self.schema
            .read()
            .expect("Sail schema lock poisoned")
            .clone()
    }

    /// Stages a node batch and merges it into the generic and typed tables.
    async fn load_nodes(&self, schema: Option<&GraphSchema>, nodes: &[Node]) -> Result<()> {
        if nodes.is_empty() {
            return Ok(());
        }
        let batch = nodes_record_batch(nodes)?;
        self.stage_record_batch(NODE_STAGE_VIEW, batch).await?;
        self.run_command(&merge_nodes_from_view_sql(), vec![])
            .await?;
        if let Some(schema) = schema {
            for node_type in &schema.nodes {
                if nodes.iter().any(|node| node.label == node_type.label) {
                    self.run_command(&typed_node_merge_from_view_sql(node_type)?, vec![])
                        .await?;
                }
            }
        }
        Ok(())
    }

    /// Stages an edge batch and merges it into the generic and typed tables.
    async fn load_edges(
        &self,
        schema: Option<&GraphSchema>,
        edges: &[Edge],
        node_labels: &BTreeMap<&NodeId, &Label>,
    ) -> Result<()> {
        if edges.is_empty() {
            return Ok(());
        }
        let batch = edges_record_batch(edges, node_labels)?;
        self.stage_record_batch(EDGE_STAGE_VIEW, batch).await?;
        self.run_command(&merge_edges_from_view_sql(), vec![])
            .await?;
        if let Some(schema) = schema {
            for edge_type in &schema.edges {
                if edges.iter().any(|edge| edge.label == edge_type.label) {
                    self.run_command(&typed_edge_merge_from_view_sql(edge_type)?, vec![])
                        .await?;
                }
            }
        }
        Ok(())
    }
}

// ── GraphStore ────────────────────────────────────────────────────────────────

#[async_trait]
impl GraphStore for SailGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        self.bootstrap().await?;
        for statement in sail_schema_sql(schema)? {
            self.run_command(&statement, vec![]).await?;
        }
        *self.schema.write().expect("Sail schema lock poisoned") = Some(schema.clone());
        Ok(())
    }

    async fn put_node(&self, node: &Node) -> Result<PutOutcome> {
        let schema = self.current_schema();
        if let Some(schema) = schema.as_ref() {
            schema.validate_node(node)?;
        }
        self.load_nodes(schema.as_ref(), std::slice::from_ref(node))
            .await?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        let schema = self.current_schema();
        if let Some(schema) = schema.as_ref() {
            schema.validate_edge_props(edge)?;
        }
        self.load_edges(
            schema.as_ref(),
            std::slice::from_ref(edge),
            &BTreeMap::new(),
        )
        .await?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        let schema = self.current_schema();
        if let Some(schema) = schema.as_ref() {
            schema.validate_graph(graph)?;
        }
        let node_labels: BTreeMap<&NodeId, &Label> = graph
            .nodes
            .iter()
            .map(|node| (&node.id, &node.label))
            .collect();
        let batch = self.config.batch_size.max(1);
        let mut report = LoadReport::default();
        for chunk in graph.nodes.chunks(batch) {
            self.load_nodes(schema.as_ref(), chunk).await?;
            report.nodes += chunk.len();
        }
        for chunk in graph.edges.chunks(batch) {
            self.load_edges(schema.as_ref(), chunk, &node_labels)
                .await?;
            report.edges += chunk.len();
        }
        Ok(report)
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        let sql = "SELECT id, label, props FROM grust_nodes WHERE id = ? LIMIT 1";
        Ok(self
            .run_query(sql, vec![lit_str(id.as_str())])
            .await?
            .into_iter()
            .next())
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        let mut conditions = Vec::new();
        let mut args = Vec::new();
        if let Some(from) = &query.from {
            conditions.push("src_id = ?");
            args.push(lit_str(from.as_str()));
        }
        if let Some(to) = &query.to {
            conditions.push("dst_id = ?");
            args.push(lit_str(to.as_str()));
        }
        if let Some(label) = &query.label {
            conditions.push("edge_type = ?");
            args.push(lit_str(label.as_str()));
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };
        let sql = format!(
            "SELECT src_id, src_label, dst_id, dst_label, edge_type, props FROM grust_edges{}",
            where_clause
        );
        self.run_edge_query(&sql, args).await
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        let (sql, args) = traversal_sql(&traversal)?;
        self.run_query(&sql, args).await
    }
}

// ── GraphAdminStore ───────────────────────────────────────────────────────────

#[async_trait]
impl GraphAdminStore for SailGraphStore {
    async fn bootstrap(&self) -> Result<()> {
        self.run_command(
            "CREATE TABLE IF NOT EXISTS grust_nodes USING delta AS \
             SELECT CAST(NULL AS STRING) AS id, \
                    CAST(NULL AS STRING) AS label, \
                    CAST(NULL AS STRING) AS props \
             WHERE FALSE",
            vec![],
        )
        .await?;
        self.run_command(
            "CREATE TABLE IF NOT EXISTS grust_edges USING delta AS \
             SELECT CAST(NULL AS STRING) AS src_id, \
                    CAST(NULL AS STRING) AS src_label, \
                    CAST(NULL AS STRING) AS dst_id, \
                    CAST(NULL AS STRING) AS dst_label, \
                    CAST(NULL AS STRING) AS edge_type, \
                    CAST(NULL AS STRING) AS props \
             WHERE FALSE",
            vec![],
        )
        .await?;
        Ok(())
    }

    async fn clear(&self) -> Result<()> {
        if let Some(schema) = self.current_schema() {
            for edge_type in &schema.edges {
                self.run_command(
                    &format!(
                        "DROP TABLE IF EXISTS {}",
                        sail_edge_table(edge_type.label.as_str())?
                    ),
                    vec![],
                )
                .await?;
            }
            for node_type in &schema.nodes {
                self.run_command(
                    &format!(
                        "DROP TABLE IF EXISTS {}",
                        sail_node_table(node_type.label.as_str())?
                    ),
                    vec![],
                )
                .await?;
            }
        }
        self.run_command(DROP_EDGES_SQL, vec![]).await?;
        self.run_command(DROP_NODES_SQL, vec![]).await?;
        self.bootstrap().await
    }
}

// ── GraphMutationStore ────────────────────────────────────────────────────────

#[async_trait]
impl GraphMutationStore for SailGraphStore {
    async fn delete_node(&self, id: &NodeId) -> Result<()> {
        self.stage_record_batch(DELETE_NODE_STAGE_VIEW, node_ids_record_batch(&[id])?)
            .await?;
        self.run_command(&delete_nodes_from_view_sql("grust_nodes")?, vec![])
            .await?;
        self.run_command(&delete_node_edges_from_view_sql("grust_edges")?, vec![])
            .await?;
        if let Some(schema) = self.current_schema() {
            for node_type in &schema.nodes {
                self.run_command(
                    &delete_nodes_from_view_sql(&sail_node_table(node_type.label.as_str())?)?,
                    vec![],
                )
                .await?;
            }
            for edge_type in &schema.edges {
                self.run_command(
                    &delete_node_edges_from_view_sql(&sail_edge_table(edge_type.label.as_str())?)?,
                    vec![],
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn delete_edge(&self, from: &NodeId, label: &Label, to: &NodeId) -> Result<()> {
        self.stage_record_batch(
            DELETE_EDGE_STAGE_VIEW,
            delete_edges_record_batch(&[(from, label, to)])?,
        )
        .await?;
        self.run_command(&delete_edges_from_view_sql("grust_edges", true)?, vec![])
            .await?;
        let typed_table = self
            .current_schema()
            .and_then(|schema| schema.edge_type(label).cloned());
        if let Some(edge_type) = typed_table {
            self.run_command(
                &delete_edges_from_view_sql(&sail_edge_table(edge_type.label.as_str())?, false)?,
                vec![],
            )
            .await?;
        }
        Ok(())
    }
}

// ── Arrow staging ─────────────────────────────────────────────────────────────

fn nodes_record_batch(nodes: &[Node]) -> Result<RecordBatch> {
    let schema = Arc::new(ArrowSchema::new(vec![
        ArrowField::new("id", DataType::Utf8, false),
        ArrowField::new("label", DataType::Utf8, false),
        ArrowField::new("props", DataType::Utf8, true),
    ]));
    let props = nodes
        .iter()
        .map(|node| props_to_json(&node.props))
        .collect::<Result<Vec<_>>>()?;
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from_iter_values(
                nodes.iter().map(|node| node.id.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                nodes.iter().map(|node| node.label.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                props.iter().map(String::as_str),
            )),
        ],
    )
    .map_err(|e| GrustError::Backend(format!("Arrow node batch build failed: {e}")))
}

fn edges_record_batch(
    edges: &[Edge],
    node_labels: &BTreeMap<&NodeId, &Label>,
) -> Result<RecordBatch> {
    let schema = Arc::new(ArrowSchema::new(vec![
        ArrowField::new("src_id", DataType::Utf8, false),
        ArrowField::new("src_label", DataType::Utf8, false),
        ArrowField::new("dst_id", DataType::Utf8, false),
        ArrowField::new("dst_label", DataType::Utf8, false),
        ArrowField::new("edge_type", DataType::Utf8, false),
        ArrowField::new("props", DataType::Utf8, true),
        ArrowField::new("edge_key", DataType::Utf8, false),
        ArrowField::new("id", DataType::Utf8, true),
    ]));
    let props = edges
        .iter()
        .map(|edge| props_to_json(&edge.props))
        .collect::<Result<Vec<_>>>()?;
    let label_of = |id: &NodeId| {
        node_labels
            .get(id)
            .map(|label| label.as_str())
            .unwrap_or("")
    };
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from_iter_values(
                edges.iter().map(|edge| edge.from.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                edges.iter().map(|edge| label_of(&edge.from)),
            )),
            Arc::new(StringArray::from_iter_values(
                edges.iter().map(|edge| edge.to.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                edges.iter().map(|edge| label_of(&edge.to)),
            )),
            Arc::new(StringArray::from_iter_values(
                edges.iter().map(|edge| edge.label.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                props.iter().map(String::as_str),
            )),
            Arc::new(StringArray::from_iter_values(edges.iter().map(edge_key))),
            Arc::new(StringArray::from(
                edges
                    .iter()
                    .map(|edge| edge.id.as_ref().map(EdgeId::as_str))
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .map_err(|e| GrustError::Backend(format!("Arrow edge batch build failed: {e}")))
}

fn node_ids_record_batch(ids: &[&NodeId]) -> Result<RecordBatch> {
    let schema = Arc::new(ArrowSchema::new(vec![ArrowField::new(
        "id",
        DataType::Utf8,
        false,
    )]));
    RecordBatch::try_new(
        schema,
        vec![Arc::new(StringArray::from_iter_values(
            ids.iter().map(|id| id.as_str()),
        ))],
    )
    .map_err(|e| GrustError::Backend(format!("Arrow node delete batch build failed: {e}")))
}

fn delete_edges_record_batch(edges: &[(&NodeId, &Label, &NodeId)]) -> Result<RecordBatch> {
    let schema = Arc::new(ArrowSchema::new(vec![
        ArrowField::new("src_id", DataType::Utf8, false),
        ArrowField::new("dst_id", DataType::Utf8, false),
        ArrowField::new("edge_type", DataType::Utf8, false),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from_iter_values(
                edges.iter().map(|(from, _, _)| from.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                edges.iter().map(|(_, _, to)| to.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                edges.iter().map(|(_, label, _)| label.as_str()),
            )),
        ],
    )
    .map_err(|e| GrustError::Backend(format!("Arrow edge delete batch build failed: {e}")))
}

fn ipc_bytes(batch: &RecordBatch) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    {
        let cursor = Cursor::new(&mut data);
        let mut writer = StreamWriter::try_new(cursor, batch.schema().as_ref())
            .map_err(|e| GrustError::Backend(format!("Arrow IPC write failed: {e}")))?;
        writer
            .write(batch)
            .map_err(|e| GrustError::Backend(format!("Arrow IPC write failed: {e}")))?;
        writer
            .finish()
            .map_err(|e| GrustError::Backend(format!("Arrow IPC write failed: {e}")))?;
    }
    Ok(data)
}

// ── SQL builders ──────────────────────────────────────────────────────────────

fn merge_nodes_from_view_sql() -> String {
    format!(
        "MERGE INTO grust_nodes AS t \
         USING {NODE_STAGE_VIEW} AS s ON t.id = s.id \
         WHEN MATCHED THEN UPDATE SET t.label = s.label, t.props = s.props \
         WHEN NOT MATCHED THEN INSERT (id, label, props) VALUES (s.id, s.label, s.props)"
    )
}

fn merge_edges_from_view_sql() -> String {
    format!(
        "MERGE INTO grust_edges AS t \
         USING {EDGE_STAGE_VIEW} AS s \
         ON t.src_id = s.src_id AND t.dst_id = s.dst_id AND t.edge_type = s.edge_type \
         WHEN MATCHED THEN UPDATE SET t.src_label = s.src_label, t.dst_label = s.dst_label, t.props = s.props \
         WHEN NOT MATCHED THEN INSERT (src_id, src_label, dst_id, dst_label, edge_type, props) \
           VALUES (s.src_id, s.src_label, s.dst_id, s.dst_label, s.edge_type, s.props)"
    )
}

fn delete_nodes_from_view_sql(table: &str) -> Result<String> {
    Ok(format!(
        "MERGE INTO {} AS t USING {DELETE_NODE_STAGE_VIEW} AS s \
         ON t.id = s.id WHEN MATCHED THEN DELETE",
        sql_table_ref(table)?
    ))
}

fn delete_node_edges_from_view_sql(table: &str) -> Result<String> {
    Ok(format!(
        "MERGE INTO {} AS t USING {DELETE_NODE_STAGE_VIEW} AS s \
         ON t.src_id = s.id OR t.dst_id = s.id WHEN MATCHED THEN DELETE",
        sql_table_ref(table)?
    ))
}

fn delete_edges_from_view_sql(table: &str, include_label: bool) -> Result<String> {
    let label_match = if include_label {
        " AND t.edge_type = s.edge_type"
    } else {
        ""
    };
    Ok(format!(
        "MERGE INTO {} AS t USING {DELETE_EDGE_STAGE_VIEW} AS s \
         ON t.src_id = s.src_id AND t.dst_id = s.dst_id{label_match} \
         WHEN MATCHED THEN DELETE",
        sql_table_ref(table)?
    ))
}

pub fn sail_out_degrees_sql() -> String {
    "SELECT src_id AS id, COUNT(*) AS degree FROM grust_edges GROUP BY src_id".to_string()
}

pub fn sail_in_degrees_sql() -> String {
    "SELECT dst_id AS id, COUNT(*) AS degree FROM grust_edges GROUP BY dst_id".to_string()
}

pub fn sail_degrees_sql() -> String {
    "SELECT id, SUM(degree) AS degree FROM (\
       SELECT src_id AS id, COUNT(*) AS degree FROM grust_edges GROUP BY src_id \
       UNION ALL \
       SELECT dst_id AS id, COUNT(*) AS degree FROM grust_edges GROUP BY dst_id\
     ) degree_events GROUP BY id"
        .to_string()
}

pub fn sail_degree_pairs_sql() -> String {
    "SELECT n.id AS id, \
            COALESCE(in_degrees.degree, 0) AS in_degree, \
            COALESCE(out_degrees.degree, 0) AS out_degree \
       FROM grust_nodes n \
       LEFT JOIN (SELECT dst_id AS id, COUNT(*) AS degree FROM grust_edges GROUP BY dst_id) in_degrees \
         ON n.id = in_degrees.id \
       LEFT JOIN (SELECT src_id AS id, COUNT(*) AS degree FROM grust_edges GROUP BY src_id) out_degrees \
         ON n.id = out_degrees.id"
        .to_string()
}

fn sail_schema_sql(schema: &GraphSchema) -> Result<Vec<String>> {
    let mut statements = Vec::new();
    for node_type in &schema.nodes {
        let fields = node_type
            .fields
            .iter()
            .map(|field| {
                Ok(format!(
                    "{} {}",
                    sql_ident(&field.name)?,
                    sail_sql_type(&field.ty)
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        let fields = if fields.is_empty() {
            String::new()
        } else {
            format!(", {fields}")
        };
        statements.push(format!(
            "CREATE TABLE IF NOT EXISTS {} (id STRING NOT NULL{fields}) USING delta",
            sail_node_table(node_type.label.as_str())?
        ));
    }
    for edge_type in &schema.edges {
        let fields = edge_type
            .fields
            .iter()
            .map(|field| {
                Ok(format!(
                    "{} {}",
                    sql_ident(&field.name)?,
                    sail_sql_type(&field.ty)
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        let fields = if fields.is_empty() {
            String::new()
        } else {
            format!(", {fields}")
        };
        statements.push(format!(
            "CREATE TABLE IF NOT EXISTS {} (edge_key STRING NOT NULL, id STRING, src_id STRING NOT NULL, dst_id STRING NOT NULL{fields}) USING delta",
            sail_edge_table(edge_type.label.as_str())?
        ));
    }
    Ok(statements)
}

/// SQL expression extracting one typed field from the staged plain-JSON props
/// column.
fn props_field_expr(props_column: &str, field: &Field) -> Result<String> {
    validate_json_key(&field.name)?;
    let raw = format!("GET_JSON_OBJECT({props_column}, '$.{}')", field.name);
    Ok(match field.ty {
        FieldType::String | FieldType::DateTime => raw,
        FieldType::Int => format!("CAST({raw} AS BIGINT)"),
        FieldType::Float => format!("CAST({raw} AS DOUBLE)"),
        FieldType::Bool => format!("CAST({raw} AS BOOLEAN)"),
        FieldType::StringArray | FieldType::IntArray | FieldType::FloatArray | FieldType::Json => {
            raw
        }
    })
}

fn typed_node_merge_from_view_sql(node_type: &NodeType) -> Result<String> {
    let mut select_columns = vec!["s.id AS id".to_string()];
    let mut insert_columns = vec!["id".to_string()];
    for field in &node_type.fields {
        let column = sql_ident(&field.name)?;
        select_columns.push(format!(
            "{} AS {column}",
            props_field_expr("s.props", field)?
        ));
        insert_columns.push(column);
    }
    let updates = insert_columns
        .iter()
        .filter(|column| column.as_str() != "id")
        .map(|column| format!("t.{column} = s.{column}"))
        .collect::<Vec<_>>()
        .join(", ");
    let update_clause = if updates.is_empty() {
        String::new()
    } else {
        format!(" WHEN MATCHED THEN UPDATE SET {updates}")
    };
    Ok(format!(
        "MERGE INTO {} AS t USING (SELECT {} FROM {NODE_STAGE_VIEW} s WHERE s.label = {}) AS s \
         ON t.id = s.id{update_clause} WHEN NOT MATCHED THEN INSERT ({}) VALUES ({})",
        sail_node_table(node_type.label.as_str())?,
        select_columns.join(", "),
        sql_str(node_type.label.as_str()),
        insert_columns.join(", "),
        insert_columns
            .iter()
            .map(|column| format!("s.{column}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn typed_edge_merge_from_view_sql(edge_type: &EdgeType) -> Result<String> {
    let mut select_columns = vec![
        "s.edge_key AS edge_key".to_string(),
        "s.id AS id".to_string(),
        "s.src_id AS src_id".to_string(),
        "s.dst_id AS dst_id".to_string(),
    ];
    let mut insert_columns = vec![
        "edge_key".to_string(),
        "id".to_string(),
        "src_id".to_string(),
        "dst_id".to_string(),
    ];
    for field in &edge_type.fields {
        let column = sql_ident(&field.name)?;
        select_columns.push(format!(
            "{} AS {column}",
            props_field_expr("s.props", field)?
        ));
        insert_columns.push(column);
    }
    let updates = insert_columns
        .iter()
        .filter(|column| column.as_str() != "edge_key")
        .map(|column| format!("t.{column} = s.{column}"))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "MERGE INTO {} AS t USING (SELECT {} FROM {EDGE_STAGE_VIEW} s WHERE s.edge_type = {}) AS s \
         ON t.edge_key = s.edge_key WHEN MATCHED THEN UPDATE SET {updates} \
         WHEN NOT MATCHED THEN INSERT ({}) VALUES ({})",
        sail_edge_table(edge_type.label.as_str())?,
        select_columns.join(", "),
        sql_str(edge_type.label.as_str()),
        insert_columns.join(", "),
        insert_columns
            .iter()
            .map(|column| format!("s.{column}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

// Joins match nodes to edges by id only: node ids are globally unique (the
// MERGE key in grust_nodes), and grust_edges rows written without the full
// graph in scope carry empty src_label/dst_label, so label equality must not
// be part of the join. The generated `?` slots are bound as Spark Connect
// named arguments before execution.
fn traversal_sql(traversal: &Traversal) -> Result<(String, Vec<expression::Literal>)> {
    if traversal.steps.is_empty() {
        // Just return the start node(s)
        let (where_clause, args) = start_clause(&traversal.start, "n0")?;
        let limit = limit_clause(traversal.limit);
        return Ok((
            format!("SELECT n0.id, n0.label, n0.props FROM grust_nodes n0{where_clause}{limit}"),
            args,
        ));
    }

    let mut joins = Vec::new();
    let mut args = Vec::new();
    let last_node_alias = format!("n{}", traversal.steps.len());

    for (i, step) in traversal.steps.iter().enumerate() {
        let prev_node = format!("n{i}");
        let edge_alias = format!("e{i}");
        let next_node = format!("n{}", i + 1);

        let edge_type_cond = step
            .edge
            .as_ref()
            .map(|label| {
                args.push(lit_str(label.as_str()));
                format!(" AND {edge_alias}.edge_type = ?")
            })
            .unwrap_or_default();

        match &step.direction {
            Direction::Out => {
                joins.push(format!(
                    "JOIN grust_edges {edge_alias} ON {edge_alias}.src_id = {prev_node}.id{edge_type_cond}"
                ));
                joins.push(format!(
                    "JOIN grust_nodes {next_node} ON {next_node}.id = {edge_alias}.dst_id"
                ));
            }
            Direction::In => {
                joins.push(format!(
                    "JOIN grust_edges {edge_alias} ON {edge_alias}.dst_id = {prev_node}.id{edge_type_cond}"
                ));
                joins.push(format!(
                    "JOIN grust_nodes {next_node} ON {next_node}.id = {edge_alias}.src_id"
                ));
            }
            Direction::Both => {
                joins.push(format!(
                    "JOIN grust_edges {edge_alias} ON ({edge_alias}.src_id = {prev_node}.id OR {edge_alias}.dst_id = {prev_node}.id){edge_type_cond}"
                ));
                joins.push(format!(
                    "JOIN grust_nodes {next_node} ON {next_node}.id = (CASE WHEN {edge_alias}.src_id = {prev_node}.id THEN {edge_alias}.dst_id ELSE {edge_alias}.src_id END)"
                ));
            }
        }

        if let Some(label) = &step.node {
            args.push(lit_str(label.as_str()));
            let join = joins.last_mut().expect("node join exists");
            join.push_str(&format!(" AND {next_node}.label = ?"));
        }
    }

    let (start_where, start_args) = start_clause(&traversal.start, "n0")?;
    args.extend(start_args);
    let limit = limit_clause(traversal.limit);
    let join_str = joins.join(" ");
    Ok((
        format!(
            "SELECT {last_node_alias}.id, {last_node_alias}.label, {last_node_alias}.props \
             FROM grust_nodes n0 {join_str}{start_where}{limit}"
        ),
        args,
    ))
}

fn start_clause(start: &Start, alias: &str) -> Result<(String, Vec<expression::Literal>)> {
    Ok(match start {
        Start::Node(id) => (format!(" WHERE {alias}.id = ?"), vec![lit_str(id.as_str())]),
        Start::NodesByLabel(label) => (
            format!(" WHERE {alias}.label = ?"),
            vec![lit_str(label.as_str())],
        ),
        Start::NodesByProperty { label, key, value } => {
            validate_json_key(key)?;
            let json_value = format!("GET_JSON_OBJECT({alias}.props, '$.{key}')");
            let mut args = vec![lit_str(label.as_str())];
            let val_expr = match value {
                Value::String(s) => {
                    args.push(lit_str(s));
                    format!("{json_value} = ?")
                }
                Value::Int(n) => {
                    args.push(lit_long(*n));
                    format!("CAST({json_value} AS BIGINT) = ?")
                }
                Value::Float(f) => {
                    args.push(lit_double(*f));
                    format!("CAST({json_value} AS DOUBLE) = ?")
                }
                Value::Bool(b) => {
                    args.push(lit_bool(*b));
                    format!("CAST({json_value} AS BOOLEAN) = ?")
                }
                _ => format!("{json_value} IS NOT NULL"),
            };
            (format!(" WHERE {alias}.label = ? AND {val_expr}"), args)
        }
    })
}

fn lit_str(value: &str) -> expression::Literal {
    expression::Literal {
        literal_type: Some(expression::literal::LiteralType::String(value.to_string())),
        ..Default::default()
    }
}

fn lit_long(value: i64) -> expression::Literal {
    expression::Literal {
        literal_type: Some(expression::literal::LiteralType::Long(value)),
        ..Default::default()
    }
}

fn lit_double(value: f64) -> expression::Literal {
    expression::Literal {
        literal_type: Some(expression::literal::LiteralType::Double(value)),
        ..Default::default()
    }
}

fn lit_bool(value: bool) -> expression::Literal {
    expression::Literal {
        literal_type: Some(expression::literal::LiteralType::Boolean(value)),
        ..Default::default()
    }
}

fn bind_sql_arguments(
    sql: &str,
    args: Vec<expression::Literal>,
) -> Result<(String, HashMap<String, Expression>)> {
    let mut query = String::with_capacity(sql.len() + args.len() * 3);
    let mut parts = sql.split('?');
    let Some(first) = parts.next() else {
        return Ok((sql.to_string(), HashMap::new()));
    };
    query.push_str(first);

    let mut named_arguments = HashMap::with_capacity(args.len());
    let mut used = 0;
    for part in parts {
        let Some(arg) = args.get(used) else {
            return Err(GrustError::Backend(format!(
                "missing SQL argument {used} for query: {sql}"
            )));
        };
        let name = format!("p{}", used + 1);
        query.push(':');
        query.push_str(&name);
        named_arguments.insert(name, lit_expr(arg.clone()));
        query.push_str(part);
        used += 1;
    }
    if used != args.len() {
        return Err(GrustError::Backend(format!(
            "unused SQL arguments: query used {used}, got {}",
            args.len()
        )));
    }
    Ok((query, named_arguments))
}

fn lit_expr(literal: expression::Literal) -> Expression {
    Expression {
        common: None,
        expr_type: Some(expression::ExprType::Literal(literal)),
    }
}

fn validate_arrow_view_name(name: &str) -> Result<()> {
    let normalized = schema_identifier(name)?;
    if normalized == name {
        Ok(())
    } else {
        Err(GrustError::Schema(format!(
            "Arrow view name '{name}' must be a safe lower_snake SQL identifier"
        )))
    }
}

fn sail_sql_type(ty: &FieldType) -> &'static str {
    match ty {
        FieldType::String
        | FieldType::DateTime
        | FieldType::StringArray
        | FieldType::IntArray
        | FieldType::FloatArray
        | FieldType::Json => "STRING",
        FieldType::Int => "BIGINT",
        FieldType::Float => "DOUBLE",
        FieldType::Bool => "BOOLEAN",
    }
}

fn sail_node_table(label: &str) -> Result<String> {
    Ok(format!("grust_node_{}", schema_identifier(label)?))
}

fn sail_edge_table(label: &str) -> Result<String> {
    Ok(format!("grust_edge_{}", schema_identifier(label)?))
}

fn sql_ident(value: &str) -> Result<String> {
    let identifier = schema_identifier(value)?;
    Ok(format!("`{identifier}`"))
}

fn sql_table_ref(value: &str) -> Result<String> {
    sql_ident(value)
}

fn limit_clause(limit: Option<u32>) -> String {
    limit.map(|n| format!(" LIMIT {n}")).unwrap_or_default()
}

fn sql_str(s: &str) -> String {
    // Spark SQL string literals treat backslash as an escape character, so
    // double backslashes as well as single quotes.
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "''"))
}

fn validate_json_key(value: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    if valid {
        Ok(())
    } else {
        Err(GrustError::Schema(format!(
            "invalid JSON property key '{value}'"
        )))
    }
}

// ── Props JSON ────────────────────────────────────────────────────────────────

/// Serializes props as plain (untagged) JSON so SQL `GET_JSON_OBJECT` paths
/// like `$.name` resolve directly to the value.
fn props_to_json(props: &Props) -> Result<String> {
    let mut map = serde_json::Map::new();
    for (key, value) in props {
        if let Value::Float(f) = value
            && !f.is_finite()
        {
            return Err(GrustError::Serialization(format!(
                "non-finite float {f} in property '{key}' cannot be stored as JSON"
            )));
        }
        if let Value::FloatArray(values) = value
            && values.iter().any(|f| !f.is_finite())
        {
            return Err(GrustError::Serialization(format!(
                "non-finite float in property '{key}' cannot be stored as JSON"
            )));
        }
        map.insert(key.clone(), value.to_json());
    }
    serde_json::to_string(&serde_json::Value::Object(map))
        .map_err(|e| GrustError::Serialization(e.to_string()))
}

/// Parses props from JSON, accepting both the plain form written by this
/// backend and the legacy tagged `{"type": ..., "value": ...}` form.
fn props_from_json(data: &str) -> Result<Props> {
    let raw: BTreeMap<String, serde_json::Value> = serde_json::from_str(data)
        .map_err(|e| GrustError::Serialization(format!("props JSON parse: {e}")))?;
    Ok(raw
        .into_iter()
        .map(|(key, value)| (key, Value::from_json(value)))
        .collect())
}

// ── Arrow parsing ─────────────────────────────────────────────────────────────

fn parse_nodes_from_arrow(data: &[u8]) -> Result<Vec<Node>> {
    let reader = StreamReader::try_new(Cursor::new(data), None)
        .map_err(|e| GrustError::Backend(format!("Arrow IPC read failed: {e}")))?;
    let schema = reader.schema();
    let id_idx = schema
        .index_of("id")
        .map_err(|_| GrustError::Schema("grust_nodes missing 'id' column".into()))?;
    let label_idx = schema
        .index_of("label")
        .map_err(|_| GrustError::Schema("grust_nodes missing 'label' column".into()))?;
    let props_idx = schema
        .index_of("props")
        .map_err(|_| GrustError::Schema("grust_nodes missing 'props' column".into()))?;

    let mut nodes = Vec::new();
    for batch in reader {
        let batch = batch.map_err(|e| GrustError::Backend(format!("Arrow batch error: {e}")))?;
        let ids = batch
            .column(id_idx)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| GrustError::Schema("id column is not string".into()))?;
        let labels = batch
            .column(label_idx)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| GrustError::Schema("label column is not string".into()))?;
        let props_col = batch
            .column(props_idx)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| GrustError::Schema("props column is not string".into()))?;

        for i in 0..batch.num_rows() {
            let id = ids.value(i);
            let label = labels.value(i);
            let props = if props_col.is_null(i) || props_col.value(i).is_empty() {
                Props::new()
            } else {
                props_from_json(props_col.value(i))?
            };
            nodes.push(Node {
                id: NodeId::new(id),
                label: Label::new(label),
                props,
            });
        }
    }
    Ok(nodes)
}

fn parse_edges_from_arrow(data: &[u8]) -> Result<Vec<Edge>> {
    let reader = StreamReader::try_new(Cursor::new(data), None)
        .map_err(|e| GrustError::Backend(format!("Arrow IPC read failed: {e}")))?;
    let schema = reader.schema();
    let src_id_idx = schema
        .index_of("src_id")
        .map_err(|_| GrustError::Schema("grust_edges missing 'src_id' column".into()))?;
    let dst_id_idx = schema
        .index_of("dst_id")
        .map_err(|_| GrustError::Schema("grust_edges missing 'dst_id' column".into()))?;
    let edge_type_idx = schema
        .index_of("edge_type")
        .map_err(|_| GrustError::Schema("grust_edges missing 'edge_type' column".into()))?;
    let props_idx = schema
        .index_of("props")
        .map_err(|_| GrustError::Schema("grust_edges missing 'props' column".into()))?;
    let id_idx = schema.index_of("id").ok();

    let mut edges = Vec::new();
    for batch in reader {
        let batch = batch.map_err(|e| GrustError::Backend(format!("Arrow batch error: {e}")))?;
        let src_ids = batch
            .column(src_id_idx)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| GrustError::Schema("src_id column is not string".into()))?;
        let dst_ids = batch
            .column(dst_id_idx)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| GrustError::Schema("dst_id column is not string".into()))?;
        let edge_types = batch
            .column(edge_type_idx)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| GrustError::Schema("edge_type column is not string".into()))?;
        let props_col = batch
            .column(props_idx)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| GrustError::Schema("props column is not string".into()))?;
        let ids = if let Some(id_idx) = id_idx {
            Some(
                batch
                    .column(id_idx)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| GrustError::Schema("id column is not string".into()))?,
            )
        } else {
            None
        };

        for i in 0..batch.num_rows() {
            let props = if props_col.is_null(i) || props_col.value(i).is_empty() {
                Props::new()
            } else {
                props_from_json(props_col.value(i))?
            };
            let id = ids.and_then(|ids| {
                if ids.is_null(i) || ids.value(i).is_empty() {
                    None
                } else {
                    Some(EdgeId::new(ids.value(i)))
                }
            });
            edges.push(Edge {
                id,
                from: NodeId::new(src_ids.value(i)),
                to: NodeId::new(dst_ids.value(i)),
                label: Label::new(edge_types.value(i)),
                props,
            });
        }
    }
    Ok(edges)
}

fn parse_degrees_from_arrow(data: &[u8]) -> Result<Vec<SailDegreeRow>> {
    let reader = StreamReader::try_new(Cursor::new(data), None)
        .map_err(|e| GrustError::Backend(format!("Arrow IPC read failed: {e}")))?;
    let schema = reader.schema();
    let id_idx = schema
        .index_of("id")
        .map_err(|_| GrustError::Schema("degree result missing 'id' column".into()))?;
    let degree_idx = schema
        .index_of("degree")
        .map_err(|_| GrustError::Schema("degree result missing 'degree' column".into()))?;

    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch.map_err(|e| GrustError::Backend(format!("Arrow batch error: {e}")))?;
        let ids = string_column(&batch, id_idx, "id")?;
        let degrees = int64_column(&batch, degree_idx, "degree")?;
        for i in 0..batch.num_rows() {
            rows.push(SailDegreeRow {
                id: NodeId::new(ids.value(i)),
                degree: usize_from_i64(degrees.value(i), "degree")?,
            });
        }
    }
    Ok(rows)
}

fn parse_degree_pairs_from_arrow(data: &[u8]) -> Result<Vec<SailDegreePairRow>> {
    let reader = StreamReader::try_new(Cursor::new(data), None)
        .map_err(|e| GrustError::Backend(format!("Arrow IPC read failed: {e}")))?;
    let schema = reader.schema();
    let id_idx = schema
        .index_of("id")
        .map_err(|_| GrustError::Schema("degree pair result missing 'id' column".into()))?;
    let in_degree_idx = schema
        .index_of("in_degree")
        .map_err(|_| GrustError::Schema("degree pair result missing 'in_degree' column".into()))?;
    let out_degree_idx = schema
        .index_of("out_degree")
        .map_err(|_| GrustError::Schema("degree pair result missing 'out_degree' column".into()))?;

    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch.map_err(|e| GrustError::Backend(format!("Arrow batch error: {e}")))?;
        let ids = string_column(&batch, id_idx, "id")?;
        let in_degrees = int64_column(&batch, in_degree_idx, "in_degree")?;
        let out_degrees = int64_column(&batch, out_degree_idx, "out_degree")?;
        for i in 0..batch.num_rows() {
            rows.push(SailDegreePairRow {
                id: NodeId::new(ids.value(i)),
                in_degree: usize_from_i64(in_degrees.value(i), "in_degree")?,
                out_degree: usize_from_i64(out_degrees.value(i), "out_degree")?,
            });
        }
    }
    Ok(rows)
}

fn string_column<'a>(batch: &'a RecordBatch, index: usize, name: &str) -> Result<&'a StringArray> {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| GrustError::Schema(format!("{name} column is not string")))
}

fn int64_column<'a>(batch: &'a RecordBatch, index: usize, name: &str) -> Result<&'a Int64Array> {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| GrustError::Schema(format!("{name} column is not int64")))
}

fn usize_from_i64(value: i64, name: &str) -> Result<usize> {
    usize::try_from(value)
        .map_err(|_| GrustError::Schema(format!("{name} value {value} cannot be represented")))
}

#[cfg(test)]
mod tests;
