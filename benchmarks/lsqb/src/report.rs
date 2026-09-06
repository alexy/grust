#![allow(dead_code)] // Report schemas are shared by the legacy, policy, and matrix runners.

use serde::{Deserialize, Serialize};

use grust_cypher::ReadQueryPolicy;

use crate::queries::RustRowEstimate;

mod execution_plan;
pub use execution_plan::ExecutionPlan;
pub(crate) use execution_plan::deserialize_present_execution_plan;

/// Legacy schema-v2 constant retained with its original meaning.
pub const COMPARISON_REPORT_SCHEMA_VERSION: u32 = 2;
pub const COMPARISON_REPORT_SCHEMA_VERSION_V2: u32 = COMPARISON_REPORT_SCHEMA_VERSION;
pub const COMPARISON_REPORT_SCHEMA_VERSION_V3: u32 = 3;
pub const POLICY_REPORT_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub warning: &'static str,
    pub suite: SuiteIdentity,
    pub environment: Environment,
    pub graph: GraphSize,
    pub runs: Vec<RunResult>,
    pub valid: bool,
}

#[derive(Debug, Serialize)]
pub struct SuiteIdentity {
    pub name: String,
    pub track: String,
    pub source_url: &'static str,
    pub source_commit: &'static str,
    pub source_tree: &'static str,
    pub query_tree: &'static str,
    pub example_dataset_tree: &'static str,
    pub license: &'static str,
    pub classification: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Environment {
    pub grust_revision: String,
    pub backend: String,
    pub scale_factor: String,
    pub repetitions: usize,
    pub rust_version: String,
    pub container_image: String,
    pub container_image_id: String,
    pub container_os: String,
    pub container_arch: String,
    pub docker_engine_version: String,
    pub docker_cpus: String,
    pub docker_memory_bytes: String,
    pub resource_limit_scope: String,
    pub postgres_image: String,
    pub host_cpu: String,
}

#[derive(Debug, Serialize)]
pub struct GraphSize {
    pub nodes: usize,
    pub edges: usize,
}

#[derive(Debug, Serialize)]
pub struct RunResult {
    pub repetition: usize,
    pub load_ns: u128,
    pub queries: Vec<QueryResult>,
}

#[derive(Debug, Serialize)]
pub struct QueryResult {
    pub id: String,
    pub source_sha256: String,
    pub adapter_sha256: String,
    pub claim: String,
    pub execution_mode: String,
    pub expected_count: i64,
    pub actual_count: Option<i64>,
    pub elapsed_ns: u128,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PolicyReport {
    pub schema_version: u32,
    pub warning: &'static str,
    pub suite: SuiteIdentity,
    pub environment: Environment,
    pub graph: GraphSize,
    pub policy: PolicyLimits,
    pub runs: Vec<PolicyRunResult>,
    pub valid: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PolicyLimits {
    pub max_query_bytes: usize,
    pub max_parameter_bytes: usize,
    pub max_graph_nodes: usize,
    pub max_graph_edges: usize,
    pub max_graph_bytes: usize,
    pub max_candidate_work: usize,
    pub max_intermediate_bytes: usize,
    pub max_result_rows: usize,
    pub max_output_bytes: usize,
    pub max_range_items: usize,
    pub max_union_arms: usize,
    pub max_path_length: u64,
    pub max_execution_time_ms: u128,
    pub allow_graph_selection: bool,
    pub allow_catalog_procedures: bool,
    pub require_match: bool,
}

impl From<&ReadQueryPolicy> for PolicyLimits {
    fn from(policy: &ReadQueryPolicy) -> Self {
        Self {
            max_query_bytes: policy.max_query_bytes,
            max_parameter_bytes: policy.max_parameter_bytes,
            max_graph_nodes: policy.max_graph_nodes,
            max_graph_edges: policy.max_graph_edges,
            max_graph_bytes: policy.max_graph_bytes,
            max_candidate_work: policy.max_candidate_work,
            max_intermediate_bytes: policy.max_intermediate_bytes,
            max_result_rows: policy.max_result_rows,
            max_output_bytes: policy.max_output_bytes,
            max_range_items: policy.max_range_items,
            max_union_arms: policy.max_union_arms,
            max_path_length: policy.max_path_length,
            max_execution_time_ms: policy.max_execution_time.as_millis(),
            allow_graph_selection: policy.allow_graph_selection,
            allow_catalog_procedures: policy.allow_catalog_procedures,
            require_match: policy.require_match,
        }
    }
}

/// Exact per-case deviations from the report's effective base policy and input.
///
/// Empty objects are serialized for cases with no deviations so evidence
/// validators can require an exhaustive, deterministic case configuration.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct PolicyCaseOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_candidate_work: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_catalog_procedures: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameter_payload_bytes: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct PolicyRunResult {
    pub repetition: usize,
    pub attacks: Vec<PolicyResult>,
}

#[derive(Debug, Serialize)]
pub struct PolicyResult {
    pub id: String,
    pub source_sha256: String,
    pub overrides: PolicyCaseOverrides,
    pub expected_rejection: String,
    pub actual_rejection: String,
    pub elapsed_ns: u128,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod policy_report_tests {
    use super::*;

    #[test]
    fn policy_snapshot_serializes_every_read_policy_field_with_an_explicit_time_unit() {
        let value = serde_json::to_value(PolicyLimits::from(&ReadQueryPolicy::default())).unwrap();
        let fields = value.as_object().unwrap();

        assert_eq!(fields.len(), 16);
        assert_eq!(value["max_execution_time_ms"], 2_000);
        assert_eq!(value["allow_graph_selection"], false);
        assert_eq!(value["allow_catalog_procedures"], false);
        assert_eq!(value["require_match"], true);
    }

    #[test]
    fn no_case_override_serializes_as_an_empty_object() {
        assert_eq!(
            serde_json::to_value(PolicyCaseOverrides::default()).unwrap(),
            serde_json::json!({})
        );
    }
}

/// Terminal state for every declared backend/case cell in a schema-v2 report.
///
/// Unsupported, unavailable, and not-applicable cells remain visible evidence;
/// they are deliberately not aliases for either a pass or a failure.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeStatus {
    Pass,
    Mismatch,
    Unsupported,
    Unavailable,
    Timeout,
    Error,
    NotApplicable,
}

impl OutcomeStatus {
    pub fn is_pass(self) -> bool {
        self == Self::Pass
    }

