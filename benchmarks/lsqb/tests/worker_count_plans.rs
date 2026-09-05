//! Real worker protocol integration, not a benchmark evidence cohort.
//! Only the pinned example and in-process Memory/Turso stores are used.

#![cfg(unix)]

use std::path::Path;
use std::process::Command;

use grust_lsqb_runner::matrix_worker::WORKER_MARKER;
use grust_lsqb_runner::observation_process::{self, WorkerOutcome};
use grust_lsqb_runner::queries::{load_adversarial, load_baseline_for_scale};
use grust_lsqb_runner::report::{ExecutionPlan, ObservationTerminationV3};

fn check_worker(backend: &str, query_id: &str, plan: ExecutionPlan) {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    let upstream = directory.join("upstream/lsqb");
    let (suite, cases) = if query_id.starts_with('a') {
        (
            "adversarial",
            load_adversarial(&directory.join("attacks")).unwrap(),
        )
    } else {
        (
            "baseline",
            load_baseline_for_scale(&upstream, "example").unwrap(),
        )
    };
    let case = cases.iter().find(|case| case.id == query_id).unwrap();
    let token = format!("test-{backend}-{query_id}");
    let mut command = Command::new(env!("CARGO_BIN_EXE_grust-lsqb-matrix"));
    command
        .arg(WORKER_MARKER)
        .env_remove("GRUST_LSQB_SAIL_OWNED_SESSION")
        .env("GRUST_LSQB_WORKER_BACKEND", backend)
        .env("GRUST_LSQB_WORKER_SUITE", suite)
        .env("GRUST_LSQB_WORKER_SCALE", "example")
        .env("GRUST_LSQB_WORKER_QUERY_ID", query_id)
        .env("GRUST_LSQB_WORKER_LSQB_ROOT", upstream)
        .env("GRUST_LSQB_WORKER_ATTACKS_DIR", directory.join("attacks"))
        .env("GRUST_LSQB_WORKER_TOKEN", &token)
        .env("GRUST_LSQB_WORKER_QUERY_TIMEOUT_MS", "10000")
        .env("GRUST_LSQB_WORKER_ATTACH", "0");
    // The production coordinator owns setup/query/reap deadlines and cleans
    // up the complete child process group even when a test assertion fails.
    let observed = observation_process::run(&mut command, &token, 10_000, 100, 2_000, 10_000)
        .unwrap_or_else(|error| panic!("{backend}/{query_id} worker: {error}"));
    assert_eq!(observed.outcome, WorkerOutcome::Pass);
    assert_eq!(observed.termination, ObservationTerminationV3::NormalExit);
    assert_eq!(observed.actual_count, Some(case.expected_count));
    assert_eq!(observed.require_declared_plan().unwrap(), plan);
    assert!(observed.setup_ns > 0);
    assert!(observed.elapsed_ns > 0);
}

#[test]
fn memory_worker_declares_and_executes_every_pinned_count_plan() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut cases = load_baseline_for_scale(&directory.join("upstream/lsqb"), "example").unwrap();
    cases.extend(load_adversarial(&directory.join("attacks")).unwrap());
    assert_eq!(cases.len(), 22);
    for case in cases {
        check_worker("memory", &case.id, ExecutionPlan::CountFactorized);
    }
}

#[test]
fn turso_worker_declares_scalar_sql_and_reference_routes() {
    check_worker("turso", "q1", ExecutionPlan::SqlCount);
    check_worker("turso", "q6", ExecutionPlan::ClausePipeline);
}
