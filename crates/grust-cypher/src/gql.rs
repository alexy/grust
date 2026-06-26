//! GQL conformance spine (Unit 1 of `docs/GQL_GOAL.md`).
//!
//! This module is the standard-shaped control surface for Grust's path from the
//! current strict writable Cypher subset toward ISO/IEC 39075:2024 GQL. It owns:
//!
//! - [`GqlConformanceProfile`]: the named profiles (strict write, portable GQL,
//!   full ISO/IEC 39075).
//! - [`GqlFeature`] + [`GqlFeatureFamily`] + [`GqlFeatureStatus`]: a feature
//!   taxonomy with stable string identifiers, mapping each cataloged feature to
//!   a family, its current implementation status, the lowest profile that
//!   includes it, and a one-line summary.
//! - [`feature_manifest`] and [`support_summary`]: a generated, machine- and
//!   human-readable report of what Grust supports, rejects, plans, or defers.
//! - [`GqlError`] + [`GqlErrorKind`] and the `gql_*` constructors: feature-tagged
//!   structured errors that name the feature family and conformance level, while
//!   still flowing through the existing `grust_core::Result` plumbing.
//! - [`GqlManifestCase`] and [`load_manifest_cases`]: test-case metadata for the
//!   conformance corpus under `crates/grust-cypher/tests/gql/`.
//!
//! Scope note: Unit 1 establishes the spine and the manifest. Re-routing the
//! existing call sites (which today raise the legacy `GrustError::Cypher*`
//! variants via the `cypher_syntax`/`cypher_unresolved_identity`/
//! `cypher_unsupported_cardinality` helpers) onto feature-tagged `gql_*` errors
//! happens in Unit 4, when the typed-AST + semantic-analysis path lands. The
//! legacy helpers and their error variants are preserved unchanged here so the
//! existing 327 tests keep passing.

use std::fmt;

use grust_core::{GrustError, Result};
use serde::{Deserialize, Serialize};

/// Conformance profiles along the path from strict writable Cypher to full GQL.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GqlConformanceProfile {
    /// The current, deliberately strict writable Cypher surface.
    StrictWrite,
    /// The portable Grust GQL profile: backend-neutral reads, expressions,
    /// composition, and pattern matching executed by the Memory reference.
    PortableGql,
    /// The target profile: the selected mandatory ISO/IEC 39075:2024 features.
    Full39075,
}

impl GqlConformanceProfile {
    /// Stable identifier used in reports and manifests.
    pub const fn id(self) -> &'static str {
        match self {
            GqlConformanceProfile::StrictWrite => "strict-write",
            GqlConformanceProfile::PortableGql => "portable-gql",
            GqlConformanceProfile::Full39075 => "full-39075",
        }
    }

    /// Profiles ordered from narrowest to widest.
    pub const fn rank(self) -> u8 {
        match self {
            GqlConformanceProfile::StrictWrite => 0,
            GqlConformanceProfile::PortableGql => 1,
            GqlConformanceProfile::Full39075 => 2,
        }
    }

    /// True when `self` includes everything in `other` (wider or equal).
    pub const fn includes(self, other: GqlConformanceProfile) -> bool {
        self.rank() >= other.rank()
    }
}

impl fmt::Display for GqlConformanceProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// Feature families, aligned with `docs/GrustCypherBackends.md`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GqlFeatureFamily {
    ParserAndSemantics,
    ResolvedWrites,
    BroadMatchedWrites,
    RowProducingRelationshipWrites,
    ReturningAndAggregates,
    PredicatesAndExpressions,
    ReadOnlyMatching,
    PathMatching,
    QueryComposition,
    TypeSystem,
    ConstraintsAndIndexes,
    Transactions,
    CatalogAndSession,
    ProceduresAndFunctions,
    NativePassthrough,
}

impl GqlFeatureFamily {
    pub const fn id(self) -> &'static str {
        match self {
            GqlFeatureFamily::ParserAndSemantics => "parser-and-semantics",
            GqlFeatureFamily::ResolvedWrites => "resolved-writes",
            GqlFeatureFamily::BroadMatchedWrites => "broad-matched-writes",
            GqlFeatureFamily::RowProducingRelationshipWrites => "row-producing-relationship-writes",
            GqlFeatureFamily::ReturningAndAggregates => "returning-and-aggregates",
            GqlFeatureFamily::PredicatesAndExpressions => "predicates-and-expressions",
            GqlFeatureFamily::ReadOnlyMatching => "read-only-matching",
            GqlFeatureFamily::PathMatching => "path-matching",
            GqlFeatureFamily::QueryComposition => "query-composition",
            GqlFeatureFamily::TypeSystem => "type-system",
            GqlFeatureFamily::ConstraintsAndIndexes => "constraints-and-indexes",
            GqlFeatureFamily::Transactions => "transactions",
            GqlFeatureFamily::CatalogAndSession => "catalog-and-session",
            GqlFeatureFamily::ProceduresAndFunctions => "procedures-and-functions",
            GqlFeatureFamily::NativePassthrough => "native-passthrough",
        }
    }
}

impl fmt::Display for GqlFeatureFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// Current implementation status of a feature in this working tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GqlFeatureStatus {
    /// Implemented and covered by tests today.
    Supported,
    /// Deliberately rejected in the current surface with a structured error.
    Rejected,
    /// Named in the completion plan as near-term work, not yet implemented.
    Planned,
    /// Targeted only at the full ISO/IEC 39075 profile, deferred.
    Future,
}

impl GqlFeatureStatus {
    pub const fn id(self) -> &'static str {
        match self {
            GqlFeatureStatus::Supported => "supported",
            GqlFeatureStatus::Rejected => "rejected",
            GqlFeatureStatus::Planned => "planned",
            GqlFeatureStatus::Future => "future",
        }
    }
}

impl fmt::Display for GqlFeatureStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// Static description of a single cataloged feature.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GqlFeatureDescriptor {
    pub feature: GqlFeature,
    pub id: &'static str,
    pub family: GqlFeatureFamily,
    pub status: GqlFeatureStatus,
    /// Lowest profile that includes this feature.
    pub min_profile: GqlConformanceProfile,
    pub summary: &'static str,
}

