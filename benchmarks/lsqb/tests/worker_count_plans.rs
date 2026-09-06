//! Real worker protocol integration, not a benchmark evidence cohort.
//! Only the pinned example and in-process Memory/Turso stores are used.

#![cfg(unix)]

use std::path::Path;
use std::process::Command;

use grust_lsqb_runner::backend::turso_snapshot;
use grust_lsqb_runner::dataset::load_projected_dataset;
use grust_lsqb_runner::matrix_worker::WORKER_MARKER;
use grust_lsqb_runner::observation_process::{self, WorkerOutcome};
use grust_lsqb_runner::queries::{load_adversarial, load_baseline_for_scale};
use grust_lsqb_runner::report::{ExecutionPlan, ObservationTerminationV3};

fn check_worker(backend: &str, query_id: &str, plan: ExecutionPlan) {
    check_worker_with(backend, query_id, plan, &[]);
}

fn check_worker_with(
    backend: &str,
    query_id: &str,
    plan: ExecutionPlan,
    extra_env: &[(&str, &Path)],
) {
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
    for (name, value) in extra_env {
        command.env(name, value);
    }
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
fn turso_worker_declares_the_resident_route_for_every_pinned_case() {
    // A Turso worker copies the coordinator's prebuilt store; this test plays
    // the coordinator, keeping the file alive across both workers.
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    let graph = load_projected_dataset(
        &directory.join("upstream/lsqb/data/social-network-sfexample-projected-fk"),
    )
    .unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let coordinator = runtime
        .block_on(turso_snapshot::prepare_from_chunks(std::iter::once(Ok(
            graph,
        ))))
        .unwrap();
    let source = coordinator.turso_snapshot_path().unwrap();
    let env = [(turso_snapshot::ENV_SNAPSHOT, source)];
    check_worker_with("turso", "q1", ExecutionPlan::CountFactorized, &env);
    check_worker_with("turso", "q6", ExecutionPlan::CountFactorized, &env);
    drop(coordinator);
}
