//! Secret-safe, out-of-band progress events for long-running matrix cells.

use std::io::{self, Write};

use serde::Serialize;

use crate::report::OutcomeStatus;

const PREFIX: &[u8] = b"grust-lsqb-progress ";
const ATOMIC_PROGRESS_BYTES: usize = 512;

struct BoundedLine {
    content: Vec<u8>,
    overflowed: bool,
}

impl BoundedLine {
    fn new() -> Self {
        Self {
            content: Vec::with_capacity(ATOMIC_PROGRESS_BYTES),
            overflowed: false,
        }
    }
}

impl Write for BoundedLine {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.len() > ATOMIC_PROGRESS_BYTES - self.content.len() {
            self.overflowed = true;
            return Err(io::Error::other(
                "progress event exceeds the atomic write bound",
            ));
        }
        self.content.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryPhase {
    Warmup,
    Measurement,
}

#[derive(Clone, Copy, Debug)]
pub struct PhaseProgress<'a> {
    backend: &'a str,
    suite: &'a str,
    scale: &'a str,
    phase: QueryPhase,
    iteration_total: u32,
}

impl<'a> PhaseProgress<'a> {
    pub fn new(
        backend: &'a str,
        suite: &'a str,
        scale: &'a str,
        phase: QueryPhase,
        iteration_total: u32,
    ) -> Self {
        Self {
            backend,
            suite,
            scale,
            phase,
            iteration_total,
        }
    }

    pub fn iteration_total(self) -> u32 {
        self.iteration_total
    }

    pub fn is_warmup(self) -> bool {
        self.phase == QueryPhase::Warmup
    }

    pub fn query(
        self,
        iteration: u32,
        query_position: u32,
        query_total: u32,
        query_id: &'a str,
    ) -> QueryProgress<'a> {
        QueryProgress {
            backend: self.backend,
            suite: self.suite,
            scale: self.scale,
            phase: self.phase,
            iteration,
            iteration_total: self.iteration_total,
            query_position,
            query_total,
            query_id,
        }
    }
}

/// Fields are deliberately limited to authenticated catalog/query IDs and
/// numeric protocol coordinates. Query text, connection details, paths,
/// errors, and result values cannot enter a progress event.
#[derive(Clone, Copy, Debug)]
pub struct QueryProgress<'a> {
    backend: &'a str,
    suite: &'a str,
    scale: &'a str,
    phase: QueryPhase,
    iteration: u32,
    iteration_total: u32,
    query_position: u32,
    query_total: u32,
    query_id: &'a str,
}

#[derive(Serialize)]
struct QueryStartEvent<'a> {
    event: &'static str,
    backend: &'a str,
    suite: &'a str,
    scale: &'a str,
    phase: QueryPhase,
    iteration: u32,
    iteration_total: u32,
    query_position: u32,
    query_total: u32,
    query_id: &'a str,
}

#[derive(Serialize)]
struct QueryFinishEvent<'a> {
    event: &'static str,
    backend: &'a str,
    suite: &'a str,
    scale: &'a str,
    phase: QueryPhase,
    iteration: u32,
    iteration_total: u32,
    query_position: u32,
    query_total: u32,
    query_id: &'a str,
    outcome: OutcomeStatus,
    elapsed_ns: u64,
}

#[derive(Serialize)]
struct QueryReadyEvent<'a> {
    event: &'static str,
    backend: &'a str,
    suite: &'a str,
    scale: &'a str,
    phase: QueryPhase,
    iteration: u32,
    iteration_total: u32,
    query_position: u32,
    query_total: u32,
    query_id: &'a str,
    setup_ns: u64,
}

pub fn query_start(progress: QueryProgress<'_>) {
    let stderr = io::stderr();
    let mut writer = stderr.lock();
    let _ = write_query_start(&mut writer, progress);
}

