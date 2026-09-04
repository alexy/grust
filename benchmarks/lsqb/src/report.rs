use serde::Serialize;

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
    pub container_os: String,
    pub container_arch: String,
    pub docker_engine_version: String,
    pub docker_cpus: String,
    pub docker_memory_bytes: String,
    pub postgres_image: String,
    pub host_cpu: &'static str,
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

#[derive(Debug, Serialize)]
pub struct PolicyLimits {
    pub max_candidate_work: usize,
    pub max_intermediate_bytes: usize,
    pub intermediate_attack_max_candidate_work: usize,
    pub intermediate_attack_parameter_bytes: usize,
    pub max_range_items: usize,
    pub max_union_arms: usize,
    pub max_path_length: u64,
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
    pub expected_rejection: String,
    pub actual_rejection: String,
    pub elapsed_ns: u128,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
