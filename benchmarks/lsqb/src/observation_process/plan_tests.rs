use super::*;

fn ready_record(plan: Option<ExecutionPlan>) -> Vec<u8> {
    let mut bytes = Vec::new();
    match plan {
        Some(plan) => write_ready_with_plan(&mut bytes, "plan-test", 1, plan).unwrap(),
        None => write_ready(&mut bytes, "plan-test", 1).unwrap(),
    }
    bytes
}

#[test]
fn execution_plan_ready_is_additive_and_native_ready_bytes_stay_unchanged() {
    let legacy = ready_record(None);
    assert_eq!(
        String::from_utf8(legacy.clone()).unwrap(),
        "{\"protocol\":\"grust-lsqb-observation-worker-v1\",\"event\":\"ready\",\"token\":\"plan-test\",\"setup_ns\":1}\n"
    );
    assert_eq!(
        serde_json::from_slice::<WorkerReady>(&legacy).unwrap().plan,
        None
    );
    for plan in [
        ExecutionPlan::ClausePipeline,
        ExecutionPlan::SqlRowSource,
        ExecutionPlan::BackendNative,
    ] {
        let bytes = ready_record(Some(plan));
        assert_eq!(
            serde_json::from_slice::<WorkerReady>(&bytes).unwrap().plan,
            Some(plan)
        );
    }
}

#[test]
fn unknown_and_null_execution_plans_are_invalid_ready_records() {
    for invalid in [
        serde_json::json!("count-factorized"),
        serde_json::Value::Null,
    ] {
        let mut value: serde_json::Value = serde_json::from_slice(&ready_record(None)).unwrap();
        value["plan"] = invalid;
        assert!(serde_json::from_value::<WorkerReady>(value).is_err());
    }
}

#[cfg(unix)]
fn worker(plan: Option<ExecutionPlan>, outcome: &str) -> Command {
    // SIGALRM caps the whole fixture, including a blocked GO read, at four
    // seconds even if the coordinator panics. No children or busy loops.
    let script = r#"
import json, os, signal, sys, time
signal.alarm(4)
signal.signal(signal.SIGTERM, signal.SIG_IGN)
print(os.environ['READY_RECORD'], flush=True)
go = json.loads(sys.stdin.readline())
outcome = os.environ['TEST_OUTCOME']
if outcome == 'hang':
    time.sleep(3)
    sys.exit(0)
print(json.dumps({
    'protocol': 'grust-lsqb-observation-worker-v1',
    'event': 'result', 'token': 'plan-test', 'go_nonce': go['go_nonce'],
    'outcome': outcome, 'actual_count': 42 if outcome == 'pass' else None,
    'worker_elapsed_ns': 1,
}), flush=True)
"#;
    let mut command = Command::new("python3");
    command
        .arg("-c")
        .arg(script)
        .env(
            "READY_RECORD",
            String::from_utf8(ready_record(plan)).unwrap().trim_end(),
        )
        .env("TEST_OUTCOME", outcome);
    command
}

#[test]
#[cfg(unix)]
fn ready_execution_plan_survives_success_error_and_backend_timeout() {
    for (outcome, expected, plan) in [
        ("pass", WorkerOutcome::Pass, ExecutionPlan::SqlRowSource),
        ("error", WorkerOutcome::Error, ExecutionPlan::ClausePipeline),
        (
            "timeout",
            WorkerOutcome::Timeout,
            ExecutionPlan::BackendNative,
        ),
    ] {
        let observation = run(
            &mut worker(Some(plan), outcome),
            "plan-test",
            500,
            10,
            1000,
            1000,
        )
        .unwrap();
        assert_eq!(observation.outcome, expected);
        assert_eq!(observation.require_declared_plan().unwrap(), plan);
    }
}

#[test]
#[cfg(unix)]
fn ready_execution_plan_survives_hard_timeout_without_a_result_record() {
    let observation = run(
        &mut worker(Some(ExecutionPlan::ClausePipeline), "hang"),
        "plan-test",
        25,
        10,
        1000,
        1000,
    )
    .unwrap();
    assert_eq!(observation.outcome, WorkerOutcome::Timeout);
    assert_eq!(
        observation.termination,
        ObservationTerminationV3::DeadlineSigkill
    );
    assert_eq!(observation.actual_count, None);
    assert_eq!(
        observation.require_declared_plan().unwrap(),
        ExecutionPlan::ClausePipeline
    );
}

#[test]
#[cfg(unix)]
fn legacy_worker_is_supported_but_cannot_supply_a_new_matrix_plan() {
    let observation = run(&mut worker(None, "pass"), "plan-test", 500, 10, 1000, 1000).unwrap();
    assert_eq!(observation.outcome, WorkerOutcome::Pass);
    assert_eq!(observation.plan, None);
    assert!(
        observation
            .require_declared_plan()
            .unwrap_err()
            .contains("omitted")
    );
}
