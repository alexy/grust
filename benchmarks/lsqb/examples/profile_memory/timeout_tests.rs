//! Self-expiring subprocess tests; never run dataset or query workloads.

use super::*;
use std::process::{Child, Command, Stdio};

const CHILD_MODE: &str = "GRUST_PROFILE_MEMORY_BLOCKED_STDOUT_TEST";
const CHILD_TEST: &str = "support::timeout_tests::blocked_stdout_child";

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn hard_deadlines_exit_even_when_stdout_is_full_and_locked() {
    for mode in ["overall", "query"] {
        let mut child = OwnedChild(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", CHILD_TEST, "--nocapture"])
                .env(CHILD_MODE, mode)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        // Deliberately never read the pipe. Parent kill/reap is a second bound
        // independent of the child's kernel alarm and the watchdog under test.
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "{mode} child did not terminate");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(124), "{mode}: {status}");
    }
}

#[test]
fn blocked_stdout_child() {
    let Some(mode) = std::env::var_os(CHILD_MODE) else {
        return;
    };
    // SAFETY: this isolated test subprocess owns its signal disposition. A
    // default SIGALRM kills it within four seconds even if the watchdog regresses
    // or the test parent disappears; it does not affect the parent process.
    unsafe {
        libc::signal(libc::SIGALRM, libc::SIG_DFL);
        libc::alarm(4);
    }
    // In query mode the independent alarm fires before the overall bound, so
    // exit 124 proves the per-query deadline rather than its overall fallback.
    let watchdog = Watchdog::start(if mode == "query" { 5 } else { 1 }).unwrap();
    if mode == "query" {
        watchdog.begin_query("blocked-output", 500).unwrap();
    }
    // Exceeds the supported Unix pipe capacities and blocks while holding the
    // same stdout lock used by real progress events. No unbounded loop/fixture.
    Progress::new()
        .emit(
            "blocked_output",
            json!({"payload": "x".repeat(4 * 1024 * 1024)}),
        )
        .unwrap();
    panic!("unconsumed stdout unexpectedly accepted the entire payload");
}
