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
pub struct CypherMutationOptions {
    pub create_mode: CypherCreateMode,
    pub node_id_policy: CypherNodeIdPolicy,
}

impl Default for CypherMutationOptions {
    fn default() -> Self {
        Self {
            create_mode: CypherCreateMode::UpsertCompatible,
            node_id_policy: CypherNodeIdPolicy::ExplicitOnly,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CypherGeneratedNodeId {
    pub variable: Option<String>,
    pub id: NodeId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CypherMutationResult {
    pub report: CypherMutationReport,
    pub generated_node_ids: Vec<CypherGeneratedNodeId>,
}

// ── Store ─────────────────────────────────────────────────────────────────────

/// Session-scoped temp views used to stage Arrow batches before MERGE.
const NODE_STAGE_VIEW: &str = "grust_stage_nodes";
const EDGE_STAGE_VIEW: &str = "grust_stage_edges";
const DELETE_NODE_STAGE_VIEW: &str = "grust_delete_node_ids";
const DELETE_EDGE_STAGE_VIEW: &str = "grust_delete_edges";
pub const GRUST_NODES_TABLE: &str = "grust_nodes";
pub const GRUST_EDGES_TABLE: &str = "grust_edges";
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

#[derive(Default)]
struct CypherMutationPlanner {
    node_bindings: HashMap<String, NodeId>,
    node_id_policy: CypherNodeIdPolicy,
    generated_node_ids: Vec<CypherGeneratedNodeId>,
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
        if pattern.contains("->") {
            let parsed = self.parse_edge_pattern(pattern)?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::UpsertEdge {
                    kind,
                    edge: parsed.edge,
                },
            ]));
        }

        let (node, rest) = parse_cypher_node_pattern(pattern)?;
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
        if pattern.contains("->") {
            let parsed = self.parse_edge_pattern(pattern)?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::DeleteEdge {
                    from: parsed.from_id,
                    label: parsed.edge.label,
                    to: parsed.to_id,
                },
            ]));
        }

        let (node, rest) = parse_cypher_node_pattern(pattern)?;
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
        let target = parse_required_cypher_variable(target.trim(), "MATCH DELETE target")?;

        if pattern.contains("->") {
            let parsed = self.parse_edge_match_pattern(pattern)?;
            let Some(edge_variable) = parsed.relationship.variable.clone() else {
                return Err(cypher_syntax(
                    "MATCH edge DELETE requires the relationship pattern to bind the DELETE target"
                        .to_string(),
                ));
            };
            if edge_variable != target {
                return Err(cypher_syntax(format!(
                    "MATCH edge DELETE target '{target}' does not match relationship variable '{edge_variable}'"
                )));
            }
            let from_id = self.resolved_endpoint_id(&parsed.from)?;
            let to_id = self.resolved_endpoint_id(&parsed.to)?;
            let edge_id = optional_string_prop(&parsed.relationship.props, "id");
            if let (Some(from), Some(to), None) = (from_id, to_id, edge_id) {
                if parsed
                    .relationship
                    .props
                    .keys()
                    .any(|key| key.as_str() != "id")
                {
                    return Err(cypher_unsupported_cardinality(
                        "MATCH edge DELETE only supports endpoint, type, and optional edge id identity",
                    ));
                }
                return Ok(GraphMutationPlan::new(vec![
                    GraphMutationPlanOp::DeleteEdge {
                        from,
                        label: parsed.relationship.label,
                        to,
                    },
                ]));
            }
            let relationship = self.relationship_match_from_pattern(parsed, "MATCH edge DELETE")?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::DeleteMatchingEdges {
                    relationship,
                    cardinality: GraphMutationCardinality::BoundedMany,
                },
            ]));
        }

        let (node, rest) = parse_cypher_node_pattern(pattern)?;
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
        if let Some(id) = optional_string_prop(&node.props, "id") {
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
        {
            let id = self.resolve_node_id(&node, "MATCH node DELETE")?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::DeleteNode(id),
            ]));
        }
        let cardinality = match_node_cardinality(&node);
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::DeleteMatchingNodes {
                label: node.label,
                props: node.props,
                cardinality,
            },
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
        for pattern in split_top_level_patterns(match_clause)? {
            let (node, rest) = parse_cypher_node_pattern(pattern)?;
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
            self.resolve_node_id(&node, &format!("MATCH {keyword} node"))?;
        }

        if !edge_pattern.contains("->") {
            return Err(cypher_syntax(format!(
                "MATCH {keyword} currently supports one relationship pattern only",
            )));
        }
        let parsed = self.parse_edge_pattern(edge_pattern)?;
        if parsed.from_variable.is_none() || parsed.to_variable.is_none() {
            return Err(cypher_syntax(format!(
                "MATCH {keyword} relationship endpoints must be bound variables"
            )));
        }
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::UpsertEdge {
                kind,
                edge: parsed.edge,
            },
        ]))
    }

    fn parse_match_set(&mut self, statement: &str) -> Result<GraphMutationPlan> {
        let (pattern, assignment) = split_match_set(statement)?;
        let assignment = parse_patch_assignment(assignment)?;

        if pattern.contains("->") {
            let parsed = self.parse_edge_match_pattern(pattern)?;
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
            if parsed
                .relationship
                .props
                .keys()
                .any(|key| key.as_str() != "id")
            {
                return Err(cypher_unsupported_cardinality(
                    "MATCH edge SET only supports endpoint, type, and optional edge id identity",
                ));
            }
            let from_id = self.resolved_endpoint_id(&parsed.from)?;
            let to_id = self.resolved_endpoint_id(&parsed.to)?;
            if let (Some(from), Some(to)) = (from_id, to_id) {
                let id = optional_string_prop(&parsed.relationship.props, "id").map(EdgeId::new);
                return Ok(GraphMutationPlan::new(vec![
                    GraphMutationPlanOp::PatchEdge {
                        from,
                        label: parsed.relationship.label,
                        to,
                        id,
                        props: assignment.props,
                    },
                ]));
            }
            let relationship = self.relationship_match_from_pattern(parsed, "MATCH edge SET")?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::PatchMatchingEdges {
                    relationship,
                    patch: assignment.props,
                    cardinality: GraphMutationCardinality::BoundedMany,
                },
            ]));
        }

        let (node, rest) = parse_cypher_node_pattern(pattern)?;
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
        if optional_string_prop(&node.props, "id").is_none()
            && node
                .variable
                .as_ref()
                .is_none_or(|variable| !self.node_bindings.contains_key(variable))
        {
            let cardinality = match_node_cardinality(&node);
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::PatchMatchingNodes {
                    label: node.label,
                    props: node.props,
                    patch: assignment.props,
                    cardinality,
                },
            ]));
        }
        let id = self.resolve_node_id(&node, "MATCH node SET")?;
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::PatchNode {
                id,
                props: assignment.props,
            },
        ]))
    }

    fn parse_match_remove(&mut self, statement: &str) -> Result<GraphMutationPlan> {
        let (pattern, target) = split_match_remove(statement)?;
        let (target, key) = parse_property_ref(target, "MATCH REMOVE target")?;

        if pattern.contains("->") {
            let parsed = self.parse_edge_match_pattern(pattern)?;
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
            if parsed
                .relationship
                .props
                .keys()
                .any(|key| key.as_str() != "id")
            {
                return Err(cypher_unsupported_cardinality(
                    "MATCH edge REMOVE only supports endpoint, type, and optional edge id identity",
                ));
            }
            let from_id = self.resolved_endpoint_id(&parsed.from)?;
            let to_id = self.resolved_endpoint_id(&parsed.to)?;
            if let (Some(from), Some(to)) = (from_id, to_id) {
                let id = optional_string_prop(&parsed.relationship.props, "id").map(EdgeId::new);
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
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::RemoveMatchingEdgeProps {
                    relationship,
                    keys: vec![key],
                    cardinality: GraphMutationCardinality::BoundedMany,
                },
            ]));
        }

        let (node, rest) = parse_cypher_node_pattern(pattern)?;
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
        if optional_string_prop(&node.props, "id").is_none()
            && node
                .variable
                .as_ref()
                .is_none_or(|variable| !self.node_bindings.contains_key(variable))
        {
            let cardinality = match_node_cardinality(&node);
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::RemoveMatchingNodeProps {
                    label: node.label,
                    props: node.props,
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
        let (from, rest) = parse_cypher_node_pattern(pattern)?;
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
        let (to, rest) = parse_cypher_node_pattern(rest)?;
        if !rest.trim().is_empty() {
            return Err(cypher_syntax(format!(
                "unsupported writable Cypher edge pattern suffix: {}",
                rest.trim()
            )));
        }

        let from_id = self.resolve_node_id(&from, "edge mutation source node")?;
        let to_id = self.resolve_node_id(&to, "edge mutation destination node")?;
        let relationship = parse_cypher_relationship(rel)?;
        let mut edge = Edge::new(
            relationship.label,
            from_id.clone(),
            to_id.clone(),
            relationship.props,
        );
        if let Some(id) = edge
            .props
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            edge = edge.with_id(id);
        }
        Ok(ParsedCypherEdge {
            from_id,
            to_id,
            edge,
            from_variable: from.variable,
            to_variable: to.variable,
        })
    }

    fn parse_edge_match_pattern(&self, pattern: &str) -> Result<ParsedCypherEdgeMatch> {
        let (from, rest) = parse_cypher_node_pattern(pattern)?;
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
        let (to, rest) = parse_cypher_node_pattern(rest)?;
        if !rest.trim().is_empty() {
            return Err(cypher_syntax(format!(
                "unsupported writable Cypher edge pattern suffix: {}",
                rest.trim()
            )));
        }
        let relationship = parse_cypher_relationship(rel)?;
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
        if parsed
            .relationship
            .props
            .keys()
            .any(|key| key.as_str() != "id")
        {
            return Err(cypher_unsupported_cardinality(format!(
                "{context} only supports endpoint, type, and optional edge id identity"
            )));
        }
        let id = optional_string_prop(&parsed.relationship.props, "id").map(EdgeId::new);
        let from = self.node_match_from_pattern(parsed.from, context)?;
        let to = self.node_match_from_pattern(parsed.to, context)?;
        Ok(GraphRelationshipMatch {
            from,
            label: parsed.relationship.label,
            to,
            id,
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

#[derive(Debug)]
struct ParsedCypherNode {
    variable: Option<String>,
    label: Option<Label>,
    props: Props,
}

#[derive(Debug)]
struct ParsedCypherEdge {
    from_id: NodeId,
    to_id: NodeId,
    edge: Edge,
    from_variable: Option<String>,
    to_variable: Option<String>,
}

#[derive(Debug)]
struct ParsedCypherEdgeMatch {
    from: ParsedCypherNode,
    relationship: ParsedCypherRelationship,
    to: ParsedCypherNode,
}

#[derive(Debug)]
struct ParsedCypherRelationship {
    variable: Option<String>,
    label: Label,
    props: Props,
}

fn parse_cypher_node_pattern(input: &str) -> Result<(ParsedCypherNode, &str)> {
    let input = input.trim_start();
    let input = input.strip_prefix('(').ok_or_else(|| {
        GrustError::Unsupported("writable Cypher node pattern must start with '('".to_string())
    })?;
    let close = find_matching(input, '(', ')')?;
    let body = input[..close].trim();
    let rest = &input[close + 1..];
    let (variable, label, props) = parse_cypher_node_body(body)?;
    Ok((
        ParsedCypherNode {
            variable,
            label,
            props,
        },
        rest,
    ))
}

fn parse_cypher_node_body(body: &str) -> Result<(Option<String>, Option<Label>, Props)> {
    let (head, props) = split_cypher_body_props(body)?;
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

fn parse_cypher_relationship(body: &str) -> Result<ParsedCypherRelationship> {
    let (head, props) = split_cypher_body_props(body.trim())?;
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
    })
}

fn match_node_cardinality(node: &ParsedCypherNode) -> GraphMutationCardinality {
    if node.label.is_some() || !node.props.is_empty() {
        GraphMutationCardinality::BoundedMany
    } else {
        GraphMutationCardinality::UnboundedMany
    }
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

struct PatchAssignment {
    target: String,
    props: Props,
}

fn parse_patch_assignment(assignment: &str) -> Result<PatchAssignment> {
    if let Some(index) = find_unquoted_sequence(assignment, "+=") {
        let target = parse_required_cypher_variable(&assignment[..index], "MATCH SET target")?;
        let props = parse_cypher_props_map_literal(&assignment[index + 2..])?;
        return Ok(PatchAssignment { target, props });
    }
    let Some(index) = find_unquoted(assignment, '=') else {
        return Err(cypher_syntax(
            "MATCH SET only supports map patch or literal property assignment",
        ));
    };
    let (target, key) = parse_property_ref(&assignment[..index], "MATCH SET target")?;
    let value = parse_cypher_literal(&assignment[index + 1..])?;
    Ok(PatchAssignment {
        target,
        props: Props::from([(key, value)]),
    })
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

fn parse_cypher_props_map_literal(value: &str) -> Result<Props> {
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
    parse_cypher_props(&body[..close])
}

fn split_cypher_body_props(body: &str) -> Result<(&str, Props)> {
    let body = body.trim();
    if let Some(open) = body.find('{') {
        let close = find_matching(&body[open + 1..], '{', '}')? + open + 1;
        if !body[close + 1..].trim().is_empty() {
            return Err(GrustError::Unsupported(
                "unsupported content after Cypher property map".to_string(),
            ));
        }
        Ok((&body[..open], parse_cypher_props(&body[open + 1..close])?))
    } else {
        Ok((body, Props::new()))
    }
}

fn parse_cypher_props(body: &str) -> Result<Props> {
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
        let value = parse_cypher_literal(&entry[colon + 1..])?;
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

fn parse_cypher_literal(value: &str) -> Result<Value> {
    let value = value.trim();
    if value.is_empty() {
        return Err(GrustError::Unsupported(
            "Cypher property value cannot be empty".to_string(),
        ));
    }
    if is_quoted(value) {
        return Ok(Value::String(parse_cypher_string(value)?));
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

fn split_top_level_commas(value: &str) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
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
            ',' => {
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

fn find_unquoted(value: &str, target: char) -> Option<usize> {
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
            ch if ch == target => return Some(index),
            _ => {}
        }
    }
    None
}

fn find_unquoted_keyword(value: &str, keyword: &str) -> Option<usize> {
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
            _ => {
                if value[index..]
                    .get(..keyword.len())
                    .is_some_and(|candidate| candidate.eq_ignore_ascii_case(keyword))
                    && keyword_boundary(value[..index].chars().next_back())
                    && keyword_boundary(value[index + keyword.len()..].chars().next())
                {
                    return Some(index);
                }
            }
        }
    }
    None
}

fn find_unquoted_sequence(value: &str, target: &str) -> Option<usize> {
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
            _ if value[index..].starts_with(target) => return Some(index),
            _ => {}
        }
    }
    None
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
        let (plan, generated_node_ids) = sail_cypher_mutation_plan_with_options(cypher, options)?;
        let mut report = plan.report();
        if options.create_mode == CypherCreateMode::ErrorIfExists {
            self.check_strict_create_conflicts(&plan)
                .await
                .map_err(cypher_execution_error)?;
        }
        self.apply_cypher_mutation_plan(&plan, &mut report)
            .await
            .map_err(cypher_execution_error)?;
        Ok(CypherMutationResult {
            report,
            generated_node_ids,
        })
    }

    async fn apply_cypher_mutation_plan(
        &self,
        plan: &GraphMutationPlan,
        report: &mut CypherMutationReport,
    ) -> Result<()> {
        for operation in &plan.operations {
            match operation {
                GraphMutationPlanOp::PatchMatchingNodes {
                    label,
                    props,
                    patch,
                    ..
                } => {
                    self.apply_patch_matching_nodes(label.as_ref(), props, patch, report)
                        .await?;
                }
                GraphMutationPlanOp::RemoveMatchingNodeProps {
                    label, props, keys, ..
                } => {
                    self.apply_remove_matching_node_props(label.as_ref(), props, keys, report)
                        .await?;
                }
                GraphMutationPlanOp::DeleteMatchingNodes { label, props, .. } => {
                    self.apply_delete_matching_nodes(label.as_ref(), props, report)
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
                _ => {
                    let mutation = GraphMutation::from(operation.clone());
                    self.apply_mutations(std::slice::from_ref(&mutation))
                        .await?;
                }
            }
        }
        Ok(())
    }

    async fn apply_patch_matching_nodes(
        &self,
        label: Option<&Label>,
        props: &Props,
        patch: &Props,
        report: &mut CypherMutationReport,
    ) -> Result<()> {
        let (sql, args) = matching_nodes_sql(label, props)?;
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

    async fn apply_remove_matching_node_props(
        &self,
        label: Option<&Label>,
        props: &Props,
        keys: &[String],
        report: &mut CypherMutationReport,
    ) -> Result<()> {
        let (sql, args) = matching_nodes_sql(label, props)?;
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
        for edge in edges {
            self.delete_edge(&edge.from, &edge.label, &edge.to).await?;
        }
        Ok(())
    }

    async fn matching_edges(&self, relationship: &GraphRelationshipMatch) -> Result<Vec<Edge>> {
        let (sql, args) = matching_edges_sql(relationship)?;
        self.run_edge_query(&sql, args).await
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
        report: &mut CypherMutationReport,
    ) -> Result<()> {
        let (sql, args) = matching_nodes_sql(label, props)?;
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
                _ => {}
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
        self.apply_cypher_mutation_plan(plan, &mut report)
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
