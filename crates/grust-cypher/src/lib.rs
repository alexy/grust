use std::collections::{BTreeMap, BTreeSet, HashMap};

use grust_core::prelude::*;
use serde::{Deserialize, Serialize};

pub mod ast;
pub mod gql;
pub mod lexer;
pub mod parser;
pub mod semantics;
pub use gql::{
    feature_manifest, gql_cardinality, gql_execution, gql_name, gql_syntax, gql_type, load_manifest,
    load_manifest_cases, support_counts, support_summary, unsupported_gql_feature,
    GqlConformanceProfile, GqlError, GqlErrorKind, GqlExpectation, GqlFeature, GqlFeatureDescriptor,
    GqlFeatureFamily, GqlFeatureStatus, GqlManifest, GqlManifestCase, GqlRequirement,
    GqlSupportCounts,
};

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

pub fn cypher_mutation_plan(cypher: &str) -> Result<GraphMutationPlan> {
    let (plan, _) =
        sail_cypher_mutation_plan_with_options(cypher, CypherMutationOptions::default())?;
    Ok(plan)
}

pub fn sail_cypher_mutation_plan(cypher: &str) -> Result<GraphMutationPlan> {
    cypher_mutation_plan(cypher)
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

/// Applies parsed Cypher `CREATE CONSTRAINT` DDL through a backend's native
/// constraint path.
///
/// This helper preserves `IF NOT EXISTS` reporting from
/// [`GraphStore::apply_native_constraint`]. `DROP CONSTRAINT` is rejected until
/// Grust has backend-neutral native drop semantics.
pub async fn apply_cypher_native_constraints<S>(
    store: &S,
    cypher: &str,
) -> Result<GraphNativeConstraintReport>
where
    S: GraphStore + Sync,
{
    let mut report = GraphNativeConstraintReport::default();
    for statement in cypher_ddl(cypher)? {
        match statement {
            CypherDdlStatement::CreateConstraint {
                if_not_exists,
                constraint,
                ..
            } => {
                let applied = store
                    .apply_native_constraint(GraphNativeConstraintRequest {
                        constraint,
                        if_not_exists,
                    })
                    .await?;
                report.applied += applied.applied;
                report.skipped += applied.skipped;
            }
            CypherDdlStatement::DropConstraint { .. } => {
                return Err(cypher_syntax(
                    "native Cypher constraint application does not support DROP CONSTRAINT",
                ));
            }
        }
    }
    Ok(report)
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
pub fn cypher_ddl(cypher: &str) -> Result<Vec<CypherDdlStatement>> {
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

pub fn sail_cypher_ddl(cypher: &str) -> Result<Vec<CypherDdlStatement>> {
    cypher_ddl(cypher)
}

/// Parses Cypher constraint DDL and returns only the resulting
/// [`GraphConstraint`] values, discarding names and `IF [NOT] EXISTS` flags.
///
/// `DROP CONSTRAINT` statements are rejected because they carry no constraint
/// body; use [`sail_cypher_ddl`] when those are needed.
pub fn cypher_constraints(cypher: &str) -> Result<Vec<GraphConstraint>> {
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

pub fn sail_cypher_constraints(cypher: &str) -> Result<Vec<GraphConstraint>> {
    cypher_constraints(cypher)
}

pub fn cypher_mutation_plan_with_options(
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

pub fn sail_cypher_mutation_plan_with_options(
    cypher: &str,
    options: CypherMutationOptions,
) -> Result<(GraphMutationPlan, Vec<CypherGeneratedNodeId>)> {
    cypher_mutation_plan_with_options(cypher, options)
}

pub struct CypherPlannedMutationWithReturn {
    pub plan: GraphMutationPlan,
    pub generated_node_ids: Vec<CypherGeneratedNodeId>,
    pub node_bindings: HashMap<String, NodeId>,
    pub edge_bindings: HashMap<String, CypherBoundEdgeIdentity>,
    pub row_node_bindings: HashMap<String, GraphNodeMatch>,
    pub row_edge_match_bindings: HashMap<String, GraphRelationshipMatch>,
    pub row_edge_bindings: HashMap<String, CypherRowProducedEdgeBinding>,
    pub row_path_bindings: HashMap<String, CypherRowProducedPathBinding>,
    pub return_clause: CypherReturnClause,
}

pub fn cypher_mutation_plan_with_return_options(
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

pub fn sail_cypher_mutation_plan_with_return_options(
    cypher: &str,
    options: CypherMutationOptions,
) -> Result<CypherPlannedMutationWithReturn> {
    cypher_mutation_plan_with_return_options(cypher, options)
}

#[derive(Default)]
pub(crate) struct CypherMutationPlanner {
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
        let (path_variable, pattern) = parse_path_binding(pattern, "MATCH DELETE")?;

        if find_unquoted_sequence(pattern, "->").is_some() {
            let mut parsed = self.parse_edge_match_pattern(pattern)?;
            apply_edge_where_predicates(&mut parsed, where_predicates, "MATCH edge DELETE")?;
            let Some(edge_variable) = parsed.relationship.variable.as_ref() else {
                return Err(cypher_syntax(
                    "MATCH edge DELETE requires the relationship pattern to bind the DELETE target"
                        .to_string(),
                ));
            };
            let edge_variable = edge_variable.clone();
            let path_forces_endpoint_rows =
                path_variable.is_some() && targets.iter().any(|target| target == &edge_variable);
            let mut delete_relationship_rows = false;
            let mut row_endpoint_targets = Vec::new();
            let mut resolved_endpoint_ops = Vec::new();
            for target in &targets {
                if target == &edge_variable {
                    delete_relationship_rows = true;
                } else if parsed.from.variable.as_deref() == Some(target.as_str()) {
                    if path_forces_endpoint_rows
                        || self.endpoint_delete_requires_relationship_rows(&parsed.from)?
                    {
                        row_endpoint_targets.push(GraphRelationshipEndpoint::From);
                    } else {
                        resolved_endpoint_ops.extend(
                            self.lower_match_edge_endpoint_delete(
                                &parsed,
                                GraphRelationshipEndpoint::From,
                                target,
                            )?
                            .operations,
                        );
                    }
                } else if parsed.to.variable.as_deref() == Some(target.as_str()) {
                    if path_forces_endpoint_rows
                        || self.endpoint_delete_requires_relationship_rows(&parsed.to)?
                    {
                        row_endpoint_targets.push(GraphRelationshipEndpoint::To);
                    } else {
                        resolved_endpoint_ops.extend(
                            self.lower_match_edge_endpoint_delete(
                                &parsed,
                                GraphRelationshipEndpoint::To,
                                target,
                            )?
                            .operations,
                        );
                    }
                } else {
                    return Err(cypher_syntax(format!(
                        "MATCH edge DELETE target '{target}' is not bound by the relationship pattern"
                    )));
                }
            }
            if path_variable.is_some()
                && (!delete_relationship_rows || !resolved_endpoint_ops.is_empty())
            {
                return Err(cypher_syntax(
                    "MATCH edge DELETE path variables require deleting the matched relationship variable and do not support separately resolved endpoint deletes",
                ));
            }

            if !row_endpoint_targets.is_empty() {
                row_endpoint_targets.sort_by_key(|endpoint| match endpoint {
                    GraphRelationshipEndpoint::From => 0,
                    GraphRelationshipEndpoint::To => 1,
                });
                row_endpoint_targets.dedup();
                let relationship =
                    self.relationship_match_from_pattern(parsed.clone(), "MATCH edge DELETE")?;
                if path_variable.is_some() {
                    for endpoint in &row_endpoint_targets {
                        let node = match endpoint {
                            GraphRelationshipEndpoint::From => &parsed.from,
                            GraphRelationshipEndpoint::To => &parsed.to,
                        };
                        self.promote_node_variable_to_row_binding(node)?;
                    }
                }
                self.bind_path_variable_for_edge_match(
                    path_variable.as_deref(),
                    &parsed,
                    "MATCH edge DELETE",
                )?;
                if self.bind_delete_return_rows || path_variable.is_some() {
                    self.bind_row_edge_match_variable(&edge_variable, &relationship)?;
                }
                let mut operations = vec![GraphMutationPlanOp::DeleteRelationshipRows {
                    relationship,
                    delete_edges: delete_relationship_rows,
                    endpoint_nodes: row_endpoint_targets.clone(),
                    target_count: usize::from(delete_relationship_rows)
                        + row_endpoint_targets.len(),
                    cardinality: GraphMutationCardinality::BoundedMany,
                }];
                operations.extend(resolved_endpoint_ops);
                return Ok(GraphMutationPlan::new(operations));
            }

            let mut plan = GraphMutationPlan::default();
            for target in targets {
                if target == edge_variable {
                    plan.operations.extend(
                        self.lower_match_edge_delete(
                            parsed.clone(),
                            &edge_variable,
                            path_variable.as_deref(),
                        )?
                        .operations,
                    );
                } else if parsed.from.variable.as_deref() == Some(target.as_str()) {
                    plan.operations.extend(
                        self.lower_match_edge_endpoint_delete(
                            &parsed,
                            GraphRelationshipEndpoint::From,
                            &target,
                        )?
                        .operations,
                    );
                } else if parsed.to.variable.as_deref() == Some(target.as_str()) {
                    plan.operations.extend(
                        self.lower_match_edge_endpoint_delete(
                            &parsed,
                            GraphRelationshipEndpoint::To,
                            &target,
                        )?
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

        if path_variable.is_some() {
            return Err(cypher_syntax(
                "MATCH node DELETE does not support path variables",
            ));
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
        path_variable: Option<&str>,
    ) -> Result<GraphMutationPlan> {
        let from_id = self.resolved_endpoint_id(&parsed.from)?;
        let to_id = self.resolved_endpoint_id(&parsed.to)?;
        let edge_id = optional_string_prop(&parsed.relationship.props, "id");
        if path_variable.is_none()
            && let (Some(from), Some(to), None) = (from_id, to_id, edge_id)
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
        self.bind_path_variable_for_edge_match(path_variable, &parsed, "MATCH edge DELETE")?;
        let relationship = self.relationship_match_from_pattern(parsed, "MATCH edge DELETE")?;
        if self.bind_delete_return_rows || path_variable.is_some() {
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
        parsed: &ParsedCypherEdgeMatch,
        endpoint: GraphRelationshipEndpoint,
        _target: &str,
    ) -> Result<GraphMutationPlan> {
        let node = match endpoint {
            GraphRelationshipEndpoint::From => &parsed.from,
            GraphRelationshipEndpoint::To => &parsed.to,
        };
        if !node.predicates.is_empty() {
            let relationship =
                self.relationship_match_from_pattern(parsed.clone(), "MATCH edge DELETE")?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::DeleteRelationshipRows {
                    relationship,
                    delete_edges: false,
                    endpoint_nodes: vec![endpoint],
                    target_count: 1,
                    cardinality: GraphMutationCardinality::BoundedMany,
                },
            ]));
        }
        let Some(id) = self.resolved_endpoint_id(node)? else {
            let relationship =
                self.relationship_match_from_pattern(parsed.clone(), "MATCH edge DELETE")?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::DeleteRelationshipRows {
                    relationship,
                    delete_edges: false,
                    endpoint_nodes: vec![endpoint],
                    target_count: 1,
                    cardinality: GraphMutationCardinality::BoundedMany,
                },
            ]));
        };
        self.bind_node_variable(node, &id)?;
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::DeleteNode(id),
        ]))
    }

    fn endpoint_delete_requires_relationship_rows(
        &mut self,
        node: &ParsedCypherNode,
    ) -> Result<bool> {
        if !node.predicates.is_empty() {
            return Ok(true);
        }
        Ok(self.resolved_endpoint_id(node)?.is_none())
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
        let (path_variable, pattern) = parse_path_binding(pattern, "MATCH SET")?;

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
                    self.bind_path_variable_for_edge_match(
                        path_variable.as_deref(),
                        &parsed,
                        "MATCH edge SET",
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
                    self.bind_path_variable_for_edge_match(
                        path_variable.as_deref(),
                        &parsed,
                        "MATCH edge SET",
                    )?;
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
                    self.bind_path_variable_for_edge_match(
                        path_variable.as_deref(),
                        &parsed,
                        "MATCH edge SET",
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
                self.bind_path_variable_for_edge_match(
                    path_variable.as_deref(),
                    &parsed,
                    "MATCH edge SET",
                )?;
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
                self.bind_path_variable_for_edge_match(
                    path_variable.as_deref(),
                    &parsed,
                    "MATCH edge SET",
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
            self.bind_path_variable_for_edge_match(
                path_variable.as_deref(),
                &parsed,
                "MATCH edge SET",
            )?;
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
        let (path_variable, pattern) = parse_path_binding(pattern, "MATCH REMOVE")?;
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
                self.bind_path_variable_for_edge_match(
                    path_variable.as_deref(),
                    &parsed,
                    "MATCH edge REMOVE",
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
            self.bind_path_variable_for_edge_match(
                path_variable.as_deref(),
                &parsed,
                "MATCH edge REMOVE",
            )?;
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

    fn bind_path_variable_for_edge_match(
        &mut self,
        path_variable: Option<&str>,
        parsed: &ParsedCypherEdgeMatch,
        context: &str,
    ) -> Result<()> {
        let Some(path_variable) = path_variable else {
            return Ok(());
        };
        let Some(from_variable) = parsed.from.variable.as_ref() else {
            return Err(cypher_syntax(format!(
                "{context} path variables require the source node pattern to bind a variable"
            )));
        };
        let Some(edge_variable) = parsed.relationship.variable.as_ref() else {
            return Err(cypher_syntax(format!(
                "{context} path variables require the relationship pattern to bind a variable"
            )));
        };
        let Some(to_variable) = parsed.to.variable.as_ref() else {
            return Err(cypher_syntax(format!(
                "{context} path variables require the destination node pattern to bind a variable"
            )));
        };
        if !self.node_bindings.contains_key(from_variable) {
            self.bind_row_node_variable(&parsed.from)?;
        }
        if !self.node_bindings.contains_key(to_variable) {
            self.bind_row_node_variable(&parsed.to)?;
        }
        self.bind_row_path_variable(
            path_variable,
            CypherRowProducedPathBinding {
                from_variable: from_variable.clone(),
                edge_variable: edge_variable.clone(),
                to_variable: to_variable.clone(),
            },
        )
    }

    fn promote_node_variable_to_row_binding(&mut self, node: &ParsedCypherNode) -> Result<()> {
        let Some(variable) = &node.variable else {
            return Ok(());
        };
        self.node_bindings.remove(variable);
        self.bind_row_node_variable(node)
    }
}

pub fn cypher_syntax(message: impl Into<String>) -> GrustError {
    GrustError::CypherSyntax(message.into())
}

pub fn cypher_unresolved_identity(message: impl Into<String>) -> GrustError {
    GrustError::CypherUnresolvedIdentity(message.into())
}

pub fn cypher_unsupported_cardinality(message: impl Into<String>) -> GrustError {
    GrustError::CypherUnsupportedCardinality(message.into())
}

pub mod cypher_parser {
    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum CypherStatement<'a> {
        Match(&'a str),
        Create(&'a str),
        Merge(&'a str),
        Delete(&'a str),
    }

    pub fn classify_statement(cypher: &str) -> Result<CypherStatement<'_>> {
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

pub fn cypher_execution_error(error: GrustError) -> GrustError {
    match error {
        GrustError::CypherSyntax(_)
        | GrustError::CypherUnresolvedIdentity(_)
        | GrustError::CypherUnsupportedCardinality(_)
        | GrustError::CypherExecution(_) => error,
        other => GrustError::CypherExecution(other.to_string()),
    }
}

pub(crate) fn parse_cypher_ddl_statement(statement: &str) -> Result<CypherDdlStatement> {
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

pub(crate) fn parse_create_constraint(rest: &str) -> Result<CypherDdlStatement> {
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

pub(crate) fn parse_drop_constraint(rest: &str) -> Result<CypherDdlStatement> {
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
pub(crate) fn constraint_name(header: &str) -> Result<Option<String>> {
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
pub(crate) fn parse_constraint_pattern(pattern: &str) -> Result<(bool, String, Label)> {
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
pub(crate) fn parse_constraint_var_label(body: &str) -> Result<(String, Label)> {
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
pub(crate) fn parse_constraint_predicate(predicate: &str, pattern_variable: &str) -> Result<(bool, String)> {
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
pub fn unique_node_conflict<'a>(
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
pub fn unique_edge_conflict(
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

pub(crate) fn split_cypher_statements(cypher: &str) -> Result<Vec<&str>> {
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

pub(crate) fn strip_cypher_comments(cypher: &str) -> Result<String> {
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
pub(crate) struct ParsedCypherNode {
    variable: Option<String>,
    label: Option<Label>,
    props: Props,
    predicates: Vec<GraphPropertyPredicate>,
}

#[derive(Debug)]
pub(crate) struct ParsedCypherEdge {
    from_id: NodeId,
    to_id: NodeId,
    edge: Edge,
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedCypherEdgeMatch {
    from: ParsedCypherNode,
    relationship: ParsedCypherRelationship,
    to: ParsedCypherNode,
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedCypherRelationship {
    variable: Option<String>,
    label: Label,
    props: Props,
    predicates: Vec<GraphPropertyPredicate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CypherBoundEdgeIdentity {
    pub from: NodeId,
    pub label: Label,
    pub to: NodeId,
    pub id: Option<EdgeId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherRowProducedEdgeBinding {
    pub kind: GraphMutationPlanKind,
    pub from_variable: String,
    pub from: GraphNodeMatch,
    pub to_variable: String,
    pub to: GraphNodeMatch,
    pub label: Label,
    pub props: Props,
    pub edge_id_policy: GraphRowEdgeIdPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CypherRowProducedPathBinding {
    pub from_variable: String,
    pub edge_variable: String,
    pub to_variable: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ParsedWherePredicate {
    target: String,
    predicate: GraphPropertyPredicate,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CypherWhereBoolean<'a> {
    Predicate(&'a str),
    Not(Box<CypherWhereBoolean<'a>>),
    And(Vec<CypherWhereBoolean<'a>>),
    Or(Vec<CypherWhereBoolean<'a>>),
}

pub(crate) fn parse_cypher_node_pattern<'a>(
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

pub(crate) fn parse_cypher_node_body(
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

pub(crate) fn parse_optional_cypher_variable(value: &str) -> Result<Option<String>> {
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

pub(crate) fn parse_required_cypher_variable(value: &str, context: &str) -> Result<String> {
    parse_optional_cypher_variable(value)?
        .ok_or_else(|| GrustError::Unsupported(format!("{context} requires a variable name")))
}

pub(crate) fn is_cypher_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

pub(crate) fn parse_cypher_relationship(
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

pub(crate) fn validate_optional_edge_id_property(props: &Props) -> Result<()> {
    edge_id_from_props(props).map(|_| ())
}

pub fn edge_id_from_props(props: &Props) -> Result<Option<String>> {
    match props.get("id") {
        Some(Value::String(id)) => Ok(Some(id.clone())),
        Some(_) => Err(cypher_syntax(
            "relationship id property must be a string literal",
        )),
        None => Ok(None),
    }
}

pub(crate) fn match_node_cardinality(node: &ParsedCypherNode) -> GraphMutationCardinality {
    if node.label.is_some() || !node.props.is_empty() || !node.predicates.is_empty() {
        GraphMutationCardinality::BoundedMany
    } else {
        GraphMutationCardinality::UnboundedMany
    }
}

pub(crate) fn split_match_where<'a>(
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
    let ast = parse_where_boolean_ast(where_clause)?;
    let mut predicates = lower_where_boolean_ast(&ast, parameters)?;
    canonicalize_where_predicates(&mut predicates)?;
    Ok((match_pattern, predicates))
}

pub(crate) fn canonicalize_where_predicates(predicates: &mut Vec<ParsedWherePredicate>) -> Result<()> {
    dedupe_where_predicates(predicates);
    merge_where_membership_predicates(predicates)?;
    merge_where_equality_membership_predicates(predicates)?;
    merge_where_order_predicates(predicates);
    merge_where_equality_order_predicates(predicates);
    merge_where_membership_order_predicates(predicates)?;
    Ok(())
}

pub(crate) fn dedupe_where_predicates(predicates: &mut Vec<ParsedWherePredicate>) {
    let mut deduped = Vec::with_capacity(predicates.len());
    for predicate in predicates.drain(..) {
        if !deduped.contains(&predicate) {
            deduped.push(predicate);
        }
    }
    *predicates = deduped;
}

pub(crate) fn merge_where_membership_predicates(predicates: &mut Vec<ParsedWherePredicate>) -> Result<()> {
    let mut merged = Vec::with_capacity(predicates.len());
    for predicate in predicates.drain(..) {
        if !matches!(
            predicate.predicate.op,
            GraphPredicateOp::In | GraphPredicateOp::NotIn
        ) {
            merged.push(predicate);
            continue;
        }

        let Some(existing) = merged
            .iter_mut()
            .find(|existing: &&mut ParsedWherePredicate| {
                existing.target == predicate.target
                    && existing.predicate.key == predicate.predicate.key
                    && existing.predicate.op == predicate.predicate.op
            })
        else {
            merged.push(predicate);
            continue;
        };

        match predicate.predicate.op {
            GraphPredicateOp::In => {
                let intersection = intersect_membership_values(
                    &existing.predicate.value,
                    &predicate.predicate.value,
                )?;
                existing.predicate.value = Value::Json(serde_json::Value::Array(intersection));
            }
            GraphPredicateOp::NotIn => {
                let union =
                    union_membership_values(&existing.predicate.value, &predicate.predicate.value)?;
                existing.predicate.value = Value::Json(serde_json::Value::Array(union));
            }
            _ => unreachable!(),
        }
    }
    *predicates = merged;
    Ok(())
}

pub(crate) fn intersect_membership_values(left: &Value, right: &Value) -> Result<Vec<serde_json::Value>> {
    let right_values = cypher_in_predicate_values(right)?
        .into_iter()
        .map(|value| value.to_json())
        .collect::<Vec<_>>();
    let mut intersection = Vec::new();
    for value in cypher_in_predicate_values(left)? {
        let value = value.to_json();
        if right_values.contains(&value) {
            push_unique(&mut intersection, value);
        }
    }
    Ok(intersection)
}

pub(crate) fn union_membership_values(left: &Value, right: &Value) -> Result<Vec<serde_json::Value>> {
    let mut union = Vec::new();
    for value in cypher_in_predicate_values(left)?
        .into_iter()
        .chain(cypher_in_predicate_values(right)?)
    {
        push_unique(&mut union, value.to_json());
    }
    Ok(union)
}

pub(crate) fn merge_where_equality_membership_predicates(
    predicates: &mut Vec<ParsedWherePredicate>,
) -> Result<()> {
    let mut merged = Vec::with_capacity(predicates.len());
    for predicate in predicates.drain(..) {
        if !matches!(
            predicate.predicate.op,
            GraphPredicateOp::Equal
                | GraphPredicateOp::NotEqual
                | GraphPredicateOp::In
                | GraphPredicateOp::NotIn
        ) {
            merged.push(predicate);
            continue;
        }

        let Some(existing) = merged
            .iter_mut()
            .find(|existing: &&mut ParsedWherePredicate| {
                existing.target == predicate.target
                    && existing.predicate.key == predicate.predicate.key
                    && matches!(
                        existing.predicate.op,
                        GraphPredicateOp::Equal
                            | GraphPredicateOp::NotEqual
                            | GraphPredicateOp::In
                            | GraphPredicateOp::NotIn
                    )
            })
        else {
            merged.push(predicate);
            continue;
        };

        if !merge_where_equality_membership_pair(existing, &predicate)? {
            merged.push(predicate);
        }
    }
    *predicates = merged;
    Ok(())
}

pub(crate) fn merge_where_equality_membership_pair(
    existing: &mut ParsedWherePredicate,
    incoming: &ParsedWherePredicate,
) -> Result<bool> {
    match (existing.predicate.op, incoming.predicate.op) {
        (GraphPredicateOp::Equal, GraphPredicateOp::Equal) => {
            if existing.predicate.value != incoming.predicate.value {
                set_where_no_match(existing);
            }
        }
        (GraphPredicateOp::Equal, GraphPredicateOp::In) => {
            if !membership_contains_value(&incoming.predicate.value, &existing.predicate.value)? {
                set_where_no_match(existing);
            }
        }
        (GraphPredicateOp::In, GraphPredicateOp::Equal) => {
            if membership_contains_value(&existing.predicate.value, &incoming.predicate.value)? {
                existing.predicate.op = GraphPredicateOp::Equal;
                existing.predicate.value = incoming.predicate.value.clone();
            } else {
                set_where_no_match(existing);
            }
        }
        (GraphPredicateOp::Equal, GraphPredicateOp::NotIn) => {
            if membership_contains_value(&incoming.predicate.value, &existing.predicate.value)? {
                set_where_no_match(existing);
            }
        }
        (GraphPredicateOp::NotIn, GraphPredicateOp::Equal) => {
            if membership_contains_value(&existing.predicate.value, &incoming.predicate.value)? {
                set_where_no_match(existing);
            } else {
                existing.predicate.op = GraphPredicateOp::Equal;
                existing.predicate.value = incoming.predicate.value.clone();
            }
        }
        (GraphPredicateOp::In, GraphPredicateOp::NotIn) => {
            let values =
                difference_membership_values(&existing.predicate.value, &incoming.predicate.value)?;
            existing.predicate.value = Value::Json(serde_json::Value::Array(values));
        }
        (GraphPredicateOp::NotIn, GraphPredicateOp::In) => {
            let values =
                difference_membership_values(&incoming.predicate.value, &existing.predicate.value)?;
            existing.predicate.op = GraphPredicateOp::In;
            existing.predicate.value = Value::Json(serde_json::Value::Array(values));
        }
        (GraphPredicateOp::Equal, GraphPredicateOp::NotEqual) => {
            if existing.predicate.value == incoming.predicate.value {
                set_where_no_match(existing);
            }
        }
        (GraphPredicateOp::NotEqual, GraphPredicateOp::Equal) => {
            if existing.predicate.value == incoming.predicate.value {
                set_where_no_match(existing);
            } else {
                existing.predicate.op = GraphPredicateOp::Equal;
                existing.predicate.value = incoming.predicate.value.clone();
            }
        }
        (GraphPredicateOp::NotEqual, GraphPredicateOp::NotEqual) => {
            let Some(incoming_value) = membership_item_json(&incoming.predicate.value) else {
                return Ok(false);
            };
            let Some(existing_value) = membership_item_json(&existing.predicate.value) else {
                return Ok(false);
            };
            existing.predicate.op = GraphPredicateOp::NotIn;
            existing.predicate.value = Value::Json(serde_json::Value::Array(vec![
                existing_value,
                incoming_value,
            ]));
        }
        (GraphPredicateOp::In, GraphPredicateOp::NotEqual) => {
            let Some(excluded) = membership_item_json(&incoming.predicate.value) else {
                return Ok(false);
            };
            let values = difference_membership_json_values(&existing.predicate.value, &[excluded])?;
            existing.predicate.value = Value::Json(serde_json::Value::Array(values));
        }
        (GraphPredicateOp::NotEqual, GraphPredicateOp::In) => {
            let Some(excluded) = membership_item_json(&existing.predicate.value) else {
                return Ok(false);
            };
            let values = difference_membership_json_values(&incoming.predicate.value, &[excluded])?;
            existing.predicate.op = GraphPredicateOp::In;
            existing.predicate.value = Value::Json(serde_json::Value::Array(values));
        }
        (GraphPredicateOp::NotIn, GraphPredicateOp::NotEqual) => {
            let Some(value) = membership_item_json(&incoming.predicate.value) else {
                return Ok(false);
            };
            let union = union_membership_json_values(&existing.predicate.value, value)?;
            existing.predicate.value = Value::Json(serde_json::Value::Array(union));
        }
        (GraphPredicateOp::NotEqual, GraphPredicateOp::NotIn) => {
            let Some(value) = membership_item_json(&existing.predicate.value) else {
                return Ok(false);
            };
            let union = union_membership_json_values(&incoming.predicate.value, value)?;
            existing.predicate.op = GraphPredicateOp::NotIn;
            existing.predicate.value = Value::Json(serde_json::Value::Array(union));
        }
        (GraphPredicateOp::In, GraphPredicateOp::In)
        | (GraphPredicateOp::NotIn, GraphPredicateOp::NotIn) => {
            unreachable!("same-op membership predicates are merged before equality folding")
        }
        _ => unreachable!("only equality and membership predicates reach this merge path"),
    }
    Ok(true)
}

pub(crate) fn membership_contains_value(membership: &Value, value: &Value) -> Result<bool> {
    let value = value.to_json();
    Ok(cypher_in_predicate_values(membership)?
        .into_iter()
        .any(|candidate| candidate.to_json() == value))
}

pub(crate) fn difference_membership_values(
    positive: &Value,
    excluded: &Value,
) -> Result<Vec<serde_json::Value>> {
    let excluded = cypher_in_predicate_values(excluded)?
        .into_iter()
        .map(|value| value.to_json())
        .collect::<Vec<_>>();
    let mut difference = Vec::new();
    for value in cypher_in_predicate_values(positive)? {
        let value = value.to_json();
        if !excluded.contains(&value) {
            push_unique(&mut difference, value);
        }
    }
    Ok(difference)
}

pub(crate) fn difference_membership_json_values(
    positive: &Value,
    excluded: &[serde_json::Value],
) -> Result<Vec<serde_json::Value>> {
    let mut difference = Vec::new();
    for value in cypher_in_predicate_values(positive)? {
        let value = value.to_json();
        if !excluded.contains(&value) {
            push_unique(&mut difference, value);
        }
    }
    Ok(difference)
}

pub(crate) fn union_membership_json_values(
    membership: &Value,
    value: serde_json::Value,
) -> Result<Vec<serde_json::Value>> {
    let mut union = cypher_in_predicate_values(membership)?
        .into_iter()
        .map(|value| value.to_json())
        .collect::<Vec<_>>();
    push_unique(&mut union, value);
    Ok(union)
}

pub(crate) fn membership_item_json(value: &Value) -> Option<serde_json::Value> {
    validate_cypher_in_item(value).ok()?;
    Some(value.to_json())
}

pub(crate) fn set_where_no_match(predicate: &mut ParsedWherePredicate) {
    predicate.predicate.op = GraphPredicateOp::In;
    predicate.predicate.value = Value::Json(serde_json::Value::Array(Vec::new()));
}

pub(crate) fn is_where_no_match(predicate: &ParsedWherePredicate) -> bool {
    predicate.predicate.op == GraphPredicateOp::In
        && matches!(
            &predicate.predicate.value,
            Value::Json(serde_json::Value::Array(values)) if values.is_empty()
        )
}

pub(crate) fn merge_where_order_predicates(predicates: &mut Vec<ParsedWherePredicate>) {
    let mut merged = Vec::with_capacity(predicates.len());
    for predicate in predicates.drain(..) {
        if !is_order_predicate_op(predicate.predicate.op) {
            merged.push(predicate);
            continue;
        }

        let Some(existing) = merged
            .iter_mut()
            .find(|existing: &&mut ParsedWherePredicate| {
                existing.target == predicate.target
                    && existing.predicate.key == predicate.predicate.key
                    && is_order_predicate_op(existing.predicate.op)
            })
        else {
            merged.push(predicate);
            continue;
        };

        if !merge_where_order_pair(existing, &predicate) {
            merged.push(predicate);
        }
    }
    *predicates = merged;
}

pub(crate) fn merge_where_equality_order_predicates(predicates: &mut Vec<ParsedWherePredicate>) {
    let mut merged = Vec::with_capacity(predicates.len());
    for predicate in predicates.drain(..) {
        if !matches!(predicate.predicate.op, GraphPredicateOp::Equal)
            && !is_order_predicate_op(predicate.predicate.op)
        {
            merged.push(predicate);
            continue;
        }

        let Some(existing) = merged
            .iter_mut()
            .find(|existing: &&mut ParsedWherePredicate| {
                existing.target == predicate.target
                    && existing.predicate.key == predicate.predicate.key
                    && (matches!(existing.predicate.op, GraphPredicateOp::Equal)
                        || is_order_predicate_op(existing.predicate.op))
            })
        else {
            merged.push(predicate);
            continue;
        };

        if !merge_where_equality_order_pair(existing, &predicate) {
            merged.push(predicate);
        }
    }
    *predicates = merged;
}

pub(crate) fn merge_where_equality_order_pair(
    existing: &mut ParsedWherePredicate,
    incoming: &ParsedWherePredicate,
) -> bool {
    match (existing.predicate.op, incoming.predicate.op) {
        (GraphPredicateOp::Equal, op) if is_order_predicate_op(op) => {
            if !equality_satisfies_order_bound(
                &existing.predicate.value,
                op,
                &incoming.predicate.value,
            ) {
                set_where_no_match(existing);
            }
            true
        }
        (op, GraphPredicateOp::Equal) if is_order_predicate_op(op) => {
            if equality_satisfies_order_bound(
                &incoming.predicate.value,
                op,
                &existing.predicate.value,
            ) {
                existing.predicate.op = GraphPredicateOp::Equal;
                existing.predicate.value = incoming.predicate.value.clone();
            } else {
                set_where_no_match(existing);
            }
            true
        }
        _ => false,
    }
}

pub(crate) fn equality_satisfies_order_bound(
    equality_value: &Value,
    bound_op: GraphPredicateOp,
    bound_value: &Value,
) -> bool {
    compare_where_order_values(equality_value, bound_value).is_some_and(|ordering| match bound_op {
        GraphPredicateOp::GreaterThan => ordering.is_gt(),
        GraphPredicateOp::GreaterThanOrEqual => ordering.is_gt() || ordering.is_eq(),
        GraphPredicateOp::LessThan => ordering.is_lt(),
        GraphPredicateOp::LessThanOrEqual => ordering.is_lt() || ordering.is_eq(),
        _ => false,
    })
}

pub(crate) fn order_bound_excludes_value(
    bound_op: GraphPredicateOp,
    bound_value: &Value,
    value: &Value,
) -> bool {
    compare_where_order_values(value, bound_value)
        .map(|_| !equality_satisfies_order_bound(value, bound_op, bound_value))
        .unwrap_or(false)
}

pub(crate) fn merge_where_membership_order_predicates(
    predicates: &mut Vec<ParsedWherePredicate>,
) -> Result<()> {
    loop {
        let before_len = predicates.len();
        merge_where_membership_order_predicates_once(predicates)?;
        if predicates.len() == before_len {
            break;
        }
    }
    Ok(())
}

pub(crate) fn merge_where_membership_order_predicates_once(
    predicates: &mut Vec<ParsedWherePredicate>,
) -> Result<()> {
    let mut merged = Vec::with_capacity(predicates.len());
    for predicate in predicates.drain(..) {
        if predicate.predicate.op != GraphPredicateOp::In
            && !is_order_predicate_op(predicate.predicate.op)
        {
            merged.push(predicate);
            continue;
        }

        let Some(existing) = merged
            .iter_mut()
            .find(|existing: &&mut ParsedWherePredicate| {
                existing.target == predicate.target
                    && existing.predicate.key == predicate.predicate.key
                    && (existing.predicate.op == GraphPredicateOp::In
                        || is_order_predicate_op(existing.predicate.op))
            })
        else {
            merged.push(predicate);
            continue;
        };

        if !merge_where_membership_order_pair(existing, &predicate)? {
            merged.push(predicate);
        }
    }
    *predicates = merged;
    Ok(())
}

pub(crate) fn merge_where_membership_order_pair(
    existing: &mut ParsedWherePredicate,
    incoming: &ParsedWherePredicate,
) -> Result<bool> {
    match (existing.predicate.op, incoming.predicate.op) {
        (GraphPredicateOp::In, op) if is_order_predicate_op(op) => {
            let values = filter_membership_values_by_order_bound(
                &existing.predicate.value,
                op,
                &incoming.predicate.value,
            )?;
            existing.predicate.value = Value::Json(serde_json::Value::Array(values));
            Ok(true)
        }
        (op, GraphPredicateOp::In) if is_order_predicate_op(op) => {
            let values = filter_membership_values_by_order_bound(
                &incoming.predicate.value,
                op,
                &existing.predicate.value,
            )?;
            existing.predicate.op = GraphPredicateOp::In;
            existing.predicate.value = Value::Json(serde_json::Value::Array(values));
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub(crate) fn filter_membership_values_by_order_bound(
    membership: &Value,
    bound_op: GraphPredicateOp,
    bound_value: &Value,
) -> Result<Vec<serde_json::Value>> {
    let mut filtered = Vec::new();
    for value in cypher_in_predicate_values(membership)? {
        if equality_satisfies_order_bound(&value, bound_op, bound_value) {
            push_unique(&mut filtered, value.to_json());
        }
    }
    Ok(filtered)
}

pub(crate) fn is_order_predicate_op(op: GraphPredicateOp) -> bool {
    matches!(
        op,
        GraphPredicateOp::GreaterThan
            | GraphPredicateOp::GreaterThanOrEqual
            | GraphPredicateOp::LessThan
            | GraphPredicateOp::LessThanOrEqual
    )
}

pub(crate) fn is_positive_string_predicate_op(op: GraphPredicateOp) -> bool {
    matches!(
        op,
        GraphPredicateOp::StartsWith | GraphPredicateOp::EndsWith | GraphPredicateOp::Contains
    )
}

pub(crate) fn where_predicate_uses_only_string_values(predicate: &ParsedWherePredicate) -> Result<bool> {
    match predicate.predicate.op {
        GraphPredicateOp::Equal => Ok(matches!(predicate.predicate.value, Value::String(_))),
        GraphPredicateOp::In => {
            membership_values_all_satisfy(&predicate.predicate.value, |value| {
                Ok(matches!(value, Value::String(_)))
            })
        }
        _ => Ok(false),
    }
}

pub(crate) fn merge_where_order_pair(
    existing: &mut ParsedWherePredicate,
    incoming: &ParsedWherePredicate,
) -> bool {
    match (
        order_bound_kind(existing.predicate.op),
        order_bound_kind(incoming.predicate.op),
    ) {
        (Some(OrderBoundKind::Lower), Some(OrderBoundKind::Lower)) => {
            if order_lower_is_stricter(
                incoming.predicate.op,
                &incoming.predicate.value,
                existing.predicate.op,
                &existing.predicate.value,
            ) {
                existing.predicate.op = incoming.predicate.op;
                existing.predicate.value = incoming.predicate.value.clone();
            }
            true
        }
        (Some(OrderBoundKind::Upper), Some(OrderBoundKind::Upper)) => {
            if order_upper_is_stricter(
                incoming.predicate.op,
                &incoming.predicate.value,
                existing.predicate.op,
                &existing.predicate.value,
            ) {
                existing.predicate.op = incoming.predicate.op;
                existing.predicate.value = incoming.predicate.value.clone();
            }
            true
        }
        (Some(OrderBoundKind::Lower), Some(OrderBoundKind::Upper)) => {
            if order_bounds_are_contradictory(
                existing.predicate.op,
                &existing.predicate.value,
                incoming.predicate.op,
                &incoming.predicate.value,
            ) {
                set_where_no_match(existing);
                true
            } else {
                false
            }
        }
        (Some(OrderBoundKind::Upper), Some(OrderBoundKind::Lower)) => {
            if order_bounds_are_contradictory(
                incoming.predicate.op,
                &incoming.predicate.value,
                existing.predicate.op,
                &existing.predicate.value,
            ) {
                set_where_no_match(existing);
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OrderBoundKind {
    Lower,
    Upper,
}

pub(crate) fn order_bound_kind(op: GraphPredicateOp) -> Option<OrderBoundKind> {
    match op {
        GraphPredicateOp::GreaterThan | GraphPredicateOp::GreaterThanOrEqual => {
            Some(OrderBoundKind::Lower)
        }
        GraphPredicateOp::LessThan | GraphPredicateOp::LessThanOrEqual => {
            Some(OrderBoundKind::Upper)
        }
        _ => None,
    }
}

pub(crate) fn order_lower_is_stricter(
    candidate_op: GraphPredicateOp,
    candidate_value: &Value,
    current_op: GraphPredicateOp,
    current_value: &Value,
) -> bool {
    match compare_where_order_values(candidate_value, current_value) {
        Some(std::cmp::Ordering::Greater) => true,
        Some(std::cmp::Ordering::Equal) => {
            candidate_op == GraphPredicateOp::GreaterThan
                && current_op == GraphPredicateOp::GreaterThanOrEqual
        }
        _ => false,
    }
}

pub(crate) fn order_upper_is_stricter(
    candidate_op: GraphPredicateOp,
    candidate_value: &Value,
    current_op: GraphPredicateOp,
    current_value: &Value,
) -> bool {
    match compare_where_order_values(candidate_value, current_value) {
        Some(std::cmp::Ordering::Less) => true,
        Some(std::cmp::Ordering::Equal) => {
            candidate_op == GraphPredicateOp::LessThan
                && current_op == GraphPredicateOp::LessThanOrEqual
        }
        _ => false,
    }
}

pub(crate) fn order_bounds_are_contradictory(
    lower_op: GraphPredicateOp,
    lower_value: &Value,
    upper_op: GraphPredicateOp,
    upper_value: &Value,
) -> bool {
    match compare_where_order_values(lower_value, upper_value) {
        Some(std::cmp::Ordering::Greater) => true,
        Some(std::cmp::Ordering::Equal) => {
            lower_op == GraphPredicateOp::GreaterThan || upper_op == GraphPredicateOp::LessThan
        }
        _ => false,
    }
}

pub(crate) fn compare_where_order_values(left: &Value, right: &Value) -> Option<std::cmp::Ordering> {
    match (left, right) {
        (Value::Int(left), Value::Int(right)) => Some(left.cmp(right)),
        (Value::Float(left), Value::Float(right)) => left.partial_cmp(right),
        (Value::Int(left), Value::Float(right)) => (*left as f64).partial_cmp(right),
        (Value::Float(left), Value::Int(right)) => left.partial_cmp(&(*right as f64)),
        (Value::String(left), Value::String(right)) => Some(left.cmp(right)),
        _ => None,
    }
}

pub(crate) fn split_top_level_and(value: &str) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    let mut rest = value.trim();
    while let Some(index) = find_top_level_keyword(rest, "AND")? {
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
    let mut flattened = Vec::new();
    for part in parts {
        let stripped = strip_enclosing_parentheses(part)?;
        if stripped != part.trim() {
            if find_top_level_keyword(stripped, "AND")?.is_some() {
                flattened.extend(split_top_level_and(stripped)?);
            } else {
                flattened.push(part);
            }
        } else {
            flattened.push(part);
        }
    }
    Ok(flattened)
}

pub(crate) fn split_top_level_or(value: &str) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    let mut rest = value.trim();
    while let Some(index) = find_top_level_keyword(rest, "OR")? {
        let part = rest[..index].trim();
        if part.is_empty() {
            return Err(cypher_syntax("empty predicate before OR".to_string()));
        }
        parts.push(part);
        rest = rest[index + "OR".len()..].trim();
    }
    if rest.is_empty() {
        return Err(cypher_syntax("empty predicate after OR".to_string()));
    }
    parts.push(rest);
    let mut flattened = Vec::new();
    for part in parts {
        let stripped = strip_enclosing_parentheses(part)?;
        if stripped != part.trim() {
            flattened.extend(split_top_level_or(stripped)?);
        } else {
            flattened.push(part);
        }
    }
    Ok(flattened)
}

pub(crate) fn parse_where_boolean_ast(predicate: &str) -> Result<CypherWhereBoolean<'_>> {
    let predicate = strip_enclosing_parentheses(predicate.trim())?;
    if predicate.is_empty() {
        return Err(cypher_syntax("MATCH WHERE requires a predicate"));
    }
    let or_terms = split_top_level_or(predicate)?;
    if or_terms.len() > 1 {
        return Ok(CypherWhereBoolean::Or(
            or_terms
                .into_iter()
                .map(parse_where_boolean_ast)
                .collect::<Result<Vec<_>>>()?,
        ));
    }
    let and_terms = split_top_level_and(predicate)?;
    if and_terms.len() > 1 {
        return Ok(CypherWhereBoolean::And(
            and_terms
                .into_iter()
                .map(parse_where_boolean_ast)
                .collect::<Result<Vec<_>>>()?,
        ));
    }
    if let Some(after_not) = strip_leading_keyword(predicate, "NOT") {
        let inner = after_not.trim();
        if inner.is_empty() {
            return Err(cypher_syntax(
                "MATCH WHERE NOT requires a predicate".to_string(),
            ));
        }
        return Ok(CypherWhereBoolean::Not(Box::new(parse_where_boolean_ast(
            inner,
        )?)));
    }
    Ok(CypherWhereBoolean::Predicate(predicate))
}

pub(crate) fn lower_where_boolean_ast(
    predicate: &CypherWhereBoolean<'_>,
    parameters: &CypherParameters,
) -> Result<Vec<ParsedWherePredicate>> {
    match predicate {
        CypherWhereBoolean::Predicate(predicate) => {
            Ok(vec![parse_where_predicate(predicate, parameters)?])
        }
        CypherWhereBoolean::Not(inner) => lower_negated_where_boolean_ast(inner, parameters),
        CypherWhereBoolean::And(terms) => {
            let mut predicates = Vec::new();
            for term in terms {
                predicates.extend(lower_where_boolean_ast(term, parameters)?);
            }
            Ok(predicates)
        }
        CypherWhereBoolean::Or(terms) => {
            if terms.iter().any(where_boolean_contains_and) {
                return lower_where_boolean_or_branches(terms, parameters);
            }
            match parse_where_or_fold_ast_terms(terms, parameters, GraphPredicateOp::In) {
                Ok(predicate) => Ok(vec![predicate]),
                Err(_) => lower_where_boolean_or_branches(terms, parameters),
            }
        }
    }
}

pub(crate) fn lower_where_boolean_or_branches(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Vec<ParsedWherePredicate>> {
    let branches = terms
        .iter()
        .map(|term| {
            let mut branch = lower_where_boolean_ast(term, parameters)?;
            canonicalize_where_predicates(&mut branch)?;
            Ok(branch)
        })
        .collect::<Result<Vec<_>>>()?;
    lower_where_or_branches(branches)
}

pub(crate) fn lower_negated_where_boolean_ast(
    predicate: &CypherWhereBoolean<'_>,
    parameters: &CypherParameters,
) -> Result<Vec<ParsedWherePredicate>> {
    match predicate {
        CypherWhereBoolean::Predicate(predicate) => {
            let negated = format!("NOT {predicate}");
            Ok(vec![parse_where_predicate(&negated, parameters)?])
        }
        CypherWhereBoolean::Or(terms) => {
            if !terms.iter().any(where_boolean_contains_and)
                && let Ok(predicate) =
                    parse_where_or_fold_ast_terms(terms, parameters, GraphPredicateOp::NotIn)
            {
                return Ok(vec![predicate]);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) = lower_negated_null_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) = lower_negated_null_string_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) =
                    lower_negated_null_mixed_string_order_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) =
                    lower_negated_null_string_order_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) =
                    lower_negated_null_mixed_string_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) = lower_negated_null_order_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) =
                    lower_negated_null_mixed_order_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) = lower_negated_order_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) = lower_negated_string_order_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) = lower_negated_mixed_string_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) =
                    lower_negated_mixed_string_order_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) = lower_negated_mixed_order_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            let mut predicates = lower_where_boolean_or_branches(terms, parameters)?;
            if predicates.len() != 1 || is_where_no_match(&predicates[0]) {
                return Err(cypher_syntax(
                    "MATCH WHERE NOT over OR only supports collapsed bounded predicate terms",
                ));
            }
            predicates[0].predicate.op = inverted_graph_predicate_op(predicates[0].predicate.op);
            Ok(predicates)
        }
        CypherWhereBoolean::Not(inner) => lower_where_boolean_ast(inner, parameters),
        CypherWhereBoolean::And(terms) => {
            let mut parsed = Vec::new();
            for term in terms {
                let negated = lower_negated_where_boolean_ast(term, parameters)?;
                if negated.len() != 1 {
                    return Err(cypher_syntax(
                        "MATCH WHERE NOT over AND only supports foldable bounded predicate terms",
                    ));
                }
                parsed.extend(negated);
            }
            if let Some(first) = parsed.first()
                && parsed.iter().all(|predicate| predicate == first)
            {
                return Ok(vec![first.clone()]);
            }
            match parse_where_or_fold_parsed_terms(parsed.clone(), GraphPredicateOp::In) {
                Ok(predicate) => Ok(vec![predicate]),
                Err(_) => {
                    let branches = parsed
                        .into_iter()
                        .map(|predicate| vec![predicate])
                        .collect::<Vec<_>>();
                    let predicates = lower_where_or_branches(branches)?;
                    if predicates.len() != 1 || is_where_no_match(&predicates[0]) {
                        return Err(cypher_syntax(
                            "MATCH WHERE NOT over AND only supports collapsed bounded predicate terms",
                        ));
                    }
                    Ok(predicates)
                }
            }
        }
    }
}

pub(crate) fn lower_negated_null_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }
    let mut has_null_branch = false;
    for predicate in &parsed {
        if where_predicate_implies_is_null(predicate)? {
            has_null_branch = true;
            break;
        }
    }
    if !has_null_branch {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    let mut has_not_null_guard = false;
    for predicate in parsed {
        if where_predicate_implies_is_null(&predicate)? {
            if !has_not_null_guard {
                predicates.push(ParsedWherePredicate {
                    target: predicate.target,
                    predicate: GraphPropertyPredicate {
                        key: predicate.predicate.key,
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                });
                has_not_null_guard = true;
            }
            continue;
        }
        let op = match predicate.predicate.op {
            GraphPredicateOp::Equal => GraphPredicateOp::NotEqual,
            GraphPredicateOp::In => GraphPredicateOp::NotIn,
            _ => return Ok(None),
        };
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op,
                value: predicate.predicate.value,
            },
        });
    }
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_null_string_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut string_terms = Vec::new();
    let mut has_null_branch = false;
    for predicate in &parsed {
        if where_predicate_implies_is_null(predicate)? {
            has_null_branch = true;
        } else if is_positive_string_predicate_op(predicate.predicate.op) {
            string_terms.push(predicate.clone());
        } else {
            return Ok(None);
        }
    }
    if !has_null_branch || string_terms.is_empty() {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    if string_terms.len() == 1 {
        let predicate = string_terms
            .into_iter()
            .next()
            .expect("checked non-empty string terms");
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    } else {
        predicates.push(parse_where_or_fold_parsed_terms(
            string_terms,
            GraphPredicateOp::NotIn,
        )?);
    }
    let mut has_not_null_guard = false;
    for predicate in parsed {
        if where_predicate_implies_is_null(&predicate)? {
            if !has_not_null_guard {
                predicates.push(ParsedWherePredicate {
                    target: predicate.target,
                    predicate: GraphPropertyPredicate {
                        key: predicate.predicate.key,
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                });
                has_not_null_guard = true;
            }
            continue;
        }
    }
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_null_string_order_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut string_terms = Vec::new();
    let mut order_terms = Vec::new();
    let mut has_null_branch = false;
    for predicate in &parsed {
        if where_predicate_implies_is_null(predicate)? {
            has_null_branch = true;
        } else if is_positive_string_predicate_op(predicate.predicate.op) {
            string_terms.push(predicate.clone());
        } else if is_order_predicate_op(predicate.predicate.op) {
            if !matches!(predicate.predicate.value, Value::String(_)) {
                return Ok(None);
            }
            order_terms.push(predicate.clone());
        } else {
            return Ok(None);
        }
    }
    if !has_null_branch || string_terms.is_empty() || order_terms.is_empty() {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    if string_terms.len() == 1 {
        let predicate = string_terms
            .into_iter()
            .next()
            .expect("checked non-empty string terms");
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    } else {
        predicates.push(parse_where_or_fold_parsed_terms(
            string_terms,
            GraphPredicateOp::NotIn,
        )?);
    }
    for predicate in order_terms {
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    }

    let mut has_not_null_guard = false;
    for predicate in parsed {
        if where_predicate_implies_is_null(&predicate)? && !has_not_null_guard {
            predicates.push(ParsedWherePredicate {
                target: predicate.target,
                predicate: GraphPropertyPredicate {
                    key: predicate.predicate.key,
                    op: GraphPredicateOp::IsNotNull,
                    value: Value::Null,
                },
            });
            has_not_null_guard = true;
        }
    }
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_null_mixed_string_order_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut string_terms = Vec::new();
    let mut order_terms = Vec::new();
    let mut scalar_terms = Vec::new();
    let mut has_null_branch = false;
    for predicate in &parsed {
        if where_predicate_implies_is_null(predicate)? {
            has_null_branch = true;
        } else if is_positive_string_predicate_op(predicate.predicate.op) {
            string_terms.push(predicate.clone());
        } else if is_order_predicate_op(predicate.predicate.op) {
            if !matches!(predicate.predicate.value, Value::String(_)) {
                return Ok(None);
            }
            order_terms.push(predicate.clone());
        } else if matches!(
            predicate.predicate.op,
            GraphPredicateOp::Equal | GraphPredicateOp::In
        ) {
            if !where_predicate_uses_only_string_values(predicate)? {
                return Ok(None);
            }
            scalar_terms.push(predicate.clone());
        } else {
            return Ok(None);
        }
    }
    if !has_null_branch
        || string_terms.is_empty()
        || order_terms.is_empty()
        || scalar_terms.is_empty()
    {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    if string_terms.len() == 1 {
        let predicate = string_terms
            .into_iter()
            .next()
            .expect("checked non-empty string terms");
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    } else {
        predicates.push(parse_where_or_fold_parsed_terms(
            string_terms,
            GraphPredicateOp::NotIn,
        )?);
    }
    for predicate in order_terms.into_iter().chain(scalar_terms) {
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    }

    let mut has_not_null_guard = false;
    for predicate in parsed {
        if where_predicate_implies_is_null(&predicate)? && !has_not_null_guard {
            predicates.push(ParsedWherePredicate {
                target: predicate.target,
                predicate: GraphPropertyPredicate {
                    key: predicate.predicate.key,
                    op: GraphPredicateOp::IsNotNull,
                    value: Value::Null,
                },
            });
            has_not_null_guard = true;
        }
    }
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_null_mixed_string_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut string_terms = Vec::new();
    let mut scalar_terms = Vec::new();
    let mut has_null_branch = false;
    for predicate in &parsed {
        if where_predicate_implies_is_null(predicate)? {
            has_null_branch = true;
        } else if is_positive_string_predicate_op(predicate.predicate.op) {
            string_terms.push(predicate.clone());
        } else if matches!(
            predicate.predicate.op,
            GraphPredicateOp::Equal | GraphPredicateOp::In
        ) {
            scalar_terms.push(predicate.clone());
        } else {
            return Ok(None);
        }
    }
    if !has_null_branch || string_terms.is_empty() || scalar_terms.is_empty() {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    if string_terms.len() == 1 {
        let predicate = string_terms
            .into_iter()
            .next()
            .expect("checked non-empty string terms");
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    } else {
        predicates.push(parse_where_or_fold_parsed_terms(
            string_terms,
            GraphPredicateOp::NotIn,
        )?);
    }
    for predicate in scalar_terms {
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    }

    let mut has_not_null_guard = false;
    for predicate in parsed {
        if where_predicate_implies_is_null(&predicate)? {
            if !has_not_null_guard {
                predicates.push(ParsedWherePredicate {
                    target: predicate.target,
                    predicate: GraphPropertyPredicate {
                        key: predicate.predicate.key,
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                });
                has_not_null_guard = true;
            }
        }
    }
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_null_order_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut order_bounds = Vec::new();
    let mut has_null_branch = false;
    for predicate in &parsed {
        if where_predicate_implies_is_null(predicate)? {
            has_null_branch = true;
        } else if is_order_predicate_op(predicate.predicate.op) {
            order_bounds.push(&predicate.predicate.value);
        } else {
            return Ok(None);
        }
    }
    if !has_null_branch || order_bounds.is_empty() {
        return Ok(None);
    }
    let Some(first_order_bound) = order_bounds.first() else {
        return Ok(None);
    };
    if order_bounds
        .iter()
        .any(|bound| compare_where_order_values(bound, first_order_bound).is_none())
    {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    let mut has_not_null_guard = false;
    for predicate in parsed {
        if where_predicate_implies_is_null(&predicate)? {
            if !has_not_null_guard {
                predicates.push(ParsedWherePredicate {
                    target: predicate.target,
                    predicate: GraphPropertyPredicate {
                        key: predicate.predicate.key,
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                });
                has_not_null_guard = true;
            }
            continue;
        }
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    }
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_null_mixed_order_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut order_bounds = Vec::new();
    let mut has_null_branch = false;
    let mut has_equality_or_membership = false;
    for predicate in &parsed {
        if where_predicate_implies_is_null(predicate)? {
            has_null_branch = true;
            continue;
        }
        if is_order_predicate_op(predicate.predicate.op) {
            order_bounds.push(&predicate.predicate.value);
            continue;
        }
        if matches!(
            predicate.predicate.op,
            GraphPredicateOp::Equal | GraphPredicateOp::In
        ) {
            has_equality_or_membership = true;
            continue;
        }
        return Ok(None);
    }
    if !has_null_branch || order_bounds.is_empty() || !has_equality_or_membership {
        return Ok(None);
    }
    let Some(first_order_bound) = order_bounds.first() else {
        return Ok(None);
    };
    if order_bounds
        .iter()
        .any(|bound| compare_where_order_values(bound, first_order_bound).is_none())
    {
        return Ok(None);
    }

    for predicate in &parsed {
        if where_predicate_implies_is_null(predicate)? {
            continue;
        }
        match predicate.predicate.op {
            GraphPredicateOp::Equal => {
                if predicate.predicate.value == Value::Null
                    || order_bounds.iter().any(|bound| {
                        compare_where_order_values(&predicate.predicate.value, bound).is_none()
                    })
                {
                    return Ok(None);
                }
            }
            GraphPredicateOp::In => {
                for value in cypher_in_predicate_values(&predicate.predicate.value)? {
                    if value == Value::Null
                        || order_bounds
                            .iter()
                            .any(|bound| compare_where_order_values(&value, bound).is_none())
                    {
                        return Ok(None);
                    }
                }
            }
            _ => {}
        }
    }

    let mut predicates = Vec::new();
    let mut has_not_null_guard = false;
    for predicate in parsed {
        if where_predicate_implies_is_null(&predicate)? {
            if !has_not_null_guard {
                predicates.push(ParsedWherePredicate {
                    target: predicate.target,
                    predicate: GraphPropertyPredicate {
                        key: predicate.predicate.key,
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                });
                has_not_null_guard = true;
            }
            continue;
        }
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    }
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_order_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target
            || predicate.predicate.key != first.predicate.key
            || !is_order_predicate_op(predicate.predicate.op)
            || compare_where_order_values(&predicate.predicate.value, &first.predicate.value)
                .is_none()
    }) {
        return Ok(None);
    }

    let mut predicates = parsed
        .into_iter()
        .map(|predicate| ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        })
        .collect::<Vec<_>>();
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_string_order_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut string_terms = Vec::new();
    let mut order_terms = Vec::new();
    for predicate in &parsed {
        if is_positive_string_predicate_op(predicate.predicate.op) {
            string_terms.push(predicate.clone());
        } else if is_order_predicate_op(predicate.predicate.op) {
            if !matches!(predicate.predicate.value, Value::String(_)) {
                return Ok(None);
            }
            order_terms.push(predicate.clone());
        } else {
            return Ok(None);
        }
    }
    if string_terms.is_empty() || order_terms.is_empty() {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    if string_terms.len() == 1 {
        let predicate = string_terms
            .into_iter()
            .next()
            .expect("checked non-empty string terms");
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    } else {
        predicates.push(parse_where_or_fold_parsed_terms(
            string_terms,
            GraphPredicateOp::NotIn,
        )?);
    }
    for predicate in order_terms {
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    }
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_mixed_string_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut string_terms = Vec::new();
    let mut scalar_terms = Vec::new();
    for predicate in &parsed {
        if is_positive_string_predicate_op(predicate.predicate.op) {
            string_terms.push(predicate.clone());
        } else if matches!(
            predicate.predicate.op,
            GraphPredicateOp::Equal | GraphPredicateOp::In
        ) {
            if !where_predicate_uses_only_string_values(predicate)? {
                return Ok(None);
            }
            scalar_terms.push(predicate.clone());
        } else {
            return Ok(None);
        }
    }
    if string_terms.is_empty() || scalar_terms.is_empty() {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    if string_terms.len() == 1 {
        let predicate = string_terms
            .into_iter()
            .next()
            .expect("checked non-empty string terms");
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    } else {
        predicates.push(parse_where_or_fold_parsed_terms(
            string_terms,
            GraphPredicateOp::NotIn,
        )?);
    }
    for predicate in scalar_terms {
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    }
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_mixed_string_order_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut string_terms = Vec::new();
    let mut order_terms = Vec::new();
    let mut scalar_terms = Vec::new();
    for predicate in &parsed {
        if is_positive_string_predicate_op(predicate.predicate.op) {
            string_terms.push(predicate.clone());
        } else if is_order_predicate_op(predicate.predicate.op) {
            if !matches!(predicate.predicate.value, Value::String(_)) {
                return Ok(None);
            }
            order_terms.push(predicate.clone());
        } else if matches!(
            predicate.predicate.op,
            GraphPredicateOp::Equal | GraphPredicateOp::In
        ) {
            if !where_predicate_uses_only_string_values(predicate)? {
                return Ok(None);
            }
            scalar_terms.push(predicate.clone());
        } else {
            return Ok(None);
        }
    }
    if string_terms.is_empty() || order_terms.is_empty() || scalar_terms.is_empty() {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    if string_terms.len() == 1 {
        let predicate = string_terms
            .into_iter()
            .next()
            .expect("checked non-empty string terms");
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    } else {
        predicates.push(parse_where_or_fold_parsed_terms(
            string_terms,
            GraphPredicateOp::NotIn,
        )?);
    }
    for predicate in order_terms.into_iter().chain(scalar_terms) {
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    }
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_mixed_order_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let has_order = parsed
        .iter()
        .any(|predicate| is_order_predicate_op(predicate.predicate.op));
    let has_equality_or_membership = parsed.iter().any(|predicate| {
        matches!(
            predicate.predicate.op,
            GraphPredicateOp::Equal | GraphPredicateOp::In
        )
    });
    if !has_order || !has_equality_or_membership {
        return Ok(None);
    }

    let order_bounds = parsed
        .iter()
        .filter(|predicate| is_order_predicate_op(predicate.predicate.op))
        .map(|predicate| &predicate.predicate.value)
        .collect::<Vec<_>>();
    for predicate in &parsed {
        match predicate.predicate.op {
            GraphPredicateOp::Equal => {
                if predicate.predicate.value == Value::Null
                    || order_bounds.iter().any(|bound| {
                        compare_where_order_values(&predicate.predicate.value, bound).is_none()
                    })
                {
                    return Ok(None);
                }
            }
            GraphPredicateOp::In => {
                for value in cypher_in_predicate_values(&predicate.predicate.value)? {
                    if value == Value::Null
                        || order_bounds
                            .iter()
                            .any(|bound| compare_where_order_values(&value, bound).is_none())
                    {
                        return Ok(None);
                    }
                }
            }
            op if is_order_predicate_op(op) => {
                if order_bounds.iter().any(|bound| {
                    compare_where_order_values(&predicate.predicate.value, bound).is_none()
                }) {
                    return Ok(None);
                }
            }
            _ => return Ok(None),
        }
    }

    let mut predicates = parsed
        .into_iter()
        .map(|predicate| ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        })
        .collect::<Vec<_>>();
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn where_boolean_contains_and(predicate: &CypherWhereBoolean<'_>) -> bool {
    match predicate {
        CypherWhereBoolean::And(_) => true,
        CypherWhereBoolean::Not(inner) => where_boolean_contains_and(inner),
        CypherWhereBoolean::Or(terms) => terms.iter().any(where_boolean_contains_and),
        CypherWhereBoolean::Predicate(_) => false,
    }
}

pub(crate) fn parse_where_or_fold_ast_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
    op: GraphPredicateOp,
) -> Result<ParsedWherePredicate> {
    let mut parsed = Vec::new();
    for term in terms {
        collect_where_or_fold_ast_terms(term, parameters, &mut parsed)?;
    }
    parse_where_or_fold_parsed_terms(parsed, op)
}

pub(crate) fn collect_where_or_fold_ast_terms(
    predicate: &CypherWhereBoolean<'_>,
    parameters: &CypherParameters,
    parsed: &mut Vec<ParsedWherePredicate>,
) -> Result<()> {
    match predicate {
        CypherWhereBoolean::Or(terms) => {
            for term in terms {
                collect_where_or_fold_ast_terms(term, parameters, parsed)?;
            }
            Ok(())
        }
        _ => {
            parsed.push(parse_where_single_predicate_ast(predicate, parameters)?);
            Ok(())
        }
    }
}

pub(crate) fn parse_where_single_predicate_ast(
    predicate: &CypherWhereBoolean<'_>,
    parameters: &CypherParameters,
) -> Result<ParsedWherePredicate> {
    match predicate {
        CypherWhereBoolean::Predicate(predicate) => parse_where_predicate(predicate, parameters),
        CypherWhereBoolean::Not(_) | CypherWhereBoolean::And(_) | CypherWhereBoolean::Or(_) => Err(
            cypher_syntax("MATCH WHERE OR only supports bounded predicate terms"),
        ),
    }
}

pub(crate) fn parse_where_or_fold_parsed_terms(
    parsed: Vec<ParsedWherePredicate>,
    op: GraphPredicateOp,
) -> Result<ParsedWherePredicate> {
    let first = parsed
        .first()
        .ok_or_else(|| cypher_syntax("MATCH WHERE OR requires at least one predicate"))?;
    if matches!(
        first.predicate.op,
        GraphPredicateOp::Equal | GraphPredicateOp::In
    ) {
        return parse_where_or_membership_fold_terms(parsed, op);
    }
    if matches!(
        first.predicate.op,
        GraphPredicateOp::StartsWith
            | GraphPredicateOp::StartsWithAny
            | GraphPredicateOp::EndsWith
            | GraphPredicateOp::EndsWithAny
            | GraphPredicateOp::Contains
            | GraphPredicateOp::ContainsAny
    ) {
        return parse_where_or_string_fold_terms(parsed, op);
    }
    Err(cypher_syntax(
        "MATCH WHERE OR only supports same-property equality, membership, or matching string predicate disjunctions",
    ))
}

pub(crate) fn lower_where_or_branches(
    branches: Vec<Vec<ParsedWherePredicate>>,
) -> Result<Vec<ParsedWherePredicate>> {
    let mut no_match_predicate = None;
    let mut branches = branches
        .into_iter()
        .filter(|branch| {
            if let Some(predicate) = branch.iter().find(|predicate| is_where_no_match(predicate)) {
                no_match_predicate.get_or_insert_with(|| predicate.clone());
                false
            } else {
                true
            }
        })
        .collect::<Vec<_>>();
    prune_subsumed_where_or_branches(&mut branches)?;

    if branches.len() == 1 {
        return Ok(branches.into_iter().next().expect("single branch"));
    }
    if branches.is_empty() {
        return Ok(vec![no_match_predicate.ok_or_else(|| {
            cypher_syntax("MATCH WHERE OR requires at least one predicate")
        })?]);
    }

    let first = branches
        .first()
        .ok_or_else(|| cypher_syntax("MATCH WHERE OR requires at least one predicate"))?;
    for varying_index in 0..first.len() {
        let mut common = first.clone();
        let varying = common.remove(varying_index);
        let mut fold_terms = vec![varying];
        let mut matched = true;
        for branch in branches.iter().skip(1) {
            let mut remaining = branch.clone();
            for common_predicate in &common {
                if let Some(index) = remaining
                    .iter()
                    .position(|predicate| predicate == common_predicate)
                {
                    remaining.remove(index);
                } else {
                    matched = false;
                    break;
                }
            }
            if !matched || remaining.len() != 1 {
                matched = false;
                break;
            }
            fold_terms.push(remaining.remove(0));
        }
        if matched {
            let folded = parse_where_or_fold_parsed_terms(fold_terms, GraphPredicateOp::In)?;
            common.push(folded);
            return Ok(common);
        }
    }

    Err(cypher_syntax(
        "MATCH WHERE OR of AND groups only supports identical bounded predicates plus one same-property foldable predicate",
    ))
}

pub(crate) fn prune_subsumed_where_or_branches(branches: &mut Vec<Vec<ParsedWherePredicate>>) -> Result<()> {
    let mut keep = Vec::with_capacity(branches.len());
    for index in 0..branches.len() {
        let mut redundant = false;
        for (candidate_index, candidate) in branches.iter().enumerate() {
            if candidate_index != index && where_branch_implies_all(&branches[index], candidate)? {
                let candidate_implies_branch =
                    where_branch_implies_all(candidate, &branches[index])?;
                let candidate_preferred =
                    where_branch_preferred_for_subsumption(candidate, &branches[index])?;
                let branch_preferred =
                    where_branch_preferred_for_subsumption(&branches[index], candidate)?;
                if candidate.len() < branches[index].len()
                    || !candidate_implies_branch
                    || candidate_preferred
                    || (!branch_preferred && candidate_index < index)
                {
                    redundant = true;
                    break;
                }
            }
        }
        if !redundant {
            keep.push(branches[index].clone());
        }
    }
    *branches = keep;
    Ok(())
}

pub(crate) fn where_branch_implies_all(
    branch: &[ParsedWherePredicate],
    required: &[ParsedWherePredicate],
) -> Result<bool> {
    for required in required {
        let mut implied = false;
        for predicate in branch {
            if where_predicate_implies(predicate, required)? {
                implied = true;
                break;
            }
        }
        if !implied {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn where_branch_preferred_for_subsumption(
    candidate: &[ParsedWherePredicate],
    branch: &[ParsedWherePredicate],
) -> Result<bool> {
    for branch_predicate in branch {
        for candidate_predicate in candidate {
            if candidate_predicate.target != branch_predicate.target
                || candidate_predicate.predicate.key != branch_predicate.predicate.key
            {
                continue;
            }
            match (
                candidate_predicate.predicate.op,
                branch_predicate.predicate.op,
            ) {
                (GraphPredicateOp::Equal, GraphPredicateOp::In)
                    if membership_values_all_satisfy(
                        &branch_predicate.predicate.value,
                        |value| Ok(value == &candidate_predicate.predicate.value),
                    )? =>
                {
                    return Ok(true);
                }
                (GraphPredicateOp::NotIn, GraphPredicateOp::NotEqual)
                    if membership_values_all_satisfy(
                        &candidate_predicate.predicate.value,
                        |value| Ok(value == &branch_predicate.predicate.value),
                    )? =>
                {
                    return Ok(true);
                }
                _ => {}
            }
        }
    }
    Ok(false)
}

pub(crate) fn where_predicate_implies(
    predicate: &ParsedWherePredicate,
    required: &ParsedWherePredicate,
) -> Result<bool> {
    if predicate == required {
        return Ok(true);
    }
    if predicate.target != required.target || predicate.predicate.key != required.predicate.key {
        return Ok(false);
    }
    match (predicate.predicate.op, required.predicate.op) {
        (_, GraphPredicateOp::IsNull) => where_predicate_implies_is_null(predicate),
        (_, GraphPredicateOp::IsNotNull) => where_predicate_implies_is_not_null(predicate),
        (GraphPredicateOp::Equal, GraphPredicateOp::In) => {
            membership_contains_value(&required.predicate.value, &predicate.predicate.value)
        }
        (GraphPredicateOp::Equal, GraphPredicateOp::NotIn) => Ok(!membership_contains_value(
            &required.predicate.value,
            &predicate.predicate.value,
        )?),
        (GraphPredicateOp::Equal, GraphPredicateOp::NotEqual) => {
            Ok(predicate.predicate.value != required.predicate.value)
        }
        (GraphPredicateOp::Equal, op) if is_order_predicate_op(op) => {
            Ok(equality_satisfies_order_bound(
                &predicate.predicate.value,
                op,
                &required.predicate.value,
            ))
        }
        (GraphPredicateOp::In, GraphPredicateOp::Equal) => {
            membership_values_all_satisfy(&predicate.predicate.value, |value| {
                Ok(value == &required.predicate.value)
            })
        }
        (GraphPredicateOp::In, GraphPredicateOp::In) => {
            membership_values_all_satisfy(&predicate.predicate.value, |value| {
                membership_contains_value(&required.predicate.value, value)
            })
        }
        (GraphPredicateOp::In, GraphPredicateOp::NotIn) => {
            membership_values_all_satisfy(&predicate.predicate.value, |value| {
                Ok(!membership_contains_value(
                    &required.predicate.value,
                    value,
                )?)
            })
        }
        (GraphPredicateOp::In, GraphPredicateOp::NotEqual) => {
            membership_values_all_satisfy(&predicate.predicate.value, |value| {
                Ok(value != &required.predicate.value)
            })
        }
        (GraphPredicateOp::In, op) if is_order_predicate_op(op) => {
            membership_values_all_satisfy(&predicate.predicate.value, |value| {
                Ok(equality_satisfies_order_bound(
                    value,
                    op,
                    &required.predicate.value,
                ))
            })
        }
        (GraphPredicateOp::NotIn, GraphPredicateOp::NotIn) => {
            membership_values_all_satisfy(&required.predicate.value, |value| {
                membership_contains_value(&predicate.predicate.value, value)
            })
        }
        (GraphPredicateOp::NotIn, GraphPredicateOp::NotEqual) => {
            membership_contains_value(&predicate.predicate.value, &required.predicate.value)
        }
        (GraphPredicateOp::NotEqual, GraphPredicateOp::NotIn) => {
            membership_values_all_satisfy(&required.predicate.value, |value| {
                Ok(value == &predicate.predicate.value)
            })
        }
        (op, GraphPredicateOp::NotEqual) if is_order_predicate_op(op) => Ok(
            order_bound_excludes_value(op, &predicate.predicate.value, &required.predicate.value),
        ),
        (op, GraphPredicateOp::NotIn) if is_order_predicate_op(op) => {
            membership_values_all_satisfy(&required.predicate.value, |value| {
                Ok(order_bound_excludes_value(
                    op,
                    &predicate.predicate.value,
                    value,
                ))
            })
        }
        (GraphPredicateOp::StartsWith, GraphPredicateOp::StartsWithAny)
        | (GraphPredicateOp::EndsWith, GraphPredicateOp::EndsWithAny)
        | (GraphPredicateOp::Contains, GraphPredicateOp::ContainsAny) => {
            string_group_contains_value(&required.predicate.value, &predicate.predicate.value)
        }
        (GraphPredicateOp::StartsWithAny, GraphPredicateOp::StartsWithAny)
        | (GraphPredicateOp::EndsWithAny, GraphPredicateOp::EndsWithAny)
        | (GraphPredicateOp::ContainsAny, GraphPredicateOp::ContainsAny) => {
            string_group_values_all_satisfy(&predicate.predicate.value, |value| {
                string_group_contains_value(&required.predicate.value, value)
            })
        }
        (GraphPredicateOp::NotStartsWithAny, GraphPredicateOp::NotStartsWith)
        | (GraphPredicateOp::NotEndsWithAny, GraphPredicateOp::NotEndsWith)
        | (GraphPredicateOp::NotContainsAny, GraphPredicateOp::NotContains) => {
            string_group_contains_value(&predicate.predicate.value, &required.predicate.value)
        }
        (GraphPredicateOp::NotStartsWith, GraphPredicateOp::NotStartsWithAny)
        | (GraphPredicateOp::NotEndsWith, GraphPredicateOp::NotEndsWithAny)
        | (GraphPredicateOp::NotContains, GraphPredicateOp::NotContainsAny) => {
            string_group_values_all_satisfy(&required.predicate.value, |value| {
                Ok(value == &predicate.predicate.value)
            })
        }
        (GraphPredicateOp::NotStartsWithAny, GraphPredicateOp::NotStartsWithAny)
        | (GraphPredicateOp::NotEndsWithAny, GraphPredicateOp::NotEndsWithAny)
        | (GraphPredicateOp::NotContainsAny, GraphPredicateOp::NotContainsAny) => {
            string_group_values_all_satisfy(&required.predicate.value, |value| {
                string_group_contains_value(&predicate.predicate.value, value)
            })
        }
        (op, required_op) if is_order_predicate_op(op) && is_order_predicate_op(required_op) => {
            Ok(order_predicate_implies_order_bound(
                op,
                &predicate.predicate.value,
                required_op,
                &required.predicate.value,
            ))
        }
        _ => Ok(false),
    }
}

pub(crate) fn where_predicate_implies_is_null(predicate: &ParsedWherePredicate) -> Result<bool> {
    match predicate.predicate.op {
        GraphPredicateOp::Equal => Ok(predicate.predicate.value == Value::Null),
        GraphPredicateOp::In => {
            membership_values_all_satisfy(&predicate.predicate.value, |value| {
                Ok(value == &Value::Null)
            })
        }
        _ => Ok(false),
    }
}

pub(crate) fn where_predicate_implies_is_not_null(predicate: &ParsedWherePredicate) -> Result<bool> {
    match predicate.predicate.op {
        GraphPredicateOp::Equal => Ok(predicate.predicate.value != Value::Null),
        GraphPredicateOp::NotEqual => Ok(predicate.predicate.value == Value::Null),
        GraphPredicateOp::In => {
            membership_values_all_satisfy(&predicate.predicate.value, |value| {
                Ok(value != &Value::Null)
            })
        }
        GraphPredicateOp::NotIn => {
            membership_contains_value(&predicate.predicate.value, &Value::Null)
        }
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
        | GraphPredicateOp::NotContainsAny
        | GraphPredicateOp::GreaterThan
        | GraphPredicateOp::GreaterThanOrEqual
        | GraphPredicateOp::LessThan
        | GraphPredicateOp::LessThanOrEqual => Ok(true),
        _ => Ok(false),
    }
}

pub(crate) fn string_group_contains_value(group: &Value, value: &Value) -> Result<bool> {
    let Value::String(needle) = value else {
        return Ok(false);
    };
    Ok(match group {
        Value::StringArray(values) => values.iter().any(|value| value == needle),
        Value::Json(json) => json
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(needle))),
        _ => false,
    })
}

pub(crate) fn string_group_values_all_satisfy(
    group: &Value,
    mut predicate: impl FnMut(&Value) -> Result<bool>,
) -> Result<bool> {
    match group {
        Value::StringArray(values) => {
            for value in values {
                if !predicate(&Value::from(value.as_str()))? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Value::Json(json) => {
            let Some(values) = json.as_array() else {
                return Ok(false);
            };
            for value in values {
                let Some(value) = value.as_str() else {
                    return Ok(false);
                };
                if !predicate(&Value::from(value))? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub(crate) fn order_predicate_implies_order_bound(
    op: GraphPredicateOp,
    value: &Value,
    required_op: GraphPredicateOp,
    required_value: &Value,
) -> bool {
    match (order_bound_kind(op), order_bound_kind(required_op)) {
        (Some(OrderBoundKind::Lower), Some(OrderBoundKind::Lower)) => {
            order_lower_is_stricter(op, value, required_op, required_value)
        }
        (Some(OrderBoundKind::Upper), Some(OrderBoundKind::Upper)) => {
            order_upper_is_stricter(op, value, required_op, required_value)
        }
        _ => false,
    }
}

pub(crate) fn membership_values_all_satisfy(
    membership: &Value,
    mut predicate: impl FnMut(&Value) -> Result<bool>,
) -> Result<bool> {
    for value in cypher_in_predicate_values(membership)? {
        if !predicate(&value)? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn parse_where_or_membership_fold_terms(
    mut parsed: Vec<ParsedWherePredicate>,
    op: GraphPredicateOp,
) -> Result<ParsedWherePredicate> {
    let first = parsed
        .first()
        .ok_or_else(|| cypher_syntax("MATCH WHERE OR requires at least one predicate"))?;
    if !matches!(
        first.predicate.op,
        GraphPredicateOp::Equal | GraphPredicateOp::In
    ) {
        return Err(cypher_syntax(
            "MATCH WHERE OR only supports same-property equality or membership disjunctions",
        ));
    }
    let target = first.target.clone();
    let key = first.predicate.key.clone();
    let mut values = Vec::with_capacity(parsed.len());
    for predicate in parsed.drain(..) {
        if predicate.target != target || predicate.predicate.key != key {
            return Err(cypher_syntax(
                "MATCH WHERE OR only supports same-property equality or membership disjunctions",
            ));
        }
        match predicate.predicate.op {
            GraphPredicateOp::Equal => {
                validate_cypher_in_item(&predicate.predicate.value)?;
                push_unique(&mut values, predicate.predicate.value.to_json());
            }
            GraphPredicateOp::In => {
                for value in cypher_in_predicate_values(&predicate.predicate.value)? {
                    push_unique(&mut values, value.to_json());
                }
            }
            _ => {
                return Err(cypher_syntax(
                    "MATCH WHERE OR only supports same-property equality or membership disjunctions",
                ));
            }
        }
    }
    Ok(ParsedWherePredicate {
        target,
        predicate: GraphPropertyPredicate {
            key,
            op,
            value: Value::Json(serde_json::Value::Array(values)),
        },
    })
}

pub(crate) fn parse_where_or_string_fold_terms(
    mut parsed: Vec<ParsedWherePredicate>,
    op: GraphPredicateOp,
) -> Result<ParsedWherePredicate> {
    let first = parsed
        .first()
        .ok_or_else(|| cypher_syntax("MATCH WHERE OR requires at least one predicate"))?;
    let target = first.target.clone();
    let key = first.predicate.key.clone();
    let Some(string_op) = string_fold_base_op(first.predicate.op) else {
        return Err(cypher_syntax(
            "MATCH WHERE OR only supports matching string predicate disjunctions",
        ));
    };
    let folded_op = match (op, string_op) {
        (GraphPredicateOp::In, GraphPredicateOp::StartsWith) => GraphPredicateOp::StartsWithAny,
        (GraphPredicateOp::NotIn, GraphPredicateOp::StartsWith) => {
            GraphPredicateOp::NotStartsWithAny
        }
        (GraphPredicateOp::In, GraphPredicateOp::EndsWith) => GraphPredicateOp::EndsWithAny,
        (GraphPredicateOp::NotIn, GraphPredicateOp::EndsWith) => GraphPredicateOp::NotEndsWithAny,
        (GraphPredicateOp::In, GraphPredicateOp::Contains) => GraphPredicateOp::ContainsAny,
        (GraphPredicateOp::NotIn, GraphPredicateOp::Contains) => GraphPredicateOp::NotContainsAny,
        _ => {
            return Err(cypher_syntax(
                "MATCH WHERE OR only supports matching string predicate disjunctions",
            ));
        }
    };
    let mut needles = Vec::with_capacity(parsed.len());
    for predicate in parsed.drain(..) {
        if predicate.target != target
            || predicate.predicate.key != key
            || string_fold_base_op(predicate.predicate.op) != Some(string_op)
        {
            return Err(cypher_syntax(
                "MATCH WHERE OR only supports matching same-property string predicate disjunctions",
            ));
        }
        match predicate.predicate.op {
            GraphPredicateOp::StartsWith
            | GraphPredicateOp::NotStartsWith
            | GraphPredicateOp::EndsWith
            | GraphPredicateOp::NotEndsWith
            | GraphPredicateOp::Contains => {
                let Some(needle) = predicate.predicate.value.as_str() else {
                    return Err(cypher_syntax(
                        "MATCH WHERE string predicates require string literals or parameters",
                    ));
                };
                push_unique(&mut needles, needle.to_string());
            }
            GraphPredicateOp::NotContains => {
                let Some(needle) = predicate.predicate.value.as_str() else {
                    return Err(cypher_syntax(
                        "MATCH WHERE string predicates require string literals or parameters",
                    ));
                };
                push_unique(&mut needles, needle.to_string());
            }
            GraphPredicateOp::StartsWithAny
            | GraphPredicateOp::NotStartsWithAny
            | GraphPredicateOp::EndsWithAny
            | GraphPredicateOp::NotEndsWithAny
            | GraphPredicateOp::ContainsAny
            | GraphPredicateOp::NotContainsAny => {
                let Value::StringArray(values) = &predicate.predicate.value else {
                    return Err(cypher_syntax(
                        "MATCH WHERE grouped string predicates require string list values",
                    ));
                };
                for needle in values {
                    push_unique(&mut needles, needle.clone());
                }
            }
            _ => {
                return Err(cypher_syntax(
                    "MATCH WHERE OR only supports matching string predicate disjunctions",
                ));
            }
        }
    }
    Ok(ParsedWherePredicate {
        target,
        predicate: GraphPropertyPredicate {
            key,
            op: folded_op,
            value: Value::from(needles),
        },
    })
}

pub(crate) fn string_fold_base_op(op: GraphPredicateOp) -> Option<GraphPredicateOp> {
    match op {
        GraphPredicateOp::StartsWith
        | GraphPredicateOp::StartsWithAny
        | GraphPredicateOp::NotStartsWith
        | GraphPredicateOp::NotStartsWithAny => Some(GraphPredicateOp::StartsWith),
        GraphPredicateOp::EndsWith
        | GraphPredicateOp::EndsWithAny
        | GraphPredicateOp::NotEndsWith
        | GraphPredicateOp::NotEndsWithAny => Some(GraphPredicateOp::EndsWith),
        GraphPredicateOp::Contains
        | GraphPredicateOp::ContainsAny
        | GraphPredicateOp::NotContains
        | GraphPredicateOp::NotContainsAny => Some(GraphPredicateOp::Contains),
        _ => None,
    }
}

pub(crate) fn push_unique<T: PartialEq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

pub(crate) fn parse_where_predicate(
    predicate: &str,
    parameters: &CypherParameters,
) -> Result<ParsedWherePredicate> {
    let predicate = strip_enclosing_parentheses(predicate.trim())?;
    let (predicate, negated) = if let Some(after_not) = strip_leading_keyword(predicate, "NOT") {
        let predicate = strip_enclosing_parentheses(after_not.trim())?;
        if predicate.is_empty() || strip_leading_keyword(predicate, "NOT").is_some() {
            return Err(cypher_syntax(
                "MATCH WHERE NOT only supports a single property comparison",
            ));
        }
        (predicate, true)
    } else {
        (predicate, false)
    };
    if let Some(index) = find_unquoted_keyword(predicate, "IS") {
        let rest = predicate[index + "IS".len()..].trim();
        let words = rest.split_whitespace().collect::<Vec<_>>();
        let op = if words.len() == 2
            && words[0].eq_ignore_ascii_case("NOT")
            && words[1].eq_ignore_ascii_case("NULL")
        {
            GraphPredicateOp::IsNotNull
        } else if words.len() == 1 && words[0].eq_ignore_ascii_case("NULL") {
            GraphPredicateOp::IsNull
        } else {
            return Err(cypher_syntax(
                "MATCH WHERE IS predicates only support IS NULL or IS NOT NULL",
            ));
        };
        let op = if negated {
            inverted_graph_predicate_op(op)
        } else {
            op
        };
        let (target, key) = parse_property_ref(&predicate[..index], "MATCH WHERE predicate")?;
        return Ok(ParsedWherePredicate {
            target,
            predicate: GraphPropertyPredicate {
                key,
                op,
                value: Value::Null,
            },
        });
    }
    for (keyword, op) in [
        ("STARTS WITH", GraphPredicateOp::StartsWith),
        ("ENDS WITH", GraphPredicateOp::EndsWith),
        ("CONTAINS", GraphPredicateOp::Contains),
    ] {
        if let Some(index) = find_top_level_keyword_sequence(predicate, keyword)? {
            let (target, key) =
                parse_property_ref(&predicate[..index], "MATCH WHERE string predicate")?;
            let value = parse_cypher_literal(&predicate[index + keyword.len()..], parameters)?;
            if !matches!(value, Value::String(_)) {
                return Err(cypher_syntax(
                    "MATCH WHERE string predicates require string literals or parameters",
                ));
            }
            let op = if negated {
                inverted_graph_predicate_op(op)
            } else {
                op
            };
            return Ok(ParsedWherePredicate {
                target,
                predicate: GraphPropertyPredicate { key, op, value },
            });
        }
    }
    if let Some(index) = find_top_level_keyword_sequence(predicate, "IN")? {
        let (target, key) = parse_property_ref(&predicate[..index], "MATCH WHERE IN predicate")?;
        let value = parse_cypher_in_values(&predicate[index + "IN".len()..], parameters)?;
        let op = if negated {
            GraphPredicateOp::NotIn
        } else {
            GraphPredicateOp::In
        };
        return Ok(ParsedWherePredicate {
            target,
            predicate: GraphPropertyPredicate { key, op, value },
        });
    }
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
            let op = if negated {
                inverted_graph_predicate_op(op)
            } else {
                op
            };
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

pub(crate) fn inverted_graph_predicate_op(op: GraphPredicateOp) -> GraphPredicateOp {
    match op {
        GraphPredicateOp::Equal => GraphPredicateOp::NotEqual,
        GraphPredicateOp::NotEqual => GraphPredicateOp::Equal,
        GraphPredicateOp::IsNull => GraphPredicateOp::IsNotNull,
        GraphPredicateOp::IsNotNull => GraphPredicateOp::IsNull,
        GraphPredicateOp::StartsWith => GraphPredicateOp::NotStartsWith,
        GraphPredicateOp::NotStartsWith => GraphPredicateOp::StartsWith,
        GraphPredicateOp::StartsWithAny => GraphPredicateOp::NotStartsWithAny,
        GraphPredicateOp::NotStartsWithAny => GraphPredicateOp::StartsWithAny,
        GraphPredicateOp::EndsWith => GraphPredicateOp::NotEndsWith,
        GraphPredicateOp::NotEndsWith => GraphPredicateOp::EndsWith,
        GraphPredicateOp::EndsWithAny => GraphPredicateOp::NotEndsWithAny,
        GraphPredicateOp::NotEndsWithAny => GraphPredicateOp::EndsWithAny,
        GraphPredicateOp::Contains => GraphPredicateOp::NotContains,
        GraphPredicateOp::NotContains => GraphPredicateOp::Contains,
        GraphPredicateOp::ContainsAny => GraphPredicateOp::NotContainsAny,
        GraphPredicateOp::NotContainsAny => GraphPredicateOp::ContainsAny,
        GraphPredicateOp::In => GraphPredicateOp::NotIn,
        GraphPredicateOp::NotIn => GraphPredicateOp::In,
        GraphPredicateOp::GreaterThan => GraphPredicateOp::LessThanOrEqual,
        GraphPredicateOp::GreaterThanOrEqual => GraphPredicateOp::LessThan,
        GraphPredicateOp::LessThan => GraphPredicateOp::GreaterThanOrEqual,
        GraphPredicateOp::LessThanOrEqual => GraphPredicateOp::GreaterThan,
    }
}

pub(crate) fn apply_node_where_predicates(
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

pub(crate) fn apply_match_where_predicates(
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

pub(crate) fn apply_edge_where_predicates(
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

pub(crate) fn split_match_delete(statement: &str) -> Result<(&str, &str)> {
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

pub(crate) fn parse_match_delete_targets(targets: &str) -> Result<Vec<String>> {
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

pub(crate) fn split_match_edge_upsert<'a>(statement: &'a str, keyword: &str) -> Result<(&'a str, &'a str)> {
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

pub(crate) fn parse_path_binding<'a>(pattern: &'a str, context: &str) -> Result<(Option<String>, &'a str)> {
    let Some(index) = find_unquoted(pattern, '=') else {
        return Ok((None, pattern.trim()));
    };
    let variable = parse_required_cypher_variable(
        pattern[..index].trim(),
        &format!("{context} path variable"),
    )?;
    let relationship_pattern = pattern[index + 1..].trim();
    if !relationship_pattern.starts_with('(') {
        return Err(cypher_syntax(format!(
            "{context} path variable must bind a relationship pattern"
        )));
    }
    Ok((Some(variable), relationship_pattern))
}

pub(crate) fn parse_row_path_binding(pattern: &str) -> Result<(Option<String>, &str)> {
    parse_path_binding(pattern, "MATCH CREATE/MERGE")
}

pub(crate) fn split_match_set(statement: &str) -> Result<(&str, &str)> {
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

pub(crate) fn split_match_remove(statement: &str) -> Result<(&str, &str)> {
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

pub(crate) fn split_final_return(statement: &str) -> Result<(&str, &str)> {
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

pub(crate) fn find_return_control_clause(return_clause: &str) -> Option<usize> {
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

pub(crate) fn is_return_alias_keyword_prefix(prefix: &str) -> bool {
    let mut words = prefix.split_whitespace().rev();
    words
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case("AS"))
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnClause {
    pub projections: Vec<CypherReturnProjection>,
    pub order_by: Vec<CypherOrderItem>,
    pub skip: Option<usize>,
    pub limit: Option<usize>,
    pub distinct: bool,
}

/// One `ORDER BY` term, resolved to the index of a returned column. Ordering by
/// expressions that are not part of the projection is not supported.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CypherOrderItem {
    pub column: usize,
    pub descending: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnProjection {
    pub variable: String,
    pub target: CypherReturnTarget,
    pub column: String,
    pub expression: String,
    pub element: CypherReturnElement,
    pub aggregate: Option<CypherReturnAggregate>,
    pub distinct: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CypherReturnTarget {
    All,
    Element,
    Literal(Value),
    Property(String),
    MapProjection(CypherReturnMapProjection),
    ListProjection(CypherReturnListProjection),
    Case(CypherReturnCase),
    Coalesce(CypherReturnCoalesce),
    PropertyExists(String),
    PropertySize(String),
    PropertyListIndex(CypherReturnListIndexProjection),
    PropertyListSlice(CypherReturnListSlice),
    PropertyListContains(CypherReturnListContains),
    PropertyListPredicate(CypherReturnListPredicateProjection),
    PropertyListElement(CypherReturnListElementProjection),
    PropertyListTail(CypherReturnListTailProjection),
    PropertyAbs(CypherReturnAbsProjection),
    PropertyNumericRound(CypherReturnNumericRoundProjection),
    PropertyNumericSign(CypherReturnNumericSignProjection),
    PropertyNumericCast(CypherReturnNumericCastProjection),
    PropertyListCast(CypherReturnListCastProjection),
    PropertyToBoolean(CypherReturnToBooleanProjection),
    PropertyToString(CypherReturnToStringProjection),
    PropertyStringTransform(CypherReturnStringTransformProjection),
    PropertyStringTrim(CypherReturnStringTrimProjection),
    PropertyIsEmpty(CypherReturnIsEmptyProjection),
    PropertyStringReverse(CypherReturnStringReverseProjection),
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
pub struct CypherReturnCoalesce {
    variable: Option<String>,
    terms: Vec<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListProjection {
    variable: Option<String>,
    terms: Vec<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnMapProjection {
    variable: String,
    entries: Vec<CypherReturnMapProjectionEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CypherReturnMapProjectionEntry {
    output_key: String,
    value: CypherReturnTarget,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherReturnStringTransform {
    Lower,
    Upper,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnStringTransformProjection {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
    transform: CypherReturnStringTransform,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherReturnStringTrim {
    Both,
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnStringTrimProjection {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
    trim: CypherReturnStringTrim,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnToStringProjection {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnAbsProjection {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnNumericRoundProjection {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
    round: CypherReturnNumericRound,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnNumericSignProjection {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnNumericCastProjection {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
    cast: CypherReturnNumericCast,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnToBooleanProjection {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListCastProjection {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
    cast: CypherReturnListCast,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnStringReverseProjection {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnIsEmptyProjection {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListElementProjection {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
    element: CypherReturnListElement,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListTailProjection {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherReturnNumericRound {
    Ceil,
    Floor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherReturnNumericCast {
    Integer,
    Float,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherReturnListCast {
    String,
    Integer,
    Float,
    Boolean,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherReturnListElement {
    Head,
    Last,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnCase {
    key: String,
    equals: Value,
    then_target: Box<CypherReturnTarget>,
    else_target: Box<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnSubstring {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
    start: usize,
    length: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnStringSplit {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
    delimiter: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnStringSlice {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
    side: CypherReturnStringSliceSide,
    length: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListSlice {
    key: String,
    start: Option<CypherReturnListBound>,
    end: Option<CypherReturnListBound>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListIndexProjection {
    key: String,
    index: CypherReturnListBound,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListBound {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListContains {
    key: String,
    needle: Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CypherReturnListPredicate {
    Any,
    All,
    None,
    Single,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListPredicateProjection {
    key: String,
    predicate: CypherReturnListPredicate,
    item_variable: String,
    equals_variable: Option<String>,
    equals: Box<CypherReturnTarget>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CypherReturnStringSliceSide {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnReplace {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
    search: String,
    replacement: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnStringPredicateProjection {
    variable: Option<String>,
    target: Box<CypherReturnTarget>,
    predicate: CypherReturnStringPredicate,
    needle: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CypherReturnStringPredicate {
    StartsWith,
    EndsWith,
    Contains,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherReturnElement {
    Node,
    Edge,
    RowNode,
    RowEdge,
    RowPath,
    Literal,
    Aggregate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherReturnAggregate {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Collect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CypherReturnTargetMaterialization {
    Star,
    Element,
    DirectProperty,
    ScalarProjection,
    ElementFunction,
    PathFunction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CypherReturnScalarProjectionKind {
    Star,
    Element,
    DirectProperty,
    Literal,
    Map,
    List,
    Conditional,
    Coalesce,
    Introspection,
    ListAccess,
    ListPredicate,
    Numeric,
    Conversion,
    String,
    ElementFunction,
    PathFunction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CypherReturnScalarAstFamily {
    Binding,
    Wrapper,
    Value,
    Control,
    Introspection,
    List,
    Numeric,
    Conversion,
    String,
}

pub(crate) enum CypherReturnScalarAst<'a> {
    Star,
    Element,
    DirectProperty(&'a str),
    Literal(&'a Value),
    Map(&'a CypherReturnMapProjection),
    List(&'a CypherReturnListProjection),
    Conditional(&'a CypherReturnCase),
    Coalesce(&'a CypherReturnCoalesce),
    PropertyExists(&'a str),
    PropertySize(&'a str),
    PropertyListIndex(&'a CypherReturnListIndexProjection),
    PropertyListSlice(&'a CypherReturnListSlice),
    PropertyListContains(&'a CypherReturnListContains),
    PropertyListPredicate(&'a CypherReturnListPredicateProjection),
    PropertyListElement(&'a CypherReturnListElementProjection),
    PropertyListTail(&'a CypherReturnListTailProjection),
    PropertyAbs(&'a CypherReturnAbsProjection),
    PropertyNumericRound(&'a CypherReturnNumericRoundProjection),
    PropertyNumericSign(&'a CypherReturnNumericSignProjection),
    PropertyNumericCast(&'a CypherReturnNumericCastProjection),
    PropertyListCast(&'a CypherReturnListCastProjection),
    PropertyToBoolean(&'a CypherReturnToBooleanProjection),
    PropertyToString(&'a CypherReturnToStringProjection),
    PropertyStringTransform(&'a CypherReturnStringTransformProjection),
    PropertyStringTrim(&'a CypherReturnStringTrimProjection),
    PropertyIsEmpty(&'a CypherReturnIsEmptyProjection),
    PropertyStringReverse(&'a CypherReturnStringReverseProjection),
    PropertyStringSplit(&'a CypherReturnStringSplit),
    PropertySubstring(&'a CypherReturnSubstring),
    PropertyStringSlice(&'a CypherReturnStringSlice),
    PropertyReplace(&'a CypherReturnReplace),
    PropertyStringPredicate(&'a CypherReturnStringPredicateProjection),
    ElementFunction,
    PathFunction,
}

pub(crate) fn parse_cypher_return_clause(
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
        let (variable, target) = if let Some((variable, target)) =
            parse_restricted_return_target_expression(expression, parameters)?
        {
            (variable.unwrap_or_default(), target)
        } else if expression.contains('(') || expression.contains(')') {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN only supports bound element, property, and restricted path projections",
            ));
        } else if let Ok((variable, key)) = parse_property_ref(expression, "RETURN projection") {
            (variable, CypherReturnTarget::Property(key))
        } else {
            (
                parse_required_cypher_variable(expression, "RETURN projection")?,
                CypherReturnTarget::Element,
            )
        };
        let element =
            if matches!(target, CypherReturnTarget::Literal(_))
                || matches!(
                    target,
                    CypherReturnTarget::Coalesce(CypherReturnCoalesce { variable: None, .. })
                        | CypherReturnTarget::ListProjection(CypherReturnListProjection {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyStringTransform(
                            CypherReturnStringTransformProjection { variable: None, .. },
                        )
                        | CypherReturnTarget::PropertyStringTrim(
                            CypherReturnStringTrimProjection { variable: None, .. }
                        )
                        | CypherReturnTarget::PropertyStringReverse(
                            CypherReturnStringReverseProjection { variable: None, .. },
                        )
                        | CypherReturnTarget::PropertyIsEmpty(CypherReturnIsEmptyProjection {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyStringSplit(CypherReturnStringSplit {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertySubstring(CypherReturnSubstring {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyStringSlice(CypherReturnStringSlice {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyStringPredicate(
                            CypherReturnStringPredicateProjection { variable: None, .. },
                        )
                        | CypherReturnTarget::PropertyReplace(CypherReturnReplace {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyToString(CypherReturnToStringProjection {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyAbs(CypherReturnAbsProjection {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyNumericRound(
                            CypherReturnNumericRoundProjection { variable: None, .. },
                        )
                        | CypherReturnTarget::PropertyNumericSign(
                            CypherReturnNumericSignProjection { variable: None, .. },
                        )
                        | CypherReturnTarget::PropertyNumericCast(
                            CypherReturnNumericCastProjection { variable: None, .. },
                        )
                        | CypherReturnTarget::PropertyToBoolean(CypherReturnToBooleanProjection {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyListElement(
                            CypherReturnListElementProjection { variable: None, .. },
                        )
                        | CypherReturnTarget::PropertyListTail(CypherReturnListTailProjection {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyListCast(CypherReturnListCastProjection {
                            variable: None,
                            ..
                        })
                )
            {
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
pub(crate) fn split_return_control(clause: &str) -> (&str, &str) {
    match find_return_control_clause(clause) {
        Some(index) => (clause[..index].trim(), clause[index..].trim()),
        None => (clause.trim(), ""),
    }
}

/// Parses a `ORDER BY ... [SKIP/OFFSET n] [LIMIT n]` control clause. Cypher's
/// canonical `ORDER BY`, then row offset, then `LIMIT` ordering is required,
/// and `ORDER BY` terms must reference returned column names or aliases.
pub(crate) fn parse_return_control(
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
pub(crate) fn split_before_keywords<'a>(value: &'a str, keywords: &[&str]) -> (&'a str, &'a str) {
    let split = keywords
        .iter()
        .filter_map(|keyword| find_unquoted_keyword(value, keyword))
        .min();
    match split {
        Some(index) => (value[..index].trim(), &value[index..]),
        None => (value.trim(), ""),
    }
}

pub(crate) fn parse_order_items(items: &str, order_keys: &[Vec<String>]) -> Result<Vec<CypherOrderItem>> {
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

pub(crate) fn parse_aggregate_projection(
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
    if let Some((variable, target)) = parse_restricted_return_target_expression(body, parameters)? {
        return Ok(Some((aggregate, variable, target, distinct)));
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

pub(crate) fn parse_restricted_return_target_expression(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnTarget)>> {
    if let Some((variable, path_target)) = parse_return_path_function_projection(expression)? {
        return Ok(Some((Some(variable), path_target)));
    }
    if let Some((variable, element_target)) = parse_return_element_function_projection(expression)?
    {
        return Ok(Some((Some(variable), element_target)));
    }
    if let Some((variable, coalesce)) = parse_return_coalesce_projection(expression, parameters)? {
        return Ok(Some((variable, CypherReturnTarget::Coalesce(coalesce))));
    }
    if let Some((variable, key)) = parse_return_exists_projection(expression)? {
        return Ok(Some((
            Some(variable),
            CypherReturnTarget::PropertyExists(key),
        )));
    }
    if let Some((variable, key)) = parse_return_size_projection(expression)? {
        return Ok(Some((
            Some(variable),
            CypherReturnTarget::PropertySize(key),
        )));
    }
    if let Some((variable, slice)) = parse_return_list_slice_projection(expression, parameters)? {
        return Ok(Some((
            Some(variable),
            CypherReturnTarget::PropertyListSlice(slice),
        )));
    }
    if let Some((variable, predicate)) =
        parse_return_list_predicate_projection(expression, parameters)?
    {
        return Ok(Some((
            Some(variable),
            CypherReturnTarget::PropertyListPredicate(predicate),
        )));
    }
    if let Some((variable, contains)) =
        parse_return_list_contains_projection(expression, parameters)?
    {
        return Ok(Some((
            Some(variable),
            CypherReturnTarget::PropertyListContains(contains),
        )));
    }
    if let Some((variable, index)) = parse_return_list_index_projection(expression, parameters)? {
        return Ok(Some((
            Some(variable),
            CypherReturnTarget::PropertyListIndex(index),
        )));
    }
    if let Some((variable, element)) = parse_return_list_element_projection(expression, parameters)?
    {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyListElement(element),
        )));
    }
    if let Some((variable, tail)) = parse_return_list_tail_projection(expression, parameters)? {
        return Ok(Some((variable, CypherReturnTarget::PropertyListTail(tail))));
    }
    if let Some((variable, abs)) = parse_return_abs_projection(expression, parameters)? {
        return Ok(Some((variable, CypherReturnTarget::PropertyAbs(abs))));
    }
    if let Some((variable, round)) = parse_return_numeric_round_projection(expression, parameters)?
    {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyNumericRound(round),
        )));
    }
    if let Some((variable, sign)) = parse_return_numeric_sign_projection(expression, parameters)? {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyNumericSign(sign),
        )));
    }
    if let Some((variable, cast)) = parse_return_numeric_cast_projection(expression, parameters)? {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyNumericCast(cast),
        )));
    }
    if let Some((variable, cast)) = parse_return_list_cast_projection(expression, parameters)? {
        return Ok(Some((variable, CypherReturnTarget::PropertyListCast(cast))));
    }
    if let Some((variable, to_boolean)) =
        parse_return_to_boolean_projection(expression, parameters)?
    {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyToBoolean(to_boolean),
        )));
    }
    if let Some((variable, to_string)) = parse_return_to_string_projection(expression, parameters)?
    {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyToString(to_string),
        )));
    }
    if let Some((variable, transform)) =
        parse_return_string_transform_projection(expression, parameters)?
    {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyStringTransform(transform),
        )));
    }
    if let Some((variable, trim)) = parse_return_string_trim_projection(expression, parameters)? {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyStringTrim(trim),
        )));
    }
    if let Some((variable, is_empty)) = parse_return_is_empty_projection(expression, parameters)? {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyIsEmpty(is_empty),
        )));
    }
    if let Some((variable, reverse)) =
        parse_return_string_reverse_projection(expression, parameters)?
    {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyStringReverse(reverse),
        )));
    }
    if let Some((variable, split)) = parse_return_string_split_projection(expression, parameters)? {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyStringSplit(split),
        )));
    }
    if let Some((variable, substring)) = parse_return_substring_projection(expression, parameters)?
    {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertySubstring(substring),
        )));
    }
    if let Some((variable, slice)) = parse_return_string_slice_projection(expression, parameters)? {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyStringSlice(slice),
        )));
    }
    if let Some((variable, replace)) = parse_return_replace_projection(expression, parameters)? {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyReplace(replace),
        )));
    }
    if let Some((variable, predicate)) =
        parse_return_string_predicate_projection(expression, parameters)?
    {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyStringPredicate(predicate),
        )));
    }
    if let Some(range) = parse_return_range_projection(expression, parameters)? {
        return Ok(Some((None, CypherReturnTarget::Literal(range))));
    }
    if let Some((variable, case)) = parse_return_case_projection(expression, parameters)? {
        return Ok(Some((Some(variable), CypherReturnTarget::Case(case))));
    }
    if let Some(literal) = parse_return_literal_projection(expression, parameters)? {
        return Ok(Some((None, CypherReturnTarget::Literal(literal))));
    }
    if let Some(list) = parse_return_list_projection(expression, parameters)? {
        return Ok(Some((
            list.variable.clone(),
            CypherReturnTarget::ListProjection(list),
        )));
    }
    if let Some(map) = parse_return_map_projection(expression, parameters)? {
        return Ok(Some((
            Some(map.variable.clone()),
            CypherReturnTarget::MapProjection(map),
        )));
    }
    Ok(None)
}

pub(crate) fn parse_return_literal_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<Value>> {
    let expression = expression.trim();
    if !is_return_literal_candidate(expression) {
        return Ok(None);
    }
    parse_cypher_literal(expression, parameters).map(Some)
}

pub(crate) fn parse_return_range_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<Value>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("range") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN range projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax("RETURN range projection requires arguments"));
    }
    let arguments = split_top_level_commas(body)?;
    if !(2..=3).contains(&arguments.len()) {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN range requires start, end, and optional step",
        ));
    }
    let start = parse_integer_literal_argument(arguments[0], parameters, "RETURN range start")?;
    let end = parse_integer_literal_argument(arguments[1], parameters, "RETURN range end")?;
    let step = if let Some(step) = arguments.get(2) {
        parse_integer_literal_argument(step, parameters, "RETURN range step")?
    } else {
        1
    };
    restricted_range_value(start, end, step).map(Some)
}

pub(crate) fn parse_integer_literal_argument(
    expression: &str,
    parameters: &CypherParameters,
    context: &str,
) -> Result<i64> {
    let value = parse_cypher_literal(expression.trim(), parameters)?;
    let Value::Int(value) = value else {
        return Err(cypher_unsupported_cardinality(format!(
            "{context} must be an integer literal or parameter"
        )));
    };
    Ok(value)
}

pub(crate) fn restricted_range_value(start: i64, end: i64, step: i64) -> Result<Value> {
    const MAX_RANGE_VALUES: usize = 1_000_000;

    if step == 0 {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN range step must be non-zero",
        ));
    }

    let mut values = Vec::new();
    let mut current = start;
    while (step > 0 && current <= end) || (step < 0 && current >= end) {
        if values.len() == MAX_RANGE_VALUES {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN range would produce too many values",
            ));
        }
        values.push(current);
        let Some(next) = current.checked_add(step) else {
            break;
        };
        current = next;
    }
    Ok(Value::IntArray(values))
}

pub(crate) fn parse_return_list_slice_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(String, CypherReturnListSlice)>> {
    let expression = expression.trim();
    let Some(open) = find_unquoted(expression, '[') else {
        return Ok(None);
    };
    if open == 0 {
        return Ok(None);
    }
    if !expression.ends_with(']') {
        return Err(cypher_syntax("RETURN list slice projection is missing ']'"));
    }
    let target = expression[..open].trim();
    let bounds = expression[open + 1..expression.len() - 1].trim();
    let Some(dotdot) = find_unquoted_sequence(bounds, "..") else {
        return Ok(None);
    };
    if find_unquoted_sequence(&bounds[dotdot + 2..], "..").is_some() {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list slices only support one '..' range",
        ));
    }
    if target.is_empty() {
        return Err(cypher_syntax(
            "RETURN list slice projection requires variable.property[start..end]",
        ));
    }
    if bounds.contains('[') || bounds.contains(']') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list slices only support integer bounds",
        ));
    }
    let start = parse_optional_list_bound(
        bounds[..dotdot].trim(),
        parameters,
        "RETURN list slice start",
    )?;
    let end = parse_optional_list_bound(
        bounds[dotdot + 2..].trim(),
        parameters,
        "RETURN list slice end",
    )?;
    let (variable, key) =
        parse_property_ref(target, "RETURN list slice projection").map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN list slices require a variable.property target",
            )
        })?;
    let mut expected_variable = Some(variable.clone());
    if let Some(start) = &start {
        merge_single_return_variable(
            &mut expected_variable,
            start.variable.clone(),
            "writable Cypher RETURN list slice bounds must reference the list target variable",
        )?;
    }
    if let Some(end) = &end {
        merge_single_return_variable(
            &mut expected_variable,
            end.variable.clone(),
            "writable Cypher RETURN list slice bounds must reference the list target variable",
        )?;
    }
    Ok(Some((variable, CypherReturnListSlice { key, start, end })))
}

pub(crate) fn parse_optional_list_bound(
    expression: &str,
    parameters: &CypherParameters,
    context: &str,
) -> Result<Option<CypherReturnListBound>> {
    if expression.is_empty() {
        return Ok(None);
    }
    parse_return_list_bound(expression, parameters, context).map(Some)
}

pub(crate) fn parse_return_list_bound(
    expression: &str,
    parameters: &CypherParameters,
    context: &str,
) -> Result<CypherReturnListBound> {
    let (variable, target) = parse_nested_restricted_scalar_target(
        expression,
        parameters,
        context,
        "writable Cypher RETURN list indexes and slices only support restricted scalar integer bounds",
    )?;
    Ok(CypherReturnListBound {
        variable,
        target: Box::new(target),
    })
}

pub(crate) fn parse_return_list_contains_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(String, CypherReturnListContains)>> {
    let expression = expression.trim();
    let Some(in_index) = find_unquoted_keyword(expression, "IN") else {
        return Ok(None);
    };
    if expression.contains('(') || expression.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN IN only supports literal IN variable.property",
        ));
    }
    let needle = expression[..in_index].trim();
    let haystack = expression[in_index + "IN".len()..].trim();
    if needle.is_empty() || haystack.is_empty() {
        return Err(cypher_syntax(
            "RETURN list membership projection requires needle IN variable.property",
        ));
    }
    let needle = parse_cypher_literal(needle, parameters).map_err(|_| {
        cypher_unsupported_cardinality(
            "writable Cypher RETURN IN needle must be a literal or parameter",
        )
    })?;
    let (variable, key) = parse_property_ref(haystack, "RETURN list membership projection")
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN IN requires a variable.property haystack",
            )
        })?;
    Ok(Some((variable, CypherReturnListContains { key, needle })))
}

pub(crate) fn parse_return_list_predicate_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(String, CypherReturnListPredicateProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let predicate = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "any" => CypherReturnListPredicate::Any,
        "all" => CypherReturnListPredicate::All,
        "none" => CypherReturnListPredicate::None,
        "single" => CypherReturnListPredicate::Single,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN list predicate projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN list predicate projection requires item IN variable.property WHERE item = value",
        ));
    }
    let Some(in_index) = find_unquoted_keyword(body, "IN") else {
        return Err(cypher_syntax(
            "RETURN list predicate projection requires item IN variable.property WHERE item = value",
        ));
    };
    let item_variable = parse_required_cypher_variable(
        body[..in_index].trim(),
        "RETURN list predicate item variable",
    )?;
    let rest = body[in_index + "IN".len()..].trim();
    let Some(where_index) = find_unquoted_keyword(rest, "WHERE") else {
        return Err(cypher_syntax(
            "RETURN list predicate projection requires WHERE item = value",
        ));
    };
    let haystack = rest[..where_index].trim();
    let condition = rest[where_index + "WHERE".len()..].trim();
    let Some(equals_index) = find_unquoted(condition, '=') else {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list predicates only support equality predicates",
        ));
    };
    if find_unquoted(&condition[equals_index + 1..], '=').is_some() {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list predicates only support one equality predicate",
        ));
    }
    let left = condition[..equals_index].trim();
    let right = condition[equals_index + 1..].trim();
    if left != item_variable {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list predicates require the WHERE left side to be the list item variable",
        ));
    }
    if right.is_empty() {
        return Err(cypher_syntax(
            "RETURN list predicate projection requires an equality value",
        ));
    }
    let (variable, key) = parse_property_ref(haystack, "RETURN list predicate projection")
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN list predicates require a variable.property haystack",
            )
        })?;
    let (equals_variable, equals) = parse_nested_restricted_scalar_target(
        right,
        parameters,
        "RETURN list predicate equality value",
        "writable Cypher RETURN list predicate equality value must be a restricted scalar value",
    )?;
    if let Some(equals_variable) = equals_variable.as_ref()
        && equals_variable != &variable
    {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list predicate equality value must reference the haystack variable",
        ));
    }
    Ok(Some((
        variable,
        CypherReturnListPredicateProjection {
            key,
            predicate,
            item_variable,
            equals_variable,
            equals: Box::new(equals),
        },
    )))
}

pub(crate) fn parse_return_list_index_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(String, CypherReturnListIndexProjection)>> {
    let expression = expression.trim();
    let Some(open) = find_unquoted(expression, '[') else {
        return Ok(None);
    };
    if open == 0 {
        return Ok(None);
    }
    if !expression.ends_with(']') {
        return Err(cypher_syntax("RETURN list index projection is missing ']'"));
    }
    let target = expression[..open].trim();
    let index = expression[open + 1..expression.len() - 1].trim();
    if target.is_empty() || index.is_empty() {
        return Err(cypher_syntax(
            "RETURN list index projection requires variable.property[index]",
        ));
    }
    if index.contains('[') || index.contains(']') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list indexes only support a single integer index",
        ));
    }
    let (variable, key) =
        parse_property_ref(target, "RETURN list index projection").map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN list indexes require a variable.property target",
            )
        })?;
    let index = parse_return_list_bound(index, parameters, "RETURN list index")?;
    let mut expected_variable = Some(variable.clone());
    merge_single_return_variable(
        &mut expected_variable,
        index.variable.clone(),
        "writable Cypher RETURN list index expressions must reference the list target variable",
    )?;
    Ok(Some((
        variable,
        CypherReturnListIndexProjection { key, index },
    )))
}

pub(crate) fn is_return_literal_candidate(expression: &str) -> bool {
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

pub(crate) fn parse_return_coalesce_projection(
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
        let (argument_variable, target) = parse_return_coalesce_argument(argument, parameters)?;
        merge_return_coalesce_variable(&mut variable, argument_variable)?;
        terms.push(target);
    }

    Ok(Some((
        variable.clone(),
        CypherReturnCoalesce { variable, terms },
    )))
}

pub(crate) fn parse_return_coalesce_argument(
    argument: &str,
    parameters: &CypherParameters,
) -> Result<(Option<String>, CypherReturnTarget)> {
    parse_nested_restricted_scalar_target(
        argument,
        parameters,
        "RETURN coalesce projection",
        "writable Cypher RETURN coalesce only supports restricted scalar arguments",
    )
}

pub(crate) fn parse_nested_restricted_scalar_target(
    expression: &str,
    parameters: &CypherParameters,
    property_context: &str,
    unsupported_message: &'static str,
) -> Result<(Option<String>, CypherReturnTarget)> {
    if let Some((variable, target)) =
        parse_restricted_return_target_expression(expression, parameters)?
    {
        if matches!(
            target,
            CypherReturnTarget::ListProjection(_) | CypherReturnTarget::MapProjection(_)
        ) {
            return Err(cypher_unsupported_cardinality(unsupported_message));
        }
        return Ok((variable, target));
    }
    let (variable, key) = parse_property_ref(expression, property_context)
        .map_err(|_| cypher_unsupported_cardinality(unsupported_message))?;
    Ok((Some(variable), CypherReturnTarget::Property(key)))
}

pub(crate) fn merge_return_coalesce_variable(
    variable: &mut Option<String>,
    argument_variable: Option<String>,
) -> Result<()> {
    merge_single_return_variable(
        variable,
        argument_variable,
        "writable Cypher RETURN coalesce arguments must reference one variable",
    )
}

pub(crate) fn merge_single_return_variable(
    variable: &mut Option<String>,
    argument_variable: Option<String>,
    message: &'static str,
) -> Result<()> {
    let Some(argument_variable) = argument_variable else {
        return Ok(());
    };
    if let Some(variable) = variable {
        if variable != &argument_variable {
            return Err(cypher_unsupported_cardinality(message));
        }
    } else {
        *variable = Some(argument_variable);
    }
    Ok(())
}

pub(crate) fn parse_return_exists_projection(expression: &str) -> Result<Option<(String, String)>> {
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

pub(crate) fn parse_return_size_projection(expression: &str) -> Result<Option<(String, String)>> {
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

pub(crate) fn parse_return_list_element_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnListElementProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let element = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "head" => CypherReturnListElement::Head,
        "last" => CypherReturnListElement::Last,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN list element projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN list element projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN list element projection",
        "writable Cypher RETURN head/last only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnListElementProjection {
            variable,
            target: Box::new(target),
            element,
        },
    )))
}

pub(crate) fn parse_return_list_tail_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnListTailProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("tail") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN tail projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax("RETURN tail projection requires an argument"));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN tail projection",
        "writable Cypher RETURN tail only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnListTailProjection {
            variable,
            target: Box::new(target),
        },
    )))
}

pub(crate) fn parse_return_abs_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnAbsProjection)>> {
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
        return Err(cypher_syntax("RETURN abs projection requires an argument"));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN abs projection",
        "writable Cypher RETURN abs only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnAbsProjection {
            variable,
            target: Box::new(target),
        },
    )))
}

pub(crate) fn parse_return_numeric_round_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnNumericRoundProjection)>> {
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
            "RETURN numeric rounding projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN numeric rounding projection",
        "writable Cypher RETURN ceil/floor only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnNumericRoundProjection {
            variable,
            target: Box::new(target),
            round,
        },
    )))
}

pub(crate) fn parse_return_numeric_sign_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnNumericSignProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("sign") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN sign projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax("RETURN sign projection requires an argument"));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN sign projection",
        "writable Cypher RETURN sign only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnNumericSignProjection {
            variable,
            target: Box::new(target),
        },
    )))
}

pub(crate) fn parse_return_numeric_cast_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnNumericCastProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let cast = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "tointeger" => CypherReturnNumericCast::Integer,
        "tofloat" => CypherReturnNumericCast::Float,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN numeric cast projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN numeric cast projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN numeric cast projection",
        "writable Cypher RETURN toInteger/toFloat only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnNumericCastProjection {
            variable,
            target: Box::new(target),
            cast,
        },
    )))
}

pub(crate) fn parse_return_list_cast_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnListCastProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let cast = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "tostringlist" => CypherReturnListCast::String,
        "tointegerlist" => CypherReturnListCast::Integer,
        "tofloatlist" => CypherReturnListCast::Float,
        "tobooleanlist" => CypherReturnListCast::Boolean,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN list cast projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN list cast projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN list cast projection",
        "writable Cypher RETURN list casts only support restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnListCastProjection {
            variable,
            target: Box::new(target),
            cast,
        },
    )))
}

pub(crate) fn parse_return_to_boolean_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnToBooleanProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("toBoolean") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN toBoolean projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN toBoolean projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN toBoolean projection",
        "writable Cypher RETURN toBoolean only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnToBooleanProjection {
            variable,
            target: Box::new(target),
        },
    )))
}

pub(crate) fn parse_return_to_string_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnToStringProjection)>> {
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
            "RETURN toString projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN toString projection",
        "writable Cypher RETURN toString only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnToStringProjection {
            variable,
            target: Box::new(target),
        },
    )))
}

pub(crate) fn parse_return_is_empty_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnIsEmptyProjection)>> {
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
            "RETURN isEmpty projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN isEmpty projection",
        "writable Cypher RETURN isEmpty only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnIsEmptyProjection {
            variable,
            target: Box::new(target),
        },
    )))
}

pub(crate) fn parse_return_string_transform_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnStringTransformProjection)>> {
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
            "RETURN string transform projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN string transform projection",
        "writable Cypher RETURN string transforms only support restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnStringTransformProjection {
            variable,
            target: Box::new(target),
            transform,
        },
    )))
}

pub(crate) fn parse_return_string_trim_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnStringTrimProjection)>> {
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
            "RETURN string trim projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN string trim projection",
        "writable Cypher RETURN string trims only support restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnStringTrimProjection {
            variable,
            target: Box::new(target),
            trim,
        },
    )))
}

