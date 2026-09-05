use std::env;
use std::future::Future;
use std::path::Path;
use std::time::{Duration, Instant};

use grust_lsqb_runner::backend::{self, PreparedBackend, QueryExecutionError};
use grust_lsqb_runner::dataset;
use grust_lsqb_runner::matrix_args::MatrixArguments;
use grust_lsqb_runner::matrix_catalog::{self, BackendCatalogEntry, QueryCapability};
use grust_lsqb_runner::matrix_progress::{self, PhaseProgress, QueryPhase};
use grust_lsqb_runner::provenance;
use grust_lsqb_runner::queries::{self, DatasetStats, QueryCase};
use grust_lsqb_runner::report::{
    self, BackendCellV2, COMPARISON_REPORT_SCHEMA_VERSION, ComparisonEnvironmentV2,
    ComparisonReportV2, ExecutionClass, ExecutionDescriptorV2, OutcomeStatus, QueryObservationV2,
    QueryOrder, QueryOutcomeV2, SuiteIdentityV2, TimingBoundary, TimingProtocolV2,
};
use grust_lsqb_runner::safe_output;

const WARNING: &str = "These are not LDBC Benchmark Results.";
const MATERIALIZATION_DISALLOWED: &str = "performance.materialization-disallowed";
const MATERIALIZATION_DISALLOWED_DETAIL: &str = "larger LSQB tiers refuse whole-backend materialization; only in-process reference, backend row-source, and backend-native aggregate paths are admitted";
const DOWNLOADED_RUST_ROW_LIMIT: i64 = 1_000_000;
const RUST_ROW_LIMIT: &str = "performance.rust-row-limit";
const RUST_ROW_LIMIT_DETAIL: &str = "downloaded LSQB tiers refuse Rust row-producing execution when the certified exact cardinality, upper bound, or lower bound exceeds the canonical 1000000-row safety limit; backend-native aggregate execution remains admitted";
const RUST_ROW_BOUND_UNAVAILABLE: &str = "performance.rust-row-bound-unavailable";
const RUST_ROW_BOUND_UNAVAILABLE_DETAIL: &str = "downloaded LSQB tiers refuse Rust row-producing execution when only a lower bound at or below the canonical 1000000-row safety limit is certified; an exact cardinality or upper bound is required for admission";

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("grust-lsqb-matrix: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let arguments = MatrixArguments::parse()?;
    let catalog = matrix_catalog::backend(&arguments.backend)?;
    let data_dir = arguments.lsqb_root.join(format!(
        "data/social-network-sf{}-projected-fk",
        arguments.scale
    ));
    let fingerprint = dataset::fingerprint_projected_dataset(&data_dir)?;
    let inspected = dataset::inspect_projected_dataset(&data_dir)?;
    let stats = DatasetStats {
        nodes: inspected.nodes,
        edges: inspected.edges,
        person_nodes: inspected.person_nodes,
    };
    let cases = match arguments.suite.as_str() {
        "baseline" => queries::load_baseline_for_scale(&arguments.lsqb_root, &arguments.scale)?,
        "adversarial" => queries::load_adversarial_for_scale(
            &arguments.attacks_dir,
            &arguments.lsqb_root,
            &arguments.scale,
            stats,
        )?,
        _ => unreachable!("MatrixArguments validates suites"),
    };
    let timing = TimingProtocolV2 {
        warmup_iterations: arguments.warmups,
        measurement_iterations: arguments.runs,
        query_timeout_ms: arguments.query_timeout_ms,
        cell_timeout_ms: arguments.cell_timeout_ms,
        query_order: QueryOrder::Rotating,
        boundary: TimingBoundary::SubmitToScalarConsumed,
        reload_before_each_iteration: false,
    };
    let dataset = provenance::lsqb_dataset_identity(&arguments.scale, stats, &fingerprint)?;
    let backend = run_backend(catalog, &arguments, &cases, &data_dir).await?;
    let valid = cell_valid(&backend);
    let report = ComparisonReportV2 {
        schema_version: COMPARISON_REPORT_SCHEMA_VERSION,
        warning: WARNING.to_string(),
        experiment_id: format!("lsqb-{}-sf{}", arguments.suite, arguments.scale),
        suite: suite_identity(&arguments.suite),
        environment: environment(),
        dataset,
        timing,
        backends: vec![backend],
        complete: false,
        valid,
    };
    write_report(&arguments.output, &report)?;
    println!("{}", arguments.output.display());
    if valid {
        Ok(())
    } else {
        Err("one or more executed supported cases did not pass".to_string())
    }
}

