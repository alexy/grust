mod dataset;
mod policy;
mod queries;
mod report;

use std::env;
use std::path::{Path, PathBuf};
use std::time::Instant;

use grust_core::{Graph, GraphAdminStore, GraphStore, Value};
use grust_cypher::pushdown::{NoTypeHints, SqlDialect, plan_read};
use grust_cypher::{CypherParameters, CypherResultTable, ReadQueryPolicy};
use grust_memory::MemoryGraphStore;
use grust_postgres_core::{PostgresGraphConfig, PostgresGraphStore, PostgresReadDialect};
use grust_turso::{TursoGraphStore, TursoReadDialect};
use queries::{LSQB_COMMIT, LSQB_EXAMPLE_DATA_TREE, LSQB_QUERY_TREE, LSQB_TREE, QueryCase};
use report::{
    Environment, GraphSize, PolicyLimits, PolicyReport, PolicyRunResult, QueryResult, Report,
    RunResult, SuiteIdentity,
};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("grust-lsqb-runner: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let arguments = Arguments::parse()?;
    let data_dir = arguments.lsqb_root.join(format!(
        "data/social-network-sf{}-projected-fk",
        arguments.scale
    ));
    let graph = dataset::load_projected_dataset(&data_dir)?;
    if arguments.suite == "policy" {
        return run_policy_suite(&arguments, &graph);
    }
    let cases = match arguments.suite.as_str() {
        "baseline" => queries::load_baseline(&arguments.lsqb_root)?,
        "adversarial" => queries::load_adversarial(&arguments.attacks_dir)?,
        other => {
            return Err(format!(
                "unknown suite {other:?}; use baseline, adversarial, or policy"
            ));
        }
    };

    let mut runs = Vec::with_capacity(arguments.runs);
    for repetition in 1..=arguments.runs {
        runs.push(run_once(&arguments.backend, repetition, &graph, &cases).await?);
    }
    let valid = runs
        .iter()
        .flat_map(|run| &run.queries)
        .all(|query| query.status == "pass");
    let report = Report {
        schema_version: 1,
        warning: "These are not LDBC Benchmark Results.",
        suite: SuiteIdentity {
            name: if arguments.suite == "baseline" {
                "GDC-maintained LSQB compatibility run".to_string()
            } else {
                "adversari.al LSQB-derived graph attacks".to_string()
            },
            track: arguments.suite.clone(),
            source_url: "https://github.com/ldbc/lsqb",
            source_commit: LSQB_COMMIT,
            source_tree: LSQB_TREE,
            query_tree: LSQB_QUERY_TREE,
            example_dataset_tree: LSQB_EXAMPLE_DATA_TREE,
            license: "Apache-2.0",
            classification: "LSQB is a GDC-maintained microbenchmark, not an official LDBC benchmark",
        },
        environment: Environment {
            grust_revision: env::var("GRUST_SOURCE_REVISION").unwrap_or_else(|_| "unknown".into()),
            backend: arguments.backend.clone(),
            scale_factor: arguments.scale,
            repetitions: arguments.runs,
            rust_version: env::var("RUST_VERSION").unwrap_or_else(|_| "unknown".into()),
            container_image: env::var("BENCHMARK_IMAGE").unwrap_or_else(|_| "unknown".into()),
            container_os: env::var("CONTAINER_OS").unwrap_or_else(|_| "linux".into()),
            container_arch: env::var("CONTAINER_ARCH").unwrap_or_else(|_| "unknown".into()),
            docker_engine_version: env::var("DOCKER_ENGINE_VERSION")
                .unwrap_or_else(|_| "unknown".into()),
            docker_cpus: env::var("DOCKER_CPUS").unwrap_or_else(|_| "unknown".into()),
            docker_memory_bytes: env::var("DOCKER_MEMORY_BYTES")
                .unwrap_or_else(|_| "unknown".into()),
            postgres_image: env::var("POSTGRES_IMAGE").unwrap_or_else(|_| "not used".into()),
            host_cpu: "intentionally omitted; Docker-reported architecture and resource allocation are authoritative for this run",
        },
        graph: GraphSize {
            nodes: graph.nodes.len(),
            edges: graph.edges.len(),
        },
        runs,
        valid,
    };

    let json = serde_json::to_string_pretty(&report).map_err(|err| err.to_string())?;
    if let Some(parent) = arguments.output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
    }
    std::fs::write(&arguments.output, format!("{json}\n"))
        .map_err(|err| format!("cannot write {}: {err}", arguments.output.display()))?;
    println!("{}", arguments.output.display());
    if valid {
        Ok(())
    } else {
        Err("one or more query counts failed validation".to_string())
    }
}