pub(crate) fn parse_return_string_reverse_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnStringReverseProjection)>> {
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
            "RETURN string reverse projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN string reverse projection",
        "writable Cypher RETURN reverse only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnStringReverseProjection {
            variable,
            target: Box::new(target),
        },
    )))
}

pub(crate) fn parse_return_string_split_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnStringSplit)>> {
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
            "writable Cypher RETURN split requires a restricted scalar argument and delimiter",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        arguments[0].trim(),
        parameters,
        "RETURN string split projection",
        "writable Cypher RETURN split requires a restricted scalar first argument",
    )?;
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
    Ok(Some((
        variable.clone(),
        CypherReturnStringSplit {
            variable,
            target: Box::new(target),
            delimiter,
        },
    )))
}

pub(crate) fn parse_return_substring_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnSubstring)>> {
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
            "writable Cypher RETURN substring requires a restricted scalar argument, start, and optional length",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        arguments[0].trim(),
        parameters,
        "RETURN substring projection",
        "writable Cypher RETURN substring requires a restricted scalar first argument",
    )?;
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
        variable.clone(),
        CypherReturnSubstring {
            variable,
            target: Box::new(target),
            start,
            length,
        },
    )))
}

pub(crate) fn parse_non_negative_usize_literal(
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

pub(crate) fn parse_return_string_slice_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnStringSlice)>> {
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
            "writable Cypher RETURN left/right requires a restricted scalar argument and length",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        arguments[0].trim(),
        parameters,
        "RETURN string slice projection",
        "writable Cypher RETURN left/right requires a restricted scalar first argument",
    )?;
    let length = parse_non_negative_usize_literal(
        arguments[1].trim(),
        parameters,
        "RETURN left/right length",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnStringSlice {
            variable,
            target: Box::new(target),
            side,
            length,
        },
    )))
}