async fn run_backend(
    catalog: &BackendCatalogEntry,
    arguments: &MatrixArguments,
    cases: &[QueryCase],
    data_dir: &Path,
) -> Result<BackendCellV2, String> {
    let identity = backend::identity(catalog.id, catalog.adapter);
    if catalog.query_capability == QueryCapability::ExportOnly {
        return nonexecuted_cell(
            identity,
            cases,
            OutcomeStatus::NotApplicable,
            "adapter.export-only",
            "CocoIndex is a target-state export adapter, not a query backend",
            catalog,
            &arguments.scale,
        );
    }
    if !matrix_catalog::compiled(catalog.feature) {
        return nonexecuted_cell(
            identity,
            cases,
            OutcomeStatus::Unavailable,
            "runner.feature-not-compiled",
            &format!(
                "rebuild grust-lsqb-matrix with Cargo feature {}",
                catalog.feature.unwrap_or("unknown")
            ),
            catalog,
            &arguments.scale,
        );
    }
    if !admitted_at_scale(catalog.query_capability, &arguments.scale) {
        return nonexecuted_cell(
            identity,
            cases,
            OutcomeStatus::Unsupported,
            MATERIALIZATION_DISALLOWED,
            MATERIALIZATION_DISALLOWED_DETAIL,
            catalog,
            &arguments.scale,
        );
    }

    let (prepared, graph) = match prepare_backend(catalog, arguments, data_dir).await {
        Ok(prepared) => prepared,
        Err(error) => {
            if let Some(error) = error.strip_prefix("dataset.load: ") {
                return nonexecuted_cell(
                    identity,
                    cases,
                    OutcomeStatus::Error,
                    "dataset.load",
                    error,
                    catalog,
                    &arguments.scale,
                );
            }
            let (status, code) = classify_setup_failure(&error, identity.resource_components > 1);
            return nonexecuted_cell(
                identity,
                cases,
                status,
                code,
                &error,
                catalog,
                &arguments.scale,
            );
        }
    };
    let queries = execute_protocol(&prepared, &graph, cases, arguments, catalog.id).await?;
    Ok(BackendCellV2 {
        backend: identity,
        setup_outcome: OutcomeStatus::Pass,
        setup_detail: None,
        load_ns: Some(prepared.load_ns),
        queries,
    })
}

fn admitted_at_scale(capability: QueryCapability, scale: &str) -> bool {
    scale == "example" || capability != QueryCapability::MaterializeThenReference
}

async fn prepare_backend(
    catalog: &BackendCatalogEntry,
    arguments: &MatrixArguments,
    data_dir: &Path,
) -> Result<(PreparedBackend, grust_core::Graph), String> {
    if streams_projected_dataset(catalog.id, &arguments.scale) {
        let chunks = dataset::projected_dataset_chunks(data_dir, 10_000)
            .map_err(|error| format!("dataset.load: {error}"))?;
        let prepared = PreparedBackend::prepare_projected_chunks(catalog.id, chunks).await?;
        return Ok((prepared, grust_core::Graph::default()));
    }

    #[cfg(feature = "falkor")]
    if catalog.id == "falkor" && arguments.scale != "example" {
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
        // Row-source/native backends own their loaded state. Retaining a second
        // full source graph during measurement would unfairly consume the
        // runner memory limit; query-level reference fallback is rejected on
        // downloaded scales below.
        grust_core::Graph::default()
    };
    Ok((prepared, source))
}

fn streams_projected_dataset(backend: &str, scale: &str) -> bool {
    scale != "example" && matches!(backend, "memory" | "turso" | "postgres" | "sail")
}

async fn execute_protocol(
    backend: &PreparedBackend,
    graph: &grust_core::Graph,
    cases: &[QueryCase],
    arguments: &MatrixArguments,
    backend_id: &str,
) -> Result<Vec<QueryOutcomeV2>, String> {
    let mut outcomes = cases
        .iter()
        .map(|case| match backend.execution(case) {
            Ok(execution) => query_outcome_for_scale(case, execution, &arguments.scale),
            Err(error) => Ok(QueryOutcomeV2 {
                id: case.id.clone(),
                source_sha256: case.source_sha256.clone(),
                adapter_sha256: Some(queries::sha256(case.executable.as_bytes())),
                execution: ExecutionDescriptorV2 {
                    class: None,
                    language: "unknown".to_string(),
                    transport: "unknown".to_string(),
                    backend_query_sha256: None,
                },
                expected_count: Some(case.expected_count),
                rust_rows: None,
                outcome: OutcomeStatus::Error,
                reason_code: Some("query.classification".to_string()),
                detail: Some(error),
                warmups: Vec::new(),
                measurements: Vec::new(),
            }),
        })
        .collect::<Result<Vec<_>, String>>()?;

    run_phase(
        backend,
        graph,
        cases,
        &mut outcomes,
        arguments.query_timeout_ms,
        PhaseProgress::new(
            backend_id,
            &arguments.suite,
            &arguments.scale,
            QueryPhase::Warmup,
            arguments.warmups,
        ),
    )
    .await;
    run_phase(
        backend,
        graph,
        cases,
        &mut outcomes,
        arguments.query_timeout_ms,
        PhaseProgress::new(
            backend_id,
            &arguments.suite,
            &arguments.scale,
            QueryPhase::Measurement,
            arguments.runs,
        ),
    )
    .await;
    for outcome in &mut outcomes {
        if outcome.execution.class.is_some() && outcome.outcome != OutcomeStatus::Unsupported {
            finalize_query_outcome(outcome);
        }
    }
    Ok(outcomes)
}

