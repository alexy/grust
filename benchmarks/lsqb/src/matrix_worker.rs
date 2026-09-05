//! Internal one-observation matrix worker.
//!
//! This protocol is intentionally private to the matrix coordinator. Worker
//! stdout is reserved for bounded READY/result records and worker failures are
//! reported to the coordinator only as fixed infrastructure errors.

use std::env;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::backend::{self, PreparedBackend, QueryExecutionError};
use crate::dataset;
use crate::matrix_args::MatrixArguments;
use crate::matrix_catalog::{BackendCatalogEntry, QueryCapability};
use crate::observation_process::{self, WorkerOutcome};
use crate::queries::{self, QueryCase};
use crate::report::LoadStrategyV3;

pub const WORKER_MARKER: &str = "--internal-observation-worker";
const ENV_BACKEND: &str = "GRUST_LSQB_WORKER_BACKEND";
const ENV_SUITE: &str = "GRUST_LSQB_WORKER_SUITE";
const ENV_SCALE: &str = "GRUST_LSQB_WORKER_SCALE";
const ENV_QUERY_ID: &str = "GRUST_LSQB_WORKER_QUERY_ID";
const ENV_LSQB_ROOT: &str = "GRUST_LSQB_WORKER_LSQB_ROOT";
const ENV_ATTACKS_DIR: &str = "GRUST_LSQB_WORKER_ATTACKS_DIR";
const ENV_TOKEN: &str = "GRUST_LSQB_WORKER_TOKEN";
const ENV_TIMEOUT: &str = "GRUST_LSQB_WORKER_QUERY_TIMEOUT_MS";
const ENV_ATTACH: &str = "GRUST_LSQB_WORKER_ATTACH";

pub async fn run_internal() -> Result<(), String> {
    let arguments = WorkerArguments::from_environment()?;
    let catalog = crate::matrix_catalog::backend(&arguments.backend)?;
    if !crate::matrix_catalog::compiled(catalog.feature) {
        return Err("worker backend is not compiled".to_string());
    }
    let cases = load_cases(&arguments)?;
    let case = cases
        .iter()
        .find(|case| case.id == arguments.query_id)
        .ok_or_else(|| "worker query identity is not in the selected suite".to_string())?;
    let data_dir = arguments.lsqb_root.join(format!(
        "data/social-network-sf{}-projected-fk",
        arguments.scale
    ));
    let setup_started = Instant::now();
    let (backend, source) = if arguments.attach {
        attach_for_observation(catalog, &arguments.scale, &data_dir).await?
    } else {
        prepare_for_matrix(catalog, &arguments.scale, &data_dir).await?
    };
    backend
        .execution(case)
        .map_err(|_| "worker could not classify its query".to_string())?;
    let setup_ns = duration_ns(setup_started.elapsed());

    let stdout = std::io::stdout();
    let mut output = BufWriter::new(stdout.lock());
    observation_process::write_ready(&mut output, &arguments.token, setup_ns)?;
    let stdin = std::io::stdin();
    let go_nonce =
        observation_process::read_go(&mut BufReader::new(stdin.lock()), &arguments.token)?;

    let started = Instant::now();
    let backend_timeout_ms =
        backend::worker_query_timeout_ms(&arguments.backend, arguments.query_timeout_ms);
    let result = backend
        .execute_count(case, &source, backend_timeout_ms)
        .await;
    let elapsed_ns = duration_ns(started.elapsed());
    let (outcome, count) = match result {
        Ok(count) => (WorkerOutcome::Pass, Some(count)),
        Err(QueryExecutionError::Timeout(_)) => (WorkerOutcome::Timeout, None),
        Err(QueryExecutionError::Error(error))
            if looks_like_acknowledged_backend_timeout(&arguments.backend, &error) =>
        {
            (WorkerOutcome::Timeout, None)
        }
        Err(QueryExecutionError::Error(_)) => (WorkerOutcome::Error, None),
    };
    observation_process::write_result(
        &mut output,
        &arguments.token,
        &go_nonce,
        outcome,
        count,
        elapsed_ns,
    )
}

pub fn command(
    arguments: &MatrixArguments,
    query_id: &str,
    token: &str,
    attach: bool,
) -> Result<Command, String> {
    let executable = env::current_exe()
        .map_err(|_| "failed to resolve the matrix worker executable".to_string())?;
    let mut command = Command::new(executable);
    command
        .arg(WORKER_MARKER)
        .env(ENV_BACKEND, &arguments.backend)
        .env(ENV_SUITE, &arguments.suite)
        .env(ENV_SCALE, &arguments.scale)
        .env(ENV_QUERY_ID, query_id)
        .env(ENV_LSQB_ROOT, &arguments.lsqb_root)
        .env(ENV_ATTACKS_DIR, &arguments.attacks_dir)
        .env(ENV_TOKEN, token)
        .env(ENV_TIMEOUT, arguments.query_timeout_ms.to_string())
        .env(ENV_ATTACH, if attach { "1" } else { "0" });
    Ok(command)
}

pub async fn prepare_for_matrix(
    catalog: &BackendCatalogEntry,
    scale: &str,
    data_dir: &Path,
) -> Result<(PreparedBackend, grust_core::Graph), String> {
    if streams_projected_dataset(catalog.id, scale) {
        let chunks = dataset::projected_dataset_chunks(data_dir, 10_000)
            .map_err(|error| format!("dataset.load: {error}"))?;
        let prepared = PreparedBackend::prepare_projected_chunks(catalog.id, chunks).await?;
        return Ok((prepared, grust_core::Graph::default()));
    }

    #[cfg(feature = "falkor")]
    if catalog.id == "falkor" && scale != "example" {
        let chunks = dataset::projected_dataset_chunks(data_dir, 10_000)
            .map_err(|error| format!("dataset.load: {error}"))?;
        let prepared = backend::prepare_falkor_chunks(chunks).await?;
        return Ok((prepared, grust_core::Graph::default()));
    }

    let graph = dataset::load_projected_dataset(data_dir)
        .map_err(|error| format!("dataset.load: {error}"))?;
    let prepared = PreparedBackend::prepare(catalog.id, &graph).await?;
    let source = if catalog.query_capability == QueryCapability::MaterializeThenReference {
        graph
    } else {
        grust_core::Graph::default()
    };
    Ok((prepared, source))
}