/// Feature taxonomy for Grust's GQL/Cypher surface.
///
/// Variants reflect the *current* working tree (see `docs/CypherWrite.md` and
/// `RESTART.md`): `Supported` and `Rejected` variants describe today's strict
/// writable surface; `Planned`/`Future` variants are placeholders that the
/// later Units will move to `Supported` as they land.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum GqlFeature {
    // --- parser & semantics (strict write, supported) ---
    StatementClassification,
    OrderedMultiStatementBatch,
    LocalNodeVariableReuse,

    // --- resolved writes (strict write, supported) ---
    CreateNodeExplicitId,
    MergeNodeExplicitId,
    CreateEdgeResolvedEndpoints,
    MergeEdgeResolvedEndpoints,
    DeleteResolvedNode,
    DeleteResolvedEdge,
    GeneratedNodeIdOptIn,
    GeneratedRelationshipIdOptIn,

    // --- broad / matched writes (strict write, supported) ---
    MatchDeleteNodeResolved,
    MatchDeleteEdgeResolvedEndpoints,
    MatchDeleteRelationshipRowEndpoints,
    MatchCreateEdgeBoundEndpoints,
    MatchMergeEdgeBoundEndpoints,
    BroadNodeMatchDelete,
    BroadRelationshipMatchDelete,
    SetNodePropertyMap,
    SetEdgePropertyMap,
    SetNodeProperty,
    SetEdgeProperty,
    RemoveNodeProperty,
    RemoveEdgeProperty,

    // --- row-producing relationship writes (strict write, supported) ---
    MatchCreateEdgeRowProducing,
    MatchMergeEdgeRowProducing,
    RelationshipVariableProjection,

    // --- returning & aggregates (strict write, supported / planned) ---
    RestrictedReturnProjection,
    RestrictedReturnAggregate,
    RestrictedPathVariableReturn,
    ReturnStar,

    // --- predicates & expressions ---
    EqualityPredicate,
    MembershipInPredicate,
    NullPredicate,
    StringPredicate,
    OrderedComparisonPredicate,
    NestedNegatedOrPredicateGroups,
    GeneralExpressionTree,
    ThreeValuedLogic,
    ScalarFunctionRegistry,
    AggregateFunctionRegistry,

    // --- constraints & indexes ---
    CreateConstraint,
    DropConstraint,
    ConstraintRegistry,
    IndexDefinition,

    // --- read-only matching ---
    ReadOnlyMatchReturn,
    LabelTypePredicateMatch,
    OptionalMatch,

    // --- path matching ---
    PathVariableBinding,
    BoundedPathPattern,
    QuantifiedPathPattern,
    ShortestPath,

    // --- query composition ---
    WithClause,
    UnionClause,
    Subquery,
    DistinctOrderingLimit,

    // --- type system ---
    TemporalValues,
    DurationValues,
    DecimalValues,
    PathValues,
    GraphValues,

    // --- transactions ---
    TransactionControl,
    SessionControl,

    // --- catalog & session ---
    GraphTypeDefinition,
    CatalogMetadata,
    NamedGraphSelection,

    // --- procedures & functions ---
    ProcedureCall,
    TableValuedFunction,

    // --- native passthrough ---
    NativeCypherPassthrough,

    // --- deliberately rejected forms in the current surface ---
    RejectCreateNodeWithoutExplicitIdentity,
    RejectMergeWithoutExplicitIdentity,
    RejectUnresolvedEdgeEndpointWrite,
    RejectNonLiteralAssignmentValue,
    RejectTrailingNodeCreationAfterRowProducingEdge,
}

