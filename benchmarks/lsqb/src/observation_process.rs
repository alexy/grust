//! Killable per-observation worker process boundary for matrix benchmarks.
//!
//! The coordinator owns the deadline. A worker completes all unmeasured setup,
//! emits `READY`, and waits for a token-bound `GO` before it starts query work.
//! Worker stdout is a private control channel; it is never forwarded to the
//! benchmark log or used as publication evidence.

use std::fs::File;
use std::io::{BufRead, Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::report::ObservationTerminationV3;

pub const WORKER_PROTOCOL: &str = "grust-lsqb-observation-worker-v1";
const MAX_CONTROL_LINE_BYTES: usize = 16 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(1);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerOutcome {
    Pass,
    Timeout,
    Error,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerReady {
    pub protocol: String,
    pub event: String,
    pub token: String,
    pub setup_ns: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerResult {
    pub protocol: String,
    pub event: String,
    pub token: String,
    pub go_nonce: String,
    pub outcome: WorkerOutcome,
    pub actual_count: Option<i64>,
    pub worker_elapsed_ns: u64,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkerGo<'a> {
    protocol: &'static str,
    command: &'static str,
    token: &'a str,
    go_nonce: &'a str,
}

#[derive(Debug)]
pub struct IsolatedObservation {
    pub outcome: WorkerOutcome,
    pub actual_count: Option<i64>,
    pub setup_ns: u64,
    pub elapsed_ns: u64,
    pub termination: ObservationTerminationV3,
    pub recovery_ns: u64,
}

/// Run one already-configured command in a fresh process group.
///
/// Setup is deliberately outside the query interval. The interval begins just
/// before `GO` is written and ends when the token-bound result is consumed.
pub fn run(
    command: &mut Command,
    token: &str,
    timeout_ms: u64,
    reap_grace_ms: u64,
    kill_reap_timeout_ms: u64,
    ready_timeout_ms: u64,
) -> Result<IsolatedObservation, String> {
    run_with_ready(
        command,
        token,
        timeout_ms,
        reap_grace_ms,
        kill_reap_timeout_ms,
        ready_timeout_ms,
        |_| {},
    )
}

/// Run one observation and report validated worker readiness before `GO`.
///
/// The callback is deliberately outside the measured interval. It exists only
/// to let the durable coordinator emit a meaningful setup-complete progress
/// event; it is never used as a control-plane input.
pub fn run_with_ready<F>(
    command: &mut Command,
    token: &str,
    timeout_ms: u64,
    reap_grace_ms: u64,
    kill_reap_timeout_ms: u64,
    ready_timeout_ms: u64,
    on_ready: F,
) -> Result<IsolatedObservation, String>
where
    F: FnOnce(u64),
{
    validate_token(token)?;
    require_process_group_support()?;
    let go_nonce = random_go_nonce()?;
    let go = serde_json::to_vec(&WorkerGo {
        protocol: WORKER_PROTOCOL,
        command: "go",
        token,
        go_nonce: &go_nonce,
    })
    .map_err(|_| "failed to encode observation worker GO record".to_string())?;
    let setup_started = Instant::now();
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_process_group(command);
    let mut child = command
        .spawn()
        .map_err(|_| "failed to spawn observation worker".to_string())?;
    let process_group = child.id();
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate_and_reap(
                &mut child,
                process_group,
                reap_grace_ms,
                kill_reap_timeout_ms,
            )?;
            return Err("observation worker stdout was not captured".to_string());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            terminate_and_reap(
                &mut child,
                process_group,
                reap_grace_ms,
                kill_reap_timeout_ms,
            )?;
            return Err("observation worker stderr was not captured".to_string());
        }
    };
    if configure_nonblocking(&stdout).is_err() || configure_nonblocking(&stderr).is_err() {
        terminate_and_reap(
            &mut child,
            process_group,
            reap_grace_ms,
            kill_reap_timeout_ms,
        )?;
        return Err("failed to configure bounded observation pipes".to_string());
    }
    let reader_timeout = Duration::from_millis(kill_reap_timeout_ms);
    let (line_receiver, stdout_thread) = control_reader(stdout, reader_timeout);
    let stderr_thread = discard_reader(stderr, reader_timeout);

    let ready_line = match receive_until_exit(
        &mut child,
        &line_receiver,
        Duration::from_millis(ready_timeout_ms),
    ) {
        Ok(line) => line,
        Err(error) => {
            terminate_and_reap(
                &mut child,
                process_group,
                reap_grace_ms,
                kill_reap_timeout_ms,
            )?;
            finish_reader_threads(stdout_thread, stderr_thread)?;
            return Err(error);
        }
    };
    let ready: WorkerReady = match parse_control(&ready_line, "READY") {
        Ok(ready) => ready,
        Err(error) => {
            terminate_and_reap(
                &mut child,
                process_group,
                reap_grace_ms,
                kill_reap_timeout_ms,
            )?;
            finish_reader_threads(stdout_thread, stderr_thread)?;
            return Err(error);
        }
    };
    if ready.protocol != WORKER_PROTOCOL || ready.event != "ready" || ready.token != token {
        terminate_and_reap(
            &mut child,
            process_group,
            reap_grace_ms,
            kill_reap_timeout_ms,
        )?;
        finish_reader_threads(stdout_thread, stderr_thread)?;
        return Err("observation worker emitted an invalid READY record".to_string());
    }

    let setup_elapsed = setup_started.elapsed();
    let setup_ns = duration_ns(setup_elapsed);
    if setup_elapsed > Duration::from_millis(ready_timeout_ms) || ready.setup_ns > setup_ns {
        terminate_and_reap(
            &mut child,
            process_group,
            reap_grace_ms,
            kill_reap_timeout_ms,
        )?;
        finish_reader_threads(stdout_thread, stderr_thread)?;
        return Err(
            "observation worker did not become READY within the configured timeout".to_string(),
        );
    }
    match line_receiver.try_recv() {
        Err(TryRecvError::Empty) => {}
        Ok(_) => {
            terminate_and_reap(
                &mut child,
                process_group,
                reap_grace_ms,
                kill_reap_timeout_ms,
            )?;
            finish_reader_threads(stdout_thread, stderr_thread)?;
            return Err("observation worker emitted a control record before GO".to_string());
        }
        Err(TryRecvError::Disconnected) => {
            terminate_and_reap(
                &mut child,
                process_group,
                reap_grace_ms,
                kill_reap_timeout_ms,
            )?;
            finish_reader_threads(stdout_thread, stderr_thread)?;
            return Err("observation worker closed its control channel before GO".to_string());
        }
    }

    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            terminate_and_reap(
                &mut child,
                process_group,
                reap_grace_ms,
                kill_reap_timeout_ms,
            )?;
            finish_reader_threads(stdout_thread, stderr_thread)?;
            return Err("observation worker stdin was not captured".to_string());
        }
    };
    on_ready(setup_ns);
    let started = Instant::now();
    if stdin
        .write_all(&go)
        .and_then(|()| stdin.write_all(b"\n"))
        .and_then(|()| stdin.flush())
        .is_err()
    {
        terminate_and_reap(
            &mut child,
            process_group,
            reap_grace_ms,
            kill_reap_timeout_ms,
        )?;
        finish_reader_threads(stdout_thread, stderr_thread)?;
        return Err("failed to send GO to observation worker".to_string());
    }
    drop(stdin);

    let timeout = Duration::from_millis(timeout_ms);
    let remaining = timeout.saturating_sub(started.elapsed());
    enum Reception {
        OnTime(String, Duration),
        Deadline(Duration),
    }
    let reception = match line_receiver.recv_timeout(remaining) {
        Ok(line) => {
            let elapsed = started.elapsed();
            if elapsed < timeout {
                Reception::OnTime(line, elapsed)
            } else {
                Reception::Deadline(elapsed)
            }
        }
        Err(RecvTimeoutError::Timeout) => Reception::Deadline(started.elapsed()),
        Err(RecvTimeoutError::Disconnected) => {
            terminate_and_reap(
                &mut child,
                process_group,
                reap_grace_ms,
                kill_reap_timeout_ms,
            )?;
            finish_reader_threads(stdout_thread, stderr_thread)?;
            return Err("observation worker exited without a result record".to_string());
        }
    };
    let (result_line, elapsed) = match reception {
        Reception::OnTime(line, elapsed) => (line, elapsed),
        Reception::Deadline(deadline_elapsed) => {
            // Record the monotonic instant at which the coordinator observed
            // the deadline. TERM/KILL/reap work begins afterwards and belongs
            // exclusively to `recovery_ns`; the configured cutoff itself
            // remains available as `timing.query_timeout_ms`.
            let deadline_elapsed_ns = duration_ns(deadline_elapsed);
            let recovery_started = Instant::now();
            let termination = terminate_at_deadline(
                &mut child,
                process_group,
                reap_grace_ms,
                kill_reap_timeout_ms,
            )?;
            finish_reader_threads(stdout_thread, stderr_thread)?;
            return Ok(IsolatedObservation {
                outcome: WorkerOutcome::Timeout,
                actual_count: None,
                setup_ns,
                elapsed_ns: deadline_elapsed_ns,
                termination,
                recovery_ns: duration_ns(recovery_started.elapsed()),
            });
        }
    };
    let result: WorkerResult = match parse_control(&result_line, "result") {
        Ok(result) => result,
        Err(error) => {
            terminate_and_reap(
                &mut child,
                process_group,
                reap_grace_ms,
                kill_reap_timeout_ms,
            )?;
            finish_reader_threads(stdout_thread, stderr_thread)?;
            return Err(error);
        }
    };
    if result.protocol != WORKER_PROTOCOL
        || result.event != "result"
        || result.token != token
        || result.go_nonce != go_nonce
    {
        terminate_and_reap(
            &mut child,
            process_group,
            reap_grace_ms,
            kill_reap_timeout_ms,
        )?;
        finish_reader_threads(stdout_thread, stderr_thread)?;
        return Err("observation worker emitted an invalid result record".to_string());
    }
    if (result.outcome == WorkerOutcome::Pass) != result.actual_count.is_some()
        || result.worker_elapsed_ns > duration_ns(elapsed)
    {
        terminate_and_reap(
            &mut child,
            process_group,
            reap_grace_ms,
            kill_reap_timeout_ms,
        )?;
        finish_reader_threads(stdout_thread, stderr_thread)?;
        return Err("observation worker emitted an incoherent result record".to_string());
    }

    let recovery_started = Instant::now();
    reap_completed(
        &mut child,
        process_group,
        reap_grace_ms,
        kill_reap_timeout_ms,
    )?;
    finish_reader_threads(stdout_thread, stderr_thread)?;
    reject_trailing_control(&line_receiver)?;
    Ok(IsolatedObservation {
        outcome: result.outcome,
        actual_count: result.actual_count,
        setup_ns,
        elapsed_ns: duration_ns(elapsed),
        termination: if result.outcome == WorkerOutcome::Timeout {
            ObservationTerminationV3::BackendTimeout
        } else {
            ObservationTerminationV3::NormalExit
        },
        recovery_ns: duration_ns(recovery_started.elapsed()),
    })
}