pub(crate) fn parse_return_replace_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnReplace)>> {
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
            "writable Cypher RETURN replace requires a restricted scalar argument, search, and replacement",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        arguments[0].trim(),
        parameters,
        "RETURN replace projection",
        "writable Cypher RETURN replace requires a restricted scalar first argument",
    )?;
    let search =
        parse_string_literal_argument(arguments[1].trim(), parameters, "RETURN replace search")?;
    let replacement = parse_string_literal_argument(
        arguments[2].trim(),
        parameters,
        "RETURN replace replacement",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnReplace {
            variable,
            target: Box::new(target),
            search,
            replacement,
        },
    )))
}

pub(crate) fn parse_return_string_predicate_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnStringPredicateProjection)>> {
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
            "writable Cypher RETURN string predicates require a restricted scalar argument and needle",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        arguments[0].trim(),
        parameters,
        "RETURN string predicate projection",
        "writable Cypher RETURN string predicates require a restricted scalar first argument",
    )?;
    let needle = parse_string_literal_argument(
        arguments[1].trim(),
        parameters,
        "RETURN string predicate needle",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnStringPredicateProjection {
            variable,
            target: Box::new(target),
            predicate,
            needle,
        },
    )))
}

pub(crate) fn parse_string_literal_argument(
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

pub(crate) fn parse_return_element_function_projection(
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

pub(crate) fn parse_return_map_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<CypherReturnMapProjection>> {
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
            "RETURN map projection requires at least one entry",
        ));
    }
    let mut entries = Vec::new();
    for selector in split_top_level_commas(body)? {
        let selector = selector.trim();
        if selector.is_empty() {
            return Err(cypher_syntax(
                "RETURN map projection contains an empty entry",
            ));
        }
        let entry = if let Some(key) = selector.strip_prefix('.') {
            let key = key.trim();
            validate_json_key(key)?;
            CypherReturnMapProjectionEntry {
                output_key: key.to_string(),
                value: CypherReturnTarget::Property(key.to_string()),
            }
        } else {
            let Some(colon) = find_unquoted(selector, ':') else {
                return Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN map projections only support .property selectors and key: literal/property entries",
                ));
            };
            if find_unquoted(&selector[colon + 1..], ':').is_some() {
                return Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN map projection entries only support one ':' separator",
                ));
            }
            let output_key = selector[..colon].trim();
            validate_json_key(output_key)?;
            let value = selector[colon + 1..].trim();
            if value.is_empty() {
                return Err(cypher_syntax(
                    "RETURN map projection entry requires a value",
                ));
            }
            let value = parse_return_map_projection_value(value, &variable, parameters)?;
            CypherReturnMapProjectionEntry {
                output_key: output_key.to_string(),
                value,
            }
        };
        if entries
            .iter()
            .any(|existing: &CypherReturnMapProjectionEntry| {
                existing.output_key == entry.output_key
            })
        {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN map projection entries must have unique output keys",
            ));
        }
        entries.push(entry);
    }
    Ok(Some(CypherReturnMapProjection { variable, entries }))
}

