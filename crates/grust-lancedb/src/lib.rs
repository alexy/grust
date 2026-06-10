use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

use arrow::array::{Array as _, BooleanArray, Float64Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::{RecordBatch, RecordBatchIterator};
use async_trait::async_trait;
use futures::TryStreamExt;
use grust_core::prelude::*;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{Connection, Error as LanceError, Table};

#[derive(Clone, Debug)]
pub struct LanceDbConfig {
    pub uri: String,
    pub table_prefix: String,
    pub batch_size: usize,
}

impl Default for LanceDbConfig {
    fn default() -> Self {
        Self {
            uri: "data/grust-lancedb".to_string(),
            table_prefix: "grust".to_string(),
            batch_size: 500,
        }
    }
}

#[derive(Clone)]
pub struct LanceDbGraphStore {
    config: LanceDbConfig,
    db: Connection,
    schema: Arc<RwLock<Option<GraphSchema>>>,
}

impl LanceDbGraphStore {
    pub async fn connect(config: LanceDbConfig) -> Result<Self> {
        validate_table_prefix(&config.table_prefix)?;
        let db = lancedb::connect(&config.uri)
            .execute()
            .await
            .map_err(|err| {
                GrustError::Backend(format!(
                    "failed to connect to LanceDB at {}: {err}",
                    config.uri
                ))
            })?;
        Ok(Self {
            config,
            db,
            schema: Arc::new(RwLock::new(None)),
        })
    }

    pub fn config(&self) -> &LanceDbConfig {
        &self.config
    }

    fn nodes_table_name(&self) -> String {
        format!("{}_nodes", self.config.table_prefix)
    }

    fn edges_table_name(&self) -> String {
        format!("{}_edges", self.config.table_prefix)
    }

    async fn open_nodes(&self) -> Result<Table> {
        self.open_table(&self.nodes_table_name()).await
    }

    async fn open_edges(&self) -> Result<Table> {
        self.open_table(&self.edges_table_name()).await
    }

    async fn open_table(&self, name: &str) -> Result<Table> {
        self.db.open_table(name).execute().await.map_err(|err| {
            GrustError::Backend(format!("failed to open LanceDB table {name}: {err}"))
        })
    }

    async fn table_exists(&self, name: &str) -> Result<bool> {
        let names =
            self.db.table_names().execute().await.map_err(|err| {
                GrustError::Backend(format!("failed to list LanceDB tables: {err}"))
            })?;
        Ok(names.iter().any(|existing| existing == name))
    }

    async fn query_nodes(&self, filter: Option<String>, limit: Option<u32>) -> Result<Vec<Node>> {
        let table = self.open_nodes().await?;
        let mut query = table.query();
        if let Some(filter) = filter {
            query = query.only_if(filter);
        }
        if let Some(limit) = limit {
            query = query.limit(limit as usize);
        }
        let batches = query
            .execute()
            .await
            .map_err(|err| GrustError::Backend(format!("LanceDB node query failed: {err}")))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|err| GrustError::Backend(format!("LanceDB node stream failed: {err}")))?;
        batches_to_nodes(&batches)
    }

    async fn query_edges(&self, filter: Option<String>) -> Result<Vec<Edge>> {
        let table = self.open_edges().await?;
        let mut query = table.query();
        if let Some(filter) = filter {
            query = query.only_if(filter);
        }
        let batches = query
            .execute()
            .await
            .map_err(|err| GrustError::Backend(format!("LanceDB edge query failed: {err}")))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|err| GrustError::Backend(format!("LanceDB edge stream failed: {err}")))?;
        batches_to_edges(&batches)
    }

    async fn put_nodes_batch(&self, nodes: &[Node]) -> Result<()> {
        if nodes.is_empty() {
            return Ok(());
        }
        let table = self.open_nodes().await?;
        let data = node_batch_reader(nodes)?;
        let mut merge = table.merge_insert(&["id"]);
        merge
            .when_matched_update_all(None)
            .when_not_matched_insert_all();
        merge.execute(data).await.map_err(|err| {
            GrustError::Backend(format!("LanceDB node merge_insert failed: {err}"))
        })?;
        Ok(())
    }

    async fn put_edges_batch(&self, edges: &[Edge]) -> Result<()> {
        if edges.is_empty() {
            return Ok(());
        }
        let table = self.open_edges().await?;
        let data = edge_batch_reader(edges)?;
        let mut merge = table.merge_insert(&["key"]);
        merge
            .when_matched_update_all(None)
            .when_not_matched_insert_all();
        merge.execute(data).await.map_err(|err| {
            GrustError::Backend(format!("LanceDB edge merge_insert failed: {err}"))
        })?;
        Ok(())
    }
}

