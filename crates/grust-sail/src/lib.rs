use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Cursor;
use std::sync::{Arc, RwLock};

use arrow::array::{Array as _, Int64Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field as ArrowField, Schema as ArrowSchema};
use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use async_trait::async_trait;
use grust_core::prelude::*;
use serde::{Deserialize, Serialize};
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

pub type CypherMutationReport = GraphMutationReport;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherCreateMode {
    UpsertCompatible,
    ErrorIfExists,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherNodeIdPolicy {
    ExplicitOnly,
    GenerateForCreate,
}

impl Default for CypherNodeIdPolicy {
    fn default() -> Self {
        Self::ExplicitOnly
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherRelationshipIdPolicy {
    ExplicitOnly,
    GenerateForRowCreate,
    GenerateForRowCreateAndMerge,
}

impl Default for CypherRelationshipIdPolicy {
    fn default() -> Self {
        Self::ExplicitOnly
    }
}

pub type CypherParameters = BTreeMap<String, Value>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherNullAssignment {
    /// Preserve Grust's value model: `SET n.key = null` stores `Value::Null`.
    StoreNull,
    /// Cypher-compatibility mode: `SET n.key = null` lowers to `REMOVE n.key`.
    RemoveProperty,
}

impl Default for CypherNullAssignment {
    fn default() -> Self {
        Self::StoreNull
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherMutationOptions {
    pub create_mode: CypherCreateMode,
    pub node_id_policy: CypherNodeIdPolicy,
    pub relationship_id_policy: CypherRelationshipIdPolicy,
    /// Collect written node identities in `CypherMutationResult`.
    ///
    /// This is opt-in so broad write result payloads remain deliberate. The
    /// mutation report remains count-oriented either way.
    pub collect_written_node_identities: bool,
    /// Collect written edge identities in `CypherMutationResult`.
    ///
    /// This is opt-in because row-producing edge writes can materialize a large
    /// identity vector. The mutation report remains count-oriented either way.
    pub collect_written_edge_identities: bool,
    /// Controls how explicit property assignment to `null` is planned.
    ///
    /// This applies to `SET n.key = null` and `SET e.key = null`; map patches
    /// such as `SET n += {key: null}` always store `Value::Null`.
    pub null_assignment: CypherNullAssignment,
    pub parameters: CypherParameters,
}

impl Default for CypherMutationOptions {
    fn default() -> Self {
        Self {
            create_mode: CypherCreateMode::UpsertCompatible,
            node_id_policy: CypherNodeIdPolicy::ExplicitOnly,
            relationship_id_policy: CypherRelationshipIdPolicy::ExplicitOnly,
            collect_written_node_identities: false,
            collect_written_edge_identities: false,
            null_assignment: CypherNullAssignment::StoreNull,
            parameters: CypherParameters::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CypherGeneratedNodeId {
    pub variable: Option<String>,
    pub id: NodeId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CypherWrittenNodeIdentity {
    pub kind: GraphMutationPlanKind,
    pub label: Label,
    pub id: NodeId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CypherWrittenEdgeIdentity {
    pub kind: GraphMutationPlanKind,
    pub from: NodeId,
    pub label: Label,
    pub to: NodeId,
    pub id: Option<EdgeId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CypherMutationResult {
    pub report: CypherMutationReport,
    pub generated_node_ids: Vec<CypherGeneratedNodeId>,
    pub written_node_identities: Vec<CypherWrittenNodeIdentity>,
    pub written_edge_identities: Vec<CypherWrittenEdgeIdentity>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherResultTable {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherMutationTableResult {
    pub mutation: CypherMutationResult,
    pub table: CypherResultTable,
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

pub fn sail_cypher_mutation_plan(cypher: &str) -> Result<GraphMutationPlan> {
    let (plan, _) =
        sail_cypher_mutation_plan_with_options(cypher, CypherMutationOptions::default())?;
    Ok(plan)
}

/// A parsed Cypher schema (DDL) statement.
///
/// DDL is deliberately kept separate from the data-mutation plan: constraint
/// statements describe schema intent that callers apply to a [`GraphSchema`]
/// (and then to a backend through [`GraphStore::apply_schema`]), rather than
/// flowing through [`GraphMutationStore`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CypherDdlStatement {
    /// `CREATE CONSTRAINT [name] [IF NOT EXISTS] FOR ... REQUIRE ... IS ...`.
    CreateConstraint {
        name: Option<String>,
        if_not_exists: bool,
        constraint: GraphConstraint,
    },
    /// `DROP CONSTRAINT name [IF EXISTS]`.
    DropConstraint { name: String, if_exists: bool },
}

/// A named Cypher constraint stored outside [`GraphSchema`].
///
/// `GraphSchema` remains the portable enforcement shape and stores unnamed
/// [`GraphConstraint`] values. This registry layer preserves Cypher constraint
/// names so callers can apply `DROP CONSTRAINT name` deterministically before
/// passing the resulting constraints into a schema.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NamedGraphConstraint {
    pub name: String,
    pub constraint: GraphConstraint,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CypherDdlApplicationReport {
    pub created: usize,
    pub skipped: usize,
    pub dropped: usize,
    pub missing: usize,
}

impl CypherDdlApplicationReport {
    fn merge(&mut self, other: Self) {
        self.created += other.created;
        self.skipped += other.skipped;
        self.dropped += other.dropped;
        self.missing += other.missing;
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CypherSchemaApplication {
    pub schema: GraphSchema,
    pub report: CypherDdlApplicationReport,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CypherSchemaManager {
    pub schema: GraphSchema,
    pub registry: CypherConstraintRegistry,
}

impl CypherSchemaManager {
    pub fn new(schema: GraphSchema) -> Self {
        let registry = CypherConstraintRegistry::from_schema(&schema);
        Self { schema, registry }
    }

    pub fn with_registry(schema: GraphSchema, registry: CypherConstraintRegistry) -> Self {
        Self { schema, registry }
    }

    pub fn from_registry_json(schema: GraphSchema, registry_json: &str) -> Result<Self> {
        Ok(Self::with_registry(
            schema,
            CypherConstraintRegistry::from_json(registry_json)?,
        ))
    }

    pub fn registry_json(&self) -> Result<String> {
        self.registry.to_json()
    }

    pub async fn apply_cypher_ddl<S>(
        &mut self,
        store: &S,
        cypher: &str,
    ) -> Result<CypherSchemaApplication>
    where
        S: GraphStore + Sync,
    {
        let applied =
            apply_cypher_ddl_to_schema(store, &self.schema, &mut self.registry, cypher).await?;
        self.schema = applied.schema.clone();
        Ok(applied)
    }
}

/// Named constraint metadata for applying parsed Cypher DDL.
///
/// The registry is intentionally separate from backend persistence. Callers can
/// parse DDL with [`sail_cypher_ddl`], apply it here, then build or update a
/// [`GraphSchema`] from [`CypherConstraintRegistry::constraints`] before calling
/// [`GraphStore::apply_schema`]. [`CypherConstraintRegistry::to_json`] and
/// [`CypherConstraintRegistry::from_json`] provide a caller-owned persistence
/// hook for storing that named metadata outside backend-native schema storage.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CypherConstraintRegistry {
    named: BTreeMap<String, GraphConstraint>,
    anonymous: Vec<GraphConstraint>,
}

impl CypherConstraintRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_schema(schema: &GraphSchema) -> Self {
        Self {
            named: BTreeMap::new(),
            anonymous: schema.constraints.clone(),
        }
    }

    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|err| {
            GrustError::Serialization(format!(
                "Cypher constraint registry JSON parse error: {err}"
            ))
        })
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|err| {
            GrustError::Serialization(format!(
                "Cypher constraint registry JSON serialization error: {err}"
            ))
        })
    }

    pub fn named_constraints(&self) -> Vec<NamedGraphConstraint> {
        self.named
            .iter()
            .map(|(name, constraint)| NamedGraphConstraint {
                name: name.clone(),
                constraint: constraint.clone(),
            })
            .collect()
    }

    pub fn anonymous_constraints(&self) -> &[GraphConstraint] {
        &self.anonymous
    }

    pub fn constraints(&self) -> Vec<GraphConstraint> {
        self.named
            .values()
            .cloned()
            .chain(self.anonymous.iter().cloned())
            .collect()
    }

    pub fn apply_to_schema(&self, schema: &GraphSchema) -> GraphSchema {
        let mut schema = schema.clone();
        schema.constraints = self.constraints();
        schema
    }

    pub fn apply_statement(
        &mut self,
        statement: CypherDdlStatement,
    ) -> Result<CypherDdlApplicationReport> {
        match statement {
            CypherDdlStatement::CreateConstraint {
                name,
                if_not_exists,
                constraint,
            } => {
                if let Some(name) = name {
                    if self.named.contains_key(&name) {
                        if if_not_exists {
                            return Ok(CypherDdlApplicationReport {
                                skipped: 1,
                                ..Default::default()
                            });
                        }
                        return Err(GrustError::CypherExecution(format!(
                            "constraint '{name}' already exists"
                        )));
                    }
                    self.named.insert(name, constraint);
                } else {
                    self.anonymous.push(constraint);
                }
                Ok(CypherDdlApplicationReport {
                    created: 1,
                    ..Default::default()
                })
            }
            CypherDdlStatement::DropConstraint { name, if_exists } => {
                if self.named.remove(&name).is_some() {
                    return Ok(CypherDdlApplicationReport {
                        dropped: 1,
                        ..Default::default()
                    });
                }
                if if_exists {
                    return Ok(CypherDdlApplicationReport {
                        missing: 1,
                        ..Default::default()
                    });
                }
                Err(GrustError::CypherExecution(format!(
                    "constraint '{name}' does not exist"
                )))
            }
        }
    }

    pub fn apply_statements(
        &mut self,
        statements: impl IntoIterator<Item = CypherDdlStatement>,
    ) -> Result<CypherDdlApplicationReport> {
        let mut next = self.clone();
        let mut report = CypherDdlApplicationReport::default();
        for statement in statements {
            report.merge(next.apply_statement(statement)?);
        }
        *self = next;
        Ok(report)
    }

    pub fn apply_cypher(&mut self, cypher: &str) -> Result<CypherDdlApplicationReport> {
        self.apply_statements(sail_cypher_ddl(cypher)?)
    }
}

pub async fn apply_cypher_ddl_to_schema<S>(
    store: &S,
    schema: &GraphSchema,
    registry: &mut CypherConstraintRegistry,
    cypher: &str,
) -> Result<CypherSchemaApplication>
where
    S: GraphStore + Sync,
{
    let mut next = registry.clone();
    let report = next.apply_cypher(cypher)?;
    let schema = next.apply_to_schema(schema);
    store.apply_schema(&schema).await?;
    *registry = next;
    Ok(CypherSchemaApplication { schema, report })
}

/// Parses one or more Cypher DDL statements (currently `CREATE CONSTRAINT` and
/// `DROP CONSTRAINT`) into backend-neutral [`CypherDdlStatement`] values.
///
/// Supported constraint forms:
///
/// ```cypher
/// CREATE CONSTRAINT person_id IF NOT EXISTS
/// FOR (n:Person) REQUIRE n.id IS UNIQUE;
/// CREATE CONSTRAINT FOR (n:Person) REQUIRE n.name IS NOT NULL;
/// CREATE CONSTRAINT FOR ()-[r:KNOWS]-() REQUIRE r.since IS NOT NULL;
/// DROP CONSTRAINT person_id IF EXISTS;
/// ```
///
/// The legacy `ON ... ASSERT ...` spelling is accepted as a synonym for
/// `FOR ... REQUIRE ...`. Composite/node-key constraints, index DDL, and
/// property existence on multiple keys are rejected with a clear error.
pub fn sail_cypher_ddl(cypher: &str) -> Result<Vec<CypherDdlStatement>> {
    let cypher = strip_cypher_comments(cypher)?;
    let statements = split_cypher_statements(&cypher)?;
    if statements.is_empty() {
        return Err(cypher_syntax("Cypher DDL statement is empty"));
    }
    statements
        .into_iter()
        .map(|statement| parse_cypher_ddl_statement(statement.trim()))
        .collect()
}

/// Parses Cypher constraint DDL and returns only the resulting
/// [`GraphConstraint`] values, discarding names and `IF [NOT] EXISTS` flags.
///
/// `DROP CONSTRAINT` statements are rejected because they carry no constraint
/// body; use [`sail_cypher_ddl`] when those are needed.
pub fn sail_cypher_constraints(cypher: &str) -> Result<Vec<GraphConstraint>> {
    sail_cypher_ddl(cypher)?
        .into_iter()
        .map(|statement| match statement {
            CypherDdlStatement::CreateConstraint { constraint, .. } => Ok(constraint),
            CypherDdlStatement::DropConstraint { .. } => Err(cypher_syntax(
                "sail_cypher_constraints does not accept DROP CONSTRAINT statements",
            )),
        })
        .collect()
}

fn sail_cypher_mutation_plan_with_options(
    cypher: &str,
    options: CypherMutationOptions,
) -> Result<(GraphMutationPlan, Vec<CypherGeneratedNodeId>)> {
    let cypher = strip_cypher_comments(cypher)?;
    let statements = split_cypher_statements(&cypher)?;
    if statements.is_empty() {
        return Err(cypher_syntax("writable Cypher statement is empty"));
    }

    let mut planner = CypherMutationPlanner {
        node_id_policy: options.node_id_policy,
        relationship_id_policy: options.relationship_id_policy,
        null_assignment: options.null_assignment,
        parameters: options.parameters,
        ..CypherMutationPlanner::default()
    };
    let mut plan = GraphMutationPlan::default();
    for statement in statements {
        for operation in planner.plan_statement(statement)?.operations {
            plan.push(operation);
        }
    }
    Ok((plan, planner.generated_node_ids))
}

struct CypherPlannedMutationWithReturn {
    plan: GraphMutationPlan,
    generated_node_ids: Vec<CypherGeneratedNodeId>,
    node_bindings: HashMap<String, NodeId>,
    edge_bindings: HashMap<String, CypherBoundEdgeIdentity>,
    row_node_bindings: HashMap<String, GraphNodeMatch>,
    row_edge_match_bindings: HashMap<String, GraphRelationshipMatch>,
    row_edge_bindings: HashMap<String, CypherRowProducedEdgeBinding>,
    row_path_bindings: HashMap<String, CypherRowProducedPathBinding>,
    return_clause: CypherReturnClause,
}

fn sail_cypher_mutation_plan_with_return_options(
    cypher: &str,
    options: CypherMutationOptions,
) -> Result<CypherPlannedMutationWithReturn> {
    let cypher = strip_cypher_comments(cypher)?;
    let mut statements = split_cypher_statements(&cypher)?;
    if statements.is_empty() {
        return Err(cypher_syntax("writable Cypher statement is empty"));
    }

    let final_statement = statements
        .pop()
        .expect("checked non-empty statement collection");
    for statement in &statements {
        if find_unquoted_keyword(statement, "RETURN").is_some() {
            return Err(cypher_syntax(
                "writable Cypher only supports RETURN on the final statement",
            ));
        }
    }
    let (final_mutation, return_clause) = split_final_return(final_statement)?;
    if final_mutation.trim().is_empty() {
        return Err(cypher_syntax(
            "writable Cypher RETURN requires a preceding mutation statement",
        ));
    }

    let mut planner = CypherMutationPlanner {
        node_id_policy: options.node_id_policy,
        relationship_id_policy: options.relationship_id_policy,
        null_assignment: options.null_assignment,
        parameters: options.parameters,
        bind_delete_return_rows: true,
        ..CypherMutationPlanner::default()
    };
    let mut plan = GraphMutationPlan::default();
    for statement in statements {
        for operation in planner.plan_statement(statement)?.operations {
            plan.push(operation);
        }
    }
    for operation in planner.plan_statement(final_mutation)?.operations {
        plan.push(operation);
    }
    let return_clause = parse_cypher_return_clause(
        return_clause,
        &planner.node_bindings,
        &planner.edge_bindings,
        &planner.row_node_bindings,
        &planner.row_edge_match_bindings,
        &planner.row_edge_bindings,
        &planner.row_path_bindings,
        &planner.parameters,
    )?;
    Ok(CypherPlannedMutationWithReturn {
        plan,
        generated_node_ids: planner.generated_node_ids,
        node_bindings: planner.node_bindings,
        edge_bindings: planner.edge_bindings,
        row_node_bindings: planner.row_node_bindings,
        row_edge_match_bindings: planner.row_edge_match_bindings,
        row_edge_bindings: planner.row_edge_bindings,
        row_path_bindings: planner.row_path_bindings,
        return_clause,
    })
}

#[derive(Default)]
struct CypherMutationPlanner {
    node_bindings: HashMap<String, NodeId>,
    edge_bindings: HashMap<String, CypherBoundEdgeIdentity>,
    row_node_bindings: HashMap<String, GraphNodeMatch>,
    row_edge_match_bindings: HashMap<String, GraphRelationshipMatch>,
    row_edge_bindings: HashMap<String, CypherRowProducedEdgeBinding>,
    row_path_bindings: HashMap<String, CypherRowProducedPathBinding>,
    node_id_policy: CypherNodeIdPolicy,
    relationship_id_policy: CypherRelationshipIdPolicy,
    null_assignment: CypherNullAssignment,
    parameters: CypherParameters,
    generated_node_ids: Vec<CypherGeneratedNodeId>,
    bind_delete_return_rows: bool,
}

impl CypherMutationPlanner {
    fn plan_statement(&mut self, cypher: &str) -> Result<GraphMutationPlan> {
        let cypher = cypher.trim();
        match cypher_parser::classify_statement(cypher)? {
            cypher_parser::CypherStatement::Match(rest) => self.parse_match(rest),
            cypher_parser::CypherStatement::Create(rest) => {
                self.parse_upsert(rest, GraphMutationPlanKind::Create)
            }
            cypher_parser::CypherStatement::Merge(rest) => {
                self.parse_upsert(rest, GraphMutationPlanKind::Merge)
            }
            cypher_parser::CypherStatement::Delete(rest) => self.parse_delete(rest),
        }
    }

    fn parse_upsert(
        &mut self,
        pattern: &str,
        kind: GraphMutationPlanKind,
    ) -> Result<GraphMutationPlan> {
        if find_unquoted_sequence(pattern, "->").is_some() {
            let parsed = self.parse_edge_pattern(pattern)?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::UpsertEdge {
                    kind,
                    edge: parsed.edge,
                },
            ]));
        }

        let (node, rest) = parse_cypher_node_pattern(pattern, &self.parameters)?;
        if !rest.trim().is_empty() {
            return Err(cypher_syntax(format!(
                "unsupported writable Cypher node pattern suffix: {}",
                rest.trim()
            )));
        }
        let label = node
            .label
            .clone()
            .ok_or_else(|| cypher_syntax("node CREATE/MERGE requires a label"))?;
        let id = match optional_string_prop(&node.props, "id") {
            Some(id) => id,
            None if kind == GraphMutationPlanKind::Create
                && self.node_id_policy == CypherNodeIdPolicy::GenerateForCreate =>
            {
                let id = format!("node-{}", uuid::Uuid::new_v4());
                self.generated_node_ids.push(CypherGeneratedNodeId {
                    variable: node.variable.clone(),
                    id: NodeId::new(id.clone()),
                });
                id
            }
            None => {
                return Err(cypher_unresolved_identity(
                    "node CREATE/MERGE requires explicit string property 'id'",
                ));
            }
        };
        self.bind_node_variable(&node, &NodeId::new(id.clone()))?;
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::UpsertNode {
                kind,
                node: Node::new(label, id, node.props),
            },
        ]))
    }

    fn parse_delete(&mut self, pattern: &str) -> Result<GraphMutationPlan> {
        if find_unquoted_sequence(pattern, "->").is_some() {
            let parsed = self.parse_edge_pattern(pattern)?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::DeleteEdge {
                    from: parsed.from_id,
                    label: parsed.edge.label,
                    to: parsed.to_id,
                },
            ]));
        }

        let (node, rest) = parse_cypher_node_pattern(pattern, &self.parameters)?;
        if !rest.trim().is_empty() {
            return Err(cypher_syntax(format!(
                "unsupported writable Cypher delete pattern suffix: {}",
                rest.trim()
            )));
        }
        let id = self.resolve_node_id(&node, "node DELETE")?;
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::DeleteNode(id),
        ]))
    }

    fn parse_match(&mut self, statement: &str) -> Result<GraphMutationPlan> {
        if find_unquoted_keyword(statement, "DELETE").is_some() {
            return self.parse_match_delete(statement);
        }
        if find_unquoted_keyword(statement, "MERGE").is_some() {
            return self.parse_match_merge(statement);
        }
        if find_unquoted_keyword(statement, "CREATE").is_some() {
            return self.parse_match_create(statement);
        }
        if find_unquoted_keyword(statement, "SET").is_some() {
            return self.parse_match_set(statement);
        }
        if find_unquoted_keyword(statement, "REMOVE").is_some() {
            return self.parse_match_remove(statement);
        }
        Err(cypher_syntax(
            "only ID-resolved MATCH ... DELETE, MATCH ... CREATE/MERGE edge, MATCH ... SET, and MATCH ... REMOVE forms are supported in writable Cypher".to_string(),
        ))
    }

    fn parse_match_delete(&mut self, statement: &str) -> Result<GraphMutationPlan> {
        let (pattern, target) = split_match_delete(statement)?;
        let targets = parse_match_delete_targets(target)?;
        let (pattern, where_predicates) = split_match_where(pattern, &self.parameters)?;

        if find_unquoted_sequence(pattern, "->").is_some() {
            let mut parsed = self.parse_edge_match_pattern(pattern)?;
            apply_edge_where_predicates(&mut parsed, where_predicates, "MATCH edge DELETE")?;
            let Some(edge_variable) = parsed.relationship.variable.as_ref() else {
                return Err(cypher_syntax(
                    "MATCH edge DELETE requires the relationship pattern to bind the DELETE target"
                        .to_string(),
                ));
            };
            let mut plan = GraphMutationPlan::default();
            for target in targets {
                if target == *edge_variable {
                    plan.operations.extend(
                        self.lower_match_edge_delete(parsed.clone(), edge_variable)?
                            .operations,
                    );
                } else if parsed.from.variable.as_deref() == Some(target.as_str()) {
                    plan.operations.extend(
                        self.lower_match_edge_endpoint_delete(&parsed.from, &target)?
                            .operations,
                    );
                } else if parsed.to.variable.as_deref() == Some(target.as_str()) {
                    plan.operations.extend(
                        self.lower_match_edge_endpoint_delete(&parsed.to, &target)?
                            .operations,
                    );
                } else {
                    return Err(cypher_syntax(format!(
                        "MATCH edge DELETE target '{target}' is not bound by the relationship pattern"
                    )));
                }
            }
            if plan.operations.is_empty() {
                return Err(cypher_syntax(
                    "MATCH edge DELETE requires at least one target",
                ));
            }
            return Ok(plan);
        }

        if targets.len() != 1 {
            return Err(cypher_syntax(
                "MATCH node DELETE supports one target for a single-node pattern",
            ));
        }
        let target = targets
            .into_iter()
            .next()
            .expect("checked one delete target");
        let (mut node, rest) = parse_cypher_node_pattern(pattern, &self.parameters)?;
        apply_node_where_predicates(&mut node, where_predicates, "MATCH node DELETE")?;
        if !rest.trim().is_empty() {
            return Err(cypher_syntax(format!(
                "unsupported writable Cypher MATCH DELETE pattern suffix: {}",
                rest.trim()
            )));
        }
        let Some(node_variable) = &node.variable else {
            return Err(cypher_syntax(
                "MATCH node DELETE requires the node pattern to bind the DELETE target".to_string(),
            ));
        };
        if node_variable != &target {
            return Err(cypher_syntax(format!(
                "MATCH node DELETE target '{target}' does not match node variable '{node_variable}'"
            )));
        }
        if let Some(id) = optional_string_prop(&node.props, "id")
            && node.predicates.is_empty()
        {
            self.bind_node_variable(&node, &NodeId::new(id.clone()))?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::DeleteNode(NodeId::new(id)),
            ]));
        }
        if node.variable.is_some()
            && node
                .variable
                .as_ref()
                .and_then(|variable| self.node_bindings.get(variable))
                .is_some()
            && node.predicates.is_empty()
        {
            let id = self.resolve_node_id(&node, "MATCH node DELETE")?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::DeleteNode(id),
            ]));
        }
        let cardinality = match_node_cardinality(&node);
        if self.bind_delete_return_rows {
            self.bind_row_node_variable(&node)?;
        }
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::DeleteMatchingNodes {
                label: node.label,
                props: node.props,
                predicates: node.predicates,
                cardinality,
            },
        ]))
    }

    fn lower_match_edge_delete(
        &mut self,
        parsed: ParsedCypherEdgeMatch,
        edge_variable: &str,
    ) -> Result<GraphMutationPlan> {
        let from_id = self.resolved_endpoint_id(&parsed.from)?;
        let to_id = self.resolved_endpoint_id(&parsed.to)?;
        let edge_id = optional_string_prop(&parsed.relationship.props, "id");
        if let (Some(from), Some(to), None) = (from_id, to_id, edge_id)
            && !has_relationship_predicates_beyond_id(&parsed.relationship.props)
            && parsed.from.predicates.is_empty()
            && parsed.to.predicates.is_empty()
            && parsed.relationship.predicates.is_empty()
        {
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::DeleteEdge {
                    from,
                    label: parsed.relationship.label,
                    to,
                },
            ]));
        }
        let relationship = self.relationship_match_from_pattern(parsed, "MATCH edge DELETE")?;
        if self.bind_delete_return_rows {
            self.bind_row_edge_match_variable(edge_variable, &relationship)?;
        }
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::DeleteMatchingEdges {
                relationship,
                cardinality: GraphMutationCardinality::BoundedMany,
            },
        ]))
    }

    fn lower_match_edge_endpoint_delete(
        &mut self,
        node: &ParsedCypherNode,
        target: &str,
    ) -> Result<GraphMutationPlan> {
        if !node.predicates.is_empty() {
            return Err(cypher_unsupported_cardinality(format!(
                "MATCH edge DELETE target '{target}' cannot delete endpoint rows selected only by predicates"
            )));
        }
        let id = self.resolved_endpoint_id(node)?.ok_or_else(|| {
            cypher_unsupported_cardinality(format!(
                "MATCH edge DELETE target '{target}' must resolve to a stable node id"
            ))
        })?;
        self.bind_node_variable(node, &id)?;
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::DeleteNode(id),
        ]))
    }

    fn parse_match_merge(&mut self, statement: &str) -> Result<GraphMutationPlan> {
        self.parse_match_edge_upsert(statement, "MERGE", GraphMutationPlanKind::Merge)
    }

    fn parse_match_create(&mut self, statement: &str) -> Result<GraphMutationPlan> {
        self.parse_match_edge_upsert(statement, "CREATE", GraphMutationPlanKind::Create)
    }

    fn parse_match_edge_upsert(
        &mut self,
        statement: &str,
        keyword: &str,
        kind: GraphMutationPlanKind,
    ) -> Result<GraphMutationPlan> {
        let (match_clause, edge_pattern) = split_match_edge_upsert(statement, keyword)?;
        let (match_clause, where_predicates) = split_match_where(match_clause, &self.parameters)?;
        let mut matched_nodes = BTreeMap::new();
        for pattern in split_top_level_patterns(match_clause)? {
            let (node, rest) = parse_cypher_node_pattern(pattern, &self.parameters)?;
            if !rest.trim().is_empty() {
                return Err(cypher_syntax(format!(
                    "unsupported writable Cypher MATCH pattern suffix: {}",
                    rest.trim()
                )));
            }
            if node.variable.is_none() {
                return Err(cypher_syntax(format!(
                    "MATCH {keyword} requires each matched node pattern to bind a variable"
                )));
            }
            let variable = node.variable.clone().expect("checked above");
            if matched_nodes.insert(variable.clone(), node).is_some() {
                return Err(cypher_unresolved_identity(format!(
                    "MATCH {keyword} cannot bind variable '{variable}' more than once"
                )));
            }
        }
        apply_match_where_predicates(
            &mut matched_nodes,
            where_predicates,
            &format!("MATCH {keyword}"),
        )?;

        let (path_variable, edge_pattern) = parse_row_path_binding(edge_pattern)?;
        if find_unquoted_sequence(edge_pattern, "->").is_none() {
            return Err(cypher_syntax(format!(
                "MATCH {keyword} currently supports one relationship pattern only",
            )));
        }
        let parsed = self.parse_edge_match_pattern(edge_pattern)?;
        let Some(from_variable) = parsed.from.variable.as_ref() else {
            return Err(cypher_syntax(format!(
                "MATCH {keyword} relationship endpoints must be bound variables"
            )));
        };
        let Some(to_variable) = parsed.to.variable.as_ref() else {
            return Err(cypher_syntax(format!(
                "MATCH {keyword} relationship endpoints must be bound variables"
            )));
        };
        if parsed.from.label.is_some()
            || !parsed.from.props.is_empty()
            || !parsed.from.predicates.is_empty()
            || parsed.to.label.is_some()
            || !parsed.to.props.is_empty()
            || !parsed.to.predicates.is_empty()
        {
            return Err(cypher_syntax(format!(
                "MATCH {keyword} relationship endpoints must reference bound variables only"
            )));
        }
        let Some(from_node) = matched_nodes.get(from_variable).cloned() else {
            return Err(cypher_unresolved_identity(format!(
                "MATCH {keyword} relationship source variable '{from_variable}' is not bound"
            )));
        };
        let Some(to_node) = matched_nodes.get(to_variable).cloned() else {
            return Err(cypher_unresolved_identity(format!(
                "MATCH {keyword} relationship destination variable '{to_variable}' is not bound"
            )));
        };
        if !parsed.relationship.predicates.is_empty() {
            return Err(cypher_syntax(format!(
                "MATCH {keyword} relationship creation does not accept relationship WHERE predicates"
            )));
        }

        let from_id = self.resolved_endpoint_id(&from_node)?;
        let to_id = self.resolved_endpoint_id(&to_node)?;
        if let (Some(from), Some(to)) = (from_id, to_id)
            && from_node.predicates.is_empty()
            && to_node.predicates.is_empty()
        {
            let mut edge = Edge::new(
                parsed.relationship.label.clone(),
                from,
                to,
                parsed.relationship.props.clone(),
            );
            if let Some(id) = edge
                .props
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                edge = edge.with_id(id);
            }
            self.bind_edge_variable(
                &parsed.relationship,
                CypherBoundEdgeIdentity {
                    from: edge.from.clone(),
                    label: edge.label.clone(),
                    to: edge.to.clone(),
                    id: edge.id.clone(),
                },
            )?;
            self.bind_node_variable(&from_node, &edge.from)?;
            self.bind_node_variable(&to_node, &edge.to)?;
            if let Some(path_variable) = path_variable {
                let Some(edge_variable) = parsed.relationship.variable.as_ref() else {
                    return Err(cypher_syntax(format!(
                        "MATCH {keyword} path variables require the relationship pattern to bind a variable"
                    )));
                };
                self.bind_row_path_variable(
                    &path_variable,
                    CypherRowProducedPathBinding {
                        from_variable: from_variable.clone(),
                        edge_variable: edge_variable.clone(),
                        to_variable: to_variable.clone(),
                    },
                )?;
            }
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::UpsertEdge { kind, edge },
            ]));
        }
        validate_optional_edge_id_property(&parsed.relationship.props)?;
        let from = self.node_match_from_pattern(from_node, "MATCH CREATE source")?;
        let to = self.node_match_from_pattern(to_node, "MATCH CREATE destination")?;
        let edge_id_policy = self.row_edge_id_policy(kind);
        if let Some(variable) = &parsed.relationship.variable {
            self.bind_row_edge_variable(
                variable,
                CypherRowProducedEdgeBinding {
                    kind,
                    from_variable: from_variable.clone(),
                    from: from.clone(),
                    to_variable: to_variable.clone(),
                    to: to.clone(),
                    label: parsed.relationship.label.clone(),
                    props: parsed.relationship.props.clone(),
                    edge_id_policy,
                },
            )?;
        }
        if let Some(path_variable) = path_variable {
            let Some(edge_variable) = parsed.relationship.variable.as_ref() else {
                return Err(cypher_syntax(format!(
                    "MATCH {keyword} path variables require the relationship pattern to bind a variable"
                )));
            };
            self.bind_row_path_variable(
                &path_variable,
                CypherRowProducedPathBinding {
                    from_variable: from_variable.clone(),
                    edge_variable: edge_variable.clone(),
                    to_variable: to_variable.clone(),
                },
            )?;
        }
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
                kind,
                from,
                to,
                label: parsed.relationship.label,
                props: parsed.relationship.props,
                edge_id_policy,
                cardinality: GraphMutationCardinality::BoundedMany,
            },
        ]))
    }

    fn row_edge_id_policy(&self, kind: GraphMutationPlanKind) -> GraphRowEdgeIdPolicy {
        match (kind, self.relationship_id_policy) {
            (GraphMutationPlanKind::Create, CypherRelationshipIdPolicy::GenerateForRowCreate) => {
                GraphRowEdgeIdPolicy::GenerateForCreate
            }
            (
                GraphMutationPlanKind::Create | GraphMutationPlanKind::Merge,
                CypherRelationshipIdPolicy::GenerateForRowCreateAndMerge,
            ) => GraphRowEdgeIdPolicy::GenerateForCreateAndMerge,
            _ => GraphRowEdgeIdPolicy::ExplicitOnly,
        }
    }

    fn parse_match_set(&mut self, statement: &str) -> Result<GraphMutationPlan> {
        let (pattern, assignments) = split_match_set(statement)?;
        let assignments =
            parse_patch_assignments(assignments, &self.parameters, self.null_assignment)?;
        let mut plan = GraphMutationPlan::default();
        for assignment in assignments {
            plan.operations.extend(
                self.lower_match_set_assignment(pattern, assignment)?
                    .operations,
            );
        }
        Ok(plan)
    }

    fn lower_match_set_assignment(
        &mut self,
        pattern: &str,
        assignment: PatchAssignment,
    ) -> Result<GraphMutationPlan> {
        let (pattern, where_predicates) = split_match_where(pattern, &self.parameters)?;

        if find_unquoted_sequence(pattern, "->").is_some() {
            let mut parsed = self.parse_edge_match_pattern(pattern)?;
            apply_edge_where_predicates(&mut parsed, where_predicates, "MATCH edge SET")?;
            let Some(edge_variable) = parsed.relationship.variable.clone() else {
                return Err(cypher_syntax(
                    "MATCH edge SET requires the relationship pattern to bind the patch target",
                ));
            };
            if edge_variable != assignment.target {
                return Err(cypher_syntax(format!(
                    "MATCH edge SET target '{}' does not match relationship variable '{edge_variable}'",
                    assignment.target
                )));
            }
            let kind = assignment.kind;
            let from_id = self.resolved_endpoint_id(&parsed.from)?;
            let to_id = self.resolved_endpoint_id(&parsed.to)?;
            if let PatchAssignmentKind::NumericExpression {
                key,
                source_target,
                source_key,
                op,
                operand,
            } = kind
            {
                if source_target != assignment.target {
                    return Err(cypher_unsupported_cardinality(
                        "MATCH edge SET numeric expressions cannot reference another variable",
                    ));
                }
                let (relationship, cardinality) = if let (Some(from), Some(to)) =
                    (from_id.clone(), to_id.clone())
                    && !has_relationship_predicates_beyond_id(&parsed.relationship.props)
                    && parsed.from.predicates.is_empty()
                    && parsed.to.predicates.is_empty()
                    && parsed.relationship.predicates.is_empty()
                {
                    let id =
                        optional_string_prop(&parsed.relationship.props, "id").map(EdgeId::new);
                    self.bind_edge_variable(
                        &parsed.relationship,
                        CypherBoundEdgeIdentity {
                            from: from.clone(),
                            label: parsed.relationship.label.clone(),
                            to: to.clone(),
                            id: id.clone(),
                        },
                    )?;
                    (
                        GraphRelationshipMatch {
                            from: GraphNodeMatch {
                                label: None,
                                props: Props::from([(
                                    "id".to_string(),
                                    Value::from(from.as_str()),
                                )]),
                                predicates: Vec::new(),
                            },
                            label: parsed.relationship.label,
                            to: GraphNodeMatch {
                                label: None,
                                props: Props::from([("id".to_string(), Value::from(to.as_str()))]),
                                predicates: Vec::new(),
                            },
                            id,
                            props: Props::new(),
                            predicates: Vec::new(),
                        },
                        GraphMutationCardinality::SingleIdentity,
                    )
                } else {
                    let relationship =
                        self.relationship_match_from_pattern(parsed, "MATCH edge SET")?;
                    self.bind_row_edge_match_variable(&edge_variable, &relationship)?;
                    (relationship, GraphMutationCardinality::BoundedMany)
                };
                return Ok(GraphMutationPlan::new(vec![
                    GraphMutationPlanOp::UpdateMatchingEdgeProperty {
                        relationship,
                        target_key: key,
                        source_key,
                        op,
                        operand,
                        cardinality,
                    },
                ]));
            }
            if let PatchAssignmentKind::RemoveProperty { key } = kind {
                if let (Some(from), Some(to)) = (from_id, to_id)
                    && !has_relationship_predicates_beyond_id(&parsed.relationship.props)
                    && parsed.from.predicates.is_empty()
                    && parsed.to.predicates.is_empty()
                    && parsed.relationship.predicates.is_empty()
                {
                    let id =
                        optional_string_prop(&parsed.relationship.props, "id").map(EdgeId::new);
                    self.bind_edge_variable(
                        &parsed.relationship,
                        CypherBoundEdgeIdentity {
                            from: from.clone(),
                            label: parsed.relationship.label.clone(),
                            to: to.clone(),
                            id: id.clone(),
                        },
                    )?;
                    return Ok(GraphMutationPlan::new(vec![
                        GraphMutationPlanOp::RemoveEdgeProps {
                            from,
                            label: parsed.relationship.label,
                            to,
                            id,
                            keys: vec![key],
                        },
                    ]));
                }
                let relationship =
                    self.relationship_match_from_pattern(parsed, "MATCH edge SET")?;
                self.bind_row_edge_match_variable(&edge_variable, &relationship)?;
                return Ok(GraphMutationPlan::new(vec![
                    GraphMutationPlanOp::RemoveMatchingEdgeProps {
                        relationship,
                        keys: vec![key],
                        cardinality: GraphMutationCardinality::BoundedMany,
                    },
                ]));
            };
            let PatchAssignmentKind::Props(props) = kind else {
                unreachable!("numeric expression handled above");
            };
            if let (Some(from), Some(to)) = (from_id, to_id)
                && !has_relationship_predicates_beyond_id(&parsed.relationship.props)
                && parsed.from.predicates.is_empty()
                && parsed.to.predicates.is_empty()
                && parsed.relationship.predicates.is_empty()
            {
                let id = optional_string_prop(&parsed.relationship.props, "id").map(EdgeId::new);
                self.bind_edge_variable(
                    &parsed.relationship,
                    CypherBoundEdgeIdentity {
                        from: from.clone(),
                        label: parsed.relationship.label.clone(),
                        to: to.clone(),
                        id: id.clone(),
                    },
                )?;
                return Ok(GraphMutationPlan::new(vec![
                    GraphMutationPlanOp::PatchEdge {
                        from,
                        label: parsed.relationship.label,
                        to,
                        id,
                        props,
                    },
                ]));
            }
            let relationship = self.relationship_match_from_pattern(parsed, "MATCH edge SET")?;
            self.bind_row_edge_match_variable(&edge_variable, &relationship)?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::PatchMatchingEdges {
                    relationship,
                    patch: props,
                    cardinality: GraphMutationCardinality::BoundedMany,
                },
            ]));
        }

        let (mut node, rest) = parse_cypher_node_pattern(pattern, &self.parameters)?;
        apply_node_where_predicates(&mut node, where_predicates, "MATCH node SET")?;
        if !rest.trim().is_empty() {
            return Err(cypher_syntax(format!(
                "unsupported writable Cypher MATCH SET pattern suffix: {}",
                rest.trim()
            )));
        }
        let Some(node_variable) = &node.variable else {
            return Err(cypher_syntax(
                "MATCH SET requires the node pattern to bind the patch target".to_string(),
            ));
        };
        if node_variable != &assignment.target {
            return Err(cypher_syntax(format!(
                "MATCH SET target '{}' does not match node variable '{node_variable}'",
                assignment.target
            )));
        }
        let numeric_expression = match assignment.kind {
            PatchAssignmentKind::NumericExpression {
                key,
                source_target,
                source_key,
                op,
                operand,
            } => {
                if source_target != assignment.target {
                    return Err(cypher_unsupported_cardinality(
                        "MATCH SET numeric expressions cannot reference another variable",
                    ));
                }
                Some((key, source_key, op, operand))
            }
            PatchAssignmentKind::RemoveProperty { key } => {
                if (optional_string_prop(&node.props, "id").is_none()
                    && node
                        .variable
                        .as_ref()
                        .is_none_or(|variable| !self.node_bindings.contains_key(variable)))
                    || !node.predicates.is_empty()
                {
                    let cardinality = match_node_cardinality(&node);
                    self.bind_row_node_variable(&node)?;
                    return Ok(GraphMutationPlan::new(vec![
                        GraphMutationPlanOp::RemoveMatchingNodeProps {
                            label: node.label,
                            props: node.props,
                            predicates: node.predicates,
                            keys: vec![key],
                            cardinality,
                        },
                    ]));
                }
                let id = self.resolve_node_id(&node, "MATCH node SET")?;
                return Ok(GraphMutationPlan::new(vec![
                    GraphMutationPlanOp::RemoveNodeProps {
                        id,
                        keys: vec![key],
                    },
                ]));
            }
            PatchAssignmentKind::Props(props) => {
                if (optional_string_prop(&node.props, "id").is_none()
                    && node
                        .variable
                        .as_ref()
                        .is_none_or(|variable| !self.node_bindings.contains_key(variable)))
                    || !node.predicates.is_empty()
                {
                    let cardinality = match_node_cardinality(&node);
                    self.bind_row_node_variable(&node)?;
                    return Ok(GraphMutationPlan::new(vec![
                        GraphMutationPlanOp::PatchMatchingNodes {
                            label: node.label,
                            props: node.props,
                            predicates: node.predicates,
                            patch: props,
                            cardinality,
                        },
                    ]));
                }
                let id = self.resolve_node_id(&node, "MATCH node SET")?;
                return Ok(GraphMutationPlan::new(vec![
                    GraphMutationPlanOp::PatchNode { id, props },
                ]));
            }
        };
        let (target_key, source_key, op, operand) = numeric_expression.expect("expression checked");
        if (optional_string_prop(&node.props, "id").is_none()
            && node
                .variable
                .as_ref()
                .is_none_or(|variable| !self.node_bindings.contains_key(variable)))
            || !node.predicates.is_empty()
        {
            let cardinality = match_node_cardinality(&node);
            self.bind_row_node_variable(&node)?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::UpdateMatchingNodeProperty {
                    label: node.label,
                    props: node.props,
                    predicates: node.predicates,
                    target_key,
                    source_key,
                    op,
                    operand,
                    cardinality,
                },
            ]));
        }
        let id = self.resolve_node_id(&node, "MATCH node SET")?;
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::UpdateMatchingNodeProperty {
                label: None,
                props: Props::from([("id".to_string(), Value::from(id.as_str()))]),
                predicates: Vec::new(),
                target_key,
                source_key,
                op,
                operand,
                cardinality: GraphMutationCardinality::SingleIdentity,
            },
        ]))
    }

    fn parse_match_remove(&mut self, statement: &str) -> Result<GraphMutationPlan> {
        let (pattern, target) = split_match_remove(statement)?;
        let (pattern, where_predicates) = split_match_where(pattern, &self.parameters)?;
        let (target, key) = parse_property_ref(target, "MATCH REMOVE target")?;

        if find_unquoted_sequence(pattern, "->").is_some() {
            let mut parsed = self.parse_edge_match_pattern(pattern)?;
            apply_edge_where_predicates(&mut parsed, where_predicates, "MATCH edge REMOVE")?;
            let Some(edge_variable) = parsed.relationship.variable.clone() else {
                return Err(cypher_syntax(
                    "MATCH edge REMOVE requires the relationship pattern to bind the remove target",
                ));
            };
            if edge_variable != target {
                return Err(cypher_syntax(format!(
                    "MATCH edge REMOVE target '{target}' does not match relationship variable '{edge_variable}'"
                )));
            }
            let from_id = self.resolved_endpoint_id(&parsed.from)?;
            let to_id = self.resolved_endpoint_id(&parsed.to)?;
            if let (Some(from), Some(to)) = (from_id, to_id)
                && !has_relationship_predicates_beyond_id(&parsed.relationship.props)
                && parsed.from.predicates.is_empty()
                && parsed.to.predicates.is_empty()
                && parsed.relationship.predicates.is_empty()
            {
                let id = optional_string_prop(&parsed.relationship.props, "id").map(EdgeId::new);
                self.bind_edge_variable(
                    &parsed.relationship,
                    CypherBoundEdgeIdentity {
                        from: from.clone(),
                        label: parsed.relationship.label.clone(),
                        to: to.clone(),
                        id: id.clone(),
                    },
                )?;
                return Ok(GraphMutationPlan::new(vec![
                    GraphMutationPlanOp::RemoveEdgeProps {
                        from,
                        label: parsed.relationship.label,
                        to,
                        id,
                        keys: vec![key],
                    },
                ]));
            }
            let relationship = self.relationship_match_from_pattern(parsed, "MATCH edge REMOVE")?;
            self.bind_row_edge_match_variable(&edge_variable, &relationship)?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::RemoveMatchingEdgeProps {
                    relationship,
                    keys: vec![key],
                    cardinality: GraphMutationCardinality::BoundedMany,
                },
            ]));
        }

        let (mut node, rest) = parse_cypher_node_pattern(pattern, &self.parameters)?;
        apply_node_where_predicates(&mut node, where_predicates, "MATCH node REMOVE")?;
        if !rest.trim().is_empty() {
            return Err(cypher_syntax(format!(
                "unsupported writable Cypher MATCH REMOVE pattern suffix: {}",
                rest.trim()
            )));
        }
        let Some(node_variable) = &node.variable else {
            return Err(cypher_syntax(
                "MATCH REMOVE requires the node pattern to bind the remove target",
            ));
        };
        if node_variable != &target {
            return Err(cypher_syntax(format!(
                "MATCH REMOVE target '{target}' does not match node variable '{node_variable}'"
            )));
        }
        if (optional_string_prop(&node.props, "id").is_none()
            && node
                .variable
                .as_ref()
                .is_none_or(|variable| !self.node_bindings.contains_key(variable)))
            || !node.predicates.is_empty()
        {
            let cardinality = match_node_cardinality(&node);
            self.bind_row_node_variable(&node)?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::RemoveMatchingNodeProps {
                    label: node.label,
                    props: node.props,
                    predicates: node.predicates,
                    keys: vec![key],
                    cardinality,
                },
            ]));
        }
        let id = self.resolve_node_id(&node, "MATCH node REMOVE")?;
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::RemoveNodeProps {
                id,
                keys: vec![key],
            },
        ]))
    }

    fn parse_edge_pattern(&mut self, pattern: &str) -> Result<ParsedCypherEdge> {
        let (from, rest) = parse_cypher_node_pattern(pattern, &self.parameters)?;
        let rest = rest.trim_start();
        let rest = rest
            .strip_prefix("-[")
            .ok_or_else(|| cypher_syntax("edge mutation requires a directed -[...]-> pattern"))?;
        let rel_end = find_matching(rest, '[', ']')?;
        let rel = &rest[..rel_end];
        let rest = rest[rel_end + 1..].trim_start();
        let rest = rest
            .strip_prefix("->")
            .ok_or_else(|| cypher_syntax("edge mutation requires outgoing '->' direction"))?;
        let (to, rest) = parse_cypher_node_pattern(rest, &self.parameters)?;
        if !rest.trim().is_empty() {
            return Err(cypher_syntax(format!(
                "unsupported writable Cypher edge pattern suffix: {}",
                rest.trim()
            )));
        }

        let from_id = self.resolve_node_id(&from, "edge mutation source node")?;
        let to_id = self.resolve_node_id(&to, "edge mutation destination node")?;
        let relationship = parse_cypher_relationship(rel, &self.parameters)?;
        let mut edge = Edge::new(
            relationship.label.clone(),
            from_id.clone(),
            to_id.clone(),
            relationship.props.clone(),
        );
        if let Some(id) = edge
            .props
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            edge = edge.with_id(id);
        }
        self.bind_edge_variable(
            &relationship,
            CypherBoundEdgeIdentity {
                from: edge.from.clone(),
                label: edge.label.clone(),
                to: edge.to.clone(),
                id: edge.id.clone(),
            },
        )?;
        Ok(ParsedCypherEdge {
            from_id,
            to_id,
            edge,
        })
    }

    fn parse_edge_match_pattern(&self, pattern: &str) -> Result<ParsedCypherEdgeMatch> {
        let (from, rest) = parse_cypher_node_pattern(pattern, &self.parameters)?;
        let rest = rest.trim_start();
        let rest = rest
            .strip_prefix("-[")
            .ok_or_else(|| cypher_syntax("edge mutation requires a directed -[...]-> pattern"))?;
        let rel_end = find_matching(rest, '[', ']')?;
        let rel = &rest[..rel_end];
        let rest = rest[rel_end + 1..].trim_start();
        let rest = rest
            .strip_prefix("->")
            .ok_or_else(|| cypher_syntax("edge mutation requires outgoing '->' direction"))?;
        let (to, rest) = parse_cypher_node_pattern(rest, &self.parameters)?;
        if !rest.trim().is_empty() {
            return Err(cypher_syntax(format!(
                "unsupported writable Cypher edge pattern suffix: {}",
                rest.trim()
            )));
        }
        let relationship = parse_cypher_relationship(rel, &self.parameters)?;
        Ok(ParsedCypherEdgeMatch {
            from,
            relationship,
            to,
        })
    }

    fn relationship_match_from_pattern(
        &mut self,
        parsed: ParsedCypherEdgeMatch,
        context: &str,
    ) -> Result<GraphRelationshipMatch> {
        let id = optional_string_prop(&parsed.relationship.props, "id").map(EdgeId::new);
        let mut props = parsed.relationship.props;
        props.remove("id");
        let from = self.node_match_from_pattern(parsed.from, context)?;
        let to = self.node_match_from_pattern(parsed.to, context)?;
        Ok(GraphRelationshipMatch {
            from,
            label: parsed.relationship.label,
            to,
            id,
            props,
            predicates: parsed.relationship.predicates,
        })
    }

    fn node_match_from_pattern(
        &mut self,
        node: ParsedCypherNode,
        context: &str,
    ) -> Result<GraphNodeMatch> {
        if node.props.contains_key("id") && optional_string_prop(&node.props, "id").is_none() {
            return Err(cypher_unresolved_identity(format!(
                "{context} endpoint id must be a string"
            )));
        }
        let mut props = node.props.clone();
        if let Some(id) = self.resolved_endpoint_id(&node)?
            && !props.contains_key("id")
        {
            props.insert("id".to_string(), Value::from(id.as_str()));
        }
        Ok(GraphNodeMatch {
            label: node.label,
            props,
            predicates: node.predicates,
        })
    }

    fn resolved_endpoint_id(&mut self, node: &ParsedCypherNode) -> Result<Option<NodeId>> {
        if node.props.contains_key("id") && optional_string_prop(&node.props, "id").is_none() {
            return Err(cypher_unresolved_identity(
                "edge mutation endpoint id must be a string",
            ));
        }
        if let Some(id) = optional_string_prop(&node.props, "id") {
            let id = NodeId::new(id);
            self.bind_node_variable(node, &id)?;
            return Ok(Some(id));
        }
        if let Some(variable) = &node.variable
            && let Some(id) = self.node_bindings.get(variable)
        {
            return Ok(Some(id.clone()));
        }
        Ok(None)
    }

    fn resolve_node_id(&mut self, node: &ParsedCypherNode, context: &str) -> Result<NodeId> {
        if let Some(id) = optional_string_prop(&node.props, "id") {
            let id = NodeId::new(id);
            self.bind_node_variable(node, &id)?;
            return Ok(id);
        }
        if let Some(variable) = &node.variable {
            if let Some(id) = self.node_bindings.get(variable) {
                return Ok(id.clone());
            }
            return Err(cypher_unresolved_identity(format!(
                "{context} variable '{variable}' is not bound to a node id"
            )));
        }
        Err(cypher_unresolved_identity(format!(
            "{context} requires explicit string property 'id'"
        )))
    }

    fn bind_node_variable(&mut self, node: &ParsedCypherNode, id: &NodeId) -> Result<()> {
        let Some(variable) = &node.variable else {
            return Ok(());
        };
        if let Some(existing) = self.node_bindings.get(variable) {
            if existing != id {
                return Err(cypher_unresolved_identity(format!(
                    "Cypher variable '{variable}' is already bound to node id '{}'",
                    existing.as_str()
                )));
            }
            return Ok(());
        }
        self.node_bindings.insert(variable.clone(), id.clone());
        Ok(())
    }

    fn bind_edge_variable(
        &mut self,
        relationship: &ParsedCypherRelationship,
        identity: CypherBoundEdgeIdentity,
    ) -> Result<()> {
        let Some(variable) = &relationship.variable else {
            return Ok(());
        };
        if let Some(existing) = self.edge_bindings.get(variable) {
            if existing != &identity {
                return Err(cypher_unresolved_identity(format!(
                    "Cypher relationship variable '{variable}' is already bound to a different edge identity"
                )));
            }
            return Ok(());
        }
        if self.node_bindings.contains_key(variable) {
            return Err(cypher_unresolved_identity(format!(
                "Cypher variable '{variable}' is already bound to a node id"
            )));
        }
        self.edge_bindings.insert(variable.clone(), identity);
        Ok(())
    }

    fn bind_row_node_variable(&mut self, node: &ParsedCypherNode) -> Result<()> {
        let Some(variable) = &node.variable else {
            return Ok(());
        };
        if self.edge_bindings.contains_key(variable)
            || self.row_edge_bindings.contains_key(variable)
            || self.row_path_bindings.contains_key(variable)
        {
            return Err(cypher_unresolved_identity(format!(
                "Cypher variable '{variable}' is already bound to a relationship"
            )));
        }
        let binding = GraphNodeMatch {
            label: node.label.clone(),
            props: node.props.clone(),
            predicates: node.predicates.clone(),
        };
        if let Some(existing) = self.row_node_bindings.get(variable) {
            if existing != &binding {
                return Err(cypher_unresolved_identity(format!(
                    "Cypher variable '{variable}' is already bound to a different node match"
                )));
            }
            return Ok(());
        }
        if self.node_bindings.contains_key(variable) {
            return Err(cypher_unresolved_identity(format!(
                "Cypher variable '{variable}' is already bound to a node id"
            )));
        }
        self.row_node_bindings.insert(variable.clone(), binding);
        Ok(())
    }

    fn bind_row_edge_match_variable(
        &mut self,
        variable: &str,
        binding: &GraphRelationshipMatch,
    ) -> Result<()> {
        if self.node_bindings.contains_key(variable)
            || self.row_node_bindings.contains_key(variable)
            || self.row_path_bindings.contains_key(variable)
        {
            return Err(cypher_unresolved_identity(format!(
                "Cypher variable '{variable}' is already bound to a node"
            )));
        }
        if self.edge_bindings.contains_key(variable)
            || self.row_edge_bindings.contains_key(variable)
        {
            return Err(cypher_unresolved_identity(format!(
                "Cypher relationship variable '{variable}' is already bound"
            )));
        }
        if let Some(existing) = self.row_edge_match_bindings.get(variable) {
            if existing != binding {
                return Err(cypher_unresolved_identity(format!(
                    "Cypher relationship variable '{variable}' is already bound to a different relationship match"
                )));
            }
            return Ok(());
        }
        self.row_edge_match_bindings
            .insert(variable.to_string(), binding.clone());
        Ok(())
    }

    fn bind_row_edge_variable(
        &mut self,
        variable: &str,
        binding: CypherRowProducedEdgeBinding,
    ) -> Result<()> {
        if self.node_bindings.contains_key(variable) {
            return Err(cypher_unresolved_identity(format!(
                "Cypher variable '{variable}' is already bound to a node id"
            )));
        }
        if self.row_path_bindings.contains_key(variable) {
            return Err(cypher_unresolved_identity(format!(
                "Cypher path variable '{variable}' is already bound"
            )));
        }
        if self.edge_bindings.contains_key(variable)
            || self.row_edge_bindings.contains_key(variable)
        {
            return Err(cypher_unresolved_identity(format!(
                "Cypher relationship variable '{variable}' is already bound"
            )));
        }
        self.row_edge_bindings.insert(variable.to_string(), binding);
        Ok(())
    }

    fn bind_row_path_variable(
        &mut self,
        variable: &str,
        binding: CypherRowProducedPathBinding,
    ) -> Result<()> {
        if self.node_bindings.contains_key(variable)
            || self.row_node_bindings.contains_key(variable)
        {
            return Err(cypher_unresolved_identity(format!(
                "Cypher path variable '{variable}' is already bound to a node"
            )));
        }
        if self.edge_bindings.contains_key(variable)
            || self.row_edge_match_bindings.contains_key(variable)
            || self.row_edge_bindings.contains_key(variable)
        {
            return Err(cypher_unresolved_identity(format!(
                "Cypher path variable '{variable}' is already bound to a relationship"
            )));
        }
        if let Some(existing) = self.row_path_bindings.get(variable) {
            if existing != &binding {
                return Err(cypher_unresolved_identity(format!(
                    "Cypher path variable '{variable}' is already bound to a different path"
                )));
            }
            return Ok(());
        }
        self.row_path_bindings.insert(variable.to_string(), binding);
        Ok(())
    }
}