impl GqlFeature {
    /// Every cataloged feature, in declaration order.
    pub const ALL: &'static [GqlFeature] = &[
        GqlFeature::StatementClassification,
        GqlFeature::OrderedMultiStatementBatch,
        GqlFeature::LocalNodeVariableReuse,
        GqlFeature::CreateNodeExplicitId,
        GqlFeature::MergeNodeExplicitId,
        GqlFeature::CreateEdgeResolvedEndpoints,
        GqlFeature::MergeEdgeResolvedEndpoints,
        GqlFeature::DeleteResolvedNode,
        GqlFeature::DeleteResolvedEdge,
        GqlFeature::GeneratedNodeIdOptIn,
        GqlFeature::GeneratedRelationshipIdOptIn,
        GqlFeature::MatchDeleteNodeResolved,
        GqlFeature::MatchDeleteEdgeResolvedEndpoints,
        GqlFeature::MatchDeleteRelationshipRowEndpoints,
        GqlFeature::MatchCreateEdgeBoundEndpoints,
        GqlFeature::MatchMergeEdgeBoundEndpoints,
        GqlFeature::BroadNodeMatchDelete,
        GqlFeature::BroadRelationshipMatchDelete,
        GqlFeature::SetNodePropertyMap,
        GqlFeature::SetEdgePropertyMap,
        GqlFeature::SetNodeProperty,
        GqlFeature::SetEdgeProperty,
        GqlFeature::RemoveNodeProperty,
        GqlFeature::RemoveEdgeProperty,
        GqlFeature::MatchCreateEdgeRowProducing,
        GqlFeature::MatchMergeEdgeRowProducing,
        GqlFeature::RelationshipVariableProjection,
        GqlFeature::RestrictedReturnProjection,
        GqlFeature::RestrictedReturnAggregate,
        GqlFeature::RestrictedPathVariableReturn,
        GqlFeature::ReturnStar,
        GqlFeature::EqualityPredicate,
        GqlFeature::MembershipInPredicate,
        GqlFeature::NullPredicate,
        GqlFeature::StringPredicate,
        GqlFeature::OrderedComparisonPredicate,
        GqlFeature::NestedNegatedOrPredicateGroups,
        GqlFeature::GeneralExpressionTree,
        GqlFeature::ThreeValuedLogic,
        GqlFeature::ScalarFunctionRegistry,
        GqlFeature::AggregateFunctionRegistry,
        GqlFeature::CreateConstraint,
        GqlFeature::DropConstraint,
        GqlFeature::ConstraintRegistry,
        GqlFeature::IndexDefinition,
        GqlFeature::ReadOnlyMatchReturn,
        GqlFeature::LabelTypePredicateMatch,
        GqlFeature::OptionalMatch,
        GqlFeature::PathVariableBinding,
        GqlFeature::BoundedPathPattern,
        GqlFeature::QuantifiedPathPattern,
        GqlFeature::ShortestPath,
        GqlFeature::WithClause,
        GqlFeature::UnionClause,
        GqlFeature::Subquery,
        GqlFeature::DistinctOrderingLimit,
        GqlFeature::TemporalValues,
        GqlFeature::DurationValues,
        GqlFeature::DecimalValues,
        GqlFeature::PathValues,
        GqlFeature::GraphValues,
        GqlFeature::TransactionControl,
        GqlFeature::SessionControl,
        GqlFeature::GraphTypeDefinition,
        GqlFeature::CatalogMetadata,
        GqlFeature::NamedGraphSelection,
        GqlFeature::ProcedureCall,
        GqlFeature::TableValuedFunction,
        GqlFeature::NativeCypherPassthrough,
        GqlFeature::RejectCreateNodeWithoutExplicitIdentity,
        GqlFeature::RejectMergeWithoutExplicitIdentity,
        GqlFeature::RejectUnresolvedEdgeEndpointWrite,
        GqlFeature::RejectNonLiteralAssignmentValue,
        GqlFeature::RejectTrailingNodeCreationAfterRowProducingEdge,
    ];

    /// Full static descriptor for this feature.
    pub const fn descriptor(self) -> GqlFeatureDescriptor {
        use GqlConformanceProfile::*;
        use GqlFeatureFamily::*;
        use GqlFeatureStatus::*;

        macro_rules! d {
            ($id:literal, $fam:expr, $status:expr, $prof:expr, $sum:literal) => {
                GqlFeatureDescriptor {
                    feature: self,
                    id: $id,
                    family: $fam,
                    status: $status,
                    min_profile: $prof,
                    summary: $sum,
                }
            };
        }

        match self {
            GqlFeature::StatementClassification => d!(
                "statement-classification",
                ParserAndSemantics,
                Supported,
                StrictWrite,
                "Classify a single statement as MATCH/CREATE/MERGE/DELETE"
            ),
            GqlFeature::OrderedMultiStatementBatch => d!(
                "ordered-multi-statement-batch",
                ParserAndSemantics,
                Supported,
                StrictWrite,
                "Ordered batch lowered to one plan; report aggregates across the batch"
            ),
            GqlFeature::LocalNodeVariableReuse => d!(
                "local-node-variable-reuse",
                ParserAndSemantics,
                Supported,
                StrictWrite,
                "Local node variables introduced by explicit-id patterns, reused later in the batch"
            ),
            GqlFeature::CreateNodeExplicitId => d!(
                "create-node-explicit-id",
                ResolvedWrites,
                Supported,
                StrictWrite,
                "CREATE (:Label {id: ...}) when the node id is explicit"
            ),
            GqlFeature::MergeNodeExplicitId => d!(
                "merge-node-explicit-id",
                ResolvedWrites,
                Supported,
                StrictWrite,
                "MERGE (:Label {id: ...}) idempotent upsert by explicit identity"
            ),
            GqlFeature::CreateEdgeResolvedEndpoints => d!(
                "create-edge-resolved-endpoints",
                ResolvedWrites,
                Supported,
                StrictWrite,
                "CREATE of an edge when both endpoint node ids resolve before execution"
            ),
            GqlFeature::MergeEdgeResolvedEndpoints => d!(
                "merge-edge-resolved-endpoints",
                ResolvedWrites,
                Supported,
                StrictWrite,
                "MERGE of an edge when both endpoint node ids resolve before execution"
            ),
            GqlFeature::DeleteResolvedNode => d!(
                "delete-resolved-node",
                ResolvedWrites,
                Supported,
                StrictWrite,
                "DELETE a resolved node, removing incident edges per the mutation contract"
            ),
            GqlFeature::DeleteResolvedEdge => d!(
                "delete-resolved-edge",
                ResolvedWrites,
                Supported,
                StrictWrite,
                "DELETE a resolved edge by endpoints and type"
            ),
            GqlFeature::GeneratedNodeIdOptIn => d!(
                "generated-node-id-opt-in",
                ResolvedWrites,
                Supported,
                StrictWrite,
                "Opt-in generated node ids for CREATE via CypherNodeIdPolicy::GenerateForCreate"
            ),
            GqlFeature::GeneratedRelationshipIdOptIn => d!(
                "generated-relationship-id-opt-in",
                ResolvedWrites,
                Supported,
                StrictWrite,
                "Opt-in generated relationship ids for row-producing CREATE/MERGE"
            ),
            GqlFeature::MatchDeleteNodeResolved => d!(
                "match-delete-node-resolved",
                BroadMatchedWrites,
                Supported,
                StrictWrite,
                "MATCH (n {id: ...}) DELETE n lowers to a resolved node delete"
            ),
            GqlFeature::MatchDeleteEdgeResolvedEndpoints => d!(
                "match-delete-edge-resolved-endpoints",
                BroadMatchedWrites,
                Supported,
                StrictWrite,
                "MATCH (:Src {id})-[e]->(:Dst {id}) DELETE e lowers to an edge delete"
            ),
            GqlFeature::MatchDeleteRelationshipRowEndpoints => d!(
                "match-delete-relationship-row-endpoints",
                BroadMatchedWrites,
                Supported,
                StrictWrite,
                "DELETE e, a captures matched rows once, then deletes relationship rows and endpoint nodes"
            ),
            GqlFeature::MatchCreateEdgeBoundEndpoints => d!(
                "match-create-edge-bound-endpoints",
                BroadMatchedWrites,
                Supported,
                StrictWrite,
                "MATCH (a {id}), (b {id}) CREATE (a)-[:T]->(b) edge upsert with bound endpoints"
            ),
            GqlFeature::MatchMergeEdgeBoundEndpoints => d!(
                "match-merge-edge-bound-endpoints",
                BroadMatchedWrites,
                Supported,
                StrictWrite,
                "MATCH (a {id}), (b {id}) MERGE (a)-[:T]->(b) edge upsert with bound endpoints"
            ),
            GqlFeature::BroadNodeMatchDelete => d!(
                "broad-node-match-delete",
                BroadMatchedWrites,
                Supported,
                StrictWrite,
                "MATCH (n:Label {..}) DELETE n cardinality-aware matched node delete"
            ),
            GqlFeature::BroadRelationshipMatchDelete => d!(
                "broad-relationship-match-delete",
                BroadMatchedWrites,
                Supported,
                StrictWrite,
                "MATCH ... DELETE e over broad relationship matches with property predicates"
            ),
            GqlFeature::SetNodePropertyMap => d!(
                "set-node-property-map",
                BroadMatchedWrites,
                Supported,
                StrictWrite,
                "SET n += { .. } resolved or cardinality-aware matching-node patch"
            ),
            GqlFeature::SetEdgePropertyMap => d!(
                "set-edge-property-map",
                BroadMatchedWrites,
                Supported,
                StrictWrite,
                "SET e += { .. } resolved or cardinality-aware matching-edge patch"
            ),
            GqlFeature::SetNodeProperty => d!(
                "set-node-property",
                BroadMatchedWrites,
                Supported,
                StrictWrite,
                "SET n.key = value one-key patch (literal values only)"
            ),
            GqlFeature::SetEdgeProperty => d!(
                "set-edge-property",
                BroadMatchedWrites,
                Supported,
                StrictWrite,
                "SET e.key = value one-key patch (literal values only)"
            ),
            GqlFeature::RemoveNodeProperty => d!(
                "remove-node-property",
                BroadMatchedWrites,
                Supported,
                StrictWrite,
                "REMOVE n.key explicit property removal"
            ),
            GqlFeature::RemoveEdgeProperty => d!(
                "remove-edge-property",
                BroadMatchedWrites,
                Supported,
                StrictWrite,
                "REMOVE e.key explicit property removal"
            ),
            GqlFeature::MatchCreateEdgeRowProducing => d!(
                "match-create-edge-row-producing",
                RowProducingRelationshipWrites,
                Supported,
                StrictWrite,
                "CREATE one edge per matched endpoint-node pair; materializes rows at execution"
            ),
            GqlFeature::MatchMergeEdgeRowProducing => d!(
                "match-merge-edge-row-producing",
                RowProducingRelationshipWrites,
                Supported,
                StrictWrite,
                "MERGE one idempotent edge per matched endpoint-node pair"
            ),
            GqlFeature::RelationshipVariableProjection => d!(
                "relationship-variable-projection",
                RowProducingRelationshipWrites,
                Supported,
                StrictWrite,
                "Relationship variables projected as one result row per produced edge"
            ),
            GqlFeature::RestrictedReturnProjection => d!(
                "restricted-return-projection",
                ReturningAndAggregates,
                Supported,
                StrictWrite,
                "Restricted scalar/element RETURN projection over write results"
            ),
            GqlFeature::RestrictedReturnAggregate => d!(
                "restricted-return-aggregate",
                ReturningAndAggregates,
                Supported,
                StrictWrite,
                "Restricted aggregate returning over write results"
            ),
            GqlFeature::RestrictedPathVariableReturn => d!(
                "restricted-path-variable-return",
                ReturningAndAggregates,
                Supported,
                StrictWrite,
                "Restricted path variables over matched write rows; JSON path shape"
            ),
            GqlFeature::ReturnStar => d!(
                "return-star",
                ReturningAndAggregates,
                Supported,
                PortableGql,
                "RETURN * / WITH * over bound variables (read reference; deterministic ordering)"
            ),
            GqlFeature::EqualityPredicate => d!(
                "equality-predicate",
                PredicatesAndExpressions,
                Supported,
                StrictWrite,
                "Equality property predicates in MATCH filters"
            ),
            GqlFeature::MembershipInPredicate => d!(
                "membership-in-predicate",
                PredicatesAndExpressions,
                Supported,
                StrictWrite,
                "IN membership predicates over literal value lists"
            ),
            GqlFeature::NullPredicate => d!(
                "null-predicate",
                PredicatesAndExpressions,
                Supported,
                StrictWrite,
                "IS NULL / IS NOT NULL predicates"
            ),
            GqlFeature::StringPredicate => d!(
                "string-predicate",
                PredicatesAndExpressions,
                Supported,
                StrictWrite,
                "String predicates (STARTS WITH / ENDS WITH / CONTAINS family)"
            ),
            GqlFeature::OrderedComparisonPredicate => d!(
                "ordered-comparison-predicate",
                PredicatesAndExpressions,
                Supported,
                StrictWrite,
                "Ordered comparison predicates (<, <=, >, >=)"
            ),
            GqlFeature::NestedNegatedOrPredicateGroups => d!(
                "nested-negated-or-predicate-groups",
                PredicatesAndExpressions,
                Supported,
                StrictWrite,
                "Nested negated same-property OR groups lowered to bounded AND vectors"
            ),
            GqlFeature::GeneralExpressionTree => d!(
                "general-expression-tree",
                PredicatesAndExpressions,
                Supported,
                PortableGql,
                "General expression evaluator in the read reference (arithmetic, boolean, comparison, null, list, CASE, property/parameter)"
            ),
            GqlFeature::ThreeValuedLogic => d!(
                "three-valued-logic",
                PredicatesAndExpressions,
                Supported,
                PortableGql,
                "TRUE/FALSE/UNKNOWN boolean logic with WHERE keep-only-TRUE (read reference)"
            ),
            GqlFeature::ScalarFunctionRegistry => d!(
                "scalar-function-registry",
                PredicatesAndExpressions,
                Supported,
                PortableGql,
                "Scalar function registry in the read reference (string/numeric casts, coalesce, size, ...)"
            ),
            GqlFeature::AggregateFunctionRegistry => d!(
                "aggregate-function-registry",
                PredicatesAndExpressions,
                Supported,
                PortableGql,
                "count/sum/avg/min/max/collect with implicit GROUP BY (read reference)"
            ),
            GqlFeature::CreateConstraint => d!(
                "create-constraint",
                ConstraintsAndIndexes,
                Supported,
                StrictWrite,
                "CREATE CONSTRAINT DDL registered in the constraint registry"
            ),
            GqlFeature::DropConstraint => d!(
                "drop-constraint",
                ConstraintsAndIndexes,
                Supported,
                StrictWrite,
                "DROP CONSTRAINT DDL"
            ),
            GqlFeature::ConstraintRegistry => d!(
                "constraint-registry",
                ConstraintsAndIndexes,
                Supported,
                StrictWrite,
                "Typed constraint registry metadata"
            ),
            GqlFeature::IndexDefinition => d!(
                "index-definition",
                ConstraintsAndIndexes,
                Planned,
                PortableGql,
                "Index DDL with per-backend capability reporting (Unit 11)"
            ),
            GqlFeature::ReadOnlyMatchReturn => d!(
                "read-only-match-return",
                ReadOnlyMatching,
                Supported,
                PortableGql,
                "Read-only MATCH ... RETURN on the Memory reference (no write)"
            ),
            GqlFeature::LabelTypePredicateMatch => d!(
                "label-type-predicate-match",
                ReadOnlyMatching,
                Supported,
                PortableGql,
                "Single label + property-equality predicates in read matching (Memory reference)"
            ),
            GqlFeature::OptionalMatch => d!(
                "optional-match",
                ReadOnlyMatching,
                Supported,
                PortableGql,
                "OPTIONAL MATCH with null-padding semantics (Memory reference)"
            ),
            GqlFeature::PathVariableBinding => d!(
                "path-variable-binding",
                PathMatching,
                Supported,
                PortableGql,
                "Fixed-length path variables + nodes()/relationships()/length() (read reference; not yet over variable-length)"
            ),
            GqlFeature::BoundedPathPattern => d!(
                "bounded-path-pattern",
                PathMatching,
                Supported,
                PortableGql,
                "Fixed-length node/edge/path patterns with direction (Memory reference)"
            ),
            GqlFeature::QuantifiedPathPattern => d!(
                "quantified-path-pattern",
                PathMatching,
                Supported,
                PortableGql,
                "Variable-length relationships (*min..max), no repeated nodes (Memory reference)"
            ),
            GqlFeature::ShortestPath => d!(
                "shortest-path",
                PathMatching,
                Future,
                Full39075,
                "Shortest-path families (Unit 9)"
            ),
            GqlFeature::WithClause => d!(
                "with-clause",
                QueryComposition,
                Supported,
                PortableGql,
                "WITH horizon: projection, aggregation, WHERE, DISTINCT/ORDER/SKIP/LIMIT (read reference)"
            ),
            GqlFeature::UnionClause => d!(
                "union-clause",
                QueryComposition,
                Supported,
                PortableGql,
                "UNION / UNION ALL set composition (read reference)"
            ),
            GqlFeature::Subquery => d!(
                "subquery",
                QueryComposition,
                Future,
                PortableGql,
                "Subquery skeletons with scope visibility (Unit 8)"
            ),
            GqlFeature::DistinctOrderingLimit => d!(
                "distinct-ordering-limit",
                QueryComposition,
                Supported,
                PortableGql,
                "DISTINCT, ORDER BY, SKIP, LIMIT in read composition (read reference)"
            ),
            GqlFeature::TemporalValues => d!(
                "temporal-values",
                TypeSystem,
                Supported,
                PortableGql,
                "Typed temporal values (Value::DateTime) with chronological comparison/ordering (Unit T)"
            ),
            GqlFeature::DurationValues => d!(
                "duration-values",
                TypeSystem,
                Supported,
                PortableGql,
                "Typed duration values (Value::Duration), ISO 8601 constructor, +/- arithmetic, ordering (Unit T)"
            ),
            GqlFeature::DecimalValues => d!(
                "decimal-values",
                TypeSystem,
                Supported,
                PortableGql,
                "Typed lossless decimal values (Value::Decimal), constructor, exact +/-/* arithmetic, ordering (Unit T)"
            ),
            GqlFeature::PathValues => d!(
                "path-values",
                TypeSystem,
                Future,
                PortableGql,
                "First-class path values in the type lattice (Unit T)"
            ),
            GqlFeature::GraphValues => d!(
                "graph-values",
                TypeSystem,
                Future,
                Full39075,
                "First-class graph values in the type lattice (Unit T)"
            ),
            GqlFeature::TransactionControl => d!(
                "transaction-control",
                Transactions,
                Future,
                Full39075,
                "Transaction statements with atomicity/rollback capability reporting (Unit 13)"
            ),
            GqlFeature::SessionControl => d!(
                "session-control",
                Transactions,
                Future,
                Full39075,
                "Session statements and session capability reporting (Unit 13)"
            ),
            GqlFeature::GraphTypeDefinition => d!(
                "graph-type-definition",
                CatalogAndSession,
                Future,
                Full39075,
                "Graph type definitions for nodes/edges/labels/properties (Unit 11)"
            ),
            GqlFeature::CatalogMetadata => d!(
                "catalog-metadata",
                CatalogAndSession,
                Future,
                Full39075,
                "Catalog metadata and named graph collections (Unit 11)"
            ),
            GqlFeature::NamedGraphSelection => d!(
                "named-graph-selection",
                CatalogAndSession,
                Future,
                Full39075,
                "Named graph selection and session defaults (Units 11, 13)"
            ),
            GqlFeature::ProcedureCall => d!(
                "procedure-call",
                ProceduresAndFunctions,
                Supported,
                PortableGql,
                "Read-only catalog procedures via CALL [YIELD]: db.labels, db.relationshipTypes, db.propertyKeys (Unit 14)"
            ),
            GqlFeature::TableValuedFunction => d!(
                "table-valued-function",
                ProceduresAndFunctions,
                Future,
                Full39075,
                "Table-valued functions (Unit 14)"
            ),
            GqlFeature::NativeCypherPassthrough => d!(
                "native-cypher-passthrough",
                NativePassthrough,
                Planned,
                Full39075,
                "Backend-native Cypher/SurrealQL/Falkor passthrough, separate from portable conformance (Unit 14)"
            ),
            GqlFeature::RejectCreateNodeWithoutExplicitIdentity => d!(
                "reject-create-node-without-explicit-identity",
                ResolvedWrites,
                Rejected,
                StrictWrite,
                "CREATE without an explicit node id is rejected unless GenerateForCreate is selected"
            ),
            GqlFeature::RejectMergeWithoutExplicitIdentity => d!(
                "reject-merge-without-explicit-identity",
                ResolvedWrites,
                Rejected,
                StrictWrite,
                "MERGE without an explicit identity is rejected"
            ),
            GqlFeature::RejectUnresolvedEdgeEndpointWrite => d!(
                "reject-unresolved-edge-endpoint-write",
                ResolvedWrites,
                Rejected,
                StrictWrite,
                "Writing an edge whose endpoint node ids do not resolve is rejected"
            ),
            GqlFeature::RejectNonLiteralAssignmentValue => d!(
                "reject-non-literal-assignment-value",
                BroadMatchedWrites,
                Rejected,
                StrictWrite,
                "Non-literal SET assignment values are rejected (literal-only in the strict surface)"
            ),
            GqlFeature::RejectTrailingNodeCreationAfterRowProducingEdge => d!(
                "reject-trailing-node-creation-after-row-producing-edge",
                RowProducingRelationshipWrites,
                Rejected,
                StrictWrite,
                "Row-producing edge CREATE rejects trailing node creation"
            ),
        }
    }

    /// Stable string identifier.
    pub const fn id(self) -> &'static str {
        self.descriptor().id
    }

    pub const fn family(self) -> GqlFeatureFamily {
        self.descriptor().family
    }

    pub const fn status(self) -> GqlFeatureStatus {
        self.descriptor().status
    }

    pub const fn min_profile(self) -> GqlConformanceProfile {
        self.descriptor().min_profile
    }

    pub const fn summary(self) -> &'static str {
        self.descriptor().summary
    }

    /// Resolve a feature from its stable string id.
    pub fn from_id(id: &str) -> Option<GqlFeature> {
        GqlFeature::ALL
            .iter()
            .copied()
            .find(|feature| feature.id() == id)
    }

    /// True when this feature is part of (included by) the given profile and is
    /// currently implemented.
    pub fn is_supported_in(self, profile: GqlConformanceProfile) -> bool {
        self.status() == GqlFeatureStatus::Supported && profile.includes(self.min_profile())
    }
}

