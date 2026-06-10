use std::io::Cursor;
use std::sync::RwLock;

use arrow::array::{Array as _, StringArray};
use arrow::ipc::reader::StreamReader;
use async_trait::async_trait;
use grust_core::prelude::*;
use tokio::sync::Mutex;
use tonic::transport::Channel;

#[allow(clippy::all, unused_imports, dead_code)]
mod spark_connect;
use spark_connect as sc;

use sc::spark_connect_service_client::SparkConnectServiceClient;
use sc::{
    Command, ExecutePlanRequest, Plan, Relation, Sql, SqlCommand, UserContext, command,
    execute_plan_response, plan, relation,
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

// ── Store ─────────────────────────────────────────────────────────────────────

pub struct SailGraphStore {
    config: SailConfig,
    client: Mutex<SparkConnectServiceClient<Channel>>,
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
            client: Mutex::new(client),
            schema: RwLock::new(None),
        })
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    fn user_context(&self) -> UserContext {
        UserContext {
            user_id: self.config.user_id.clone(),
            user_name: self.config.user_id.clone(),
            extensions: vec![],
        }
    }

    #[allow(deprecated)]
    fn command_request(&self, sql: impl Into<String>) -> ExecutePlanRequest {
        ExecutePlanRequest {
            session_id: self.config.session_id.clone(),
            user_context: Some(self.user_context()),
            plan: Some(Plan {
                op_type: Some(plan::OpType::Command(Command {
                    command_type: Some(command::CommandType::SqlCommand(SqlCommand {
                        sql: sql.into(),
                        ..Default::default()
                    })),
                })),
            }),
            client_type: Some("grust-sail/0.1.0".to_string()),
            ..Default::default()
        }
    }

    fn query_request(&self, sql: impl Into<String>) -> ExecutePlanRequest {
        ExecutePlanRequest {
            session_id: self.config.session_id.clone(),
            user_context: Some(self.user_context()),
            plan: Some(Plan {
                op_type: Some(plan::OpType::Root(Relation {
                    common: None,
                    rel_type: Some(relation::RelType::Sql(Sql {
                        query: sql.into(),
                        ..Default::default()
                    })),
                })),
            }),
            client_type: Some("grust-sail/0.1.0".to_string()),
            ..Default::default()
        }
    }

    async fn run_command(&self, sql: &str) -> Result<()> {
        let req = self.command_request(sql);
        let mut client = self.client.lock().await;
        let mut stream = client
            .execute_plan(req)
            .await
            .map_err(|e| GrustError::Backend(format!("execute_plan failed: {e}")))?
            .into_inner();
        loop {
            match stream.message().await {
                Ok(None) => break,
                Ok(Some(_)) => {}
                Err(e) => return Err(GrustError::Backend(format!("Sail stream error: {e}"))),
            }
        }
        Ok(())
    }

    async fn run_query(&self, sql: &str) -> Result<Vec<Node>> {
        let req = self.query_request(sql);
        let mut client = self.client.lock().await;
        let mut stream = client
            .execute_plan(req)
            .await
            .map_err(|e| GrustError::Backend(format!("execute_plan failed: {e}")))?
            .into_inner();
        let mut nodes = Vec::new();
        loop {
            match stream.message().await {
                Ok(None) => break,
                Ok(Some(resp)) => {
                    if let Some(execute_plan_response::ResponseType::ArrowBatch(batch)) =
                        resp.response_type
                    {
                        if batch.row_count > 0 {
                            nodes.extend(parse_nodes_from_arrow(&batch.data)?);
                        }
                    }
                }
                Err(e) => return Err(GrustError::Backend(format!("Sail stream error: {e}"))),
            }
        }
        Ok(nodes)
    }

    async fn run_edge_query(&self, sql: &str) -> Result<Vec<Edge>> {
        let req = self.query_request(sql);
        let mut client = self.client.lock().await;
        let mut stream = client
            .execute_plan(req)
            .await
            .map_err(|e| GrustError::Backend(format!("execute_plan failed: {e}")))?
            .into_inner();
        let mut edges = Vec::new();
        loop {
            match stream.message().await {
                Ok(None) => break,
                Ok(Some(resp)) => {
                    if let Some(execute_plan_response::ResponseType::ArrowBatch(batch)) =
                        resp.response_type
                    {
                        if batch.row_count > 0 {
                            edges.extend(parse_edges_from_arrow(&batch.data)?);
                        }
                    }
                }
                Err(e) => return Err(GrustError::Backend(format!("Sail stream error: {e}"))),
            }
        }
        Ok(edges)
    }
}