pub fn write_ready(writer: &mut impl Write, token: &str, setup_ns: u64) -> Result<(), String> {
    write_control(
        writer,
        &WorkerReady {
            protocol: WORKER_PROTOCOL.to_string(),
            event: "ready".to_string(),
            token: token.to_string(),
            setup_ns,
        },
    )
}

pub fn read_go(reader: &mut impl BufRead, token: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Go {
        protocol: String,
        command: String,
        token: String,
        go_nonce: String,
    }

    let line = match read_bounded_control_line(reader)
        .map_err(|_| "failed to read worker GO record".to_string())?
    {
        Some(BoundedControlLine::Line(line)) => line,
        Some(BoundedControlLine::Oversized | BoundedControlLine::InvalidUtf8) | None => {
            return Err("invalid worker GO record length".to_string());
        }
    };
    let go: Go = parse_control(&line, "GO")?;
    if go.protocol != WORKER_PROTOCOL
        || go.command != "go"
        || go.token != token
        || !valid_go_nonce(&go.go_nonce)
    {
        return Err("worker received an invalid GO record".to_string());
    }
    Ok(go.go_nonce)
}

pub fn write_result(
    writer: &mut impl Write,
    token: &str,
    go_nonce: &str,
    outcome: WorkerOutcome,
    actual_count: Option<i64>,
    elapsed_ns: u64,
) -> Result<(), String> {
    write_control(
        writer,
        &WorkerResult {
            protocol: WORKER_PROTOCOL.to_string(),
            event: "result".to_string(),
            token: token.to_string(),
            go_nonce: go_nonce.to_string(),
            outcome,
            actual_count,
            worker_elapsed_ns: elapsed_ns,
        },
    )
}