#[async_trait]
impl GraphStore for LanceDbGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        self.bootstrap().await?;
        for node_type in &schema.nodes {
            let table = self.typed_node_table_name(node_type.label.as_str())?;
            if !self.table_exists(&table).await? {
                self.db
                    .create_empty_table(&table, typed_node_schema(node_type))
                    .execute()
                    .await
                    .map_err(|err| {
                        GrustError::Backend(format!(
                            "failed to create LanceDB typed node table {table}: {err}"
                        ))
                    })?;
            }
        }
        for edge_type in &schema.edges {
            let table = self.typed_edge_table_name(edge_type.label.as_str())?;
            if !self.table_exists(&table).await? {
                self.db
                    .create_empty_table(&table, typed_edge_schema(edge_type))
                    .execute()
                    .await
                    .map_err(|err| {
                        GrustError::Backend(format!(
                            "failed to create LanceDB typed edge table {table}: {err}"
                        ))
                    })?;
            }
        }
        *self.schema.write().expect("LanceDB schema lock poisoned") = Some(schema.clone());
        Ok(())
    }

    async fn put_node(&self, node: &Node) -> Result<NodeId> {
        if let Some(schema) = self
            .schema
            .read()
            .expect("LanceDB schema lock poisoned")
            .as_ref()
        {
            schema.validate_node(node)?;
        }
        self.put_nodes_batch(std::slice::from_ref(node)).await?;
        self.put_typed_nodes_batch(std::slice::from_ref(node))
            .await?;
        Ok(node.id.clone())
    }

    async fn put_edge(&self, edge: &Edge) -> Result<Option<EdgeId>> {
        self.put_edges_batch(std::slice::from_ref(edge)).await?;
        self.put_typed_edges_batch(std::slice::from_ref(edge))
            .await?;
        Ok(edge.id.clone())
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        if let Some(schema) = self
            .schema
            .read()
            .expect("LanceDB schema lock poisoned")
            .as_ref()
        {
            schema.validate_graph(graph)?;
        }
        let batch_size = self.config.batch_size.max(1);
        let mut report = LoadReport::default();
        for chunk in graph.nodes.chunks(batch_size) {
            self.put_nodes_batch(chunk).await?;
            self.put_typed_nodes_batch(chunk).await?;
            report.nodes += chunk.len();
        }
        for chunk in graph.edges.chunks(batch_size) {
            self.put_edges_batch(chunk).await?;
            self.put_typed_edges_batch(chunk).await?;
            report.edges += chunk.len();
        }
        Ok(report)
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        Ok(self
            .query_nodes(Some(format!("id = {}", sql_str(id.as_str()))), Some(1))
            .await?
            .into_iter()
            .next())
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        self.query_edges(edge_query_filter(query)).await
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        let mut current = self
            .query_nodes(Some(start_filter(&traversal.start)?), traversal.limit)
            .await?;

        for step in traversal.steps {
            let mut next_ids = BTreeSet::new();
            for node in &current {
                let edges = self
                    .query_edges(Some(step_edge_filter(node.id.as_str(), &step)))
                    .await?;
                for edge in edges {
                    match step.direction {
                        Direction::Out => {
                            next_ids.insert(edge.to);
                        }
                        Direction::In => {
                            next_ids.insert(edge.from);
                        }
                        Direction::Both => {
                            if edge.from == node.id {
                                next_ids.insert(edge.to);
                            } else {
                                next_ids.insert(edge.from);
                            }
                        }
                    }
                }
            }

            let mut next = Vec::new();
            for id in next_ids {
                if let Some(node) = self.get_node(&id).await? {
                    if step.node.as_ref().is_none_or(|label| label == &node.label) {
                        next.push(node);
                    }
                }
            }
            if let Some(limit) = traversal.limit {
                next.truncate(limit as usize);
            }
            current = next;
        }

        if let Some(limit) = traversal.limit {
            current.truncate(limit as usize);
        }
        Ok(current)
    }
}