// ── GraphStore ────────────────────────────────────────────────────────────────

#[async_trait]
impl GraphStore for SailGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        self.bootstrap().await?;
        for statement in sail_schema_sql(schema)? {
            self.run_command(&statement).await?;
        }
        *self.schema.write().expect("Sail schema lock poisoned") = Some(schema.clone());
        Ok(())
    }

    async fn put_node(&self, node: &Node) -> Result<NodeId> {
        let schema = self
            .schema
            .read()
            .expect("Sail schema lock poisoned")
            .clone();
        if let Some(schema) = schema.as_ref() {
            schema.validate_node(node)?;
        }
        let sql = merge_nodes_sql(std::slice::from_ref(node))?;
        self.run_command(&sql).await?;
        for sql in merge_typed_nodes_sql(schema.as_ref(), std::slice::from_ref(node))? {
            self.run_command(&sql).await?;
        }
        Ok(node.id.clone())
    }

    async fn put_edge(&self, edge: &Edge) -> Result<Option<EdgeId>> {
        let sql = merge_edges_sql(std::slice::from_ref(edge))?;
        self.run_command(&sql).await?;
        Ok(edge.id.clone())
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        let schema = self
            .schema
            .read()
            .expect("Sail schema lock poisoned")
            .clone();
        if let Some(schema) = schema.as_ref() {
            schema.validate_graph(graph)?;
        }
        let batch = self.config.batch_size.max(1);
        let mut report = LoadReport::default();
        for chunk in graph.nodes.chunks(batch) {
            let sql = merge_nodes_sql(chunk)?;
            self.run_command(&sql).await?;
            for sql in merge_typed_nodes_sql(schema.as_ref(), chunk)? {
                self.run_command(&sql).await?;
            }
            report.nodes += chunk.len();
        }
        for chunk in graph.edges.chunks(batch) {
            let sql = merge_edges_sql(chunk)?;
            self.run_command(&sql).await?;
            for sql in merge_typed_edges_sql(schema.as_ref(), chunk)? {
                self.run_command(&sql).await?;
            }
            report.edges += chunk.len();
        }
        Ok(report)
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        let sql = format!(
            "SELECT id, label, props FROM grust_nodes WHERE id = {} LIMIT 1",
            sql_str(id.as_str())
        );
        Ok(self.run_query(&sql).await?.into_iter().next())
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        let mut conditions = Vec::new();
        if let Some(from) = &query.from {
            conditions.push(format!("src_id = {}", sql_str(from.as_str())));
        }
        if let Some(to) = &query.to {
            conditions.push(format!("dst_id = {}", sql_str(to.as_str())));
        }
        if let Some(label) = &query.label {
            conditions.push(format!("edge_type = {}", sql_str(label.as_str())));
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
        self.run_edge_query(&sql).await
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        let sql = traversal_sql(&traversal)?;
        self.run_query(&sql).await
    }
}

// ── GraphAdminStore ───────────────────────────────────────────────────────────

#[async_trait]
impl GraphAdminStore for SailGraphStore {
    async fn bootstrap(&self) -> Result<()> {
        self.run_command(
            "CREATE TABLE IF NOT EXISTS grust_nodes (\
                id STRING NOT NULL, \
                label STRING NOT NULL, \
                props STRING\
            ) USING delta",
        )
        .await?;
        self.run_command(
            "CREATE TABLE IF NOT EXISTS grust_edges (\
                src_id STRING NOT NULL, \
                src_label STRING NOT NULL, \
                dst_id STRING NOT NULL, \
                dst_label STRING NOT NULL, \
                edge_type STRING NOT NULL, \
                props STRING\
            ) USING delta",
        )
        .await?;
        Ok(())
    }