impl fmt::Display for GqlFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl From<GqlFeature> for String {
    fn from(feature: GqlFeature) -> Self {
        feature.id().to_string()
    }
}

impl TryFrom<String> for GqlFeature {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        GqlFeature::from_id(&value).ok_or_else(|| format!("unknown GQL feature id: {value}"))
    }
}

/// The full feature manifest: every cataloged feature with its descriptor.
pub fn feature_manifest() -> Vec<GqlFeatureDescriptor> {
    GqlFeature::ALL
        .iter()
        .copied()
        .map(GqlFeature::descriptor)
        .collect()
}

/// Counts of features by status (supported, rejected, planned, future).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GqlSupportCounts {
    pub supported: usize,
    pub rejected: usize,
    pub planned: usize,
    pub future: usize,
}

impl GqlSupportCounts {
    pub fn total(&self) -> usize {
        self.supported + self.rejected + self.planned + self.future
    }
}

/// Tally the manifest by status.
pub fn support_counts() -> GqlSupportCounts {
    let mut counts = GqlSupportCounts::default();
    for feature in GqlFeature::ALL.iter().copied() {
        match feature.status() {
            GqlFeatureStatus::Supported => counts.supported += 1,
            GqlFeatureStatus::Rejected => counts.rejected += 1,
            GqlFeatureStatus::Planned => counts.planned += 1,
            GqlFeatureStatus::Future => counts.future += 1,
        }
    }
    counts
}