fn cypher_syntax(message: impl Into<String>) -> GrustError {
    GrustError::CypherSyntax(message.into())
}

fn cypher_unresolved_identity(message: impl Into<String>) -> GrustError {
    GrustError::CypherUnresolvedIdentity(message.into())
}

fn cypher_unsupported_cardinality(message: impl Into<String>) -> GrustError {
    GrustError::CypherUnsupportedCardinality(message.into())
}

mod cypher_parser {
    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(super) enum CypherStatement<'a> {
        Match(&'a str),
        Create(&'a str),
        Merge(&'a str),
        Delete(&'a str),
    }

    pub(super) fn classify_statement(cypher: &str) -> Result<CypherStatement<'_>> {
        if let Some(rest) = strip_leading_keyword(cypher, "MATCH") {
            return Ok(CypherStatement::Match(rest));
        }
        if find_unquoted_keyword(cypher, "SET").is_some() {
            return Err(cypher_syntax("writable Cypher SET is not supported in v1"));
        }
        if find_unquoted_keyword(cypher, "REMOVE").is_some() {
            return Err(cypher_syntax(
                "writable Cypher REMOVE is not supported in v1",
            ));
        }
        if let Some(rest) = strip_leading_keyword(cypher, "CREATE") {
            return Ok(CypherStatement::Create(rest));
        }
        if let Some(rest) = strip_leading_keyword(cypher, "MERGE") {
            return Ok(CypherStatement::Merge(rest));
        }
        if let Some(rest) = strip_leading_keyword(cypher, "DELETE") {
            return Ok(CypherStatement::Delete(rest));
        }
        Err(cypher_syntax(format!(
            "unsupported writable Cypher statement; expected CREATE, MERGE, or DELETE: {cypher}"
        )))
    }
}

fn cypher_execution_error(error: GrustError) -> GrustError {
    match error {
        GrustError::CypherSyntax(_)
        | GrustError::CypherUnresolvedIdentity(_)
        | GrustError::CypherUnsupportedCardinality(_)
        | GrustError::CypherExecution(_) => error,
        other => GrustError::CypherExecution(other.to_string()),
    }
}

fn parse_cypher_ddl_statement(statement: &str) -> Result<CypherDdlStatement> {
    if let Some(rest) = strip_leading_keyword(statement, "CREATE") {
        let rest = rest.trim_start();
        if let Some(rest) = strip_leading_keyword(rest, "CONSTRAINT") {
            return parse_create_constraint(rest.trim());
        }
        return Err(cypher_syntax(
            "only CREATE CONSTRAINT is supported as Cypher CREATE DDL",
        ));
    }
    if let Some(rest) = strip_leading_keyword(statement, "DROP") {
        let rest = rest.trim_start();
        if let Some(rest) = strip_leading_keyword(rest, "CONSTRAINT") {
            return parse_drop_constraint(rest.trim());
        }
        return Err(cypher_syntax(
            "only DROP CONSTRAINT is supported as Cypher DROP DDL",
        ));
    }
    Err(cypher_syntax(format!(
        "unsupported Cypher DDL statement; expected CREATE CONSTRAINT or DROP CONSTRAINT: {statement}"
    )))
}

fn parse_create_constraint(rest: &str) -> Result<CypherDdlStatement> {
    // Split the header (`[name] [IF NOT EXISTS]`) from the body, which starts
    // at `FOR` (or the legacy `ON`).
    let (for_index, body) = find_unquoted_keyword(rest, "FOR")
        .map(|index| (index, &rest[index + "FOR".len()..]))
        .or_else(|| {
            find_unquoted_keyword(rest, "ON").map(|index| (index, &rest[index + "ON".len()..]))
        })
        .ok_or_else(|| cypher_syntax("CREATE CONSTRAINT requires a FOR (or ON) pattern clause"))?;
    let header = rest[..for_index].trim();

    let (name, if_not_exists) = if let Some(if_index) = find_unquoted_keyword(header, "IF") {
        let tail = header[if_index + "IF".len()..].trim();
        if !tail.eq_ignore_ascii_case("NOT EXISTS")
            && tail.split_whitespace().collect::<Vec<_>>() != ["NOT", "EXISTS"]
        {
            return Err(cypher_syntax(
                "CREATE CONSTRAINT only supports the IF NOT EXISTS modifier",
            ));
        }
        (constraint_name(header[..if_index].trim())?, true)
    } else {
        (constraint_name(header)?, false)
    };

    // Body: `<pattern> REQUIRE <predicate>` (or legacy `ASSERT`).
    let (require_index, require_len) = find_unquoted_keyword(body, "REQUIRE")
        .map(|index| (index, "REQUIRE".len()))
        .or_else(|| find_unquoted_keyword(body, "ASSERT").map(|index| (index, "ASSERT".len())))
        .ok_or_else(|| {
            cypher_syntax("CREATE CONSTRAINT requires a REQUIRE (or ASSERT) predicate clause")
        })?;
    let pattern = body[..require_index].trim();
    let predicate = body[require_index + require_len..].trim();

    let (is_edge, pattern_variable, label) = parse_constraint_pattern(pattern)?;
    let (unique, key) = parse_constraint_predicate(predicate, &pattern_variable)?;

    let constraint = match (is_edge, unique) {
        (false, true) => GraphConstraint::NodePropertyUnique { label, key },
        (false, false) => GraphConstraint::NodePropertyRequired { label, key },
        (true, true) => GraphConstraint::EdgePropertyUnique { label, key },
        (true, false) => GraphConstraint::EdgePropertyRequired { label, key },
    };
    Ok(CypherDdlStatement::CreateConstraint {
        name,
        if_not_exists,
        constraint,
    })
}

fn parse_drop_constraint(rest: &str) -> Result<CypherDdlStatement> {
    let (name, if_exists) = if let Some(if_index) = find_unquoted_keyword(rest, "IF") {
        let tail = rest[if_index + "IF".len()..].trim();
        if !tail.eq_ignore_ascii_case("EXISTS") {
            return Err(cypher_syntax(
                "DROP CONSTRAINT only supports the IF EXISTS modifier",
            ));
        }
        (rest[..if_index].trim(), true)
    } else {
        (rest.trim(), false)
    };
    if !is_cypher_identifier(name) {
        return Err(cypher_syntax("DROP CONSTRAINT requires a constraint name"));
    }
    Ok(CypherDdlStatement::DropConstraint {
        name: name.to_string(),
        if_exists,
    })
}

/// Parses the optional constraint name in a `CREATE CONSTRAINT` header.
fn constraint_name(header: &str) -> Result<Option<String>> {
    let header = header.trim();
    if header.is_empty() {
        return Ok(None);
    }
    if is_cypher_identifier(header) {
        Ok(Some(header.to_string()))
    } else {
        Err(cypher_syntax(format!(
            "unsupported CREATE CONSTRAINT name: {header}"
        )))
    }
}

/// Parses a constraint `FOR` pattern, returning whether it is a relationship
/// pattern, the bound variable, and the single label/type.
fn parse_constraint_pattern(pattern: &str) -> Result<(bool, String, Label)> {
    let pattern = pattern.trim();
    if let Some(open) = pattern.find('[') {
        let close = pattern[open + 1..]
            .find(']')
            .map(|offset| offset + open + 1)
            .ok_or_else(|| cypher_syntax("constraint relationship pattern is missing ']'"))?;
        let (variable, label) = parse_constraint_var_label(&pattern[open + 1..close])?;
        return Ok((true, variable, label));
    }
    let open = pattern.find('(').ok_or_else(|| {
        cypher_syntax("constraint pattern must be a node or relationship pattern")
    })?;
    let close = pattern[open + 1..]
        .find(')')
        .map(|offset| offset + open + 1)
        .ok_or_else(|| cypher_syntax("constraint node pattern is missing ')'"))?;
    let (variable, label) = parse_constraint_var_label(&pattern[open + 1..close])?;
    Ok((false, variable, label))
}

/// Parses the `variable:Label` body inside a constraint pattern.
fn parse_constraint_var_label(body: &str) -> Result<(String, Label)> {
    let (variable, label) = body
        .split_once(':')
        .ok_or_else(|| cypher_syntax("constraint pattern requires variable:Label"))?;
    let variable = parse_required_cypher_variable(variable.trim(), "constraint pattern variable")?;
    let label = label.trim();
    if label.is_empty() {
        return Err(cypher_syntax("constraint pattern requires a label or type"));
    }
    Ok((variable, Label::new(label.to_string())))
}

/// Parses a `variable.key IS [NOT NULL|UNIQUE]` constraint predicate, returning
/// `(is_unique, key)`. The predicate variable must match the pattern variable.
fn parse_constraint_predicate(predicate: &str, pattern_variable: &str) -> Result<(bool, String)> {
    let is_index = find_unquoted_keyword(predicate, "IS").ok_or_else(|| {
        cypher_syntax("constraint predicate requires 'IS UNIQUE' or 'IS NOT NULL'")
    })?;
    let (variable, key) = parse_property_ref(predicate[..is_index].trim(), "constraint predicate")?;
    if variable != pattern_variable {
        return Err(cypher_syntax(format!(
            "constraint predicate variable '{variable}' does not match pattern variable '{pattern_variable}'"
        )));
    }
    let kind = predicate[is_index + "IS".len()..].trim();
    if kind.eq_ignore_ascii_case("UNIQUE") {
        Ok((true, key))
    } else if kind.split_whitespace().collect::<Vec<_>>() == ["NOT", "NULL"] {
        Ok((false, key))
    } else {
        Err(cypher_syntax(format!(
            "unsupported constraint predicate; expected IS UNIQUE or IS NOT NULL, got: {kind}"
        )))
    }
}

/// Returns the id of a persisted node that conflicts with `candidate` under a
/// `NodePropertyUnique(label, key)` constraint, or `None` if there is no
/// conflict. A node with the same id as `candidate` is an update, not a
/// conflict, and nodes without the constrained property are ignored.
fn unique_node_conflict<'a>(
    existing: &'a [Node],
    candidate: &Node,
    label: &Label,
    key: &str,
) -> Option<&'a NodeId> {
    if &candidate.label != label {
        return None;
    }
    let value = candidate.props.get(key)?;
    existing
        .iter()
        .find(|node| {
            &node.label == label && node.id != candidate.id && node.props.get(key) == Some(value)
        })
        .map(|node| &node.id)
}

