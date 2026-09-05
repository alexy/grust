//! Lossless incremental observations in the host-captured cell log.
//!
//! Unlike bounded progress telemetry, these records are synchronous and fail
//! the cell on write failure. They are diagnostics, never a completion receipt.

use std::io::{self, Write};

use crate::matrix_args::MatrixArguments;
use crate::report::QueryObservationV3;

pub fn record(
    arguments: &MatrixArguments,
    query_id: &str,
    warmup: bool,
    observation: &QueryObservationV3,
) -> Result<(), String> {
    let value = observation_event(
        &arguments.backend,
        &arguments.suite,
        &arguments.scale,
        query_id,
        warmup,
        observation,
    );
    write_record(&mut io::stdout().lock(), &value)
        .map_err(|error| format!("cannot persist incremental observation: {error}"))
}

fn observation_event(
    backend: &str,
    suite: &str,
    scale: &str,
    query_id: &str,
    warmup: bool,
    observation: &QueryObservationV3,
) -> serde_json::Value {
    serde_json::json!({
        "event": "observation-recorded",
        "journal_schema_version": 1,
        "report_schema_version": 3,
        "complete": false,
        "backend": backend,
        "suite": suite,
        "scale": scale,
        "query_id": query_id,
        "phase": if warmup { "warmup" } else { "measurement" },
        "observation": observation,
    })
}

fn write_record(writer: &mut impl Write, value: &serde_json::Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_observation_preserves_the_worker_execution_plan() {
        use crate::report::{ExecutionPlan, ObservationTerminationV3, OutcomeStatus};
        let observation = QueryObservationV3 {
            iteration: 1,
            query_position: 2,
            plan: Some(ExecutionPlan::ClausePipeline),
            setup_ns: 3,
            elapsed_ns: 25_000_000,
            recovery_ns: 4,
            termination: ObservationTerminationV3::DeadlineSigkill,
            actual_count: None,
            outcome: OutcomeStatus::Timeout,
            detail: None,
        };
        for warmup in [false, true] {
            let event =
                observation_event("memory", "baseline", "example", "q4", warmup, &observation);
            let mut bytes = Vec::new();
            write_record(&mut bytes, &event).unwrap();
            let decoded: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(decoded["observation"]["plan"], "clause-pipeline");
            assert_eq!(decoded["complete"], false);
            assert_eq!(
                decoded["phase"],
                if warmup { "warmup" } else { "measurement" }
            );
            assert_eq!(
                serde_json::from_value::<QueryObservationV3>(decoded["observation"].clone())
                    .unwrap(),
                observation
            );
        }
    }

    #[test]
    fn writes_one_complete_json_line() {
        let value = serde_json::json!({"actual_count": 42, "complete": false});
        let mut bytes = Vec::new();
        write_record(&mut bytes, &value).unwrap();
        assert_eq!(bytes.iter().filter(|&&byte| byte == b'\n').count(), 1);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            value
        );
    }

    #[test]
    fn propagates_failed_writes() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(write_record(&mut Broken, &serde_json::json!({})).is_err());
    }
}