    pub fn was_executed(self) -> bool {
        matches!(
            self,
            Self::Pass | Self::Mismatch | Self::Timeout | Self::Error
        )
    }
}

/// Where the measured graph-query work completed.
///
/// Reports must group these classes separately; matching count semantics do
/// not make their performance figures interchangeable.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionClass {
    InProcessReference,
    BackendNativeAggregate,
    /// A durable store whose worker built a resident typed index of its own
    /// contents outside the query boundary; the count then runs in Rust over
    /// that index. Distinct from the in-process reference (no store) and
    /// from backend-native execution (the store's own engine).
    BackendResidentIndexRustCount,
    BackendRowSourceRustProjection,
    BackendMaterializeRustReference,
    BackendNeutralPolicy,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueryOrder {
    Fixed,
    Rotating,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TimingBoundary {
    SubmitToScalarConsumed,
}

/// Schema-v3's coordinator/worker timing interval.
///
/// This deliberately has a different wire value from `TimingBoundary`: v2
/// measured the backend API call in-process, while v3 starts immediately
/// before the coordinator writes GO and ends when it consumes the result.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TimingBoundaryV3 {
    CoordinatorGoToResultConsumed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TimingProtocolV2 {
    pub warmup_iterations: u32,
    pub measurement_iterations: u32,
    /// Soft per-query deadline; an expired operation is drained before the
    /// next observation so samples never overlap.
    pub query_timeout_ms: u64,
    /// Hard wall-clock limit enforced around the named Compose cell by the
    /// publication orchestrator.
    pub cell_timeout_ms: u64,
    pub query_order: QueryOrder,
    pub boundary: TimingBoundary,
    pub reload_before_each_iteration: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExecutionDescriptorV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<ExecutionClass>,
    pub language: String,
    pub transport: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_query_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueryObservationV2 {
    /// One-based iteration number within either the warm-up or measurement set.
    pub iteration: u32,
    /// One-based query position in this iteration's explicitly recorded order.
    pub query_position: u32,
    pub elapsed_ns: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_count: Option<i64>,
    pub outcome: OutcomeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueryOutcomeV2 {
    pub id: String,
    pub source_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter_sha256: Option<String>,
    pub execution: ExecutionDescriptorV2,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_count: Option<i64>,
    pub rust_rows: Option<RustRowEstimate>,
    pub outcome: OutcomeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub warmups: Vec<QueryObservationV2>,
    pub measurements: Vec<QueryObservationV2>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackendIdentityV2 {
    pub name: String,
    pub adapter: String,
    pub adapter_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner_image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner_image_id: Option<String>,
    pub resource_components: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_threads: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackendCellV2 {
    pub backend: BackendIdentityV2,
    pub setup_outcome: OutcomeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_ns: Option<u64>,
    pub queries: Vec<QueryOutcomeV2>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DatasetIdentityV2 {
    pub scale_factor: String,
    pub model: String,
    pub source_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extracted_manifest_sha256: Option<String>,
    pub csv_files: usize,
    pub csv_bytes: u64,
    pub nodes: usize,
    pub edges: usize,
    pub person_nodes: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ComparisonEnvironmentV2 {
    pub grust_revision: String,
    pub container_os: String,
    pub container_arch: String,
    pub docker_engine_version: String,
    pub cpu_model: String,
    pub cpu_limit: String,
    pub memory_limit_bytes: u64,
    #[serde(default = "default_resource_limit_scope")]
    pub resource_limit_scope: String,
}

fn default_resource_limit_scope() -> String {
    "per-container".to_string()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ComparisonReportV2 {
    pub schema_version: u32,
    pub warning: String,
    pub experiment_id: String,
    pub suite: SuiteIdentityV2,
    pub environment: ComparisonEnvironmentV2,
    pub dataset: DatasetIdentityV2,
    pub timing: TimingProtocolV2,
    pub backends: Vec<BackendCellV2>,
    /// True only when every backend declared by the experiment has a terminal
    /// cell, including explicit unsupported/not-applicable cells.
    pub complete: bool,
    /// True only when all executed supported measurements matched their oracle.
    pub valid: bool,
}

/// Hard-deadline enforcement used by schema-v3 matrix observations.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TimeoutEnforcementV3 {
    CoordinatorProcessGroup,
}

/// How an observation worker left its isolated process boundary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObservationTerminationV3 {
    NormalExit,
    BackendTimeout,
    /// The worker group had already disappeared when deadline enforcement
    /// attempted to signal it, and the coordinator subsequently reaped it.
    DeadlineObservedExit,
    DeadlineSigterm,
    DeadlineSigkill,
}

/// Whether graph setup can be shared safely with a fresh observation worker.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoadStrategyV3 {
    OnceWorkerAttach,
    PerObservationWorkerReload,
    NotExecuted,
}

/// Proof required after forcibly terminating a worker.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryContractV3 {
    ProcessGroupAbsent,
    PostgresSessionAbsent,
    FalkorServerDeadline,
    FailClosed,
    NotApplicable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TimingProtocolV3 {
    pub warmup_iterations: u32,
    pub measurement_iterations: u32,
    pub query_timeout_ms: u64,
    /// Maximum unmeasured setup time before GO. Expiry is a fatal cell error.
    pub worker_ready_timeout_ms: u64,
    /// Grace between a deadline SIGTERM and escalation to SIGKILL.
    pub query_reap_grace_ms: u64,
    /// Maximum wait after SIGKILL for the worker group to disappear.
    pub query_kill_reap_timeout_ms: u64,
    /// Maximum wait for backend-specific server-side quiescence proof.
    pub query_recovery_timeout_ms: u64,
    /// Last-resort wall-clock limit around the complete container cell.
    pub cell_timeout_ms: u64,
    pub timeout_enforcement: TimeoutEnforcementV3,
    pub query_order: QueryOrder,
    pub boundary: TimingBoundaryV3,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackendLifecycleV3 {
    pub load_strategy: LoadStrategyV3,
    pub recovery_contract: RecoveryContractV3,
    /// Present only when the cell stopped early because the backend could
    /// not prove quiescence after an unacknowledged worker exit. The named
    /// observation is the last one taken; every query left short of the
    /// sampling contract is an explicit `error` with the same reason code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminated: Option<CellTerminationV3>,
}

/// Why and where an executed cell stopped taking observations.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CellTerminationV3 {
    pub query_id: String,
    /// `warmup` or `measurement`.
    pub phase: String,
    pub iteration: u32,
    pub reason_code: String,
    pub detail: String,
}

pub const QUIESCENCE_UNPROVEN: &str = "backend.quiescence-unproven";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueryObservationV3 {
    pub iteration: u32,
    pub query_position: u32,
    /// Worker-declared route, absent only in legacy observations. Backend-native
    /// describes opaque server execution, not a claimed server physical plan.
    #[serde(
        default,
        deserialize_with = "deserialize_present_execution_plan",
        skip_serializing_if = "Option::is_none"
    )]
    pub plan: Option<ExecutionPlan>,
    /// Worker preparation/attach interval completed before READY and GO.
    pub setup_ns: u64,
    /// GO through result consumption or the coordinator's observed deadline.
    /// Deadline observations may exceed the configured cutoff by scheduler
    /// wake-up latency; termination and reap time is recorded in `recovery_ns`.
    pub elapsed_ns: u64,
    /// Result/deadline through proven worker and backend quiescence.
    pub recovery_ns: u64,
    pub termination: ObservationTerminationV3,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_count: Option<i64>,
    pub outcome: OutcomeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueryOutcomeV3 {
    pub id: String,
    pub source_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter_sha256: Option<String>,
    pub execution: ExecutionDescriptorV2,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_count: Option<i64>,
    pub rust_rows: Option<RustRowEstimate>,
    pub outcome: OutcomeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub warmups: Vec<QueryObservationV3>,
    pub measurements: Vec<QueryObservationV3>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackendCellV3 {
    pub backend: BackendIdentityV2,
    pub lifecycle: BackendLifecycleV3,
    pub setup_outcome: OutcomeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_ns: Option<u64>,
    pub queries: Vec<QueryOutcomeV3>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ComparisonReportV3 {
    pub schema_version: u32,
    pub warning: String,
    pub experiment_id: String,
    pub suite: SuiteIdentityV2,
    pub environment: ComparisonEnvironmentV2,
    pub dataset: DatasetIdentityV2,
    pub timing: TimingProtocolV3,
    pub backends: Vec<BackendCellV3>,
    pub complete: bool,
    pub valid: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SuiteIdentityV2 {
    pub name: String,
    pub track: String,
    pub source_url: String,
    pub source_commit: String,
    pub source_tree: String,
    pub query_tree: String,
    pub expected_output_sha256: String,
    pub license: String,
    pub classification: String,
}

#[cfg(test)]
mod v2_tests {
    use super::*;

    #[test]
    fn outcome_names_are_stable_and_exhaustive() {
        let statuses = [
            OutcomeStatus::Pass,
            OutcomeStatus::Mismatch,
            OutcomeStatus::Unsupported,
            OutcomeStatus::Unavailable,
            OutcomeStatus::Timeout,
            OutcomeStatus::Error,
            OutcomeStatus::NotApplicable,
        ];
        assert_eq!(
            serde_json::to_value(statuses).unwrap(),
            serde_json::json!([
                "pass",
                "mismatch",
                "unsupported",
                "unavailable",
                "timeout",
                "error",
                "not_applicable"
            ])
        );
        assert!(OutcomeStatus::Pass.was_executed());
        assert!(!OutcomeStatus::Unsupported.was_executed());
    }

    #[test]
    fn execution_and_timing_names_are_stable() {
        let protocol = TimingProtocolV2 {
            warmup_iterations: 2,
            measurement_iterations: 10,
            query_timeout_ms: 30_000,
            cell_timeout_ms: 3_600_000,
            query_order: QueryOrder::Rotating,
            boundary: TimingBoundary::SubmitToScalarConsumed,
            reload_before_each_iteration: false,
        };
        let value = serde_json::to_value(protocol).unwrap();
        assert_eq!(value["query_order"], "rotating");
        assert_eq!(value["boundary"], "submit-to-scalar-consumed");
        assert_eq!(
            serde_json::to_value(ExecutionClass::BackendNativeAggregate).unwrap(),
            "backend-native-aggregate"
        );
    }

    #[test]
    fn schema_v2_round_trips_a_terminal_backend_cell() {
        let observation = QueryObservationV2 {
            iteration: 1,
            query_position: 1,
            elapsed_ns: 42,
            actual_count: Some(8),
            outcome: OutcomeStatus::Pass,
            detail: None,
        };
        let report = ComparisonReportV2 {
            schema_version: COMPARISON_REPORT_SCHEMA_VERSION_V2,
            warning: "These are not LDBC Benchmark Results.".to_string(),
            experiment_id: "test".to_string(),
            suite: SuiteIdentityV2 {
                name: "LSQB compatibility".to_string(),
                track: "portable".to_string(),
                source_url: "https://github.com/ldbc/lsqb".to_string(),
                source_commit: "commit".to_string(),
                source_tree: "tree".to_string(),
                query_tree: "queries".to_string(),
                expected_output_sha256: "oracle".to_string(),
                license: "Apache-2.0".to_string(),
                classification: "unaudited microbenchmark".to_string(),
            },
            environment: ComparisonEnvironmentV2 {
                grust_revision: "revision".to_string(),
                container_os: "linux".to_string(),
                container_arch: "arm64".to_string(),
                docker_engine_version: "test".to_string(),
                cpu_model: "test".to_string(),
                cpu_limit: "4".to_string(),
                memory_limit_bytes: 8_000_000_000,
                resource_limit_scope: "per-container".to_string(),
            },
            dataset: DatasetIdentityV2 {
                scale_factor: "example".to_string(),
                model: "projected-fk".to_string(),
                source_url: "repository tree".to_string(),
                archive_sha256: None,
                archive_bytes: None,
                extracted_manifest_sha256: Some("manifest".to_string()),
                csv_files: 36,
                csv_bytes: 123,
                nodes: 28,
                edges: 72,
                person_nodes: 5,
            },
            timing: TimingProtocolV2 {
                warmup_iterations: 1,
                measurement_iterations: 1,
                query_timeout_ms: 1_000,
                cell_timeout_ms: 60_000,
                query_order: QueryOrder::Fixed,
                boundary: TimingBoundary::SubmitToScalarConsumed,
                reload_before_each_iteration: false,
            },
            backends: vec![BackendCellV2 {
                backend: BackendIdentityV2 {
                    name: "memory".to_string(),
                    adapter: "grust-memory".to_string(),
                    adapter_version: "0.13.0".to_string(),
                    runner_image: Some("runner:0.13.0".to_string()),
                    runner_image_id: Some("sha256:runner".to_string()),
                    resource_components: 1,
                    service_version: None,
                    image: None,
                    image_id: None,
                    worker_threads: Some(1),
                },
                setup_outcome: OutcomeStatus::Pass,
                setup_detail: None,
                load_ns: Some(10),
                queries: vec![QueryOutcomeV2 {
                    id: "q1".to_string(),
                    source_sha256: "source".to_string(),
                    adapter_sha256: Some("adapter".to_string()),
                    execution: ExecutionDescriptorV2 {
                        class: Some(ExecutionClass::InProcessReference),
                        language: "portable-cypher".to_string(),
                        transport: "in-process".to_string(),
                        backend_query_sha256: None,
                    },
                    expected_count: Some(8),
                    rust_rows: Some(RustRowEstimate {
                        kind: crate::queries::RustRowCardinality::Exact,
                        rows: 8,
                    }),
                    outcome: OutcomeStatus::Pass,
                    reason_code: None,
                    detail: None,
                    warmups: vec![observation.clone()],
                    measurements: vec![observation],
                }],
            }],
            complete: true,
            valid: true,
        };

        assert!(report.backends[0].queries[0].outcome.is_pass());
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(
            value["environment"]["resource_limit_scope"],
            "per-container"
        );
        assert_eq!(
            value["backends"][0]["backend"]["runner_image"],
            "runner:0.13.0"
        );
        assert_eq!(
            value["backends"][0]["backend"]["runner_image_id"],
            "sha256:runner"
        );
        assert_eq!(value["backends"][0]["backend"]["resource_components"], 1);
        let encoded = serde_json::to_string(&report).unwrap();
        let decoded: ComparisonReportV2 = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, report);
    }

    #[test]
    fn legacy_environment_defaults_to_per_container_limits() {
        let environment: ComparisonEnvironmentV2 = serde_json::from_value(serde_json::json!({
            "grust_revision": "revision",
            "container_os": "linux",
            "container_arch": "arm64",
            "docker_engine_version": "test",
            "cpu_model": "test",
            "cpu_limit": "4",
            "memory_limit_bytes": 8_000_000_000_u64
        }))
        .unwrap();
        assert_eq!(environment.resource_limit_scope, "per-container");
    }
}

#[cfg(test)]
mod v3_tests {
    use super::*;

    #[test]
    fn hard_timeout_contract_round_trips_without_changing_v2() {
        let timing = TimingProtocolV3 {
            warmup_iterations: 0,
            measurement_iterations: 1,
            query_timeout_ms: 10,
            worker_ready_timeout_ms: 100,
            query_reap_grace_ms: 5,
            query_kill_reap_timeout_ms: 20,
            query_recovery_timeout_ms: 30,
            cell_timeout_ms: 1_000,
            timeout_enforcement: TimeoutEnforcementV3::CoordinatorProcessGroup,
            query_order: QueryOrder::Rotating,
            boundary: TimingBoundaryV3::CoordinatorGoToResultConsumed,
        };
        let value = serde_json::to_value(&timing).unwrap();
        assert_eq!(value["timeout_enforcement"], "coordinator-process-group");
        assert_eq!(value["boundary"], "coordinator-go-to-result-consumed");
        assert_eq!(value["query_reap_grace_ms"], 5);
        assert!(value.get("reload_before_each_iteration").is_none());
        assert_eq!(
            serde_json::from_value::<TimingProtocolV3>(value).unwrap(),
            timing
        );
        assert_eq!(COMPARISON_REPORT_SCHEMA_VERSION_V2, 2);
        assert_eq!(COMPARISON_REPORT_SCHEMA_VERSION, 2);
        assert_eq!(COMPARISON_REPORT_SCHEMA_VERSION_V3, 3);
    }

    #[test]
    fn every_observation_discloses_setup_termination_and_recovery() {
        let observation = QueryObservationV3 {
            iteration: 1,
            query_position: 2,
            plan: Some(ExecutionPlan::ClausePipeline),
            setup_ns: 3,
            elapsed_ns: 10_000_000,
            recovery_ns: 4,
            termination: ObservationTerminationV3::DeadlineSigkill,
            actual_count: None,
            outcome: OutcomeStatus::Timeout,
            detail: Some("fixed timeout detail".to_string()),
        };
        let value = serde_json::to_value(&observation).unwrap();
        assert_eq!(value["setup_ns"], 3);
        assert_eq!(value["termination"], "deadline-sigkill");
        assert_eq!(value["recovery_ns"], 4);
        assert_eq!(
            serde_json::from_value::<QueryObservationV3>(value).unwrap(),
            observation
        );
    }
}