/// Returns the [`edge_key`] of a persisted edge that conflicts with `candidate`
/// under an `EdgePropertyUnique(label, key)` constraint, or `None`. An edge with
/// the same structural key as `candidate` is an update, not a conflict.
fn unique_edge_conflict(
    existing: &[Edge],
    candidate: &Edge,
    label: &Label,
    key: &str,
) -> Option<String> {
    if &candidate.label != label {
        return None;
    }
    let value = candidate.props.get(key)?;
    let candidate_key = edge_key(candidate);
    existing
        .iter()
        .find(|edge| {
            &edge.label == label
                && edge_key(edge) != candidate_key
                && edge.props.get(key) == Some(value)
        })
        .map(edge_key)
}

fn split_cypher_statements(cypher: &str) -> Result<Vec<&str>> {
    let mut statements = Vec::new();
    let mut start = 0usize;
    let mut quote = None;
    let mut escaped = false;

    for (index, ch) in cypher.char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active {
                quote = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' => quote = Some(ch),
            ';' => {
                let statement = cypher[start..index].trim();
                if !statement.is_empty() {
                    statements.push(statement);
                }
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }

    if quote.is_some() {
        return Err(cypher_syntax(
            "Cypher statement has an unterminated string literal".to_string(),
        ));
    }

    let statement = cypher[start..].trim();
    if !statement.is_empty() {
        statements.push(statement);
    }
    Ok(statements)
}

fn strip_cypher_comments(cypher: &str) -> Result<String> {
    let mut output = String::with_capacity(cypher.len());
    let mut chars = cypher.char_indices().peekable();
    let mut quote = None;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;

    while let Some((_, ch)) = chars.next() {
        if line_comment {
            if ch == '\n' {
                line_comment = false;
                output.push(ch);
            }
            continue;
        }
        if block_comment {
            if ch == '*' && chars.peek().is_some_and(|(_, next)| *next == '/') {
                chars.next();
                block_comment = false;
            } else if ch == '\n' {
                output.push(ch);
            }
            continue;
        }
        if let Some(active) = quote {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active {
                quote = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                output.push(ch);
            }
            '/' if chars.peek().is_some_and(|(_, next)| *next == '/') => {
                chars.next();
                line_comment = true;
            }
            '/' if chars.peek().is_some_and(|(_, next)| *next == '*') => {
                chars.next();
                block_comment = true;
            }
            _ => output.push(ch),
        }
    }

    if block_comment {
        return Err(cypher_syntax(
            "Cypher statement has an unterminated block comment".to_string(),
        ));
    }
    Ok(output)
}

#[derive(Clone, Debug)]
struct ParsedCypherNode {
    variable: Option<String>,
    label: Option<Label>,
    props: Props,
    predicates: Vec<GraphPropertyPredicate>,
}

#[derive(Debug)]
struct ParsedCypherEdge {
    from_id: NodeId,
    to_id: NodeId,
    edge: Edge,
}

#[derive(Clone, Debug)]
struct ParsedCypherEdgeMatch {
    from: ParsedCypherNode,
    relationship: ParsedCypherRelationship,
    to: ParsedCypherNode,
}

#[derive(Clone, Debug)]
struct ParsedCypherRelationship {
    variable: Option<String>,
    label: Label,
    props: Props,
    predicates: Vec<GraphPropertyPredicate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CypherBoundEdgeIdentity {
    from: NodeId,
    label: Label,
    to: NodeId,
    id: Option<EdgeId>,
}

#[derive(Clone, Debug, PartialEq)]
struct CypherRowProducedEdgeBinding {
    kind: GraphMutationPlanKind,
    from_variable: String,
    from: GraphNodeMatch,
    to_variable: String,
    to: GraphNodeMatch,
    label: Label,
    props: Props,
    edge_id_policy: GraphRowEdgeIdPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CypherRowProducedPathBinding {
    from_variable: String,
    edge_variable: String,
    to_variable: String,
}

struct ParsedWherePredicate {
    target: String,
    predicate: GraphPropertyPredicate,
}

fn parse_cypher_node_pattern<'a>(
    input: &'a str,
    parameters: &CypherParameters,
) -> Result<(ParsedCypherNode, &'a str)> {
    let input = input.trim_start();
    let input = input.strip_prefix('(').ok_or_else(|| {
        GrustError::Unsupported("writable Cypher node pattern must start with '('".to_string())
    })?;
    let close = find_matching(input, '(', ')')?;
    let body = input[..close].trim();
    let rest = &input[close + 1..];
    let (variable, label, props) = parse_cypher_node_body(body, parameters)?;
    Ok((
        ParsedCypherNode {
            variable,
            label,
            props,
            predicates: Vec::new(),
        },
        rest,
    ))
}

fn parse_cypher_node_body(
    body: &str,
    parameters: &CypherParameters,
) -> Result<(Option<String>, Option<Label>, Props)> {
    let (head, props) = split_cypher_body_props(body, parameters)?;
    let head = head.trim();
    let (variable, label) = if let Some((variable, label)) = head.split_once(':') {
        let label = label.trim();
        (
            parse_optional_cypher_variable(variable.trim())?,
            if label.is_empty() {
                None
            } else {
                Some(Label::new(label.to_string()))
            },
        )
    } else {
        (parse_optional_cypher_variable(head)?, None)
    };
    Ok((variable, label, props))
}

fn parse_optional_cypher_variable(value: &str) -> Result<Option<String>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if is_cypher_identifier(value) {
        return Ok(Some(value.to_string()));
    }
    Err(GrustError::Unsupported(format!(
        "unsupported Cypher variable name: {value}"
    )))
}

fn parse_required_cypher_variable(value: &str, context: &str) -> Result<String> {
    parse_optional_cypher_variable(value)?
        .ok_or_else(|| GrustError::Unsupported(format!("{context} requires a variable name")))
}

fn is_cypher_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn parse_cypher_relationship(
    body: &str,
    parameters: &CypherParameters,
) -> Result<ParsedCypherRelationship> {
    let (head, props) = split_cypher_body_props(body.trim(), parameters)?;
    let Some((variable, label)) = head.trim().split_once(':') else {
        return Err(GrustError::Unsupported(
            "edge CREATE/MERGE/DELETE requires a relationship type".into(),
        ));
    };
    let label = label.trim();
    if label.is_empty() {
        return Err(GrustError::Unsupported(
            "edge CREATE/MERGE/DELETE requires a relationship type".into(),
        ));
    }
    Ok(ParsedCypherRelationship {
        variable: parse_optional_cypher_variable(variable.trim())?,
        label: Label::new(label.to_string()),
        props,
        predicates: Vec::new(),
    })
}

fn validate_optional_edge_id_property(props: &Props) -> Result<()> {
    edge_id_from_props(props).map(|_| ())
}

fn edge_id_from_props(props: &Props) -> Result<Option<String>> {
    match props.get("id") {
        Some(Value::String(id)) => Ok(Some(id.clone())),
        Some(_) => Err(cypher_syntax(
            "relationship id property must be a string literal",
        )),
        None => Ok(None),
    }
}

fn match_node_cardinality(node: &ParsedCypherNode) -> GraphMutationCardinality {
    if node.label.is_some() || !node.props.is_empty() || !node.predicates.is_empty() {
        GraphMutationCardinality::BoundedMany
    } else {
        GraphMutationCardinality::UnboundedMany
    }
}

fn split_match_where<'a>(
    pattern: &'a str,
    parameters: &CypherParameters,
) -> Result<(&'a str, Vec<ParsedWherePredicate>)> {
    let Some(index) = find_unquoted_keyword(pattern, "WHERE") else {
        return Ok((pattern.trim(), Vec::new()));
    };
    let match_pattern = pattern[..index].trim();
    let where_clause = pattern[index + "WHERE".len()..].trim();
    if match_pattern.is_empty() || where_clause.is_empty() {
        return Err(cypher_syntax(
            "MATCH WHERE requires both a pattern and predicate".to_string(),
        ));
    }
    let predicates = split_top_level_and(where_clause)?
        .into_iter()
        .map(|predicate| parse_where_predicate(predicate, parameters))
        .collect::<Result<Vec<_>>>()?;
    Ok((match_pattern, predicates))
}

fn split_top_level_and(value: &str) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    let mut rest = value.trim();
    while let Some(index) = find_unquoted_keyword(rest, "AND") {
        let part = rest[..index].trim();
        if part.is_empty() {
            return Err(cypher_syntax("empty predicate before AND".to_string()));
        }
        parts.push(part);
        rest = rest[index + "AND".len()..].trim();
    }
    if rest.is_empty() {
        return Err(cypher_syntax("empty predicate after AND".to_string()));
    }
    parts.push(rest);
    Ok(parts)
}

fn parse_where_predicate(
    predicate: &str,
    parameters: &CypherParameters,
) -> Result<ParsedWherePredicate> {
    for (token, op) in [
        (">=", GraphPredicateOp::GreaterThanOrEqual),
        ("<=", GraphPredicateOp::LessThanOrEqual),
        ("<>", GraphPredicateOp::NotEqual),
        ("!=", GraphPredicateOp::NotEqual),
        ("=", GraphPredicateOp::Equal),
        (">", GraphPredicateOp::GreaterThan),
        ("<", GraphPredicateOp::LessThan),
    ] {
        let index = if token.len() == 1 {
            find_unquoted(predicate, token.chars().next().expect("operator"))
        } else {
            find_unquoted_sequence(predicate, token)
        };
        if let Some(index) = index {
            let (target, key) = parse_property_ref(&predicate[..index], "MATCH WHERE predicate")?;
            let value = parse_cypher_literal(&predicate[index + token.len()..], parameters)?;
            if matches!(
                op,
                GraphPredicateOp::GreaterThan
                    | GraphPredicateOp::GreaterThanOrEqual
                    | GraphPredicateOp::LessThan
                    | GraphPredicateOp::LessThanOrEqual
            ) && !matches!(value, Value::Int(_) | Value::Float(_) | Value::String(_))
            {
                return Err(cypher_syntax(
                    "MATCH WHERE ordered comparisons require integer, float, or string literals",
                ));
            }
            return Ok(ParsedWherePredicate {
                target,
                predicate: GraphPropertyPredicate { key, op, value },
            });
        }
    }
    Err(cypher_syntax(
        "MATCH WHERE only supports property comparisons against literals or parameters",
    ))
}

fn apply_node_where_predicates(
    node: &mut ParsedCypherNode,
    predicates: Vec<ParsedWherePredicate>,
    context: &str,
) -> Result<()> {
    for predicate in predicates {
        if node.variable.as_deref() == Some(predicate.target.as_str()) {
            node.predicates.push(predicate.predicate);
        } else {
            return Err(cypher_unresolved_identity(format!(
                "{context} WHERE references unknown variable '{}'",
                predicate.target
            )));
        }
    }
    Ok(())
}

fn apply_match_where_predicates(
    nodes: &mut BTreeMap<String, ParsedCypherNode>,
    predicates: Vec<ParsedWherePredicate>,
    context: &str,
) -> Result<()> {
    for predicate in predicates {
        let Some(node) = nodes.get_mut(&predicate.target) else {
            return Err(cypher_unresolved_identity(format!(
                "{context} WHERE references unknown variable '{}'",
                predicate.target
            )));
        };
        node.predicates.push(predicate.predicate);
    }
    Ok(())
}

fn apply_edge_where_predicates(
    edge: &mut ParsedCypherEdgeMatch,
    predicates: Vec<ParsedWherePredicate>,
    context: &str,
) -> Result<()> {
    for predicate in predicates {
        if edge.from.variable.as_deref() == Some(predicate.target.as_str()) {
            edge.from.predicates.push(predicate.predicate);
        } else if edge.to.variable.as_deref() == Some(predicate.target.as_str()) {
            edge.to.predicates.push(predicate.predicate);
        } else if edge.relationship.variable.as_deref() == Some(predicate.target.as_str()) {
            edge.relationship.predicates.push(predicate.predicate);
        } else {
            return Err(cypher_unresolved_identity(format!(
                "{context} WHERE references unknown variable '{}'",
                predicate.target
            )));
        }
    }
    Ok(())
}

fn split_match_delete(statement: &str) -> Result<(&str, &str)> {
    if let Some(index) = find_unquoted_keyword(statement, "DELETE") {
        let pattern = statement[..index].trim();
        let target = statement[index + "DELETE".len()..].trim();
        if pattern.is_empty() || target.is_empty() {
            return Err(GrustError::Unsupported(
                "MATCH DELETE requires both a pattern and a delete target".to_string(),
            ));
        }
        return Ok((pattern, target));
    }
    Err(GrustError::Unsupported(
        "only ID-resolved MATCH ... DELETE is supported in writable Cypher".to_string(),
    ))
}

fn parse_match_delete_targets(targets: &str) -> Result<Vec<String>> {
    split_top_level_commas(targets)?
        .into_iter()
        .map(str::trim)
        .map(|target| {
            if target.is_empty() {
                Err(cypher_syntax("MATCH DELETE contains an empty target"))
            } else {
                parse_required_cypher_variable(target, "MATCH DELETE target")
            }
        })
        .collect()
}

fn split_match_edge_upsert<'a>(statement: &'a str, keyword: &str) -> Result<(&'a str, &'a str)> {
    if let Some(index) = find_unquoted_keyword(statement, keyword) {
        let match_clause = statement[..index].trim();
        let edge_pattern = statement[index + keyword.len()..].trim();
        if match_clause.is_empty() || edge_pattern.is_empty() {
            return Err(GrustError::Unsupported(format!(
                "MATCH {keyword} requires both matched node patterns and an edge pattern"
            )));
        }
        return Ok((match_clause, edge_pattern));
    }
    Err(GrustError::Unsupported(format!(
        "only ID-resolved MATCH ... {keyword} edge is supported in writable Cypher",
    )))
}

fn parse_row_path_binding(pattern: &str) -> Result<(Option<String>, &str)> {
    let Some(index) = find_unquoted(pattern, '=') else {
        return Ok((None, pattern.trim()));
    };
    let variable = parse_required_cypher_variable(
        pattern[..index].trim(),
        "MATCH CREATE/MERGE path variable",
    )?;
    let relationship_pattern = pattern[index + 1..].trim();
    if !relationship_pattern.starts_with('(') {
        return Err(cypher_syntax(
            "MATCH CREATE/MERGE path variable must bind a relationship pattern",
        ));
    }
    Ok((Some(variable), relationship_pattern))
}

fn split_match_set(statement: &str) -> Result<(&str, &str)> {
    if let Some(index) = find_unquoted_keyword(statement, "SET") {
        let pattern = statement[..index].trim();
        let assignment = statement[index + "SET".len()..].trim();
        if pattern.is_empty() || assignment.is_empty() {
            return Err(GrustError::Unsupported(
                "MATCH SET requires both a pattern and a patch assignment".to_string(),
            ));
        }
        return Ok((pattern, assignment));
    }
    Err(GrustError::Unsupported(
        "only ID-resolved MATCH ... SET += is supported in writable Cypher".to_string(),
    ))
}

fn split_match_remove(statement: &str) -> Result<(&str, &str)> {
    if let Some(index) = find_unquoted_keyword(statement, "REMOVE") {
        let pattern = statement[..index].trim();
        let target = statement[index + "REMOVE".len()..].trim();
        if pattern.is_empty() || target.is_empty() {
            return Err(cypher_syntax(
                "MATCH REMOVE requires both a pattern and a property target",
            ));
        }
        return Ok((pattern, target));
    }
    Err(cypher_syntax(
        "only ID-resolved MATCH ... REMOVE property is supported in writable Cypher",
    ))
}

fn split_final_return(statement: &str) -> Result<(&str, &str)> {
    let Some(index) = find_unquoted_keyword(statement, "RETURN") else {
        return Err(cypher_syntax(
            "writable Cypher returning execution requires a final RETURN clause",
        ));
    };
    let mutation = statement[..index].trim();
    let return_clause = statement[index + "RETURN".len()..].trim();
    if return_clause.is_empty() {
        return Err(cypher_syntax("RETURN requires at least one projection"));
    }
    Ok((mutation, return_clause))
}

fn find_return_control_clause(return_clause: &str) -> Option<usize> {
    let mut earliest: Option<usize> = None;
    for keyword in ["ORDER", "LIMIT", "SKIP", "OFFSET"] {
        let mut offset = 0usize;
        let mut rest = return_clause;
        while let Some(index) = find_unquoted_keyword(rest, keyword) {
            let absolute = offset + index;
            let previous = return_clause[..absolute]
                .chars()
                .rev()
                .find(|ch| !ch.is_whitespace());
            if previous != Some('.') && !is_return_alias_keyword_prefix(&return_clause[..absolute])
            {
                if keyword != "ORDER"
                    || rest[index + keyword.len()..]
                        .trim_start()
                        .get(..2)
                        .is_some_and(|value| value.eq_ignore_ascii_case("BY"))
                {
                    // Keep the earliest control keyword across all three so the
                    // projection/control split point is correct regardless of
                    // the order the keywords appear in the clause.
                    earliest = Some(earliest.map_or(absolute, |current| current.min(absolute)));
                    break;
                }
            }
            let next = index + keyword.len();
            offset += next;
            rest = &rest[next..];
        }
    }
    earliest
}

fn is_return_alias_keyword_prefix(prefix: &str) -> bool {
    let mut words = prefix.split_whitespace().rev();
    words
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case("AS"))
}

#[derive(Clone, Debug, PartialEq)]
struct CypherReturnClause {
    projections: Vec<CypherReturnProjection>,
    order_by: Vec<CypherOrderItem>,
    skip: Option<usize>,
    limit: Option<usize>,
    distinct: bool,
}