pub(crate) fn parse_return_map_projection_value(
    value: &str,
    map_variable: &str,
    parameters: &CypherParameters,
) -> Result<CypherReturnTarget> {
    let (value_variable, target) = parse_nested_restricted_scalar_target(
        value,
        parameters,
        "RETURN map projection entry",
        "writable Cypher RETURN map projection entries only support restricted scalar values",
    )?;
    if let Some(value_variable) = value_variable
        && value_variable != map_variable
    {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN map projection values must reference the projection variable",
        ));
    }
    Ok(target)
}

pub(crate) fn parse_return_list_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<CypherReturnListProjection>> {
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
            "RETURN list projection requires at least one item",
        ));
    }
    let mut variable = None;
    let mut terms = Vec::new();
    for item in split_top_level_commas(body)? {
        let item = item.trim();
        if item.is_empty() {
            return Err(cypher_syntax(
                "RETURN list projection contains an empty item",
            ));
        }
        let (item_variable, target) = parse_return_list_projection_term(item, parameters)?;
        merge_return_list_projection_variable(&mut variable, item_variable)?;
        terms.push(target);
    }
    Ok(Some(CypherReturnListProjection { variable, terms }))
}

pub(crate) fn parse_return_list_projection_term(
    item: &str,
    parameters: &CypherParameters,
) -> Result<(Option<String>, CypherReturnTarget)> {
    parse_nested_restricted_scalar_target(
        item,
        parameters,
        "RETURN list projection",
        "writable Cypher RETURN list projections only support restricted scalar items",
    )
}