fn write_control(writer: &mut impl Write, value: &impl Serialize) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|_| "failed to encode worker control record".to_string())?;
    writer
        .write_all(b"\n")
        .and_then(|()| writer.flush())
        .map_err(|_| "failed to flush worker control record".to_string())
}

fn parse_control<T: for<'de> Deserialize<'de>>(line: &str, label: &str) -> Result<T, String> {
    if line.len() > MAX_CONTROL_LINE_BYTES {
        return Err(format!(
            "observation worker {label} record exceeded its size limit"
        ));
    }
    serde_json::from_str(line)
        .map_err(|_| format!("observation worker emitted a malformed {label} record"))
}

fn receive_until_exit(
    child: &mut Child,
    receiver: &Receiver<String>,
    timeout: Duration,
) -> Result<String, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(
                "observation worker did not become READY within the configured timeout".to_string(),
            );
        }
        match receiver.recv_timeout(remaining.min(Duration::from_millis(10))) {
            Ok(line) if Instant::now() < deadline => return Ok(line),
            Ok(_) => {
                return Err(
                    "observation worker did not become READY within the configured timeout"
                        .to_string(),
                );
            }
            Err(RecvTimeoutError::Timeout) => {
                if child
                    .try_wait()
                    .map_err(|_| "failed to inspect observation worker".to_string())?
                    .is_some()
                {
                    return Err("observation worker exited before READY".to_string());
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(
                    "observation worker closed its control channel before READY".to_string()
                );
            }
        }
    }
}