async fn run_phase(
    backend: &PreparedBackend,
    graph: &grust_core::Graph,
    cases: &[QueryCase],
    outcomes: &mut [QueryOutcomeV2],
    timeout_ms: u64,
    progress: PhaseProgress<'_>,
) {
    if cases.is_empty() {
        return;
    }
    let query_total = u32::try_from(cases.len()).unwrap_or(u32::MAX);
    for iteration in 1..=progress.iteration_total() {
        let rotation = (iteration as usize - 1) % cases.len();
        for position in 0..cases.len() {
            let index = (position + rotation) % cases.len();
            if outcomes[index].execution.class.is_none()
                || outcomes[index].outcome == OutcomeStatus::Unsupported
            {
                continue;
            }
            let query_progress = progress.query(
                iteration,
                position as u32 + 1,
                query_total,
                &cases[index].id,
            );
            matrix_progress::query_start(query_progress);
            let (result, elapsed_ns) =
                execute_with_timeout(backend, &cases[index], graph, timeout_ms).await;
            let observation = match result {
                Ok(count) => QueryObservationV2 {
                    iteration,
                    query_position: position as u32 + 1,
                    elapsed_ns,
                    actual_count: Some(count),
                    outcome: if count == cases[index].expected_count {
                        OutcomeStatus::Pass
                    } else {
                        OutcomeStatus::Mismatch
                    },
                    detail: None,
                },
                Err(QueryExecutionError::Error(error)) => QueryObservationV2 {
                    iteration,
                    query_position: position as u32 + 1,
                    elapsed_ns,
                    actual_count: None,
                    outcome: OutcomeStatus::Error,
                    detail: Some(error),
                },
                Err(QueryExecutionError::Timeout(detail)) => QueryObservationV2 {
                    iteration,
                    query_position: position as u32 + 1,
                    elapsed_ns,
                    actual_count: None,
                    outcome: OutcomeStatus::Timeout,
                    detail: Some(detail),
                },
            };
            matrix_progress::query_finish(
                query_progress,
                observation.outcome,
                observation.elapsed_ns,
            );
            if progress.is_warmup() {
                outcomes[index].warmups.push(observation);
            } else {
                outcomes[index].measurements.push(observation);
            }
        }
    }
}

async fn execute_with_timeout(
    backend: &PreparedBackend,
    case: &QueryCase,
    graph: &grust_core::Graph,
    timeout_ms: u64,
) -> (Result<i64, QueryExecutionError>, u64) {
    let started = Instant::now();
    let result = if backend.manages_query_timeout() {
        backend.execute_count(case, graph, timeout_ms).await
    } else {
        quiescent_timeout(backend.execute_count(case, graph, timeout_ms), timeout_ms).await
    };
    // Use this same elapsed observation for both deadline classification and
    // the report. Two separately sampled timers can otherwise turn a valid
    // near-deadline result into a non-timeout observation whose recorded
    // duration exceeds the receipt validator's deadline.
    let elapsed = started.elapsed();
    finish_timed_result(result, elapsed, timeout_ms)
}

fn finish_timed_result(
    result: Result<i64, QueryExecutionError>,
    elapsed: Duration,
    timeout_ms: u64,
) -> (Result<i64, QueryExecutionError>, u64) {
    let result = if elapsed > Duration::from_millis(timeout_ms)
        && !matches!(&result, Err(QueryExecutionError::Timeout(_)))
    {
        Err(QueryExecutionError::Timeout(format!(
            "exceeded {timeout_ms} ms; backend work completed and quiesced after the deadline"
        )))
    } else {
        result
    };
    let elapsed_ns = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
    (result, elapsed_ns)
}