#[async_trait]
impl GraphAdminStore for LanceDbGraphStore {
    async fn bootstrap(&self) -> Result<()> {
        let nodes = self.nodes_table_name();
        if !self.table_exists(&nodes).await? {
            self.db
                .create_empty_table(&nodes, nodes_schema())
                .execute()
                .await
                .map_err(|err| {
                    GrustError::Backend(format!("failed to create LanceDB table {nodes}: {err}"))
                })?;
        }

        let edges = self.edges_table_name();
        if !self.table_exists(&edges).await? {
            self.db
                .create_empty_table(&edges, edges_schema())
                .execute()
                .await
                .map_err(|err| {
                    GrustError::Backend(format!("failed to create LanceDB table {edges}: {err}"))
                })?;
        }

        Ok(())
    }

    async fn clear(&self) -> Result<()> {
        self.drop_table_if_exists(&self.edges_table_name()).await?;
        self.drop_table_if_exists(&self.nodes_table_name()).await?;
        self.bootstrap().await
    }
}

impl LanceDbGraphStore {
    async fn drop_table_if_exists(&self, name: &str) -> Result<()> {
        match self.db.drop_table(name, &[]).await {
            Ok(()) => Ok(()),
            Err(LanceError::TableNotFound { .. }) => Ok(()),
            Err(err) => Err(GrustError::Backend(format!(
                "failed to drop LanceDB table {name}: {err}"
            ))),
        }
    }

    fn typed_node_table_name(&self, label: &str) -> Result<String> {
        Ok(format!(
            "{}_node_{}",
            self.config.table_prefix,
            schema_identifier(label)?
        ))
    }

    fn typed_edge_table_name(&self, label: &str) -> Result<String> {
        Ok(format!(
            "{}_edge_{}",
            self.config.table_prefix,
            schema_identifier(label)?
        ))
    }

    async fn put_typed_nodes_batch(&self, nodes: &[Node]) -> Result<()> {
        let schema = self
            .schema
            .read()
            .expect("LanceDB schema lock poisoned")
            .clone();
        let Some(schema) = schema else {
            return Ok(());
        };

        for node_type in &schema.nodes {
            let typed_nodes = nodes
                .iter()
                .filter(|node| node.label == node_type.label)
                .collect::<Vec<_>>();
            if typed_nodes.is_empty() {
                continue;
            }
            let table = self
                .open_table(&self.typed_node_table_name(node_type.label.as_str())?)
                .await?;
            let data = typed_node_batch_reader(node_type, &typed_nodes)?;
            let mut merge = table.merge_insert(&["id"]);
            merge
                .when_matched_update_all(None)
                .when_not_matched_insert_all();
            merge.execute(data).await.map_err(|err| {
                GrustError::Backend(format!(
                    "LanceDB typed node merge_insert failed for {}: {err}",
                    node_type.label.as_str()
                ))
            })?;
        }
        Ok(())
    }