fn run_policy_suite(arguments: &Arguments, graph: &Graph) -> Result<(), String> {
    if arguments.backend != "portable-policy" {
        return Err("the policy suite uses --backend portable-policy; it is a backend-neutral preflight and execution guard".to_string());
    }
    let mut runs = Vec::with_capacity(arguments.runs);
    for repetition in 1..=arguments.runs {
        runs.push(PolicyRunResult {
            repetition,
            attacks: policy::run_policy_attacks(graph, &arguments.attacks_dir)?,
        });
    }
    let valid = runs
        .iter()
        .flat_map(|run| &run.attacks)
        .all(|attack| attack.status == "pass");
    let defaults = ReadQueryPolicy::default();
    let report = PolicyReport {
        schema_version: 1,
        warning: "These are not LDBC Benchmark Results.",
        suite: SuiteIdentity {
            name: "adversari.al bounded graph-read policy attacks".to_string(),
            track: "policy".to_string(),
            source_url: "https://github.com/ldbc/lsqb",
            source_commit: LSQB_COMMIT,
            source_tree: LSQB_TREE,
            query_tree: LSQB_QUERY_TREE,
            example_dataset_tree: LSQB_EXAMPLE_DATA_TREE,
            license: "Apache-2.0",
            classification: "adversari.al policy extension over the pinned LSQB example graph; not an LSQB track",
        },
        environment: Environment {
            grust_revision: env::var("GRUST_SOURCE_REVISION").unwrap_or_else(|_| "unknown".into()),
            backend: "portable-policy".to_string(),
            scale_factor: arguments.scale.clone(),
            repetitions: arguments.runs,
            rust_version: env::var("RUST_VERSION").unwrap_or_else(|_| "unknown".into()),
            container_image: env::var("BENCHMARK_IMAGE").unwrap_or_else(|_| "unknown".into()),
            container_os: env::var("CONTAINER_OS").unwrap_or_else(|_| "linux".into()),
            container_arch: env::var("CONTAINER_ARCH").unwrap_or_else(|_| "unknown".into()),
            docker_engine_version: env::var("DOCKER_ENGINE_VERSION")
                .unwrap_or_else(|_| "unknown".into()),
            docker_cpus: env::var("DOCKER_CPUS").unwrap_or_else(|_| "unknown".into()),
            docker_memory_bytes: env::var("DOCKER_MEMORY_BYTES")
                .unwrap_or_else(|_| "unknown".into()),
            postgres_image: env::var("POSTGRES_IMAGE").unwrap_or_else(|_| "not used".into()),
            host_cpu: "intentionally omitted; Docker-reported architecture and resource allocation are authoritative for this run",
        },
        graph: GraphSize {
            nodes: graph.nodes.len(),
            edges: graph.edges.len(),
        },
        policy: PolicyLimits {
            max_candidate_work: policy::POLICY_MAX_CANDIDATE_WORK,
            max_intermediate_bytes: defaults.max_intermediate_bytes,
            intermediate_attack_max_candidate_work: policy::INTERMEDIATE_ATTACK_MAX_CANDIDATE_WORK,
            intermediate_attack_parameter_bytes: policy::INTERMEDIATE_ATTACK_PARAMETER_BYTES,
            max_range_items: defaults.max_range_items,
            max_union_arms: defaults.max_union_arms,
            max_path_length: defaults.max_path_length,
        },
        runs,
        valid,
    };
    write_report(arguments, &report, valid)
}