/// Generate a human-readable Markdown support summary across all families.
///
/// This is the "print or generate a current support summary" deliverable of
/// Unit 1. It is deterministic and ordering-stable for use in docs and tests.
pub fn support_summary() -> String {
    let counts = support_counts();
    let mut out = String::new();
    out.push_str("# Grust GQL/Cypher Support Summary\n\n");
    out.push_str(&format!(
        "Total cataloged features: {} (supported {}, rejected {}, planned {}, future {}).\n\n",
        counts.total(),
        counts.supported,
        counts.rejected,
        counts.planned,
        counts.future
    ));

    let families = [
        GqlFeatureFamily::ParserAndSemantics,
        GqlFeatureFamily::ResolvedWrites,
        GqlFeatureFamily::BroadMatchedWrites,
        GqlFeatureFamily::RowProducingRelationshipWrites,
        GqlFeatureFamily::ReturningAndAggregates,
        GqlFeatureFamily::PredicatesAndExpressions,
        GqlFeatureFamily::ReadOnlyMatching,
        GqlFeatureFamily::PathMatching,
        GqlFeatureFamily::QueryComposition,
        GqlFeatureFamily::TypeSystem,
        GqlFeatureFamily::ConstraintsAndIndexes,
        GqlFeatureFamily::Transactions,
        GqlFeatureFamily::CatalogAndSession,
        GqlFeatureFamily::ProceduresAndFunctions,
        GqlFeatureFamily::NativePassthrough,
    ];

    for family in families {
        let entries: Vec<_> = GqlFeature::ALL
            .iter()
            .copied()
            .filter(|f| f.family() == family)
            .collect();
        if entries.is_empty() {
            continue;
        }
        out.push_str(&format!("## {}\n\n", family.id()));
        for feature in entries {
            let d = feature.descriptor();
            out.push_str(&format!(
                "- `{}` [{}, {}]: {}\n",
                d.id,
                d.status.id(),
                d.min_profile.id(),
                d.summary
            ));
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Backend conformance profiles (Unit 12)
// ---------------------------------------------------------------------------

/// The role a backend plays in the GQL/Cypher conformance picture.
///
/// Only [`GqlBackendRole::CypherExecutor`] backends are part of the *executing*
/// conformance set (they run the portable Cypher surface); the others are
/// catalogued honestly so the matrix is not overclaimed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GqlBackendRole {
    /// Executes portable Cypher (a `CypherMutationExecutor` and/or read path).
    CypherExecutor,
    /// A SQL / SQL-PGQ graph store with its own surface; no portable Cypher
    /// executor yet (it could join the executing set later).
    SqlGraphBackend,
    /// An export/sync target, not a query backend.
    SyncTarget,
    /// Internal-only (`publish = false`); out of the facade and the executing
    /// conformance set (conformance/cost artifacts stay test-only).
    Internal,
}

impl GqlBackendRole {
    pub const fn id(self) -> &'static str {
        match self {
            GqlBackendRole::CypherExecutor => "cypher-executor",
            GqlBackendRole::SqlGraphBackend => "sql-graph-backend",
            GqlBackendRole::SyncTarget => "sync-target",
            GqlBackendRole::Internal => "internal",
        }
    }
}

impl fmt::Display for GqlBackendRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// The backends Grust catalogs for GQL/Cypher conformance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GqlBackend {
    Memory,
    Sail,
    Turso,
    Postgres,
    PostgresPgq,
    Helix,
    Ladybug,
    CocoIndex,
}