/// One `ORDER BY` term, resolved to the index of a returned column. Ordering by
/// expressions that are not part of the projection is not supported.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CypherOrderItem {
    column: usize,
    descending: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct CypherReturnProjection {
    variable: String,
    target: CypherReturnTarget,
    column: String,
    expression: String,
    element: CypherReturnElement,
    aggregate: Option<CypherReturnAggregate>,
    distinct: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum CypherReturnTarget {
    All,
    Element,
    Literal(Value),
    Property(String),
    MapProjection(Vec<String>),
    ListProjection(Vec<String>),
    Case(CypherReturnCase),
    Coalesce(CypherReturnCoalesce),
    PropertyExists(String),
    PropertySize(String),
    PropertyAbs(String),
    PropertyNumericRound {
        key: String,
        round: CypherReturnNumericRound,
    },
    PropertyToString(String),
    PropertyStringTransform {
        key: String,
        transform: CypherReturnStringTransform,
    },
    PropertyStringTrim {
        key: String,
        trim: CypherReturnStringTrim,
    },
    PropertyIsEmpty(String),
    PropertyStringReverse(String),
    PropertyStringSplit(CypherReturnStringSplit),
    PropertySubstring(CypherReturnSubstring),
    PropertyStringSlice(CypherReturnStringSlice),
    PropertyReplace(CypherReturnReplace),
    PropertyStringPredicate(CypherReturnStringPredicateProjection),
    NodeLabels,
    RelationshipType,
    ElementProperties,
    ElementKeys,
    ElementId,
    RelationshipStartNode,
    RelationshipEndNode,
    PathLength,
    PathNodes,
    PathRelationships,
}

#[derive(Clone, Debug, PartialEq)]
struct CypherReturnCoalesce {
    variable: Option<String>,
    terms: Vec<CypherReturnCoalesceTerm>,
}

#[derive(Clone, Debug, PartialEq)]
enum CypherReturnCoalesceTerm {
    Property(String),
    Literal(Value),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CypherReturnStringTransform {
    Lower,
    Upper,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CypherReturnStringTrim {
    Both,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CypherReturnNumericRound {
    Ceil,
    Floor,
}

#[derive(Clone, Debug, PartialEq)]
struct CypherReturnCase {
    key: String,
    equals: Value,
    then_value: Value,
    else_value: Value,
}

#[derive(Clone, Debug, PartialEq)]
struct CypherReturnSubstring {
    key: String,
    start: usize,
    length: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
struct CypherReturnStringSplit {
    key: String,
    delimiter: String,
}

#[derive(Clone, Debug, PartialEq)]
struct CypherReturnStringSlice {
    key: String,
    side: CypherReturnStringSliceSide,
    length: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CypherReturnStringSliceSide {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
struct CypherReturnReplace {
    key: String,
    search: String,
    replacement: String,
}

#[derive(Clone, Debug, PartialEq)]
struct CypherReturnStringPredicateProjection {
    key: String,
    predicate: CypherReturnStringPredicate,
    needle: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CypherReturnStringPredicate {
    StartsWith,
    EndsWith,
    Contains,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CypherReturnElement {
    Node,
    Edge,
    RowNode,
    RowEdge,
    RowPath,
    Literal,
    Aggregate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CypherReturnAggregate {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Collect,
}

fn parse_cypher_return_clause(
    clause: &str,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_bindings: &HashMap<String, GraphNodeMatch>,
    row_edge_match_bindings: &HashMap<String, GraphRelationshipMatch>,
    row_edge_bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    parameters: &CypherParameters,
) -> Result<CypherReturnClause> {
    let (projection_clause, control_clause) = split_return_control(clause);
    if projection_clause.eq_ignore_ascii_case("DISTINCT") {
        return Err(cypher_syntax("RETURN DISTINCT requires a projection"));
    }
    let (projection_clause, distinct) =
        if let Some(after_distinct) = strip_leading_keyword(projection_clause, "DISTINCT") {
            let projection_clause = after_distinct.trim();
            if projection_clause.is_empty() {
                return Err(cypher_syntax("RETURN DISTINCT requires a projection"));
            }
            (projection_clause, true)
        } else {
            (projection_clause, false)
        };
    let mut projections = Vec::new();
    for projection in split_top_level_commas(projection_clause)? {
        let projection = projection.trim();
        if projection.is_empty() {
            return Err(cypher_syntax("RETURN contains an empty projection"));
        }
        let (expression, alias) = split_return_alias(projection)?;
        if let Some((aggregate, variable, target, distinct)) =
            parse_aggregate_projection(expression, parameters)?
        {
            if let Some(variable) = variable.as_ref() {
                validate_return_variable_binding(
                    variable,
                    node_bindings,
                    edge_bindings,
                    row_node_bindings,
                    row_edge_match_bindings,
                    row_edge_bindings,
                    row_path_bindings,
                )?;
                if matches!(
                    target,
                    CypherReturnTarget::PathLength
                        | CypherReturnTarget::PathNodes
                        | CypherReturnTarget::PathRelationships
                ) && !row_path_bindings.contains_key(variable)
                {
                    return Err(cypher_unsupported_cardinality(
                        "writable Cypher RETURN path functions require a bound path variable",
                    ));
                }
                let element = cypher_return_element_for_variable(
                    variable,
                    node_bindings,
                    edge_bindings,
                    row_node_bindings,
                    row_edge_match_bindings,
                    row_edge_bindings,
                    row_path_bindings,
                )?;
                validate_return_function_target(&target, element)?;
            }
            projections.push(CypherReturnProjection {
                variable: variable.unwrap_or_default(),
                target,
                column: alias.unwrap_or_else(|| expression.trim().to_string()),
                expression: expression.trim().to_string(),
                element: CypherReturnElement::Aggregate,
                aggregate: Some(aggregate),
                distinct,
            });
            continue;
        }
        if projection == "*" {
            append_star_return_projections(
                &mut projections,
                node_bindings,
                edge_bindings,
                row_node_bindings,
                row_edge_match_bindings,
                row_edge_bindings,
                row_path_bindings,
            )?;
            continue;
        }
        let (variable, target) = if let Some((variable, path_target)) =
            parse_return_path_function_projection(expression)?
        {
            (variable, path_target)
        } else if let Some((variable, element_target)) =
            parse_return_element_function_projection(expression)?
        {
            (variable, element_target)
        } else if let Some((variable, coalesce)) =
            parse_return_coalesce_projection(expression, parameters)?
        {
            (
                variable.unwrap_or_default(),
                CypherReturnTarget::Coalesce(coalesce),
            )
        } else if let Some((variable, key)) = parse_return_exists_projection(expression)? {
            (variable, CypherReturnTarget::PropertyExists(key))
        } else if let Some((variable, key)) = parse_return_size_projection(expression)? {
            (variable, CypherReturnTarget::PropertySize(key))
        } else if let Some((variable, key)) = parse_return_abs_projection(expression)? {
            (variable, CypherReturnTarget::PropertyAbs(key))
        } else if let Some((variable, key, round)) =
            parse_return_numeric_round_projection(expression)?
        {
            (
                variable,
                CypherReturnTarget::PropertyNumericRound { key, round },
            )
        } else if let Some((variable, key)) = parse_return_to_string_projection(expression)? {
            (variable, CypherReturnTarget::PropertyToString(key))
        } else if let Some((variable, key, transform)) =
            parse_return_string_transform_projection(expression)?
        {
            (
                variable,
                CypherReturnTarget::PropertyStringTransform { key, transform },
            )
        } else if let Some((variable, key, trim)) = parse_return_string_trim_projection(expression)?
        {
            (
                variable,
                CypherReturnTarget::PropertyStringTrim { key, trim },
            )
        } else if let Some((variable, key)) = parse_return_is_empty_projection(expression)? {
            (variable, CypherReturnTarget::PropertyIsEmpty(key))
        } else if let Some((variable, key)) = parse_return_string_reverse_projection(expression)? {
            (variable, CypherReturnTarget::PropertyStringReverse(key))
        } else if let Some((variable, split)) =
            parse_return_string_split_projection(expression, parameters)?
        {
            (variable, CypherReturnTarget::PropertyStringSplit(split))
        } else if let Some((variable, substring)) =
            parse_return_substring_projection(expression, parameters)?
        {
            (variable, CypherReturnTarget::PropertySubstring(substring))
        } else if let Some((variable, slice)) =
            parse_return_string_slice_projection(expression, parameters)?
        {
            (variable, CypherReturnTarget::PropertyStringSlice(slice))
        } else if let Some((variable, replace)) =
            parse_return_replace_projection(expression, parameters)?
        {
            (variable, CypherReturnTarget::PropertyReplace(replace))
        } else if let Some((variable, predicate)) =
            parse_return_string_predicate_projection(expression, parameters)?
        {
            (
                variable,
                CypherReturnTarget::PropertyStringPredicate(predicate),
            )
        } else if expression.contains('(') || expression.contains(')') {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN only supports bound element, property, and restricted path projections",
            ));
        } else if let Some((variable, case)) = parse_return_case_projection(expression, parameters)?
        {
            (variable, CypherReturnTarget::Case(case))
        } else if let Some(literal) = parse_return_literal_projection(expression, parameters)? {
            (String::new(), CypherReturnTarget::Literal(literal))
        } else if let Some((variable, keys)) = parse_return_list_projection(expression)? {
            (variable, CypherReturnTarget::ListProjection(keys))
        } else if let Some((variable, keys)) = parse_return_map_projection(expression)? {
            (variable, CypherReturnTarget::MapProjection(keys))
        } else if let Ok((variable, key)) = parse_property_ref(expression, "RETURN projection") {
            (variable, CypherReturnTarget::Property(key))
        } else {
            (
                parse_required_cypher_variable(expression, "RETURN projection")?,
                CypherReturnTarget::Element,
            )
        };
        let element = if matches!(target, CypherReturnTarget::Literal(_))
            || matches!(
                target,
                CypherReturnTarget::Coalesce(CypherReturnCoalesce { variable: None, .. })
            ) {
            CypherReturnElement::Literal
        } else {
            cypher_return_element_for_variable(
                &variable,
                node_bindings,
                edge_bindings,
                row_node_bindings,
                row_edge_match_bindings,
                row_edge_bindings,
                row_path_bindings,
            )?
        };
        if matches!(
            target,
            CypherReturnTarget::PathLength
                | CypherReturnTarget::PathNodes
                | CypherReturnTarget::PathRelationships
        ) && element != CypherReturnElement::RowPath
        {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN path functions require a bound path variable",
            ));
        }
        validate_return_function_target(&target, element)?;
        projections.push(CypherReturnProjection {
            variable,
            target,
            column: alias.unwrap_or_else(|| expression.trim().to_string()),
            expression: expression.trim().to_string(),
            element,
            aggregate: None,
            distinct: false,
        });
    }
    if projections.is_empty() {
        return Err(cypher_syntax("RETURN requires at least one projection"));
    }
    let order_keys = projections
        .iter()
        .map(|projection| vec![projection.column.clone(), projection.expression.clone()])
        .collect::<Vec<_>>();
    let (order_by, skip, limit) = parse_return_control(control_clause, &order_keys)?;
    Ok(CypherReturnClause {
        projections,
        order_by,
        skip,
        limit,
        distinct,
    })
}

/// Splits a RETURN clause into its projection list and the optional trailing
/// `ORDER BY` / `SKIP` / `LIMIT` control clause.
fn split_return_control(clause: &str) -> (&str, &str) {
    match find_return_control_clause(clause) {
        Some(index) => (clause[..index].trim(), clause[index..].trim()),
        None => (clause.trim(), ""),
    }
}

/// Parses a `ORDER BY ... [SKIP/OFFSET n] [LIMIT n]` control clause. Cypher's
/// canonical `ORDER BY`, then row offset, then `LIMIT` ordering is required,
/// and `ORDER BY` terms must reference returned column names or aliases.
fn parse_return_control(
    control: &str,
    order_keys: &[Vec<String>],
) -> Result<(Vec<CypherOrderItem>, Option<usize>, Option<usize>)> {
    let mut rest = control.trim();
    let mut order_by = Vec::new();
    if let Some(after_order) = strip_leading_keyword(rest, "ORDER") {
        let after_by = strip_leading_keyword(after_order.trim_start(), "BY")
            .ok_or_else(|| cypher_syntax("ORDER must be followed by BY"))?;
        let (items, tail) = split_before_keywords(after_by, &["SKIP", "OFFSET", "LIMIT"]);
        order_by = parse_order_items(items, order_keys)?;
        rest = tail.trim_start();
    }
    let mut skip = None;
    if let Some(after_skip) =
        strip_leading_keyword(rest, "SKIP").or_else(|| strip_leading_keyword(rest, "OFFSET"))
    {
        let (count, tail) = split_before_keywords(after_skip, &["LIMIT"]);
        skip = Some(parse_return_count(count, "SKIP/OFFSET")?);
        rest = tail.trim_start();
    }
    let mut limit = None;
    if let Some(after_limit) = strip_leading_keyword(rest, "LIMIT") {
        limit = parse_return_limit(after_limit)?;
        rest = "";
    }
    if !rest.trim().is_empty() {
        return Err(cypher_syntax(format!(
            "unsupported RETURN clause tail; expected ORDER BY, SKIP/OFFSET, then LIMIT: {}",
            rest.trim()
        )));
    }
    Ok((order_by, skip, limit))
}

/// Returns the slice of `value` before the first top-level occurrence of any of
/// `keywords`, plus the remainder starting at that keyword.
fn split_before_keywords<'a>(value: &'a str, keywords: &[&str]) -> (&'a str, &'a str) {
    let split = keywords
        .iter()
        .filter_map(|keyword| find_unquoted_keyword(value, keyword))
        .min();
    match split {
        Some(index) => (value[..index].trim(), &value[index..]),
        None => (value.trim(), ""),
    }
}

fn parse_order_items(items: &str, order_keys: &[Vec<String>]) -> Result<Vec<CypherOrderItem>> {
    let mut order_by = Vec::new();
    for item in split_top_level_commas(items)? {
        let item = item.trim();
        if item.is_empty() {
            return Err(cypher_syntax("ORDER BY contains an empty term"));
        }
        let (expression, descending) = if let Some(prefix) = strip_trailing_keyword(item, "DESC") {
            (prefix, true)
        } else if let Some(prefix) = strip_trailing_keyword(item, "DESCENDING") {
            (prefix, true)
        } else if let Some(prefix) = strip_trailing_keyword(item, "ASC") {
            (prefix, false)
        } else if let Some(prefix) = strip_trailing_keyword(item, "ASCENDING") {
            (prefix, false)
        } else {
            (item, false)
        };
        let expression = expression.trim();
        let column = order_keys
            .iter()
            .position(|keys| keys.iter().any(|key| key == expression))
            .ok_or_else(|| {
                cypher_unsupported_cardinality(format!(
                    "ORDER BY '{expression}' must reference a returned column, alias, or projection expression"
                ))
            })?;
        order_by.push(CypherOrderItem { column, descending });
    }
    if order_by.is_empty() {
        return Err(cypher_syntax("ORDER BY requires at least one term"));
    }
    Ok(order_by)
}

fn parse_aggregate_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<
    Option<(
        CypherReturnAggregate,
        Option<String>,
        CypherReturnTarget,
        bool,
    )>,
> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let aggregate = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "count" => CypherReturnAggregate::Count,
        "sum" => CypherReturnAggregate::Sum,
        "avg" => CypherReturnAggregate::Avg,
        "min" => CypherReturnAggregate::Min,
        "max" => CypherReturnAggregate::Max,
        "collect" => CypherReturnAggregate::Collect,
        _ => {
            return Ok(None);
        }
    };
    let aggregate_name = match aggregate {
        CypherReturnAggregate::Count => "COUNT",
        CypherReturnAggregate::Sum => "SUM",
        CypherReturnAggregate::Avg => "AVG",
        CypherReturnAggregate::Min => "MIN",
        CypherReturnAggregate::Max => "MAX",
        CypherReturnAggregate::Collect => "COLLECT",
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(format!(
            "{aggregate_name} projection is missing ')'"
        )));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    let (body, distinct) = if let Some(after_distinct) = strip_leading_keyword(body, "DISTINCT") {
        let body = after_distinct.trim();
        if body.is_empty() {
            return Err(cypher_syntax(format!(
                "{aggregate_name} DISTINCT requires a target"
            )));
        }
        (body, true)
    } else {
        (body, false)
    };
    if let Some((variable, target)) = parse_return_path_function_projection(body)? {
        return Ok(Some((aggregate, Some(variable), target, distinct)));
    }
    if let Some((variable, target)) = parse_return_element_function_projection(body)? {
        return Ok(Some((aggregate, Some(variable), target, distinct)));
    }
    if !matches!(
        aggregate,
        CypherReturnAggregate::Count | CypherReturnAggregate::Collect
    ) && body == "*"
    {
        return Err(cypher_unsupported_cardinality(format!(
            "writable Cypher RETURN does not support {aggregate_name}(*)"
        )));
    }
    if body == "*" {
        if aggregate == CypherReturnAggregate::Count && distinct {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN does not support COUNT(DISTINCT *)",
            ));
        }
        return Ok(Some((aggregate, None, CypherReturnTarget::All, distinct)));
    }
    if let Some(literal) = parse_return_literal_projection(body, parameters)? {
        return Ok(Some((
            aggregate,
            None,
            CypherReturnTarget::Literal(literal),
            distinct,
        )));
    }
    if let Some((variable, coalesce)) = parse_return_coalesce_projection(body, parameters)? {
        return Ok(Some((
            aggregate,
            variable,
            CypherReturnTarget::Coalesce(coalesce),
            distinct,
        )));
    }
    if let Some((variable, key)) = parse_return_exists_projection(body)? {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::PropertyExists(key),
            distinct,
        )));
    }
    if let Some((variable, key)) = parse_return_size_projection(body)? {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::PropertySize(key),
            distinct,
        )));
    }
    if let Some((variable, key)) = parse_return_abs_projection(body)? {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::PropertyAbs(key),
            distinct,
        )));
    }
    if let Some((variable, key, round)) = parse_return_numeric_round_projection(body)? {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::PropertyNumericRound { key, round },
            distinct,
        )));
    }
    if let Some((variable, key)) = parse_return_to_string_projection(body)? {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::PropertyToString(key),
            distinct,
        )));
    }
    if let Some((variable, key, transform)) = parse_return_string_transform_projection(body)? {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::PropertyStringTransform { key, transform },
            distinct,
        )));
    }
    if let Some((variable, key, trim)) = parse_return_string_trim_projection(body)? {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::PropertyStringTrim { key, trim },
            distinct,
        )));
    }
    if let Some((variable, key)) = parse_return_is_empty_projection(body)? {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::PropertyIsEmpty(key),
            distinct,
        )));
    }
    if let Some((variable, key)) = parse_return_string_reverse_projection(body)? {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::PropertyStringReverse(key),
            distinct,
        )));
    }
    if let Some((variable, split)) = parse_return_string_split_projection(body, parameters)? {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::PropertyStringSplit(split),
            distinct,
        )));
    }
    if let Some((variable, substring)) = parse_return_substring_projection(body, parameters)? {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::PropertySubstring(substring),
            distinct,
        )));
    }
    if let Some((variable, slice)) = parse_return_string_slice_projection(body, parameters)? {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::PropertyStringSlice(slice),
            distinct,
        )));
    }
    if let Some((variable, replace)) = parse_return_replace_projection(body, parameters)? {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::PropertyReplace(replace),
            distinct,
        )));
    }
    if let Some((variable, predicate)) = parse_return_string_predicate_projection(body, parameters)?
    {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::PropertyStringPredicate(predicate),
            distinct,
        )));
    }
    if let Some((variable, case)) = parse_return_case_projection(body, parameters)? {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::Case(case),
            distinct,
        )));
    }
    if let Some((variable, keys)) = parse_return_list_projection(body)? {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::ListProjection(keys),
            distinct,
        )));
    }
    if let Some((variable, keys)) = parse_return_map_projection(body)? {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::MapProjection(keys),
            distinct,
        )));
    }
    if !matches!(
        aggregate,
        CypherReturnAggregate::Count | CypherReturnAggregate::Collect
    ) && !body.contains('.')
    {
        return Err(cypher_unsupported_cardinality(format!(
            "writable Cypher RETURN only supports {aggregate_name}(variable.property) or restricted CASE"
        )));
    }
    if let Ok((variable, key)) = parse_property_ref(body, "RETURN aggregate projection") {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::Property(key),
            distinct,
        )));
    }
    if !matches!(
        aggregate,
        CypherReturnAggregate::Count | CypherReturnAggregate::Collect
    ) {
        return Err(cypher_unsupported_cardinality(format!(
            "writable Cypher RETURN only supports {aggregate_name}(variable.property) or restricted CASE"
        )));
    }
    Ok(Some((
        aggregate,
        Some(parse_required_cypher_variable(
            body,
            "RETURN aggregate projection",
        )?),
        CypherReturnTarget::Element,
        distinct,
    )))
}

fn parse_return_literal_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<Value>> {
    let expression = expression.trim();
    if !is_return_literal_candidate(expression) {
        return Ok(None);
    }
    parse_cypher_literal(expression, parameters).map(Some)
}

fn is_return_literal_candidate(expression: &str) -> bool {
    if expression.starts_with('\'')
        || expression.starts_with('"')
        || expression.starts_with('$')
        || expression.eq_ignore_ascii_case("true")
        || expression.eq_ignore_ascii_case("false")
        || expression.eq_ignore_ascii_case("null")
    {
        return true;
    }
    expression
        .chars()
        .next()
        .is_some_and(|ch| ch == '-' || ch == '+' || ch.is_ascii_digit())
}

fn parse_return_coalesce_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnCoalesce)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("coalesce") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN coalesce projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN coalesce projection requires at least one argument",
        ));
    }

    let mut variable = None;
    let mut terms = Vec::new();
    for argument in split_top_level_commas(body)? {
        let argument = argument.trim();
        if argument.is_empty() {
            return Err(cypher_syntax(
                "RETURN coalesce projection contains an empty argument",
            ));
        }
        if let Some(literal) = parse_return_literal_projection(argument, parameters)? {
            terms.push(CypherReturnCoalesceTerm::Literal(literal));
            continue;
        }
        let (argument_variable, key) = parse_property_ref(argument, "RETURN coalesce projection")
            .map_err(|_| {
                cypher_unsupported_cardinality(
                    "writable Cypher RETURN coalesce only supports variable.property and literal arguments",
                )
            })?;
        if let Some(variable) = &variable {
            if variable != &argument_variable {
                return Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN coalesce arguments must reference one variable",
                ));
            }
        } else {
            variable = Some(argument_variable);
        }
        terms.push(CypherReturnCoalesceTerm::Property(key));
    }

    Ok(Some((
        variable.clone(),
        CypherReturnCoalesce { variable, terms },
    )))
}

fn parse_return_exists_projection(expression: &str) -> Result<Option<(String, String)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("exists") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN exists projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN exists projection requires a property reference",
        ));
    }
    if body.contains('(') || body.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN exists only supports variable.property arguments",
        ));
    }
    parse_property_ref(body, "RETURN exists projection")
        .map(Some)
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN exists only supports variable.property arguments",
            )
        })
}

fn parse_return_size_projection(expression: &str) -> Result<Option<(String, String)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("size") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN size projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN size projection requires a property reference",
        ));
    }
    if body.contains('(') || body.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN size only supports variable.property arguments",
        ));
    }
    parse_property_ref(body, "RETURN size projection")
        .map(Some)
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN size only supports variable.property arguments",
            )
        })
}

fn parse_return_abs_projection(expression: &str) -> Result<Option<(String, String)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("abs") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN abs projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN abs projection requires a property reference",
        ));
    }
    if body.contains('(') || body.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN abs only supports variable.property arguments",
        ));
    }
    parse_property_ref(body, "RETURN abs projection")
        .map(Some)
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN abs only supports variable.property arguments",
            )
        })
}

fn parse_return_numeric_round_projection(
    expression: &str,
) -> Result<Option<(String, String, CypherReturnNumericRound)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let round = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "ceil" => CypherReturnNumericRound::Ceil,
        "floor" => CypherReturnNumericRound::Floor,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN numeric rounding projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN numeric rounding projection requires a property reference",
        ));
    }
    if body.contains('(') || body.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN ceil/floor only supports variable.property arguments",
        ));
    }
    parse_property_ref(body, "RETURN numeric rounding projection")
        .map(|(variable, key)| Some((variable, key, round)))
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN ceil/floor only supports variable.property arguments",
            )
        })
}

fn parse_return_to_string_projection(expression: &str) -> Result<Option<(String, String)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("toString") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN toString projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN toString projection requires a property reference",
        ));
    }
    if body.contains('(') || body.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toString only supports variable.property arguments",
        ));
    }
    parse_property_ref(body, "RETURN toString projection")
        .map(Some)
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN toString only supports variable.property arguments",
            )
        })
}

fn parse_return_is_empty_projection(expression: &str) -> Result<Option<(String, String)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("isEmpty") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN isEmpty projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN isEmpty projection requires a property reference",
        ));
    }
    if body.contains('(') || body.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN isEmpty only supports variable.property arguments",
        ));
    }
    parse_property_ref(body, "RETURN isEmpty projection")
        .map(Some)
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN isEmpty only supports variable.property arguments",
            )
        })
}

fn parse_return_string_transform_projection(
    expression: &str,
) -> Result<Option<(String, String, CypherReturnStringTransform)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let transform = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "tolower" => CypherReturnStringTransform::Lower,
        "toupper" => CypherReturnStringTransform::Upper,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN string transform projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN string transform projection requires a property reference",
        ));
    }
    if body.contains('(') || body.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN string transforms only support variable.property arguments",
        ));
    }
    let (variable, key) =
        parse_property_ref(body, "RETURN string transform projection").map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN string transforms only support variable.property arguments",
            )
        })?;
    Ok(Some((variable, key, transform)))
}

fn parse_return_string_trim_projection(
    expression: &str,
) -> Result<Option<(String, String, CypherReturnStringTrim)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let trim = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "trim" => CypherReturnStringTrim::Both,
        "ltrim" => CypherReturnStringTrim::Left,
        "rtrim" => CypherReturnStringTrim::Right,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN string trim projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN string trim projection requires a property reference",
        ));
    }
    if body.contains('(') || body.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN string trims only support variable.property arguments",
        ));
    }
    let (variable, key) =
        parse_property_ref(body, "RETURN string trim projection").map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN string trims only support variable.property arguments",
            )
        })?;
    Ok(Some((variable, key, trim)))
}

fn parse_return_string_reverse_projection(expression: &str) -> Result<Option<(String, String)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("reverse") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN string reverse projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN string reverse projection requires a property reference",
        ));
    }
    if body.contains('(') || body.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN reverse only supports variable.property arguments",
        ));
    }
    parse_property_ref(body, "RETURN string reverse projection")
        .map(Some)
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN reverse only supports variable.property arguments",
            )
        })
}

fn parse_return_string_split_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(String, CypherReturnStringSplit)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("split") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN string split projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN string split projection requires arguments",
        ));
    }
    let arguments = split_top_level_commas(body)?;
    if arguments.len() != 2 {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN split requires variable.property and delimiter",
        ));
    }
    let (variable, key) = parse_property_ref(arguments[0].trim(), "RETURN string split projection")
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN split requires a variable.property first argument",
            )
        })?;
    let delimiter = parse_string_literal_argument(
        arguments[1].trim(),
        parameters,
        "RETURN string split delimiter",
    )?;
    if delimiter.is_empty() {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN split delimiter must be non-empty",
        ));
    }
    Ok(Some((variable, CypherReturnStringSplit { key, delimiter })))
}

fn parse_return_substring_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(String, CypherReturnSubstring)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("substring") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN substring projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN substring projection requires arguments",
        ));
    }
    let arguments = split_top_level_commas(body)?;
    if !(2..=3).contains(&arguments.len()) {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN substring requires variable.property, start, and optional length",
        ));
    }
    let (variable, key) = parse_property_ref(arguments[0].trim(), "RETURN substring projection")
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN substring requires a variable.property first argument",
            )
        })?;
    let start = parse_non_negative_usize_literal(
        arguments[1].trim(),
        parameters,
        "RETURN substring start",
    )?;
    let length = if let Some(length) = arguments.get(2) {
        Some(parse_non_negative_usize_literal(
            length.trim(),
            parameters,
            "RETURN substring length",
        )?)
    } else {
        None
    };
    Ok(Some((
        variable,
        CypherReturnSubstring { key, start, length },
    )))
}

fn parse_non_negative_usize_literal(
    expression: &str,
    parameters: &CypherParameters,
    context: &str,
) -> Result<usize> {
    let value = parse_cypher_literal(expression, parameters)?;
    let Value::Int(value) = value else {
        return Err(cypher_unsupported_cardinality(format!(
            "{context} must be an integer literal or parameter"
        )));
    };
    usize::try_from(value).map_err(|_| {
        cypher_unsupported_cardinality(format!("{context} must be a non-negative integer"))
    })
}

fn parse_return_string_slice_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(String, CypherReturnStringSlice)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let side = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "left" => CypherReturnStringSliceSide::Left,
        "right" => CypherReturnStringSliceSide::Right,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN string slice projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN string slice projection requires arguments",
        ));
    }
    let arguments = split_top_level_commas(body)?;
    if arguments.len() != 2 {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN left/right requires variable.property and length",
        ));
    }
    let (variable, key) = parse_property_ref(arguments[0].trim(), "RETURN string slice projection")
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN left/right requires a variable.property first argument",
            )
        })?;
    let length = parse_non_negative_usize_literal(
        arguments[1].trim(),
        parameters,
        "RETURN left/right length",
    )?;
    Ok(Some((
        variable,
        CypherReturnStringSlice { key, side, length },
    )))
}

fn parse_return_replace_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(String, CypherReturnReplace)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("replace") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN replace projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN replace projection requires arguments",
        ));
    }
    let arguments = split_top_level_commas(body)?;
    if arguments.len() != 3 {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN replace requires variable.property, search, and replacement",
        ));
    }
    let (variable, key) = parse_property_ref(arguments[0].trim(), "RETURN replace projection")
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN replace requires a variable.property first argument",
            )
        })?;
    let search =
        parse_string_literal_argument(arguments[1].trim(), parameters, "RETURN replace search")?;
    let replacement = parse_string_literal_argument(
        arguments[2].trim(),
        parameters,
        "RETURN replace replacement",
    )?;
    Ok(Some((
        variable,
        CypherReturnReplace {
            key,
            search,
            replacement,
        },
    )))
}

fn parse_return_string_predicate_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(String, CypherReturnStringPredicateProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let predicate = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "startswith" => CypherReturnStringPredicate::StartsWith,
        "endswith" => CypherReturnStringPredicate::EndsWith,
        "contains" => CypherReturnStringPredicate::Contains,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN string predicate projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN string predicate projection requires arguments",
        ));
    }
    let arguments = split_top_level_commas(body)?;
    if arguments.len() != 2 {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN string predicates require variable.property and needle",
        ));
    }
    let (variable, key) = parse_property_ref(
        arguments[0].trim(),
        "RETURN string predicate projection",
    )
    .map_err(|_| {
        cypher_unsupported_cardinality(
            "writable Cypher RETURN string predicates require a variable.property first argument",
        )
    })?;
    let needle = parse_string_literal_argument(
        arguments[1].trim(),
        parameters,
        "RETURN string predicate needle",
    )?;
    Ok(Some((
        variable,
        CypherReturnStringPredicateProjection {
            key,
            predicate,
            needle,
        },
    )))
}

fn parse_string_literal_argument(
    expression: &str,
    parameters: &CypherParameters,
    context: &str,
) -> Result<String> {
    let value = parse_cypher_literal(expression, parameters)?;
    let Value::String(value) = value else {
        return Err(cypher_unsupported_cardinality(format!(
            "{context} must be a string literal or parameter"
        )));
    };
    Ok(value)
}

fn parse_return_element_function_projection(
    expression: &str,
) -> Result<Option<(String, CypherReturnTarget)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let target = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "labels" => CypherReturnTarget::NodeLabels,
        "type" => CypherReturnTarget::RelationshipType,
        "properties" => CypherReturnTarget::ElementProperties,
        "keys" => CypherReturnTarget::ElementKeys,
        "id" | "elementid" => CypherReturnTarget::ElementId,
        "startnode" => CypherReturnTarget::RelationshipStartNode,
        "endnode" => CypherReturnTarget::RelationshipEndNode,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN element function projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.contains('(') || body.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN element functions do not support nested expressions",
        ));
    }
    let variable = parse_required_cypher_variable(body, "RETURN element function variable")?;
    Ok(Some((variable, target)))
}

fn parse_return_map_projection(expression: &str) -> Result<Option<(String, Vec<String>)>> {
    let expression = expression.trim();
    let Some(open) = find_unquoted(expression, '{') else {
        return Ok(None);
    };
    if !expression.ends_with('}') {
        return Err(cypher_syntax("RETURN map projection is missing '}'"));
    }
    let variable = parse_required_cypher_variable(
        expression[..open].trim(),
        "RETURN map projection variable",
    )?;
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN map projection requires at least one property selector",
        ));
    }
    let mut keys = Vec::new();
    for selector in split_top_level_commas(body)? {
        let selector = selector.trim();
        let Some(key) = selector.strip_prefix('.') else {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN map projections only support .property selectors",
            ));
        };
        let key = key.trim();
        validate_json_key(key)?;
        keys.push(key.to_string());
    }
    Ok(Some((variable, keys)))
}

fn parse_return_list_projection(expression: &str) -> Result<Option<(String, Vec<String>)>> {
    let expression = expression.trim();
    if !expression.starts_with('[') {
        return Ok(None);
    }
    if !expression.ends_with(']') {
        return Err(cypher_syntax("RETURN list projection is missing ']'"));
    }
    let body = expression[1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN list projection requires at least one property reference",
        ));
    }
    let mut variable = None;
    let mut keys = Vec::new();
    for item in split_top_level_commas(body)? {
        let (item_variable, key) = parse_property_ref(item.trim(), "RETURN list projection")?;
        if let Some(variable) = &variable {
            if variable != &item_variable {
                return Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN list projections must reference one variable",
                ));
            }
        } else {
            variable = Some(item_variable);
        }
        keys.push(key);
    }
    Ok(Some((
        variable.expect("list projection contains at least one item"),
        keys,
    )))
}