fn write_report(
    arguments: &Arguments,
    report: &impl serde::Serialize,
    valid: bool,
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(report).map_err(|err| err.to_string())?;
    if let Some(parent) = arguments.output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
    }
    std::fs::write(&arguments.output, format!("{json}\n"))
        .map_err(|err| format!("cannot write {}: {err}", arguments.output.display()))?;
    println!("{}", arguments.output.display());
    if valid {
        Ok(())
    } else {
        Err("one or more cases failed validation".to_string())
    }
}

async fn run_once(
    backend: &str,
    repetition: usize,
    graph: &Graph,
    cases: &[QueryCase],
) -> Result<RunResult, String> {
    match backend {
        "memory" => run_memory(repetition, graph, cases).await,
        "turso" => run_turso(repetition, graph, cases).await,
        "postgres" => run_postgres(repetition, graph, cases).await,
        other => Err(format!(
            "unknown backend {other:?}; use memory, turso, or postgres"
        )),
    }
}

async fn run_memory(
    repetition: usize,
    graph: &Graph,
    cases: &[QueryCase],
) -> Result<RunResult, String> {
    let store = MemoryGraphStore::new();
    let started = Instant::now();
    store
        .put_graph(graph)
        .await
        .map_err(|err| err.to_string())?;
    let load_ns = started.elapsed().as_nanos();
    let snapshot = store.graph();
    let mut queries = Vec::with_capacity(cases.len());
    for case in cases {
        queries.push(measure(case, "in-memory-reference", || {
            grust_cypher::read::run_read_query(
                &snapshot,
                &case.executable,
                &CypherParameters::new(),
            )
            .map_err(|err| err.to_string())
        }));
    }
    Ok(RunResult {
        repetition,
        load_ns,
        queries,
    })
}

async fn run_turso(
    repetition: usize,
    graph: &Graph,
    cases: &[QueryCase],
) -> Result<RunResult, String> {
    let store = TursoGraphStore::in_memory()
        .await
        .map_err(|err| err.to_string())?;
    let started = Instant::now();
    store.bootstrap().await.map_err(|err| err.to_string())?;
    store
        .put_graph(graph)
        .await
        .map_err(|err| err.to_string())?;
    let load_ns = started.elapsed().as_nanos();
    let mut queries = Vec::with_capacity(cases.len());
    let dialect = TursoReadDialect::new("grust");
    for case in cases {
        let execution_mode = sql_execution_mode(case, &dialect)?;
        let started = Instant::now();
        let table = store
            .run_read_query(&case.executable, &CypherParameters::new())
            .await
            .map_err(|err| err.to_string());
        queries.push(query_result(
            case,
            &execution_mode,
            started.elapsed().as_nanos(),
            table,
        ));
    }
    Ok(RunResult {
        repetition,
        load_ns,
        queries,
    })
}

async fn run_postgres(
    repetition: usize,
    graph: &Graph,
    cases: &[QueryCase],
) -> Result<RunResult, String> {
    let connection_string = env::var("POSTGRES_URL")
        .unwrap_or_else(|_| "host=postgres user=postgres password=postgres dbname=grust".into());
    let config = PostgresGraphConfig {
        connection_string,
        table_prefix: "lsqb".to_string(),
        ..PostgresGraphConfig::default()
    };
    let dialect = PostgresReadDialect::new(&config);
    let store = PostgresGraphStore::connect(config)
        .await
        .map_err(|err| err.to_string())?;
    let started = Instant::now();
    store.bootstrap().await.map_err(|err| err.to_string())?;
    store.clear().await.map_err(|err| err.to_string())?;
    store
        .put_graph(graph)
        .await
        .map_err(|err| err.to_string())?;
    let load_ns = started.elapsed().as_nanos();
    let mut queries = Vec::with_capacity(cases.len());
    for case in cases {
        let execution_mode = sql_execution_mode(case, &dialect)?;
        let started = Instant::now();
        let table = store
            .run_read_query(&case.executable, &CypherParameters::new())
            .await
            .map_err(|err| err.to_string());
        queries.push(query_result(
            case,
            &execution_mode,
            started.elapsed().as_nanos(),
            table,
        ));
    }
    Ok(RunResult {
        repetition,
        load_ns,
        queries,
    })
}