/// An honest per-backend capability record. Booleans reflect the *current*
/// working tree (verified against the code), not aspirations.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GqlBackendDescriptor {
    pub backend: GqlBackend,
    pub id: &'static str,
    pub crate_name: &'static str,
    pub role: GqlBackendRole,
    /// Published to crates.io (not `publish = false`).
    pub publishable: bool,
    /// Exposed as a `grust-graph` facade feature.
    pub in_facade: bool,
    /// Implements `CypherMutationExecutor` (executes the writable Cypher subset).
    pub cypher_writes: bool,
    /// Executes the portable read core (as the reference, or via SQL pushdown).
    pub portable_reads: bool,
    /// Has SQL read **pushdown** wired into a `run_read_query` entrypoint.
    pub read_pushdown: bool,
    pub summary: &'static str,
}

impl GqlBackend {
    pub const fn descriptor(self) -> GqlBackendDescriptor {
        use GqlBackend::*;
        use GqlBackendRole::*;
        let (id, crate_name, role, publishable, in_facade, writes, reads, pushdown, summary) =
            match self {
                Memory => (
                    "memory", "grust-memory", CypherExecutor, true, true, true, true, false,
                    "In-memory reference executor: the portable read oracle and the strict-write reference.",
                ),
                Sail => (
                    "sail", "grust-sail", CypherExecutor, true, true, true, true, true,
                    "Sail/Spark: CypherMutationExecutor plus run_read_query SQL pushdown over grust_nodes/grust_edges.",
                ),
                Turso => (
                    "turso", "grust-turso", CypherExecutor, true, true, true, false, false,
                    "libSQL/Turso: CypherMutationExecutor (bounded matched-node patches); the embedded SQL differential oracle for read pushdown.",
                ),
                Postgres => (
                    "postgres", "grust-postgres", SqlGraphBackend, true, true, false, false, false,
                    "PostgreSQL universal-table SQL backend; no portable Cypher executor yet.",
                ),
                PostgresPgq => (
                    "postgres-pgq", "grust-postgres-pgq", SqlGraphBackend, true, true, false, false, false,
                    "PostgreSQL SQL/PGQ: native PROPERTY GRAPH + GRAPH_TABLE traversal; no portable Cypher executor yet.",
                ),
                Helix => (
                    "helix", "grust-helix", Internal, false, false, false, false, false,
                    "Internal only (publish=false): out of the facade and the executing-conformance set.",
                ),
                Ladybug => (
                    "ladybug", "grust-ladybug", Internal, false, false, false, false, false,
                    "Internal only (publish=false): out of the facade and the executing-conformance set.",
                ),
                CocoIndex => (
                    "cocoindex", "grust-cocoindex", SyncTarget, true, true, false, false, false,
                    "Sync/export target, not a query backend; out of the executing-conformance set.",
                ),
            };
        GqlBackendDescriptor {
            backend: self,
            id,
            crate_name,
            role,
            publishable,
            in_facade,
            cypher_writes: writes,
            portable_reads: reads,
            read_pushdown: pushdown,
            summary,
        }
    }

    /// All catalogued backends, narrowest concern first.
    pub const ALL: [GqlBackend; 8] = [
        GqlBackend::Memory,
        GqlBackend::Sail,
        GqlBackend::Turso,
        GqlBackend::Postgres,
        GqlBackend::PostgresPgq,
        GqlBackend::Helix,
        GqlBackend::Ladybug,
        GqlBackend::CocoIndex,
    ];
}

/// Descriptors for every catalogued backend.
pub fn backend_manifest() -> Vec<GqlBackendDescriptor> {
    GqlBackend::ALL.iter().map(|b| b.descriptor()).collect()
}

/// The backends in the *executing* Cypher-conformance set (role
/// [`GqlBackendRole::CypherExecutor`]).
pub fn cypher_conformance_backends() -> Vec<GqlBackend> {
    GqlBackend::ALL
        .iter()
        .copied()
        .filter(|b| b.descriptor().role == GqlBackendRole::CypherExecutor)
        .collect()
}

// ---------------------------------------------------------------------------
// Structured, feature-tagged errors
// ---------------------------------------------------------------------------

/// Standard-shaped error categories for GQL processing.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GqlErrorKind {
    /// A recognized but unimplemented/unsupported standard feature.
    UnsupportedFeature,
    /// Lexical or grammatical error.
    Syntax,
    /// Name/binding/scope resolution error.
    Name,
    /// Type or coercion error.
    Type,
    /// Cardinality (too-many/too-few rows or matches) error.
    Cardinality,
    /// Execution-time failure.
    Execution,
}

impl GqlErrorKind {
    pub const fn id(self) -> &'static str {
        match self {
            GqlErrorKind::UnsupportedFeature => "unsupported-feature",
            GqlErrorKind::Syntax => "syntax",
            GqlErrorKind::Name => "name",
            GqlErrorKind::Type => "type",
            GqlErrorKind::Cardinality => "cardinality",
            GqlErrorKind::Execution => "execution",
        }
    }
}

impl fmt::Display for GqlErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// A feature-tagged structured GQL error.
///
/// Converts into the existing [`GrustError`] transport via `From`, so it flows
/// through `grust_core::Result` without changing the workspace error surface.
/// The feature id and conformance profile are embedded in the rendered message
/// so they survive the conversion until a richer error variant lands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GqlError {
    pub kind: GqlErrorKind,
    pub feature: Option<GqlFeature>,
    pub profile: Option<GqlConformanceProfile>,
    pub message: String,
}