async fn quiescent_timeout<F>(future: F, timeout_ms: u64) -> Result<i64, QueryExecutionError>
where
    F: Future<Output = Result<i64, QueryExecutionError>>,
{
    let started = Instant::now();
    tokio::pin!(future);
    match tokio::time::timeout(Duration::from_millis(timeout_ms), future.as_mut()).await {
        Ok(result) if started.elapsed() <= Duration::from_millis(timeout_ms) => result,
        Ok(_) => Err(QueryExecutionError::Timeout(format!(
            "exceeded {timeout_ms} ms; non-yielding backend work completed and quiesced after the deadline"
        ))),
        Err(_) => {
            // Dropping an in-flight database/Spark future does not prove that
            // its server-side work stopped. Await completion before the next
            // sample so a timeout can never overlap or contaminate it.
            let _completed_after_deadline = future.await;
            Err(QueryExecutionError::Timeout(format!(
                "exceeded {timeout_ms} ms; backend work quiesced before the next sample"
            )))
        }
    }
}

fn finalize_query_outcome(outcome: &mut QueryOutcomeV2) {
    let observations = || outcome.warmups.iter().chain(outcome.measurements.iter());
    let status = if observations().any(|item| item.outcome == OutcomeStatus::Error) {
        OutcomeStatus::Error
    } else if observations().any(|item| item.outcome == OutcomeStatus::Timeout) {
        OutcomeStatus::Timeout
    } else if observations().any(|item| item.outcome == OutcomeStatus::Mismatch) {
        OutcomeStatus::Mismatch
    } else {
        OutcomeStatus::Pass
    };
    let detail = observations()
        .find(|item| item.outcome == status)
        .and_then(|item| item.detail.clone());
    outcome.outcome = status;
    outcome.reason_code = match status {
        OutcomeStatus::Error => Some("query.execution".to_string()),
        OutcomeStatus::Timeout => Some("query.timeout".to_string()),
        OutcomeStatus::Mismatch => Some("query.oracle-mismatch".to_string()),
        _ => None,
    };
    outcome.detail = detail;
}

fn query_outcome(
    case: &QueryCase,
    execution: ExecutionDescriptorV2,
    rust_rows: Option<queries::RustRowEstimate>,
) -> QueryOutcomeV2 {
    QueryOutcomeV2 {
        id: case.id.clone(),
        source_sha256: case.source_sha256.clone(),
        adapter_sha256: Some(queries::sha256(case.executable.as_bytes())),
        execution,
        expected_count: Some(case.expected_count),
        rust_rows,
        outcome: OutcomeStatus::Error,
        reason_code: None,
        detail: None,
        warmups: Vec::new(),
        measurements: Vec::new(),
    }
}

fn query_outcome_for_scale(
    case: &QueryCase,
    mut execution: ExecutionDescriptorV2,
    scale: &str,
) -> Result<QueryOutcomeV2, String> {
    let rust_rows = rust_rows_for_execution(case, &execution, scale)?;
    if scale != "example"
        && execution.class == Some(ExecutionClass::BackendMaterializeRustReference)
    {
        execution.transport = "not executed".to_string();
        return Ok(QueryOutcomeV2 {
            id: case.id.clone(),
            source_sha256: case.source_sha256.clone(),
            adapter_sha256: Some(queries::sha256(case.executable.as_bytes())),
            execution,
            expected_count: Some(case.expected_count),
            rust_rows,
            outcome: OutcomeStatus::Unsupported,
            reason_code: Some(MATERIALIZATION_DISALLOWED.to_string()),
            detail: Some(MATERIALIZATION_DISALLOWED_DETAIL.to_string()),
            warmups: Vec::new(),
            measurements: Vec::new(),
        });
    }
    let rust_row_refusal = if scale != "example"
        && matches!(
            execution.class,
            Some(
                ExecutionClass::InProcessReference | ExecutionClass::BackendRowSourceRustProjection
            )
        ) {
        let estimate = rust_rows.ok_or_else(|| {
            format!(
                "query {:?} has no Rust-row admission evidence for {:?}",
                case.id, execution.class
            )
        })?;
        rust_row_refusal(estimate)
    } else {
        None
    };
    if let Some((reason, detail)) = rust_row_refusal {
        execution.transport = "not executed".to_string();
        return Ok(QueryOutcomeV2 {
            id: case.id.clone(),
            source_sha256: case.source_sha256.clone(),
            adapter_sha256: Some(queries::sha256(case.executable.as_bytes())),
            execution,
            expected_count: Some(case.expected_count),
            rust_rows,
            outcome: OutcomeStatus::Unsupported,
            reason_code: Some(reason.to_string()),
            detail: Some(detail.to_string()),
            warmups: Vec::new(),
            measurements: Vec::new(),
        });
    }
    Ok(query_outcome(case, execution, rust_rows))
}