struct ReaderThread {
    shutdown: Arc<AtomicBool>,
    handle: thread::JoinHandle<()>,
    join_timeout: Duration,
}

fn control_reader(
    mut stdout: impl Read + Send + 'static,
    join_timeout: Duration,
) -> (Receiver<String>, ReaderThread) {
    let (sender, receiver) = mpsc::channel();
    let shutdown = Arc::new(AtomicBool::new(false));
    let reader_shutdown = Arc::clone(&shutdown);
    let handle = thread::spawn(move || {
        // READY, result, and at most one trailing record are sufficient to
        // validate the entire protocol. Capping the reader also prevents a
        // faulty worker from building an unbounded coordinator-side queue.
        let mut buffer = [0_u8; 4096];
        let mut line = Vec::with_capacity(256);
        let mut line_bytes = 0_usize;
        let mut oversized = false;
        let mut records = 0_u8;
        loop {
            if reader_shutdown.load(Ordering::Acquire) {
                break;
            }
            match stdout.read(&mut buffer) {
                Ok(0) => {
                    if line_bytes != 0 && !send_control_frame(&sender, &line, oversized) {
                        return;
                    }
                    break;
                }
                Ok(bytes) => {
                    for byte in &buffer[..bytes] {
                        line_bytes = line_bytes.saturating_add(1);
                        if line_bytes > MAX_CONTROL_LINE_BYTES {
                            oversized = true;
                            line.clear();
                        } else if *byte != b'\n' && !oversized {
                            line.push(*byte);
                        }
                        if *byte == b'\n' {
                            if !send_control_frame(&sender, &line, oversized) {
                                return;
                            }
                            records += 1;
                            if records == 3 {
                                return;
                            }
                            line.clear();
                            line_bytes = 0;
                            oversized = false;
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(POLL_INTERVAL);
                }
                Err(_) => break,
            }
        }
    });
    (
        receiver,
        ReaderThread {
            shutdown,
            handle,
            join_timeout,
        },
    )
}

fn send_control_frame(sender: &mpsc::Sender<String>, bytes: &[u8], oversized: bool) -> bool {
    let line = if oversized {
        "x".repeat(MAX_CONTROL_LINE_BYTES + 1)
    } else {
        String::from_utf8(bytes.to_vec()).unwrap_or_else(|_| "invalid-utf8".to_string())
    };
    sender.send(line).is_ok()
}

enum BoundedControlLine {
    Line(String),
    Oversized,
    InvalidUtf8,
}

/// Read and drain exactly one line while retaining at most the protocol cap.
/// `BufRead::read_line` may allocate without limit before callers can inspect
/// the length, so the coordinator and worker both use this bounded reader.
fn read_bounded_control_line(
    reader: &mut impl BufRead,
) -> std::io::Result<Option<BoundedControlLine>> {
    let mut bytes = Vec::with_capacity(256);
    let mut oversized = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if bytes.is_empty() && !oversized {
                return Ok(None);
            }
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |position| position + 1);
        if !oversized {
            if bytes.len().saturating_add(consumed) > MAX_CONTROL_LINE_BYTES {
                oversized = true;
                bytes.clear();
            } else {
                bytes.extend_from_slice(&available[..consumed]);
            }
        }
        reader.consume(consumed);
        if newline.is_some() {
            break;
        }
    }
    if oversized {
        return Ok(Some(BoundedControlLine::Oversized));
    }
    Ok(Some(match String::from_utf8(bytes) {
        Ok(line) => BoundedControlLine::Line(line),
        Err(_) => BoundedControlLine::InvalidUtf8,
    }))
}

