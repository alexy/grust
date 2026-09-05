use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use grust_core::{Props, Value};
use grust_lsqb_runner::dataset;
use neo4rs::{BoltType, Graph, Query, query};

pub(super) async fn scalar(
    graph: &Graph,
    statement: Query,
    commit: bool,
) -> Result<i64, &'static str> {
    let mut tx = graph
        .start_txn()
        .await
        .map_err(|_| "Neo4j transaction failed")?;
    let mut stream = tx
        .execute(statement)
        .await
        .map_err(|_| "Neo4j query failed")?;
    let row = stream
        .next(&mut tx)
        .await
        .map_err(|_| "Neo4j result fetch failed")?
        .ok_or("Neo4j result has no row")?;
    let values: BTreeMap<String, i64> = row
        .to()
        .map_err(|_| "Neo4j result is not an integer scalar")?;
    let value = super::scalar_value(&values)?;
    if stream
        .next(&mut tx)
        .await
        .map_err(|_| "Neo4j result completion failed")?
        .is_some()
    {
        return Err("Neo4j result has multiple rows");
    }
    if commit {
        tx.commit().await.map_err(|_| "Neo4j commit failed")?;
    } else {
        tx.rollback().await.map_err(|_| "Neo4j rollback failed")?;
    }
    Ok(value)
}

fn identifier(value: &str) -> Result<String, &'static str> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err("dataset label is not an identifier");
    }
    Ok(format!("`{value}`"))
}

fn properties(props: &Props) -> Result<HashMap<String, BoltType>, &'static str> {
    props
        .iter()
        .map(|(key, value)| {
            let value = match value {
                Value::String(value) => value.clone().into(),
                Value::Int(value) => (*value).into(),
                Value::Bool(value) => (*value).into(),
                _ => return Err("unexpected property type in projected dataset"),
            };
            Ok((key.clone(), value))
        })
        .collect()
}

pub(super) async fn load(
    graph: &Graph,
    directory: &Path,
    mut progress: impl FnMut(usize, usize) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    if std::env::var("NEO4J_BENCHMARK_DISPOSABLE").as_deref() != Ok("1") {
        return Err("import requires explicit NEO4J_BENCHMARK_DISPOSABLE=1");
    }
    if scalar(graph, query("MATCH (n) RETURN count(n)"), false).await? != 0 {
        return Err("refusing to import into a nonempty Neo4j database");
    }
    let mut tx = graph
        .start_txn()
        .await
        .map_err(|_| "Neo4j schema transaction failed")?;
    tx.run(query("CREATE CONSTRAINT grust_benchmark_id IF NOT EXISTS FOR (n:__GrustBenchmark) REQUIRE n.id IS UNIQUE"))
        .await.map_err(|_| "Neo4j benchmark index creation failed")?;
    tx.commit()
        .await
        .map_err(|_| "Neo4j schema commit failed")?;
    let chunks = dataset::projected_dataset_chunks(directory, 10_000)
        .map_err(|_| "projected dataset chunks unavailable")?;
    for chunk in chunks {
        let chunk = chunk.map_err(|_| "projected dataset decoding failed")?;
        let mut groups = BTreeMap::<String, Vec<HashMap<String, BoltType>>>::new();
        for node in &chunk.nodes {
            let mut labels = identifier(node.label.as_str())?;
            if let Some(Value::String(kind)) = node.props.get("kind")
                && ["Post", "Comment"].contains(&kind.as_str())
            {
                labels.push(':');
                labels.push_str(&identifier(kind)?);
            }
            groups
                .entry(labels)
                .or_default()
                .push(properties(&node.props)?);
        }
        for (labels, rows) in groups {
            let count = rows.len() as i64;
            let statement = format!(
                "UNWIND $rows AS props CREATE (n:__GrustBenchmark:{labels}) SET n = props RETURN count(n)"
            );
            if scalar(graph, query(&statement).param("rows", rows), true).await? != count {
                return Err("Neo4j node import count differs");
            }
        }
        let mut groups = BTreeMap::<String, Vec<HashMap<String, BoltType>>>::new();
        for edge in &chunk.edges {
            let row = HashMap::from([
                ("from".into(), edge.from.as_str().into()),
                ("to".into(), edge.to.as_str().into()),
                (
                    "id".into(),
                    edge.id
                        .as_ref()
                        .ok_or("projected edge id missing")?
                        .as_str()
                        .into(),
                ),
            ]);
            groups
                .entry(identifier(edge.label.as_str())?)
                .or_default()
                .push(row);
        }
        for (label, rows) in groups {
            let count = rows.len() as i64;
            let statement = format!(
                "UNWIND $rows AS row MATCH (a:__GrustBenchmark {{id:row.from}}), (b:__GrustBenchmark {{id:row.to}}) CREATE (a)-[r:{label} {{id:row.id}}]->(b) RETURN count(r)"
            );
            if scalar(graph, query(&statement).param("rows", rows), true).await? != count {
                return Err("Neo4j edge import count differs");
            }
        }
        progress(chunk.nodes.len(), chunk.edges.len())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identifiers_cannot_inject_cypher() {
        assert_eq!(identifier("HAS_TAG"), Ok("`HAS_TAG`".into()));
        for invalid in ["", "Foo` DELETE n", "a:b", "a b"] {
            assert!(identifier(invalid).is_err());
        }
    }
    #[test]
    fn only_projected_scalar_properties_are_accepted() {
        assert!(properties(&Props::from([("id".into(), Value::from("Person:1"))])).is_ok());
        assert!(properties(&Props::from([("unsupported".into(), Value::Null)])).is_err());
    }
}