pub(crate) fn merge_return_list_projection_variable(
    variable: &mut Option<String>,
    item_variable: Option<String>,
) -> Result<()> {
    let Some(item_variable) = item_variable else {
        return Ok(());
    };
    if let Some(variable) = variable {
        if variable != &item_variable {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN list projections must reference one variable",
            ));
        }
    } else {
        *variable = Some(item_variable);
    }
    Ok(())
}

pub(crate) fn parse_return_case_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(String, CypherReturnCase)>> {
    let expression = expression.trim();
    let Some(after_case) = strip_leading_keyword(expression, "CASE") else {
        return Ok(None);
    };
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
    )
    .map_err(|_| {
        cypher_unsupported_cardinality(
            "writable Cypher RETURN CASE only supports property equality predicates",
        )
    })?;
    let equals = parse_cypher_literal(&condition[equals_index + 1..], parameters)?;
    let (then_variable, then_target) = parse_nested_restricted_scalar_target(
        then_value,
        parameters,
        "RETURN CASE THEN value",
        "writable Cypher RETURN CASE branches only support restricted scalar values",
    )?;
    let (else_variable, else_target) = parse_nested_restricted_scalar_target(
        else_value,
        parameters,
        "RETURN CASE ELSE value",
        "writable Cypher RETURN CASE branches only support restricted scalar values",
    )?;
    let mut branch_variable = Some(variable.clone());
    merge_single_return_variable(
        &mut branch_variable,
        then_variable,
        "writable Cypher RETURN CASE branches must reference the predicate variable",
    )?;
    merge_single_return_variable(
        &mut branch_variable,
        else_variable,
        "writable Cypher RETURN CASE branches must reference the predicate variable",
    )?;
    Ok(Some((
        variable,
        CypherReturnCase {
            key,
            equals,
            then_target: Box::new(then_target),
            else_target: Box::new(else_target),
        },
    )))
}