fn parse_return_case_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(String, CypherReturnCase)>> {
    let expression = expression.trim();
    let Some(after_case) = strip_leading_keyword(expression, "CASE") else {
        return Ok(None);
    };
    if expression.contains('(') || expression.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN CASE does not support function calls or nested expressions",
        ));
    }
    let after_when = strip_leading_keyword(after_case.trim_start(), "WHEN")
        .ok_or_else(|| cypher_syntax("RETURN CASE requires WHEN"))?;
    let Some(then_index) = find_unquoted_keyword(after_when, "THEN") else {
        return Err(cypher_syntax("RETURN CASE requires THEN"));
    };
    let condition = after_when[..then_index].trim();
    let after_then = after_when[then_index + "THEN".len()..].trim_start();
    let Some(else_index) = find_unquoted_keyword(after_then, "ELSE") else {
        return Err(cypher_syntax("RETURN CASE requires ELSE"));
    };
    let then_value = after_then[..else_index].trim();
    let after_else = after_then[else_index + "ELSE".len()..].trim_start();
    let Some(end_index) = find_unquoted_keyword(after_else, "END") else {
        return Err(cypher_syntax("RETURN CASE requires END"));
    };
    let else_value = after_else[..end_index].trim();
    if !after_else[end_index + "END".len()..].trim().is_empty() {
        return Err(cypher_syntax(
            "RETURN CASE does not support trailing content after END",
        ));
    }
    let Some(equals_index) = find_unquoted(condition, '=') else {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN CASE only supports property equality predicates",
        ));
    };
    let (variable, key) = parse_property_ref(
        condition[..equals_index].trim(),
        "RETURN CASE predicate property",
    )?;
    let equals = parse_cypher_literal(&condition[equals_index + 1..], parameters)?;
    let then_value = parse_cypher_literal(then_value, parameters)?;
    let else_value = parse_cypher_literal(else_value, parameters)?;
    Ok(Some((
        variable,
        CypherReturnCase {
            key,
            equals,
            then_value,
            else_value,
        },
    )))
}

fn parse_return_path_function_projection(
    expression: &str,
) -> Result<Option<(String, CypherReturnTarget)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let target = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "length" => CypherReturnTarget::PathLength,
        "nodes" => CypherReturnTarget::PathNodes,
        "relationships" => CypherReturnTarget::PathRelationships,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN path function projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.contains('(') || body.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN path functions do not support nested expressions",
        ));
    }
    let variable = parse_required_cypher_variable(body, "RETURN path function variable")?;
    Ok(Some((variable, target)))
}

fn append_star_return_projections(
    projections: &mut Vec<CypherReturnProjection>,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_bindings: &HashMap<String, GraphNodeMatch>,
    row_edge_match_bindings: &HashMap<String, GraphRelationshipMatch>,
    row_edge_bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
) -> Result<()> {
    let mut variables = BTreeSet::new();
    variables.extend(node_bindings.keys().cloned());
    variables.extend(edge_bindings.keys().cloned());
    variables.extend(row_node_bindings.keys().cloned());
    variables.extend(row_edge_match_bindings.keys().cloned());
    variables.extend(row_edge_bindings.keys().cloned());
    variables.extend(row_path_bindings.keys().cloned());
    for binding in row_edge_bindings.values() {
        variables.insert(binding.from_variable.clone());
        variables.insert(binding.to_variable.clone());
    }
    if variables.is_empty() {
        return Err(cypher_unresolved_identity(
            "RETURN * has no variables bound by the write plan",
        ));
    }
    for variable in variables {
        let element = cypher_return_element_for_variable(
            &variable,
            node_bindings,
            edge_bindings,
            row_node_bindings,
            row_edge_match_bindings,
            row_edge_bindings,
            row_path_bindings,
        )?;
        projections.push(CypherReturnProjection {
            variable: variable.clone(),
            target: CypherReturnTarget::Element,
            column: variable.clone(),
            expression: variable,
            element,
            aggregate: None,
            distinct: false,
        });
    }
    Ok(())
}

fn cypher_return_element_for_variable(
    variable: &str,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_bindings: &HashMap<String, GraphNodeMatch>,
    row_edge_match_bindings: &HashMap<String, GraphRelationshipMatch>,
    row_edge_bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
) -> Result<CypherReturnElement> {
    match (
        node_bindings.contains_key(variable),
        edge_bindings.contains_key(variable),
        row_node_bindings.contains_key(variable),
        row_edge_match_bindings.contains_key(variable),
        row_edge_bindings.contains_key(variable),
        row_edge_endpoint_variable(variable, row_edge_bindings),
        row_path_bindings.contains_key(variable),
    ) {
        (true, false, false, false, false, false, false)
        | (true, false, false, false, false, true, false) => Ok(CypherReturnElement::Node),
        (false, true, false, false, false, false, false) => Ok(CypherReturnElement::Edge),
        (false, false, true, false, false, false, false)
        | (false, false, true, false, false, true, false)
        | (false, false, false, false, false, true, false) => Ok(CypherReturnElement::RowNode),
        (false, false, false, true, false, false, false)
        | (false, false, false, false, true, false, false) => Ok(CypherReturnElement::RowEdge),
        (false, false, false, false, false, false, true) => Ok(CypherReturnElement::RowPath),
        (true, _, _, _, _, _, _) | (_, true, _, _, _, _, _) | (_, _, true, _, _, _, _) => {
            Err(cypher_unresolved_identity(format!(
                "RETURN variable '{variable}' is ambiguously bound",
            )))
        }
        (false, false, false, true, true, _, _) => Err(cypher_unresolved_identity(format!(
            "RETURN relationship variable '{variable}' is ambiguously bound",
        ))),
        (false, false, false, _, _, true, _) => Err(cypher_unresolved_identity(format!(
            "RETURN variable '{variable}' is ambiguously bound",
        ))),
        (false, false, false, _, _, _, true) => Err(cypher_unresolved_identity(format!(
            "RETURN path variable '{variable}' is ambiguously bound",
        ))),
        (false, false, false, false, false, false, false) => Err(cypher_unresolved_identity(
            format!("RETURN references variable '{variable}' that is not bound by the write plan"),
        )),
    }
}

fn row_edge_endpoint_variable(
    variable: &str,
    row_edge_bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
) -> bool {
    row_edge_bindings
        .values()
        .any(|binding| binding.from_variable == variable || binding.to_variable == variable)
}

fn validate_return_variable_binding(
    variable: &str,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_bindings: &HashMap<String, GraphNodeMatch>,
    row_edge_match_bindings: &HashMap<String, GraphRelationshipMatch>,
    row_edge_bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
) -> Result<()> {
    match (
        node_bindings.contains_key(variable),
        edge_bindings.contains_key(variable),
        row_node_bindings.contains_key(variable),
        row_edge_match_bindings.contains_key(variable),
        row_edge_bindings.contains_key(variable),
        row_edge_endpoint_variable(variable, row_edge_bindings),
        row_path_bindings.contains_key(variable),
    ) {
        (true, false, false, false, false, false, false)
        | (true, false, false, false, false, true, false)
        | (false, true, false, false, false, false, false)
        | (false, false, true, false, false, false, false)
        | (false, false, true, false, false, true, false)
        | (false, false, false, true, false, false, false)
        | (false, false, false, false, true, false, false)
        | (false, false, false, false, false, true, false)
        | (false, false, false, false, false, false, true) => Ok(()),
        (true, _, _, _, _, _, _) | (_, true, _, _, _, _, _) | (_, _, true, _, _, _, _) => {
            Err(cypher_unresolved_identity(format!(
                "RETURN variable '{variable}' is ambiguously bound",
            )))
        }
        (false, false, false, true, true, _, _) => Err(cypher_unresolved_identity(format!(
            "RETURN relationship variable '{variable}' is ambiguously bound",
        ))),
        (false, false, false, _, _, true, _) => Err(cypher_unresolved_identity(format!(
            "RETURN variable '{variable}' is ambiguously bound",
        ))),
        (false, false, false, _, _, _, true) => Err(cypher_unresolved_identity(format!(
            "RETURN path variable '{variable}' is ambiguously bound",
        ))),
        (false, false, false, false, false, false, false) => Err(cypher_unresolved_identity(
            format!("RETURN references variable '{variable}' that is not bound by the write plan"),
        )),
    }
}

fn validate_return_function_target(
    target: &CypherReturnTarget,
    element: CypherReturnElement,
) -> Result<()> {
    match target {
        CypherReturnTarget::NodeLabels
            if !matches!(
                element,
                CypherReturnElement::Node | CypherReturnElement::RowNode
            ) =>
        {
            Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN labels(...) requires a bound node variable",
            ))
        }
        CypherReturnTarget::RelationshipType
            if !matches!(
                element,
                CypherReturnElement::Edge | CypherReturnElement::RowEdge
            ) =>
        {
            Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN type(...) requires a bound relationship variable",
            ))
        }
        CypherReturnTarget::ElementProperties
        | CypherReturnTarget::ElementKeys
        | CypherReturnTarget::ElementId
            if !matches!(
                element,
                CypherReturnElement::Node
                    | CypherReturnElement::Edge
                    | CypherReturnElement::RowNode
                    | CypherReturnElement::RowEdge
            ) =>
        {
            Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN properties(...), keys(...), id(...), and elementId(...) require a bound node or relationship variable",
            ))
        }
        CypherReturnTarget::RelationshipStartNode | CypherReturnTarget::RelationshipEndNode
            if !matches!(
                element,
                CypherReturnElement::Edge | CypherReturnElement::RowEdge
            ) =>
        {
            Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN startNode(...) and endNode(...) require a bound relationship variable",
            ))
        }
        _ => Ok(()),
    }
}

fn strip_trailing_keyword<'a>(value: &'a str, keyword: &str) -> Option<&'a str> {
    let value = value.trim_end();
    let candidate = value.get(value.len().checked_sub(keyword.len())?..)?;
    if !candidate.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let prefix = &value[..value.len() - keyword.len()];
    // Require a word boundary so we do not strip the tail of an identifier.
    if prefix
        .chars()
        .next_back()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(prefix.trim_end())
}

fn parse_return_count(value: &str, context: &str) -> Result<usize> {
    let value = value.trim();
    value.parse::<usize>().map_err(|_| {
        cypher_syntax(format!(
            "{context} requires a non-negative integer, got '{value}'"
        ))
    })
}

fn parse_return_limit(value: &str) -> Result<Option<usize>> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("ALL") {
        return Ok(None);
    }
    Ok(Some(parse_return_count(value, "LIMIT")?))
}

fn split_return_alias(projection: &str) -> Result<(&str, Option<String>)> {
    let Some(index) = find_unquoted_keyword(projection, "AS") else {
        return Ok((projection, None));
    };
    let expression = projection[..index].trim();
    let alias = projection[index + "AS".len()..].trim();
    if expression.is_empty() || alias.is_empty() {
        return Err(cypher_syntax(
            "RETURN aliases require both an expression and an alias",
        ));
    }
    let alias = parse_required_cypher_variable(alias, "RETURN alias")?;
    Ok((expression, Some(alias)))
}

struct PatchAssignment {
    target: String,
    kind: PatchAssignmentKind,
}

enum PatchAssignmentKind {
    Props(Props),
    RemoveProperty {
        key: String,
    },
    NumericExpression {
        key: String,
        source_target: String,
        source_key: String,
        op: GraphNumericOp,
        operand: Value,
    },
}

fn parse_patch_assignment(
    assignment: &str,
    parameters: &CypherParameters,
    null_assignment: CypherNullAssignment,
) -> Result<PatchAssignment> {
    if let Some(index) = find_unquoted_sequence(assignment, "+=") {
        let target = parse_required_cypher_variable(&assignment[..index], "MATCH SET target")?;
        let props = parse_cypher_props_map_literal(&assignment[index + 2..], parameters)?;
        return Ok(PatchAssignment {
            target,
            kind: PatchAssignmentKind::Props(props),
        });
    }
    let Some(index) = find_unquoted(assignment, '=') else {
        return Err(cypher_syntax(
            "MATCH SET only supports map patch or literal property assignment",
        ));
    };
    let (target, key) = parse_property_ref(&assignment[..index], "MATCH SET target")?;
    let rhs = &assignment[index + 1..];
    if let Some(expression) = parse_numeric_expression(rhs, parameters)? {
        return Ok(PatchAssignment {
            target,
            kind: PatchAssignmentKind::NumericExpression {
                key,
                source_target: expression.source_target,
                source_key: expression.source_key,
                op: expression.op,
                operand: expression.operand,
            },
        });
    }
    let value = parse_cypher_literal(rhs, parameters)?;
    if value == Value::Null && null_assignment == CypherNullAssignment::RemoveProperty {
        return Ok(PatchAssignment {
            target,
            kind: PatchAssignmentKind::RemoveProperty { key },
        });
    }
    Ok(PatchAssignment {
        target,
        kind: PatchAssignmentKind::Props(Props::from([(key, value)])),
    })
}

fn parse_patch_assignments(
    assignments: &str,
    parameters: &CypherParameters,
    null_assignment: CypherNullAssignment,
) -> Result<Vec<PatchAssignment>> {
    split_top_level_commas(assignments)?
        .into_iter()
        .map(str::trim)
        .map(|assignment| {
            if assignment.is_empty() {
                Err(cypher_syntax("MATCH SET contains an empty assignment"))
            } else {
                parse_patch_assignment(assignment, parameters, null_assignment)
            }
        })
        .collect()
}

fn cypher_written_edge_identity(
    kind: GraphMutationPlanKind,
    edge: &Edge,
) -> CypherWrittenEdgeIdentity {
    CypherWrittenEdgeIdentity {
        kind,
        from: edge.from.clone(),
        label: edge.label.clone(),
        to: edge.to.clone(),
        id: edge.id.clone(),
    }
}

fn cypher_written_node_identity(
    kind: GraphMutationPlanKind,
    node: &Node,
) -> CypherWrittenNodeIdentity {
    CypherWrittenNodeIdentity {
        kind,
        label: node.label.clone(),
        id: node.id.clone(),
    }
}

fn cypher_mutation_result_from_plan(
    report: CypherMutationReport,
    generated_node_ids: Vec<CypherGeneratedNodeId>,
    plan: &GraphMutationPlan,
    options: &CypherMutationOptions,
    row_edge_bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
) -> Result<CypherMutationResult> {
    let mut written_node_identities = Vec::new();
    let mut written_edge_identities = Vec::new();
    for operation in &plan.operations {
        if options.collect_written_node_identities
            && let GraphMutationPlanOp::UpsertNode { kind, node } = operation
        {
            written_node_identities.push(cypher_written_node_identity(*kind, node));
        }
        if options.collect_written_edge_identities {
            match operation {
                GraphMutationPlanOp::UpsertEdge { kind, edge } => {
                    written_edge_identities.push(cypher_written_edge_identity(*kind, edge));
                }
                GraphMutationPlanOp::UpsertEdgesFromNodeMatches { .. } => {}
                _ => {}
            }
        }
    }
    if options.collect_written_edge_identities {
        for (variable, binding) in row_edge_bindings {
            let Some(edges) = row_edge_values.get(variable) else {
                continue;
            };
            written_edge_identities.extend(
                edges
                    .iter()
                    .map(|edge| cypher_written_edge_identity(binding.kind, edge)),
            );
        }
    }
    Ok(CypherMutationResult {
        report,
        generated_node_ids,
        written_node_identities,
        written_edge_identities,
    })
}

/// Restricted row table produced by writable Cypher execution before `RETURN`
/// projection.
///
/// This model deliberately contains only variables whose values are already
/// owned by the write path:
///
/// - concrete node and relationship variables are tracked separately by the
///   resolved mutation plan;
/// - row-node variables come from broad `MATCH ... SET/REMOVE/DELETE` rows or
///   endpoint-aligned row-producing relationship writes;
/// - row-edge variables come from broad relationship mutations or
///   row-producing relationship writes.
/// - row-path variables are assembled from aligned row-node and row-edge
///   variables produced by one row-producing relationship write.
///
/// It is not a general Cypher read-query row model. Keeping this vocabulary
/// explicit prevents restricted writable `RETURN` from growing path or
/// arbitrary read-query semantics accidentally.
struct CypherWriteResultRows<'a> {
    row_nodes: &'a HashMap<String, Vec<Node>>,
    row_edges: &'a HashMap<String, Vec<Edge>>,
    row_paths: &'a HashMap<String, CypherRowProducedPathBinding>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CypherWriteResultBindingKind {
    RowNode,
    RowEdge,
    RowPath,
}

impl<'a> CypherWriteResultRows<'a> {
    fn new(
        row_nodes: &'a HashMap<String, Vec<Node>>,
        row_edges: &'a HashMap<String, Vec<Edge>>,
        row_paths: &'a HashMap<String, CypherRowProducedPathBinding>,
    ) -> Self {
        Self {
            row_nodes,
            row_edges,
            row_paths,
        }
    }

    fn row_nodes(&self, variable: &str) -> Option<&'a [Node]> {
        self.row_nodes.get(variable).map(Vec::as_slice)
    }

    fn row_edges(&self, variable: &str) -> Option<&'a [Edge]> {
        self.row_edges.get(variable).map(Vec::as_slice)
    }

    fn binding_kind(&self, variable: &str) -> Option<CypherWriteResultBindingKind> {
        if self.row_nodes.contains_key(variable) {
            Some(CypherWriteResultBindingKind::RowNode)
        } else if self.row_edges.contains_key(variable) {
            Some(CypherWriteResultBindingKind::RowEdge)
        } else if self.row_paths.contains_key(variable) {
            Some(CypherWriteResultBindingKind::RowPath)
        } else {
            None
        }
    }

    fn variable_names(&self) -> BTreeSet<String> {
        self.row_nodes
            .keys()
            .chain(self.row_edges.keys())
            .chain(self.row_paths.keys())
            .cloned()
            .collect()
    }

    fn row_count_for_return(&self, return_clause: &CypherReturnClause) -> Result<usize> {
        let mut row_count = None;
        for projection in &return_clause.projections {
            let count = match projection.element {
                CypherReturnElement::RowNode => self
                    .row_nodes(&projection.variable)
                    .map(<[Node]>::len)
                    .ok_or_else(|| {
                        cypher_unsupported_cardinality(format!(
                            "writable Cypher RETURN cannot materialize matched node variable '{}'",
                            projection.variable
                        ))
                    })?,
                CypherReturnElement::RowEdge => self
                    .row_edges(&projection.variable)
                    .map(<[Edge]>::len)
                    .ok_or_else(|| {
                        cypher_unsupported_cardinality(format!(
                            "writable Cypher RETURN cannot materialize row-producing relationship variable '{}'",
                            projection.variable
                        ))
                    })?,
                CypherReturnElement::RowPath => {
                    let path = self.row_paths.get(&projection.variable).ok_or_else(|| {
                        cypher_unsupported_cardinality(format!(
                            "writable Cypher RETURN cannot materialize path variable '{}'",
                            projection.variable
                        ))
                    })?;
                    self.row_edges(&path.edge_variable).map(<[Edge]>::len).unwrap_or(1)
                }
                CypherReturnElement::Node
                | CypherReturnElement::Edge
                | CypherReturnElement::Literal
                | CypherReturnElement::Aggregate => continue,
            };
            Self::merge_row_count(&mut row_count, count)?;
        }
        if let Some(count) = row_count {
            return Ok(count);
        }
        if let Some(count) = self.materialized_row_count()? {
            return Ok(count);
        }
        Ok(1)
    }

    fn materialized_row_count(&self) -> Result<Option<usize>> {
        let mut row_count = None;
        for count in self
            .row_nodes
            .values()
            .map(Vec::len)
            .chain(self.row_edges.values().map(Vec::len))
        {
            Self::merge_row_count(&mut row_count, count)?;
        }
        Ok(row_count)
    }

    fn merge_row_count(row_count: &mut Option<usize>, count: usize) -> Result<()> {
        if row_count.is_some_and(|current| current != count) {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN does not support mixing row-producing variables with different row counts",
            ));
        }
        *row_count = Some(count);
        Ok(())
    }
}

async fn evaluate_cypher_return_table<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    return_clause: &CypherReturnClause,
) -> Result<CypherResultTable>
where
    S: GraphStore + Sync,
{
    let columns = return_clause
        .projections
        .iter()
        .map(|projection| projection.column.clone())
        .collect::<Vec<_>>();
    let write_rows =
        CypherWriteResultRows::new(row_node_values, row_edge_values, row_path_bindings);
    let row_count = write_rows.row_count_for_return(return_clause)?;
    let mut nodes = HashMap::new();
    let mut edges = HashMap::new();
    if return_clause
        .projections
        .iter()
        .all(|projection| projection.element == CypherReturnElement::Aggregate)
    {
        let mut row = Vec::with_capacity(return_clause.projections.len());
        for projection in &return_clause.projections {
            let value = evaluate_return_aggregate(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                &mut nodes,
                &mut edges,
                projection,
                row_count,
            )
            .await?;
            row.push(value);
        }
        let mut rows = vec![row];
        if return_clause.distinct {
            apply_return_distinct(&mut rows)?;
        }
        apply_return_control(
            &mut rows,
            &return_clause.order_by,
            return_clause.skip,
            return_clause.limit,
        );
        return Ok(CypherResultTable { columns, rows });
    }
    if return_clause
        .projections
        .iter()
        .any(|projection| projection.element == CypherReturnElement::Aggregate)
    {
        return evaluate_grouped_cypher_return_table(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            return_clause,
            columns,
            row_count,
            &mut nodes,
            &mut edges,
        )
        .await;
    }
    let mut rows = Vec::with_capacity(row_count);
    for row_index in 0..row_count {
        let mut row = Vec::with_capacity(return_clause.projections.len());
        for projection in &return_clause.projections {
            row.push(
                evaluate_scalar_return_projection(
                    store,
                    node_bindings,
                    edge_bindings,
                    row_node_values,
                    row_edge_values,
                    row_path_bindings,
                    &mut nodes,
                    &mut edges,
                    projection,
                    row_index,
                )
                .await?,
            );
        }
        rows.push(row);
    }
    if return_clause.distinct {
        apply_return_distinct(&mut rows)?;
    }
    apply_return_control(
        &mut rows,
        &return_clause.order_by,
        return_clause.skip,
        return_clause.limit,
    );
    Ok(CypherResultTable { columns, rows })
}

struct CypherReturnGroup {
    scalar_values: Vec<Value>,
    aggregate_states: Vec<Option<CypherGroupedAggregateState>>,
}

enum CypherGroupedAggregateState {
    Count {
        count: usize,
        distinct: Option<BTreeSet<String>>,
    },
    Values {
        aggregate: CypherReturnAggregate,
        values: Vec<Value>,
        distinct: bool,
    },
}

impl CypherGroupedAggregateState {
    fn new(projection: &CypherReturnProjection) -> Result<Self> {
        let aggregate = projection.aggregate.ok_or_else(|| {
            cypher_unsupported_cardinality("RETURN aggregate projection is missing aggregate kind")
        })?;
        Ok(match aggregate {
            CypherReturnAggregate::Count => CypherGroupedAggregateState::Count {
                count: 0,
                distinct: projection.distinct.then(BTreeSet::new),
            },
            CypherReturnAggregate::Sum
            | CypherReturnAggregate::Avg
            | CypherReturnAggregate::Min
            | CypherReturnAggregate::Max
            | CypherReturnAggregate::Collect => CypherGroupedAggregateState::Values {
                aggregate,
                values: Vec::new(),
                distinct: projection.distinct,
            },
        })
    }