fn discard_reader(mut reader: impl Read + Send + 'static, join_timeout: Duration) -> ReaderThread {
    let shutdown = Arc::new(AtomicBool::new(false));
    let reader_shutdown = Arc::clone(&shutdown);
    let handle = thread::spawn(move || drain(&mut reader, &reader_shutdown));
    ReaderThread {
        shutdown,
        handle,
        join_timeout,
    }
}

fn drain(reader: &mut impl Read, shutdown: &AtomicBool) {
    let mut buffer = [0_u8; 8192];
    while !shutdown.load(Ordering::Acquire) {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(POLL_INTERVAL);
            }
            Err(_) => break,
        }
    }
}

fn finish_reader_threads(stdout: ReaderThread, stderr: ReaderThread) -> Result<(), String> {
    let deadline = Instant::now() + stdout.join_timeout.max(stderr.join_timeout);
    while (!stdout.handle.is_finished() || !stderr.handle.is_finished())
        && Instant::now() < deadline
    {
        thread::sleep(POLL_INTERVAL);
    }
    let closed_naturally = stdout.handle.is_finished() && stderr.handle.is_finished();
    stdout.shutdown.store(true, Ordering::Release);
    stderr.shutdown.store(true, Ordering::Release);
    stdout
        .handle
        .join()
        .map_err(|_| "observation worker stdout reader panicked".to_string())?;
    stderr
        .handle
        .join()
        .map_err(|_| "observation worker stderr reader panicked".to_string())?;
    if closed_naturally {
        Ok(())
    } else {
        Err("observation worker pipes remained open after process-group recovery".to_string())
    }
}

fn reject_trailing_control(receiver: &Receiver<String>) -> Result<(), String> {
    if receiver.try_iter().next().is_some() {
        Err("observation worker emitted trailing control records".to_string())
    } else {
        Ok(())
    }
}

