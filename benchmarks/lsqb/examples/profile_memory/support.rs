use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub(super) const HELP: &str = "Memory indexed profiling diagnostic (never publication evidence)
Usage: profile_memory [--scale example|0.1|0.3] [--query ID] [--iterations N]
  --query-timeout-ms N  Hard per-query process deadline (default 30000, max 3600000)
  --max-seconds N       Hard overall deadline including load (default 120, max 86400)
  --progress-every N    Report first, last and every Nth iteration (default 100)
  --chunk-size N        CSV decode chunk rows (default 10000, max 100000)
  --lsqb-root PATH      Existing pinned upstream checkout (default upstream/lsqb)
  --dataset-dir PATH    Existing CSV directory, independent of query/oracle root
  --attacks-dir PATH    Existing attack sources (default attacks)
Defaults: example dataset, all 22 queries, one pass; no downloads or output files.
Timeout exits 124 without flushing output; the external wrapper records terminal status.
Source/count/plan errors exit 1.";

pub(super) struct Options {
    pub scale: String,
    pub query: Option<String>,
    pub iterations: u64,
    pub query_timeout_ms: u64,
    pub max_seconds: u64,
    pub progress_every: u64,
    pub chunk_size: usize,
    pub lsqb_root: PathBuf,
    pub dataset_dir: Option<PathBuf>,
    pub attacks_dir: PathBuf,
}

impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Self>, String> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut options = Self {
            scale: "example".into(),
            query: None,
            iterations: 1,
            query_timeout_ms: 30_000,
            max_seconds: 120,
            progress_every: 100,
            chunk_size: 10_000,
            lsqb_root: root.join("upstream/lsqb"),
            dataset_dir: None,
            attacks_dir: root.join("attacks"),
        };
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            if flag == "--help" || flag == "-h" {
                return Ok(None);
            }
            let value = args
                .next()
                .ok_or_else(|| format!("{flag} requires a value"))?;
            match flag.as_str() {
                "--scale" if matches!(value.as_str(), "example" | "0.1" | "0.3") => {
                    options.scale = value;
                }
                "--scale" => return Err("scale must be example, 0.1, or 0.3".into()),
                "--query" if !value.is_empty() => options.query = Some(value),
                "--iterations" => options.iterations = positive(&flag, &value, 1_000_000)?,
                "--query-timeout-ms" => {
                    options.query_timeout_ms = positive(&flag, &value, 3_600_000)?;
                }
                "--max-seconds" => options.max_seconds = positive(&flag, &value, 86_400)?,
                "--progress-every" => options.progress_every = positive(&flag, &value, 1_000_000)?,
                "--chunk-size" => options.chunk_size = positive(&flag, &value, 100_000)? as usize,
                "--lsqb-root" => options.lsqb_root = value.into(),
                "--dataset-dir" => options.dataset_dir = Some(value.into()),
                "--attacks-dir" => options.attacks_dir = value.into(),
                _ => return Err(format!("unknown option {flag:?}")),
            }
        }
        Ok(Some(options))
    }
}

fn positive(flag: &str, value: &str, maximum: u64) -> Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| (1..=maximum).contains(value))
        .ok_or_else(|| format!("{flag} must be in 1..={maximum}"))
}

#[derive(Clone)]
pub(super) struct Progress {
    started: Instant,
}

impl Progress {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }

    pub fn emit(&self, event: &str, fields: Value) -> Result<(), String> {
        self.write(&mut io::stdout().lock(), event, fields)
            .map_err(|error| format!("diagnostic progress output failed: {error}"))
    }

    fn write(&self, output: &mut impl Write, event: &str, fields: Value) -> io::Result<()> {
        let record = json!({
            "schema": "grust-lsqb-memory-profile-diagnostic-v1",
            "publication_eligible": false,
            "event": event,
            "pid": std::process::id(),
            "since_start_ms": self.started.elapsed().as_secs_f64() * 1_000.0,
            "detail": fields,
        });
        serde_json::to_writer(&mut *output, &record)?;
        output.write_all(b"\n")?;
        output.flush()
    }
}

enum Message {
    Query { id: String, deadline: Instant },
    Idle,
    Stop,
}

pub(super) struct Watchdog {
    sender: SyncSender<Message>,
    thread: Option<JoinHandle<()>>,
}

impl Watchdog {
    pub fn start(max_seconds: u64) -> Result<Self, String> {
        let overall = Instant::now()
            .checked_add(Duration::from_secs(max_seconds))
            .ok_or("overall deadline overflow")?;
        // Bound queued control messages even when the selected query is tiny.
        let (sender, receiver) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("memory-profile-deadline".into())
            .spawn(move || {
                if wait_for_timeout(receiver, overall).is_some() {
                    // All work belongs to this Memory-only diagnostic process.
                    // There are no child processes, services, or restart loops.
                    // SAFETY: _exit has no pointer preconditions and terminates
                    // this process. Intentionally bypass Rust/C cleanup: stdout
                    // may be blocked or locked by the main thread, so neither
                    // logging nor std::process::exit can enforce this deadline.
                    unsafe { libc::_exit(124) };
                }
            })
            .map_err(|error| format!("cannot create deadline thread: {error}"))?;
        Ok(Self {
            sender,
            thread: Some(thread),
        })
    }