pub fn query_finish(progress: QueryProgress<'_>, outcome: OutcomeStatus, elapsed_ns: u64) {
    let stderr = io::stderr();
    let mut writer = stderr.lock();
    let _ = write_query_finish(&mut writer, progress, outcome, elapsed_ns);
}

pub fn query_ready(progress: QueryProgress<'_>, setup_ns: u64) {
    let stderr = io::stderr();
    let mut writer = stderr.lock();
    let _ = write_query_ready(&mut writer, progress, setup_ns);
}

fn write_query_start(writer: &mut impl Write, progress: QueryProgress<'_>) -> io::Result<()> {
    write_event(
        writer,
        &QueryStartEvent {
            event: "query_start",
            backend: progress.backend,
            suite: progress.suite,
            scale: progress.scale,
            phase: progress.phase,
            iteration: progress.iteration,
            iteration_total: progress.iteration_total,
            query_position: progress.query_position,
            query_total: progress.query_total,
            query_id: progress.query_id,
        },
    )
}

fn write_query_finish(
    writer: &mut impl Write,
    progress: QueryProgress<'_>,
    outcome: OutcomeStatus,
    elapsed_ns: u64,
) -> io::Result<()> {
    write_event(
        writer,
        &QueryFinishEvent {
            event: "query_finish",
            backend: progress.backend,
            suite: progress.suite,
            scale: progress.scale,
            phase: progress.phase,
            iteration: progress.iteration,
            iteration_total: progress.iteration_total,
            query_position: progress.query_position,
            query_total: progress.query_total,
            query_id: progress.query_id,
            outcome,
            elapsed_ns,
        },
    )
}

fn write_query_ready(
    writer: &mut impl Write,
    progress: QueryProgress<'_>,
    setup_ns: u64,
) -> io::Result<()> {
    write_event(
        writer,
        &QueryReadyEvent {
            event: "query_ready",
            backend: progress.backend,
            suite: progress.suite,
            scale: progress.scale,
            phase: progress.phase,
            iteration: progress.iteration,
            iteration_total: progress.iteration_total,
            query_position: progress.query_position,
            query_total: progress.query_total,
            query_id: progress.query_id,
            setup_ns,
        },
    )
}