fn reap_completed(
    child: &mut Child,
    process_group: u32,
    term_grace_ms: u64,
    kill_reap_timeout_ms: u64,
) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + Duration::from_millis(kill_reap_timeout_ms);
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|_| "failed to reap observation worker".to_string())?
        {
            if !status.success() {
                terminate_and_reap(child, process_group, term_grace_ms, kill_reap_timeout_ms)?;
                return Err("observation worker failed after writing its result".to_string());
            }
            if process_group_exists(process_group)? {
                terminate_and_reap(child, process_group, term_grace_ms, kill_reap_timeout_ms)?;
                return Err(
                    "observation worker group remained active after writing its result".to_string(),
                );
            }
            return Ok(status);
        }
        if Instant::now() >= deadline {
            terminate_and_reap(child, process_group, term_grace_ms, kill_reap_timeout_ms)?;
            return Err("observation worker did not exit within the reap grace".to_string());
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn terminate_and_reap(
    child: &mut Child,
    process_group: u32,
    term_grace_ms: u64,
    kill_reap_timeout_ms: u64,
) -> Result<(), String> {
    terminate_at_deadline(child, process_group, term_grace_ms, kill_reap_timeout_ms).map(drop)
}

fn terminate_at_deadline(
    child: &mut Child,
    process_group: u32,
    term_grace_ms: u64,
    kill_reap_timeout_ms: u64,
) -> Result<ObservationTerminationV3, String> {
    let term_signal = signal_process_group(process_group, libc::SIGTERM)?;
    let term_deadline = Instant::now() + Duration::from_millis(term_grace_ms);
    if reap_group_until(child, process_group, term_deadline)? {
        return Ok(match term_signal {
            SignalDelivery::Delivered => ObservationTerminationV3::DeadlineSigterm,
            SignalDelivery::GroupAbsent => ObservationTerminationV3::DeadlineObservedExit,
        });
    }
    let kill_signal = signal_process_group(process_group, libc::SIGKILL)?;
    let kill_deadline = Instant::now() + Duration::from_millis(kill_reap_timeout_ms);
    if reap_group_until(child, process_group, kill_deadline)? {
        Ok(match (term_signal, kill_signal) {
            (_, SignalDelivery::Delivered) => ObservationTerminationV3::DeadlineSigkill,
            (SignalDelivery::Delivered, SignalDelivery::GroupAbsent) => {
                ObservationTerminationV3::DeadlineSigterm
            }
            (SignalDelivery::GroupAbsent, SignalDelivery::GroupAbsent) => {
                ObservationTerminationV3::DeadlineObservedExit
            }
        })
    } else {
        Err("observation worker could not be reaped after SIGKILL".to_string())
    }
}

fn reap_group_until(
    child: &mut Child,
    process_group: u32,
    deadline: Instant,
) -> Result<bool, String> {
    loop {
        let leader_reaped = child
            .try_wait()
            .map_err(|_| "failed to reap observation worker".to_string())?
            .is_some();
        if leader_reaped && !process_group_exists(process_group)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn configure_nonblocking(stream: &impl std::os::fd::AsRawFd) -> Result<(), String> {
    let descriptor = stream.as_raw_fd();
    // SAFETY: both fcntl operations target a live pipe descriptor owned by
    // this coordinator. They only inspect and add the nonblocking status bit.
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags < 0 {
        return Err("failed to inspect observation pipe flags".to_string());
    }
    // SAFETY: `descriptor` is unchanged and `flags | O_NONBLOCK` is a valid
    // F_SETFL status-mask value.
    if unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err("failed to set observation pipe nonblocking".to_string());
    }
    Ok(())
}

#[cfg(not(unix))]
fn configure_nonblocking<T>(_stream: &T) -> Result<(), String> {
    Err("hard observation isolation requires Unix nonblocking pipes".to_string())
}

#[cfg(unix)]
fn require_process_group_support() -> Result<(), String> {
    Ok(())
}

#[cfg(not(unix))]
fn require_process_group_support() -> Result<(), String> {
    Err("hard observation isolation requires a Unix process group".to_string())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SignalDelivery {
    Delivered,
    GroupAbsent,
}

#[cfg(unix)]
fn signal_process_group(process_group: u32, signal: i32) -> Result<SignalDelivery, String> {
    let group = i32::try_from(process_group)
        .map_err(|_| "observation worker process group was out of range".to_string())?;
    // SAFETY: `kill` receives a validated child process-group id and a fixed
    // signal. A negative id targets only that dedicated group.
    let result = unsafe { libc::kill(-group, signal) };
    if result == 0 {
        return Ok(SignalDelivery::Delivered);
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => Ok(SignalDelivery::GroupAbsent),
        _ => Err("failed to kill observation worker process group".to_string()),
    }
}

#[cfg(not(unix))]
fn signal_process_group(_process_group: u32, _signal: i32) -> Result<SignalDelivery, String> {
    Err("hard observation isolation requires a Unix process group".to_string())
}

#[cfg(unix)]
fn process_group_exists(process_group: u32) -> Result<bool, String> {
    let group = i32::try_from(process_group)
        .map_err(|_| "observation worker process group was out of range".to_string())?;
    // SAFETY: signal zero performs an existence check without delivering a
    // signal. The id is the dedicated child process group created above.
    let result = unsafe { libc::kill(-group, 0) };
    if result == 0 {
        return Ok(true);
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err("failed to inspect observation worker process group".to_string()),
    }
}

#[cfg(not(unix))]
fn process_group_exists(_process_group: u32) -> Result<bool, String> {
    Err("hard observation isolation requires a Unix process group".to_string())
}

fn validate_token(token: &str) -> Result<(), String> {
    if token.is_empty()
        || token.len() > 128
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("invalid observation worker token".to_string());
    }
    Ok(())
}

fn random_go_nonce() -> Result<String, String> {
    let mut random = [0_u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut random))
        .map_err(|_| "failed to create an observation GO nonce".to_string())?;
    let mut encoded = String::with_capacity(random.len() * 2);
    for byte in random {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}")
            .map_err(|_| "failed to encode an observation GO nonce".to_string())?;
    }
    Ok(encoded)
}