impl GqlError {
    pub fn new(kind: GqlErrorKind, message: impl Into<String>) -> Self {
        GqlError {
            kind,
            feature: None,
            profile: None,
            message: message.into(),
        }
    }

    pub fn with_feature(mut self, feature: GqlFeature) -> Self {
        self.feature = Some(feature);
        self
    }

    pub fn with_profile(mut self, profile: GqlConformanceProfile) -> Self {
        self.profile = Some(profile);
        self
    }
}

impl fmt::Display for GqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[gql:{}", self.kind)?;
        if let Some(feature) = self.feature {
            write!(f, " feature={} family={}", feature.id(), feature.family())?;
        }
        if let Some(profile) = self.profile {
            write!(f, " profile={profile}")?;
        }
        write!(f, "] {}", self.message)
    }
}

impl std::error::Error for GqlError {}

impl From<GqlError> for GrustError {
    fn from(error: GqlError) -> Self {
        let rendered = error.to_string();
        match error.kind {
            GqlErrorKind::Syntax => GrustError::CypherSyntax(rendered),
            GqlErrorKind::Name => GrustError::CypherUnresolvedIdentity(rendered),
            GqlErrorKind::Cardinality => GrustError::CypherUnsupportedCardinality(rendered),
            GqlErrorKind::UnsupportedFeature => GrustError::Unsupported(rendered),
            // `Type` has no dedicated transport variant yet; it maps to the
            // generic execution channel until the Unit T type system lands a
            // first-class variant. The `[gql:type ...]` tag preserves the kind.
            GqlErrorKind::Type | GqlErrorKind::Execution => GrustError::CypherExecution(rendered),
        }
    }
}

/// Construct an unsupported-feature error naming the feature and target profile.
pub fn unsupported_gql_feature(
    feature: GqlFeature,
    profile: GqlConformanceProfile,
    message: impl Into<String>,
) -> GrustError {
    GqlError::new(GqlErrorKind::UnsupportedFeature, message)
        .with_feature(feature)
        .with_profile(profile)
        .into()
}

/// Construct a feature-tagged GQL syntax error.
pub fn gql_syntax(message: impl Into<String>) -> GrustError {
    GqlError::new(GqlErrorKind::Syntax, message).into()
}

/// Construct a feature-tagged GQL name/binding error.
pub fn gql_name(message: impl Into<String>) -> GrustError {
    GqlError::new(GqlErrorKind::Name, message).into()
}

/// Construct a feature-tagged GQL type error.
pub fn gql_type(message: impl Into<String>) -> GrustError {
    GqlError::new(GqlErrorKind::Type, message).into()
}

/// Construct a feature-tagged GQL cardinality error.
pub fn gql_cardinality(message: impl Into<String>) -> GrustError {
    GqlError::new(GqlErrorKind::Cardinality, message).into()
}

/// Construct a feature-tagged GQL execution error.
pub fn gql_execution(message: impl Into<String>) -> GrustError {
    GqlError::new(GqlErrorKind::Execution, message).into()
}

// ---------------------------------------------------------------------------
// Conformance manifest test-case metadata
// ---------------------------------------------------------------------------

/// How a conformance case is classified for a given run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GqlRequirement {
    /// Must pass for the profile to be claimed.
    Required,
    /// May pass; informative.
    Optional,
    /// Pass/skip depends on the backend's declared capabilities.
    BackendGated,
    /// Targeted at a future profile; expected to be unsupported today.
    Future,
}

/// Expected outcome for a conformance case.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GqlExpectation {
    /// The statement is expected to be accepted/executed.
    Supported,
    /// The statement is expected to be rejected with a structured error.
    Rejected,
}

/// A single conformance manifest case loaded from `tests/gql/*.json`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlManifestCase {
    /// Unique case id within the corpus.
    pub id: String,
    /// The cataloged feature this case exercises (stable feature id).
    pub feature: GqlFeature,
    /// Conformance classification.
    pub requirement: GqlRequirement,
    /// The statement under test.
    pub statement: String,
    /// Expected outcome.
    pub expectation: GqlExpectation,
    /// For `Rejected` cases, the expected structured error kind.
    #[serde(default)]
    pub error_kind: Option<GqlErrorKind>,
    /// Optional free-form note.
    #[serde(default)]
    pub notes: Option<String>,
}

/// A named manifest file: a profile plus its cases.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlManifest {
    pub profile: GqlConformanceProfile,
    pub cases: Vec<GqlManifestCase>,
}

/// Parse a conformance manifest from JSON, validating internal consistency.
///
/// Each case's `feature` is validated by `serde` against the [`GqlFeature`]
/// taxonomy on deserialization (unknown ids are a parse error). This additionally
/// checks that `Rejected` cases carry an `errorKind` and that case ids are unique.
pub fn load_manifest(json: &str) -> Result<GqlManifest> {
    let manifest: GqlManifest = serde_json::from_str(json)
        .map_err(|err| GqlError::new(GqlErrorKind::Syntax, format!("invalid manifest: {err}")))?;

    let mut seen = std::collections::BTreeSet::new();
    for case in &manifest.cases {
        if !seen.insert(case.id.as_str()) {
            return Err(GqlError::new(
                GqlErrorKind::Name,
                format!("duplicate manifest case id: {}", case.id),
            )
            .into());
        }
        if case.expectation == GqlExpectation::Rejected && case.error_kind.is_none() {
            return Err(GqlError::new(
                GqlErrorKind::Syntax,
                format!("rejected case {} must specify an errorKind", case.id),
            )
            .into());
        }
    }
    Ok(manifest)
}