fn rust_row_refusal(estimate: queries::RustRowEstimate) -> Option<(&'static str, &'static str)> {
    if estimate.rows > DOWNLOADED_RUST_ROW_LIMIT {
        Some((RUST_ROW_LIMIT, RUST_ROW_LIMIT_DETAIL))
    } else if estimate.kind == queries::RustRowCardinality::LowerBound {
        Some((
            RUST_ROW_BOUND_UNAVAILABLE,
            RUST_ROW_BOUND_UNAVAILABLE_DETAIL,
        ))
    } else {
        None
    }
}

fn rust_rows_for_execution(
    case: &QueryCase,
    execution: &ExecutionDescriptorV2,
    scale: &str,
) -> Result<Option<queries::RustRowEstimate>, String> {
    let plan = match execution.class {
        Some(
            ExecutionClass::InProcessReference | ExecutionClass::BackendMaterializeRustReference,
        ) => queries::RustRowPlan::InProcess,
        Some(ExecutionClass::BackendRowSourceRustProjection) => queries::RustRowPlan::RowSource,
        Some(ExecutionClass::BackendNativeAggregate | ExecutionClass::BackendNeutralPolicy)
        | None => return Ok(None),
    };
    queries::rust_row_estimate(&case.id, scale, plan).map(Some)
}