fn valid_go_nonce(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(all(test, unix))]
#[path = "observation_process/cleanup_tests.rs"]
mod cleanup_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn shell(script: &str, token: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(script).env("TEST_TOKEN", token);
        command
    }

    #[cfg(unix)]
    const READY_ONLY: &str = r#"printf '%s\n' "{\"protocol\":\"grust-lsqb-observation-worker-v1\",\"event\":\"ready\",\"token\":\"$TEST_TOKEN\",\"setup_ns\":7}""#;

    #[cfg(unix)]
    const READY: &str = r#"printf '%s\n' "{\"protocol\":\"grust-lsqb-observation-worker-v1\",\"event\":\"ready\",\"token\":\"$TEST_TOKEN\",\"setup_ns\":7}"; read go; go_nonce=${go#*\"go_nonce\":\"}; go_nonce=${go_nonce%%\"*}"#;

    #[test]
    #[cfg(unix)]
    fn spinning_worker_is_killed_at_deadline_and_next_worker_succeeds() {
        let token = "spin-1";
        let mut spinner = shell(
            &format!("{READY}; trap '' TERM; while :; do :; done"),
            token,
        );
        let started = Instant::now();
        let timed_out = run(&mut spinner, token, 25, 10, 500, 500).unwrap();
        assert_eq!(timed_out.outcome, WorkerOutcome::Timeout);
        assert_eq!(
            timed_out.termination,
            ObservationTerminationV3::DeadlineSigkill
        );
        assert!(timed_out.elapsed_ns >= 25_000_000);
        assert!(timed_out.recovery_ns >= 10_000_000);
        assert!(started.elapsed() < Duration::from_secs(2));

        let next = "next-1";
        let result = format!(
            "{READY}; printf '{{\"protocol\":\"grust-lsqb-observation-worker-v1\",\"event\":\"result\",\"token\":\"%s\",\"go_nonce\":\"%s\",\"outcome\":\"pass\",\"actual_count\":1,\"worker_elapsed_ns\":1}}\\n' \"$TEST_TOKEN\" \"$go_nonce\""
        );
        let mut worker = shell(&result, next);
        let completed = run(&mut worker, next, 500, 10, 500, 500).unwrap();
        assert_eq!(completed.outcome, WorkerOutcome::Pass);
        assert_eq!(completed.actual_count, Some(1));
    }

    #[test]
    #[cfg(unix)]
    fn timeout_kills_descendants_in_the_worker_group() {
        let token = "tree-1";
        let script = format!("{READY}; trap '' TERM; (while :; do :; done) & while :; do :; done");
        let mut worker = shell(&script, token);
        let observation = run(&mut worker, token, 25, 10, 1_000, 500).unwrap();
        assert_eq!(
            observation.termination,
            ObservationTerminationV3::DeadlineSigkill
        );
    }

    #[test]
    #[cfg(unix)]
    fn deadline_records_an_exit_observed_before_signal_delivery() {
        let mut worker = shell("exit 0", "already-exited-1");
        configure_process_group(&mut worker);
        let mut child = worker.spawn().unwrap();
        let process_group = child.id();
        child.wait().unwrap();

        assert_eq!(
            terminate_at_deadline(&mut child, process_group, 0, 100).unwrap(),
            ObservationTerminationV3::DeadlineObservedExit
        );
    }

    #[test]
    #[cfg(unix)]
    fn escaped_pipe_holder_fails_recovery_within_the_reader_bound() {
        let pid_file = tempfile::NamedTempFile::new().unwrap();
        let script = format!(
            "python3 -c 'import os,time; os.setsid(); open(os.environ[\"ESCAPED_PID_FILE\"], \"w\").write(str(os.getpid())); time.sleep(10)' & while [ ! -s \"$ESCAPED_PID_FILE\" ]; do :; done; {READY}; trap '' TERM; while :; do :; done"
        );
        let mut worker = shell(&script, "escaped-1");
        worker.env("ESCAPED_PID_FILE", pid_file.path());
        let started = Instant::now();
        let error = run(&mut worker, "escaped-1", 20, 5, 50, 500).unwrap_err();
        assert!(error.contains("pipes remained open"));
        assert!(started.elapsed() < Duration::from_secs(2));

        let pid = std::fs::read_to_string(pid_file.path())
            .unwrap()
            .parse::<i32>()
            .unwrap();
        // SAFETY: the PID came from the deliberately escaped fixture process.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }

    #[test]
    #[cfg(unix)]
    fn malformed_and_late_results_are_not_accepted_as_success() {
        let token = "bad-1";
        let mut malformed = shell(&format!("{READY}; printf 'not-json\\n'"), token);
        assert!(run(&mut malformed, token, 500, 10, 500, 500).is_err());

        let late_token = "late-1";
        let late = format!(
            "{READY}; sleep 0.05; printf '{{\"protocol\":\"grust-lsqb-observation-worker-v1\",\"event\":\"result\",\"token\":\"%s\",\"go_nonce\":\"%s\",\"outcome\":\"pass\",\"actual_count\":1,\"worker_elapsed_ns\":1}}\\n' \"$TEST_TOKEN\" \"$go_nonce\""
        );
        let mut late_worker = shell(&late, late_token);
        let observation = run(&mut late_worker, late_token, 10, 10, 500, 500).unwrap();
        assert_eq!(observation.outcome, WorkerOutcome::Timeout);
        assert!(matches!(
            observation.termination,
            ObservationTerminationV3::DeadlineObservedExit
                | ObservationTerminationV3::DeadlineSigterm
                | ObservationTerminationV3::DeadlineSigkill
        ));
    }

    #[test]
    #[cfg(unix)]
    fn result_must_be_nonce_bound_to_go_and_cannot_be_emitted_early() {
        let token = "early-1";
        let premature = format!(
            "{READY_ONLY}; printf '{{\"protocol\":\"grust-lsqb-observation-worker-v1\",\"event\":\"result\",\"token\":\"%s\",\"go_nonce\":\"00000000000000000000000000000000\",\"outcome\":\"pass\",\"actual_count\":1,\"worker_elapsed_ns\":1}}\\n' \"$TEST_TOKEN\"; read go"
        );
        let error = run(&mut shell(&premature, token), token, 500, 10, 500, 500).unwrap_err();
        assert!(error.contains("before GO") || error.contains("invalid result"));
    }

    #[test]
    #[cfg(unix)]
    fn wrong_token_duplicate_and_oversized_records_are_fatal() {
        let token = "strict-1";
        let wrong = format!(
            "{READY}; printf '{{\"protocol\":\"grust-lsqb-observation-worker-v1\",\"event\":\"result\",\"token\":\"wrong\",\"go_nonce\":\"%s\",\"outcome\":\"pass\",\"actual_count\":1,\"worker_elapsed_ns\":1}}\\n' \"$go_nonce\""
        );
        assert!(run(&mut shell(&wrong, token), token, 500, 10, 500, 500).is_err());

        let duplicate = format!(
            "{READY}; line=$(printf '{{\"protocol\":\"grust-lsqb-observation-worker-v1\",\"event\":\"result\",\"token\":\"strict-2\",\"go_nonce\":\"%s\",\"outcome\":\"pass\",\"actual_count\":1,\"worker_elapsed_ns\":1}}' \"$go_nonce\"); printf '%s\\n%s\\n' \"$line\" \"$line\""
        );
        assert!(
            run(
                &mut shell(&duplicate, "strict-2"),
                "strict-2",
                500,
                10,
                500,
                500,
            )
            .is_err()
        );

        let padded_duplicate = format!(
            "{READY}; line=$(printf '{{\"protocol\":\"grust-lsqb-observation-worker-v1\",\"event\":\"result\",\"token\":\"strict-4\",\"go_nonce\":\"%s\",\"outcome\":\"pass\",\"actual_count\":1,\"worker_elapsed_ns\":1}}' \"$go_nonce\"); printf '%s\\n\\n%s\\n' \"$line\" \"$line\""
        );
        assert!(
            run(
                &mut shell(&padded_duplicate, "strict-4"),
                "strict-4",
                500,
                10,
                500,
                500,
            )
            .is_err()
        );

        let oversized = format!(
            "{READY}; i=0; while [ $i -lt 17000 ]; do printf x; i=$((i+1)); done; printf '\\n'"
        );
        assert!(
            run(
                &mut shell(&oversized, "strict-3"),
                "strict-3",
                500,
                10,
                500,
                500,
            )
            .is_err()
        );
    }

    #[test]
    #[cfg(unix)]
    fn ready_wait_is_bounded_and_worker_stderr_is_never_echoed() {
        let started = Instant::now();
        let mut unready = shell("sleep 10", "ready-1");
        let error = run(&mut unready, "ready-1", 500, 10, 500, 10).unwrap_err();
        assert!(error.contains("READY"));
        assert!(started.elapsed() < Duration::from_secs(2));

        let mut late_ready = shell(&format!("sleep 0.03; {READY_ONLY}; read go"), "ready-2");
        let error = run(&mut late_ready, "ready-2", 500, 10, 500, 10).unwrap_err();
        assert!(error.contains("READY"));

        let secret = "postgres://user:sentinel-secret@example.invalid/db";
        let leaking = format!("{READY}; printf '%s\\n' '{secret}' >&2; printf 'bad\\n'");
        let error = run(
            &mut shell(&leaking, "secret-1"),
            "secret-1",
            500,
            10,
            500,
            500,
        )
        .unwrap_err();
        assert!(!error.contains(secret));
        assert!(!error.contains("sentinel-secret"));
    }

    #[test]
    fn control_records_do_not_echo_untrusted_values_in_errors() {
        let secret = "postgres://user:secret@example.invalid/db";
        let error = parse_control::<WorkerResult>(secret, "result").unwrap_err();
        assert!(!error.contains(secret));
        assert!(!error.contains("secret"));
    }
}