fn measure(
    case: &QueryCase,
    execution_mode: &str,
    execute: impl FnOnce() -> Result<CypherResultTable, String>,
) -> QueryResult {
    let started = Instant::now();
    let result = execute();
    query_result(case, execution_mode, started.elapsed().as_nanos(), result)
}

fn query_result(
    case: &QueryCase,
    execution_mode: &str,
    elapsed_ns: u128,
    table: Result<CypherResultTable, String>,
) -> QueryResult {
    let (actual_count, error) = match table {
        Ok(table) => match count_from(&table) {
            Ok(count) => (Some(count), None),
            Err(error) => (None, Some(error)),
        },
        Err(error) => (None, Some(error)),
    };
    QueryResult {
        id: case.id.clone(),
        source_sha256: case.source_sha256.clone(),
        adapter_sha256: queries::sha256(case.executable.as_bytes()),
        claim: case.claim.clone(),
        execution_mode: execution_mode.to_string(),
        expected_count: case.expected_count,
        actual_count,
        elapsed_ns,
        status: if actual_count == Some(case.expected_count) {
            "pass".to_string()
        } else {
            "fail".to_string()
        },
        error,
    }
}

fn sql_execution_mode(case: &QueryCase, dialect: &dyn SqlDialect) -> Result<String, String> {
    let params = CypherParameters::new();
    let plan = plan_read(&case.executable, &params, &NoTypeHints)
        .map_err(|error| format!("cannot classify {} execution path: {error}", case.id))?;
    Ok(if plan.is_some_and(|plan| plan.supported_by(dialect)) {
        "sql-row-source-pushdown+rust-projection"
    } else {
        "in-memory-reference-fallback"
    }
    .to_string())
}

fn count_from(table: &CypherResultTable) -> Result<i64, String> {
    if table.rows.len() != 1 || table.rows[0].len() != 1 {
        return Err(format!(
            "expected one result cell, got {} columns and {} rows",
            table.columns.len(),
            table.rows.len()
        ));
    }
    match &table.rows[0][0] {
        Value::Int(value) => Ok(*value),
        value => Err(format!("expected integer count, got {value:?}")),
    }
}

struct Arguments {
    backend: String,
    suite: String,
    scale: String,
    runs: usize,
    lsqb_root: PathBuf,
    attacks_dir: PathBuf,
    output: PathBuf,
}

impl Arguments {
    fn parse() -> Result<Self, String> {
        let mut backend = "memory".to_string();
        let mut suite = "baseline".to_string();
        let mut scale = "example".to_string();
        let mut runs = 5_usize;
        let mut lsqb_root = PathBuf::from("/opt/lsqb");
        let mut attacks_dir = PathBuf::from("/opt/grust-attacks");
        let mut output = None;
        let mut args = env::args().skip(1);
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| format!("{flag} requires a value"))?;
            match flag.as_str() {
                "--backend" => backend = value,
                "--suite" => suite = value,
                "--scale" => scale = value,
                "--runs" => {
                    runs = value
                        .parse()
                        .map_err(|_| format!("invalid --runs value {value:?}"))?;
                }
                "--lsqb-root" => lsqb_root = PathBuf::from(value),
                "--attacks-dir" => attacks_dir = PathBuf::from(value),
                "--output" => output = Some(PathBuf::from(value)),
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        if runs == 0 {
            return Err("--runs must be greater than zero".to_string());
        }
        let output = output
            .unwrap_or_else(|| Path::new("out").join(format!("{suite}-{backend}-sf{scale}.json")));
        Ok(Self {
            backend,
            suite,
            scale,
            runs,
            lsqb_root,
            attacks_dir,
            output,
        })
    }
}