pub(crate) fn parse_return_path_function_projection(
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

pub(crate) fn append_star_return_projections(
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

pub(crate) fn cypher_return_element_for_variable(
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

pub(crate) fn row_edge_endpoint_variable(
    variable: &str,
    row_edge_bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
) -> bool {
    row_edge_bindings
        .values()
        .any(|binding| binding.from_variable == variable || binding.to_variable == variable)
}

pub(crate) fn validate_return_variable_binding(
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

pub(crate) fn validate_return_function_target(
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

pub(crate) fn strip_trailing_keyword<'a>(value: &'a str, keyword: &str) -> Option<&'a str> {
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

pub(crate) fn parse_return_count(value: &str, context: &str) -> Result<usize> {
    let value = value.trim();
    value.parse::<usize>().map_err(|_| {
        cypher_syntax(format!(
            "{context} requires a non-negative integer, got '{value}'"
        ))
    })
}

pub(crate) fn parse_return_limit(value: &str) -> Result<Option<usize>> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("ALL") {
        return Ok(None);
    }
    Ok(Some(parse_return_count(value, "LIMIT")?))
}

pub(crate) fn split_return_alias(projection: &str) -> Result<(&str, Option<String>)> {
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

pub(crate) struct PatchAssignment {
    target: String,
    kind: PatchAssignmentKind,
}

pub(crate) enum PatchAssignmentKind {
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

pub(crate) fn parse_patch_assignment(
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

pub(crate) fn parse_patch_assignments(
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

pub fn cypher_written_edge_identity(
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

pub fn cypher_written_node_identity(
    kind: GraphMutationPlanKind,
    node: &Node,
) -> CypherWrittenNodeIdentity {
    CypherWrittenNodeIdentity {
        kind,
        label: node.label.clone(),
        id: node.id.clone(),
    }
}

pub(crate) fn cypher_mutation_result_from_plan(
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
pub(crate) struct CypherWriteResultRows<'a> {
    row_nodes: &'a HashMap<String, Vec<Node>>,
    row_edges: &'a HashMap<String, Vec<Edge>>,
    row_paths: &'a HashMap<String, CypherRowProducedPathBinding>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CypherWriteResultBindingKind {
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

    fn path_row_count(&self, variable: &str) -> Result<usize> {
        let path = self.row_paths.get(variable).ok_or_else(|| {
            cypher_unsupported_cardinality(format!(
                "writable Cypher RETURN cannot materialize path variable '{variable}'"
            ))
        })?;
        let mut row_count = None;
        if let Some(edges) = self.row_edges(&path.edge_variable) {
            Self::merge_row_count(&mut row_count, edges.len())?;
        }
        for endpoint in [&path.from_variable, &path.to_variable] {
            if let Some(nodes) = self.row_nodes(endpoint) {
                Self::merge_row_count(&mut row_count, nodes.len())?;
            }
        }
        Ok(row_count.unwrap_or(1))
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
                    self.path_row_count(&projection.variable)?
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

pub async fn evaluate_cypher_return_table<S>(
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

pub(crate) struct CypherReturnGroup {
    scalar_values: Vec<Value>,
    aggregate_states: Vec<Option<CypherGroupedAggregateState>>,
}

pub(crate) enum CypherGroupedAggregateState {
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
    let _kind = classify_return_scalar_projection(&projection.target);
    let expression = scalar_return_ast(&projection.target);
    match classify_return_scalar_ast_family(&expression) {
        CypherReturnScalarAstFamily::Binding => {
            evaluate_scalar_binding_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
        CypherReturnScalarAstFamily::Wrapper => {
            evaluate_scalar_wrapper_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
        CypherReturnScalarAstFamily::Value => {
            evaluate_scalar_value_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
        CypherReturnScalarAstFamily::Control => {
            evaluate_scalar_control_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
        CypherReturnScalarAstFamily::Introspection => {
            evaluate_scalar_introspection_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
        CypherReturnScalarAstFamily::List => {
            evaluate_scalar_list_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
        CypherReturnScalarAstFamily::Numeric => {
            evaluate_scalar_numeric_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
        CypherReturnScalarAstFamily::Conversion => {
            evaluate_scalar_conversion_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
        CypherReturnScalarAstFamily::String => {
            evaluate_scalar_string_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn evaluate_scalar_binding_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::Star => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN * is only supported inside COUNT(*) or COLLECT(*)",
        )),
        CypherReturnScalarAst::Element => {
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
        CypherReturnScalarAst::DirectProperty(key) => {
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
        _ => unreachable!("non-binding scalar expression routed to binding evaluator"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn evaluate_scalar_wrapper_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::ElementFunction => {
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
        CypherReturnScalarAst::PathFunction => {
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
        _ => unreachable!("non-wrapper scalar expression routed to wrapper evaluator"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn evaluate_scalar_value_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::Literal(value) => Ok(value.clone()),
        CypherReturnScalarAst::Map(map) => {
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
                map,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::List(list) => {
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
                list,
                row_index,
            )
            .await
        }
        _ => unreachable!("non-value scalar expression routed to value expression evaluator"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn evaluate_scalar_control_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::Conditional(case) => {
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
        CypherReturnScalarAst::Coalesce(coalesce) => {
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
        _ => unreachable!("non-control scalar expression routed to control expression evaluator"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn evaluate_scalar_introspection_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::PropertyExists(key) => {
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
        CypherReturnScalarAst::PropertySize(key) => {
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
        _ => {
            unreachable!("non-introspection scalar expression routed to introspection evaluator")
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn evaluate_scalar_numeric_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::PropertyAbs(abs) => {
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
                abs,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyNumericRound(round) => {
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
                round,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyNumericSign(sign) => {
            materialize_return_property_numeric_sign_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                sign,
                row_index,
            )
            .await
        }
        _ => unreachable!("non-numeric scalar expression routed to numeric expression evaluator"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn evaluate_scalar_conversion_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::PropertyNumericCast(cast) => {
            materialize_return_property_numeric_cast_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                cast,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyListCast(cast) => {
            materialize_return_property_list_cast_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                cast,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyToBoolean(to_boolean) => {
            materialize_return_property_to_boolean_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                to_boolean,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyToString(to_string) => {
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
                to_string,
                row_index,
            )
            .await
        }
        _ => {
            unreachable!(
                "non-conversion scalar expression routed to conversion expression evaluator"
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn evaluate_scalar_list_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::PropertyListIndex(index) => {
            materialize_return_property_list_index_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                index,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyListSlice(slice) => {
            materialize_return_property_list_slice_value_at(
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
        CypherReturnScalarAst::PropertyListContains(contains) => {
            materialize_return_property_list_contains_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                contains,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyListPredicate(predicate) => {
            materialize_return_property_list_predicate_value_at(
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
        CypherReturnScalarAst::PropertyListElement(element) => {
            materialize_return_property_list_element_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                element,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyListTail(tail) => {
            materialize_return_property_list_tail_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                tail,
                row_index,
            )
            .await
        }
        _ => unreachable!("non-list scalar expression routed to list expression evaluator"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn evaluate_scalar_string_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::PropertyStringTransform(transform) => {
            materialize_return_string_transform_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                transform,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyStringTrim(trim) => {
            materialize_return_string_trim_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                trim,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyIsEmpty(is_empty) => {
            materialize_return_is_empty_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                is_empty,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyStringReverse(reverse) => {
            materialize_return_string_reverse_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                reverse,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyStringSplit(split) => {
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
        CypherReturnScalarAst::PropertySubstring(substring) => {
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
        CypherReturnScalarAst::PropertyStringSlice(slice) => {
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
        CypherReturnScalarAst::PropertyReplace(replace) => {
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
        CypherReturnScalarAst::PropertyStringPredicate(predicate) => {
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
        _ => unreachable!("non-string scalar expression routed to string expression evaluator"),
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
    match classify_return_target_materialization(&projection.target) {
        CypherReturnTargetMaterialization::Star
            if aggregate == CypherReturnAggregate::Count && !projection.distinct =>
        {
            Ok(vec![Value::Int(1)])
        }
        CypherReturnTargetMaterialization::Star if aggregate == CypherReturnAggregate::Collect => {
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
        CypherReturnTargetMaterialization::Star => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN aggregates other than COUNT and COLLECT require variable.property",
        )),
        CypherReturnTargetMaterialization::Element
            if aggregate == CypherReturnAggregate::Count && !projection.distinct =>
        {
            Ok(vec![Value::Int(1)])
        }
        CypherReturnTargetMaterialization::Element
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
        CypherReturnTargetMaterialization::Element => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN aggregates other than COUNT and COLLECT require variable.property",
        )),
        CypherReturnTargetMaterialization::DirectProperty => {
            let CypherReturnTarget::Property(key) = &projection.target else {
                unreachable!("direct property classification requires property target");
            };
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
        CypherReturnTargetMaterialization::ElementFunction => {
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
        CypherReturnTargetMaterialization::PathFunction => {
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
        CypherReturnTargetMaterialization::ScalarProjection => {
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
    map: &CypherReturnMapProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let mut output = serde_json::Map::new();
    for entry in &map.entries {
        let value = Box::pin(evaluate_scalar_return_projection(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            &CypherReturnProjection {
                target: entry.value.clone(),
                variable: map.variable.clone(),
                column: projection.column.clone(),
                expression: projection.expression.clone(),
                element: projection.element,
                aggregate: None,
                distinct: false,
            },
            row_index,
        ))
        .await?
        .to_json();
        output.insert(entry.output_key.clone(), value);
    }
    Ok(Value::Json(serde_json::Value::Object(output)))
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
    list: &CypherReturnListProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let mut values = Vec::with_capacity(list.terms.len());
    for term in &list.terms {
        let value = Box::pin(evaluate_scalar_return_projection(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            &CypherReturnProjection {
                target: term.clone(),
                variable: list
                    .variable
                    .clone()
                    .unwrap_or_else(|| projection.variable.clone()),
                column: projection.column.clone(),
                expression: projection.expression.clone(),
                element: projection.element,
                aggregate: None,
                distinct: false,
            },
            row_index,
        ))
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
    let target = if actual == case.equals {
        case.then_target.as_ref()
    } else {
        case.else_target.as_ref()
    };
    Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: target.clone(),
            variable: projection.variable.clone(),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await
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
        let value = Box::pin(evaluate_scalar_return_projection(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            &CypherReturnProjection {
                target: term.clone(),
                variable: coalesce
                    .variable
                    .clone()
                    .unwrap_or_else(|| projection.variable.clone()),
                column: projection.column.clone(),
                expression: projection.expression.clone(),
                element: projection.element,
                aggregate: None,
                distinct: false,
            },
            row_index,
        ))
        .await?;
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

pub(crate) fn restricted_size_value(value: Value) -> Result<Value> {
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
async fn materialize_return_property_list_index_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    index: &CypherReturnListIndexProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let index_value = evaluate_return_list_bound_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        &index.index,
        row_index,
        "RETURN list index",
    )
    .await?;
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
        &index.key,
        row_index,
    )
    .await?;
    restricted_list_index_value(value, index_value)
}

#[allow(clippy::too_many_arguments)]
async fn evaluate_return_list_bound_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    bound: &CypherReturnListBound,
    row_index: usize,
    context: &str,
) -> Result<usize>
where
    S: GraphStore + Sync,
{
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*bound.target).clone(),
            variable: bound
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    let Value::Int(value) = value else {
        return Err(cypher_unsupported_cardinality(format!(
            "{context} must evaluate to an integer"
        )));
    };
    usize::try_from(value).map_err(|_| {
        cypher_unsupported_cardinality(format!("{context} must evaluate to a non-negative integer"))
    })
}

pub(crate) fn restricted_list_index_value(value: Value, index: usize) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::StringArray(values) => Ok(values.get(index).map(Value::from).unwrap_or(Value::Null)),
        Value::IntArray(values) => Ok(values
            .get(index)
            .copied()
            .map(Value::Int)
            .unwrap_or(Value::Null)),
        Value::FloatArray(values) => Ok(values
            .get(index)
            .copied()
            .map(Value::Float)
            .unwrap_or(Value::Null)),
        Value::Json(serde_json::Value::Array(values)) => Ok(values
            .get(index)
            .cloned()
            .map(Value::from_json)
            .unwrap_or(Value::Null)),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list indexes only support array values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_list_slice_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    slice: &CypherReturnListSlice,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let start = if let Some(start) = &slice.start {
        Some(
            evaluate_return_list_bound_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                start,
                row_index,
                "RETURN list slice start",
            )
            .await?,
        )
    } else {
        None
    };
    let end = if let Some(end) = &slice.end {
        Some(
            evaluate_return_list_bound_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                end,
                row_index,
                "RETURN list slice end",
            )
            .await?,
        )
    } else {
        None
    };
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
    restricted_list_slice_value(value, start, end)
}

pub(crate) fn bounded_list_slice_indexes(
    len: usize,
    start: Option<usize>,
    end: Option<usize>,
) -> (usize, usize) {
    let start = start.unwrap_or(0).min(len);
    let end = end.unwrap_or(len).min(len);
    if end < start {
        (start, start)
    } else {
        (start, end)
    }
}

pub(crate) fn restricted_list_slice_value(
    value: Value,
    start: Option<usize>,
    end: Option<usize>,
) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::StringArray(values) => {
            let (start, end) = bounded_list_slice_indexes(values.len(), start, end);
            Ok(Value::StringArray(values[start..end].to_vec()))
        }
        Value::IntArray(values) => {
            let (start, end) = bounded_list_slice_indexes(values.len(), start, end);
            Ok(Value::IntArray(values[start..end].to_vec()))
        }
        Value::FloatArray(values) => {
            let (start, end) = bounded_list_slice_indexes(values.len(), start, end);
            Ok(Value::FloatArray(values[start..end].to_vec()))
        }
        Value::Json(serde_json::Value::Array(values)) => {
            let (start, end) = bounded_list_slice_indexes(values.len(), start, end);
            Ok(Value::from_json(serde_json::Value::Array(
                values[start..end].to_vec(),
            )))
        }
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list slices only support array values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_list_contains_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    contains: &CypherReturnListContains,
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
        &contains.key,
        row_index,
    )
    .await?;
    restricted_list_contains_value(value, &contains.needle)
}

pub(crate) fn restricted_list_contains_value(value: Value, needle: &Value) -> Result<Value> {
    if matches!(needle, Value::Null) {
        return Ok(Value::Null);
    }
    match value {
        Value::Null => Ok(Value::Null),
        Value::StringArray(values) => {
            Ok(Value::Bool(needle.as_str().is_some_and(|needle| {
                values.iter().any(|value| value == needle)
            })))
        }
        Value::IntArray(values) => Ok(Value::Bool(matches!(
            needle,
            Value::Int(needle) if values.iter().any(|value| value == needle)
        ))),
        Value::FloatArray(values) => Ok(Value::Bool(matches!(
            needle,
            Value::Float(needle) if values.iter().any(|value| value == needle)
        ))),
        Value::Json(serde_json::Value::Array(values)) => Ok(Value::Bool(
            values
                .into_iter()
                .map(Value::from_json)
                .any(|value| &value == needle),
        )),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN IN only supports array values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_list_predicate_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    predicate: &CypherReturnListPredicateProjection,
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
    let needle = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: predicate.equals.as_ref().clone(),
            variable: predicate
                .equals_variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_list_predicate_value(value, needle, predicate.predicate)
}

pub(crate) fn restricted_list_predicate_value(
    value: Value,
    needle: Value,
    predicate: CypherReturnListPredicate,
) -> Result<Value> {
    if matches!(needle, Value::Null) {
        return Ok(Value::Null);
    }
    match value {
        Value::Null => Ok(Value::Null),
        Value::StringArray(values) => Ok(Value::Bool(evaluate_list_predicate_matches(
            values
                .iter()
                .map(|value| matches!(needle.as_str(), Some(needle) if value == needle)),
            predicate,
        ))),
        Value::IntArray(values) => Ok(Value::Bool(evaluate_list_predicate_matches(
            values
                .iter()
                .map(|value| matches!(needle, Value::Int(needle) if *value == needle)),
            predicate,
        ))),
        Value::FloatArray(values) => Ok(Value::Bool(evaluate_list_predicate_matches(
            values
                .iter()
                .map(|value| matches!(needle, Value::Float(needle) if *value == needle)),
            predicate,
        ))),
        Value::Json(serde_json::Value::Array(values)) => {
            Ok(Value::Bool(evaluate_list_predicate_matches(
                values
                    .into_iter()
                    .map(Value::from_json)
                    .map(|value| value == needle),
                predicate,
            )))
        }
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list predicates only support array values",
        )),
    }
}

pub(crate) fn evaluate_list_predicate_matches(
    matches: impl IntoIterator<Item = bool>,
    predicate: CypherReturnListPredicate,
) -> bool {
    let mut total = 0usize;
    let mut matched = 0usize;
    for is_match in matches {
        total += 1;
        if is_match {
            matched += 1;
        }
    }
    match predicate {
        CypherReturnListPredicate::Any => matched > 0,
        CypherReturnListPredicate::All => matched == total,
        CypherReturnListPredicate::None => matched == 0,
        CypherReturnListPredicate::Single => matched == 1,
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_list_element_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    element: &CypherReturnListElementProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*element.target).clone(),
            variable: element
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_list_element_value(value, element.element)
}

pub(crate) fn restricted_list_element_value(value: Value, element: CypherReturnListElement) -> Result<Value> {
    let select = |len: usize| match element {
        CypherReturnListElement::Head => 0,
        CypherReturnListElement::Last => len.saturating_sub(1),
    };
    match value {
        Value::Null => Ok(Value::Null),
        Value::StringArray(values) => Ok(values
            .get(select(values.len()))
            .map(Value::from)
            .unwrap_or(Value::Null)),
        Value::IntArray(values) => Ok(values
            .get(select(values.len()))
            .copied()
            .map(Value::Int)
            .unwrap_or(Value::Null)),
        Value::FloatArray(values) => Ok(values
            .get(select(values.len()))
            .copied()
            .map(Value::Float)
            .unwrap_or(Value::Null)),
        Value::Json(serde_json::Value::Array(values)) => Ok(values
            .get(select(values.len()))
            .cloned()
            .map(Value::from_json)
            .unwrap_or(Value::Null)),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN head/last only supports array values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_list_tail_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    tail: &CypherReturnListTailProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*tail.target).clone(),
            variable: tail
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_list_tail_value(value)
}

pub(crate) fn restricted_list_tail_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::StringArray(values) => Ok(Value::StringArray(
            values.into_iter().skip(1).collect::<Vec<_>>(),
        )),
        Value::IntArray(values) => Ok(Value::IntArray(
            values.into_iter().skip(1).collect::<Vec<_>>(),
        )),
        Value::FloatArray(values) => Ok(Value::FloatArray(
            values.into_iter().skip(1).collect::<Vec<_>>(),
        )),
        Value::Json(serde_json::Value::Array(values)) => Ok(Value::from_json(
            serde_json::Value::Array(values.into_iter().skip(1).collect()),
        )),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN tail only supports array values",
        )),
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
    abs: &CypherReturnAbsProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*abs.target).clone(),
            variable: abs
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_abs_value(value)
}

pub(crate) fn restricted_abs_value(value: Value) -> Result<Value> {
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
    round: &CypherReturnNumericRoundProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*round.target).clone(),
            variable: round
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_numeric_round_value(value, round.round)
}

pub(crate) fn restricted_numeric_round_value(value: Value, round: CypherReturnNumericRound) -> Result<Value> {
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
async fn materialize_return_property_numeric_sign_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    sign: &CypherReturnNumericSignProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*sign.target).clone(),
            variable: sign
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_numeric_sign_value(value)
}

pub(crate) fn restricted_numeric_sign_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Int(value) => Ok(Value::Int(value.signum())),
        Value::Float(value) => {
            if !value.is_finite() {
                return Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN sign only supports finite numeric values",
                ));
            }
            let sign = if value > 0.0 {
                1.0
            } else if value < 0.0 {
                -1.0
            } else {
                0.0
            };
            Ok(Value::Float(sign))
        }
        Value::Json(serde_json::Value::Number(value)) => {
            if let Some(value) = value.as_i64() {
                restricted_numeric_sign_value(Value::Int(value))
            } else if let Some(value) = value.as_f64() {
                restricted_numeric_sign_value(Value::Float(value))
            } else {
                Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN sign only supports finite numeric values",
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
            "writable Cypher RETURN sign only supports numeric values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_numeric_cast_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    cast: &CypherReturnNumericCastProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*cast.target).clone(),
            variable: cast
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_numeric_cast_value(value, cast.cast)
}

pub(crate) fn restricted_numeric_cast_value(value: Value, cast: CypherReturnNumericCast) -> Result<Value> {
    match cast {
        CypherReturnNumericCast::Integer => restricted_to_integer_value(value),
        CypherReturnNumericCast::Float => restricted_to_float_value(value),
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_list_cast_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    cast: &CypherReturnListCastProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*cast.target).clone(),
            variable: cast
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_list_cast_value(value, cast.cast)
}

pub(crate) fn restricted_list_cast_value(value: Value, cast: CypherReturnListCast) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::StringArray(values) => restricted_string_array_cast_value(values, cast),
        Value::IntArray(values) => restricted_int_array_cast_value(values, cast),
        Value::FloatArray(values) => restricted_float_array_cast_value(values, cast),
        Value::Json(serde_json::Value::Array(values)) => {
            let values = values.into_iter().map(Value::from_json).collect::<Vec<_>>();
            restricted_json_array_cast_value(values, cast)
        }
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list casts only support array values",
        )),
    }
}

pub(crate) fn restricted_string_array_cast_value(
    values: Vec<String>,
    cast: CypherReturnListCast,
) -> Result<Value> {
    match cast {
        CypherReturnListCast::String => Ok(Value::StringArray(values)),
        CypherReturnListCast::Integer => values
            .into_iter()
            .map(|value| parse_integer_string_value(&value).and_then(expect_int_value))
            .collect::<Result<Vec<_>>>()
            .map(Value::IntArray),
        CypherReturnListCast::Float => values
            .into_iter()
            .map(|value| parse_float_string_value(&value).and_then(expect_float_value))
            .collect::<Result<Vec<_>>>()
            .map(Value::FloatArray),
        CypherReturnListCast::Boolean => values
            .into_iter()
            .map(|value| parse_boolean_string_value(&value).and_then(expect_bool_value))
            .collect::<Result<Vec<_>>>()
            .map(|values| Value::Json(serde_json::Value::Array(values))),
    }
}

pub(crate) fn restricted_int_array_cast_value(values: Vec<i64>, cast: CypherReturnListCast) -> Result<Value> {
    match cast {
        CypherReturnListCast::String => Ok(Value::StringArray(
            values.into_iter().map(|value| value.to_string()).collect(),
        )),
        CypherReturnListCast::Integer => Ok(Value::IntArray(values)),
        CypherReturnListCast::Float => Ok(Value::FloatArray(
            values.into_iter().map(|value| value as f64).collect(),
        )),
        CypherReturnListCast::Boolean => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toBooleanList only supports boolean or boolean-string array values",
        )),
    }
}

pub(crate) fn restricted_float_array_cast_value(
    values: Vec<f64>,
    cast: CypherReturnListCast,
) -> Result<Value> {
    match cast {
        CypherReturnListCast::String => Ok(Value::StringArray(
            values.into_iter().map(|value| value.to_string()).collect(),
        )),
        CypherReturnListCast::Integer => values
            .into_iter()
            .map(|value| float_to_integer_value(value).and_then(expect_int_value))
            .collect::<Result<Vec<_>>>()
            .map(Value::IntArray),
        CypherReturnListCast::Float => {
            if values.iter().any(|value| !value.is_finite()) {
                return Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN toFloatList only supports finite numeric values",
                ));
            }
            Ok(Value::FloatArray(values))
        }
        CypherReturnListCast::Boolean => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toBooleanList only supports boolean or boolean-string array values",
        )),
    }
}

pub(crate) fn restricted_json_array_cast_value(
    values: Vec<Value>,
    cast: CypherReturnListCast,
) -> Result<Value> {
    match cast {
        CypherReturnListCast::String => values
            .into_iter()
            .map(|value| restricted_to_string_value(value).and_then(expect_string_value))
            .collect::<Result<Vec<_>>>()
            .map(Value::StringArray),
        CypherReturnListCast::Integer => values
            .into_iter()
            .map(|value| restricted_to_integer_value(value).and_then(expect_int_value))
            .collect::<Result<Vec<_>>>()
            .map(Value::IntArray),
        CypherReturnListCast::Float => values
            .into_iter()
            .map(|value| restricted_to_float_value(value).and_then(expect_float_value))
            .collect::<Result<Vec<_>>>()
            .map(Value::FloatArray),
        CypherReturnListCast::Boolean => values
            .into_iter()
            .map(|value| restricted_to_boolean_value(value).and_then(expect_bool_value))
            .collect::<Result<Vec<_>>>()
            .map(|values| Value::Json(serde_json::Value::Array(values))),
    }
}

pub(crate) fn expect_string_value(value: Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(value),
        Value::Null => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toStringList does not support null array elements",
        )),
        _ => Err(GrustError::CypherExecution(
            "RETURN toStringList produced a non-string value".to_string(),
        )),
    }
}

pub(crate) fn expect_int_value(value: Value) -> Result<i64> {
    match value {
        Value::Int(value) => Ok(value),
        Value::Null => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toIntegerList does not support null array elements",
        )),
        _ => Err(GrustError::CypherExecution(
            "RETURN toIntegerList produced a non-integer value".to_string(),
        )),
    }
}

pub(crate) fn expect_float_value(value: Value) -> Result<f64> {
    match value {
        Value::Float(value) => Ok(value),
        Value::Null => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toFloatList does not support null array elements",
        )),
        _ => Err(GrustError::CypherExecution(
            "RETURN toFloatList produced a non-float value".to_string(),
        )),
    }
}

pub(crate) fn expect_bool_value(value: Value) -> Result<serde_json::Value> {
    match value {
        Value::Bool(value) => Ok(serde_json::Value::Bool(value)),
        Value::Null => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toBooleanList does not support null array elements",
        )),
        _ => Err(GrustError::CypherExecution(
            "RETURN toBooleanList produced a non-boolean value".to_string(),
        )),
    }
}

pub(crate) fn restricted_to_integer_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Int(value) => Ok(Value::Int(value)),
        Value::Float(value) => float_to_integer_value(value),
        Value::String(value) => parse_integer_string_value(&value),
        Value::Json(serde_json::Value::Null) => Ok(Value::Null),
        Value::Json(serde_json::Value::Number(value)) => {
            if let Some(value) = value.as_i64() {
                Ok(Value::Int(value))
            } else if let Some(value) = value.as_f64() {
                float_to_integer_value(value)
            } else {
                Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN toInteger only supports finite numeric values",
                ))
            }
        }
        Value::Json(serde_json::Value::String(value)) => parse_integer_string_value(&value),
        Value::Bool(_)
        | Value::DateTime(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toInteger only supports numeric or integer-string values",
        )),
    }
}

pub(crate) fn restricted_to_float_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Int(value) => Ok(Value::Float(value as f64)),
        Value::Float(value) if value.is_finite() => Ok(Value::Float(value)),
        Value::Float(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toFloat only supports finite numeric values",
        )),
        Value::String(value) => parse_float_string_value(&value),
        Value::Json(serde_json::Value::Null) => Ok(Value::Null),
        Value::Json(serde_json::Value::Number(value)) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(Value::Float)
            .ok_or_else(|| {
                cypher_unsupported_cardinality(
                    "writable Cypher RETURN toFloat only supports finite numeric values",
                )
            }),
        Value::Json(serde_json::Value::String(value)) => parse_float_string_value(&value),
        Value::Bool(_)
        | Value::DateTime(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toFloat only supports numeric or numeric-string values",
        )),
    }
}

pub(crate) fn float_to_integer_value(value: f64) -> Result<Value> {
    if !value.is_finite() || value.trunc() < i64::MIN as f64 || value.trunc() > i64::MAX as f64 {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toInteger only supports finite in-range numeric values",
        ));
    }
    Ok(Value::Int(value.trunc() as i64))
}

pub(crate) fn parse_integer_string_value(value: &str) -> Result<Value> {
    value.trim().parse::<i64>().map(Value::Int).map_err(|_| {
        cypher_unsupported_cardinality(
            "writable Cypher RETURN toInteger only supports integer-string values",
        )
    })
}

pub(crate) fn parse_float_string_value(value: &str) -> Result<Value> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(Value::Float)
        .ok_or_else(|| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN toFloat only supports finite numeric-string values",
            )
        })
}

#[allow(clippy::too_many_arguments)]
async fn materialize_return_property_to_boolean_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    to_boolean: &CypherReturnToBooleanProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*to_boolean.target).clone(),
            variable: to_boolean
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_to_boolean_value(value)
}

pub(crate) fn restricted_to_boolean_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Bool(value) => Ok(Value::Bool(value)),
        Value::String(value) => parse_boolean_string_value(&value),
        Value::Json(serde_json::Value::Null) => Ok(Value::Null),
        Value::Json(serde_json::Value::Bool(value)) => Ok(Value::Bool(value)),
        Value::Json(serde_json::Value::String(value)) => parse_boolean_string_value(&value),
        Value::Int(_)
        | Value::Float(_)
        | Value::DateTime(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toBoolean only supports boolean or boolean-string values",
        )),
    }
}

pub(crate) fn parse_boolean_string_value(value: &str) -> Result<Value> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Ok(Value::Bool(true)),
        "false" => Ok(Value::Bool(false)),
        _ => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toBoolean only supports true/false string values",
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
    to_string: &CypherReturnToStringProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*to_string.target).clone(),
            variable: to_string
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_to_string_value(value)
}

pub(crate) fn restricted_to_string_value(value: Value) -> Result<Value> {
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
async fn materialize_return_is_empty_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    is_empty: &CypherReturnIsEmptyProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*is_empty.target).clone(),
            variable: is_empty
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_is_empty_value(value)
}

