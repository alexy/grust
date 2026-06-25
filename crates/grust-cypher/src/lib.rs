use std::collections::{BTreeMap, BTreeSet, HashMap};

use grust_core::prelude::*;
use serde::{Deserialize, Serialize};

pub mod ast;
mod ddl;
pub mod gql;
pub mod lexer;
pub mod parser;
pub use ddl::*;
mod planner;
pub use planner::*;
mod parse;
pub use parse::*;
mod where_clause;
use where_clause::*;
mod returning;
pub use returning::*;
mod eval_rows;
pub use eval_rows::*;
mod restricted_values;
use restricted_values::*;
mod projection;
pub use projection::*;
mod primitives;
pub use primitives::*;
pub mod semantics;
pub use gql::{
    GqlConformanceProfile, GqlError, GqlErrorKind, GqlExpectation, GqlFeature,
    GqlFeatureDescriptor, GqlFeatureFamily, GqlFeatureStatus, GqlManifest, GqlManifestCase,
    GqlRequirement, GqlSupportCounts, feature_manifest, gql_cardinality, gql_execution, gql_name,
    gql_syntax, gql_type, load_manifest, load_manifest_cases, support_counts, support_summary,
    unsupported_gql_feature,
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

#[cfg(test)]
mod tests;