/// Convenience: parse just the case array from JSON.
pub fn load_manifest_cases(json: &str) -> Result<Vec<GqlManifestCase>> {
    Ok(load_manifest(json)?.cases)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_manifest_is_honest_and_consistent() {
        let m = backend_manifest();
        assert_eq!(m.len(), GqlBackend::ALL.len());
        // Unique ids.
        let mut ids = std::collections::BTreeSet::new();
        for d in &m {
            assert!(ids.insert(d.id), "duplicate backend id {}", d.id);
        }
        // The executing Cypher-conformance set is exactly Memory/Sail/Turso.
        let exec: Vec<_> = cypher_conformance_backends();
        assert_eq!(
            exec,
            vec![GqlBackend::Memory, GqlBackend::Sail, GqlBackend::Turso]
        );
        for d in &m {
            // Only Cypher executors execute writes / portable reads / pushdown.
            if d.role != GqlBackendRole::CypherExecutor {
                assert!(!d.cypher_writes, "{} should not claim cypher writes", d.id);
                assert!(!d.portable_reads, "{} should not claim portable reads", d.id);
                assert!(!d.read_pushdown, "{} should not claim read pushdown", d.id);
            }
            // Read pushdown implies portable reads.
            if d.read_pushdown {
                assert!(d.portable_reads, "{} pushdown without reads", d.id);
            }
            // Internal backends are never publishable or in the facade.
            if d.role == GqlBackendRole::Internal {
                assert!(!d.publishable && !d.in_facade, "{} internal but exposed", d.id);
            }
        }
        // Verified specifics: only Sail has read pushdown; only Memory/Sail/Turso
        // write Cypher; helix/ladybug are internal; cocoindex is a sync target.
        assert!(GqlBackend::Sail.descriptor().read_pushdown);
        assert!(!GqlBackend::Memory.descriptor().read_pushdown);
        assert!(!GqlBackend::Postgres.descriptor().cypher_writes);
        assert_eq!(GqlBackend::Helix.descriptor().role, GqlBackendRole::Internal);
        assert_eq!(GqlBackend::CocoIndex.descriptor().role, GqlBackendRole::SyncTarget);
    }

    #[test]
    fn every_feature_has_a_unique_id() {
        let mut seen = std::collections::BTreeSet::new();
        for feature in GqlFeature::ALL.iter().copied() {
            let id = feature.id();
            assert!(!id.is_empty(), "{feature:?} has empty id");
            assert!(seen.insert(id), "duplicate feature id: {id}");
        }
    }

    #[test]
    fn all_array_covers_every_descriptor_roundtrip() {
        // from_id round-trips for every feature.
        for feature in GqlFeature::ALL.iter().copied() {
            assert_eq!(GqlFeature::from_id(feature.id()), Some(feature));
        }
        assert_eq!(GqlFeature::from_id("definitely-not-a-feature"), None);
    }

    #[test]
    fn descriptor_self_reference_is_consistent() {
        for feature in GqlFeature::ALL.iter().copied() {
            assert_eq!(feature.descriptor().feature, feature);
        }
    }

    #[test]
    fn rejected_forms_are_present_in_the_manifest() {
        let rejected: Vec<_> = feature_manifest()
            .into_iter()
            .filter(|d| d.status == GqlFeatureStatus::Rejected)
            .collect();
        assert!(
            rejected.len() >= 5,
            "expected the strict-write rejected forms to be cataloged, got {}",
            rejected.len()
        );
    }

    #[test]
    fn strict_write_surface_is_classified_supported() {
        // Spot-check that representative current features are Supported in StrictWrite.
        for feature in [
            GqlFeature::CreateNodeExplicitId,
            GqlFeature::MergeNodeExplicitId,
            GqlFeature::CreateEdgeResolvedEndpoints,
            GqlFeature::DeleteResolvedNode,
            GqlFeature::OrderedMultiStatementBatch,
            GqlFeature::SetNodeProperty,
            GqlFeature::RemoveNodeProperty,
            GqlFeature::MatchCreateEdgeRowProducing,
            GqlFeature::CreateConstraint,
        ] {
            assert!(
                feature.is_supported_in(GqlConformanceProfile::StrictWrite),
                "{feature:?} should be supported in StrictWrite"
            );
        }
        // A future feature is not supported yet, even at the widest profile.
        assert!(!GqlFeature::TransactionControl.is_supported_in(GqlConformanceProfile::Full39075));
    }

    #[test]
    fn profile_inclusion_is_monotonic() {
        use GqlConformanceProfile::*;
        assert!(Full39075.includes(PortableGql));
        assert!(PortableGql.includes(StrictWrite));
        assert!(Full39075.includes(StrictWrite));
        assert!(!StrictWrite.includes(PortableGql));
    }

    #[test]
    fn support_summary_is_nonempty_and_lists_families() {
        let summary = support_summary();
        assert!(summary.contains("Support Summary"));
        assert!(summary.contains("resolved-writes"));
        assert!(summary.contains("create-node-explicit-id"));
        // deterministic: regenerating yields the same text
        assert_eq!(summary, support_summary());
    }

    #[test]
    fn support_counts_total_matches_catalog() {
        assert_eq!(support_counts().total(), GqlFeature::ALL.len());
    }

    #[test]
    fn feature_serde_uses_stable_id() {
        let json = serde_json::to_string(&GqlFeature::CreateNodeExplicitId).unwrap();
        assert_eq!(json, "\"create-node-explicit-id\"");
        let back: GqlFeature = serde_json::from_str(&json).unwrap();
        assert_eq!(back, GqlFeature::CreateNodeExplicitId);
    }

    #[test]
    fn unknown_feature_id_fails_to_deserialize() {
        let result: std::result::Result<GqlFeature, _> = serde_json::from_str("\"nope\"");
        assert!(result.is_err());
    }

    #[test]
    fn structured_errors_map_to_transport_variants() {
        let unsupported = unsupported_gql_feature(
            GqlFeature::OptionalMatch,
            GqlConformanceProfile::PortableGql,
            "OPTIONAL MATCH is not implemented yet",
        );
        assert!(matches!(unsupported, GrustError::Unsupported(_)));
        let rendered = unsupported.to_string();
        assert!(rendered.contains("feature=optional-match"));
        assert!(rendered.contains("profile=portable-gql"));

        assert!(matches!(gql_syntax("x"), GrustError::CypherSyntax(_)));
        assert!(matches!(
            gql_name("x"),
            GrustError::CypherUnresolvedIdentity(_)
        ));
        assert!(matches!(
            gql_cardinality("x"),
            GrustError::CypherUnsupportedCardinality(_)
        ));
        assert!(matches!(gql_type("x"), GrustError::CypherExecution(_)));
        assert!(matches!(gql_execution("x"), GrustError::CypherExecution(_)));
    }

    #[test]
    fn manifest_loads_and_validates() {
        let json = r#"
        {
          "profile": "strict-write",
          "cases": [
            {
              "id": "create-node-ok",
              "feature": "create-node-explicit-id",
              "requirement": "required",
              "statement": "CREATE (:Person {id: 'p1'})",
              "expectation": "supported"
            },
            {
              "id": "create-node-no-id",
              "feature": "reject-create-node-without-explicit-identity",
              "requirement": "required",
              "statement": "CREATE (:Person {name: 'no id'})",
              "expectation": "rejected",
              "errorKind": "unsupported-feature",
              "notes": "explicit id required unless GenerateForCreate"
            }
          ]
        }
        "#;
        let manifest = load_manifest(json).expect("manifest should parse");
        assert_eq!(manifest.profile, GqlConformanceProfile::StrictWrite);
        assert_eq!(manifest.cases.len(), 2);
        assert_eq!(manifest.cases[0].feature, GqlFeature::CreateNodeExplicitId);
    }

    #[test]
    fn manifest_rejects_duplicate_ids() {
        let json = r#"
        {
          "profile": "strict-write",
          "cases": [
            {"id": "dup", "feature": "create-node-explicit-id", "requirement": "required", "statement": "x", "expectation": "supported"},
            {"id": "dup", "feature": "merge-node-explicit-id", "requirement": "required", "statement": "y", "expectation": "supported"}
          ]
        }
        "#;
        assert!(load_manifest(json).is_err());
    }

    #[test]
    fn manifest_requires_error_kind_for_rejected_cases() {
        let json = r#"
        {
          "profile": "strict-write",
          "cases": [
            {"id": "bad", "feature": "reject-merge-without-explicit-identity", "requirement": "required", "statement": "MERGE (:X)", "expectation": "rejected"}
          ]
        }
        "#;
        assert!(load_manifest(json).is_err());
    }

    #[test]
    fn manifest_rejects_unknown_feature_id() {
        let json = r#"
        {
          "profile": "strict-write",
          "cases": [
            {"id": "x", "feature": "totally-made-up", "requirement": "required", "statement": "X", "expectation": "supported"}
          ]
        }
        "#;
        assert!(load_manifest(json).is_err());
    }
}
