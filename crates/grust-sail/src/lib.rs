use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::sync::{Arc, RwLock};

use arrow::array::{Array as _, Int64Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field as ArrowField, Schema as ArrowSchema};
use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use async_trait::async_trait;
use grust_core::prelude::*;
// Internal access to the full shared Cypher language layer (parser, planner,
// returning evaluator, errors, and the gql/lexer/parser/ast/semantics modules).
use grust_cypher::*;
// Explicit public re-export of the portable Cypher API that Sail executes, so
// `grust_sail::cypher_*` stays available without re-exporting the entire crate.
pub use grust_cypher::{
    CypherCatalogSnapshot, CypherConstraintRegistry, CypherCreateMode, CypherDdlApplicationReport,
    CypherDdlStatement, CypherGeneratedNodeId, CypherMutationOptions, CypherMutationReport,
    CypherMutationResult, CypherMutationTableResult, CypherNodeIdPolicy, CypherNullAssignment,
    CypherParameters, CypherRelationshipIdPolicy, CypherResultTable, CypherSchemaApplication,
    CypherSchemaManager, CypherSession, CypherWrittenEdgeIdentity, CypherWrittenNodeIdentity,
    GraphIndexDefinition, GraphIndexElement, GraphTypeDefinition, NamedGraphCatalog,
    NamedGraphConstraint, NamedGraphIndex, NamedGraphType, SessionCommand,
    apply_cypher_ddl_to_schema, apply_cypher_native_constraints, cypher_catalog_procedure,
    cypher_constraints, cypher_ddl, cypher_mutation_plan, cypher_mutation_plan_with_options,
    cypher_mutation_plan_with_return_options, ensure_catalog_graph_selection,
    ensure_query_uses_graph, execute_cypher_mutation_returning_with_options_on_store,
    query_graph_selection, run_read_query_on_named_graph, sail_cypher_constraints, sail_cypher_ddl,
    sail_cypher_mutation_plan, sail_cypher_mutation_plan_with_options,
    sail_cypher_mutation_plan_with_return_options,
};
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

