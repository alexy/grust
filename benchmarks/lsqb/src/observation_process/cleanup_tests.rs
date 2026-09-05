use super::*;

fn worker(cleanup: &str) -> Command {
    let script = format!(
        r#"
printf '%s\n' '{{"protocol":"grust-lsqb-observation-worker-v1","event":"ready","token":"cleanup","setup_ns":1}}'
read go
nonce=${{go#*\"go_nonce\":\"}}
nonce=${{nonce%%\"*}}
printf '{{"protocol":"grust-lsqb-observation-worker-v1","event":"result","token":"cleanup","go_nonce":"%s","outcome":"pass","actual_count":1,"worker_elapsed_ns":1}}\n' "$nonce"
{cleanup}
"#
    );
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(script);
    command
}

#[test]
fn post_result_cleanup_is_recovery_not_query_time() {
    let result = run(&mut worker("sleep 0.3"), "cleanup", 200, 10, 1000, 1000).unwrap();
    assert_eq!(result.outcome, WorkerOutcome::Pass);
    assert!(result.elapsed_ns < 200_000_000);
    assert!(result.recovery_ns >= 250_000_000);
}

#[test]
fn failed_cleanup_invalidates_an_already_emitted_result() {
    let error = run(&mut worker("exit 1"), "cleanup", 500, 10, 500, 500).unwrap_err();
    assert!(error.contains("failed after writing its result"));
}

#[test]
fn hung_cleanup_cannot_hold_the_coordinator_indefinitely() {
    let started = Instant::now();
    let error = run(&mut worker("sleep 10"), "cleanup", 500, 10, 50, 500).unwrap_err();
    assert!(error.contains("did not exit within the reap grace"));
    assert!(started.elapsed() < Duration::from_secs(2));
}