    fn record(&mut self, values: Vec<Value>) -> Result<()> {
        match self {
            CypherGroupedAggregateState::Count { count, distinct } => {
                if let Some(seen) = distinct {
                    for value in values {
                        let key = serde_json::to_string(&value.to_json()).map_err(|err| {
                            GrustError::CypherExecution(format!(
                                "RETURN COUNT(DISTINCT) value serialization failed: {err}"
                            ))
                        })?;
                        if seen.insert(key) {
                            *count += 1;
                        }
                    }
                } else {
                    *count += values.len();
                }
            }
            CypherGroupedAggregateState::Values {
                values: aggregate_values,
                ..
            } => {
                aggregate_values.extend(values);
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<Value> {
        match self {
            CypherGroupedAggregateState::Count { count, .. } => count_value(count),
            CypherGroupedAggregateState::Values {
                aggregate,
                mut values,
                distinct,
            } => {
                if distinct {
                    values = distinct_return_values(values)?;
                }
                evaluate_non_count_aggregate(aggregate, values)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn evaluate_grouped_cypher_return_table<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    return_clause: &CypherReturnClause,
    columns: Vec<String>,
    row_count: usize,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
) -> Result<CypherResultTable>
where
    S: GraphStore + Sync,
{
    let scalar_indices = return_clause
        .projections
        .iter()
        .enumerate()
        .filter_map(|(index, projection)| {
            (projection.element != CypherReturnElement::Aggregate).then_some(index)
        })
        .collect::<Vec<_>>();
    if scalar_indices.is_empty() {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher grouped RETURN requires at least one scalar projection",
        ));
    }

    let mut group_indexes = BTreeMap::new();
    let mut groups = Vec::<CypherReturnGroup>::new();
    for row_index in 0..row_count {
        let mut scalar_values = Vec::with_capacity(scalar_indices.len());
        for index in &scalar_indices {
            scalar_values.push(
                evaluate_scalar_return_projection(
                    store,
                    node_bindings,
                    edge_bindings,
                    row_node_values,
                    row_edge_values,
                    row_path_bindings,
                    nodes,
                    edges,
                    &return_clause.projections[*index],
                    row_index,
                )
                .await?,
            );
        }
        let group_key = return_row_key(&scalar_values, "RETURN grouping")?;
        let group_index = if let Some(group_index) = group_indexes.get(&group_key).copied() {
            group_index
        } else {
            let group_index = groups.len();
            group_indexes.insert(group_key, group_index);
            let aggregate_states = return_clause
                .projections
                .iter()
                .map(|projection| {
                    if projection.element == CypherReturnElement::Aggregate {
                        CypherGroupedAggregateState::new(projection).map(Some)
                    } else {
                        Ok(None)
                    }
                })
                .collect::<Result<Vec<_>>>()?;
            groups.push(CypherReturnGroup {
                scalar_values,
                aggregate_states,
            });
            group_index
        };

        for (projection_index, projection) in return_clause.projections.iter().enumerate() {
            if projection.element != CypherReturnElement::Aggregate {
                continue;
            }
            let values = materialize_return_aggregate_row_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await?;
            let Some(state) = groups[group_index].aggregate_states[projection_index].as_mut()
            else {
                continue;
            };
            state.record(values)?;
        }
    }

    let mut rows = Vec::with_capacity(groups.len());
    for group in groups {
        let CypherReturnGroup {
            scalar_values,
            aggregate_states,
        } = group;
        let mut aggregate_states = aggregate_states.into_iter();
        let mut scalar_cursor = 0usize;
        let mut row = Vec::with_capacity(return_clause.projections.len());
        for projection in &return_clause.projections {
            if projection.element == CypherReturnElement::Aggregate {
                let state = aggregate_states.next().flatten().ok_or_else(|| {
                    cypher_unsupported_cardinality(
                        "RETURN grouped aggregate state is missing for projection",
                    )
                })?;
                row.push(state.finish()?);
            } else {
                let _ = aggregate_states.next();
                let value = scalar_values.get(scalar_cursor).cloned().ok_or_else(|| {
                    cypher_unsupported_cardinality("RETURN grouped scalar state is missing")
                })?;
                scalar_cursor += 1;
                row.push(value);
            }
        }
        rows.push(row);
    }
    if return_clause.distinct {
        apply_return_distinct(&mut rows)?;
    }
    apply_return_control(
        &mut rows,
        &return_clause.order_by,
        return_clause.skip,
        return_clause.limit,
    );
    Ok(CypherResultTable { columns, rows })
}

#[allow(clippy::too_many_arguments)]
async fn evaluate_scalar_return_projection<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match &projection.target {
        CypherReturnTarget::All => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN * is only supported inside COUNT(*) or COLLECT(*)",
        )),
        CypherReturnTarget::Element => {
            materialize_return_element_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await
        }
        CypherReturnTarget::Literal(value) => Ok(value.clone()),
        CypherReturnTarget::Property(key) => {
            materialize_return_property_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await
        }
        CypherReturnTarget::MapProjection(keys) => {
            materialize_return_map_projection_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                keys,
                row_index,
            )
            .await
        }
        CypherReturnTarget::ListProjection(keys) => {
            materialize_return_list_projection_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                keys,
                row_index,
            )
            .await
        }
        CypherReturnTarget::Case(case) => {
            materialize_return_case_projection_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                case,
                row_index,
            )
            .await
        }
        CypherReturnTarget::Coalesce(coalesce) => {
            materialize_return_coalesce_projection_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                coalesce,
                row_index,
            )
            .await
        }
        CypherReturnTarget::PropertyExists(key) => {
            materialize_return_property_exists_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await
        }
        CypherReturnTarget::PropertySize(key) => {
            materialize_return_property_size_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await
        }
        CypherReturnTarget::PropertyAbs(key) => {
            materialize_return_property_abs_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await
        }
        CypherReturnTarget::PropertyNumericRound { key, round } => {
            materialize_return_property_numeric_round_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                *round,
                row_index,
            )
            .await
        }
        CypherReturnTarget::PropertyToString(key) => {
            materialize_return_property_to_string_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await
        }
        CypherReturnTarget::PropertyStringTransform { key, transform } => {
            materialize_return_property_string_transform_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                *transform,
                row_index,
            )
            .await
        }
        CypherReturnTarget::PropertyStringTrim { key, trim } => {
            materialize_return_property_string_trim_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                *trim,
                row_index,
            )
            .await
        }
        CypherReturnTarget::PropertyIsEmpty(key) => {
            materialize_return_property_is_empty_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await
        }
        CypherReturnTarget::PropertyStringReverse(key) => {
            materialize_return_property_string_reverse_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await
        }
        CypherReturnTarget::PropertyStringSplit(split) => {
            materialize_return_property_string_split_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                split,
                row_index,
            )
            .await
        }
        CypherReturnTarget::PropertySubstring(substring) => {
            materialize_return_property_substring_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                substring,
                row_index,
            )
            .await
        }
        CypherReturnTarget::PropertyStringSlice(slice) => {
            materialize_return_property_string_slice_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                slice,
                row_index,
            )
            .await
        }
        CypherReturnTarget::PropertyReplace(replace) => {
            materialize_return_property_replace_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                replace,
                row_index,
            )
            .await
        }
        CypherReturnTarget::PropertyStringPredicate(predicate) => {
            materialize_return_property_string_predicate_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                predicate,
                row_index,
            )
            .await
        }
        CypherReturnTarget::NodeLabels
        | CypherReturnTarget::RelationshipType
        | CypherReturnTarget::ElementProperties
        | CypherReturnTarget::ElementKeys
        | CypherReturnTarget::ElementId
        | CypherReturnTarget::RelationshipStartNode
        | CypherReturnTarget::RelationshipEndNode => {
            materialize_return_element_function_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await
        }
        CypherReturnTarget::PathLength
        | CypherReturnTarget::PathNodes
        | CypherReturnTarget::PathRelationships => {
            materialize_return_path_function_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_aggregate_row_values<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_index: usize,
) -> Result<Vec<Value>>
where
    S: GraphStore + Sync,
{
    let aggregate = projection.aggregate.ok_or_else(|| {
        cypher_unsupported_cardinality("RETURN aggregate projection is missing aggregate kind")
    })?;
    match &projection.target {
        CypherReturnTarget::All
            if aggregate == CypherReturnAggregate::Count && !projection.distinct =>
        {
            Ok(vec![Value::Int(1)])
        }
        CypherReturnTarget::All if aggregate == CypherReturnAggregate::Collect => {
            materialize_return_star_row_value(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                row_index,
            )
            .await
            .map(|value| vec![value])
        }
        CypherReturnTarget::All => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN aggregates other than COUNT and COLLECT require variable.property",
        )),
        CypherReturnTarget::Element
            if aggregate == CypherReturnAggregate::Count && !projection.distinct =>
        {
            Ok(vec![Value::Int(1)])
        }
        CypherReturnTarget::Element
            if aggregate == CypherReturnAggregate::Count
                || aggregate == CypherReturnAggregate::Collect =>
        {
            materialize_return_element_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await
            .map(|value| vec![value])
        }
        CypherReturnTarget::Element => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN aggregates other than COUNT and COLLECT require variable.property",
        )),
        CypherReturnTarget::Literal(value) => {
            Ok(non_null_return_value(value.clone()).into_iter().collect())
        }
        CypherReturnTarget::Property(key) => {
            let value = materialize_return_property_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::Case(case) => {
            let value = materialize_return_case_projection_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                case,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::Coalesce(coalesce) => {
            let value = materialize_return_coalesce_projection_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                coalesce,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::PropertyExists(key) => {
            let value = materialize_return_property_exists_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::PropertySize(key) => {
            let value = materialize_return_property_size_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::PropertyAbs(key) => {
            let value = materialize_return_property_abs_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::PropertyNumericRound { key, round } => {
            let value = materialize_return_property_numeric_round_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                *round,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::PropertyToString(key) => {
            let value = materialize_return_property_to_string_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::PropertyStringTransform { key, transform } => {
            let value = materialize_return_property_string_transform_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                *transform,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::PropertyStringTrim { key, trim } => {
            let value = materialize_return_property_string_trim_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                *trim,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::PropertyIsEmpty(key) => {
            let value = materialize_return_property_is_empty_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::PropertyStringReverse(key) => {
            let value = materialize_return_property_string_reverse_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::PropertyStringSplit(split) => {
            let value = materialize_return_property_string_split_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                split,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::PropertySubstring(substring) => {
            let value = materialize_return_property_substring_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                substring,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::PropertyStringSlice(slice) => {
            let value = materialize_return_property_string_slice_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                slice,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::PropertyReplace(replace) => {
            let value = materialize_return_property_replace_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                replace,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::PropertyStringPredicate(predicate) => {
            let value = materialize_return_property_string_predicate_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                predicate,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::NodeLabels
        | CypherReturnTarget::RelationshipType
        | CypherReturnTarget::ElementProperties
        | CypherReturnTarget::ElementKeys
        | CypherReturnTarget::ElementId
        | CypherReturnTarget::RelationshipStartNode
        | CypherReturnTarget::RelationshipEndNode => {
            let value = materialize_return_element_function_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::PathLength
        | CypherReturnTarget::PathNodes
        | CypherReturnTarget::PathRelationships => {
            let value = materialize_return_path_function_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTarget::MapProjection(_) | CypherReturnTarget::ListProjection(_) => {
            let value = evaluate_scalar_return_projection(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_element_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match projection.element {
        CypherReturnElement::Node => {
            let id = node_bindings.get(&projection.variable).ok_or_else(|| {
                cypher_unresolved_identity(format!(
                    "RETURN references variable '{}' that is not bound by the write plan",
                    projection.variable
                ))
            })?;
            let node = resolve_bound_node(store, nodes, &projection.variable, id).await?;
            graph_node_value(node)
        }
        CypherReturnElement::Edge => {
            let identity = edge_bindings.get(&projection.variable).ok_or_else(|| {
                cypher_unresolved_identity(format!(
                    "RETURN references relationship variable '{}' that is not bound by the write plan",
                    projection.variable
                ))
            })?;
            let edge =
                resolve_bound_edge_cached(store, edges, identity, &projection.variable).await?;
            graph_edge_value(edge)
        }
        CypherReturnElement::RowNode => {
            let node = row_node_values
                .get(&projection.variable)
                .and_then(|nodes| nodes.get(row_index))
                .ok_or_else(|| {
                    cypher_unsupported_cardinality(format!(
                        "writable Cypher RETURN cannot materialize matched node variable '{}'",
                        projection.variable
                    ))
                })?;
            graph_node_value(node)
        }
        CypherReturnElement::RowEdge => {
            let edge = row_edge_values
                .get(&projection.variable)
                .and_then(|edges| edges.get(row_index))
                .ok_or_else(|| {
                    cypher_unsupported_cardinality(format!(
                        "writable Cypher RETURN cannot materialize row-producing relationship variable '{}'",
                        projection.variable
                    ))
                })?;
            graph_edge_value(edge)
        }
        CypherReturnElement::RowPath => {
            materialize_return_path_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                &projection.variable,
                row_index,
            )
            .await
        }
        CypherReturnElement::Literal => match &projection.target {
            CypherReturnTarget::Literal(value) => Ok(value.clone()),
            _ => Err(cypher_unsupported_cardinality(
                "RETURN literal element received a non-literal projection",
            )),
        },
        CypherReturnElement::Aggregate => {
            match (
                node_bindings.contains_key(&projection.variable),
                edge_bindings.contains_key(&projection.variable),
                row_node_values.contains_key(&projection.variable),
                row_edge_values.contains_key(&projection.variable),
                row_path_bindings.contains_key(&projection.variable),
            ) {
                (true, false, false, false, false) => {
                    let id = node_bindings
                        .get(&projection.variable)
                        .expect("checked binding");
                    let node = resolve_bound_node(store, nodes, &projection.variable, id).await?;
                    graph_node_value(node)
                }
                (false, true, false, false, false) => {
                    let identity = edge_bindings
                        .get(&projection.variable)
                        .expect("checked binding");
                    let edge =
                        resolve_bound_edge_cached(store, edges, identity, &projection.variable)
                            .await?;
                    graph_edge_value(edge)
                }
                (false, false, true, false, false) => {
                    let node = row_node_values
                        .get(&projection.variable)
                        .and_then(|nodes| nodes.get(row_index))
                        .ok_or_else(|| {
                            cypher_unsupported_cardinality(format!(
                                "writable Cypher RETURN cannot materialize matched node variable '{}'",
                                projection.variable
                            ))
                        })?;
                    graph_node_value(node)
                }
                (false, false, false, true, false) => {
                    let edge = row_edge_values
                        .get(&projection.variable)
                        .and_then(|edges| edges.get(row_index))
                        .ok_or_else(|| {
                            cypher_unsupported_cardinality(format!(
                                "writable Cypher RETURN cannot materialize row-producing relationship variable '{}'",
                                projection.variable
                            ))
                        })?;
                    graph_edge_value(edge)
                }
                (false, false, false, false, true) => {
                    materialize_return_path_value_at(
                        store,
                        node_bindings,
                        edge_bindings,
                        row_node_values,
                        row_edge_values,
                        row_path_bindings,
                        nodes,
                        edges,
                        &projection.variable,
                        row_index,
                    )
                    .await
                }
                _ => Err(cypher_unresolved_identity(format!(
                    "RETURN references variable '{}' that is not bound by the write plan",
                    projection.variable
                ))),
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_path_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    variable: &str,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let path = row_path_bindings.get(variable).ok_or_else(|| {
        cypher_unsupported_cardinality(format!(
            "writable Cypher RETURN cannot materialize path variable '{variable}'"
        ))
    })?;
    let from = materialize_return_path_endpoint_node(
        store,
        node_bindings,
        row_node_values,
        nodes,
        &path.from_variable,
        row_index,
    )
    .await?;
    let from_value = graph_node_value(from)?.to_json();
    let edge = if let Some(edge) = row_edge_values
        .get(&path.edge_variable)
        .and_then(|edges| edges.get(row_index))
    {
        edge
    } else if let Some(identity) = edge_bindings.get(&path.edge_variable) {
        resolve_bound_edge_cached(store, edges, identity, &path.edge_variable).await?
    } else {
        return Err(cypher_unsupported_cardinality(format!(
            "writable Cypher RETURN cannot materialize path variable '{variable}'"
        )));
    };
    let to = materialize_return_path_endpoint_node(
        store,
        node_bindings,
        row_node_values,
        nodes,
        &path.to_variable,
        row_index,
    )
    .await?;
    let to_value = graph_node_value(to)?.to_json();
    let edge_value = graph_edge_value(edge)?.to_json();
    Ok(Value::Json(serde_json::json!({
        "nodes": [from_value, to_value],
        "relationships": [edge_value],
    })))
}

async fn materialize_return_path_endpoint_node<'a, S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    row_node_values: &'a HashMap<String, Vec<Node>>,
    nodes: &'a mut HashMap<String, Node>,
    variable: &str,
    row_index: usize,
) -> Result<&'a Node>
where
    S: GraphStore + Sync,
{
    if let Some(id) = node_bindings.get(variable) {
        return resolve_bound_node(store, nodes, variable, id).await;
    }
    row_node_values
        .get(variable)
        .and_then(|nodes| nodes.get(row_index))
        .ok_or_else(|| {
            cypher_unsupported_cardinality(format!(
                "writable Cypher RETURN cannot materialize path endpoint variable '{variable}'"
            ))
        })
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_map_projection_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    keys: &[String],
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let mut map = serde_json::Map::new();
    for key in keys {
        let value = materialize_return_property_value_at(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            projection,
            key,
            row_index,
        )
        .await?;
        map.insert(key.clone(), value.to_json());
    }
    Ok(Value::Json(serde_json::Value::Object(map)))
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_list_projection_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    keys: &[String],
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let mut values = Vec::with_capacity(keys.len());
    for key in keys {
        let value = materialize_return_property_value_at(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            projection,
            key,
            row_index,
        )
        .await?;
        values.push(value.to_json());
    }
    Ok(Value::Json(serde_json::Value::Array(values)))
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_case_projection_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    case: &CypherReturnCase,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let actual = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        &case.key,
        row_index,
    )
    .await?;
    if actual == case.equals {
        Ok(case.then_value.clone())
    } else {
        Ok(case.else_value.clone())
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_case_values<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    case: &CypherReturnCase,
    row_count: usize,
) -> Result<Vec<Value>>
where
    S: GraphStore + Sync,
{
    let mut values = Vec::with_capacity(row_count);
    for row_index in 0..row_count {
        let value = materialize_return_case_projection_value_at(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            projection,
            case,
            row_index,
        )
        .await?;
        if let Some(value) = non_null_return_value(value) {
            values.push(value);
        }
    }
    Ok(values)
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_coalesce_projection_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    coalesce: &CypherReturnCoalesce,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    for term in &coalesce.terms {
        let value = match term {
            CypherReturnCoalesceTerm::Property(key) => {
                materialize_return_property_value_at(
                    store,
                    node_bindings,
                    edge_bindings,
                    row_node_values,
                    row_edge_values,
                    row_path_bindings,
                    nodes,
                    edges,
                    projection,
                    key,
                    row_index,
                )
                .await?
            }
            CypherReturnCoalesceTerm::Literal(value) => value.clone(),
        };
        if value != Value::Null {
            return Ok(value);
        }
    }
    Ok(Value::Null)
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_exists_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    key: &str,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        key,
        row_index,
    )
    .await?;
    Ok(Value::Bool(value != Value::Null))
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_size_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    key: &str,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        key,
        row_index,
    )
    .await?;
    restricted_size_value(value)
}

fn restricted_size_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::Int(value.chars().count() as i64)),
        Value::DateTime(value) => Ok(Value::Int(value.as_str().chars().count() as i64)),
        Value::StringArray(values) => Ok(Value::Int(values.len() as i64)),
        Value::IntArray(values) => Ok(Value::Int(values.len() as i64)),
        Value::FloatArray(values) => Ok(Value::Int(values.len() as i64)),
        Value::Json(serde_json::Value::String(value)) => {
            Ok(Value::Int(value.chars().count() as i64))
        }
        Value::Json(serde_json::Value::Array(values)) => Ok(Value::Int(values.len() as i64)),
        Value::Json(serde_json::Value::Object(values)) => Ok(Value::Int(values.len() as i64)),
        Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Json(_) => {
            Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN size only supports string, array, or JSON collection values",
            ))
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_abs_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    key: &str,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        key,
        row_index,
    )
    .await?;
    restricted_abs_value(value)
}

fn restricted_abs_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Int(value) => value
            .checked_abs()
            .map(Value::Int)
            .ok_or_else(|| GrustError::CypherExecution("RETURN abs integer overflow".to_string())),
        Value::Float(value) => {
            let value = value.abs();
            if value.is_finite() {
                Ok(Value::Float(value))
            } else {
                Err(GrustError::CypherExecution(
                    "RETURN abs produced non-finite float".to_string(),
                ))
            }
        }
        Value::Json(serde_json::Value::Number(value)) => {
            if let Some(value) = value.as_i64() {
                restricted_abs_value(Value::Int(value))
            } else if let Some(value) = value.as_f64() {
                restricted_abs_value(Value::Float(value))
            } else {
                Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN abs only supports finite numeric values",
                ))
            }
        }
        Value::Bool(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN abs only supports numeric values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_numeric_round_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    key: &str,
    round: CypherReturnNumericRound,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        key,
        row_index,
    )
    .await?;
    restricted_numeric_round_value(value, round)
}

fn restricted_numeric_round_value(value: Value, round: CypherReturnNumericRound) -> Result<Value> {
    let round_float = |value: f64| match round {
        CypherReturnNumericRound::Ceil => value.ceil(),
        CypherReturnNumericRound::Floor => value.floor(),
    };
    match value {
        Value::Null => Ok(Value::Null),
        Value::Int(value) => Ok(Value::Int(value)),
        Value::Float(value) => {
            let value = round_float(value);
            if value.is_finite() {
                Ok(Value::Float(value))
            } else {
                Err(GrustError::CypherExecution(
                    "RETURN ceil/floor produced non-finite float".to_string(),
                ))
            }
        }
        Value::Json(serde_json::Value::Number(value)) => {
            if let Some(value) = value.as_i64() {
                Ok(Value::Int(value))
            } else if let Some(value) = value.as_f64() {
                restricted_numeric_round_value(Value::Float(value), round)
            } else {
                Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN ceil/floor only supports finite numeric values",
                ))
            }
        }
        Value::Bool(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN ceil/floor only supports numeric values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_to_string_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    key: &str,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        key,
        row_index,
    )
    .await?;
    restricted_to_string_value(value)
}

fn restricted_to_string_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Bool(value) => Ok(Value::from(value.to_string())),
        Value::Int(value) => Ok(Value::from(value.to_string())),
        Value::Float(value) => Ok(Value::from(value.to_string())),
        Value::String(value) => Ok(Value::from(value)),
        Value::DateTime(value) => Ok(Value::from(value.as_str().to_string())),
        Value::Json(serde_json::Value::Null) => Ok(Value::Null),
        Value::Json(serde_json::Value::Bool(value)) => Ok(Value::from(value.to_string())),
        Value::Json(serde_json::Value::Number(value)) => Ok(Value::from(value.to_string())),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::from(value)),
        Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(serde_json::Value::Array(_))
        | Value::Json(serde_json::Value::Object(_)) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toString only supports scalar values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_is_empty_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    key: &str,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        key,
        row_index,
    )
    .await?;
    restricted_is_empty_value(value)
}

fn restricted_is_empty_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::Bool(value.is_empty())),
        Value::DateTime(value) => Ok(Value::Bool(value.as_str().is_empty())),
        Value::StringArray(values) => Ok(Value::Bool(values.is_empty())),
        Value::IntArray(values) => Ok(Value::Bool(values.is_empty())),
        Value::FloatArray(values) => Ok(Value::Bool(values.is_empty())),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::Bool(value.is_empty())),
        Value::Json(serde_json::Value::Array(values)) => Ok(Value::Bool(values.is_empty())),
        Value::Json(serde_json::Value::Object(values)) => Ok(Value::Bool(values.is_empty())),
        Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Json(_) => {
            Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN isEmpty only supports string, array, or JSON collection values",
            ))
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_string_transform_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    key: &str,
    transform: CypherReturnStringTransform,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        key,
        row_index,
    )
    .await?;
    restricted_string_transform_value(value, transform)
}

fn restricted_string_transform_value(
    value: Value,
    transform: CypherReturnStringTransform,
) -> Result<Value> {
    let transform_value = |value: String| match transform {
        CypherReturnStringTransform::Lower => value.to_lowercase(),
        CypherReturnStringTransform::Upper => value.to_uppercase(),
    };
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::from(transform_value(value))),
        Value::DateTime(value) => Ok(Value::from(transform_value(value.as_str().to_string()))),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::from(transform_value(value))),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN string transforms only support string values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_string_trim_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    key: &str,
    trim: CypherReturnStringTrim,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        key,
        row_index,
    )
    .await?;
    restricted_string_trim_value(value, trim)
}

fn restricted_string_trim_value(value: Value, trim: CypherReturnStringTrim) -> Result<Value> {
    let trim_value = |value: String| match trim {
        CypherReturnStringTrim::Both => value.trim().to_string(),
        CypherReturnStringTrim::Left => value.trim_start().to_string(),
        CypherReturnStringTrim::Right => value.trim_end().to_string(),
    };
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::from(trim_value(value))),
        Value::DateTime(value) => Ok(Value::from(trim_value(value.as_str().to_string()))),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::from(trim_value(value))),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN string trims only support string values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_string_reverse_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    key: &str,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        key,
        row_index,
    )
    .await?;
    restricted_string_reverse_value(value)
}

fn restricted_string_reverse_value(value: Value) -> Result<Value> {
    let reverse_value = |value: String| value.chars().rev().collect::<String>();
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::from(reverse_value(value))),
        Value::DateTime(value) => Ok(Value::from(reverse_value(value.as_str().to_string()))),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::from(reverse_value(value))),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN reverse only supports string values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_string_split_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    split: &CypherReturnStringSplit,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        &split.key,
        row_index,
    )
    .await?;
    restricted_string_split_value(value, split)
}

fn restricted_string_split_value(value: Value, split: &CypherReturnStringSplit) -> Result<Value> {
    let split_value = |value: String| {
        Value::Json(serde_json::Value::Array(
            value
                .split(&split.delimiter)
                .map(|part| serde_json::Value::String(part.to_string()))
                .collect(),
        ))
    };
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(split_value(value)),
        Value::DateTime(value) => Ok(split_value(value.as_str().to_string())),
        Value::Json(serde_json::Value::String(value)) => Ok(split_value(value)),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN split only supports string values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_substring_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    substring: &CypherReturnSubstring,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        &substring.key,
        row_index,
    )
    .await?;
    restricted_substring_value(value, substring)
}

fn restricted_substring_value(value: Value, substring: &CypherReturnSubstring) -> Result<Value> {
    let slice_value = |value: String| -> String {
        let chars = value.chars().skip(substring.start);
        match substring.length {
            Some(length) => chars.take(length).collect(),
            None => chars.collect(),
        }
    };
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::from(slice_value(value))),
        Value::DateTime(value) => Ok(Value::from(slice_value(value.as_str().to_string()))),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::from(slice_value(value))),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN substring only supports string values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_string_slice_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    slice: &CypherReturnStringSlice,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        &slice.key,
        row_index,
    )
    .await?;
    restricted_string_slice_value(value, slice)
}

fn restricted_string_slice_value(value: Value, slice: &CypherReturnStringSlice) -> Result<Value> {
    let slice_value = |value: String| -> String {
        match slice.side {
            CypherReturnStringSliceSide::Left => value.chars().take(slice.length).collect(),
            CypherReturnStringSliceSide::Right => {
                let chars: Vec<_> = value.chars().collect();
                let start = chars.len().saturating_sub(slice.length);
                chars.into_iter().skip(start).collect()
            }
        }
    };
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::from(slice_value(value))),
        Value::DateTime(value) => Ok(Value::from(slice_value(value.as_str().to_string()))),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::from(slice_value(value))),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN left/right only supports string values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_replace_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    replace: &CypherReturnReplace,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        &replace.key,
        row_index,
    )
    .await?;
    restricted_replace_value(value, replace)
}

fn restricted_replace_value(value: Value, replace: &CypherReturnReplace) -> Result<Value> {
    let replace_value = |value: String| value.replace(&replace.search, &replace.replacement);
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::from(replace_value(value))),
        Value::DateTime(value) => Ok(Value::from(replace_value(value.as_str().to_string()))),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::from(replace_value(value))),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN replace only supports string values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_string_predicate_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    predicate: &CypherReturnStringPredicateProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        &predicate.key,
        row_index,
    )
    .await?;
    restricted_string_predicate_value(value, predicate)
}

fn restricted_string_predicate_value(
    value: Value,
    predicate: &CypherReturnStringPredicateProjection,
) -> Result<Value> {
    let predicate_value = |value: String| match predicate.predicate {
        CypherReturnStringPredicate::StartsWith => value.starts_with(&predicate.needle),
        CypherReturnStringPredicate::EndsWith => value.ends_with(&predicate.needle),
        CypherReturnStringPredicate::Contains => value.contains(&predicate.needle),
    };
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::Bool(predicate_value(value))),
        Value::DateTime(value) => Ok(Value::Bool(predicate_value(value.as_str().to_string()))),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::Bool(predicate_value(value))),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN string predicates only support string values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_element_function_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match projection.target {
        CypherReturnTarget::NodeLabels => match projection.element {
            CypherReturnElement::Node
            | CypherReturnElement::RowNode
            | CypherReturnElement::Aggregate => {
                let label = materialize_return_property_value_at(
                    store,
                    node_bindings,
                    edge_bindings,
                    row_node_values,
                    row_edge_values,
                    row_path_bindings,
                    nodes,
                    edges,
                    projection,
                    "label",
                    row_index,
                )
                .await?;
                let Value::String(label) = label else {
                    return Err(GrustError::CypherExecution(
                        "RETURN labels(...) could not materialize node label".to_string(),
                    ));
                };
                Ok(Value::Json(serde_json::json!([label])))
            }
            _ => Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN labels(...) requires a bound node variable",
            )),
        },
        CypherReturnTarget::RelationshipType => match projection.element {
            CypherReturnElement::Edge
            | CypherReturnElement::RowEdge
            | CypherReturnElement::Aggregate => {
                materialize_return_property_value_at(
                    store,
                    node_bindings,
                    edge_bindings,
                    row_node_values,
                    row_edge_values,
                    row_path_bindings,
                    nodes,
                    edges,
                    projection,
                    "label",
                    row_index,
                )
                .await
            }
            _ => Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN type(...) requires a bound relationship variable",
            )),
        },
        CypherReturnTarget::ElementProperties => {
            let props = materialize_return_element_props_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await?;
            Ok(props_value(&props))
        }
        CypherReturnTarget::ElementKeys => {
            let props = materialize_return_element_props_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await?;
            Ok(Value::Json(serde_json::Value::Array(
                props
                    .keys()
                    .map(|key| serde_json::Value::String(key.clone()))
                    .collect(),
            )))
        }
        CypherReturnTarget::ElementId => {
            materialize_return_property_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                "id",
                row_index,
            )
            .await
        }
        CypherReturnTarget::RelationshipStartNode | CypherReturnTarget::RelationshipEndNode => {
            let edge = materialize_return_relationship_edge_at(
                store,
                edge_bindings,
                row_edge_values,
                edges,
                projection,
                row_index,
            )
            .await?;
            let id = match projection.target {
                CypherReturnTarget::RelationshipStartNode => &edge.from,
                CypherReturnTarget::RelationshipEndNode => &edge.to,
                _ => unreachable!("checked relationship endpoint target"),
            };
            let node = store.get_node(id).await?.ok_or_else(|| {
                GrustError::CypherExecution(format!(
                    "RETURN relationship endpoint '{}' does not exist after the write",
                    id.as_str()
                ))
            })?;
            graph_node_value(&node)
        }
        _ => Err(cypher_unsupported_cardinality(
            "RETURN element function materializer received a non-element-function projection",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_element_props_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_index: usize,
) -> Result<Props>
where
    S: GraphStore + Sync,
{
    if let Some(id) = node_bindings.get(&projection.variable) {
        let node = resolve_bound_node(store, nodes, &projection.variable, id).await?;
        return Ok(node.props.clone());
    }
    if let Some(identity) = edge_bindings.get(&projection.variable) {
        let edge = resolve_bound_edge_cached(store, edges, identity, &projection.variable).await?;
        return Ok(edge.props.clone());
    }
    if let Some(row_nodes) = row_node_values.get(&projection.variable) {
        let node = row_nodes.get(row_index).ok_or_else(|| {
            cypher_unsupported_cardinality(format!(
                "writable Cypher RETURN cannot materialize matched node variable '{}'",
                projection.variable
            ))
        })?;
        return Ok(node.props.clone());
    }
    if let Some(row_edges) = row_edge_values.get(&projection.variable) {
        let edge = row_edges.get(row_index).ok_or_else(|| {
            cypher_unsupported_cardinality(format!(
                "writable Cypher RETURN cannot materialize row-producing relationship variable '{}'",
                projection.variable
            ))
        })?;
        return Ok(edge.props.clone());
    }
    Err(cypher_unresolved_identity(format!(
        "RETURN references variable '{}' that is not bound by the write plan",
        projection.variable
    )))
}

async fn materialize_return_relationship_edge_at<S>(
    store: &S,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_index: usize,
) -> Result<Edge>
where
    S: GraphStore + Sync,
{
    if let Some(identity) = edge_bindings.get(&projection.variable) {
        return resolve_bound_edge_cached(store, edges, identity, &projection.variable)
            .await
            .cloned();
    }
    if let Some(row_edges) = row_edge_values.get(&projection.variable) {
        return row_edges.get(row_index).cloned().ok_or_else(|| {
            cypher_unsupported_cardinality(format!(
                "writable Cypher RETURN cannot materialize row-producing relationship variable '{}'",
                projection.variable
            ))
        });
    }
    Err(cypher_unsupported_cardinality(format!(
        "writable Cypher RETURN cannot materialize relationship variable '{}'",
        projection.variable
    )))
}

fn props_value(props: &Props) -> Value {
    Value::Json(serde_json::Value::Object(
        props
            .iter()
            .map(|(key, value)| (key.clone(), value.to_json()))
            .collect(),
    ))
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_projection_values<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_count: usize,
) -> Result<Vec<Value>>
where
    S: GraphStore + Sync,
{
    let mut values = Vec::with_capacity(row_count);
    for row_index in 0..row_count {
        let value = evaluate_scalar_return_projection(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            projection,
            row_index,
        )
        .await?;
        if let Some(value) = non_null_return_value(value) {
            values.push(value);
        }
    }
    Ok(values)
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    key: &str,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match projection.element {
        CypherReturnElement::Node => {
            let id = node_bindings.get(&projection.variable).ok_or_else(|| {
                cypher_unresolved_identity(format!(
                    "RETURN references variable '{}' that is not bound by the write plan",
                    projection.variable
                ))
            })?;
            if key == "id" {
                return Ok(Value::from(id.as_str()));
            }
            let node = resolve_bound_node(store, nodes, &projection.variable, id).await?;
            Ok(project_node_value(node, key))
        }
        CypherReturnElement::Edge => {
            let identity = edge_bindings.get(&projection.variable).ok_or_else(|| {
                cypher_unresolved_identity(format!(
                    "RETURN references relationship variable '{}' that is not bound by the write plan",
                    projection.variable
                ))
            })?;
            if key == "id" {
                return Ok(identity
                    .id
                    .as_ref()
                    .map(|id| Value::from(id.as_str()))
                    .unwrap_or(Value::Null));
            }
            if key == "label" {
                return Ok(Value::from(identity.label.as_str()));
            }
            let edge =
                resolve_bound_edge_cached(store, edges, identity, &projection.variable).await?;
            Ok(project_edge_value(edge, key))
        }
        CypherReturnElement::RowNode => {
            let node = row_node_values
                .get(&projection.variable)
                .and_then(|nodes| nodes.get(row_index))
                .ok_or_else(|| {
                    cypher_unsupported_cardinality(format!(
                        "writable Cypher RETURN cannot materialize matched node variable '{}'",
                        projection.variable
                    ))
                })?;
            if key == "id" {
                Ok(Value::from(node.id.as_str()))
            } else {
                Ok(project_node_value(node, key))
            }
        }
        CypherReturnElement::RowEdge => {
            let edge = row_edge_values
                .get(&projection.variable)
                .and_then(|edges| edges.get(row_index))
                .ok_or_else(|| {
                    cypher_unsupported_cardinality(format!(
                        "writable Cypher RETURN cannot materialize row-producing relationship variable '{}'",
                        projection.variable
                    ))
                })?;
            if key == "id" {
                Ok(edge
                    .id
                    .as_ref()
                    .map(|id| Value::from(id.as_str()))
                    .unwrap_or(Value::Null))
            } else if key == "label" {
                Ok(Value::from(edge.label.as_str()))
            } else {
                Ok(project_edge_value(edge, key))
            }
        }
        CypherReturnElement::RowPath => {
            let _ = row_path_bindings.get(&projection.variable).ok_or_else(|| {
                cypher_unsupported_cardinality(format!(
                    "writable Cypher RETURN cannot materialize path variable '{}'",
                    projection.variable
                ))
            })?;
            Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN path properties are not supported",
            ))
        }
        CypherReturnElement::Literal => match &projection.target {
            CypherReturnTarget::Literal(value) => Ok(value.clone()),
            _ => Err(cypher_unsupported_cardinality(
                "RETURN literal element received a non-literal projection",
            )),
        },
        CypherReturnElement::Aggregate => {
            match (
                node_bindings.contains_key(&projection.variable),
                edge_bindings.contains_key(&projection.variable),
                row_node_values.contains_key(&projection.variable),
                row_edge_values.contains_key(&projection.variable),
                row_path_bindings.contains_key(&projection.variable),
            ) {
                (true, false, false, false, false) => {
                    let id = node_bindings
                        .get(&projection.variable)
                        .expect("checked binding");
                    if key == "id" {
                        return Ok(Value::from(id.as_str()));
                    }
                    let node = resolve_bound_node(store, nodes, &projection.variable, id).await?;
                    Ok(project_node_value(node, key))
                }
                (false, true, false, false, false) => {
                    let identity = edge_bindings
                        .get(&projection.variable)
                        .expect("checked binding");
                    if key == "id" {
                        return Ok(identity
                            .id
                            .as_ref()
                            .map(|id| Value::from(id.as_str()))
                            .unwrap_or(Value::Null));
                    }
                    if key == "label" {
                        return Ok(Value::from(identity.label.as_str()));
                    }
                    let edge =
                        resolve_bound_edge_cached(store, edges, identity, &projection.variable)
                            .await?;
                    Ok(project_edge_value(edge, key))
                }
                (false, false, true, false, false) => {
                    let node = row_node_values
                        .get(&projection.variable)
                        .and_then(|nodes| nodes.get(row_index))
                        .ok_or_else(|| {
                            cypher_unsupported_cardinality(format!(
                                "writable Cypher RETURN cannot materialize matched node variable '{}'",
                                projection.variable
                            ))
                        })?;
                    if key == "id" {
                        Ok(Value::from(node.id.as_str()))
                    } else {
                        Ok(project_node_value(node, key))
                    }
                }
                (false, false, false, true, false) => {
                    let edge = row_edge_values
                        .get(&projection.variable)
                        .and_then(|edges| edges.get(row_index))
                        .ok_or_else(|| {
                            cypher_unsupported_cardinality(format!(
                                "writable Cypher RETURN cannot materialize row-producing relationship variable '{}'",
                                projection.variable
                            ))
                        })?;
                    if key == "id" {
                        Ok(edge
                            .id
                            .as_ref()
                            .map(|id| Value::from(id.as_str()))
                            .unwrap_or(Value::Null))
                    } else if key == "label" {
                        Ok(Value::from(edge.label.as_str()))
                    } else {
                        Ok(project_edge_value(edge, key))
                    }
                }
                (false, false, false, false, true) => Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN path properties are not supported",
                )),
                _ => Err(cypher_unresolved_identity(format!(
                    "RETURN references variable '{}' that is not bound by the write plan",
                    projection.variable
                ))),
            }
        }
    }
}