#[derive(Clone, Debug, PartialEq)]
pub struct SailTripletRow {
    pub src: Node,
    pub edge: Edge,
    pub dst: Node,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SailGraphPatternDirection {
    Outgoing,
    Incoming,
    Undirected,
}

// ── Store ─────────────────────────────────────────────────────────────────────

/// Session-scoped temp views used to stage Arrow batches before MERGE.
const NODE_STAGE_VIEW: &str = "grust_stage_nodes";
const EDGE_STAGE_VIEW: &str = "grust_stage_edges";
const DELETE_NODE_STAGE_VIEW: &str = "grust_delete_node_ids";
const DELETE_EDGE_STAGE_VIEW: &str = "grust_delete_edges";
pub const GRUST_NODES_TABLE: &str = "grust_nodes";
pub const GRUST_EDGES_TABLE: &str = "grust_edges";
pub const CYPHER_CONSTRAINT_REGISTRY_TABLE: &str = "grust_cypher_constraint_registry";
pub const NODE_ID_COLUMN: &str = "id";
pub const NODE_LABEL_COLUMN: &str = "label";
pub const NODE_PROPS_COLUMN: &str = "props";
pub const EDGE_ID_COLUMN: &str = "id";
pub const EDGE_KEY_COLUMN: &str = "edge_key";
pub const EDGE_SRC_ID_COLUMN: &str = "src_id";
pub const EDGE_SRC_LABEL_COLUMN: &str = "src_label";
pub const EDGE_DST_ID_COLUMN: &str = "dst_id";
pub const EDGE_DST_LABEL_COLUMN: &str = "dst_label";
pub const EDGE_TYPE_COLUMN: &str = "edge_type";
pub const EDGE_PROPS_COLUMN: &str = "props";
pub const GRAPH_TABLE_KIND_PROPERTY: &str = "grust.graph.kind";
pub const GRAPH_TABLE_LABEL_PROPERTY: &str = "grust.graph.label";
pub const GRAPH_TABLE_KIND_NODE: &str = "node";
pub const GRAPH_TABLE_KIND_EDGE: &str = "edge";
const DROP_NODES_SQL: &str = "DROP TABLE IF EXISTS grust_nodes";
const DROP_EDGES_SQL: &str = "DROP TABLE IF EXISTS grust_edges";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SailGraphFieldProjection {
    PhysicalColumn(&'static str),
    JsonProperty(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SailGraphTypedTableKind {
    Node,
    Edge,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SailGraphTypedTable {
    pub kind: SailGraphTypedTableKind,
    pub label: String,
    pub table: String,
    pub columns: Vec<String>,
}

pub fn sail_node_field_projection(field: &str) -> SailGraphFieldProjection {
    match field {
        NODE_ID_COLUMN => SailGraphFieldProjection::PhysicalColumn(NODE_ID_COLUMN),
        NODE_LABEL_COLUMN => SailGraphFieldProjection::PhysicalColumn(NODE_LABEL_COLUMN),
        NODE_PROPS_COLUMN => SailGraphFieldProjection::PhysicalColumn(NODE_PROPS_COLUMN),
        _ => SailGraphFieldProjection::JsonProperty(field.to_string()),
    }
}

pub fn sail_edge_field_projection(field: &str) -> SailGraphFieldProjection {
    match field {
        EDGE_ID_COLUMN => SailGraphFieldProjection::PhysicalColumn(EDGE_ID_COLUMN),
        EDGE_KEY_COLUMN => SailGraphFieldProjection::PhysicalColumn(EDGE_KEY_COLUMN),
        EDGE_SRC_ID_COLUMN => SailGraphFieldProjection::PhysicalColumn(EDGE_SRC_ID_COLUMN),
        EDGE_SRC_LABEL_COLUMN => SailGraphFieldProjection::PhysicalColumn(EDGE_SRC_LABEL_COLUMN),
        EDGE_DST_ID_COLUMN => SailGraphFieldProjection::PhysicalColumn(EDGE_DST_ID_COLUMN),
        EDGE_DST_LABEL_COLUMN => SailGraphFieldProjection::PhysicalColumn(EDGE_DST_LABEL_COLUMN),
        EDGE_TYPE_COLUMN | NODE_LABEL_COLUMN => {
            SailGraphFieldProjection::PhysicalColumn(EDGE_TYPE_COLUMN)
        }
        EDGE_PROPS_COLUMN => SailGraphFieldProjection::PhysicalColumn(EDGE_PROPS_COLUMN),
        _ => SailGraphFieldProjection::JsonProperty(field.to_string()),
    }
}

pub fn sail_json_property_expr(props_column: &str, key: &str) -> Result<String> {
    validate_json_key(key)?;
    Ok(format!("GET_JSON_OBJECT({props_column}, '$.{key}')"))
}

pub fn sail_node_table(label: &str) -> Result<String> {
    Ok(format!("grust_node_{}", schema_identifier(label)?))
}

pub fn sail_edge_table(label: &str) -> Result<String> {
    Ok(format!("grust_edge_{}", schema_identifier(label)?))
}

pub fn sail_typed_node_columns(node_type: &NodeType) -> Result<Vec<String>> {
    let mut columns = vec![NODE_ID_COLUMN.to_string()];
    for field in &node_type.fields {
        sql_ident(&field.name)?;
        columns.push(field.name.clone());
    }
    Ok(columns)
}

pub fn sail_typed_edge_columns(edge_type: &EdgeType) -> Result<Vec<String>> {
    let mut columns = vec![
        EDGE_KEY_COLUMN.to_string(),
        EDGE_ID_COLUMN.to_string(),
        EDGE_SRC_ID_COLUMN.to_string(),
        EDGE_DST_ID_COLUMN.to_string(),
    ];
    for field in &edge_type.fields {
        sql_ident(&field.name)?;
        columns.push(field.name.clone());
    }
    Ok(columns)
}

pub fn sail_graph_schema_typed_tables(schema: &GraphSchema) -> Result<Vec<SailGraphTypedTable>> {
    let mut tables = Vec::new();
    for node_type in &schema.nodes {
        tables.push(SailGraphTypedTable {
            kind: SailGraphTypedTableKind::Node,
            label: node_type.label.as_str().to_string(),
            table: sail_node_table(node_type.label.as_str())?,
            columns: sail_typed_node_columns(node_type)?,
        });
    }
    for edge_type in &schema.edges {
        tables.push(SailGraphTypedTable {
            kind: SailGraphTypedTableKind::Edge,
            label: edge_type.label.as_str().to_string(),
            table: sail_edge_table(edge_type.label.as_str())?,
            columns: sail_typed_edge_columns(edge_type)?,
        });
    }
    Ok(tables)
}

pub fn sail_typed_node_field_compatible(field: &str) -> bool {
    field != NODE_PROPS_COLUMN
}

pub fn sail_typed_edge_field_compatible(field: &str) -> bool {
    !matches!(
        field,
        EDGE_SRC_LABEL_COLUMN | EDGE_DST_LABEL_COLUMN | EDGE_PROPS_COLUMN
    )
}

pub fn sail_typed_node_table_has_fields<F, C>(fields: &[F], columns: &[C]) -> bool
where
    F: AsRef<str>,
    C: AsRef<str>,
{
    sail_typed_node_table_missing_fields(fields, columns).is_empty()
}

pub fn sail_typed_node_table_missing_fields<F, C>(fields: &[F], columns: &[C]) -> Vec<String>
where
    F: AsRef<str>,
    C: AsRef<str>,
{
    let mut missing = fields
        .iter()
        .filter_map(|field| {
            let field = field.as_ref();
            let available = field == NODE_LABEL_COLUMN
                || (sail_typed_node_field_compatible(field)
                    && columns
                        .iter()
                        .any(|column| column.as_ref().eq_ignore_ascii_case(field)));
            (!available).then(|| field.to_string())
        })
        .collect::<Vec<_>>();
    missing.sort();
    missing.dedup();
    missing
}

pub fn sail_typed_edge_table_has_fields<F, C>(fields: &[F], columns: &[C]) -> bool
where
    F: AsRef<str>,
    C: AsRef<str>,
{
    sail_typed_edge_table_missing_fields(fields, columns).is_empty()
}

pub fn sail_typed_edge_table_missing_fields<F, C>(fields: &[F], columns: &[C]) -> Vec<String>
where
    F: AsRef<str>,
    C: AsRef<str>,
{
    let mut missing = fields
        .iter()
        .filter_map(|field| {
            let field = field.as_ref();
            let available = field == NODE_LABEL_COLUMN
                || field == EDGE_TYPE_COLUMN
                || (sail_typed_edge_field_compatible(field)
                    && columns
                        .iter()
                        .any(|column| column.as_ref().eq_ignore_ascii_case(field)));
            (!available).then(|| field.to_string())
        })
        .collect::<Vec<_>>();
    missing.sort();
    missing.dedup();
    missing
}

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

    /// Persists a named Cypher constraint registry JSON blob in Sail.
    ///
    /// This is intentionally a Grust metadata helper, not native Sail
    /// constraint/index DDL. Callers still apply the projected [`GraphSchema`]
    /// through [`GraphStore::apply_schema`] when they want constraints to affect
    /// writes.
    pub async fn save_cypher_constraint_registry(
        &self,
        name: &str,
        registry: &CypherConstraintRegistry,
    ) -> Result<()> {
        validate_cypher_constraint_registry_name(name)?;
        let registry_json = registry.to_json()?;
        self.run_command(&create_cypher_constraint_registry_table_sql(), vec![])
            .await?;
        self.run_command(
            &upsert_cypher_constraint_registry_sql(name, &registry_json)?,
            vec![],
        )
        .await
    }

    /// Loads a named Cypher constraint registry JSON blob from Sail.
    ///
    /// Missing names return `Ok(None)`. Existing rows are deserialized with
    /// [`CypherConstraintRegistry::from_json`].
    pub async fn load_cypher_constraint_registry(
        &self,
        name: &str,
    ) -> Result<Option<CypherConstraintRegistry>> {
        validate_cypher_constraint_registry_name(name)?;
        self.run_command(&create_cypher_constraint_registry_table_sql(), vec![])
            .await?;
        let sql = select_cypher_constraint_registry_sql(name)?;
        let chunks = self.query_arrow_ipc(&sql).await?;
        let Some(registry_json) = parse_optional_single_string_from_arrow(
            &chunks,
            "registry_json",
            "Cypher constraint registry",
        )?
        else {
            return Ok(None);
        };
        Ok(Some(CypherConstraintRegistry::from_json(&registry_json)?))
    }

    /// Executes the strict v1 writable-Cypher subset through Grust mutations.
    pub async fn execute_cypher_mutation(&self, cypher: &str) -> Result<CypherMutationReport> {
        self.execute_cypher_mutation_with_options(cypher, CypherMutationOptions::default())
            .await
    }

    /// Executes writable Cypher with explicit execution options.
    ///
    /// `CypherCreateMode::ErrorIfExists` performs a read-before-write preflight
    /// for Cypher `CREATE` operations. It is intentionally opt-in because the
    /// default Grust mutation path treats `CREATE` and `MERGE` as upsert intent.
    pub async fn execute_cypher_mutation_with_options(
        &self,
        cypher: &str,
        options: CypherMutationOptions,
    ) -> Result<CypherMutationReport> {
        Ok(self
            .execute_cypher_mutation_result_with_options(cypher, options)
            .await?
            .report)
    }

    /// Executes writable Cypher and returns both count-oriented mutation
    /// reporting and any IDs accepted/generated during planning.
    pub async fn execute_cypher_mutation_result_with_options(
        &self,
        cypher: &str,
        options: CypherMutationOptions,
    ) -> Result<CypherMutationResult> {
        let create_mode = options.create_mode;
        let collect_written_node_identities = options.collect_written_node_identities;
        let collect_written_edge_identities = options.collect_written_edge_identities;
        let (plan, generated_node_ids) = sail_cypher_mutation_plan_with_options(cypher, options)?;
        let mut report = plan.report();
        if create_mode == CypherCreateMode::ErrorIfExists {
            self.check_strict_create_conflicts(&plan)
                .await
                .map_err(cypher_execution_error)?;
        }
        let mut written_node_identities = Vec::new();
        let mut written_edge_identities = Vec::new();
        let node_identity_collector =
            collect_written_node_identities.then_some(&mut written_node_identities);
        let identity_collector =
            collect_written_edge_identities.then_some(&mut written_edge_identities);
        self.apply_cypher_mutation_plan(
            &plan,
            &mut report,
            node_identity_collector,
            identity_collector,
            None,
            None,
            None,
            None,
        )
        .await
        .map_err(cypher_execution_error)?;
        Ok(CypherMutationResult {
            report,
            generated_node_ids,
            written_node_identities,
            written_edge_identities,
        })
    }

    /// Executes writable Cypher with a final, strict RETURN projection.
    pub async fn execute_cypher_mutation_returning(
        &self,
        cypher: &str,
    ) -> Result<CypherMutationTableResult> {
        self.execute_cypher_mutation_returning_with_options(
            cypher,
            CypherMutationOptions::default(),
        )
        .await
    }

    /// Executes writable Cypher with a final, strict property-projection RETURN.
    ///
    /// This intentionally supports only a small write-result slice: the final
    /// statement may project elements or properties from node or relationship
    /// variables whose identities were resolved by the mutation plan, plus Sail
    /// row-producing relationship variables from restricted
    /// `MATCH ... CREATE/MERGE` edge writes. Aggregation, ordering, limiting,
    /// paths, and arbitrary read-query features remain deferred.
    pub async fn execute_cypher_mutation_returning_with_options(
        &self,
        cypher: &str,
        options: CypherMutationOptions,
    ) -> Result<CypherMutationTableResult> {
        let create_mode = options.create_mode;
        let collect_written_node_identities = options.collect_written_node_identities;
        let collect_written_edge_identities = options.collect_written_edge_identities;
        let planned = sail_cypher_mutation_plan_with_return_options(cypher, options)?;
        let mut report = planned.plan.report();
        if create_mode == CypherCreateMode::ErrorIfExists {
            self.check_strict_create_conflicts(&planned.plan)
                .await
                .map_err(cypher_execution_error)?;
        }
        let mut written_node_identities = Vec::new();
        let mut written_edge_identities = Vec::new();
        let node_identity_collector =
            collect_written_node_identities.then_some(&mut written_node_identities);
        let identity_collector =
            collect_written_edge_identities.then_some(&mut written_edge_identities);
        let mut row_node_ids = HashMap::new();
        let mut row_edge_keys = HashMap::new();
        let mut row_node_pre_delete_values = HashMap::new();
        let mut row_edge_pre_delete_values = HashMap::new();
        self.apply_cypher_mutation_plan(
            &planned.plan,
            &mut report,
            node_identity_collector,
            identity_collector,
            Some((&planned.row_node_bindings, &mut row_node_ids)),
            Some((&planned.row_edge_match_bindings, &mut row_edge_keys)),
            Some((&planned.row_node_bindings, &mut row_node_pre_delete_values)),
            Some((
                &planned.row_edge_match_bindings,
                &mut row_edge_pre_delete_values,
            )),
        )
        .await
        .map_err(cypher_execution_error)?;
        let mut row_node_values = row_node_return_values_on_store(self, row_node_ids)
            .await
            .map_err(cypher_execution_error)?;
        row_node_values.extend(row_node_pre_delete_values);
        let mut row_edge_values = row_edge_match_return_values_on_store(self, row_edge_keys)
            .await
            .map_err(cypher_execution_error)?;
        row_edge_values.extend(row_edge_pre_delete_values);
        row_edge_values.extend(
            self.row_edge_return_values(&planned.row_edge_bindings)
                .await
                .map_err(cypher_execution_error)?,
        );
        row_node_values.extend(
            row_edge_endpoint_node_values_on_store(
                self,
                &planned.node_bindings,
                &planned.row_edge_bindings,
                &row_edge_values,
            )
            .await
            .map_err(cypher_execution_error)?,
        );
        let table = evaluate_cypher_return_table(
            self,
            &planned.node_bindings,
            &planned.edge_bindings,
            &row_node_values,
            &row_edge_values,
            &planned.row_path_bindings,
            &planned.return_clause,
        )
        .await
        .map_err(cypher_execution_error)?;
        Ok(CypherMutationTableResult {
            mutation: CypherMutationResult {
                report,
                generated_node_ids: planned.generated_node_ids,
                written_node_identities,
                written_edge_identities,
            },
            table,
        })
    }

    async fn apply_cypher_mutation_plan(
        &self,
        plan: &GraphMutationPlan,
        report: &mut CypherMutationReport,
        mut written_node_identities: Option<&mut Vec<CypherWrittenNodeIdentity>>,
        mut written_edge_identities: Option<&mut Vec<CypherWrittenEdgeIdentity>>,
        mut row_node_capture: Option<(
            &HashMap<String, GraphNodeMatch>,
            &mut HashMap<String, Vec<NodeId>>,
        )>,
        mut row_edge_capture: Option<(
            &HashMap<String, GraphRelationshipMatch>,
            &mut HashMap<String, Vec<String>>,
        )>,
        mut row_node_pre_delete_capture: Option<(
            &HashMap<String, GraphNodeMatch>,
            &mut HashMap<String, Vec<Node>>,
        )>,
        mut row_edge_pre_delete_capture: Option<(
            &HashMap<String, GraphRelationshipMatch>,
            &mut HashMap<String, Vec<Edge>>,
        )>,
    ) -> Result<()> {
        for operation in &plan.operations {
            if let Some((bindings, values)) = row_node_pre_delete_capture.as_mut() {
                collect_deleted_row_node_values_for_operation(self, operation, bindings, values)
                    .await?;
            }
            if let Some((bindings, values)) = row_edge_pre_delete_capture.as_mut() {
                collect_deleted_row_edge_values_for_operation(self, operation, bindings, values)
                    .await?;
            }
            if let Some((bindings, values)) = row_node_capture.as_mut() {
                collect_row_node_ids_for_operation(self, operation, bindings, values).await?;
            }
            if let Some((bindings, values)) = row_edge_capture.as_mut() {
                collect_row_edge_keys_for_operation(self, operation, bindings, values).await?;
            }
            match operation {
                GraphMutationPlanOp::PatchMatchingNodes {
                    label,
                    props,
                    predicates,
                    patch,
                    ..
                } => {
                    self.apply_patch_matching_nodes(
                        label.as_ref(),
                        props,
                        predicates,
                        patch,
                        report,
                    )
                    .await?;
                }
                GraphMutationPlanOp::UpdateMatchingNodeProperty {
                    label,
                    props,
                    predicates,
                    target_key,
                    source_key,
                    op,
                    operand,
                    ..
                } => {
                    self.apply_update_matching_node_property(
                        label.as_ref(),
                        props,
                        predicates,
                        target_key,
                        source_key,
                        *op,
                        operand,
                        report,
                    )
                    .await?;
                }
                GraphMutationPlanOp::RemoveMatchingNodeProps {
                    label,
                    props,
                    predicates,
                    keys,
                    ..
                } => {
                    self.apply_remove_matching_node_props(
                        label.as_ref(),
                        props,
                        predicates,
                        keys,
                        report,
                    )
                    .await?;
                }
                GraphMutationPlanOp::DeleteMatchingNodes {
                    label,
                    props,
                    predicates,
                    ..
                } => {
                    self.apply_delete_matching_nodes(label.as_ref(), props, predicates, report)
                        .await?;
                }
                GraphMutationPlanOp::PatchMatchingEdges {
                    relationship,
                    patch,
                    ..
                } => {
                    self.apply_patch_matching_edges(relationship, patch, report)
                        .await?;
                }
                GraphMutationPlanOp::UpdateMatchingEdgeProperty {
                    relationship,
                    target_key,
                    source_key,
                    op,
                    operand,
                    ..
                } => {
                    self.apply_update_matching_edge_property(
                        relationship,
                        target_key,
                        source_key,
                        *op,
                        operand,
                        report,
                    )
                    .await?;
                }
                GraphMutationPlanOp::RemoveMatchingEdgeProps {
                    relationship, keys, ..
                } => {
                    self.apply_remove_matching_edge_props(relationship, keys, report)
                        .await?;
                }
                GraphMutationPlanOp::DeleteMatchingEdges { relationship, .. } => {
                    self.apply_delete_matching_edges(relationship, report)
                        .await?;
                }
                GraphMutationPlanOp::DeleteRelationshipRows {
                    relationship,
                    delete_edges,
                    endpoint_nodes,
                    ..
                } => {
                    self.apply_delete_relationship_rows(
                        relationship,
                        *delete_edges,
                        endpoint_nodes,
                        report,
                    )
                    .await?;
                }
                GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
                    kind,
                    from,
                    to,
                    label,
                    props,
                    edge_id_policy,
                    ..
                } => {
                    self.apply_upsert_edges_from_node_matches(
                        *kind,
                        from,
                        to,
                        label,
                        props,
                        *edge_id_policy,
                        report,
                        written_edge_identities.as_deref_mut(),
                    )
                    .await?;
                }
                _ => {
                    let precise_upsert = match operation {
                        GraphMutationPlanOp::UpsertNode { node, .. } => {
                            Some(CypherResolvedUpsertClassification::Node {
                                existed: self.get_node(&node.id).await?.is_some(),
                            })
                        }
                        GraphMutationPlanOp::UpsertEdge { edge, .. } => {
                            Some(CypherResolvedUpsertClassification::Edge {
                                existed: self.strict_create_edge_exists(edge).await?,
                            })
                        }
                        _ => None,
                    };
                    let mutation = GraphMutation::from(operation.clone());
                    self.apply_mutations(std::slice::from_ref(&mutation))
                        .await?;
                    if let Some(classification) = precise_upsert {
                        classification.record(report);
                    }
                    if let (Some(collector), GraphMutationPlanOp::UpsertNode { kind, node }) =
                        (written_node_identities.as_deref_mut(), operation)
                    {
                        collector.push(cypher_written_node_identity(*kind, node));
                    }
                    if let (Some(collector), GraphMutationPlanOp::UpsertEdge { kind, edge }) =
                        (written_edge_identities.as_deref_mut(), operation)
                    {
                        collector.push(cypher_written_edge_identity(*kind, edge));
                    }
                }
            }
        }
        Ok(())
    }

    async fn strict_create_edge_exists(&self, edge: &Edge) -> Result<bool> {
        let mut existing = self
            .get_edges(EdgeQuery {
                from: Some(edge.from.clone()),
                to: Some(edge.to.clone()),
                label: Some(edge.label.clone()),
            })
            .await?;

        if let Some(id) = &edge.id {
            let sql = "SELECT id, src_id, src_label, dst_id, dst_label, edge_type, props \
                       FROM grust_edges WHERE id = ? LIMIT 1";
            existing.extend(self.run_edge_query(sql, vec![lit_str(id.as_str())]).await?);
        }

        Ok(strict_create_edge_conflicts(edge, &existing))
    }

    async fn apply_patch_matching_nodes(
        &self,
        label: Option<&Label>,
        props: &Props,
        predicates: &[GraphPropertyPredicate],
        patch: &Props,
        report: &mut CypherMutationReport,
    ) -> Result<()> {
        let (sql, args) = matching_nodes_sql(label, props, predicates)?;
        let mut nodes = self.run_query(&sql, args).await?;
        report.matched_rows += nodes.len();
        report.node_patches += nodes.len();
        report.changed_nodes += nodes.len();
        if nodes.is_empty() {
            return Ok(());
        }

        for node in &mut nodes {
            for (key, value) in patch {
                node.props.insert(key.clone(), value.clone());
            }
        }
        let schema = self.current_schema();
        if let Some(schema) = schema.as_ref() {
            for node in &nodes {
                schema.validate_node(node)?;
            }
        }
        self.load_nodes(schema.as_ref(), &nodes).await
    }

    async fn apply_update_matching_node_property(
        &self,
        label: Option<&Label>,
        props: &Props,
        predicates: &[GraphPropertyPredicate],
        target_key: &str,
        source_key: &str,
        op: GraphNumericOp,
        operand: &Value,
        report: &mut CypherMutationReport,
    ) -> Result<()> {
        let (sql, args) = matching_nodes_sql(label, props, predicates)?;
        let mut nodes = self.run_query(&sql, args).await?;
        report.matched_rows += nodes.len();
        report.node_patches += nodes.len();
        report.changed_nodes += nodes.len();
        if nodes.is_empty() {
            return Ok(());
        }

        for node in &mut nodes {
            let current = node.props.get(source_key).ok_or_else(|| {
                GrustError::CypherExecution(format!(
                    "numeric expression source property '{source_key}' is missing"
                ))
            })?;
            let value = evaluate_numeric_update(current, op, operand)?;
            node.props.insert(target_key.to_string(), value);
        }
        let schema = self.current_schema();
        if let Some(schema) = schema.as_ref() {
            for node in &nodes {
                schema.validate_node(node)?;
            }
        }
        self.load_nodes(schema.as_ref(), &nodes).await
    }

    async fn apply_remove_matching_node_props(
        &self,
        label: Option<&Label>,
        props: &Props,
        predicates: &[GraphPropertyPredicate],
        keys: &[String],
        report: &mut CypherMutationReport,
    ) -> Result<()> {
        let (sql, args) = matching_nodes_sql(label, props, predicates)?;
        let mut nodes = self.run_query(&sql, args).await?;
        report.matched_rows += nodes.len();
        report.node_property_removes += nodes.len();
        report.changed_nodes += nodes.len();
        if nodes.is_empty() {
            return Ok(());
        }

        for node in &mut nodes {
            for key in keys {
                node.props.remove(key);
            }
        }
        let schema = self.current_schema();
        if let Some(schema) = schema.as_ref() {
            for node in &nodes {
                schema.validate_node(node)?;
            }
        }
        self.load_nodes(schema.as_ref(), &nodes).await
    }

    async fn apply_patch_matching_edges(
        &self,
        relationship: &GraphRelationshipMatch,
        patch: &Props,
        report: &mut CypherMutationReport,
    ) -> Result<()> {
        let mut edges = self.matching_edges(relationship).await?;
        report.matched_rows += edges.len();
        report.edge_patches += edges.len();
        report.changed_edges += edges.len();
        if edges.is_empty() {
            return Ok(());
        }

        for edge in &mut edges {
            for (key, value) in patch {
                edge.props.insert(key.clone(), value.clone());
            }
        }
        self.validate_and_load_edges(&edges).await
    }

    async fn apply_update_matching_edge_property(
        &self,
        relationship: &GraphRelationshipMatch,
        target_key: &str,
        source_key: &str,
        op: GraphNumericOp,
        operand: &Value,
        report: &mut CypherMutationReport,
    ) -> Result<()> {
        let mut edges = self.matching_edges(relationship).await?;
        report.matched_rows += edges.len();
        report.edge_patches += edges.len();
        report.changed_edges += edges.len();
        if edges.is_empty() {
            return Ok(());
        }

        for edge in &mut edges {
            let current = edge.props.get(source_key).ok_or_else(|| {
                GrustError::CypherExecution(format!(
                    "numeric expression source property '{source_key}' is missing"
                ))
            })?;
            let value = evaluate_numeric_update(current, op, operand)?;
            edge.props.insert(target_key.to_string(), value);
        }
        self.validate_and_load_edges(&edges).await
    }

    async fn apply_remove_matching_edge_props(
        &self,
        relationship: &GraphRelationshipMatch,
        keys: &[String],
        report: &mut CypherMutationReport,
    ) -> Result<()> {
        let mut edges = self.matching_edges(relationship).await?;
        report.matched_rows += edges.len();
        report.edge_property_removes += edges.len();
        report.changed_edges += edges.len();
        if edges.is_empty() {
            return Ok(());
        }

        for edge in &mut edges {
            for key in keys {
                edge.props.remove(key);
            }
        }
        self.validate_and_load_edges(&edges).await
    }

    async fn apply_delete_matching_edges(
        &self,
        relationship: &GraphRelationshipMatch,
        report: &mut CypherMutationReport,
    ) -> Result<()> {
        let edges = self.matching_edges(relationship).await?;
        report.matched_rows += edges.len();
        report.edge_deletes += edges.len();
        report.changed_edges += edges.len();
        let mut keys = edges.iter().map(edge_key).collect::<Vec<_>>();
        keys.sort();
        keys.dedup();
        self.delete_edges_by_keys(&relationship.label, &keys).await
    }

    async fn delete_edges_by_keys(&self, edge_type: &Label, keys: &[String]) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        self.stage_record_batch(DELETE_EDGE_STAGE_VIEW, edge_keys_record_batch(keys)?)
            .await?;
        self.run_command(&delete_edge_keys_from_view_sql("grust_edges")?, vec![])
            .await?;
        let typed_table = self
            .current_schema()
            .and_then(|schema| schema.edge_type(edge_type).cloned());
        if let Some(edge_type) = typed_table {
            self.run_command(
                &delete_edge_keys_from_view_sql(&sail_edge_table(edge_type.label.as_str())?)?,
                vec![],
            )
            .await?;
        }
        Ok(())
    }

    async fn matching_edges(&self, relationship: &GraphRelationshipMatch) -> Result<Vec<Edge>> {
        let (sql, args) = matching_edges_sql(relationship)?;
        self.run_edge_query(&sql, args).await
    }

    async fn row_edge_return_values(
        &self,
        bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
    ) -> Result<HashMap<String, Vec<Edge>>> {
        let mut values = HashMap::new();
        for (variable, binding) in bindings {
            let edges = self
                .edges_from_node_matches(
                    binding.kind,
                    &binding.from,
                    &binding.to,
                    &binding.label,
                    &binding.props,
                    binding.edge_id_policy,
                )
                .await?;
            match binding.kind {
                GraphMutationPlanKind::Create | GraphMutationPlanKind::Merge => {}
            }
            values.insert(variable.clone(), edges);
        }
        Ok(values)
    }

    async fn apply_upsert_edges_from_node_matches(
        &self,
        kind: GraphMutationPlanKind,
        from: &GraphNodeMatch,
        to: &GraphNodeMatch,
        label: &Label,
        props: &Props,
        edge_id_policy: GraphRowEdgeIdPolicy,
        report: &mut CypherMutationReport,
        written_edge_identities: Option<&mut Vec<CypherWrittenEdgeIdentity>>,
    ) -> Result<()> {
        let edges = self
            .edges_from_node_matches(kind, from, to, label, props, edge_id_policy)
            .await?;
        report.matched_rows += edges.len();
        report.edge_upserts += edges.len();
        report.changed_edges += edges.len();
        match kind {
            GraphMutationPlanKind::Create => {}
            GraphMutationPlanKind::Merge => {}
        }
        let mut existing = Vec::with_capacity(edges.len());
        for edge in &edges {
            existing.push(self.strict_create_edge_exists(edge).await?);
        }
        self.validate_and_load_edges(&edges).await?;
        for existed in existing {
            if existed {
                report.edge_updates += 1;
            } else {
                report.edge_inserts += 1;
            }
        }
        if let Some(collector) = written_edge_identities {
            collector.extend(
                edges
                    .iter()
                    .map(|edge| cypher_written_edge_identity(kind, edge)),
            );
        }
        Ok(())
    }

    async fn edges_from_node_matches(
        &self,
        kind: GraphMutationPlanKind,
        from: &GraphNodeMatch,
        to: &GraphNodeMatch,
        label: &Label,
        props: &Props,
        edge_id_policy: GraphRowEdgeIdPolicy,
    ) -> Result<Vec<Edge>> {
        let (from_sql, from_args) =
            matching_nodes_sql(from.label.as_ref(), &from.props, &from.predicates)?;
        let (to_sql, to_args) = matching_nodes_sql(to.label.as_ref(), &to.props, &to.predicates)?;
        let from_nodes = self.run_query(&from_sql, from_args).await?;
        let to_nodes = self.run_query(&to_sql, to_args).await?;
        let mut edges = Vec::with_capacity(from_nodes.len().saturating_mul(to_nodes.len()));
        let edge_id = edge_id_from_props(props)?;
        if edge_id.is_some() && edges.capacity() > 1 {
            return Err(cypher_unsupported_cardinality(
                "row-producing MATCH ... CREATE/MERGE with an explicit relationship id must produce exactly one edge",
            ));
        }
        for from_node in &from_nodes {
            for to_node in &to_nodes {
                let mut edge = Edge::new(
                    label.clone(),
                    from_node.id.clone(),
                    to_node.id.clone(),
                    props.clone(),
                );
                if let Some(id) = edge_id.clone() {
                    edge = edge.with_id(id);
                } else if row_edge_id_policy_generates(kind, edge_id_policy) {
                    let generated_id =
                        generated_row_edge_id(&edge.from, &edge.label, &edge.to, &edge.props);
                    edge = edge.with_id(generated_id);
                }
                edges.push(edge);
            }
        }
        Ok(edges)
    }

    async fn validate_and_load_edges(&self, edges: &[Edge]) -> Result<()> {
        let schema = self.current_schema();
        let endpoint_ids = edges
            .iter()
            .flat_map(|edge| [&edge.from, &edge.to])
            .cloned()
            .collect::<Vec<_>>();
        let endpoint_nodes = self.get_nodes(&endpoint_ids).await?;
        let node_labels = endpoint_nodes
            .iter()
            .map(|node| (&node.id, &node.label))
            .collect::<BTreeMap<_, _>>();
        if let Some(schema) = schema.as_ref() {
            for edge in edges {
                schema.validate_edge_with(edge, |id| node_labels.get(id).copied())?;
            }
        }
        self.load_edges(schema.as_ref(), edges, &node_labels).await
    }

    async fn apply_delete_matching_nodes(
        &self,
        label: Option<&Label>,
        props: &Props,
        predicates: &[GraphPropertyPredicate],
        report: &mut CypherMutationReport,
    ) -> Result<()> {
        let (sql, args) = matching_nodes_sql(label, props, predicates)?;
        let nodes = self.run_query(&sql, args).await?;
        report.matched_rows += nodes.len();
        report.node_deletes += nodes.len();
        report.changed_nodes += nodes.len();
        if nodes.is_empty() {
            return Ok(());
        }

        let ids = nodes.iter().map(|node| node.id.clone()).collect::<Vec<_>>();
        let all_edges = self.get_edges(EdgeQuery::default()).await?;
        let incident_edges = all_edges
            .iter()
            .filter(|edge| ids.iter().any(|id| id == &edge.from || id == &edge.to))
            .count();
        report.changed_edges += incident_edges;
        report.edge_deletes += incident_edges;
        self.delete_nodes_by_ids(&ids).await
    }

    async fn apply_delete_relationship_rows(
        &self,
        relationship: &GraphRelationshipMatch,
        delete_edges: bool,
        endpoint_nodes: &[GraphRelationshipEndpoint],
        report: &mut CypherMutationReport,
    ) -> Result<()> {
        let edges = self.matching_edges(relationship).await?;
        let mut ids = edges
            .iter()
            .flat_map(|edge| {
                endpoint_nodes.iter().map(|endpoint| match endpoint {
                    GraphRelationshipEndpoint::From => edge.from.clone(),
                    GraphRelationshipEndpoint::To => edge.to.clone(),
                })
            })
            .collect::<Vec<_>>();
        ids.sort();
        ids.dedup();

        let mut edge_keys = if delete_edges {
            edges.iter().map(edge_key).collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if !ids.is_empty() {
            let all_edges = self.get_edges(EdgeQuery::default()).await?;
            edge_keys.extend(
                all_edges
                    .iter()
                    .filter(|edge| ids.iter().any(|id| id == &edge.from || id == &edge.to))
                    .map(edge_key),
            );
        }
        edge_keys.sort();
        edge_keys.dedup();

        report.matched_rows += edges.len();
        report.node_deletes += ids.len();
        report.changed_nodes += ids.len();
        report.edge_deletes += edge_keys.len();
        report.changed_edges += edge_keys.len();

        if !ids.is_empty() {
            self.delete_nodes_by_ids(&ids).await?;
        }
        if delete_edges && ids.is_empty() {
            self.delete_edges_by_keys(&relationship.label, &edge_keys)
                .await?;
        }
        Ok(())
    }

    async fn check_strict_create_conflicts(&self, plan: &GraphMutationPlan) -> Result<()> {
        check_strict_create_plan_conflicts(plan)?;
        for operation in &plan.operations {
            match operation {
                GraphMutationPlanOp::UpsertNode {
                    kind: GraphMutationPlanKind::Create,
                    node,
                } => {
                    if self.get_node(&node.id).await?.is_some() {
                        return Err(GrustError::Unsupported(format!(
                            "Cypher CREATE would overwrite existing node '{}'",
                            node.id.as_str()
                        )));
                    }
                }
                GraphMutationPlanOp::UpsertEdge {
                    kind: GraphMutationPlanKind::Create,
                    edge,
                } => {
                    if self.strict_create_edge_exists(edge).await? {
                        return Err(GrustError::Unsupported(format!(
                            "Cypher CREATE would overwrite existing edge '{}'",
                            edge_key(edge)
                        )));
                    }
                }
                GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
                    kind: GraphMutationPlanKind::Create,
                    from,
                    to,
                    label,
                    props,
                    edge_id_policy,
                    ..
                } => {
                    let edges = self
                        .edges_from_node_matches(
                            GraphMutationPlanKind::Create,
                            from,
                            to,
                            label,
                            props,
                            *edge_id_policy,
                        )
                        .await?;
                    for edge in &edges {
                        if self.strict_create_edge_exists(edge).await? {
                            return Err(GrustError::Unsupported(format!(
                                "Cypher CREATE would overwrite existing edge '{}'",
                                edge_key(edge)
                            )));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn delete_nodes_by_ids(&self, ids: &[NodeId]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let id_refs = ids.iter().collect::<Vec<_>>();
        self.stage_record_batch(DELETE_NODE_STAGE_VIEW, node_ids_record_batch(&id_refs)?)
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
                "SELECT id, src_id, src_label, dst_id, dst_label, edge_type, props FROM grust_edges",
                vec![],
            )
            .await?;
        Ok(Graph::new(nodes, edges))
    }

    /// Execute a bounded read-only Cypher query, pushing the `MATCH`/`WHERE`
    /// filter down into Sail/Spark SQL where possible (Unit 15 of
    /// `docs/GQL_GOAL.md`).
    ///
    /// For the pushable subset — a single node pattern with property comparisons
    /// (see [`grust_cypher::pushdown`]) — the scan and filter are lowered to SQL
    /// over `grust_nodes`, so only the surviving rows leave Spark; the `RETURN`
    /// projection then runs through the shared Memory reference, making the result
    /// identical to [`grust_cypher::read::run_read_query`] by construction. Any
    /// other shape falls back to loading the graph and running the portable
    /// reference directly (correct, but unfiltered).
    pub async fn run_read_query(
        &self,
        cypher: &str,
        params: &CypherParameters,
    ) -> Result<CypherResultTable> {
        // Schema type hints let Spark push numeric `ORDER BY`/arithmetic (its
        // `GET_JSON_OBJECT` returns text, so the sort key must be cast).
        let hints = SailTypeHints {
            schema: self.current_schema(),
        };
        if let Some(plan) = grust_cypher::pushdown::plan_read(cypher, params, &hints)? {
            let dialect = grust_cypher::pushdown::SparkDialect;
            // `UNION`: run each arm and combine the result tables (a recursive
            // CTE for var-length arms requires recursive-CTE engine support).
            if let Some((arms, distinct)) = plan.union_arms() {
                let mut tables = Vec::with_capacity(arms.len());
                for arm in arms {
                    let rows = self.run_text_query(&arm.to_sql(&dialect)).await?;
                    tables.push(arm.project_text_rows(&dialect, rows, params)?);
                }
                return grust_cypher::pushdown::combine_union(tables, distinct);
            }
            let rows = self.run_text_query(&plan.to_sql(&dialect)).await?;
            return plan.project_text_rows(&dialect, rows, params);
        }
        let graph = self.read_graph().await?;
        grust_cypher::read::run_read_query(&graph, cypher, params)
    }

    /// Execute SQL whose result columns are all strings, returning each row as a
    /// vector of optional text cells (used by relationship-segment pushdown,
    /// which reconstructs `(a, r, b)` bindings from the projected text columns).
    async fn run_text_query(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>> {
        let mut rows = Vec::new();
        self.run_plan(self.query_request(sql, vec![])?, |data| {
            rows.extend(parse_text_rows_from_arrow(data)?);
            Ok(())
        })
        .await?;
        Ok(rows)
    }

    /// Computes out-degrees over the generic persisted Sail edge table.
    pub async fn out_degrees(&self) -> Result<Vec<SailDegreeRow>> {
        self.run_degree_query(sail_out_degrees_sql()).await
    }

    /// Computes in-degrees over the generic persisted Sail edge table.
    pub async fn in_degrees(&self) -> Result<Vec<SailDegreeRow>> {
        self.run_degree_query(sail_in_degrees_sql()).await
    }

    /// Computes total degree for each non-isolated vertex over the generic
    /// persisted Sail edge table.
    pub async fn degrees(&self) -> Result<Vec<SailDegreeRow>> {
        self.run_degree_query(sail_degrees_sql()).await
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

    /// Reads edge triplets by joining generic persisted edge rows to source and
    /// destination node rows.
    pub async fn triplets(&self) -> Result<Vec<SailTripletRow>> {
        self.triplets_for_direction(SailGraphPatternDirection::Outgoing)
            .await
    }

    /// Reads edge triplets oriented for a graph pattern direction.
    pub async fn triplets_for_direction(
        &self,
        direction: SailGraphPatternDirection,
    ) -> Result<Vec<SailTripletRow>> {
        let mut rows = Vec::new();
        self.run_plan(
            self.query_request(sail_triplets_sql_for_direction(direction), vec![])?,
            |data| {
                rows.extend(parse_triplets_from_arrow(data)?);
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

    /// Read-before-write enforcement of node uniqueness constraints.
    ///
    /// For every `NodePropertyUnique` constraint touched by `nodes`, this reads
    /// the persisted nodes of that label and rejects a write whose constrained
    /// property value already belongs to a different node id. This is a
    /// best-effort `ValidateBeforeWrite` check with an inherent race window:
    /// concurrent writers are not serialized, and the label scan cost grows
    /// with the number of persisted nodes of the label.
    async fn enforce_unique_node_constraints(
        &self,
        constraints: &[GraphConstraint],
        nodes: &[Node],
    ) -> Result<()> {
        for constraint in constraints {
            let GraphConstraint::NodePropertyUnique { label, key } = constraint else {
                continue;
            };
            if !nodes
                .iter()
                .any(|node| &node.label == label && node.props.contains_key(key))
            {
                continue;
            }
            let existing = self
                .run_query(
                    "SELECT id, label, props FROM grust_nodes WHERE label = ?",
                    vec![lit_str(label.as_str())],
                )
                .await?;
            for node in nodes {
                if let Some(conflict) = unique_node_conflict(&existing, node, label, key) {
                    return Err(GrustError::Schema(format!(
                        "node '{}' violates unique constraint on property '{}' of label '{}' (conflicts with persisted node '{}')",
                        node.id.as_str(),
                        key,
                        label.as_str(),
                        conflict.as_str()
                    )));
                }
            }
        }
        Ok(())
    }

    /// Read-before-write enforcement of edge uniqueness constraints. Shares the
    /// best-effort `ValidateBeforeWrite` semantics of
    /// [`Self::enforce_unique_node_constraints`].
    async fn enforce_unique_edge_constraints(
        &self,
        constraints: &[GraphConstraint],
        edges: &[Edge],
    ) -> Result<()> {
        for constraint in constraints {
            let GraphConstraint::EdgePropertyUnique { label, key } = constraint else {
                continue;
            };
            if !edges
                .iter()
                .any(|edge| &edge.label == label && edge.props.contains_key(key))
            {
                continue;
            }
            let existing = self
                .get_edges(EdgeQuery {
                    label: Some(label.clone()),
                    ..EdgeQuery::default()
                })
                .await?;
            for edge in edges {
                if let Some(conflict) = unique_edge_conflict(&existing, edge, label, key) {
                    return Err(GrustError::Schema(format!(
                        "edge '{}' violates unique constraint on property '{}' of label '{}' (conflicts with persisted edge '{}')",
                        edge_key(edge),
                        key,
                        label.as_str(),
                        conflict
                    )));
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

    fn constraint_capability(&self, constraint: &GraphConstraint) -> GraphConstraintCapability {
        // Required and unique constraints are both validated read-before-write:
        // required by `validate_node`/`validate_graph`, unique by an existence
        // query against persisted rows. Neither is enforced atomically by the
        // backend, so concurrent writers can still race.
        match constraint {
            GraphConstraint::NodePropertyRequired { .. }
            | GraphConstraint::EdgePropertyRequired { .. }
            | GraphConstraint::NodePropertyUnique { .. }
            | GraphConstraint::EdgePropertyUnique { .. } => {
                GraphConstraintCapability::ValidateBeforeWrite
            }
        }
    }

    async fn put_node(&self, node: &Node) -> Result<PutOutcome> {
        let schema = self.current_schema();
        if let Some(schema) = schema.as_ref() {
            schema.validate_node(node)?;
            self.enforce_unique_node_constraints(&schema.constraints, std::slice::from_ref(node))
                .await?;
        }
        self.load_nodes(schema.as_ref(), std::slice::from_ref(node))
            .await?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        let schema = self.current_schema();
        if let Some(schema) = schema.as_ref() {
            schema.validate_edge_props(edge)?;
            self.enforce_unique_edge_constraints(&schema.constraints, std::slice::from_ref(edge))
                .await?;
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
            self.enforce_unique_node_constraints(&schema.constraints, &graph.nodes)
                .await?;
            self.enforce_unique_edge_constraints(&schema.constraints, &graph.edges)
                .await?;
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

    /// Batched node read using a single `IN (...)` query rather than one round
    /// trip per id. Preserves input order and duplicates and skips missing ids,
    /// matching the [`GraphStore::get_nodes`] default contract.
    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; ids.len()].join(", ");
        let sql = format!("SELECT id, label, props FROM grust_nodes WHERE id IN ({placeholders})");
        let args = ids.iter().map(|id| lit_str(id.as_str())).collect();
        let fetched = self.run_query(&sql, args).await?;
        let by_id: HashMap<&str, &Node> = fetched
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();
        Ok(ids
            .iter()
            .filter_map(|id| by_id.get(id.as_str()).map(|node| (*node).clone()))
            .collect())
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
            "SELECT id, src_id, src_label, dst_id, dst_label, edge_type, props FROM grust_edges{}",
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
             SELECT CAST(NULL AS STRING) AS edge_key, \
                    CAST(NULL AS STRING) AS id, \
                    CAST(NULL AS STRING) AS src_id, \
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
        self.delete_nodes_by_ids(std::slice::from_ref(id)).await
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

#[async_trait]
impl CypherMutationExecutor for SailGraphStore {
    async fn execute_cypher_mutation_plan(
        &self,
        plan: &GraphMutationPlan,
    ) -> Result<GraphMutationReport> {
        let mut report = plan.report();
        self.apply_cypher_mutation_plan(plan, &mut report, None, None, None, None, None, None)
            .await
            .map_err(cypher_execution_error)?;
        Ok(report)
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

fn edge_keys_record_batch(keys: &[String]) -> Result<RecordBatch> {
    let schema = Arc::new(ArrowSchema::new(vec![ArrowField::new(
        "edge_key",
        DataType::Utf8,
        false,
    )]));
    RecordBatch::try_new(
        schema,
        vec![Arc::new(StringArray::from_iter_values(
            keys.iter().map(String::as_str),
        ))],
    )
    .map_err(|e| GrustError::Backend(format!("Arrow edge-key delete batch build failed: {e}")))
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
         WHEN MATCHED THEN UPDATE SET t.edge_key = s.edge_key, t.id = s.id, t.src_label = s.src_label, t.dst_label = s.dst_label, t.props = s.props \
         WHEN NOT MATCHED THEN INSERT (edge_key, id, src_id, src_label, dst_id, dst_label, edge_type, props) \
           VALUES (s.edge_key, s.id, s.src_id, s.src_label, s.dst_id, s.dst_label, s.edge_type, s.props)"
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

fn delete_edge_keys_from_view_sql(table: &str) -> Result<String> {
    Ok(format!(
        "MERGE INTO {} AS t USING {DELETE_EDGE_STAGE_VIEW} AS s \
         ON t.edge_key = s.edge_key WHEN MATCHED THEN DELETE",
        sql_table_ref(table)?
    ))
}

pub fn sail_out_degrees_sql() -> &'static str {
    "SELECT src_id AS id, COUNT(*) AS degree FROM grust_edges GROUP BY src_id"
}

pub fn sail_in_degrees_sql() -> &'static str {
    "SELECT dst_id AS id, COUNT(*) AS degree FROM grust_edges GROUP BY dst_id"
}

pub fn sail_degrees_sql() -> &'static str {
    "SELECT id, SUM(degree) AS degree FROM (\
       SELECT src_id AS id, COUNT(*) AS degree FROM grust_edges GROUP BY src_id \
       UNION ALL \
       SELECT dst_id AS id, COUNT(*) AS degree FROM grust_edges GROUP BY dst_id\
     ) degree_events GROUP BY id"
}

pub fn sail_degree_pairs_sql() -> &'static str {
    "SELECT n.id AS id, \
            COALESCE(in_degrees.degree, 0) AS in_degree, \
            COALESCE(out_degrees.degree, 0) AS out_degree \
       FROM grust_nodes n \
       LEFT JOIN (SELECT dst_id AS id, COUNT(*) AS degree FROM grust_edges GROUP BY dst_id) in_degrees \
         ON n.id = in_degrees.id \
       LEFT JOIN (SELECT src_id AS id, COUNT(*) AS degree FROM grust_edges GROUP BY src_id) out_degrees \
         ON n.id = out_degrees.id"
}

pub fn sail_triplets_sql() -> String {
    sail_triplets_sql_for_direction(SailGraphPatternDirection::Outgoing)
}

pub fn sail_triplets_sql_for_direction(direction: SailGraphPatternDirection) -> String {
    let outgoing = "SELECT src.id AS src_id, \
            src.label AS src_label, \
            src.props AS src_props, \
            e.id AS edge_id, \
            e.src_id AS edge_src_id, \
            e.src_label AS edge_src_label, \
            e.dst_id AS edge_dst_id, \
            e.dst_label AS edge_dst_label, \
            e.edge_type AS edge_type, \
            e.props AS edge_props, \
            dst.id AS dst_id, \
            dst.label AS dst_label, \
            dst.props AS dst_props \
       FROM grust_edges e \
       JOIN grust_nodes src ON src.id = e.src_id \
       JOIN grust_nodes dst ON dst.id = e.dst_id";
    let incoming = "SELECT dst.id AS src_id, \
            dst.label AS src_label, \
            dst.props AS src_props, \
            e.id AS edge_id, \
            e.src_id AS edge_src_id, \
            e.src_label AS edge_src_label, \
            e.dst_id AS edge_dst_id, \
            e.dst_label AS edge_dst_label, \
            e.edge_type AS edge_type, \
            e.props AS edge_props, \
            src.id AS dst_id, \
            src.label AS dst_label, \
            src.props AS dst_props \
       FROM grust_edges e \
       JOIN grust_nodes src ON src.id = e.src_id \
       JOIN grust_nodes dst ON dst.id = e.dst_id";

    match direction {
        SailGraphPatternDirection::Outgoing => outgoing.to_string(),
        SailGraphPatternDirection::Incoming => incoming.to_string(),
        SailGraphPatternDirection::Undirected => {
            format!("{outgoing} UNION ALL {incoming}")
        }
    }
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
            "CREATE TABLE IF NOT EXISTS {} (id STRING NOT NULL{fields}) USING delta TBLPROPERTIES ({} = {}, {} = {})",
            sail_node_table(node_type.label.as_str())?,
            sql_str(GRAPH_TABLE_KIND_PROPERTY),
            sql_str(GRAPH_TABLE_KIND_NODE),
            sql_str(GRAPH_TABLE_LABEL_PROPERTY),
            sql_str(node_type.label.as_str())
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
            "CREATE TABLE IF NOT EXISTS {} (edge_key STRING NOT NULL, id STRING, src_id STRING NOT NULL, dst_id STRING NOT NULL{fields}) USING delta TBLPROPERTIES ({} = {}, {} = {})",
            sail_edge_table(edge_type.label.as_str())?,
            sql_str(GRAPH_TABLE_KIND_PROPERTY),
            sql_str(GRAPH_TABLE_KIND_EDGE),
            sql_str(GRAPH_TABLE_LABEL_PROPERTY),
            sql_str(edge_type.label.as_str())
        ));
    }
    Ok(statements)
}

/// SQL expression extracting one typed field from the staged plain-JSON props
/// column.
fn props_field_expr(props_column: &str, field: &Field) -> Result<String> {
    let raw = sail_json_property_expr(props_column, &field.name)?;
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

fn matching_nodes_sql(
    label: Option<&Label>,
    props: &Props,
    predicates: &[GraphPropertyPredicate],
) -> Result<(String, Vec<expression::Literal>)> {
    let mut conditions = Vec::new();
    let mut args = Vec::new();
    if let Some(label) = label {
        conditions.push("label = ?".to_string());
        args.push(lit_str(label.as_str()));
    }
    for (key, value) in props {
        validate_json_key(key)?;
        let json_value = sail_json_property_expr("props", key)?;
        match value {
            Value::String(s) => {
                conditions.push(format!("{json_value} = ?"));
                args.push(lit_str(s));
            }
            Value::Int(n) => {
                conditions.push(format!("CAST({json_value} AS BIGINT) = ?"));
                args.push(lit_long(*n));
            }
            Value::Float(f) => {
                conditions.push(format!("CAST({json_value} AS DOUBLE) = ?"));
                args.push(lit_double(*f));
            }
            Value::Bool(b) => {
                conditions.push(format!("CAST({json_value} AS BOOLEAN) = ?"));
                args.push(lit_bool(*b));
            }
            Value::Null => conditions.push(format!("{json_value} IS NULL")),
            Value::DateTime(_)
            | Value::Decimal(_)
            | Value::Duration(_)
            | Value::StringArray(_)
            | Value::IntArray(_)
            | Value::FloatArray(_)
            | Value::Path(_)
            | Value::Json(_) => {
                let json = serde_json::to_string(&value.to_json())
                    .map_err(|err| GrustError::Serialization(err.to_string()))?;
                conditions.push(format!("{json_value} = ?"));
                args.push(lit_str(&json));
            }
        }
    }
    append_property_predicate_conditions("props", predicates, &mut conditions, &mut args)?;
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };
    Ok((
        format!("SELECT id, label, props FROM grust_nodes{where_clause}"),
        args,
    ))
}

fn matching_edges_sql(
    relationship: &GraphRelationshipMatch,
) -> Result<(String, Vec<expression::Literal>)> {
    let mut conditions = vec!["e.edge_type = ?".to_string()];
    let mut args = vec![lit_str(relationship.label.as_str())];
    if let Some(id) = &relationship.id {
        conditions.push("e.id = ?".to_string());
        args.push(lit_str(id.as_str()));
    }
    append_relationship_prop_conditions(relationship, &mut conditions, &mut args)?;
    append_node_match_conditions("src", &relationship.from, &mut conditions, &mut args)?;
    append_node_match_conditions("dst", &relationship.to, &mut conditions, &mut args)?;
    Ok((
        format!(
            "SELECT e.id, e.src_id, src.label AS src_label, e.dst_id, dst.label AS dst_label, e.edge_type, e.props \
             FROM grust_edges e \
             JOIN grust_nodes src ON src.id = e.src_id \
             JOIN grust_nodes dst ON dst.id = e.dst_id \
             WHERE {}",
            conditions.join(" AND ")
        ),
        args,
    ))
}

fn append_relationship_prop_conditions(
    relationship: &GraphRelationshipMatch,
    conditions: &mut Vec<String>,
    args: &mut Vec<expression::Literal>,
) -> Result<()> {
    for (key, value) in &relationship.props {
        validate_json_key(key)?;
        let json_value = format!("GET_JSON_OBJECT(e.props, '$.{key}')");
        match value {
            Value::String(s) => {
                conditions.push(format!("{json_value} = ?"));
                args.push(lit_str(s));
            }
            Value::Int(n) => {
                conditions.push(format!("CAST({json_value} AS BIGINT) = ?"));
                args.push(lit_long(*n));
            }
            Value::Float(f) => {
                conditions.push(format!("CAST({json_value} AS DOUBLE) = ?"));
                args.push(lit_double(*f));
            }
            Value::Bool(b) => {
                conditions.push(format!("CAST({json_value} AS BOOLEAN) = ?"));
                args.push(lit_bool(*b));
            }
            Value::Null => conditions.push(format!("{json_value} IS NULL")),
            Value::DateTime(_)
            | Value::Decimal(_)
            | Value::Duration(_)
            | Value::StringArray(_)
            | Value::IntArray(_)
            | Value::FloatArray(_)
            | Value::Path(_)
            | Value::Json(_) => {
                let json = serde_json::to_string(&value.to_json())
                    .map_err(|err| GrustError::Serialization(err.to_string()))?;
                conditions.push(format!("{json_value} = ?"));
                args.push(lit_str(&json));
            }
        }
    }
    append_property_predicate_conditions("e.props", &relationship.predicates, conditions, args)?;
    Ok(())
}

fn append_node_match_conditions(
    alias: &str,
    node: &GraphNodeMatch,
    conditions: &mut Vec<String>,
    args: &mut Vec<expression::Literal>,
) -> Result<()> {
    if let Some(label) = &node.label {
        conditions.push(format!("{alias}.label = ?"));
        args.push(lit_str(label.as_str()));
    }
    for (key, value) in &node.props {
        if key == "id" {
            let Some(id) = value.as_str() else {
                return Err(cypher_unresolved_identity(
                    "relationship endpoint id predicate must be a string",
                ));
            };
            conditions.push(format!("{alias}.id = ?"));
            args.push(lit_str(id));
            continue;
        }
        validate_json_key(key)?;
        let json_value = format!("GET_JSON_OBJECT({alias}.props, '$.{key}')");
        match value {
            Value::String(s) => {
                conditions.push(format!("{json_value} = ?"));
                args.push(lit_str(s));
            }
            Value::Int(n) => {
                conditions.push(format!("CAST({json_value} AS BIGINT) = ?"));
                args.push(lit_long(*n));
            }
            Value::Float(f) => {
                conditions.push(format!("CAST({json_value} AS DOUBLE) = ?"));
                args.push(lit_double(*f));
            }
            Value::Bool(b) => {
                conditions.push(format!("CAST({json_value} AS BOOLEAN) = ?"));
                args.push(lit_bool(*b));
            }
            Value::Null => conditions.push(format!("{json_value} IS NULL")),
            Value::DateTime(_)
            | Value::Decimal(_)
            | Value::Duration(_)
            | Value::StringArray(_)
            | Value::IntArray(_)
            | Value::FloatArray(_)
            | Value::Path(_)
            | Value::Json(_) => {
                let json = serde_json::to_string(&value.to_json())
                    .map_err(|err| GrustError::Serialization(err.to_string()))?;
                conditions.push(format!("{json_value} = ?"));
                args.push(lit_str(&json));
            }
        }
    }
    append_property_predicate_conditions(
        &format!("{alias}.props"),
        &node.predicates,
        conditions,
        args,
    )?;
    Ok(())
}

fn append_property_predicate_conditions(
    props_expr: &str,
    predicates: &[GraphPropertyPredicate],
    conditions: &mut Vec<String>,
    args: &mut Vec<expression::Literal>,
) -> Result<()> {
    for predicate in predicates {
        validate_json_key(&predicate.key)?;
        let json_value = sail_json_property_expr(props_expr, &predicate.key)?;
        match predicate.op {
            GraphPredicateOp::Equal => {
                append_property_equality_condition(&json_value, &predicate.value, conditions, args)?
            }
            GraphPredicateOp::NotEqual => append_property_inequality_condition(
                &json_value,
                &predicate.value,
                conditions,
                args,
            )?,
            GraphPredicateOp::IsNull => conditions.push(format!("{json_value} IS NULL")),
            GraphPredicateOp::IsNotNull => conditions.push(format!("{json_value} IS NOT NULL")),
            GraphPredicateOp::StartsWith
            | GraphPredicateOp::NotStartsWith
            | GraphPredicateOp::StartsWithAny
            | GraphPredicateOp::NotStartsWithAny
            | GraphPredicateOp::EndsWith
            | GraphPredicateOp::NotEndsWith
            | GraphPredicateOp::EndsWithAny
            | GraphPredicateOp::NotEndsWithAny
            | GraphPredicateOp::Contains
            | GraphPredicateOp::NotContains
            | GraphPredicateOp::ContainsAny
            | GraphPredicateOp::NotContainsAny => append_property_string_predicate_condition(
                &json_value,
                predicate.op,
                &predicate.value,
                conditions,
                args,
            )?,
            GraphPredicateOp::In | GraphPredicateOp::NotIn => append_property_in_condition(
                &json_value,
                predicate.op,
                &predicate.value,
                conditions,
                args,
            )?,
            GraphPredicateOp::GreaterThan
            | GraphPredicateOp::GreaterThanOrEqual
            | GraphPredicateOp::LessThan
            | GraphPredicateOp::LessThanOrEqual => append_property_order_condition(
                &json_value,
                predicate.op,
                &predicate.value,
                conditions,
                args,
            )?,
        }
    }
    Ok(())
}

fn append_property_equality_condition(
    json_value: &str,
    value: &Value,
    conditions: &mut Vec<String>,
    args: &mut Vec<expression::Literal>,
) -> Result<()> {
    match value {
        Value::String(s) => {
            conditions.push(format!("{json_value} = ?"));
            args.push(lit_str(s));
        }
        Value::Int(n) => {
            conditions.push(format!("CAST({json_value} AS BIGINT) = ?"));
            args.push(lit_long(*n));
        }
        Value::Float(f) => {
            conditions.push(format!("CAST({json_value} AS DOUBLE) = ?"));
            args.push(lit_double(*f));
        }
        Value::Bool(b) => {
            conditions.push(format!("CAST({json_value} AS BOOLEAN) = ?"));
            args.push(lit_bool(*b));
        }
        Value::Null => conditions.push(format!("{json_value} IS NULL")),
        Value::DateTime(_)
        | Value::Decimal(_)
        | Value::Duration(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Path(_)
        | Value::Json(_) => {
            let json = serde_json::to_string(&value.to_json())
                .map_err(|err| GrustError::Serialization(err.to_string()))?;
            conditions.push(format!("{json_value} = ?"));
            args.push(lit_str(&json));
        }
    }
    Ok(())
}

fn append_property_inequality_condition(
    json_value: &str,
    value: &Value,
    conditions: &mut Vec<String>,
    args: &mut Vec<expression::Literal>,
) -> Result<()> {
    match value {
        Value::Null => conditions.push(format!("{json_value} IS NOT NULL")),
        _ => {
            let mut inner_conditions = Vec::new();
            let mut inner_args = Vec::new();
            append_property_equality_condition(
                json_value,
                value,
                &mut inner_conditions,
                &mut inner_args,
            )?;
            let Some(condition) = inner_conditions.into_iter().next() else {
                return Ok(());
            };
            conditions.push(format!("{json_value} IS NOT NULL AND NOT ({condition})"));
            args.extend(inner_args);
        }
    }
    Ok(())
}

fn append_property_string_predicate_condition(
    json_value: &str,
    op: GraphPredicateOp,
    value: &Value,
    conditions: &mut Vec<String>,
    args: &mut Vec<expression::Literal>,
) -> Result<()> {
    match op {
        GraphPredicateOp::StartsWith => {
            let needle = cypher_string_predicate_needle(value)?;
            conditions.push(format!("STARTSWITH({json_value}, ?)"));
            args.push(lit_str(needle));
        }
        GraphPredicateOp::NotStartsWith => {
            let needle = cypher_string_predicate_needle(value)?;
            conditions.push(format!(
                "{json_value} IS NOT NULL AND NOT STARTSWITH({json_value}, ?)"
            ));
            args.push(lit_str(needle));
        }
        GraphPredicateOp::StartsWithAny => append_property_string_any_condition(
            json_value,
            "STARTSWITH",
            false,
            value,
            conditions,
            args,
        )?,
        GraphPredicateOp::NotStartsWithAny => append_property_string_any_condition(
            json_value,
            "STARTSWITH",
            true,
            value,
            conditions,
            args,
        )?,
        GraphPredicateOp::EndsWith => {
            let needle = cypher_string_predicate_needle(value)?;
            conditions.push(format!("ENDSWITH({json_value}, ?)"));
            args.push(lit_str(needle));
        }
        GraphPredicateOp::NotEndsWith => {
            let needle = cypher_string_predicate_needle(value)?;
            conditions.push(format!(
                "{json_value} IS NOT NULL AND NOT ENDSWITH({json_value}, ?)"
            ));
            args.push(lit_str(needle));
        }
        GraphPredicateOp::EndsWithAny => append_property_string_any_condition(
            json_value, "ENDSWITH", false, value, conditions, args,
        )?,
        GraphPredicateOp::NotEndsWithAny => append_property_string_any_condition(
            json_value, "ENDSWITH", true, value, conditions, args,
        )?,
        GraphPredicateOp::Contains => {
            let needle = cypher_string_predicate_needle(value)?;
            conditions.push(format!("CONTAINS({json_value}, ?)"));
            args.push(lit_str(needle));
        }
        GraphPredicateOp::NotContains => {
            let needle = cypher_string_predicate_needle(value)?;
            conditions.push(format!(
                "{json_value} IS NOT NULL AND NOT CONTAINS({json_value}, ?)"
            ));
            args.push(lit_str(needle));
        }
        GraphPredicateOp::ContainsAny => append_property_string_any_condition(
            json_value, "CONTAINS", false, value, conditions, args,
        )?,
        GraphPredicateOp::NotContainsAny => append_property_string_any_condition(
            json_value, "CONTAINS", true, value, conditions, args,
        )?,
        GraphPredicateOp::Equal
        | GraphPredicateOp::NotEqual
        | GraphPredicateOp::IsNull
        | GraphPredicateOp::IsNotNull
        | GraphPredicateOp::In
        | GraphPredicateOp::NotIn
        | GraphPredicateOp::GreaterThan
        | GraphPredicateOp::GreaterThanOrEqual
        | GraphPredicateOp::LessThan
        | GraphPredicateOp::LessThanOrEqual => unreachable!(),
    }
    Ok(())
}

fn cypher_string_predicate_needle(value: &Value) -> Result<&str> {
    value.as_str().ok_or_else(|| {
        cypher_syntax("MATCH WHERE string predicates require string literals or parameters")
    })
}

fn cypher_string_predicate_needles(value: &Value) -> Result<&[String]> {
    value.as_string_array().ok_or_else(|| {
        cypher_syntax(
            "MATCH WHERE string predicate OR groups require string literals or parameters",
        )
    })
}

fn append_property_string_any_condition(
    json_value: &str,
    sql_function: &str,
    negated: bool,
    value: &Value,
    conditions: &mut Vec<String>,
    args: &mut Vec<expression::Literal>,
) -> Result<()> {
    let needles = cypher_string_predicate_needles(value)?;
    if needles.is_empty() {
        conditions.push(if negated {
            format!("{json_value} IS NOT NULL")
        } else {
            "1 = 0".to_string()
        });
        return Ok(());
    }
    let inner = needles
        .iter()
        .map(|_| format!("{sql_function}({json_value}, ?)"))
        .collect::<Vec<_>>()
        .join(" OR ");
    if negated {
        conditions.push(format!("{json_value} IS NOT NULL AND NOT ({inner})"));
    } else {
        conditions.push(format!("({inner})"));
    }
    for needle in needles {
        args.push(lit_str(needle));
    }
    Ok(())
}

fn append_property_in_condition(
    json_value: &str,
    op: GraphPredicateOp,
    value: &Value,
    conditions: &mut Vec<String>,
    args: &mut Vec<expression::Literal>,
) -> Result<()> {
    let values = cypher_in_predicate_values(value)?;
    if values.is_empty() {
        match op {
            GraphPredicateOp::In => conditions.push("1 = 0".to_string()),
            GraphPredicateOp::NotIn => conditions.push(format!("{json_value} IS NOT NULL")),
            _ => unreachable!(),
        }
        return Ok(());
    }

    let mut equality_conditions = Vec::new();
    let mut equality_args = Vec::new();
    for value in values {
        append_property_equality_condition(
            json_value,
            &value,
            &mut equality_conditions,
            &mut equality_args,
        )?;
    }
    let condition = equality_conditions.join(" OR ");
    match op {
        GraphPredicateOp::In => conditions.push(format!("({condition})")),
        GraphPredicateOp::NotIn => {
            conditions.push(format!("{json_value} IS NOT NULL AND NOT ({condition})"))
        }
        _ => unreachable!(),
    }
    args.extend(equality_args);
    Ok(())
}

fn append_property_order_condition(
    json_value: &str,
    op: GraphPredicateOp,
    value: &Value,
    conditions: &mut Vec<String>,
    args: &mut Vec<expression::Literal>,
) -> Result<()> {
    let sql_op = match op {
        GraphPredicateOp::GreaterThan => ">",
        GraphPredicateOp::GreaterThanOrEqual => ">=",
        GraphPredicateOp::LessThan => "<",
        GraphPredicateOp::LessThanOrEqual => "<=",
        GraphPredicateOp::Equal
        | GraphPredicateOp::NotEqual
        | GraphPredicateOp::IsNull
        | GraphPredicateOp::IsNotNull
        | GraphPredicateOp::StartsWith
        | GraphPredicateOp::NotStartsWith
        | GraphPredicateOp::StartsWithAny
        | GraphPredicateOp::NotStartsWithAny
        | GraphPredicateOp::EndsWith
        | GraphPredicateOp::NotEndsWith
        | GraphPredicateOp::EndsWithAny
        | GraphPredicateOp::NotEndsWithAny
        | GraphPredicateOp::Contains
        | GraphPredicateOp::NotContains
        | GraphPredicateOp::ContainsAny
        | GraphPredicateOp::NotContainsAny
        | GraphPredicateOp::In
        | GraphPredicateOp::NotIn => unreachable!(),
    };
    match value {
        Value::Int(n) => {
            conditions.push(format!("CAST({json_value} AS BIGINT) {sql_op} ?"));
            args.push(lit_long(*n));
        }
        Value::Float(f) => {
            conditions.push(format!("CAST({json_value} AS DOUBLE) {sql_op} ?"));
            args.push(lit_double(*f));
        }
        Value::String(s) => {
            conditions.push(format!("{json_value} {sql_op} ?"));
            args.push(lit_str(s));
        }
        _ => {
            return Err(cypher_syntax(
                "MATCH WHERE ordered comparisons require integer, float, or string literals",
            ));
        }
    }
    Ok(())
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

fn validate_cypher_constraint_registry_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(GrustError::Schema(
            "Cypher constraint registry name must not be empty".to_string(),
        ));
    }
    Ok(())
}

fn create_cypher_constraint_registry_table_sql() -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {} \
         (name STRING NOT NULL, registry_json STRING NOT NULL) USING delta",
        CYPHER_CONSTRAINT_REGISTRY_TABLE
    )
}

fn upsert_cypher_constraint_registry_sql(name: &str, registry_json: &str) -> Result<String> {
    validate_cypher_constraint_registry_name(name)?;
    Ok(format!(
        "MERGE INTO {table} AS t \
         USING (SELECT {name} AS name, {registry_json} AS registry_json) AS s \
         ON t.name = s.name \
         WHEN MATCHED THEN UPDATE SET t.registry_json = s.registry_json \
         WHEN NOT MATCHED THEN INSERT (name, registry_json) VALUES (s.name, s.registry_json)",
        table = CYPHER_CONSTRAINT_REGISTRY_TABLE,
        name = sql_str(name),
        registry_json = sql_str(registry_json),
    ))
}

fn select_cypher_constraint_registry_sql(name: &str) -> Result<String> {
    validate_cypher_constraint_registry_name(name)?;
    Ok(format!(
        "SELECT registry_json FROM {} WHERE name = {} LIMIT 1",
        CYPHER_CONSTRAINT_REGISTRY_TABLE,
        sql_str(name)
    ))
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

fn parse_optional_single_string_from_arrow(
    chunks: &[Vec<u8>],
    column_name: &str,
    context: &str,
) -> Result<Option<String>> {
    let mut value = None;
    for data in chunks {
        let reader = StreamReader::try_new(Cursor::new(data), None)
            .map_err(|e| GrustError::Backend(format!("Arrow IPC read failed: {e}")))?;
        let schema = reader.schema();
        let column_idx = schema
            .index_of(column_name)
            .map_err(|_| GrustError::Schema(format!("{context} missing '{column_name}' column")))?;
        for batch in reader {
            let batch =
                batch.map_err(|e| GrustError::Backend(format!("Arrow batch error: {e}")))?;
            let values = string_column(&batch, column_idx, column_name)?;
            for row in 0..batch.num_rows() {
                if values.is_null(row) {
                    return Err(GrustError::Schema(format!(
                        "{context} column '{column_name}' must not be null"
                    )));
                }
                if value.is_some() {
                    return Err(GrustError::Schema(format!(
                        "{context} returned more than one row"
                    )));
                }
                value = Some(values.value(row).to_string());
            }
        }
    }
    Ok(value)
}

/// Property type hints for read pushdown, derived from the applied graph schema.
/// Lets Spark push numeric `ORDER BY` by casting JSON sort keys to their declared
/// type (only `Int`/`Float`/`String` are mapped; other field types stay unpushed).
struct SailTypeHints {
    schema: Option<GraphSchema>,
}

impl grust_cypher::pushdown::TypeHints for SailTypeHints {
    fn node_property_kind(
        &self,
        label: Option<&str>,
        key: &str,
    ) -> Option<grust_cypher::pushdown::ScalarKind> {
        let label = label?;
        let node_type = self
            .schema
            .as_ref()?
            .nodes
            .iter()
            .find(|n| n.label.as_str() == label)?;
        let field = node_type.fields.iter().find(|f| f.name == key)?;
        scalar_kind_of(&field.ty)
    }

    fn edge_property_kind(
        &self,
        edge_type: Option<&str>,
        key: &str,
    ) -> Option<grust_cypher::pushdown::ScalarKind> {
        let edge_type = edge_type?;
        let edge = self
            .schema
            .as_ref()?
            .edges
            .iter()
            .find(|e| e.label.as_str() == edge_type)?;
        let field = edge.fields.iter().find(|f| f.name == key)?;
        scalar_kind_of(&field.ty)
    }
}

/// Map a schema [`FieldType`] to the pushdown [`ScalarKind`] used for ORDER BY
/// casts (only the cleanly-orderable scalar types map).
fn scalar_kind_of(ty: &FieldType) -> Option<grust_cypher::pushdown::ScalarKind> {
    use grust_cypher::pushdown::ScalarKind;
    match ty {
        FieldType::Int => Some(ScalarKind::Int),
        FieldType::Float => Some(ScalarKind::Float),
        FieldType::String => Some(ScalarKind::Str),
        _ => None,
    }
}

/// Parse an Arrow IPC chunk whose columns are all UTF-8 strings into rows of
/// optional text cells, preserving column order. Used by segment pushdown.
fn parse_text_rows_from_arrow(data: &[u8]) -> Result<Vec<Vec<Option<String>>>> {
    let reader = StreamReader::try_new(Cursor::new(data), None)
        .map_err(|e| GrustError::Backend(format!("Arrow IPC read failed: {e}")))?;
    let mut out = Vec::new();
    for batch in reader {
        let batch = batch.map_err(|e| GrustError::Backend(format!("Arrow batch error: {e}")))?;
        let columns: Vec<&StringArray> = (0..batch.num_columns())
            .map(|i| {
                batch
                    .column(i)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| {
                        GrustError::Schema(format!("pushdown result column {i} is not a string"))
                    })
            })
            .collect::<Result<_>>()?;
        for row in 0..batch.num_rows() {
            out.push(
                columns
                    .iter()
                    .map(|c| {
                        if c.is_null(row) {
                            None
                        } else {
                            Some(c.value(row).to_string())
                        }
                    })
                    .collect(),
            );
        }
    }
    Ok(out)
}

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

fn parse_triplets_from_arrow(data: &[u8]) -> Result<Vec<SailTripletRow>> {
    let reader = StreamReader::try_new(Cursor::new(data), None)
        .map_err(|e| GrustError::Backend(format!("Arrow IPC read failed: {e}")))?;
    let schema = reader.schema();
    let src_id_idx = schema
        .index_of("src_id")
        .map_err(|_| GrustError::Schema("triplet result missing 'src_id' column".into()))?;
    let src_label_idx = schema
        .index_of("src_label")
        .map_err(|_| GrustError::Schema("triplet result missing 'src_label' column".into()))?;
    let src_props_idx = schema
        .index_of("src_props")
        .map_err(|_| GrustError::Schema("triplet result missing 'src_props' column".into()))?;
    let edge_id_idx = schema.index_of("edge_id").ok();
    let edge_src_id_idx = schema
        .index_of("edge_src_id")
        .map_err(|_| GrustError::Schema("triplet result missing 'edge_src_id' column".into()))?;
    let edge_dst_id_idx = schema
        .index_of("edge_dst_id")
        .map_err(|_| GrustError::Schema("triplet result missing 'edge_dst_id' column".into()))?;
    let edge_type_idx = schema
        .index_of("edge_type")
        .map_err(|_| GrustError::Schema("triplet result missing 'edge_type' column".into()))?;
    let edge_props_idx = schema
        .index_of("edge_props")
        .map_err(|_| GrustError::Schema("triplet result missing 'edge_props' column".into()))?;
    let dst_id_idx = schema
        .index_of("dst_id")
        .map_err(|_| GrustError::Schema("triplet result missing 'dst_id' column".into()))?;
    let dst_label_idx = schema
        .index_of("dst_label")
        .map_err(|_| GrustError::Schema("triplet result missing 'dst_label' column".into()))?;
    let dst_props_idx = schema
        .index_of("dst_props")
        .map_err(|_| GrustError::Schema("triplet result missing 'dst_props' column".into()))?;

    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch.map_err(|e| GrustError::Backend(format!("Arrow batch error: {e}")))?;
        let src_ids = string_column(&batch, src_id_idx, "src_id")?;
        let src_labels = string_column(&batch, src_label_idx, "src_label")?;
        let src_props = string_column(&batch, src_props_idx, "src_props")?;
        let edge_ids = if let Some(edge_id_idx) = edge_id_idx {
            Some(string_column(&batch, edge_id_idx, "edge_id")?)
        } else {
            None
        };
        let edge_src_ids = string_column(&batch, edge_src_id_idx, "edge_src_id")?;
        let edge_dst_ids = string_column(&batch, edge_dst_id_idx, "edge_dst_id")?;
        let edge_types = string_column(&batch, edge_type_idx, "edge_type")?;
        let edge_props = string_column(&batch, edge_props_idx, "edge_props")?;
        let dst_ids = string_column(&batch, dst_id_idx, "dst_id")?;
        let dst_labels = string_column(&batch, dst_label_idx, "dst_label")?;
        let dst_props = string_column(&batch, dst_props_idx, "dst_props")?;

        for i in 0..batch.num_rows() {
            let edge_id = edge_ids.and_then(|ids| {
                if ids.is_null(i) || ids.value(i).is_empty() {
                    None
                } else {
                    Some(EdgeId::new(ids.value(i)))
                }
            });
            rows.push(SailTripletRow {
                src: Node {
                    id: NodeId::new(src_ids.value(i)),
                    label: Label::new(src_labels.value(i)),
                    props: props_column_value(src_props, i)?,
                },
                edge: Edge {
                    id: edge_id,
                    from: NodeId::new(edge_src_ids.value(i)),
                    to: NodeId::new(edge_dst_ids.value(i)),
                    label: Label::new(edge_types.value(i)),
                    props: props_column_value(edge_props, i)?,
                },
                dst: Node {
                    id: NodeId::new(dst_ids.value(i)),
                    label: Label::new(dst_labels.value(i)),
                    props: props_column_value(dst_props, i)?,
                },
            });
        }
    }
    Ok(rows)
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

fn props_column_value(column: &StringArray, row: usize) -> Result<Props> {
    if column.is_null(row) || column.value(row).is_empty() {
        Ok(Props::new())
    } else {
        props_from_json(column.value(row))
    }
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