pub(crate) fn restricted_is_empty_value(value: Value) -> Result<Value> {
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
async fn materialize_return_string_transform_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    transform: &CypherReturnStringTransformProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*transform.target).clone(),
            variable: transform
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_string_transform_value(value, transform.transform)
}

pub(crate) fn restricted_string_transform_value(
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
async fn materialize_return_string_trim_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    trim: &CypherReturnStringTrimProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*trim.target).clone(),
            variable: trim
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_string_trim_value(value, trim.trim)
}

pub(crate) fn restricted_string_trim_value(value: Value, trim: CypherReturnStringTrim) -> Result<Value> {
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
async fn materialize_return_string_reverse_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    reverse: &CypherReturnStringReverseProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*reverse.target).clone(),
            variable: reverse
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_string_reverse_value(value)
}

pub(crate) fn restricted_string_reverse_value(value: Value) -> Result<Value> {
    let reverse_value = |value: String| value.chars().rev().collect::<String>();
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::from(reverse_value(value))),
        Value::DateTime(value) => Ok(Value::from(reverse_value(value.as_str().to_string()))),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::from(reverse_value(value))),
        Value::StringArray(values) => Ok(Value::StringArray(values.into_iter().rev().collect())),
        Value::IntArray(values) => Ok(Value::IntArray(values.into_iter().rev().collect())),
        Value::FloatArray(values) => Ok(Value::FloatArray(values.into_iter().rev().collect())),
        Value::Json(serde_json::Value::Array(values)) => Ok(Value::from_json(
            serde_json::Value::Array(values.into_iter().rev().collect()),
        )),
        Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Json(_) => {
            Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN reverse only supports string or array values",
            ))
        }
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
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*split.target).clone(),
            variable: split
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_string_split_value(value, split)
}

pub(crate) fn restricted_string_split_value(value: Value, split: &CypherReturnStringSplit) -> Result<Value> {
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
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*substring.target).clone(),
            variable: substring
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_substring_value(value, substring)
}

pub(crate) fn restricted_substring_value(value: Value, substring: &CypherReturnSubstring) -> Result<Value> {
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
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*slice.target).clone(),
            variable: slice
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_string_slice_value(value, slice)
}

pub(crate) fn restricted_string_slice_value(value: Value, slice: &CypherReturnStringSlice) -> Result<Value> {
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
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*replace.target).clone(),
            variable: replace
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_replace_value(value, replace)
}

pub(crate) fn restricted_replace_value(value: Value, replace: &CypherReturnReplace) -> Result<Value> {
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
    let value = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: (*predicate.target).clone(),
            variable: predicate
                .variable
                .clone()
                .unwrap_or_else(|| projection.variable.clone()),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await?;
    restricted_string_predicate_value(value, predicate)
}