fn write_event(writer: &mut impl Write, event: &impl Serialize) -> io::Result<()> {
    let mut content = BoundedLine::new();
    content.write_all(PREFIX)?;
    if let Err(error) = serde_json::to_writer(&mut content, event) {
        return if content.overflowed {
            Ok(())
        } else {
            Err(io::Error::other(error))
        };
    }
    if let Err(error) = content.write_all(b"\n") {
        return if content.overflowed {
            Ok(())
        } else {
            Err(error)
        };
    }
    debug_assert!(content.content.len() <= ATOMIC_PROGRESS_BYTES);

    // At this producer boundary, one bounded write is indivisible on a normal
    // blocking POSIX pipe. Downstream transports may reframe it; 512 bytes is
    // PIPE_BUF's portable minimum.
    writer.write_all(&content.content)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[derive(Default)]
    struct FlushTrackingWriter {
        content: Vec<u8>,
        flushes: usize,
        writes: usize,
    }

    impl Write for FlushTrackingWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            self.content.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    fn progress(query_id: &str) -> QueryProgress<'_> {
        PhaseProgress::new("memory", "adversarial", "0.1", QueryPhase::Measurement, 5)
            .query(2, 3, 13, query_id)
    }

    #[test]
    fn start_is_exact_jsonl_with_escaping_and_an_explicit_flush() {
        let mut writer = FlushTrackingWriter::default();
        write_query_start(&mut writer, progress("attack-\"line\nnext")).unwrap();

        assert_eq!(writer.flushes, 1);
        assert_eq!(writer.writes, 1);
        assert_eq!(
            String::from_utf8(writer.content).unwrap(),
            concat!(
                "grust-lsqb-progress {\"event\":\"query_start\",",
                "\"backend\":\"memory\",\"suite\":\"adversarial\",",
                "\"scale\":\"0.1\",\"phase\":\"measurement\",",
                "\"iteration\":2,\"iteration_total\":5,",
                "\"query_position\":3,\"query_total\":13,",
                "\"query_id\":\"attack-\\\"line\\nnext\"}\n"
            )
        );
    }

    #[test]
    fn finish_discloses_only_protocol_coordinates_outcome_and_elapsed_time() {
        let mut writer = FlushTrackingWriter::default();
        write_query_finish(
            &mut writer,
            progress("a7-cartesian-count"),
            OutcomeStatus::Timeout,
            42,
        )
        .unwrap();

        assert_eq!(writer.flushes, 1);
        assert_eq!(writer.writes, 1);
        let line = String::from_utf8(writer.content).unwrap();
        assert_eq!(
            line,
            concat!(
                "grust-lsqb-progress {\"event\":\"query_finish\",",
                "\"backend\":\"memory\",\"suite\":\"adversarial\",",
                "\"scale\":\"0.1\",\"phase\":\"measurement\",",
                "\"iteration\":2,\"iteration_total\":5,",
                "\"query_position\":3,\"query_total\":13,",
                "\"query_id\":\"a7-cartesian-count\",",
                "\"outcome\":\"timeout\",\"elapsed_ns\":42}\n"
            )
        );

        let payload: serde_json::Value =
            serde_json::from_str(line.strip_prefix("grust-lsqb-progress ").unwrap()).unwrap();
        let keys = payload
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            BTreeSet::from([
                "backend",
                "elapsed_ns",
                "event",
                "iteration",
                "iteration_total",
                "outcome",
                "phase",
                "query_id",
                "query_position",
                "query_total",
                "scale",
                "suite",
            ])
        );
    }

    #[test]
    fn ready_reports_setup_completion_with_one_atomic_flushed_record() {
        let mut writer = FlushTrackingWriter::default();
        write_query_ready(&mut writer, progress("q3"), 314).unwrap();

        assert_eq!(writer.flushes, 1);
        assert_eq!(writer.writes, 1);
        assert_eq!(
            String::from_utf8(writer.content).unwrap(),
            concat!(
                "grust-lsqb-progress {\"event\":\"query_ready\",",
                "\"backend\":\"memory\",\"suite\":\"adversarial\",",
                "\"scale\":\"0.1\",\"phase\":\"measurement\",",
                "\"iteration\":2,\"iteration_total\":5,",
                "\"query_position\":3,\"query_total\":13,",
                "\"query_id\":\"q3\",\"setup_ns\":314}\n"
            )
        );
    }

    #[test]
    fn every_executed_outcome_has_its_canonical_spelling() {
        for (outcome, expected) in [
            (OutcomeStatus::Pass, "pass"),
            (OutcomeStatus::Mismatch, "mismatch"),
            (OutcomeStatus::Timeout, "timeout"),
            (OutcomeStatus::Error, "error"),
        ] {
            let mut writer = FlushTrackingWriter::default();
            write_query_finish(&mut writer, progress("q1"), outcome, 1).unwrap();
            let line = String::from_utf8(writer.content).unwrap();
            let payload: serde_json::Value =
                serde_json::from_str(line.strip_prefix("grust-lsqb-progress ").unwrap()).unwrap();
            assert_eq!(payload["outcome"], expected);
        }
    }

    #[test]
    fn oversized_events_are_dropped_before_any_write_or_flush() {
        let mut writer = FlushTrackingWriter::default();
        let oversized_query_id = "x".repeat(ATOMIC_PROGRESS_BYTES);
        write_query_start(&mut writer, progress(&oversized_query_id)).unwrap();

        assert_eq!(writer.writes, 0);
        assert_eq!(writer.flushes, 0);
        assert!(writer.content.is_empty());
    }
}