    async fn clear(&self) -> Result<()> {
        self.run_command("DELETE FROM grust_nodes").await?;
        self.run_command("DELETE FROM grust_edges").await?;
        Ok(())
    }
}

// ── SQL builders ──────────────────────────────────────────────────────────────

fn merge_nodes_sql(nodes: &[Node]) -> Result<String> {
    let rows = nodes
        .iter()
        .map(|n| {
            let props = props_to_json(&n.props)?;
            Ok(format!(
                "SELECT {} AS id, {} AS label, {} AS props",
                sql_str(n.id.as_str()),
                sql_str(n.label.as_str()),
                sql_str(&props),
            ))
        })
        .collect::<Result<Vec<_>>>()?
        .join(" UNION ALL ");

    Ok(format!(
        "MERGE INTO grust_nodes AS t \
         USING ({rows}) AS s ON t.id = s.id \
         WHEN MATCHED THEN UPDATE SET t.label = s.label, t.props = s.props \
         WHEN NOT MATCHED THEN INSERT (id, label, props) VALUES (s.id, s.label, s.props)"
    ))
}

fn merge_edges_sql(edges: &[Edge]) -> Result<String> {
    let rows = edges
        .iter()
        .map(|e| {
            let props = props_to_json(&e.props)?;
            Ok(format!(
                "SELECT {} AS src_id, {} AS src_label, {} AS dst_id, {} AS dst_label, {} AS edge_type, {} AS props",
                sql_str(e.from.as_str()),
                sql_str(""),       // src_label unknown at edge time; use empty or derive
                sql_str(e.to.as_str()),
                sql_str(""),       // dst_label unknown at edge time; use empty or derive
                sql_str(e.label.as_str()),
                sql_str(&props),
            ))
        })
        .collect::<Result<Vec<_>>>()?
        .join(" UNION ALL ");

    Ok(format!(
        "MERGE INTO grust_edges AS t \
         USING ({rows}) AS s \
         ON t.src_id = s.src_id AND t.dst_id = s.dst_id AND t.edge_type = s.edge_type \
         WHEN MATCHED THEN UPDATE SET t.src_label = s.src_label, t.dst_label = s.dst_label, t.props = s.props \
         WHEN NOT MATCHED THEN INSERT (src_id, src_label, dst_id, dst_label, edge_type, props) \
           VALUES (s.src_id, s.src_label, s.dst_id, s.dst_label, s.edge_type, s.props)"
    ))
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

fn merge_typed_nodes_sql(schema: Option<&GraphSchema>, nodes: &[Node]) -> Result<Vec<String>> {
    let Some(schema) = schema else {
        return Ok(Vec::new());
    };
    schema
        .nodes
        .iter()
        .filter_map(|node_type| {
            let typed_nodes = nodes
                .iter()
                .filter(|node| node.label == node_type.label)
                .collect::<Vec<_>>();
            if typed_nodes.is_empty() {
                return None;
            }
            Some(typed_node_merge_sql(node_type, &typed_nodes))
        })
        .collect()
}

fn typed_node_merge_sql(node_type: &NodeType, nodes: &[&Node]) -> Result<String> {
    let mut insert_columns = vec!["id".to_string()];
    insert_columns.extend(
        node_type
            .fields
            .iter()
            .map(|field| sql_ident(&field.name))
            .collect::<Result<Vec<_>>>()?,
    );
    let rows = nodes
        .iter()
        .map(|node| {
            let mut columns = vec![format!("{} AS id", sql_str(node.id.as_str()))];
            columns.extend(
                node_type
                    .fields
                    .iter()
                    .map(|field| {
                        Ok(format!(
                            "{} AS {}",
                            sail_value_sql(node.props.get(&field.name), &field.ty)?,
                            sql_ident(&field.name)?
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?,
            );
            Ok(format!("SELECT {}", columns.join(", ")))
        })
        .collect::<Result<Vec<_>>>()?
        .join(" UNION ALL ");
    let updates = node_type
        .fields
        .iter()
        .map(|field| Ok(format!("t.{0} = s.{0}", sql_ident(&field.name)?)))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let update_clause = if updates.is_empty() {
        String::new()
    } else {
        format!(" WHEN MATCHED THEN UPDATE SET {updates}")
    };
    Ok(format!(
        "MERGE INTO {} AS t USING ({rows}) AS s ON t.id = s.id{update_clause} WHEN NOT MATCHED THEN INSERT ({}) VALUES ({})",
        sail_node_table(node_type.label.as_str())?,
        insert_columns.join(", "),
        insert_columns
            .iter()
            .map(|column| format!("s.{column}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn merge_typed_edges_sql(schema: Option<&GraphSchema>, edges: &[Edge]) -> Result<Vec<String>> {
    let Some(schema) = schema else {
        return Ok(Vec::new());
    };
    schema
        .edges
        .iter()
        .filter_map(|edge_type| {
            let typed_edges = edges
                .iter()
                .filter(|edge| edge.label == edge_type.label)
                .collect::<Vec<_>>();
            if typed_edges.is_empty() {
                return None;
            }
            Some(typed_edge_merge_sql(edge_type, &typed_edges))
        })
        .collect()
}

fn typed_edge_merge_sql(edge_type: &EdgeType, edges: &[&Edge]) -> Result<String> {
    let mut insert_columns = vec![
        "edge_key".to_string(),
        "id".to_string(),
        "src_id".to_string(),
        "dst_id".to_string(),
    ];
    insert_columns.extend(
        edge_type
            .fields
            .iter()
            .map(|field| sql_ident(&field.name))
            .collect::<Result<Vec<_>>>()?,
    );
    let rows = edges
        .iter()
        .map(|edge| {
            let mut columns = vec![
                format!("{} AS edge_key", sql_str(&edge_key(edge))),
                format!(
                    "{} AS id",
                    edge.id
                        .as_ref()
                        .map(|id| sql_str(id.as_str()))
                        .unwrap_or_else(|| "NULL".to_string())
                ),
                format!("{} AS src_id", sql_str(edge.from.as_str())),
                format!("{} AS dst_id", sql_str(edge.to.as_str())),
            ];
            columns.extend(
                edge_type
                    .fields
                    .iter()
                    .map(|field| {
                        Ok(format!(
                            "{} AS {}",
                            sail_value_sql(edge.props.get(&field.name), &field.ty)?,
                            sql_ident(&field.name)?
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?,
            );
            Ok(format!("SELECT {}", columns.join(", ")))
        })
        .collect::<Result<Vec<_>>>()?
        .join(" UNION ALL ");
    let updates = insert_columns
        .iter()
        .filter(|column| column.as_str() != "edge_key")
        .map(|column| format!("t.{column} = s.{column}"))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "MERGE INTO {} AS t USING ({rows}) AS s ON t.edge_key = s.edge_key WHEN MATCHED THEN UPDATE SET {updates} WHEN NOT MATCHED THEN INSERT ({}) VALUES ({})",
        sail_edge_table(edge_type.label.as_str())?,
        insert_columns.join(", "),
        insert_columns
            .iter()
            .map(|column| format!("s.{column}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn traversal_sql(traversal: &Traversal) -> Result<String> {
    if traversal.steps.is_empty() {
        // Just return the start node(s)
        let (where_clause, alias) = start_clause(&traversal.start, "n0");
        let limit = limit_clause(traversal.limit);
        return Ok(format!(
            "SELECT {alias}.id, {alias}.label, {alias}.props FROM grust_nodes {alias}{where_clause}{limit}"
        ));
    }

    let mut joins = Vec::new();
    let (start_where, _start_alias) = start_clause(&traversal.start, "n0");
    let last_node_alias = format!("n{}", traversal.steps.len());

    for (i, step) in traversal.steps.iter().enumerate() {
        let prev_node = format!("n{i}");
        let edge_alias = format!("e{i}");
        let next_node = format!("n{}", i + 1);

        let edge_type_cond = step
            .edge
            .as_ref()
            .map(|e| format!(" AND {edge_alias}.edge_type = {}", sql_str(e.as_str())))
            .unwrap_or_default();

        let next_label_cond = step
            .node
            .as_ref()
            .map(|n| format!(" AND {next_node}.label = {}", sql_str(n.as_str())))
            .unwrap_or_default();

        match &step.direction {
            Direction::Out => {
                joins.push(format!(
                    "JOIN grust_edges {edge_alias} ON {edge_alias}.src_id = {prev_node}.id AND {edge_alias}.src_label = {prev_node}.label{edge_type_cond}"
                ));
                joins.push(format!(
                    "JOIN grust_nodes {next_node} ON {next_node}.id = {edge_alias}.dst_id AND {next_node}.label = {edge_alias}.dst_label{next_label_cond}"
                ));
            }
            Direction::In => {
                joins.push(format!(
                    "JOIN grust_edges {edge_alias} ON {edge_alias}.dst_id = {prev_node}.id AND {edge_alias}.dst_label = {prev_node}.label{edge_type_cond}"
                ));
                joins.push(format!(
                    "JOIN grust_nodes {next_node} ON {next_node}.id = {edge_alias}.src_id AND {next_node}.label = {edge_alias}.src_label{next_label_cond}"
                ));
            }
            Direction::Both => {
                // Subquery unions both directions
                let edge_filter = step
                    .edge
                    .as_ref()
                    .map(|e| format!(" AND edge_type = {}", sql_str(e.as_str())))
                    .unwrap_or_default();
                joins.push(format!(
                    "JOIN (SELECT dst_id AS _nid, dst_label AS _nlabel FROM grust_edges \
                        WHERE src_id = {prev_node}.id AND src_label = {prev_node}.label{edge_filter} \
                        UNION ALL \
                        SELECT src_id AS _nid, src_label AS _nlabel FROM grust_edges \
                        WHERE dst_id = {prev_node}.id AND dst_label = {prev_node}.label{edge_filter}\
                    ) {edge_alias} ON TRUE"
                ));
                joins.push(format!(
                    "JOIN grust_nodes {next_node} ON {next_node}.id = {edge_alias}._nid AND {next_node}.label = {edge_alias}._nlabel{next_label_cond}"
                ));
            }
        }
    }

    let limit = limit_clause(traversal.limit);
    let join_str = joins.join(" ");
    Ok(format!(
        "SELECT {last_node_alias}.id, {last_node_alias}.label, {last_node_alias}.props \
         FROM grust_nodes n0 {join_str}{start_where}{limit}"
    ))
}

fn start_clause(start: &Start, alias: &str) -> (String, String) {
    let alias = alias.to_string();
    match start {
        Start::Node(id) => (
            format!(" WHERE {alias}.id = {}", sql_str(id.as_str())),
            alias,
        ),
        Start::NodesByLabel(label) => (
            format!(" WHERE {alias}.label = {}", sql_str(label.as_str())),
            alias,
        ),
        Start::NodesByProperty { label, key, value } => {
            let val_expr = match value {
                Value::String(s) => format!(
                    "GET_JSON_OBJECT({alias}.props, '$.{}') = {}",
                    key,
                    sql_str(s)
                ),
                Value::Int(n) => {
                    format!("CAST(GET_JSON_OBJECT({alias}.props, '$.{key}') AS BIGINT) = {n}")
                }
                Value::Float(f) => {
                    format!("CAST(GET_JSON_OBJECT({alias}.props, '$.{key}') AS DOUBLE) = {f}")
                }
                Value::Bool(b) => {
                    format!("CAST(GET_JSON_OBJECT({alias}.props, '$.{key}') AS BOOLEAN) = {b}")
                }
                _ => format!("GET_JSON_OBJECT({alias}.props, '$.{key}') IS NOT NULL"),
            };
            (
                format!(
                    " WHERE {alias}.label = {} AND {val_expr}",
                    sql_str(label.as_str())
                ),
                alias,
            )
        }
    }
}

fn sail_sql_type(ty: &FieldType) -> &'static str {
    match ty {
        FieldType::String | FieldType::DateTime | FieldType::StringArray | FieldType::Json => {
            "STRING"
        }
        FieldType::Int => "BIGINT",
        FieldType::Float => "DOUBLE",
        FieldType::Bool => "BOOLEAN",
    }
}

fn sail_value_sql(value: Option<&Value>, ty: &FieldType) -> Result<String> {
    Ok(match (value, ty) {
        (None, _) | (Some(Value::Null), _) => "NULL".to_string(),
        (Some(Value::String(value)), FieldType::String | FieldType::DateTime) => sql_str(value),
        (Some(Value::Int(value)), FieldType::Int) => value.to_string(),
        (Some(Value::Float(value)), FieldType::Float) => value.to_string(),
        (Some(Value::Bool(value)), FieldType::Bool) => value.to_string(),
        (Some(Value::StringArray(_)), FieldType::StringArray)
        | (Some(Value::Json(_)), FieldType::Json) => sql_str(
            &serde_json::to_string(value.expect("matched value"))
                .map_err(|err| GrustError::Serialization(err.to_string()))?,
        ),
        (Some(other), _) => {
            return Err(GrustError::Schema(format!(
                "cannot store value {other:?} as Sail typed column {ty:?}"
            )));
        }
    })
}

fn sail_node_table(label: &str) -> Result<String> {
    Ok(format!("grust_node_{}", schema_identifier(label)?))
}

fn sail_edge_table(label: &str) -> Result<String> {
    Ok(format!("grust_edge_{}", schema_identifier(label)?))
}

fn edge_key(edge: &Edge) -> String {
    edge.id
        .as_ref()
        .map(EdgeId::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            format!(
                "{}\u{1f}{}\u{1f}{}",
                edge.from.as_str(),
                edge.label.as_str(),
                edge.to.as_str()
            )
        })
}

fn schema_identifier(value: &str) -> Result<String> {
    let identifier = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    if identifier.is_empty()
        || identifier
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_digit())
    {
        return Err(GrustError::Schema(format!(
            "invalid schema identifier '{value}'"
        )));
    }
    Ok(identifier)
}

fn sql_ident(value: &str) -> Result<String> {
    let identifier = schema_identifier(value)?;
    Ok(format!("`{identifier}`"))
}

fn limit_clause(limit: Option<u32>) -> String {
    limit.map(|n| format!(" LIMIT {n}")).unwrap_or_default()
}

fn props_to_json(props: &Props) -> Result<String> {
    serde_json::to_string(props).map_err(|e| GrustError::Serialization(e.to_string()))
}

fn sql_str(s: &str) -> String {
    // Escape single quotes by doubling them (Spark SQL style)
    format!("'{}'", s.replace('\'', "''"))
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
            let props: Props = if props_col.is_null(i) || props_col.value(i).is_empty() {
                Props::new()
            } else {
                serde_json::from_str(props_col.value(i))
                    .map_err(|e| GrustError::Serialization(format!("props JSON parse: {e}")))?
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

        for i in 0..batch.num_rows() {
            let props: Props = if props_col.is_null(i) || props_col.value(i).is_empty() {
                Props::new()
            } else {
                serde_json::from_str(props_col.value(i))
                    .map_err(|e| GrustError::Serialization(format!("props JSON parse: {e}")))?
            };
            edges.push(Edge {
                id: None,
                from: NodeId::new(src_ids.value(i)),
                to: NodeId::new(dst_ids.value(i)),
                label: Label::new(edge_types.value(i)),
                props,
            });
        }
    }
    Ok(edges)
}

#[cfg(test)]
mod tests;