fn return_row_key(values: &[Value], context: &str) -> Result<String> {
    serde_json::to_string(
        &values
            .iter()
            .map(Value::to_json)
            .collect::<Vec<serde_json::Value>>(),
    )
    .map_err(|err| {
        GrustError::CypherExecution(format!("{context} key serialization failed: {err}"))
    })
}

async fn evaluate_return_aggregate<'a, S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &'a mut HashMap<String, Node>,
    edges: &'a mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_count: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let aggregate = projection.aggregate.ok_or_else(|| {
        cypher_unsupported_cardinality("RETURN aggregate projection is missing aggregate kind")
    })?;
    if aggregate == CypherReturnAggregate::Count {
        return count_return_projection(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            projection,
            row_count,
        )
        .await
        .and_then(count_value);
    }
    let values = materialize_return_aggregate_values(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        aggregate,
        row_count,
    )
    .await?;
    evaluate_non_count_aggregate(aggregate, values)
}

async fn count_return_projection<'a, S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &'a mut HashMap<String, Node>,
    edges: &'a mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_count: usize,
) -> Result<usize>
where
    S: GraphStore + Sync,
{
    if let CypherReturnTarget::Case(case) = &projection.target {
        let values = materialize_return_case_values(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            projection,
            case,
            row_count,
        )
        .await?;
        return if projection.distinct {
            Ok(distinct_return_values(values)?.len())
        } else {
            Ok(values.len())
        };
    }
    if matches!(
        projection.target,
        CypherReturnTarget::PathLength
            | CypherReturnTarget::PathNodes
            | CypherReturnTarget::PathRelationships
    ) {
        let values = materialize_return_path_function_values(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            projection,
            row_count,
        )
        .await?;
        return if projection.distinct {
            Ok(distinct_return_values(values)?.len())
        } else {
            Ok(values.len())
        };
    }
    if matches!(
        projection.target,
        CypherReturnTarget::MapProjection(_)
            | CypherReturnTarget::ListProjection(_)
            | CypherReturnTarget::Coalesce(_)
            | CypherReturnTarget::PropertyExists(_)
            | CypherReturnTarget::PropertySize(_)
            | CypherReturnTarget::PropertyAbs(_)
            | CypherReturnTarget::PropertyNumericRound { .. }
            | CypherReturnTarget::PropertyToString(_)
            | CypherReturnTarget::PropertyStringTransform { .. }
            | CypherReturnTarget::PropertyStringTrim { .. }
            | CypherReturnTarget::PropertyIsEmpty(_)
            | CypherReturnTarget::PropertyStringReverse(_)
            | CypherReturnTarget::PropertyStringSplit(_)
            | CypherReturnTarget::PropertySubstring(_)
            | CypherReturnTarget::PropertyStringSlice(_)
            | CypherReturnTarget::PropertyReplace(_)
            | CypherReturnTarget::PropertyStringPredicate(_)
            | CypherReturnTarget::NodeLabels
            | CypherReturnTarget::RelationshipType
            | CypherReturnTarget::ElementProperties
            | CypherReturnTarget::ElementKeys
            | CypherReturnTarget::ElementId
            | CypherReturnTarget::RelationshipStartNode
            | CypherReturnTarget::RelationshipEndNode
    ) {
        let values = materialize_return_projection_values(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            projection,
            row_count,
        )
        .await?;
        return if projection.distinct {
            Ok(distinct_return_values(values)?.len())
        } else {
            Ok(values.len())
        };
    }
    if let CypherReturnTarget::Literal(_) = projection.target {
        let values = materialize_return_projection_values(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            projection,
            row_count,
        )
        .await?;
        return if projection.distinct {
            Ok(distinct_return_values(values)?.len())
        } else {
            Ok(values.len())
        };
    }
    let CypherReturnTarget::Property(key) = &projection.target else {
        if projection.distinct {
            if row_path_bindings.contains_key(&projection.variable) {
                let values = materialize_return_path_values(
                    store,
                    node_bindings,
                    edge_bindings,
                    row_node_values,
                    row_edge_values,
                    row_path_bindings,
                    nodes,
                    edges,
                    &projection.variable,
                    row_count,
                )
                .await?;
                return Ok(distinct_return_values(values)?.len());
            }
            return count_distinct_elements(
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                projection,
                row_count,
            );
        }
        return Ok(row_count);
    };
    if row_path_bindings.contains_key(&projection.variable) {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN path properties are not supported",
        ));
    }
    if let Some(id) = node_bindings.get(&projection.variable) {
        if key == "id" {
            return Ok(1);
        }
        let node = resolve_bound_node(store, nodes, &projection.variable, id).await?;
        let value = project_node_value(node, key);
        return Ok(usize::from(value != Value::Null));
    }
    if let Some(identity) = edge_bindings.get(&projection.variable) {
        if key == "id" {
            return Ok(usize::from(identity.id.is_some()));
        }
        if key == "label" {
            return Ok(1);
        }
        let edge = resolve_bound_edge_cached(store, edges, identity, &projection.variable).await?;
        let value = project_edge_value(edge, key);
        return Ok(usize::from(value != Value::Null));
    }
    if let Some(row_nodes) = row_node_values.get(&projection.variable) {
        if projection.distinct {
            return count_distinct_values(row_nodes.iter().filter_map(|node| {
                if key == "id" {
                    Some(Value::from(node.id.as_str()))
                } else {
                    non_null_return_value(project_node_value(node, key))
                }
            }));
        }
        return Ok(row_nodes
            .iter()
            .filter(|node| {
                if key == "id" {
                    true
                } else {
                    project_node_value(node, key) != Value::Null
                }
            })
            .count());
    }
    if let Some(row_edges) = row_edge_values.get(&projection.variable) {
        if projection.distinct {
            return count_distinct_values(row_edges.iter().filter_map(|edge| {
                if key == "id" {
                    edge.id.as_ref().map(|id| Value::from(id.as_str()))
                } else if key == "label" {
                    Some(Value::from(edge.label.as_str()))
                } else {
                    non_null_return_value(project_edge_value(edge, key))
                }
            }));
        }
        return Ok(row_edges
            .iter()
            .filter(|edge| {
                if key == "id" {
                    edge.id.is_some()
                } else if key == "label" {
                    true
                } else {
                    project_edge_value(edge, key) != Value::Null
                }
            })
            .count());
    }
    Err(cypher_unresolved_identity(format!(
        "RETURN references variable '{}' that is not bound by the write plan",
        projection.variable
    )))
}

async fn materialize_return_aggregate_values<'a, S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &'a mut HashMap<String, Node>,
    edges: &'a mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    aggregate: CypherReturnAggregate,
    row_count: usize,
) -> Result<Vec<Value>>
where
    S: GraphStore + Sync,
{
    let mut values = match &projection.target {
        CypherReturnTarget::All if aggregate == CypherReturnAggregate::Collect => {
            let mut values = Vec::with_capacity(row_count);
            for row_index in 0..row_count {
                values.push(
                    materialize_return_star_row_value(
                        store,
                        node_bindings,
                        edge_bindings,
                        row_node_values,
                        row_edge_values,
                        row_path_bindings,
                        nodes,
                        edges,
                        row_index,
                    )
                    .await?,
                );
            }
            values
        }
        CypherReturnTarget::All => {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN aggregates other than COUNT and COLLECT require variable.property",
            ));
        }
        CypherReturnTarget::Element if aggregate == CypherReturnAggregate::Collect => {
            materialize_return_element_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
            )
            .await?
        }
        CypherReturnTarget::Element => {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN aggregates other than COUNT and COLLECT require variable.property",
            ));
        }
        CypherReturnTarget::Literal(_) => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::Property(key) => {
            materialize_return_property_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
            )
            .await?
        }
        CypherReturnTarget::Case(case) => {
            materialize_return_case_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                case,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::Coalesce(_) => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::PropertyExists(_) => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::PropertySize(_) => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::PropertyAbs(_) => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::PropertyNumericRound { .. } => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::PropertyToString(_) => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::PropertyStringTransform { .. } => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::PropertyStringTrim { .. } => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::PropertyIsEmpty(_) => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::PropertyStringReverse(_) => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::PropertyStringSplit(_) => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::PropertySubstring(_) => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::PropertyStringSlice(_) => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::PropertyReplace(_) => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::PropertyStringPredicate(_) => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::NodeLabels
        | CypherReturnTarget::RelationshipType
        | CypherReturnTarget::ElementProperties
        | CypherReturnTarget::ElementKeys
        | CypherReturnTarget::ElementId
        | CypherReturnTarget::RelationshipStartNode
        | CypherReturnTarget::RelationshipEndNode => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::PathLength
        | CypherReturnTarget::PathNodes
        | CypherReturnTarget::PathRelationships => {
            materialize_return_path_function_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTarget::MapProjection(_) | CypherReturnTarget::ListProjection(_) => {
            materialize_return_projection_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_count,
            )
            .await?
        }
    };
    if projection.distinct {
        values = distinct_return_values(values)?;
    }
    Ok(values)
}

async fn materialize_return_property_values<'a, S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &'a mut HashMap<String, Node>,
    edges: &'a mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    key: &str,
) -> Result<Vec<Value>>
where
    S: GraphStore + Sync,
{
    if row_path_bindings.contains_key(&projection.variable) {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN path properties are not supported",
        ));
    }
    if let Some(id) = node_bindings.get(&projection.variable) {
        Ok(if key == "id" {
            vec![Value::from(id.as_str())]
        } else {
            let node = resolve_bound_node(store, nodes, &projection.variable, id).await?;
            non_null_return_value(project_node_value(node, key))
                .into_iter()
                .collect()
        })
    } else if let Some(identity) = edge_bindings.get(&projection.variable) {
        Ok(if key == "id" {
            identity
                .id
                .as_ref()
                .map(|id| Value::from(id.as_str()))
                .into_iter()
                .collect()
        } else if key == "label" {
            vec![Value::from(identity.label.as_str())]
        } else {
            let edge =
                resolve_bound_edge_cached(store, edges, identity, &projection.variable).await?;
            non_null_return_value(project_edge_value(edge, key))
                .into_iter()
                .collect()
        })
    } else if let Some(row_nodes) = row_node_values.get(&projection.variable) {
        Ok(row_nodes
            .iter()
            .filter_map(|node| {
                if key == "id" {
                    Some(Value::from(node.id.as_str()))
                } else {
                    non_null_return_value(project_node_value(node, key))
                }
            })
            .collect())
    } else if let Some(row_edges) = row_edge_values.get(&projection.variable) {
        Ok(row_edges
            .iter()
            .filter_map(|edge| {
                if key == "id" {
                    edge.id.as_ref().map(|id| Value::from(id.as_str()))
                } else if key == "label" {
                    Some(Value::from(edge.label.as_str()))
                } else {
                    non_null_return_value(project_edge_value(edge, key))
                }
            })
            .collect())
    } else {
        Err(cypher_unresolved_identity(format!(
            "RETURN references variable '{}' that is not bound by the write plan",
            projection.variable
        )))
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_star_row_value<'a, S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &'a mut HashMap<String, Node>,
    edges: &'a mut HashMap<String, Edge>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let write_rows =
        CypherWriteResultRows::new(row_node_values, row_edge_values, row_path_bindings);
    let mut variables = BTreeSet::new();
    variables.extend(node_bindings.keys().cloned());
    variables.extend(edge_bindings.keys().cloned());
    variables.extend(write_rows.variable_names());

    let mut row = serde_json::Map::new();
    for variable in variables {
        let value = if let Some(id) = node_bindings.get(&variable) {
            let node = resolve_bound_node(store, nodes, &variable, id).await?;
            graph_node_value(node)?
        } else if let Some(identity) = edge_bindings.get(&variable) {
            let edge = resolve_bound_edge_cached(store, edges, identity, &variable).await?;
            graph_edge_value(edge)?
        } else {
            match write_rows.binding_kind(&variable) {
                Some(CypherWriteResultBindingKind::RowNode) => {
                    let row_nodes = write_rows.row_nodes(&variable).ok_or_else(|| {
                        cypher_unsupported_cardinality(format!(
                            "writable Cypher RETURN cannot materialize matched node variable '{variable}'"
                        ))
                    })?;
                    let node = row_nodes.get(row_index).ok_or_else(|| {
                        cypher_unsupported_cardinality(format!(
                            "writable Cypher RETURN cannot materialize matched node variable '{variable}'"
                        ))
                    })?;
                    graph_node_value(node)?
                }
                Some(CypherWriteResultBindingKind::RowEdge) => {
                    let row_edges = write_rows.row_edges(&variable).ok_or_else(|| {
                        cypher_unsupported_cardinality(format!(
                            "writable Cypher RETURN cannot materialize row-producing relationship variable '{variable}'"
                        ))
                    })?;
                    let edge = row_edges.get(row_index).ok_or_else(|| {
                        cypher_unsupported_cardinality(format!(
                            "writable Cypher RETURN cannot materialize row-producing relationship variable '{variable}'"
                        ))
                    })?;
                    graph_edge_value(edge)?
                }
                Some(CypherWriteResultBindingKind::RowPath) => {
                    materialize_return_path_value_at(
                        store,
                        node_bindings,
                        edge_bindings,
                        row_node_values,
                        row_edge_values,
                        row_path_bindings,
                        nodes,
                        edges,
                        &variable,
                        row_index,
                    )
                    .await?
                }
                None => {
                    return Err(cypher_unresolved_identity(format!(
                        "RETURN references variable '{variable}' that is not bound by the write plan"
                    )));
                }
            }
        };
        row.insert(variable, value.to_json());
    }
    Ok(Value::Json(serde_json::Value::Object(row)))
}

async fn materialize_return_element_values<'a, S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &'a mut HashMap<String, Node>,
    edges: &'a mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
) -> Result<Vec<Value>>
where
    S: GraphStore + Sync,
{
    if let Some(id) = node_bindings.get(&projection.variable) {
        let node = resolve_bound_node(store, nodes, &projection.variable, id).await?;
        Ok(vec![graph_node_value(node)?])
    } else if let Some(identity) = edge_bindings.get(&projection.variable) {
        let edge = resolve_bound_edge_cached(store, edges, identity, &projection.variable).await?;
        Ok(vec![graph_edge_value(edge)?])
    } else if let Some(row_nodes) = row_node_values.get(&projection.variable) {
        row_nodes.iter().map(graph_node_value).collect()
    } else if let Some(row_edges) = row_edge_values.get(&projection.variable) {
        row_edges.iter().map(graph_edge_value).collect()
    } else if let Some(path) = row_path_bindings.get(&projection.variable) {
        let row_count = row_edge_values
            .get(&path.edge_variable)
            .map(Vec::len)
            .unwrap_or(1);
        materialize_return_path_values(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            &projection.variable,
            row_count,
        )
        .await
    } else {
        Err(cypher_unresolved_identity(format!(
            "RETURN references variable '{}' that is not bound by the write plan",
            projection.variable
        )))
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_path_values<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    variable: &str,
    row_count: usize,
) -> Result<Vec<Value>>
where
    S: GraphStore + Sync,
{
    let mut values = Vec::with_capacity(row_count);
    for row_index in 0..row_count {
        values.push(
            materialize_return_path_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                variable,
                row_index,
            )
            .await?,
        );
    }
    Ok(values)
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_path_function_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let path = materialize_return_path_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &projection.variable,
        row_index,
    )
    .await?;
    let Value::Json(path) = path else {
        return Err(GrustError::CypherExecution(
            "RETURN path materialization did not produce JSON".to_string(),
        ));
    };
    match projection.target {
        CypherReturnTarget::PathLength => {
            let relationships = path
                .get("relationships")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    GrustError::CypherExecution(
                        "RETURN path materialization is missing relationships".to_string(),
                    )
                })?;
            count_value(relationships.len())
        }
        CypherReturnTarget::PathNodes => Ok(Value::Json(path.get("nodes").cloned().ok_or_else(
            || {
                GrustError::CypherExecution(
                    "RETURN path materialization is missing nodes".to_string(),
                )
            },
        )?)),
        CypherReturnTarget::PathRelationships => Ok(Value::Json(
            path.get("relationships").cloned().ok_or_else(|| {
                GrustError::CypherExecution(
                    "RETURN path materialization is missing relationships".to_string(),
                )
            })?,
        )),
        _ => Err(cypher_unsupported_cardinality(
            "RETURN path function materializer received a non-path projection",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_path_function_values<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_count: usize,
) -> Result<Vec<Value>>
where
    S: GraphStore + Sync,
{
    let mut values = Vec::with_capacity(row_count);
    for row_index in 0..row_count {
        let value = materialize_return_path_function_value_at(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            projection,
            row_index,
        )
        .await?;
        if let Some(value) = non_null_return_value(value) {
            values.push(value);
        }
    }
    Ok(values)
}

fn evaluate_non_count_aggregate(
    aggregate: CypherReturnAggregate,
    values: Vec<Value>,
) -> Result<Value> {
    match aggregate {
        CypherReturnAggregate::Count => unreachable!("COUNT handled before non-count aggregate"),
        CypherReturnAggregate::Sum => sum_return_values(&values),
        CypherReturnAggregate::Avg => avg_return_values(&values),
        CypherReturnAggregate::Min => Ok(values
            .into_iter()
            .min_by(compare_return_values)
            .unwrap_or(Value::Null)),
        CypherReturnAggregate::Max => Ok(values
            .into_iter()
            .max_by(compare_return_values)
            .unwrap_or(Value::Null)),
        CypherReturnAggregate::Collect => Ok(Value::Json(serde_json::Value::Array(
            values.into_iter().map(|value| value.to_json()).collect(),
        ))),
    }
}

fn distinct_return_values(values: Vec<Value>) -> Result<Vec<Value>> {
    let mut seen = BTreeSet::new();
    let mut distinct = Vec::with_capacity(values.len());
    for value in values {
        let key = serde_json::to_string(&value.to_json()).map_err(|err| {
            GrustError::CypherExecution(format!(
                "RETURN DISTINCT aggregate value serialization failed: {err}"
            ))
        })?;
        if seen.insert(key) {
            distinct.push(value);
        }
    }
    Ok(distinct)
}

fn sum_return_values(values: &[Value]) -> Result<Value> {
    let mut int_sum = 0i64;
    let mut float_sum = 0.0f64;
    let mut saw_float = false;
    for value in values {
        match value {
            Value::Int(value) if !saw_float => {
                int_sum = int_sum.checked_add(*value).ok_or_else(|| {
                    GrustError::CypherExecution("RETURN SUM integer overflow".to_string())
                })?;
            }
            Value::Int(value) => {
                float_sum += *value as f64;
            }
            Value::Float(value) => {
                if !saw_float {
                    float_sum = int_sum as f64;
                    saw_float = true;
                }
                float_sum += *value;
            }
            other => {
                return Err(cypher_unsupported_cardinality(format!(
                    "RETURN SUM only supports numeric values, got {:?}",
                    other
                )));
            }
        }
    }
    if values.is_empty() {
        Ok(Value::Null)
    } else if saw_float {
        Ok(Value::Float(float_sum))
    } else {
        Ok(Value::Int(int_sum))
    }
}

fn avg_return_values(values: &[Value]) -> Result<Value> {
    if values.is_empty() {
        return Ok(Value::Null);
    }
    let mut sum = 0.0f64;
    for value in values {
        match value {
            Value::Int(value) => sum += *value as f64,
            Value::Float(value) => sum += *value,
            other => {
                return Err(cypher_unsupported_cardinality(format!(
                    "RETURN AVG only supports numeric values, got {:?}",
                    other
                )));
            }
        }
    }
    Ok(Value::Float(sum / values.len() as f64))
}

fn count_distinct_elements(
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    projection: &CypherReturnProjection,
    row_count: usize,
) -> Result<usize> {
    if row_path_bindings.contains_key(&projection.variable) {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN aggregates over path variables are not supported",
        ));
    }
    if let Some(id) = node_bindings.get(&projection.variable) {
        return Ok(usize::from(row_count > 0 && !id.as_str().is_empty()));
    }
    if let Some(identity) = edge_bindings.get(&projection.variable) {
        return Ok(usize::from(
            row_count > 0 && !edge_identity_count_key(identity).is_empty(),
        ));
    }
    if let Some(row_nodes) = row_node_values.get(&projection.variable) {
        return Ok(row_nodes
            .iter()
            .map(|node| node.id.as_str().to_string())
            .collect::<BTreeSet<_>>()
            .len());
    }
    if let Some(row_edges) = row_edge_values.get(&projection.variable) {
        return Ok(row_edges
            .iter()
            .map(edge_count_key)
            .collect::<BTreeSet<_>>()
            .len());
    }
    Err(cypher_unresolved_identity(format!(
        "RETURN references variable '{}' that is not bound by the write plan",
        projection.variable
    )))
}

fn count_distinct_values(values: impl IntoIterator<Item = Value>) -> Result<usize> {
    let mut distinct = BTreeSet::new();
    for value in values {
        let key = serde_json::to_string(&value.to_json()).map_err(|err| {
            GrustError::CypherExecution(format!(
                "COUNT(DISTINCT) value serialization failed: {err}"
            ))
        })?;
        distinct.insert(key);
    }
    Ok(distinct.len())
}

fn non_null_return_value(value: Value) -> Option<Value> {
    (value != Value::Null).then_some(value)
}

fn edge_identity_count_key(identity: &CypherBoundEdgeIdentity) -> String {
    identity
        .id
        .as_ref()
        .map(|id| format!("id:{}", id.as_str()))
        .unwrap_or_else(|| {
            format!(
                "struct:{}:{}:{}",
                identity.from.as_str(),
                identity.label.as_str(),
                identity.to.as_str()
            )
        })
}

fn edge_count_key(edge: &Edge) -> String {
    edge.id
        .as_ref()
        .map(|id| format!("id:{}", id.as_str()))
        .unwrap_or_else(|| {
            format!(
                "struct:{}:{}:{}",
                edge.from.as_str(),
                edge.label.as_str(),
                edge.to.as_str()
            )
        })
}

fn count_value(count: usize) -> Result<Value> {
    i64::try_from(count).map(Value::Int).map_err(|_| {
        GrustError::CypherExecution(format!("RETURN count {count} cannot fit in int64"))
    })
}

fn apply_return_distinct(rows: &mut Vec<Vec<Value>>) -> Result<()> {
    let mut seen = BTreeSet::new();
    let mut distinct = Vec::with_capacity(rows.len());
    for row in rows.drain(..) {
        let key = serde_json::to_string(
            &row.iter()
                .map(Value::to_json)
                .collect::<Vec<serde_json::Value>>(),
        )
        .map_err(|err| {
            GrustError::CypherExecution(format!("RETURN DISTINCT row serialization failed: {err}"))
        })?;
        if seen.insert(key) {
            distinct.push(row);
        }
    }
    *rows = distinct;
    Ok(())
}

/// Applies `ORDER BY` (stable), then `SKIP`, then `LIMIT` to materialized rows.
fn apply_return_control(
    rows: &mut Vec<Vec<Value>>,
    order_by: &[CypherOrderItem],
    skip: Option<usize>,
    limit: Option<usize>,
) {
    if !order_by.is_empty() {
        rows.sort_by(|a, b| {
            for item in order_by {
                let ordering = compare_return_values(&a[item.column], &b[item.column]);
                let ordering = if item.descending {
                    ordering.reverse()
                } else {
                    ordering
                };
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            }
            std::cmp::Ordering::Equal
        });
    }
    if let Some(skip) = skip {
        rows.drain(0..skip.min(rows.len()));
    }
    if let Some(limit) = limit {
        rows.truncate(limit);
    }
}

/// Total ordering over result values for `ORDER BY`. Nulls sort last (ascending),
/// numbers compare numerically, strings and bools compare naturally, and values
/// of different kinds fall back to a stable type rank so sorting is deterministic.
fn compare_return_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Greater,
        (_, Value::Null) => Ordering::Less,
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Int(x), Value::Float(y)) => (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Float(x), Value::Int(y)) => x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        _ => value_kind_rank(a).cmp(&value_kind_rank(b)),
    }
}

fn value_kind_rank(value: &Value) -> u8 {
    match value {
        Value::Bool(_) => 0,
        Value::Int(_) | Value::Float(_) => 1,
        Value::String(_) => 2,
        Value::DateTime(_) => 3,
        Value::StringArray(_) | Value::IntArray(_) | Value::FloatArray(_) => 4,
        Value::Json(_) => 5,
        Value::Null => 6,
    }
}

async fn resolve_bound_edge<S>(
    store: &S,
    identity: &CypherBoundEdgeIdentity,
    variable: &str,
) -> Result<Edge>
where
    S: GraphStore + Sync,
{
    let edges = store
        .get_edges(EdgeQuery {
            from: Some(identity.from.clone()),
            to: Some(identity.to.clone()),
            label: Some(identity.label.clone()),
        })
        .await?;
    edges
        .into_iter()
        .find(|edge| identity.id.as_ref().is_none_or(|id| edge.id.as_ref() == Some(id)))
        .ok_or_else(|| {
            GrustError::CypherExecution(format!(
                "RETURN relationship variable '{variable}' resolved to an edge that does not exist after the write"
            ))
        })
}

async fn row_edge_return_values_on_store<S>(
    store: &S,
    bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
) -> Result<HashMap<String, Vec<Edge>>>
where
    S: GraphStore + Sync,
{
    let mut values = HashMap::new();
    for (variable, binding) in bindings {
        let edges = store
            .get_edges(EdgeQuery {
                from: None,
                to: None,
                label: Some(binding.label.clone()),
            })
            .await?;
        let mut rows = Vec::new();
        for edge in edges {
            if !props_match(&edge.props, &binding.props) {
                continue;
            }
            let Some(from) = store.get_node(&edge.from).await? else {
                continue;
            };
            if !node_match(&from, &binding.from) {
                continue;
            }
            let Some(to) = store.get_node(&edge.to).await? else {
                continue;
            };
            if !node_match(&to, &binding.to) {
                continue;
            }
            rows.push(edge);
        }
        match binding.kind {
            GraphMutationPlanKind::Create | GraphMutationPlanKind::Merge => {}
        }
        values.insert(variable.clone(), rows);
    }
    Ok(values)
}

