//! Native Neo4j qualification entry point. Never changes an existing database.
use std::collections::BTreeMap;
use std::time::Duration;

use neo4rs::{ConfigBuilder, Graph, query};

fn scalar_value(values: &BTreeMap<String, i64>) -> Result<i64, &'static str> {
    if values.len() != 1 {
        return Err("Neo4j scalar result must have exactly one integer column");
    }
    Ok(*values.values().next().expect("one value"))
}

async fn probe() -> Result<(), &'static str> {
    let config = ConfigBuilder::default()
        .uri(std::env::var("NEO4J_URI").map_err(|_| "NEO4J_URI is required")?)
        .user(std::env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".into()))
        .password(std::env::var("NEO4J_PASSWORD").unwrap_or_default())
        .db("neo4j")
        .max_connections(2)
        .fetch_size(10)
        .connection_timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "invalid Neo4j configuration")?;
    let graph = Graph::connect(config).map_err(|_| "Neo4j connection setup failed")?;
    // Explicit transactions do not retry. Graph::execute transparently retries
    // transient errors and must not be used for measured benchmark observations.
    let mut transaction = graph
        .start_txn()
        .await
        .map_err(|_| "Neo4j transaction failed")?;
    let mut rows = transaction
        .execute(query(
        "CALL dbms.components() YIELD name, versions, edition WHERE name = 'Neo4j Kernel' RETURN versions[0] AS version, edition",
        ))
        .await
        .map_err(|_| "Neo4j version probe failed")?;
    let row = rows
        .next(&mut transaction)
        .await
        .map_err(|_| "Neo4j version fetch failed")?
        .ok_or("Neo4j version probe returned no row")?;
    let version: String = row.get("version").map_err(|_| "invalid Neo4j version")?;
    let edition: String = row.get("edition").map_err(|_| "invalid Neo4j edition")?;
    if rows
        .next(&mut transaction)
        .await
        .map_err(|_| "Neo4j probe completion failed")?
        .is_some()
    {
        return Err("Neo4j version probe returned multiple rows");
    }
    let mut rows = transaction
        .execute(query("RETURN 42 AS count"))
        .await
        .map_err(|_| "Neo4j scalar probe failed")?;
    let row = rows
        .next(&mut transaction)
        .await
        .map_err(|_| "Neo4j scalar fetch failed")?
        .ok_or("Neo4j scalar probe returned no row")?;
    let values: BTreeMap<String, i64> = row.to().map_err(|_| "invalid Neo4j scalar shape")?;
    if scalar_value(&values)? != 42 {
        return Err("Neo4j scalar probe mismatch");
    }
    if rows
        .next(&mut transaction)
        .await
        .map_err(|_| "Neo4j scalar completion failed")?
        .is_some()
    {
        return Err("Neo4j scalar probe returned multiple rows");
    }
    transaction
        .rollback()
        .await
        .map_err(|_| "Neo4j probe rollback failed")?;
    println!(
        "{}",
        serde_json::json!({"event":"neo4j-driver-probe", "version":version,
        "edition":edition,"driver":"neo4rs","driver_version":"0.9.0-rc.10",
        "scalar":42,"benchmark_complete":false})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_alias_does_not_change_the_result() {
        for alias in ["count", "count(*)", "résultat_🦀"] {
            assert_eq!(scalar_value(&BTreeMap::from([(alias.into(), 42)])), Ok(42));
        }
    }

    #[test]
    fn empty_and_multi_column_results_are_not_counts() {
        assert!(scalar_value(&BTreeMap::new()).is_err());
        assert!(scalar_value(&BTreeMap::from([("a".into(), 1), ("b".into(), 2)])).is_err());
    }
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() != Some("probe") {
        eprintln!("usage: grust-lsqb-neo4j probe (connection via NEO4J_* environment)");
        std::process::exit(2);
    }
    let result = tokio::time::timeout(Duration::from_secs(20), probe()).await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            eprintln!("neo4j qualification: {error}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("neo4j qualification: probe deadline exceeded");
            std::process::exit(1);
        }
    }
}