    async fn put_typed_edges_batch(&self, edges: &[Edge]) -> Result<()> {
        let schema = self
            .schema
            .read()
            .expect("LanceDB schema lock poisoned")
            .clone();
        let Some(schema) = schema else {
            return Ok(());
        };

        for edge_type in &schema.edges {
            let typed_edges = edges
                .iter()
                .filter(|edge| edge.label == edge_type.label)
                .collect::<Vec<_>>();
            if typed_edges.is_empty() {
                continue;
            }
            let table = self
                .open_table(&self.typed_edge_table_name(edge_type.label.as_str())?)
                .await?;
            let data = typed_edge_batch_reader(edge_type, &typed_edges)?;
            let mut merge = table.merge_insert(&["key"]);
            merge
                .when_matched_update_all(None)
                .when_not_matched_insert_all();
            merge.execute(data).await.map_err(|err| {
                GrustError::Backend(format!(
                    "LanceDB typed edge merge_insert failed for {}: {err}",
                    edge_type.label.as_str()
                ))
            })?;
        }
        Ok(())
    }
}

fn nodes_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("label", DataType::Utf8, false),
        Field::new("props", DataType::Utf8, false),
    ]))
}

fn edges_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("key", DataType::Utf8, false),
        Field::new("id", DataType::Utf8, true),
        Field::new("from_id", DataType::Utf8, false),
        Field::new("to_id", DataType::Utf8, false),
        Field::new("label", DataType::Utf8, false),
        Field::new("props", DataType::Utf8, false),
    ]))
}

fn typed_node_schema(node_type: &NodeType) -> SchemaRef {
    let mut fields = vec![Field::new("id", DataType::Utf8, false)];
    fields.extend(node_type.fields.iter().map(arrow_field));
    Arc::new(Schema::new(fields))
}

fn typed_edge_schema(edge_type: &EdgeType) -> SchemaRef {
    let mut fields = vec![
        Field::new("key", DataType::Utf8, false),
        Field::new("id", DataType::Utf8, true),
        Field::new("from_id", DataType::Utf8, false),
        Field::new("to_id", DataType::Utf8, false),
    ];
    fields.extend(edge_type.fields.iter().map(arrow_field));
    Arc::new(Schema::new(fields))
}

fn arrow_field(field: &grust_core::Field) -> Field {
    Field::new(
        &field.name,
        match field.ty {
            FieldType::String | FieldType::DateTime | FieldType::StringArray | FieldType::Json => {
                DataType::Utf8
            }
            FieldType::Int => DataType::Int64,
            FieldType::Float => DataType::Float64,
            FieldType::Bool => DataType::Boolean,
        },
        !field.required,
    )
}