async fn attach_for_observation(
    catalog: &BackendCatalogEntry,
    scale: &str,
    data_dir: &Path,
) -> Result<(PreparedBackend, grust_core::Graph), String> {
    let source = if scale == "example"
        && catalog.query_capability == QueryCapability::MaterializeThenReference
    {
        dataset::load_projected_dataset(data_dir)
            .map_err(|error| format!("dataset.load: {error}"))?
    } else {
        grust_core::Graph::default()
    };
    let prepared = PreparedBackend::attach_existing(catalog.id, &source).await?;
    Ok((prepared, source))
}

pub fn uses_attach_worker(backend: &str) -> bool {
    backend::load_strategy(backend, true) == LoadStrategyV3::OnceWorkerAttach
}

pub fn streams_projected_dataset(backend: &str, scale: &str) -> bool {
    scale != "example" && matches!(backend, "memory" | "turso" | "postgres" | "sail")
}

fn load_cases(arguments: &WorkerArguments) -> Result<Vec<QueryCase>, String> {
    match arguments.suite.as_str() {
        "baseline" => queries::load_baseline_for_scale(&arguments.lsqb_root, &arguments.scale),
        "adversarial" => {
            let data_dir = arguments.lsqb_root.join(format!(
                "data/social-network-sf{}-projected-fk",
                arguments.scale
            ));
            let inspected = dataset::inspect_projected_dataset(&data_dir)?;
            queries::load_adversarial_for_scale(
                &arguments.attacks_dir,
                &arguments.lsqb_root,
                &arguments.scale,
                queries::DatasetStats {
                    nodes: inspected.nodes,
                    edges: inspected.edges,
                    person_nodes: inspected.person_nodes,
                },
            )
        }
        _ => Err("worker suite is invalid".to_string()),
    }
}

#[derive(Debug)]
struct WorkerArguments {
    backend: String,
    suite: String,
    scale: String,
    query_id: String,
    lsqb_root: PathBuf,
    attacks_dir: PathBuf,
    token: String,
    query_timeout_ms: u64,
    attach: bool,
}

impl WorkerArguments {
    fn from_environment() -> Result<Self, String> {
        let value =
            |name: &str| env::var(name).map_err(|_| "worker environment is incomplete".to_string());
        let backend = value(ENV_BACKEND)?;
        let suite = value(ENV_SUITE)?;
        let scale = value(ENV_SCALE)?;
        let query_id = value(ENV_QUERY_ID)?;
        let token = value(ENV_TOKEN)?;
        let query_timeout_ms = value(ENV_TIMEOUT)?
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| "worker timeout is invalid".to_string())?;
        let attach = match value(ENV_ATTACH)?.as_str() {
            "0" => false,
            "1" => true,
            _ => return Err("worker attach mode is invalid".to_string()),
        };
        if !matches!(suite.as_str(), "baseline" | "adversarial")
            || !safe_identifier(&backend)
            || !safe_identifier(&scale)
            || !safe_identifier(&query_id)
            || !safe_identifier(&token)
        {
            return Err("worker identity is invalid".to_string());
        }
        Ok(Self {
            backend,
            suite,
            scale,
            query_id,
            lsqb_root: PathBuf::from(
                env::var_os(ENV_LSQB_ROOT)
                    .ok_or_else(|| "worker environment is incomplete".to_string())?,
            ),
            attacks_dir: PathBuf::from(
                env::var_os(ENV_ATTACKS_DIR)
                    .ok_or_else(|| "worker environment is incomplete".to_string())?,
            ),
            token,
            query_timeout_ms,
            attach,
        })
    }
}

fn safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn looks_like_acknowledged_backend_timeout(backend: &str, error: &str) -> bool {
    if !matches!(backend, "postgres" | "pggraph" | "postgres-pgq") {
        return false;
    }
    let lower = error.to_ascii_lowercase();
    lower.contains("statement timeout")
        || lower.contains("query timeout")
        || lower.contains("query timed out")
        || lower.contains("canceling statement due to statement timeout")
}

fn duration_ns(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_identities_are_strict_and_secret_agnostic() {
        assert!(safe_identifier("g123-measurement-1-2"));
        assert!(!safe_identifier("postgres://user:secret@host/db"));
        assert!(!safe_identifier("q1\nforged"));
    }

    #[test]
    fn only_process_owned_or_session_scoped_backends_reload() {
        for backend in ["memory", "turso", "ladybug", "lancedb", "sail"] {
            assert!(!uses_attach_worker(backend), "{backend}");
        }
        for backend in [
            "postgres",
            "falkor",
            "surreal",
            "pggraph",
            "postgres-pgq",
            "helix",
        ] {
            assert!(uses_attach_worker(backend), "{backend}");
        }
    }

    #[test]
    fn timeout_classification_is_narrow() {
        assert!(looks_like_acknowledged_backend_timeout(
            "postgres",
            "canceling statement due to statement timeout"
        ));
        assert!(!looks_like_acknowledged_backend_timeout(
            "helix",
            "query timeout"
        ));
        assert!(!looks_like_acknowledged_backend_timeout(
            "postgres",
            "connection refused"
        ));
    }
}
