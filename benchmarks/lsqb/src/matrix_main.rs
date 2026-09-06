use std::env;
use std::path::Path;
use std::time::Instant;

use grust_lsqb_runner::backend;
use grust_lsqb_runner::dataset;
use grust_lsqb_runner::matrix_args::MatrixArguments;
use grust_lsqb_runner::matrix_catalog::{self, BackendCatalogEntry, QueryCapability};
use grust_lsqb_runner::matrix_progress::{self, PhaseProgress, QueryPhase};
use grust_lsqb_runner::matrix_worker;
use grust_lsqb_runner::observation_process::{self, WorkerOutcome};
use grust_lsqb_runner::provenance;
use grust_lsqb_runner::queries::{self, DatasetStats, QueryCase};
use grust_lsqb_runner::report::{
    self, BackendCellV3, BackendLifecycleV3, COMPARISON_REPORT_SCHEMA_VERSION_V3,
    CellTerminationV3, ComparisonEnvironmentV2, ComparisonReportV3, ExecutionClass,
    ExecutionDescriptorV2, LoadStrategyV3, ObservationTerminationV3, OutcomeStatus,
    QueryObservationV3, QueryOrder, QueryOutcomeV3, SuiteIdentityV2, TimeoutEnforcementV3,
    TimingBoundaryV3, TimingProtocolV3,
};
use grust_lsqb_runner::safe_output;
use grust_lsqb_runner::sample_schedule::rotated_index;

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
    let worker = env::args().nth(1).as_deref() == Some(matrix_worker::WORKER_MARKER);
    let result = if worker {
        matrix_worker::run_internal().await
    } else {
        run().await
    };
    if let Err(error) = result {
        if worker {
            eprintln!("grust-lsqb-matrix: internal observation worker failed");
        } else {
            eprintln!("grust-lsqb-matrix: {error}");
        }
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
    let timing = TimingProtocolV3 {
        warmup_iterations: arguments.warmups,
        measurement_iterations: arguments.runs,
        query_timeout_ms: arguments.query_timeout_ms,
        worker_ready_timeout_ms: arguments.worker_ready_timeout_ms,
        query_reap_grace_ms: arguments.query_reap_grace_ms,
        query_kill_reap_timeout_ms: arguments.query_kill_reap_timeout_ms,
        query_recovery_timeout_ms: arguments.query_recovery_timeout_ms,
        cell_timeout_ms: arguments.cell_timeout_ms,
        timeout_enforcement: TimeoutEnforcementV3::CoordinatorProcessGroup,
        query_order: QueryOrder::Rotating,
        boundary: TimingBoundaryV3::CoordinatorGoToResultConsumed,
    };
    let dataset = provenance::lsqb_dataset_identity(&arguments.scale, stats, &fingerprint)?;
    let backend = run_backend(catalog, &arguments, &cases, &data_dir).await?;
    let valid = cell_valid(&backend);
    let report = ComparisonReportV3 {
        schema_version: COMPARISON_REPORT_SCHEMA_VERSION_V3,
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
) -> Result<BackendCellV3, String> {
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

    let (prepared, graph) =
        match matrix_worker::prepare_for_matrix(catalog, &arguments.scale, data_dir).await {
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
                let (status, code) =
                    classify_setup_failure(&error, identity.resource_components > 1);
                let detail = if status == OutcomeStatus::Unavailable {
                    "backend service is unavailable"
                } else {
                    "backend setup failed"
                };
                return nonexecuted_cell(
                    identity,
                    cases,
                    status,
                    code,
                    detail,
                    catalog,
                    &arguments.scale,
                );
            }
        };
    let load_ns = prepared.load_ns;
    let (queries, terminated) =
        execute_protocol(prepared, graph, cases, arguments, catalog.id).await?;
    Ok(BackendCellV3 {
        backend: identity,
        lifecycle: BackendLifecycleV3 {
            load_strategy: backend::load_strategy(catalog.id, true),
            recovery_contract: backend::recovery_contract(catalog.id, true),
            terminated,
        },
        setup_outcome: OutcomeStatus::Pass,
        setup_detail: None,
        load_ns: Some(load_ns),
        queries,
    })
}

fn admitted_at_scale(capability: QueryCapability, scale: &str) -> bool {
    scale == "example" || capability != QueryCapability::MaterializeThenReference
}