fn node_batch_reader(nodes: &[Node]) -> Result<Box<dyn arrow::array::RecordBatchReader + Send>> {
    let schema = nodes_schema();
    let props = nodes
        .iter()
        .map(|node| props_to_json(&node.props))
        .collect::<Result<Vec<_>>>()?;
    let batch = RecordBatch::try_new(
        schema.clone(),
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
    .map_err(|err| GrustError::Serialization(format!("failed to build node batch: {err}")))?;
    Ok(Box::new(RecordBatchIterator::new(
        vec![Ok(batch)].into_iter(),
        schema,
    )))
}

fn edge_batch_reader(edges: &[Edge]) -> Result<Box<dyn arrow::array::RecordBatchReader + Send>> {
    let schema = edges_schema();
    let props = edges
        .iter()
        .map(|edge| props_to_json(&edge.props))
        .collect::<Result<Vec<_>>>()?;
    let keys = edges.iter().map(edge_key).collect::<Vec<_>>();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from_iter_values(
                keys.iter().map(String::as_str),
            )),
            Arc::new(StringArray::from(
                edges
                    .iter()
                    .map(|edge| edge.id.as_ref().map(EdgeId::as_str))
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from_iter_values(
                edges.iter().map(|edge| edge.from.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                edges.iter().map(|edge| edge.to.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                edges.iter().map(|edge| edge.label.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                props.iter().map(String::as_str),
            )),
        ],
    )
    .map_err(|err| GrustError::Serialization(format!("failed to build edge batch: {err}")))?;
    Ok(Box::new(RecordBatchIterator::new(
        vec![Ok(batch)].into_iter(),
        schema,
    )))
}

fn typed_node_batch_reader(
    node_type: &NodeType,
    nodes: &[&Node],
) -> Result<Box<dyn arrow::array::RecordBatchReader + Send>> {
    let schema = typed_node_schema(node_type);
    let mut arrays: Vec<Arc<dyn arrow::array::Array>> = vec![Arc::new(
        StringArray::from_iter_values(nodes.iter().map(|node| node.id.as_str())),
    )];
    arrays.extend(
        node_type
            .fields
            .iter()
            .map(|field| typed_prop_array(field, nodes.iter().map(|node| &node.props)))
            .collect::<Result<Vec<_>>>()?,
    );
    let batch = RecordBatch::try_new(schema.clone(), arrays).map_err(|err| {
        GrustError::Serialization(format!("failed to build typed node batch: {err}"))
    })?;
    Ok(Box::new(RecordBatchIterator::new(
        vec![Ok(batch)].into_iter(),
        schema,
    )))
}

fn typed_edge_batch_reader(
    edge_type: &EdgeType,
    edges: &[&Edge],
) -> Result<Box<dyn arrow::array::RecordBatchReader + Send>> {
    let schema = typed_edge_schema(edge_type);
    let keys = edges.iter().map(|edge| edge_key(edge)).collect::<Vec<_>>();
    let mut arrays: Vec<Arc<dyn arrow::array::Array>> = vec![
        Arc::new(StringArray::from_iter_values(
            keys.iter().map(String::as_str),
        )),
        Arc::new(StringArray::from(
            edges
                .iter()
                .map(|edge| edge.id.as_ref().map(EdgeId::as_str))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from_iter_values(
            edges.iter().map(|edge| edge.from.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            edges.iter().map(|edge| edge.to.as_str()),
        )),
    ];
    arrays.extend(
        edge_type
            .fields
            .iter()
            .map(|field| typed_prop_array(field, edges.iter().map(|edge| &edge.props)))
            .collect::<Result<Vec<_>>>()?,
    );
    let batch = RecordBatch::try_new(schema.clone(), arrays).map_err(|err| {
        GrustError::Serialization(format!("failed to build typed edge batch: {err}"))
    })?;
    Ok(Box::new(RecordBatchIterator::new(
        vec![Ok(batch)].into_iter(),
        schema,
    )))
}

fn typed_prop_array<'a>(
    field: &grust_core::Field,
    props: impl Iterator<Item = &'a Props>,
) -> Result<Arc<dyn arrow::array::Array>> {
    let values = props
        .map(|props| props.get(&field.name))
        .collect::<Vec<_>>();
    Ok(match field.ty {
        FieldType::String | FieldType::DateTime => Arc::new(StringArray::from(
            values
                .iter()
                .map(|value| value.and_then(Value::as_str))
                .collect::<Vec<_>>(),
        )),
        FieldType::Int => Arc::new(Int64Array::from(
            values
                .iter()
                .map(|value| match value {
                    Some(Value::Int(value)) => Some(*value),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        )),
        FieldType::Float => Arc::new(Float64Array::from(
            values
                .iter()
                .map(|value| match value {
                    Some(Value::Float(value)) => Some(*value),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        )),
        FieldType::Bool => Arc::new(BooleanArray::from(
            values
                .iter()
                .map(|value| match value {
                    Some(Value::Bool(value)) => Some(*value),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        )),
        FieldType::StringArray | FieldType::Json => Arc::new(StringArray::from(
            values
                .iter()
                .map(|value| {
                    value
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(|err| GrustError::Serialization(err.to_string()))
                })
                .collect::<Result<Vec<_>>>()?,
        )),
    })
}

fn batches_to_nodes(batches: &[RecordBatch]) -> Result<Vec<Node>> {
    let mut nodes = Vec::new();
    for batch in batches {
        let ids = string_column(batch, "id")?;
        let labels = string_column(batch, "label")?;
        let props = string_column(batch, "props")?;
        for row in 0..batch.num_rows() {
            nodes.push(Node {
                id: NodeId::new(ids.value(row)),
                label: Label::new(labels.value(row)),
                props: parse_props(props.value(row))?,
            });
        }
    }
    Ok(nodes)
}

fn batches_to_edges(batches: &[RecordBatch]) -> Result<Vec<Edge>> {
    let mut edges = Vec::new();
    for batch in batches {
        let ids = string_column(batch, "id")?;
        let from_ids = string_column(batch, "from_id")?;
        let to_ids = string_column(batch, "to_id")?;
        let labels = string_column(batch, "label")?;
        let props = string_column(batch, "props")?;
        for row in 0..batch.num_rows() {
            let mut edge = Edge::new(
                labels.value(row),
                from_ids.value(row),
                to_ids.value(row),
                parse_props(props.value(row))?,
            );
            if !ids.is_null(row) {
                edge.id = Some(EdgeId::new(ids.value(row)));
            }
            edges.push(edge);
        }
    }
    Ok(edges)
}

fn string_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    let index = batch
        .schema()
        .index_of(name)
        .map_err(|_| GrustError::Schema(format!("LanceDB batch missing '{name}' column")))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| GrustError::Schema(format!("LanceDB column '{name}' is not Utf8")))
}

fn edge_query_filter(query: EdgeQuery) -> Option<String> {
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
    if conditions.is_empty() {
        None
    } else {
        Some(conditions.join(" AND "))
    }
}

fn start_filter(start: &Start) -> Result<String> {
    match start {
        Start::Node(id) => Ok(format!("id = {}", sql_str(id.as_str()))),
        Start::NodesByLabel(label) => Ok(format!("label = {}", sql_str(label.as_str()))),
        Start::NodesByProperty { label, key, value } => {
            let encoded = props_to_json(&Props::from([(key.clone(), value.clone())]))?;
            Ok(format!(
                "label = {} AND props LIKE {}",
                sql_str(label.as_str()),
                sql_str(&format!("%{}%", json_property_fragment(key, &encoded)?))
            ))
        }
    }
}

fn step_edge_filter(node_id: &str, step: &Step) -> String {
    let endpoint = match step.direction {
        Direction::Out => format!("from_id = {}", sql_str(node_id)),
        Direction::In => format!("to_id = {}", sql_str(node_id)),
        Direction::Both => format!(
            "(from_id = {} OR to_id = {})",
            sql_str(node_id),
            sql_str(node_id)
        ),
    };
    if let Some(label) = &step.edge {
        format!("{endpoint} AND label = {}", sql_str(label.as_str()))
    } else {
        endpoint
    }
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

fn json_property_fragment(key: &str, encoded_single_prop: &str) -> Result<String> {
    let value = serde_json::from_str::<serde_json::Value>(encoded_single_prop)
        .map_err(|err| GrustError::Serialization(err.to_string()))?;
    let value = value
        .as_object()
        .and_then(|object| object.get(key))
        .ok_or_else(|| GrustError::Serialization("encoded property missing key".to_string()))?;
    Ok(format!("{}:{}", serde_json::to_string(key).unwrap(), value))
}

fn props_to_json(props: &Props) -> Result<String> {
    serde_json::to_string(props).map_err(|err| GrustError::Serialization(err.to_string()))
}

fn parse_props(value: &str) -> Result<Props> {
    serde_json::from_str(value)
        .map_err(|err| GrustError::Serialization(format!("props JSON parse failed: {err}")))
}

fn sql_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn validate_table_prefix(prefix: &str) -> Result<()> {
    if prefix.is_empty()
        || !prefix
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        || prefix.chars().next().is_some_and(|ch| ch.is_ascii_digit())
    {
        return Err(GrustError::Schema(format!(
            "invalid LanceDB table prefix '{prefix}'"
        )));
    }
    Ok(())
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
    validate_table_prefix(&identifier)?;
    Ok(identifier)
}

#[cfg(test)]
mod tests;