async fn row_edge_endpoint_node_values_on_store<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
    edge_values: &HashMap<String, Vec<Edge>>,
) -> Result<HashMap<String, Vec<Node>>>
where
    S: GraphStore + Sync,
{
    let mut values = HashMap::new();
    for (edge_variable, binding) in edge_bindings {
        let Some(edges) = edge_values.get(edge_variable) else {
            continue;
        };
        for (variable, endpoint) in [
            (&binding.from_variable, RowEdgeEndpoint::From),
            (&binding.to_variable, RowEdgeEndpoint::To),
        ] {
            if node_bindings.contains_key(variable) || values.contains_key(variable) {
                continue;
            }
            let mut nodes = Vec::with_capacity(edges.len());
            for edge in edges {
                let id = match endpoint {
                    RowEdgeEndpoint::From => &edge.from,
                    RowEdgeEndpoint::To => &edge.to,
                };
                let node = store.get_node(id).await?.ok_or_else(|| {
                    GrustError::CypherExecution(format!(
                        "RETURN endpoint variable '{variable}' resolved to missing node '{}'",
                        id.as_str()
                    ))
                })?;
                nodes.push(node);
            }
            values.insert(variable.clone(), nodes);
        }
    }
    Ok(values)
}

#[derive(Clone, Copy)]
enum RowEdgeEndpoint {
    From,
    To,
}

async fn collect_row_node_ids_for_operation<S>(
    store: &S,
    operation: &GraphMutationPlanOp,
    bindings: &HashMap<String, GraphNodeMatch>,
    values: &mut HashMap<String, Vec<NodeId>>,
) -> Result<()>
where
    S: GraphStore + Sync,
{
    let Some(operation_match) = operation_node_match(operation) else {
        return Ok(());
    };
    for (variable, binding) in bindings {
        if values.contains_key(variable) || binding != &operation_match {
            continue;
        }
        let nodes = matching_nodes_on_store(store, binding, variable).await?;
        values.insert(
            variable.clone(),
            nodes.into_iter().map(|node| node.id).collect(),
        );
    }
    Ok(())
}

async fn collect_row_edge_keys_for_operation<S>(
    store: &S,
    operation: &GraphMutationPlanOp,
    bindings: &HashMap<String, GraphRelationshipMatch>,
    values: &mut HashMap<String, Vec<String>>,
) -> Result<()>
where
    S: GraphStore + Sync,
{
    let Some(operation_match) = operation_relationship_match(operation) else {
        return Ok(());
    };
    for (variable, binding) in bindings {
        if values.contains_key(variable) || binding != &operation_match {
            continue;
        }
        let edges = matching_edges_on_store(store, binding).await?;
        values.insert(
            variable.clone(),
            edges.into_iter().map(|edge| edge_key(&edge)).collect(),
        );
    }
    Ok(())
}

async fn collect_deleted_row_node_values_for_operation<S>(
    store: &S,
    operation: &GraphMutationPlanOp,
    bindings: &HashMap<String, GraphNodeMatch>,
    values: &mut HashMap<String, Vec<Node>>,
) -> Result<()>
where
    S: GraphStore + Sync,
{
    let GraphMutationPlanOp::DeleteMatchingNodes {
        label,
        props,
        predicates,
        ..
    } = operation
    else {
        return Ok(());
    };
    let operation_match = GraphNodeMatch {
        label: label.clone(),
        props: props.clone(),
        predicates: predicates.clone(),
    };
    for (variable, binding) in bindings {
        if values.contains_key(variable) || binding != &operation_match {
            continue;
        }
        values.insert(
            variable.clone(),
            matching_nodes_on_store(store, binding, variable).await?,
        );
    }
    Ok(())
}

async fn collect_deleted_row_edge_values_for_operation<S>(
    store: &S,
    operation: &GraphMutationPlanOp,
    bindings: &HashMap<String, GraphRelationshipMatch>,
    values: &mut HashMap<String, Vec<Edge>>,
) -> Result<()>
where
    S: GraphStore + Sync,
{
    let GraphMutationPlanOp::DeleteMatchingEdges { relationship, .. } = operation else {
        return Ok(());
    };
    for (variable, binding) in bindings {
        if values.contains_key(variable) || binding != relationship {
            continue;
        }
        values.insert(
            variable.clone(),
            matching_edges_on_store(store, binding).await?,
        );
    }
    Ok(())
}

fn operation_node_match(operation: &GraphMutationPlanOp) -> Option<GraphNodeMatch> {
    match operation {
        GraphMutationPlanOp::PatchMatchingNodes {
            label,
            props,
            predicates,
            ..
        }
        | GraphMutationPlanOp::UpdateMatchingNodeProperty {
            label,
            props,
            predicates,
            ..
        }
        | GraphMutationPlanOp::RemoveMatchingNodeProps {
            label,
            props,
            predicates,
            ..
        } => Some(GraphNodeMatch {
            label: label.clone(),
            props: props.clone(),
            predicates: predicates.clone(),
        }),
        _ => None,
    }
}

fn operation_relationship_match(operation: &GraphMutationPlanOp) -> Option<GraphRelationshipMatch> {
    match operation {
        GraphMutationPlanOp::PatchMatchingEdges { relationship, .. }
        | GraphMutationPlanOp::UpdateMatchingEdgeProperty { relationship, .. }
        | GraphMutationPlanOp::RemoveMatchingEdgeProps { relationship, .. } => {
            Some(relationship.clone())
        }
        _ => None,
    }
}

async fn row_node_return_values_on_store<S>(
    store: &S,
    ids: HashMap<String, Vec<NodeId>>,
) -> Result<HashMap<String, Vec<Node>>>
where
    S: GraphStore + Sync,
{
    let mut values = HashMap::new();
    for (variable, ids) in ids {
        let nodes = store.get_nodes(&ids).await?;
        if nodes.len() != ids.len() {
            return Err(GrustError::CypherExecution(format!(
                "RETURN matched node variable '{variable}' includes a node that does not exist after the write"
            )));
        }
        values.insert(variable, nodes);
    }
    Ok(values)
}

async fn row_edge_match_return_values_on_store<S>(
    store: &S,
    ids: HashMap<String, Vec<String>>,
) -> Result<HashMap<String, Vec<Edge>>>
where
    S: GraphStore + Sync,
{
    let mut values = HashMap::new();
    for (variable, keys) in ids {
        let mut rows = Vec::with_capacity(keys.len());
        for key in keys {
            let edge = edge_by_key_on_store(store, &key).await?.ok_or_else(|| {
                GrustError::CypherExecution(format!(
                    "RETURN matched relationship variable '{variable}' includes an edge that does not exist after the write"
                ))
            })?;
            rows.push(edge);
        }
        values.insert(variable, rows);
    }
    Ok(values)
}

async fn edge_by_key_on_store<S>(store: &S, key: &str) -> Result<Option<Edge>>
where
    S: GraphStore + Sync,
{
    for edge in store
        .get_edges(EdgeQuery {
            from: None,
            to: None,
            label: None,
        })
        .await?
    {
        if edge_key(&edge) == key {
            return Ok(Some(edge));
        }
    }
    Ok(None)
}

fn merge_cypher_reports(report: &mut CypherMutationReport, next: CypherMutationReport) {
    report.creates += next.creates;
    report.merges += next.merges;
    report.deletes += next.deletes;
    report.patches += next.patches;
    report.property_removes += next.property_removes;
    report.matched_rows += next.matched_rows;
    report.changed_nodes += next.changed_nodes;
    report.changed_edges += next.changed_edges;
    report.node_upserts += next.node_upserts;
    report.edge_upserts += next.edge_upserts;
    report.node_deletes += next.node_deletes;
    report.edge_deletes += next.edge_deletes;
    report.node_patches += next.node_patches;
    report.edge_patches += next.edge_patches;
    report.node_property_removes += next.node_property_removes;
    report.edge_property_removes += next.edge_property_removes;
    report.node_inserts += next.node_inserts;
    report.node_updates += next.node_updates;
    report.edge_inserts += next.edge_inserts;
    report.edge_updates += next.edge_updates;
}

fn row_edge_id_policy_generates(kind: GraphMutationPlanKind, policy: GraphRowEdgeIdPolicy) -> bool {
    matches!(
        (kind, policy),
        (
            GraphMutationPlanKind::Create,
            GraphRowEdgeIdPolicy::GenerateForCreate
                | GraphRowEdgeIdPolicy::GenerateForCreateAndMerge
        ) | (
            GraphMutationPlanKind::Merge,
            GraphRowEdgeIdPolicy::GenerateForCreateAndMerge
        )
    )
}

enum CypherResolvedUpsertClassification {
    Node { existed: bool },
    Edge { existed: bool },
}

impl CypherResolvedUpsertClassification {
    fn record(self, report: &mut CypherMutationReport) {
        match self {
            CypherResolvedUpsertClassification::Node { existed: true } => {
                report.node_updates += 1;
            }
            CypherResolvedUpsertClassification::Node { existed: false } => {
                report.node_inserts += 1;
            }
            CypherResolvedUpsertClassification::Edge { existed: true } => {
                report.edge_updates += 1;
            }
            CypherResolvedUpsertClassification::Edge { existed: false } => {
                report.edge_inserts += 1;
            }
        }
    }
}

async fn matching_edges_on_store<S>(
    store: &S,
    relationship: &GraphRelationshipMatch,
) -> Result<Vec<Edge>>
where
    S: GraphStore + Sync,
{
    let edges = store
        .get_edges(EdgeQuery {
            from: None,
            to: None,
            label: Some(relationship.label.clone()),
        })
        .await?;
    let mut rows = Vec::new();
    for edge in edges {
        if relationship
            .id
            .as_ref()
            .is_some_and(|id| edge.id.as_ref() != Some(id))
        {
            continue;
        }
        if !props_match(&edge.props, &relationship.props)
            || !relationship
                .predicates
                .iter()
                .all(|predicate| predicate.matches(edge.props.get(&predicate.key)))
        {
            continue;
        }
        let Some(from) = store.get_node(&edge.from).await? else {
            continue;
        };
        if !node_match(&from, &relationship.from) {
            continue;
        }
        let Some(to) = store.get_node(&edge.to).await? else {
            continue;
        };
        if !node_match(&to, &relationship.to) {
            continue;
        }
        rows.push(edge);
    }
    Ok(rows)
}

async fn matching_nodes_on_store<S>(
    store: &S,
    binding: &GraphNodeMatch,
    variable: &str,
) -> Result<Vec<Node>>
where
    S: GraphStore + Sync,
{
    let candidates = if let Some(id) = binding
        .props
        .get("id")
        .and_then(Value::as_str)
        .map(NodeId::new)
    {
        store.get_node(&id).await?.into_iter().collect()
    } else if let Some(label) = &binding.label {
        if binding.props.is_empty() {
            store
                .traverse(Traversal {
                    start: Start::NodesByLabel(label.clone()),
                    steps: Vec::new(),
                    limit: None,
                })
                .await?
        } else if binding.props.len() == 1 {
            let (key, value) = binding
                .props
                .iter()
                .next()
                .expect("checked single property");
            store
                .traverse(Traversal {
                    start: Start::NodesByProperty {
                        label: label.clone(),
                        key: key.clone(),
                        value: value.clone(),
                    },
                    steps: Vec::new(),
                    limit: None,
                })
                .await?
        } else {
            return Err(cypher_unsupported_cardinality(format!(
                "writable Cypher RETURN cannot portably materialize matched node variable '{variable}' with multiple property predicates"
            )));
        }
    } else {
        return Err(cypher_unsupported_cardinality(format!(
            "writable Cypher RETURN cannot portably materialize matched node variable '{variable}' without a label or id"
        )));
    };
    Ok(candidates
        .into_iter()
        .filter(|node| node_match(node, binding))
        .collect())
}

fn props_match(actual: &Props, expected: &Props) -> bool {
    expected
        .iter()
        .all(|(key, value)| actual.get(key) == Some(value))
}

fn node_match(node: &Node, expected: &GraphNodeMatch) -> bool {
    expected
        .label
        .as_ref()
        .is_none_or(|label| &node.label == label)
        && expected.props.iter().all(|(key, value)| {
            if key == "id" {
                value.as_str().is_some_and(|id| node.id.as_str() == id)
            } else {
                node.props.get(key) == Some(value)
            }
        })
        && expected
            .predicates
            .iter()
            .all(|predicate| predicate.matches(node.props.get(&predicate.key)))
}

async fn resolve_bound_node<'a, S>(
    store: &S,
    nodes: &'a mut HashMap<String, Node>,
    variable: &str,
    id: &NodeId,
) -> Result<&'a Node>
where
    S: GraphStore + Sync,
{
    if !nodes.contains_key(variable) {
        let node = store.get_node(id).await?.ok_or_else(|| {
            GrustError::CypherExecution(format!(
                "RETURN variable '{variable}' resolved to node '{}' but the node does not exist after the write",
                id.as_str()
            ))
        })?;
        nodes.insert(variable.to_string(), node);
    }
    Ok(nodes
        .get(variable)
        .expect("node inserted before projection evaluation"))
}

async fn resolve_bound_edge_cached<'a, S>(
    store: &S,
    edges: &'a mut HashMap<String, Edge>,
    identity: &CypherBoundEdgeIdentity,
    variable: &str,
) -> Result<&'a Edge>
where
    S: GraphStore + Sync,
{
    if !edges.contains_key(variable) {
        let edge = resolve_bound_edge(store, identity, variable).await?;
        edges.insert(variable.to_string(), edge);
    }
    Ok(edges
        .get(variable)
        .expect("edge inserted before projection evaluation"))
}

fn project_node_value(node: &Node, key: &str) -> Value {
    if key == "label" {
        Value::from(node.label.as_str())
    } else {
        node.props.get(key).cloned().unwrap_or(Value::Null)
    }
}

fn project_edge_value(edge: &Edge, key: &str) -> Value {
    edge.props.get(key).cloned().unwrap_or(Value::Null)
}

fn graph_node_value(node: &Node) -> Result<Value> {
    serde_json::to_value(node).map(Value::from).map_err(|err| {
        GrustError::CypherExecution(format!("RETURN node serialization failed: {err}"))
    })
}

fn graph_edge_value(edge: &Edge) -> Result<Value> {
    serde_json::to_value(edge).map(Value::from).map_err(|err| {
        GrustError::CypherExecution(format!("RETURN relationship serialization failed: {err}"))
    })
}

pub async fn execute_cypher_mutation_returning_with_options_on_store<S>(
    store: &S,
    cypher: &str,
    options: CypherMutationOptions,
) -> Result<CypherMutationTableResult>
where
    S: CypherMutationExecutor + Sync,
{
    let create_mode = options.create_mode;
    let planned = sail_cypher_mutation_plan_with_return_options(cypher, options.clone())?;
    if create_mode == CypherCreateMode::ErrorIfExists {
        check_strict_create_conflicts_on_store(store, &planned.plan)
            .await
            .map_err(cypher_execution_error)?;
    }
    let mut row_node_ids = HashMap::new();
    let mut row_edge_keys = HashMap::new();
    let mut row_node_pre_delete_values = HashMap::new();
    let mut row_edge_pre_delete_values = HashMap::new();
    let mut report = CypherMutationReport::default();
    for operation in &planned.plan.operations {
        collect_deleted_row_node_values_for_operation(
            store,
            operation,
            &planned.row_node_bindings,
            &mut row_node_pre_delete_values,
        )
        .await
        .map_err(cypher_execution_error)?;
        collect_deleted_row_edge_values_for_operation(
            store,
            operation,
            &planned.row_edge_match_bindings,
            &mut row_edge_pre_delete_values,
        )
        .await
        .map_err(cypher_execution_error)?;
        collect_row_node_ids_for_operation(
            store,
            operation,
            &planned.row_node_bindings,
            &mut row_node_ids,
        )
        .await
        .map_err(cypher_execution_error)?;
        collect_row_edge_keys_for_operation(
            store,
            operation,
            &planned.row_edge_match_bindings,
            &mut row_edge_keys,
        )
        .await
        .map_err(cypher_execution_error)?;
        let operation_plan = GraphMutationPlan::new(vec![operation.clone()]);
        let operation_report = store
            .execute_cypher_mutation_plan(&operation_plan)
            .await
            .map_err(cypher_execution_error)?;
        merge_cypher_reports(&mut report, operation_report);
    }
    let mut row_node_values = row_node_return_values_on_store(store, row_node_ids)
        .await
        .map_err(cypher_execution_error)?;
    row_node_values.extend(row_node_pre_delete_values);
    let mut row_edge_values = row_edge_match_return_values_on_store(store, row_edge_keys)
        .await
        .map_err(cypher_execution_error)?;
    row_edge_values.extend(row_edge_pre_delete_values);
    row_edge_values.extend(
        row_edge_return_values_on_store(store, &planned.row_edge_bindings)
            .await
            .map_err(cypher_execution_error)?,
    );
    row_node_values.extend(
        row_edge_endpoint_node_values_on_store(
            store,
            &planned.node_bindings,
            &planned.row_edge_bindings,
            &row_edge_values,
        )
        .await
        .map_err(cypher_execution_error)?,
    );
    let table = evaluate_cypher_return_table(
        store,
        &planned.node_bindings,
        &planned.edge_bindings,
        &row_node_values,
        &row_edge_values,
        &planned.row_path_bindings,
        &planned.return_clause,
    )
    .await
    .map_err(cypher_execution_error)?;
    let mutation = cypher_mutation_result_from_plan(
        report,
        planned.generated_node_ids,
        &planned.plan,
        &options,
        &planned.row_edge_bindings,
        &row_edge_values,
    )?;
    Ok(CypherMutationTableResult { mutation, table })
}

async fn check_strict_create_conflicts_on_store<S>(
    store: &S,
    plan: &GraphMutationPlan,
) -> Result<()>
where
    S: GraphStore + Sync,
{
    check_strict_create_plan_conflicts(plan)?;
    for operation in &plan.operations {
        match operation {
            GraphMutationPlanOp::UpsertNode {
                kind: GraphMutationPlanKind::Create,
                node,
            } => {
                if store.get_node(&node.id).await?.is_some() {
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
                let mut existing = store
                    .get_edges(EdgeQuery {
                        from: Some(edge.from.clone()),
                        to: Some(edge.to.clone()),
                        label: Some(edge.label.clone()),
                    })
                    .await?;
                if edge.id.is_some() {
                    existing.extend(store.get_edges(EdgeQuery::default()).await?);
                }
                if strict_create_edge_conflicts(edge, &existing) {
                    return Err(GrustError::Unsupported(format!(
                        "Cypher CREATE would overwrite existing edge '{}'",
                        edge_key(edge)
                    )));
                }
            }
            GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
                kind: GraphMutationPlanKind::Create,
                ..
            } => {
                return Err(cypher_unsupported_cardinality(
                    "generic writable Cypher RETURN execution does not support strict CREATE checks for row-producing edge writes",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn check_strict_create_plan_conflicts(plan: &GraphMutationPlan) -> Result<()> {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for operation in &plan.operations {
        match operation {
            GraphMutationPlanOp::UpsertNode {
                kind: GraphMutationPlanKind::Create,
                node,
            } => {
                if nodes.iter().any(|id| id == &node.id) {
                    return Err(GrustError::Unsupported(format!(
                        "Cypher CREATE batch contains duplicate node '{}'",
                        node.id.as_str()
                    )));
                }
                nodes.push(node.id.clone());
            }
            GraphMutationPlanOp::UpsertEdge {
                kind: GraphMutationPlanKind::Create,
                edge,
            } => {
                if strict_create_edge_conflicts(edge, &edges) {
                    return Err(GrustError::Unsupported(format!(
                        "Cypher CREATE batch contains duplicate edge '{}'",
                        edge_key(edge)
                    )));
                }
                edges.push(edge.clone());
            }
            _ => {}
        }
    }
    Ok(())
}

struct NumericExpression {
    source_target: String,
    source_key: String,
    op: GraphNumericOp,
    operand: Value,
}

fn parse_numeric_expression(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<NumericExpression>> {
    for (index, op) in find_numeric_operator_candidates(expression) {
        let lhs = expression[..index].trim();
        let rhs = expression[index + 1..].trim();
        let Ok((source_target, source_key)) = parse_property_ref(lhs, "MATCH SET expression")
        else {
            continue;
        };
        let operand = parse_cypher_literal(rhs, parameters)?;
        if !matches!(operand, Value::Int(_) | Value::Float(_)) {
            return Err(cypher_syntax(
                "MATCH SET numeric expression operand must be an integer or float",
            ));
        }
        return Ok(Some(NumericExpression {
            source_target,
            source_key,
            op,
            operand,
        }));
    }
    Ok(None)
}

fn find_numeric_operator_candidates(expression: &str) -> Vec<(usize, GraphNumericOp)> {
    let mut candidates = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in expression.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '+' => candidates.push((index, GraphNumericOp::Add)),
            '-' if index > 0 => candidates.push((index, GraphNumericOp::Subtract)),
            '*' => candidates.push((index, GraphNumericOp::Multiply)),
            '/' => candidates.push((index, GraphNumericOp::Divide)),
            _ => {}
        }
    }
    candidates
}

fn parse_property_ref(value: &str, context: &str) -> Result<(String, String)> {
    let value = value.trim();
    let Some(index) = find_unquoted(value, '.') else {
        return Err(cypher_syntax(format!(
            "{context} requires property syntax target.key"
        )));
    };
    let target = parse_required_cypher_variable(&value[..index], context)?;
    let key = parse_cypher_prop_key(&value[index + 1..])?;
    Ok((target, key))
}

fn parse_cypher_props_map_literal(value: &str, parameters: &CypherParameters) -> Result<Props> {
    let value = value.trim();
    let Some(body) = value.strip_prefix('{') else {
        return Err(GrustError::Unsupported(
            "MATCH SET += requires a Cypher property map".to_string(),
        ));
    };
    let close = find_matching(body, '{', '}')?;
    if !body[close + 1..].trim().is_empty() {
        return Err(GrustError::Unsupported(
            "unsupported content after MATCH SET property map".to_string(),
        ));
    }
    parse_cypher_props(&body[..close], parameters)
}

fn split_cypher_body_props<'a>(
    body: &'a str,
    parameters: &CypherParameters,
) -> Result<(&'a str, Props)> {
    let body = body.trim();
    if let Some(open) = body.find('{') {
        let close = find_matching(&body[open + 1..], '{', '}')? + open + 1;
        if !body[close + 1..].trim().is_empty() {
            return Err(GrustError::Unsupported(
                "unsupported content after Cypher property map".to_string(),
            ));
        }
        Ok((
            &body[..open],
            parse_cypher_props(&body[open + 1..close], parameters)?,
        ))
    } else {
        Ok((body, Props::new()))
    }
}

fn parse_cypher_props(body: &str, parameters: &CypherParameters) -> Result<Props> {
    let mut props = Props::new();
    for entry in split_top_level_commas(body)? {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let colon = find_unquoted(entry, ':').ok_or_else(|| {
            GrustError::Unsupported(format!("Cypher property entry is missing ':': {entry}"))
        })?;
        let key = parse_cypher_prop_key(&entry[..colon])?;
        let value = parse_cypher_literal(&entry[colon + 1..], parameters)?;
        props.insert(key, value);
    }
    Ok(props)
}

fn parse_cypher_prop_key(key: &str) -> Result<String> {
    let key = key.trim();
    if key.is_empty() {
        return Err(GrustError::Unsupported(
            "Cypher property key cannot be empty".to_string(),
        ));
    }
    if is_quoted(key) {
        parse_cypher_string(key)
    } else if key
        .chars()
        .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        Ok(key.to_string())
    } else {
        Err(GrustError::Unsupported(format!(
            "unsupported Cypher property key: {key}"
        )))
    }
}

fn parse_cypher_literal(value: &str, parameters: &CypherParameters) -> Result<Value> {
    let value = value.trim();
    if value.is_empty() {
        return Err(GrustError::Unsupported(
            "Cypher property value cannot be empty".to_string(),
        ));
    }
    if is_quoted(value) {
        return Ok(Value::String(parse_cypher_string(value)?));
    }
    if let Some(parameter) = value.strip_prefix('$') {
        if !is_cypher_identifier(parameter) {
            return Err(cypher_syntax(format!(
                "unsupported Cypher parameter reference: {value}"
            )));
        }
        return parameters.get(parameter).cloned().ok_or_else(|| {
            cypher_unresolved_identity(format!("Cypher parameter '{value}' was not provided"))
        });
    }
    match value {
        "true" | "TRUE" => return Ok(Value::Bool(true)),
        "false" | "FALSE" => return Ok(Value::Bool(false)),
        "null" | "NULL" => return Ok(Value::Null),
        _ => {}
    }
    if value.contains('.') {
        return value.parse::<f64>().map(Value::Float).map_err(|_| {
            GrustError::Unsupported(format!("unsupported Cypher literal value: {value}"))
        });
    }
    value
        .parse::<i64>()
        .map(Value::Int)
        .map_err(|_| GrustError::Unsupported(format!("unsupported Cypher literal value: {value}")))
}

fn parse_cypher_string(value: &str) -> Result<String> {
    let value = value.trim();
    if !is_quoted(value) {
        return Err(GrustError::Unsupported(format!(
            "expected quoted Cypher string literal: {value}"
        )));
    }
    let quote = value.as_bytes()[0] as char;
    let inner = &value[1..value.len() - 1];
    let mut output = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let escaped = chars.next().ok_or_else(|| {
                GrustError::Unsupported("unterminated Cypher string escape".to_string())
            })?;
            output.push(match escaped {
                '\\' => '\\',
                '\'' if quote == '\'' => '\'',
                '"' if quote == '"' => '"',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
        } else {
            output.push(ch);
        }
    }
    Ok(output)
}

fn optional_string_prop(props: &Props, key: &str) -> Option<String> {
    props.get(key).and_then(Value::as_str).map(str::to_string)
}

fn has_relationship_predicates_beyond_id(props: &Props) -> bool {
    props.keys().any(|key| key.as_str() != "id")
}

fn split_top_level_commas(value: &str) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;

    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => paren_depth += 1,
            ')' => {
                paren_depth = paren_depth.checked_sub(1).ok_or_else(|| {
                    cypher_syntax("unmatched ')' in Cypher expression".to_string())
                })?;
            }
            '[' => bracket_depth += 1,
            ']' => {
                bracket_depth = bracket_depth.checked_sub(1).ok_or_else(|| {
                    cypher_syntax("unmatched ']' in Cypher expression".to_string())
                })?;
            }
            '{' => brace_depth += 1,
            '}' => {
                brace_depth = brace_depth.checked_sub(1).ok_or_else(|| {
                    cypher_syntax("unmatched '}' in Cypher expression".to_string())
                })?;
            }
            ',' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => {
                parts.push(&value[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if quote.is_some() {
        return Err(GrustError::Unsupported(
            "unterminated Cypher string literal".to_string(),
        ));
    }
    if paren_depth != 0 || bracket_depth != 0 || brace_depth != 0 {
        return Err(cypher_syntax("unclosed grouping in Cypher expression"));
    }
    parts.push(&value[start..]);
    Ok(parts)
}

fn split_top_level_patterns(value: &str) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;

    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ',' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => {
                let part = value[start..index].trim();
                if !part.is_empty() {
                    parts.push(part);
                }
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if quote.is_some() {
        return Err(GrustError::Unsupported(
            "unterminated Cypher string literal".to_string(),
        ));
    }
    let part = value[start..].trim();
    if !part.is_empty() {
        parts.push(part);
    }
    Ok(parts)
}

fn find_matching(value: &str, _open: char, close: char) -> Result<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            ch if ch == close => return Ok(index),
            _ => {}
        }
    }
    Err(GrustError::Unsupported(format!(
        "Cypher pattern is missing '{close}'"
    )))
}

/// Scans `value` left to right, skipping single- and double-quoted spans (with
/// backslash escapes inside them), and returns the first unquoted byte offset
/// where `at_unquoted(index, rest)` returns true. `rest` is `&value[index..]`.
fn scan_unquoted(value: &str, mut at_unquoted: impl FnMut(usize, &str) -> bool) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if at_unquoted(index, &value[index..]) {
            return Some(index);
        }
    }
    None
}

fn find_unquoted(value: &str, target: char) -> Option<usize> {
    scan_unquoted(value, |_, rest| rest.starts_with(target))
}

fn find_unquoted_sequence(value: &str, target: &str) -> Option<usize> {
    scan_unquoted(value, |_, rest| rest.starts_with(target))
}

fn find_unquoted_keyword(value: &str, keyword: &str) -> Option<usize> {
    scan_unquoted(value, |index, rest| {
        rest.get(..keyword.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(keyword))
            && keyword_boundary(value[..index].chars().next_back())
            && keyword_boundary(rest[keyword.len()..].chars().next())
    })
}

fn keyword_boundary(ch: Option<char>) -> bool {
    ch.is_none_or(char::is_whitespace)
}

fn strip_leading_keyword<'a>(value: &'a str, keyword: &str) -> Option<&'a str> {
    let candidate = value.get(..keyword.len())?;
    if !candidate.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &value[keyword.len()..];
    let first = rest.chars().next()?;
    if !first.is_whitespace() {
        return None;
    }
    Some(rest.trim_start())
}

fn is_quoted(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2
        && ((bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"'))
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

fn strict_create_edge_conflicts(edge: &Edge, existing: &[Edge]) -> bool {
    existing.iter().any(|existing| {
        let same_explicit_id = edge
            .id
            .as_ref()
            .is_some_and(|id| existing.id.as_ref() == Some(id));
        let same_structural_identity =
            existing.from == edge.from && existing.to == edge.to && existing.label == edge.label;
        same_explicit_id || same_structural_identity
    })
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
            | Value::StringArray(_)
            | Value::IntArray(_)
            | Value::FloatArray(_)
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
            | Value::StringArray(_)
            | Value::IntArray(_)
            | Value::FloatArray(_)
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
            | Value::StringArray(_)
            | Value::IntArray(_)
            | Value::FloatArray(_)
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
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
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
        GraphPredicateOp::Equal | GraphPredicateOp::NotEqual => unreachable!(),
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
