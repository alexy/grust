//! Native Bolt workers use the same killable READY/GO boundary as Grust.
use std::collections::BTreeMap;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use grust_lsqb_runner::observation_process::{self, IsolatedObservation, WorkerOutcome};
use neo4rs::query;
use serde_json::{Value, json};

pub(super) async fn probe() -> Result<(), &'static str> {
    if std::env::var("NEO4J_BENCHMARK_DISPOSABLE").as_deref() != Ok("1") {
        return Err("deadline probe requires an explicitly disposable service");
    }
    let (timed, recovery) = run(
        "UNWIND range(1, 100000000) AS i RETURN sum(sin(toFloat(i))) AS total",
        2_000,
    )
    .await?;
    if timed.outcome != WorkerOutcome::Timeout || timed.actual_count.is_some() {
        return Err("deadline probe did not force a coordinator timeout");
    }
    let (next, next_recovery) = run("RETURN 42 AS count", 5_000).await?;
    if next.outcome != WorkerOutcome::Pass || next.actual_count != Some(42) {
        return Err("post-timeout isolated query failed");
    }
    println!(
        "{}",
        json!({"event":"neo4j-process-deadline-probe", "benchmark_complete":false,
        "query_timeout_ms":2_000,"elapsed_ns":timed.elapsed_ns,"termination":timed.termination,
        "process_recovery_ns":timed.recovery_ns,"server_recovery":recovery,
        "next_isolated_scalar":42,"next_server_recovery":next_recovery})
    );
    Ok(())
}

pub(super) async fn worker() -> Result<(), &'static str> {
    let token = std::env::var("GRUST_NEO4J_TOKEN").map_err(|_| "worker token missing")?;
    let statement = std::env::var("GRUST_NEO4J_QUERY").map_err(|_| "worker query missing")?;
    let started = Instant::now();
    let timeout_ms: u64 = std::env::var("GRUST_NEO4J_TIMEOUT_MS")
        .map_err(|_| "worker timeout missing")?
        .parse()
        .map_err(|_| "worker timeout invalid")?;
    if timeout_ms == 0 || timeout_ms > 600_000 {
        return Err("worker timeout out of bounds");
    }
    // neo4rs applies this timeout to reads too. Keep it beyond the parent
    // deadline; the READY deadline still bounds connection/setup externally.
    let graph = super::connect_with_timeout(Duration::from_millis(timeout_ms + 10_000))?;
    let mut tx = graph
        .start_txn()
        .await
        .map_err(|_| "worker transaction failed")?;
    tx.run(query("CALL tx.setMetaData({grust_native_probe: $tag})").param("tag", token.clone()))
        .await
        .map_err(|_| "worker transaction tagging failed")?;
    observation_process::write_ready(
        &mut std::io::stdout(),
        &token,
        started.elapsed().as_nanos() as u64,
    )
    .map_err(|_| "worker READY failed")?;
    let nonce = observation_process::read_go(&mut std::io::stdin().lock(), &token)
        .map_err(|_| "worker GO failed")?;
    let started = Instant::now();
    let result = async {
        let mut rows = tx
            .execute(query(&statement))
            .await
            .map_err(|_| "worker query failed")?;
        let row = rows
            .next(&mut tx)
            .await
            .map_err(|_| "worker fetch failed")?
            .ok_or("worker scalar missing")?;
        let values: BTreeMap<String, i64> = row.to().map_err(|_| "worker scalar invalid")?;
        let count = super::scalar_value(&values)?;
        if rows
            .next(&mut tx)
            .await
            .map_err(|_| "worker completion failed")?
            .is_some()
        {
            return Err("worker returned multiple rows");
        }
        Ok(count)
    }
    .await;
    // Rollback belongs to the measured completion boundary. An error is never
    // converted into a timeout by the worker; only the coordinator owns that.
    let rolled_back = tx.rollback().await.is_ok();
    let result = result.and_then(|count| {
        if rolled_back {
            Ok(count)
        } else {
            Err("worker rollback failed")
        }
    });
    observation_process::write_result(
        &mut std::io::stdout(),
        &token,
        &nonce,
        if result.is_ok() {
            WorkerOutcome::Pass
        } else {
            WorkerOutcome::Error
        },
        result.ok(),
        started.elapsed().as_nanos() as u64,
    )
    .map_err(|_| "worker result failed")?;
    drop(graph);
    Ok(())
}

pub(super) async fn run(
    statement: &str,
    timeout_ms: u64,
) -> Result<(IsolatedObservation, Value), &'static str> {
    if timeout_ms == 0 || timeout_ms > 600_000 || statement.len() > 100_000 {
        return Err("invalid native observation bounds");
    }
    let tag = format!(
        "neo4j-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "worker clock unavailable")?
            .as_nanos()
    );
    let mut command =
        Command::new(std::env::current_exe().map_err(|_| "worker executable unavailable")?);
    command
        .arg("observation-worker")
        .env("GRUST_NEO4J_TOKEN", &tag)
        .env("GRUST_NEO4J_TIMEOUT_MS", timeout_ms.to_string())
        .env("GRUST_NEO4J_QUERY", statement);
    let worker_tag = tag.clone();
    let result = tokio::task::spawn_blocking(move || {
        observation_process::run(&mut command, &worker_tag, timeout_ms, 250, 5_000, 30_000)
    })
    .await
    .map_err(|_| "native coordinator task failed")?;
    // This executes even after a protocol/setup failure. No subsequent query
    // is permitted without separate-connection server absence verification.
    let started = Instant::now();
    let recovery = tokio::time::timeout(Duration::from_secs(15), async {
        let observer = super::connect()?;
        let ids = super::recovery::owned(&observer, &tag, false).await?;
        for id in &ids {
            super::recovery::terminate(&observer, id).await?;
        }
        while !super::recovery::owned(&observer, &tag, false)
            .await?
            .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if super::loading::scalar(&observer, query("RETURN 42 AS count"), false).await? != 42 {
            return Err("native recovery scalar mismatch");
        }
        Ok(
            json!({"transaction_tag":tag,"owned_transactions_remaining":0,
            "targeted_termination_count":ids.len(),"terminated_transaction_ids":ids,
            "subsequent_scalar":42,"server_recovery_ns":started.elapsed().as_nanos()}),
        )
    })
    .await
    .map_err(|_| "native server recovery deadline exceeded; stop dedicated service")??;
    Ok((
        result.map_err(|_| "native process boundary failed; observation rejected")?,
        recovery,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_bounds_are_rejected_before_spawning_or_connecting() {
        for deadline in [0, 600_001, u64::MAX] {
            assert_eq!(
                run("RETURN 42", deadline).await.unwrap_err(),
                "invalid native observation bounds"
            );
        }
        assert_eq!(
            run(&"x".repeat(100_001), 1).await.unwrap_err(),
            "invalid native observation bounds"
        );
    }
}
