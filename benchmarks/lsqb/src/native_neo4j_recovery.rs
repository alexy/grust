//! Verify targeted server cancellation before admitting forced query deadlines.
use neo4rs::{Graph, query};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const WORK: &str = "UNWIND range(1, 100000000) AS i RETURN sum(sin(toFloat(i))) AS total";

pub(super) async fn owned(
    graph: &Graph,
    tag: &str,
    running_only: bool,
) -> Result<Vec<String>, &'static str> {
    let mut tx = graph
        .start_txn()
        .await
        .map_err(|_| "recovery inspection transaction failed")?;
    let mut rows = tx.execute(query("SHOW TRANSACTIONS YIELD transactionId, metaData, currentQuery WHERE metaData.grust_native_probe = $tag AND (NOT $running OR currentQuery = $work) RETURN transactionId")
        .param("tag", tag).param("running", running_only).param("work", WORK)).await
        .map_err(|_| "recovery transaction inspection failed")?;
    let mut ids = Vec::new();
    while let Some(row) = rows
        .next(&mut tx)
        .await
        .map_err(|_| "recovery inspection fetch failed")?
    {
        ids.push(
            row.get::<String>("transactionId")
                .map_err(|_| "recovery transaction identity invalid")?,
        );
        if ids.len() > 1 {
            return Err("probe tag identifies multiple transactions");
        }
    }
    tx.rollback()
        .await
        .map_err(|_| "recovery inspection rollback failed")?;
    Ok(ids)
}

fn termination_ack(expected: &str, actual: &str, message: &str) -> Result<bool, &'static str> {
    if expected != actual {
        return Err("termination acknowledgement identified another transaction");
    }
    match message {
        "Transaction terminated." => Ok(true),
        // A reaped worker's Bolt disconnect can remove the transaction after
        // SHOW and before TERMINATE. This is not a termination acknowledgement;
        // callers must still independently prove absence before proceeding.
        "Transaction not found." => Ok(false),
        _ => Err("termination acknowledgement did not confirm the owned transaction"),
    }
}

pub(super) async fn terminate(graph: &Graph, id: &str) -> Result<bool, &'static str> {
    let mut tx = graph
        .start_txn()
        .await
        .map_err(|_| "termination transaction failed")?;
    let mut rows = tx.execute(query("TERMINATE TRANSACTIONS $id YIELD transactionId, message RETURN transactionId, message")
        .param("id", id)).await.map_err(|_| "targeted termination failed")?;
    let row = rows
        .next(&mut tx)
        .await
        .map_err(|_| "termination acknowledgement failed")?
        .ok_or("termination acknowledgement missing")?;
    let acknowledged = termination_ack(
        id,
        &row.get::<String>("transactionId")
            .map_err(|_| "termination identity missing")?,
        &row.get::<String>("message")
            .map_err(|_| "termination result missing")?,
    )?;
    if rows
        .next(&mut tx)
        .await
        .map_err(|_| "termination completion failed")?
        .is_some()
    {
        return Err("unexpected extra termination acknowledgement");
    }
    tx.commit()
        .await
        .map_err(|_| "termination acknowledgement commit failed")?;
    Ok(acknowledged)
}

pub(super) async fn probe() -> Result<(), &'static str> {
    if std::env::var("NEO4J_BENCHMARK_DISPOSABLE").as_deref() != Ok("1") {
        return Err("recovery probe requires an explicitly disposable service");
    }
    let graph = super::connect()?;
    let observer = super::connect()?;
    let tag = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "probe clock unavailable")?
            .as_nanos()
    );
    let mut tx = graph
        .start_txn()
        .await
        .map_err(|_| "probe transaction failed")?;
    tx.run(query("CALL tx.setMetaData({grust_native_probe: $tag})").param("tag", tag.clone()))
        .await
        .map_err(|_| "probe transaction tagging failed")?;
    let worker = tokio::spawn(async move {
        let failed = match tx.execute(query(WORK)).await {
            Ok(mut rows) => rows.next(&mut tx).await.is_err(),
            Err(_) => true,
        };
        let _ = tx.rollback().await;
        failed
    });
    let started = Instant::now();
    let ids = loop {
        let ids = owned(&observer, &tag, true).await?;
        if !ids.is_empty() {
            break ids;
        }
        if worker.is_finished() || started.elapsed() > Duration::from_secs(10) {
            // No unrelated transaction is ever terminated. A failed probe is
            // not recovery proof; its dedicated service must be stopped.
            return Err("owned stress query was not observed running; stop the dedicated service");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let recovery_started = Instant::now();
    if !terminate(&observer, &ids[0]).await? {
        return Err("stress transaction disappeared before targeted termination was acknowledged");
    }
    let failed = tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .map_err(|_| "terminated worker did not return; stop the dedicated service")?
        .map_err(|_| "terminated worker task failed")?;
    if !failed {
        return Err("stress query completed normally, cancellation not qualified");
    }
    // A terminated transaction can remain registered while its failed Bolt
    // connection is retained in the worker's pool. Destroy that pool, as a
    // reaped isolated worker does, before asking the independent observer to
    // prove absence. Never return the failed worker pool to later queries.
    drop(graph);
    loop {
        if owned(&observer, &tag, false).await?.is_empty() {
            break;
        }
        if recovery_started.elapsed() > Duration::from_secs(10) {
            return Err(
                "owned transaction still present after termination; stop the dedicated service",
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    if super::loading::scalar(&observer, query("RETURN 42 AS count"), false).await? != 42 {
        return Err("post-recovery scalar probe mismatch");
    }
    println!(
        "{}",
        serde_json::json!({"event":"neo4j-recovery-probe", "observed_running":true,
        "targeted_termination_acknowledged":true,"worker_failed":true,"owned_transactions_remaining":0,
        "subsequent_scalar":42,"recovery_ns":recovery_started.elapsed().as_nanos(),"benchmark_complete":false})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::termination_ack;

    #[test]
    fn disappearance_is_not_claimed_as_acknowledged_termination() {
        assert_eq!(
            termination_ack("owned", "owned", "Transaction terminated."),
            Ok(true)
        );
        assert_eq!(
            termination_ack("owned", "owned", "Transaction not found."),
            Ok(false)
        );
        assert!(termination_ack("owned", "other", "Transaction terminated.").is_err());
        assert!(termination_ack("owned", "owned", "unknown response").is_err());
    }
}