    pub fn begin_query(&self, id: &str, timeout_ms: u64) -> Result<(), String> {
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(timeout_ms))
            .ok_or("query deadline overflow")?;
        self.sender
            .send(Message::Query {
                id: id.into(),
                deadline,
            })
            .map_err(|_| "diagnostic deadline thread disconnected".into())
    }

    pub fn end_query(&self) -> Result<(), String> {
        self.sender
            .send(Message::Idle)
            .map_err(|_| "diagnostic deadline thread disconnected".into())
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        let _ = self.sender.send(Message::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn wait_for_timeout(
    receiver: Receiver<Message>,
    overall: Instant,
) -> Option<(&'static str, Option<String>)> {
    let mut active: Option<(String, Instant)> = None;
    loop {
        let deadline = active
            .as_ref()
            .map_or(overall, |(_, query)| overall.min(*query));
        let timed_out = || {
            let scope = if Instant::now() >= overall {
                "overall"
            } else {
                "query"
            };
            (scope, active.as_ref().map(|(id, _)| id.clone()))
        };
        if Instant::now() >= deadline {
            return Some(timed_out());
        }
        let message = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        // Queued Idle/Query messages must not erase an already-expired bound.
        if Instant::now() >= deadline {
            return Some(timed_out());
        }
        match message {
            Ok(Message::Query { id, deadline }) => active = Some((id, deadline)),
            Ok(Message::Idle) => active = None,
            Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => return None,
            Err(RecvTimeoutError::Timeout) => return Some(timed_out()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_cheap_and_all_limits_are_finite_positive() {
        let defaults = Options::parse([]).unwrap().unwrap();
        assert_eq!(defaults.scale, "example");
        assert_eq!(defaults.iterations, 1);
        assert!(defaults.dataset_dir.is_none());
        for args in [
            ["--iterations", "0"],
            ["--iterations", "1000001"],
            ["--max-seconds", "0"],
            ["--max-seconds", "86401"],
            ["--query-timeout-ms", "3600001"],
            ["--chunk-size", "0"],
            ["--progress-every", "0"],
            ["--scale", "1"],
        ] {
            assert!(Options::parse(args.map(str::to_string)).is_err());
        }
    }

    #[test]
    fn dataset_directory_is_independent_of_the_query_oracle_root() {
        let options = Options::parse([
            "--dataset-dir".into(),
            "/existing/sf0.3".into(),
            "--lsqb-root".into(),
            "/existing/queries".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(options.dataset_dir, Some(PathBuf::from("/existing/sf0.3")));
        assert_eq!(options.lsqb_root, PathBuf::from("/existing/queries"));
    }

    #[test]
    fn deadline_messages_never_extend_the_overall_limit() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Message::Query {
                id: "q2".into(),
                deadline: Instant::now() + Duration::from_secs(60),
            })
            .unwrap();
        let timeout = wait_for_timeout(receiver, Instant::now()).unwrap();
        assert_eq!(timeout, ("overall", None));
        let (sender, receiver) = mpsc::channel();
        sender.send(Message::Stop).unwrap();
        assert!(wait_for_timeout(receiver, Instant::now() + Duration::from_secs(60)).is_none());
    }

    #[test]
    fn expired_query_is_not_erased_by_queued_idle_and_guard_joins() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Message::Query {
                id: "q2".into(),
                deadline: Instant::now(),
            })
            .unwrap();
        sender.send(Message::Idle).unwrap();
        assert_eq!(
            wait_for_timeout(receiver, Instant::now() + Duration::from_secs(60)),
            Some(("query", Some("q2".into())))
        );
        drop(Watchdog::start(60).unwrap());
    }

    #[test]
    fn progress_is_one_json_line_with_a_distinct_nonpublication_schema() {
        #[derive(Default)]
        struct Output {
            bytes: Vec<u8>,
            flushes: usize,
        }
        impl Write for Output {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                self.flushes += 1;
                Ok(())
            }
        }
        let mut output = Output::default();
        Progress::new()
            .write(&mut output, "index_ready", json!({"nodes": 28}))
            .unwrap();
        assert_eq!(output.flushes, 1);
        assert_eq!(
            output.bytes.iter().filter(|&&byte| byte == b'\n').count(),
            1
        );
        let value: Value = serde_json::from_slice(&output.bytes).unwrap();
        assert_eq!(value["schema"], "grust-lsqb-memory-profile-diagnostic-v1");
        assert_eq!(value["publication_eligible"], false);
        assert_eq!(value["detail"]["nodes"], 28);
        assert!(value.get("observations").is_none());
    }
}

#[cfg(all(test, unix))]
#[path = "timeout_tests.rs"]
mod timeout_tests;