fn nonexecuted_cell(
    identity: report::BackendIdentityV2,
    cases: &[QueryCase],
    status: OutcomeStatus,
    reason_code: &str,
    detail: &str,
    catalog: &BackendCatalogEntry,
    scale: &str,
) -> Result<BackendCellV2, String> {
    let queries = cases
        .iter()
        .map(|case| {
            let execution = catalog_execution_for_case(catalog, case)?;
            Ok(QueryOutcomeV2 {
                id: case.id.clone(),
                source_sha256: case.source_sha256.clone(),
                adapter_sha256: Some(queries::sha256(case.executable.as_bytes())),
                execution: execution.clone(),
                expected_count: Some(case.expected_count),
                rust_rows: rust_rows_for_execution(case, &execution, scale)?,
                outcome: status,
                reason_code: Some(reason_code.to_string()),
                detail: Some(detail.to_string()),
                warmups: Vec::new(),
                measurements: Vec::new(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(BackendCellV2 {
        backend: identity,
        setup_outcome: status,
        setup_detail: Some(detail.to_string()),
        load_ns: None,
        queries,
    })
}

fn catalog_execution_for_case(
    catalog: &BackendCatalogEntry,
    case: &QueryCase,
) -> Result<ExecutionDescriptorV2, String> {
    let mut execution = catalog_execution(catalog);
    if catalog.query_capability == QueryCapability::PortableQuery
        && matrix_catalog::compiled(catalog.feature)
    {
        execution.class = Some(backend::portable_execution_class(catalog.id, case)?);
    } else if catalog.query_capability == QueryCapability::PortableQuery {
        execution.class = None;
    }
    Ok(execution)
}

fn catalog_execution(catalog: &BackendCatalogEntry) -> ExecutionDescriptorV2 {
    let class = if catalog.id == "memory" {
        Some(ExecutionClass::InProcessReference)
    } else {
        match catalog.query_capability {
            QueryCapability::PortableQuery => Some(ExecutionClass::BackendRowSourceRustProjection),
            QueryCapability::NativeAggregate => Some(ExecutionClass::BackendNativeAggregate),
            QueryCapability::MaterializeThenReference => {
                Some(ExecutionClass::BackendMaterializeRustReference)
            }
            QueryCapability::ExportOnly => None,
        }
    };
    ExecutionDescriptorV2 {
        class,
        language: catalog
            .default_execution
            .unwrap_or("not applicable")
            .to_string(),
        transport: "not executed".to_string(),
        backend_query_sha256: None,
    }
}

fn classify_setup_failure(error: &str, qualified_service: bool) -> (OutcomeStatus, &'static str) {
    let lower = error.to_ascii_lowercase();
    if !qualified_service
        && [
            "connect",
            "connection",
            "refused",
            "transport",
            "dns",
            "resolve",
            "timed out",
            "failed to post",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        (OutcomeStatus::Unavailable, "backend.service-unavailable")
    } else {
        (OutcomeStatus::Error, "backend.setup")
    }
}

fn suite_identity(track: &str) -> SuiteIdentityV2 {
    SuiteIdentityV2 {
        name: if track == "baseline" {
            "GDC-maintained LSQB compatibility matrix"
        } else {
            "adversari.al LSQB-derived graph attack matrix"
        }
        .to_string(),
        track: track.to_string(),
        source_url: "https://github.com/ldbc/lsqb".to_string(),
        source_commit: queries::LSQB_COMMIT.to_string(),
        source_tree: queries::LSQB_TREE.to_string(),
        query_tree: queries::LSQB_QUERY_TREE.to_string(),
        expected_output_sha256: queries::LSQB_EXPECTED_OUTPUT_SHA256.to_string(),
        license: "Apache-2.0".to_string(),
        classification: "LSQB-derived, unaudited comparison; not an official LDBC benchmark result"
            .to_string(),
    }
}

fn environment() -> ComparisonEnvironmentV2 {
    ComparisonEnvironmentV2 {
        grust_revision: env::var("GRUST_SOURCE_REVISION").unwrap_or_else(|_| "unknown".to_string()),
        container_os: env::var("CONTAINER_OS").unwrap_or_else(|_| env::consts::OS.to_string()),
        container_arch: env::var("CONTAINER_ARCH")
            .unwrap_or_else(|_| env::consts::ARCH.to_string()),
        docker_engine_version: env::var("DOCKER_ENGINE_VERSION")
            .unwrap_or_else(|_| "not reported".to_string()),
        cpu_model: env::var("HOST_CPU_MODEL").unwrap_or_else(|_| "not reported".to_string()),
        cpu_limit: env::var("DOCKER_CPUS").unwrap_or_else(|_| "not reported".to_string()),
        memory_limit_bytes: env::var("DOCKER_MEMORY_BYTES")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
        resource_limit_scope: env::var("BENCHMARK_RESOURCE_LIMIT_SCOPE")
            .unwrap_or_else(|_| "per-container".to_string()),
    }
}

fn cell_valid(cell: &BackendCellV2) -> bool {
    cell.setup_outcome != OutcomeStatus::Error
        && cell.queries.iter().all(|query| {
            !matches!(
                query.outcome,
                OutcomeStatus::Mismatch | OutcomeStatus::Timeout | OutcomeStatus::Error
            )
        })
}

fn write_report(path: &Path, report: &ComparisonReportV2) -> Result<(), String> {
    let json = serde_json::to_string_pretty(report).map_err(|err| err.to_string())?;
    safe_output::write_new(path, format!("{json}\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[test]
    fn nonexecuted_memory_is_still_an_in_process_reference() {
        let execution = catalog_execution(matrix_catalog::backend("memory").unwrap());
        assert_eq!(execution.class, Some(ExecutionClass::InProcessReference));
    }

    #[test]
    fn downloaded_scales_exclude_only_materialize_then_reference_queries() {
        assert!(admitted_at_scale(QueryCapability::PortableQuery, "0.1"));
        assert!(admitted_at_scale(QueryCapability::NativeAggregate, "0.1"));
        assert!(admitted_at_scale(QueryCapability::ExportOnly, "0.1"));
        assert!(!admitted_at_scale(
            QueryCapability::MaterializeThenReference,
            "0.1"
        ));
        assert!(admitted_at_scale(
            QueryCapability::MaterializeThenReference,
            "example"
        ));
    }

    #[test]
    fn downloaded_owned_and_row_source_backends_stream_projected_data() {
        for backend in ["memory", "turso", "postgres", "sail"] {
            assert!(streams_projected_dataset(backend, "0.1"), "{backend}");
            assert!(streams_projected_dataset(backend, "0.3"), "{backend}");
            assert!(!streams_projected_dataset(backend, "example"), "{backend}");
        }
        for backend in ["falkor", "ladybug"] {
            assert!(!streams_projected_dataset(backend, "0.3"), "{backend}");
        }
    }

    #[test]
    fn nonexecuted_portable_cells_use_query_specific_plans() {
        let case = |id: &str, executable: &str| QueryCase {
            id: id.to_string(),
            executable: executable.to_string(),
            source_sha256: "0".repeat(64),
            expected_count: 1,
            claim: "test".to_string(),
        };
        let pushed = case("q1", "MATCH (n:Person) RETURN count(*) AS count");
        let fallback = case("q9", "MATCH (a)-[:KNOWS]->(b), (a)-[:KNOWS]-(c) RETURN a");

        for backend_id in ["turso", "postgres"] {
            let catalog = matrix_catalog::backend(backend_id).unwrap();
            assert_eq!(
                catalog_execution_for_case(catalog, &pushed).unwrap().class,
                Some(ExecutionClass::BackendRowSourceRustProjection),
                "{backend_id} should push the simple count"
            );
            assert_eq!(
                catalog_execution_for_case(catalog, &fallback)
                    .unwrap()
                    .class,
                Some(ExecutionClass::BackendMaterializeRustReference),
                "{backend_id} should disclose the unsupported-shape fallback"
            );
        }

        #[cfg(feature = "sail")]
        {
            let catalog = matrix_catalog::backend("sail").unwrap();
            assert_eq!(
                catalog_execution_for_case(catalog, &pushed).unwrap().class,
                Some(ExecutionClass::BackendRowSourceRustProjection)
            );
            assert_eq!(
                catalog_execution_for_case(catalog, &fallback)
                    .unwrap()
                    .class,
                Some(ExecutionClass::BackendMaterializeRustReference)
            );
        }
    }

    #[test]
    fn downloaded_scale_refuses_query_level_materialization_fallback() {
        let case = QueryCase {
            id: "q1".to_string(),
            executable: "RETURN count(*)".to_string(),
            source_sha256: "0".repeat(64),
            expected_count: 1,
            claim: "test".to_string(),
        };
        let execution = ExecutionDescriptorV2 {
            class: Some(ExecutionClass::BackendMaterializeRustReference),
            language: "test".to_string(),
            transport: "would materialize".to_string(),
            backend_query_sha256: None,
        };

        let downloaded = query_outcome_for_scale(&case, execution.clone(), "0.1").unwrap();
        assert_eq!(downloaded.outcome, OutcomeStatus::Unsupported);
        assert_eq!(
            downloaded.reason_code.as_deref(),
            Some(MATERIALIZATION_DISALLOWED)
        );
        assert_eq!(downloaded.execution.transport, "not executed");
        assert!(downloaded.warmups.is_empty());
        assert!(downloaded.measurements.is_empty());

        let example = query_outcome_for_scale(&case, execution, "example").unwrap();
        assert_eq!(example.outcome, OutcomeStatus::Error);
        assert_eq!(example.execution.transport, "would materialize");
    }

    #[test]
    fn downloaded_scale_bounds_rust_row_production_but_keeps_native_aggregates() {
        let cartesian = QueryCase {
            id: "a7-cartesian-count".to_string(),
            executable: "MATCH (a:Person), (b:Person), (c:Person) RETURN count(*)".to_string(),
            source_sha256: "0".repeat(64),
            expected_count: 4_913_000_000,
            claim: "test".to_string(),
        };
        let execution = |class| ExecutionDescriptorV2 {
            class: Some(class),
            language: "test".to_string(),
            transport: "would produce rows".to_string(),
            backend_query_sha256: if class == ExecutionClass::BackendNativeAggregate {
                Some("1".repeat(64))
            } else {
                None
            },
        };

        for class in [
            ExecutionClass::InProcessReference,
            ExecutionClass::BackendRowSourceRustProjection,
        ] {
            let refused = query_outcome_for_scale(&cartesian, execution(class), "0.1").unwrap();
            assert_eq!(refused.outcome, OutcomeStatus::Unsupported);
            assert_eq!(refused.reason_code.as_deref(), Some(RUST_ROW_LIMIT));
            assert_eq!(refused.detail.as_deref(), Some(RUST_ROW_LIMIT_DETAIL));
            assert_eq!(refused.execution.transport, "not executed");
            assert!(refused.warmups.is_empty());
            assert!(refused.measurements.is_empty());
        }

        let native = query_outcome_for_scale(
            &cartesian,
            execution(ExecutionClass::BackendNativeAggregate),
            "0.1",
        )
        .unwrap();
        assert_eq!(native.outcome, OutcomeStatus::Error);
        assert_eq!(native.execution.transport, "would produce rows");

        let q2 = QueryCase {
            id: "q2".to_string(),
            executable: "RETURN count(*)".to_string(),
            source_sha256: "0".repeat(64),
            expected_count: 82_990,
            claim: "test".to_string(),
        };
        assert_eq!(
            query_outcome_for_scale(&q2, execution(ExecutionClass::InProcessReference), "0.1")
                .unwrap()
                .outcome,
            OutcomeStatus::Error
        );

        let q3 = QueryCase {
            id: "q3".to_string(),
            executable: "RETURN count(*)".to_string(),
            source_sha256: "0".repeat(64),
            expected_count: 30_456,
            claim: "test".to_string(),
        };
        assert_eq!(
            query_outcome_for_scale(&q3, execution(ExecutionClass::InProcessReference), "0.1")
                .unwrap()
                .outcome,
            OutcomeStatus::Unsupported
        );
        assert_eq!(
            query_outcome_for_scale(
                &q3,
                execution(ExecutionClass::BackendRowSourceRustProjection),
                "0.1"
            )
            .unwrap()
            .outcome,
            OutcomeStatus::Error
        );

        let upper_bound = queries::RustRowEstimate {
            kind: queries::RustRowCardinality::UpperBound,
            rows: DOWNLOADED_RUST_ROW_LIMIT,
        };
        assert_eq!(rust_row_refusal(upper_bound), None);
        let lower_bound = queries::RustRowEstimate {
            kind: queries::RustRowCardinality::LowerBound,
            rows: DOWNLOADED_RUST_ROW_LIMIT - 1,
        };
        let refused = rust_row_refusal(lower_bound).unwrap();
        assert_eq!(refused.0, RUST_ROW_BOUND_UNAVAILABLE);
        assert_eq!(refused.1, RUST_ROW_BOUND_UNAVAILABLE_DETAIL);
    }

    #[test]
    fn row_admission_limit_matches_the_bundled_evidence_manifest() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../evidence-manifest-v2.json")).unwrap();
        assert_eq!(
            manifest["admission"]["downloaded_rust_row_limit"],
            DOWNLOADED_RUST_ROW_LIMIT
        );
        assert_eq!(
            manifest["admission"]["row_limit_reason_code"],
            RUST_ROW_LIMIT
        );
        assert_eq!(
            manifest["admission"]["bound_unavailable_reason_code"],
            RUST_ROW_BOUND_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn async_timeout_waits_until_backend_work_is_quiescent() {
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let started = Instant::now();
        let result = quiescent_timeout(
            async move {
                tokio::time::sleep(Duration::from_millis(30)).await;
                worker_finished.store(true, Ordering::SeqCst);
                Ok(1)
            },
            1,
        )
        .await;

        assert!(matches!(result, Err(QueryExecutionError::Timeout(_))));
        assert!(finished.load(Ordering::SeqCst));
        assert!(started.elapsed() >= Duration::from_millis(25));
    }

    #[tokio::test]
    async fn non_yielding_completion_after_deadline_is_timeout() {
        let result = quiescent_timeout(
            async {
                std::thread::sleep(Duration::from_millis(20));
                Ok(1)
            },
            1,
        )
        .await;

        assert!(matches!(result, Err(QueryExecutionError::Timeout(_))));
    }

    #[test]
    fn late_completion_and_reported_elapsed_share_one_deadline_observation() {
        let (result, elapsed_ns) = finish_timed_result(Ok(1), Duration::from_millis(2), 1);

        assert!(matches!(result, Err(QueryExecutionError::Timeout(_))));
        assert_eq!(elapsed_ns, 2_000_000);
    }

    #[test]
    fn warmup_failures_are_part_of_the_reduced_query_outcome() {
        let case = QueryCase {
            id: "q1".to_string(),
            executable: "RETURN count(*)".to_string(),
            source_sha256: "0".repeat(64),
            expected_count: 1,
            claim: "test".to_string(),
        };
        let execution = ExecutionDescriptorV2 {
            class: Some(ExecutionClass::InProcessReference),
            language: "test".to_string(),
            transport: "test".to_string(),
            backend_query_sha256: None,
        };
        let mut outcome = query_outcome(
            &case,
            execution,
            Some(queries::RustRowEstimate {
                kind: queries::RustRowCardinality::Exact,
                rows: 1,
            }),
        );
        outcome.warmups.push(QueryObservationV2 {
            iteration: 1,
            query_position: 1,
            elapsed_ns: 1,
            actual_count: None,
            outcome: OutcomeStatus::Error,
            detail: Some("warmup failed".to_string()),
        });
        outcome.measurements.push(QueryObservationV2 {
            iteration: 1,
            query_position: 1,
            elapsed_ns: 1,
            actual_count: Some(1),
            outcome: OutcomeStatus::Pass,
            detail: None,
        });

        finalize_query_outcome(&mut outcome);

        assert_eq!(outcome.outcome, OutcomeStatus::Error);
        assert_eq!(outcome.reason_code.as_deref(), Some("query.execution"));
        assert_eq!(outcome.detail.as_deref(), Some("warmup failed"));
    }

    #[test]
    fn only_an_unconfigured_external_transport_failure_is_unavailable() {
        assert_eq!(
            classify_setup_failure("backend error: failed to POST Helix query", false),
            (OutcomeStatus::Unavailable, "backend.service-unavailable")
        );
        assert_eq!(
            classify_setup_failure("backend error: failed to POST Helix query", true),
            (OutcomeStatus::Error, "backend.setup")
        );
        assert_eq!(
            classify_setup_failure("backend error: Helix query failed with status 500", false),
            (OutcomeStatus::Error, "backend.setup")
        );
    }
}