pub(crate) fn restricted_string_predicate_value(
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

pub(crate) fn props_value(props: &Props) -> Value {
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

pub(crate) fn return_row_key(values: &[Value], context: &str) -> Result<String> {
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
    match classify_return_target_materialization(&projection.target) {
        CypherReturnTargetMaterialization::PathFunction => {
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
            return count_materialized_return_values(values, projection.distinct);
        }
        CypherReturnTargetMaterialization::ScalarProjection
        | CypherReturnTargetMaterialization::ElementFunction => {
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
            return count_materialized_return_values(values, projection.distinct);
        }
        CypherReturnTargetMaterialization::Star
        | CypherReturnTargetMaterialization::Element
        | CypherReturnTargetMaterialization::DirectProperty => {}
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

pub(crate) fn count_materialized_return_values(values: Vec<Value>, distinct: bool) -> Result<usize> {
    if distinct {
        Ok(distinct_return_values(values)?.len())
    } else {
        Ok(values.len())
    }
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
    let mut values = match classify_return_target_materialization(&projection.target) {
        CypherReturnTargetMaterialization::Star if aggregate == CypherReturnAggregate::Collect => {
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
        CypherReturnTargetMaterialization::Star => {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN aggregates other than COUNT and COLLECT require variable.property",
            ));
        }
        CypherReturnTargetMaterialization::Element
            if aggregate == CypherReturnAggregate::Collect =>
        {
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
        CypherReturnTargetMaterialization::Element => {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN aggregates other than COUNT and COLLECT require variable.property",
            ));
        }
        CypherReturnTargetMaterialization::DirectProperty => {
            let CypherReturnTarget::Property(key) = &projection.target else {
                unreachable!("direct property classification requires property target");
            };
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
        CypherReturnTargetMaterialization::PathFunction => {
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
        CypherReturnTargetMaterialization::ScalarProjection
        | CypherReturnTargetMaterialization::ElementFunction => {
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

pub(crate) fn classify_return_target_materialization(
    target: &CypherReturnTarget,
) -> CypherReturnTargetMaterialization {
    match target {
        CypherReturnTarget::All => CypherReturnTargetMaterialization::Star,
        CypherReturnTarget::Element => CypherReturnTargetMaterialization::Element,
        CypherReturnTarget::Property(_) => CypherReturnTargetMaterialization::DirectProperty,
        CypherReturnTarget::PathLength
        | CypherReturnTarget::PathNodes
        | CypherReturnTarget::PathRelationships => CypherReturnTargetMaterialization::PathFunction,
        CypherReturnTarget::NodeLabels
        | CypherReturnTarget::RelationshipType
        | CypherReturnTarget::ElementProperties
        | CypherReturnTarget::ElementKeys
        | CypherReturnTarget::ElementId
        | CypherReturnTarget::RelationshipStartNode
        | CypherReturnTarget::RelationshipEndNode => {
            CypherReturnTargetMaterialization::ElementFunction
        }
        CypherReturnTarget::Literal(_)
        | CypherReturnTarget::MapProjection(_)
        | CypherReturnTarget::ListProjection(_)
        | CypherReturnTarget::Case(_)
        | CypherReturnTarget::Coalesce(_)
        | CypherReturnTarget::PropertyExists(_)
        | CypherReturnTarget::PropertySize(_)
        | CypherReturnTarget::PropertyListIndex(_)
        | CypherReturnTarget::PropertyListSlice(_)
        | CypherReturnTarget::PropertyListContains(_)
        | CypherReturnTarget::PropertyListPredicate(_)
        | CypherReturnTarget::PropertyListElement(_)
        | CypherReturnTarget::PropertyListTail(_)
        | CypherReturnTarget::PropertyAbs(_)
        | CypherReturnTarget::PropertyNumericRound(_)
        | CypherReturnTarget::PropertyNumericSign(_)
        | CypherReturnTarget::PropertyNumericCast(_)
        | CypherReturnTarget::PropertyListCast(_)
        | CypherReturnTarget::PropertyToBoolean(_)
        | CypherReturnTarget::PropertyToString(_)
        | CypherReturnTarget::PropertyStringTransform(_)
        | CypherReturnTarget::PropertyStringTrim(_)
        | CypherReturnTarget::PropertyIsEmpty(_)
        | CypherReturnTarget::PropertyStringReverse(_)
        | CypherReturnTarget::PropertyStringSplit(_)
        | CypherReturnTarget::PropertySubstring(_)
        | CypherReturnTarget::PropertyStringSlice(_)
        | CypherReturnTarget::PropertyReplace(_)
        | CypherReturnTarget::PropertyStringPredicate(_) => {
            CypherReturnTargetMaterialization::ScalarProjection
        }
    }
}

pub(crate) fn classify_return_scalar_projection(
    target: &CypherReturnTarget,
) -> CypherReturnScalarProjectionKind {
    match scalar_return_ast(target) {
        CypherReturnScalarAst::Star => CypherReturnScalarProjectionKind::Star,
        CypherReturnScalarAst::Element => CypherReturnScalarProjectionKind::Element,
        CypherReturnScalarAst::DirectProperty(_) => {
            CypherReturnScalarProjectionKind::DirectProperty
        }
        CypherReturnScalarAst::Literal(_) => CypherReturnScalarProjectionKind::Literal,
        CypherReturnScalarAst::Map(_) => CypherReturnScalarProjectionKind::Map,
        CypherReturnScalarAst::List(_) => CypherReturnScalarProjectionKind::List,
        CypherReturnScalarAst::Conditional(_) => CypherReturnScalarProjectionKind::Conditional,
        CypherReturnScalarAst::Coalesce(_) => CypherReturnScalarProjectionKind::Coalesce,
        CypherReturnScalarAst::PropertyExists(_)
        | CypherReturnScalarAst::PropertySize(_)
        | CypherReturnScalarAst::PropertyIsEmpty(_) => {
            CypherReturnScalarProjectionKind::Introspection
        }
        CypherReturnScalarAst::PropertyListIndex(_)
        | CypherReturnScalarAst::PropertyListSlice(_)
        | CypherReturnScalarAst::PropertyListContains(_)
        | CypherReturnScalarAst::PropertyListElement(_)
        | CypherReturnScalarAst::PropertyListTail(_) => {
            CypherReturnScalarProjectionKind::ListAccess
        }
        CypherReturnScalarAst::PropertyListPredicate(_) => {
            CypherReturnScalarProjectionKind::ListPredicate
        }
        CypherReturnScalarAst::PropertyAbs(_)
        | CypherReturnScalarAst::PropertyNumericRound(_)
        | CypherReturnScalarAst::PropertyNumericSign(_) => {
            CypherReturnScalarProjectionKind::Numeric
        }
        CypherReturnScalarAst::PropertyNumericCast(_)
        | CypherReturnScalarAst::PropertyListCast(_)
        | CypherReturnScalarAst::PropertyToBoolean(_)
        | CypherReturnScalarAst::PropertyToString(_) => {
            CypherReturnScalarProjectionKind::Conversion
        }
        CypherReturnScalarAst::PropertyStringTransform(_)
        | CypherReturnScalarAst::PropertyStringTrim(_)
        | CypherReturnScalarAst::PropertyStringReverse(_)
        | CypherReturnScalarAst::PropertyStringSplit(_)
        | CypherReturnScalarAst::PropertySubstring(_)
        | CypherReturnScalarAst::PropertyStringSlice(_)
        | CypherReturnScalarAst::PropertyReplace(_)
        | CypherReturnScalarAst::PropertyStringPredicate(_) => {
            CypherReturnScalarProjectionKind::String
        }
        CypherReturnScalarAst::ElementFunction => CypherReturnScalarProjectionKind::ElementFunction,
        CypherReturnScalarAst::PathFunction => CypherReturnScalarProjectionKind::PathFunction,
    }
}

pub(crate) fn classify_return_scalar_ast_family(
    expression: &CypherReturnScalarAst<'_>,
) -> CypherReturnScalarAstFamily {
    match expression {
        CypherReturnScalarAst::Star
        | CypherReturnScalarAst::Element
        | CypherReturnScalarAst::DirectProperty(_) => CypherReturnScalarAstFamily::Binding,
        CypherReturnScalarAst::ElementFunction | CypherReturnScalarAst::PathFunction => {
            CypherReturnScalarAstFamily::Wrapper
        }
        CypherReturnScalarAst::Literal(_)
        | CypherReturnScalarAst::Map(_)
        | CypherReturnScalarAst::List(_) => CypherReturnScalarAstFamily::Value,
        CypherReturnScalarAst::Conditional(_) | CypherReturnScalarAst::Coalesce(_) => {
            CypherReturnScalarAstFamily::Control
        }
        CypherReturnScalarAst::PropertyExists(_) | CypherReturnScalarAst::PropertySize(_) => {
            CypherReturnScalarAstFamily::Introspection
        }
        CypherReturnScalarAst::PropertyListIndex(_)
        | CypherReturnScalarAst::PropertyListSlice(_)
        | CypherReturnScalarAst::PropertyListContains(_)
        | CypherReturnScalarAst::PropertyListPredicate(_)
        | CypherReturnScalarAst::PropertyListElement(_)
        | CypherReturnScalarAst::PropertyListTail(_) => CypherReturnScalarAstFamily::List,
        CypherReturnScalarAst::PropertyAbs(_)
        | CypherReturnScalarAst::PropertyNumericRound(_)
        | CypherReturnScalarAst::PropertyNumericSign(_) => CypherReturnScalarAstFamily::Numeric,
        CypherReturnScalarAst::PropertyNumericCast(_)
        | CypherReturnScalarAst::PropertyListCast(_)
        | CypherReturnScalarAst::PropertyToBoolean(_)
        | CypherReturnScalarAst::PropertyToString(_) => CypherReturnScalarAstFamily::Conversion,
        CypherReturnScalarAst::PropertyStringTransform(_)
        | CypherReturnScalarAst::PropertyStringTrim(_)
        | CypherReturnScalarAst::PropertyIsEmpty(_)
        | CypherReturnScalarAst::PropertyStringReverse(_)
        | CypherReturnScalarAst::PropertyStringSplit(_)
        | CypherReturnScalarAst::PropertySubstring(_)
        | CypherReturnScalarAst::PropertyStringSlice(_)
        | CypherReturnScalarAst::PropertyReplace(_)
        | CypherReturnScalarAst::PropertyStringPredicate(_) => CypherReturnScalarAstFamily::String,
    }
}

pub(crate) fn scalar_return_ast(target: &CypherReturnTarget) -> CypherReturnScalarAst<'_> {
    match target {
        CypherReturnTarget::All => CypherReturnScalarAst::Star,
        CypherReturnTarget::Element => CypherReturnScalarAst::Element,
        CypherReturnTarget::Property(key) => CypherReturnScalarAst::DirectProperty(key),
        CypherReturnTarget::Literal(value) => CypherReturnScalarAst::Literal(value),
        CypherReturnTarget::MapProjection(map) => CypherReturnScalarAst::Map(map),
        CypherReturnTarget::ListProjection(list) => CypherReturnScalarAst::List(list),
        CypherReturnTarget::Case(case) => CypherReturnScalarAst::Conditional(case),
        CypherReturnTarget::Coalesce(coalesce) => CypherReturnScalarAst::Coalesce(coalesce),
        CypherReturnTarget::PropertyExists(key) => CypherReturnScalarAst::PropertyExists(key),
        CypherReturnTarget::PropertySize(key) => CypherReturnScalarAst::PropertySize(key),
        CypherReturnTarget::PropertyListIndex(index) => {
            CypherReturnScalarAst::PropertyListIndex(index)
        }
        CypherReturnTarget::PropertyListSlice(slice) => {
            CypherReturnScalarAst::PropertyListSlice(slice)
        }
        CypherReturnTarget::PropertyListContains(contains) => {
            CypherReturnScalarAst::PropertyListContains(contains)
        }
        CypherReturnTarget::PropertyListPredicate(predicate) => {
            CypherReturnScalarAst::PropertyListPredicate(predicate)
        }
        CypherReturnTarget::PropertyListElement(element) => {
            CypherReturnScalarAst::PropertyListElement(element)
        }
        CypherReturnTarget::PropertyListTail(tail) => CypherReturnScalarAst::PropertyListTail(tail),
        CypherReturnTarget::PropertyAbs(abs) => CypherReturnScalarAst::PropertyAbs(abs),
        CypherReturnTarget::PropertyNumericRound(round) => {
            CypherReturnScalarAst::PropertyNumericRound(round)
        }
        CypherReturnTarget::PropertyNumericSign(sign) => {
            CypherReturnScalarAst::PropertyNumericSign(sign)
        }
        CypherReturnTarget::PropertyNumericCast(cast) => {
            CypherReturnScalarAst::PropertyNumericCast(cast)
        }
        CypherReturnTarget::PropertyListCast(cast) => CypherReturnScalarAst::PropertyListCast(cast),
        CypherReturnTarget::PropertyToBoolean(to_boolean) => {
            CypherReturnScalarAst::PropertyToBoolean(to_boolean)
        }
        CypherReturnTarget::PropertyToString(to_string) => {
            CypherReturnScalarAst::PropertyToString(to_string)
        }
        CypherReturnTarget::PropertyStringTransform(transform) => {
            CypherReturnScalarAst::PropertyStringTransform(transform)
        }
        CypherReturnTarget::PropertyStringTrim(trim) => {
            CypherReturnScalarAst::PropertyStringTrim(trim)
        }
        CypherReturnTarget::PropertyIsEmpty(is_empty) => {
            CypherReturnScalarAst::PropertyIsEmpty(is_empty)
        }
        CypherReturnTarget::PropertyStringReverse(reverse) => {
            CypherReturnScalarAst::PropertyStringReverse(reverse)
        }
        CypherReturnTarget::PropertyStringSplit(split) => {
            CypherReturnScalarAst::PropertyStringSplit(split)
        }
        CypherReturnTarget::PropertySubstring(substring) => {
            CypherReturnScalarAst::PropertySubstring(substring)
        }
        CypherReturnTarget::PropertyStringSlice(slice) => {
            CypherReturnScalarAst::PropertyStringSlice(slice)
        }
        CypherReturnTarget::PropertyReplace(replace) => {
            CypherReturnScalarAst::PropertyReplace(replace)
        }
        CypherReturnTarget::PropertyStringPredicate(predicate) => {
            CypherReturnScalarAst::PropertyStringPredicate(predicate)
        }
        CypherReturnTarget::NodeLabels
        | CypherReturnTarget::RelationshipType
        | CypherReturnTarget::ElementProperties
        | CypherReturnTarget::ElementKeys
        | CypherReturnTarget::ElementId
        | CypherReturnTarget::RelationshipStartNode
        | CypherReturnTarget::RelationshipEndNode => CypherReturnScalarAst::ElementFunction,
        CypherReturnTarget::PathLength
        | CypherReturnTarget::PathNodes
        | CypherReturnTarget::PathRelationships => CypherReturnScalarAst::PathFunction,
    }
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
    } else if row_path_bindings.contains_key(&projection.variable) {
        let write_rows =
            CypherWriteResultRows::new(row_node_values, row_edge_values, row_path_bindings);
        let row_count = write_rows.path_row_count(&projection.variable)?;
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

pub(crate) fn evaluate_non_count_aggregate(
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

pub(crate) fn distinct_return_values(values: Vec<Value>) -> Result<Vec<Value>> {
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

pub(crate) fn sum_return_values(values: &[Value]) -> Result<Value> {
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

pub(crate) fn avg_return_values(values: &[Value]) -> Result<Value> {
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

pub(crate) fn count_distinct_elements(
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

pub(crate) fn count_distinct_values(values: impl IntoIterator<Item = Value>) -> Result<usize> {
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

pub(crate) fn non_null_return_value(value: Value) -> Option<Value> {
    (value != Value::Null).then_some(value)
}

pub(crate) fn edge_identity_count_key(identity: &CypherBoundEdgeIdentity) -> String {
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

pub(crate) fn edge_count_key(edge: &Edge) -> String {
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

pub(crate) fn count_value(count: usize) -> Result<Value> {
    i64::try_from(count).map(Value::Int).map_err(|_| {
        GrustError::CypherExecution(format!("RETURN count {count} cannot fit in int64"))
    })
}

pub(crate) fn apply_return_distinct(rows: &mut Vec<Vec<Value>>) -> Result<()> {
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
pub(crate) fn apply_return_control(
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
pub(crate) fn compare_return_values(a: &Value, b: &Value) -> std::cmp::Ordering {
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

pub(crate) fn value_kind_rank(value: &Value) -> u8 {
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

pub async fn row_edge_endpoint_node_values_on_store<S>(
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

pub async fn row_edge_match_path_endpoint_node_values_on_store<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_match_bindings: &HashMap<String, GraphRelationshipMatch>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    edge_values: &HashMap<String, Vec<Edge>>,
) -> Result<HashMap<String, Vec<Node>>>
where
    S: GraphStore + Sync,
{
    let mut values = HashMap::new();
    for path in row_path_bindings.values() {
        if !row_edge_match_bindings.contains_key(&path.edge_variable) {
            continue;
        }
        let Some(edges) = edge_values.get(&path.edge_variable) else {
            continue;
        };
        for (variable, endpoint) in [
            (&path.from_variable, RowEdgeEndpoint::From),
            (&path.to_variable, RowEdgeEndpoint::To),
        ] {
            if node_bindings.contains_key(variable)
                || row_node_values.contains_key(variable)
                || values.contains_key(variable)
            {
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
                        "RETURN matched path endpoint variable '{variable}' resolved to missing node '{}'",
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
pub(crate) enum RowEdgeEndpoint {
    From,
    To,
}

pub async fn collect_row_node_ids_for_operation<S>(
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

pub async fn collect_row_edge_keys_for_operation<S>(
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

pub async fn collect_deleted_row_node_values_for_operation<S>(
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

pub async fn collect_deleted_row_edge_values_for_operation<S>(
    store: &S,
    operation: &GraphMutationPlanOp,
    bindings: &HashMap<String, GraphRelationshipMatch>,
    values: &mut HashMap<String, Vec<Edge>>,
) -> Result<()>
where
    S: GraphStore + Sync,
{
    let relationship = match operation {
        GraphMutationPlanOp::DeleteMatchingEdges { relationship, .. }
        | GraphMutationPlanOp::DeleteRelationshipRows { relationship, .. } => relationship,
        _ => return Ok(()),
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

pub async fn collect_deleted_path_endpoint_node_values_for_operation<S>(
    store: &S,
    operation: &GraphMutationPlanOp,
    row_edge_match_bindings: &HashMap<String, GraphRelationshipMatch>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    values: &mut HashMap<String, Vec<Node>>,
) -> Result<()>
where
    S: GraphStore + Sync,
{
    let relationship = match operation {
        GraphMutationPlanOp::DeleteMatchingEdges { relationship, .. }
        | GraphMutationPlanOp::DeleteRelationshipRows { relationship, .. } => relationship,
        _ => return Ok(()),
    };
    for path in row_path_bindings.values() {
        let Some(binding) = row_edge_match_bindings.get(&path.edge_variable) else {
            continue;
        };
        if binding != relationship {
            continue;
        }
        let edges = matching_edges_on_store(store, binding).await?;
        for (variable, endpoint) in [
            (&path.from_variable, RowEdgeEndpoint::From),
            (&path.to_variable, RowEdgeEndpoint::To),
        ] {
            if values.contains_key(variable) {
                continue;
            }
            let mut nodes = Vec::with_capacity(edges.len());
            for edge in &edges {
                let id = match endpoint {
                    RowEdgeEndpoint::From => &edge.from,
                    RowEdgeEndpoint::To => &edge.to,
                };
                let node = store.get_node(id).await?.ok_or_else(|| {
                    GrustError::CypherExecution(format!(
                        "RETURN deleted path endpoint variable '{variable}' resolved to missing node '{}'",
                        id.as_str()
                    ))
                })?;
                nodes.push(node);
            }
            values.insert(variable.clone(), nodes);
        }
    }
    Ok(())
}

pub(crate) fn operation_node_match(operation: &GraphMutationPlanOp) -> Option<GraphNodeMatch> {
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

pub(crate) fn operation_relationship_match(operation: &GraphMutationPlanOp) -> Option<GraphRelationshipMatch> {
    match operation {
        GraphMutationPlanOp::PatchMatchingEdges { relationship, .. }
        | GraphMutationPlanOp::UpdateMatchingEdgeProperty { relationship, .. }
        | GraphMutationPlanOp::RemoveMatchingEdgeProps { relationship, .. } => {
            Some(relationship.clone())
        }
        _ => None,
    }
}

pub async fn row_node_return_values_on_store<S>(
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

pub async fn row_edge_match_return_values_on_store<S>(
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

pub(crate) fn merge_cypher_reports(report: &mut CypherMutationReport, next: CypherMutationReport) {
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

pub fn row_edge_id_policy_generates(
    kind: GraphMutationPlanKind,
    policy: GraphRowEdgeIdPolicy,
) -> bool {
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

pub enum CypherResolvedUpsertClassification {
    Node { existed: bool },
    Edge { existed: bool },
}

impl CypherResolvedUpsertClassification {
    pub fn record(self, report: &mut CypherMutationReport) {
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

pub(crate) fn props_match(actual: &Props, expected: &Props) -> bool {
    expected
        .iter()
        .all(|(key, value)| actual.get(key) == Some(value))
}

pub(crate) fn node_match(node: &Node, expected: &GraphNodeMatch) -> bool {
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

pub(crate) fn project_node_value(node: &Node, key: &str) -> Value {
    if key == "label" {
        Value::from(node.label.as_str())
    } else {
        node.props.get(key).cloned().unwrap_or(Value::Null)
    }
}

pub(crate) fn project_edge_value(edge: &Edge, key: &str) -> Value {
    edge.props.get(key).cloned().unwrap_or(Value::Null)
}

pub(crate) fn graph_node_value(node: &Node) -> Result<Value> {
    serde_json::to_value(node).map(Value::from).map_err(|err| {
        GrustError::CypherExecution(format!("RETURN node serialization failed: {err}"))
    })
}

pub(crate) fn graph_edge_value(edge: &Edge) -> Result<Value> {
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
        collect_deleted_path_endpoint_node_values_for_operation(
            store,
            operation,
            &planned.row_edge_match_bindings,
            &planned.row_path_bindings,
            &mut row_node_pre_delete_values,
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
    for (variable, values) in row_edge_endpoint_node_values_on_store(
        store,
        &planned.node_bindings,
        &planned.row_edge_bindings,
        &row_edge_values,
    )
    .await
    .map_err(cypher_execution_error)?
    {
        row_node_values.entry(variable).or_insert(values);
    }
    for (variable, values) in row_edge_match_path_endpoint_node_values_on_store(
        store,
        &planned.node_bindings,
        &row_node_values,
        &planned.row_edge_match_bindings,
        &planned.row_path_bindings,
        &row_edge_values,
    )
    .await
    .map_err(cypher_execution_error)?
    {
        row_node_values.entry(variable).or_insert(values);
    }
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

pub fn check_strict_create_plan_conflicts(plan: &GraphMutationPlan) -> Result<()> {
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

pub(crate) struct NumericExpression {
    source_target: String,
    source_key: String,
    op: GraphNumericOp,
    operand: Value,
}

pub(crate) fn parse_numeric_expression(
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

pub(crate) fn find_numeric_operator_candidates(expression: &str) -> Vec<(usize, GraphNumericOp)> {
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

pub(crate) fn parse_property_ref(value: &str, context: &str) -> Result<(String, String)> {
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

pub(crate) fn parse_cypher_props_map_literal(value: &str, parameters: &CypherParameters) -> Result<Props> {
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

pub(crate) fn split_cypher_body_props<'a>(
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

pub(crate) fn parse_cypher_props(body: &str, parameters: &CypherParameters) -> Result<Props> {
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

pub(crate) fn parse_cypher_prop_key(key: &str) -> Result<String> {
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

pub(crate) fn parse_cypher_literal(value: &str, parameters: &CypherParameters) -> Result<Value> {
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

pub(crate) fn parse_cypher_in_values(value: &str, parameters: &CypherParameters) -> Result<Value> {
    let value = value.trim();
    if let Some(parameter) = value.strip_prefix('$') {
        let parsed = parse_cypher_literal(value, parameters)?;
        validate_cypher_in_values(&parsed)?;
        if !is_cypher_identifier(parameter) {
            return Err(cypher_syntax(format!(
                "unsupported Cypher parameter reference: {value}"
            )));
        }
        return Ok(parsed);
    }
    if !(value.starts_with('[') && value.ends_with(']')) {
        return Err(cypher_syntax(
            "MATCH WHERE IN predicates require a list literal or list parameter",
        ));
    }
    let inner = &value[1..value.len() - 1];
    let mut values = Vec::new();
    if !inner.trim().is_empty() {
        for item in split_top_level_commas(inner)? {
            let item = parse_cypher_literal(item, parameters)?;
            validate_cypher_in_item(&item)?;
            values.push(item.to_json());
        }
    }
    Ok(Value::Json(serde_json::Value::Array(values)))
}

pub(crate) fn validate_cypher_in_values(value: &Value) -> Result<()> {
    match value {
        Value::StringArray(_) | Value::IntArray(_) | Value::FloatArray(_) => Ok(()),
        Value::Json(serde_json::Value::Array(values)) => {
            for value in values {
                match value {
                    serde_json::Value::Bool(_)
                    | serde_json::Value::Number(_)
                    | serde_json::Value::String(_) => {}
                    serde_json::Value::Null
                    | serde_json::Value::Array(_)
                    | serde_json::Value::Object(_) => {
                        return Err(cypher_syntax(
                            "MATCH WHERE IN predicates only support scalar string, integer, float, or boolean list items",
                        ));
                    }
                }
            }
            Ok(())
        }
        _ => Err(cypher_syntax(
            "MATCH WHERE IN predicates require a list literal or list parameter",
        )),
    }
}

pub(crate) fn validate_cypher_in_item(value: &Value) -> Result<()> {
    match value {
        Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::String(_) => Ok(()),
        Value::Null
        | Value::DateTime(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_) => Err(cypher_syntax(
            "MATCH WHERE IN predicates only support scalar string, integer, float, or boolean list items",
        )),
    }
}

pub(crate) fn parse_cypher_string(value: &str) -> Result<String> {
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

pub(crate) fn optional_string_prop(props: &Props, key: &str) -> Option<String> {
    props.get(key).and_then(Value::as_str).map(str::to_string)
}

pub(crate) fn has_relationship_predicates_beyond_id(props: &Props) -> bool {
    props.keys().any(|key| key.as_str() != "id")
}

pub(crate) fn split_top_level_commas(value: &str) -> Result<Vec<&str>> {
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

pub(crate) fn find_top_level_keyword(value: &str, keyword: &str) -> Result<Option<usize>> {
    find_top_level_keyword_sequence(value, keyword)
}

pub(crate) fn find_top_level_keyword_sequence(value: &str, keyword: &str) -> Result<Option<usize>> {
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
            _ if paren_depth == 0
                && bracket_depth == 0
                && brace_depth == 0
                && value[index..]
                    .get(..keyword.len())
                    .is_some_and(|candidate| candidate.eq_ignore_ascii_case(keyword))
                && keyword_boundary(value[..index].chars().next_back())
                && keyword_boundary(value[index + keyword.len()..].chars().next()) =>
            {
                return Ok(Some(index));
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
    Ok(None)
}

pub(crate) fn strip_enclosing_parentheses(value: &str) -> Result<&str> {
    let mut value = value.trim();
    loop {
        let Some(after_open) = value.strip_prefix('(') else {
            return Ok(value);
        };
        if !value.ends_with(')') {
            return Ok(value);
        }
        let mut quote = None;
        let mut escaped = false;
        let mut paren_depth = 0usize;
        let mut closes_at_end = false;
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
                    if paren_depth == 0 {
                        closes_at_end = index + ch.len_utf8() == value.len();
                        if !closes_at_end {
                            return Ok(value);
                        }
                    }
                }
                _ => {}
            }
        }
        if quote.is_some() {
            return Err(GrustError::Unsupported(
                "unterminated Cypher string literal".to_string(),
            ));
        }
        if paren_depth != 0 {
            return Err(cypher_syntax("unclosed grouping in Cypher expression"));
        }
        if !closes_at_end {
            return Ok(value);
        }
        value = after_open[..after_open.len() - 1].trim();
    }
}

pub(crate) fn split_top_level_patterns(value: &str) -> Result<Vec<&str>> {
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

pub(crate) fn find_matching(value: &str, _open: char, close: char) -> Result<usize> {
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
pub(crate) fn scan_unquoted(value: &str, mut at_unquoted: impl FnMut(usize, &str) -> bool) -> Option<usize> {
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

pub(crate) fn find_unquoted(value: &str, target: char) -> Option<usize> {
    scan_unquoted(value, |_, rest| rest.starts_with(target))
}

pub(crate) fn find_unquoted_sequence(value: &str, target: &str) -> Option<usize> {
    scan_unquoted(value, |_, rest| rest.starts_with(target))
}

pub(crate) fn find_unquoted_keyword(value: &str, keyword: &str) -> Option<usize> {
    scan_unquoted(value, |index, rest| {
        rest.get(..keyword.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(keyword))
            && keyword_boundary(value[..index].chars().next_back())
            && keyword_boundary(rest[keyword.len()..].chars().next())
    })
}

pub(crate) fn keyword_boundary(ch: Option<char>) -> bool {
    ch.is_none_or(char::is_whitespace)
}

pub(crate) fn strip_leading_keyword<'a>(value: &'a str, keyword: &str) -> Option<&'a str> {
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

pub(crate) fn is_quoted(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2
        && ((bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"'))
}

pub fn validate_json_key(value: &str) -> Result<()> {
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

pub fn cypher_in_predicate_values(value: &Value) -> Result<Vec<Value>> {
    match value {
        Value::StringArray(values) => Ok(values.iter().map(Value::from).collect()),
        Value::IntArray(values) => Ok(values.iter().copied().map(Value::Int).collect()),
        Value::FloatArray(values) => Ok(values.iter().copied().map(Value::Float).collect()),
        Value::Json(serde_json::Value::Array(values)) => values
            .iter()
            .map(|value: &serde_json::Value| match value {
                serde_json::Value::Bool(value) => Ok(Value::Bool(*value)),
                serde_json::Value::Number(value) => value
                    .as_i64()
                    .map(Value::Int)
                    .or_else(|| value.as_f64().map(Value::Float))
                    .ok_or_else(|| cypher_syntax("unsupported numeric value in MATCH WHERE IN")),
                serde_json::Value::String(value) => Ok(Value::from(value)),
                serde_json::Value::Null
                | serde_json::Value::Array(_)
                | serde_json::Value::Object(_) => Err(cypher_syntax(
                    "MATCH WHERE IN predicates only support scalar string, integer, float, or boolean list items",
                )),
            })
            .collect(),
        _ => Err(cypher_syntax(
            "MATCH WHERE IN predicates require a list literal or list parameter",
        )),
    }
}

pub fn strict_create_edge_conflicts(edge: &Edge, existing: &[Edge]) -> bool {
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

#[cfg(test)]
mod tests;