async fn execute_protocol(
    backend: backend::PreparedBackend,
    graph: grust_core::Graph,
    cases: &[QueryCase],
    arguments: &MatrixArguments,
    backend_id: &str,
) -> Result<(Vec<QueryOutcomeV3>, Option<CellTerminationV3>), String> {
    let outcomes = cases
        .iter()
        .map(|case| match backend.execution(case) {
            Ok(execution) => query_outcome_for_scale(case, execution, &arguments.scale),
            Err(error) => Ok(QueryOutcomeV3 {
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
        .collect::<Result<Vec<_>, String>>();
    let mut outcomes = match outcomes {
        Ok(outcomes) => outcomes,
        Err(error) => {
            backend.finish().await?;
            return Err(error);
        }
    };
    // No worker inherits Rust state from this Tokio process. Persistent
    // service backends retain their loaded state and workers reconnect;
    // process-owned backends reload explicitly before each READY record.
    let coordinator = if backend_id == "sail" {
        Some(backend)
    } else {
        backend.finish().await?;
        None
    };
    drop(graph);

    let execution = async {
        let terminated = run_phase(
            cases,
            &mut outcomes,
            arguments,
            coordinator.as_ref(),
            PhaseProgress::new(
                backend_id,
                &arguments.suite,
                &arguments.scale,
                QueryPhase::Warmup,
                arguments.warmups,
            ),
        )
        .await?;
        if terminated.is_some() {
            // The backend's state is unproven: no further observation may be
            // taken in this cell, in this phase or the next.
            return Ok(terminated);
        }
        run_phase(
            cases,
            &mut outcomes,
            arguments,
            coordinator.as_ref(),
            PhaseProgress::new(
                backend_id,
                &arguments.suite,
                &arguments.scale,
                QueryPhase::Measurement,
                arguments.runs,
            ),
        )
        .await
    }
    .await;
    let cleanup = match coordinator {
        Some(owner) => owner.finish().await,
        None => Ok(()),
    };
    let terminated = execution?;
    cleanup?;
    for outcome in &mut outcomes {
        if outcome.execution.class.is_some() && outcome.outcome != OutcomeStatus::Unsupported {
            finalize_query_outcome(outcome);
        }
    }
    if let Some(termination) = &terminated {
        finalize_terminated_cell(
            &mut outcomes,
            termination,
            arguments.warmups,
            arguments.runs,
        );
    }
    Ok((outcomes, terminated))
}

async fn run_phase(
    cases: &[QueryCase],
    outcomes: &mut [QueryOutcomeV3],
    arguments: &MatrixArguments,
    coordinator: Option<&backend::PreparedBackend>,
    progress: PhaseProgress<'_>,
) -> Result<Option<CellTerminationV3>, String> {
    if cases.is_empty() {
        return Ok(None);
    }
    let query_total = u32::try_from(cases.len()).unwrap_or(u32::MAX);
    for iteration in 1..=progress.iteration_total() {
        let rotation = (iteration as usize - 1) % cases.len();
        for position in 0..cases.len() {
            let index = rotated_index(position, rotation, cases.len());
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
            let token = format!(
                "g{}-{}-{iteration}-{}",
                std::process::id(),
                if progress.is_warmup() { "w" } else { "m" },
                position + 1
            );
            let mut command = matrix_worker::command(
                arguments,
                &cases[index].id,
                &token,
                matrix_worker::uses_attach_worker(&arguments.backend),
            )?;
            if let Some(owner) = coordinator {
                owner.configure_worker(&mut command);
            }
            let mut isolated = observation_process::run_with_ready(
                &mut command,
                &token,
                arguments.query_timeout_ms,
                arguments.query_reap_grace_ms,
                arguments.query_kill_reap_timeout_ms,
                arguments.worker_ready_timeout_ms,
                |setup_ns| matrix_progress::query_ready(query_progress, setup_ns),
            )?;
            if requires_backend_recovery(isolated.outcome, isolated.termination) {
                let recovery_started = Instant::now();
                let recovered = backend::recover_after_unacknowledged_exit(
                    &arguments.backend,
                    &token,
                    arguments.query_recovery_timeout_ms,
                )
                .await;
                isolated.recovery_ns = isolated
                    .recovery_ns
                    .saturating_add(elapsed_ns(recovery_started));
                if recovered.is_err() {
                    // Fail closed, but as evidence: record this observation as
                    // the cell's terminal error and stop taking observations.
                    // The component report is still written so the matrix
                    // continues and the receipt shows the failure.
                    let detail = format!(
                        "backend {} could not prove quiescence after an unacknowledged query exit; no further observation was taken in this cell",
                        arguments.backend
                    );
                    let observation = QueryObservationV3 {
                        iteration,
                        query_position: position as u32 + 1,
                        plan: isolated.plan,
                        setup_ns: isolated.setup_ns,
                        elapsed_ns: isolated.elapsed_ns,
                        recovery_ns: isolated.recovery_ns,
                        termination: isolated.termination,
                        actual_count: None,
                        outcome: OutcomeStatus::Error,
                        detail: Some(detail.clone()),
                    };
                    grust_lsqb_runner::observation_journal::record(
                        arguments,
                        &cases[index].id,
                        progress.is_warmup(),
                        &observation,
                    )?;
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
                    eprintln!(
                        "grust-lsqb-matrix: {detail}; the cell ends as an explicit error result"
                    );
                    return Ok(Some(CellTerminationV3 {
                        query_id: cases[index].id.clone(),
                        phase: if progress.is_warmup() {
                            "warmup"
                        } else {
                            "measurement"
                        }
                        .to_string(),
                        iteration,
                        reason_code: report::QUIESCENCE_UNPROVEN.to_string(),
                        detail,
                    }));
                }
            }
            let plan = isolated.require_declared_plan()?;
            plan.validate_execution(&outcomes[index].execution, outcomes[index].rust_rows)?;
            let observation = match isolated.outcome {
                WorkerOutcome::Pass => {
                    let count = isolated.actual_count.ok_or_else(|| {
                        "observation worker omitted a successful scalar result".to_string()
                    })?;
                    QueryObservationV3 {
                        iteration,
                        query_position: position as u32 + 1,
                        plan: Some(plan),
                        setup_ns: isolated.setup_ns,
                        elapsed_ns: isolated.elapsed_ns,
                        recovery_ns: isolated.recovery_ns,
                        termination: isolated.termination,
                        actual_count: Some(count),
                        outcome: if count == cases[index].expected_count {
                            OutcomeStatus::Pass
                        } else {
                            OutcomeStatus::Mismatch
                        },
                        detail: None,
                    }
                }
                WorkerOutcome::Error => QueryObservationV3 {
                    iteration,
                    query_position: position as u32 + 1,
                    plan: Some(plan),
                    setup_ns: isolated.setup_ns,
                    elapsed_ns: isolated.elapsed_ns,
                    recovery_ns: isolated.recovery_ns,
                    termination: isolated.termination,
                    actual_count: None,
                    outcome: OutcomeStatus::Error,
                    detail: Some("observation worker reported a query execution error".to_string()),
                },
                WorkerOutcome::Timeout => QueryObservationV3 {
                    iteration,
                    query_position: position as u32 + 1,
                    plan: Some(plan),
                    setup_ns: isolated.setup_ns,
                    elapsed_ns: isolated.elapsed_ns,
                    recovery_ns: isolated.recovery_ns,
                    termination: isolated.termination,
                    actual_count: None,
                    outcome: OutcomeStatus::Timeout,
                    detail: Some(
                        "query deadline enforced and quiescence proved before the next observation"
                            .to_string(),
                    ),
                },
            };
            grust_lsqb_runner::observation_journal::record(
                arguments,
                &cases[index].id,
                progress.is_warmup(),
                &observation,
            )?;
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
    Ok(None)
}

/// After a declared termination, every admitted query short of the sampling
/// contract is an explicit error carrying the termination's reason code; the
/// terminating query keeps the termination detail, the others say they were
/// not observed. Queries that completed both phases keep their own outcome.
fn finalize_terminated_cell(
    outcomes: &mut [QueryOutcomeV3],
    termination: &CellTerminationV3,
    warmups: u32,
    runs: u32,
) {
    for outcome in outcomes.iter_mut() {
        if outcome.execution.class.is_none() || outcome.outcome == OutcomeStatus::Unsupported {
            continue;
        }
        let short =
            outcome.warmups.len() < warmups as usize || outcome.measurements.len() < runs as usize;
        if !short {
            continue;
        }
        outcome.outcome = OutcomeStatus::Error;
        outcome.reason_code = Some(termination.reason_code.clone());
        outcome.detail = Some(if outcome.id == termination.query_id {
            termination.detail.clone()
        } else {
            format!(
                "not observed to the sampling contract: the cell terminated at {} {} iteration {} ({})",
                termination.query_id,
                termination.phase,
                termination.iteration,
                termination.reason_code
            )
        });
    }
}

fn requires_backend_recovery(
    outcome: WorkerOutcome,
    termination: ObservationTerminationV3,
) -> bool {
    outcome == WorkerOutcome::Error
        || matches!(
            termination,
            ObservationTerminationV3::DeadlineObservedExit
                | ObservationTerminationV3::DeadlineSigterm
                | ObservationTerminationV3::DeadlineSigkill
        )
}

fn elapsed_ns(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn finalize_query_outcome(outcome: &mut QueryOutcomeV3) {
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
) -> QueryOutcomeV3 {
    QueryOutcomeV3 {
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
) -> Result<QueryOutcomeV3, String> {
    let rust_rows = rust_rows_for_execution(case, &execution, scale)?;
    if scale != "example"
        && execution.class == Some(ExecutionClass::BackendMaterializeRustReference)
    {
        execution.transport = "not executed".to_string();
        return Ok(QueryOutcomeV3 {
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
        return Ok(QueryOutcomeV3 {
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
    if matches!(
        execution.class,
        Some(ExecutionClass::InProcessReference | ExecutionClass::BackendResidentIndexRustCount)
    ) && backend::memory_execution_plan(case)? == report::ExecutionPlan::CountFactorized
    {
        return Ok(Some(queries::RustRowEstimate {
            kind: queries::RustRowCardinality::NotMaterialized,
            rows: 0,
        }));
    }
    let plan = match execution.class {
        Some(
            ExecutionClass::InProcessReference
            | ExecutionClass::BackendMaterializeRustReference
            | ExecutionClass::BackendResidentIndexRustCount,
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
) -> Result<BackendCellV3, String> {
    let queries = cases
        .iter()
        .map(|case| {
            let execution = catalog_execution_for_case(catalog, case)?;
            Ok(QueryOutcomeV3 {
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
    Ok(BackendCellV3 {
        backend: identity,
        lifecycle: BackendLifecycleV3 {
            load_strategy: LoadStrategyV3::NotExecuted,
            recovery_contract: backend::recovery_contract(catalog.id, false),
            terminated: None,
        },
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
        execution.backend_query_sha256 =
            backend::scalar_sql_query(catalog.id, case)?.map(|sql| queries::sha256(sql.as_bytes()));
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

fn cell_valid(cell: &BackendCellV3) -> bool {
    cell.setup_outcome != OutcomeStatus::Error
        && cell.queries.iter().all(|query| {
            !matches!(
                query.outcome,
                OutcomeStatus::Mismatch | OutcomeStatus::Timeout | OutcomeStatus::Error
            )
        })
}

fn write_report(path: &Path, report: &ComparisonReportV3) -> Result<(), String> {
    let json = serde_json::to_string_pretty(report).map_err(|err| err.to_string())?;
    safe_output::write_new(path, format!("{json}\n").as_bytes())
}

#[cfg(test)]
mod tests {
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
            assert!(
                matrix_worker::streams_projected_dataset(backend, "0.1"),
                "{backend}"
            );
            assert!(
                matrix_worker::streams_projected_dataset(backend, "0.3"),
                "{backend}"
            );
            assert!(
                !matrix_worker::streams_projected_dataset(backend, "example"),
                "{backend}"
            );
        }
        for backend in ["falkor", "ladybug"] {
            assert!(
                !matrix_worker::streams_projected_dataset(backend, "0.3"),
                "{backend}"
            );
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
                Some(ExecutionClass::BackendNativeAggregate),
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
    fn terminated_cell_marks_every_short_query_as_an_explicit_error() {
        let case = |id: &str| QueryCase {
            id: id.to_string(),
            executable: "MATCH (n) RETURN count(*)".to_string(),
            source_sha256: "0".repeat(64),
            expected_count: 1,
            claim: "test".to_string(),
        };
        let execution = ExecutionDescriptorV2 {
            class: Some(ExecutionClass::BackendNativeAggregate),
            language: "test".to_string(),
            transport: "test".to_string(),
            backend_query_sha256: Some("1".repeat(64)),
        };
        let observation = |iteration: u32, outcome: OutcomeStatus| QueryObservationV3 {
            iteration,
            query_position: 1,
            plan: None,
            setup_ns: 1,
            elapsed_ns: 1,
            recovery_ns: 1,
            termination: ObservationTerminationV3::NormalExit,
            actual_count: (outcome == OutcomeStatus::Pass).then_some(1),
            outcome,
            detail: (outcome != OutcomeStatus::Pass).then(|| "x".to_string()),
        };
        let mut complete = query_outcome(&case("q1"), execution.clone(), None);
        complete.warmups = vec![observation(1, OutcomeStatus::Pass)];
        complete.measurements = vec![observation(1, OutcomeStatus::Pass)];
        let mut terminating = query_outcome(&case("q2"), execution.clone(), None);
        terminating.warmups = vec![observation(1, OutcomeStatus::Error)];
        let mut unobserved = query_outcome(&case("q3"), execution.clone(), None);
        unobserved.warmups = vec![observation(1, OutcomeStatus::Pass)];
        let mut unsupported = query_outcome(&case("q4"), execution, None);
        unsupported.outcome = OutcomeStatus::Unsupported;
        let mut outcomes = vec![complete, terminating, unobserved, unsupported];
        for outcome in &mut outcomes[..3] {
            finalize_query_outcome(outcome);
        }
        let termination = CellTerminationV3 {
            query_id: "q2".to_string(),
            phase: "warmup".to_string(),
            iteration: 1,
            reason_code: report::QUIESCENCE_UNPROVEN.to_string(),
            detail: "backend could not prove quiescence".to_string(),
        };
        finalize_terminated_cell(&mut outcomes, &termination, 1, 1);
        assert_eq!(outcomes[0].outcome, OutcomeStatus::Pass);
        assert_eq!(outcomes[0].reason_code, None);
        assert_eq!(outcomes[1].outcome, OutcomeStatus::Error);
        assert_eq!(
            outcomes[1].reason_code.as_deref(),
            Some(report::QUIESCENCE_UNPROVEN)
        );
        assert_eq!(
            outcomes[1].detail.as_deref(),
            Some("backend could not prove quiescence")
        );
        assert_eq!(outcomes[2].outcome, OutcomeStatus::Error);
        assert_eq!(
            outcomes[2].reason_code.as_deref(),
            Some(report::QUIESCENCE_UNPROVEN)
        );
        assert!(
            outcomes[2]
                .detail
                .as_deref()
                .unwrap()
                .starts_with("not observed")
        );
        assert_eq!(outcomes[3].outcome, OutcomeStatus::Unsupported);
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

        let factorized = query_outcome_for_scale(
            &cartesian,
            execution(ExecutionClass::InProcessReference),
            "0.1",
        )
        .unwrap();
        // Error is the not-yet-executed placeholder, not a successful sample.
        assert_eq!(factorized.outcome, OutcomeStatus::Error);
        assert_eq!(factorized.reason_code, None);
        assert_eq!(
            factorized.rust_rows,
            Some(queries::RustRowEstimate {
                kind: queries::RustRowCardinality::NotMaterialized,
                rows: 0,
            })
        );
        let mut fallback = cartesian.clone();
        fallback.executable =
            "MATCH (a:Person), (b:Person), (c:Person) WITH a, b, c RETURN count(*)".into();
        for (case, class) in [
            (&fallback, ExecutionClass::InProcessReference),
            (&cartesian, ExecutionClass::BackendRowSourceRustProjection),
        ] {
            let refused = query_outcome_for_scale(case, execution(class), "0.1").unwrap();
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
            executable: "MATCH (n) WHERE n.missing = 1 RETURN count(*)".to_string(),
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

    #[test]
    fn rotation_remains_deterministic_after_any_prior_outcome() {
        let orders = (0..3)
            .map(|iteration| {
                (0..3)
                    .map(|position| rotated_index(position, iteration, 3))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert_eq!(orders, vec![vec![0, 1, 2], vec![1, 2, 0], vec![2, 0, 1]]);
    }

    #[test]
    fn unacknowledged_worker_error_requires_recovery_before_another_sample() {
        assert!(requires_backend_recovery(
            WorkerOutcome::Error,
            ObservationTerminationV3::NormalExit
        ));
        assert!(requires_backend_recovery(
            WorkerOutcome::Timeout,
            ObservationTerminationV3::DeadlineSigkill
        ));
        assert!(requires_backend_recovery(
            WorkerOutcome::Timeout,
            ObservationTerminationV3::DeadlineObservedExit
        ));
        assert!(!requires_backend_recovery(
            WorkerOutcome::Pass,
            ObservationTerminationV3::NormalExit
        ));
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
        outcome.warmups.push(QueryObservationV3 {
            iteration: 1,
            query_position: 1,
            plan: Some(report::ExecutionPlan::ClausePipeline),
            setup_ns: 1,
            elapsed_ns: 1,
            recovery_ns: 1,
            termination: ObservationTerminationV3::NormalExit,
            actual_count: None,
            outcome: OutcomeStatus::Error,
            detail: Some("warmup failed".to_string()),
        });
        outcome.measurements.push(QueryObservationV3 {
            iteration: 1,
            query_position: 1,
            plan: Some(report::ExecutionPlan::ClausePipeline),
            setup_ns: 1,
            elapsed_ns: 1,
            recovery_ns: 1,
            termination: ObservationTerminationV3::NormalExit,
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
