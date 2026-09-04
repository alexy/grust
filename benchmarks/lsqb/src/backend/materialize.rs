use std::collections::{BTreeMap, BTreeSet};

use grust_core::{EdgeQuery, Graph, GraphStore, Label, Start, Traversal, Value};
use serde::Serialize;

use crate::queries::QueryCase;

/// Materialize a backend through its portable read surface and execute one
/// shared Cypher count query over the result.
///
/// Materialization is deliberately part of this call: a caller timing the
/// returned future measures every per-label node scan, the edge scan,
/// cross-backend identity validation, and shared-reference execution. The
/// source graph supplies only the finite set of labels to scan and the exact
/// identity oracle; query evaluation uses the graph read back from `store`.
pub async fn materialized_count<S>(
    store: &S,
    source: &Graph,
    case: &QueryCase,
) -> Result<i64, String>
where
    S: GraphStore + ?Sized,
{
    let mut nodes = Vec::with_capacity(source.nodes.len());
    let labels = source
        .nodes
        .iter()
        .map(|node| node.label.clone())
        .collect::<BTreeSet<Label>>();

    for label in labels {
        let traversal = Traversal {
            start: Start::NodesByLabel(label.clone()),
            steps: Vec::new(),
            limit: None,
        };
        let mut label_nodes = store.traverse(traversal).await.map_err(|error| {
            format!(
                "{}: backend materialization failed while scanning node label '{}': {error}",
                case.id, label
            )
        })?;
        nodes.append(&mut label_nodes);
    }

    let edges = store
        .get_edges(EdgeQuery::default())
        .await
        .map_err(|error| {
            format!(
                "{}: backend materialization failed while scanning all edges: {error}",
                case.id
            )
        })?;
    let materialized = Graph::new(nodes, edges);
    validate_graph_identity(source, &materialized, &case.id)?;

    let table = grust_cypher::read::run_read_query(
        &materialized,
        &case.executable,
        &grust_cypher::CypherParameters::new(),
    )
    .map_err(|error| {
        format!(
            "{}: shared query execution over the validated backend materialization failed: {error}",
            case.id
        )
    })?;

    if table.columns.len() != 1 || table.rows.len() != 1 || table.rows[0].len() != 1 {
        let row_widths = table.rows.iter().map(Vec::len).collect::<Vec<_>>();
        return Err(format!(
            "{}: expected one scalar result cell, received {} column(s), {} row(s), and row width(s) {row_widths:?}",
            case.id,
            table.columns.len(),
            table.rows.len(),
        ));
    }
    match &table.rows[0][0] {
        Value::Int(value) => Ok(*value),
        value => Err(format!(
            "{}: expected an i64 count result, received {value:?}",
            case.id
        )),
    }
}

fn validate_graph_identity(
    source: &Graph,
    materialized: &Graph,
    case_id: &str,
) -> Result<(), String> {
    validate_multiset("node", &source.nodes, &materialized.nodes, case_id)?;
    validate_multiset("edge", &source.edges, &materialized.edges, case_id)
}

fn validate_multiset<T: Serialize>(
    kind: &str,
    source: &[T],
    materialized: &[T],
    case_id: &str,
) -> Result<(), String> {
    let source_counts = canonical_counts(kind, "source", source, case_id)?;
    let materialized_counts = canonical_counts(kind, "backend", materialized, case_id)?;
    if source_counts == materialized_counts {
        return Ok(());
    }

    let missing = multiset_difference(&source_counts, &materialized_counts);
    let unexpected = multiset_difference(&materialized_counts, &source_counts);
    Err(format!(
        "{case_id}: backend {kind} multiset differs from the source: source has {}, backend materialized {}; missing {}; unexpected {}",
        source.len(),
        materialized.len(),
        format_difference(&missing),
        format_difference(&unexpected),
    ))
}

fn canonical_counts<T: Serialize>(
    kind: &str,
    side: &str,
    values: &[T],
    case_id: &str,
) -> Result<BTreeMap<String, usize>, String> {
    let mut counts = BTreeMap::new();
    for (index, value) in values.iter().enumerate() {
        let canonical = serde_json::to_string(value).map_err(|error| {
            format!(
                "{case_id}: cannot serialize {side} {kind} at index {index} for exact identity validation: {error}"
            )
        })?;
        *counts.entry(canonical).or_insert(0) += 1;
    }
    Ok(counts)
}

fn multiset_difference(
    expected: &BTreeMap<String, usize>,
    actual: &BTreeMap<String, usize>,
) -> Vec<(String, usize)> {
    expected
        .iter()
        .filter_map(|(identity, expected_count)| {
            let actual_count = actual.get(identity).copied().unwrap_or(0);
            expected_count
                .checked_sub(actual_count)
                .filter(|difference| *difference != 0)
                .map(|difference| (identity.clone(), difference))
        })
        .collect()
}

fn format_difference(difference: &[(String, usize)]) -> String {
    if difference.is_empty() {
        return "none".to_string();
    }
    difference
        .iter()
        .take(3)
        .map(|(identity, count)| format!("{count} x {identity}"))
        .chain(
            (difference.len() > 3)
                .then(|| format!("and {} more distinct identities", difference.len() - 3)),
        )
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use grust_core::{Edge, Graph, GraphStore, Node, Props, Value};
    use grust_memory::MemoryGraphStore;

    use super::{materialized_count, validate_multiset};
    use crate::queries::QueryCase;

    fn graph() -> Graph {
        Graph::new(
            vec![
                Node::new("Person", "p1", Props::new()),
                Node::new("Person", "p2", Props::new()),
                Node::new("City", "c1", Props::new()),
            ],
            vec![Edge::new("KNOWS", "p1", "p2", Props::new())],
        )
    }

    fn query(executable: &str) -> QueryCase {
        QueryCase {
            id: "test-count".to_string(),
            executable: executable.to_string(),
            source_sha256: "test-source".to_string(),
            expected_count: 0,
            claim: "test query".to_string(),
        }
    }

    #[tokio::test]
    async fn materializes_all_labels_and_executes_the_shared_count() {
        let source = graph();
        let store = MemoryGraphStore::new();
        store.put_graph(&source).await.expect("load graph");

        let count = materialized_count(
            &store,
            &source,
            &query("MATCH (n:Person) RETURN count(*) AS count"),
        )
        .await
        .expect("materialize and count");

        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn rejects_backend_property_drift_before_query_execution() {
        let source = graph();
        let mut changed = graph();
        changed.nodes[0]
            .props
            .insert("changed".to_string(), Value::Bool(true));
        let store = MemoryGraphStore::new();
        store.put_graph(&changed).await.expect("load changed graph");

        let error = materialized_count(
            &store,
            &source,
            &query("MATCH (n) RETURN count(*) AS count"),
        )
        .await
        .expect_err("identity drift must fail");

        assert!(error.contains("backend node multiset differs from the source"));
        assert!(error.contains("missing 1 x"));
        assert!(error.contains("unexpected 1 x"));
    }

    #[test]
    fn multiset_validation_ignores_order_but_preserves_duplicate_counts() {
        let source = vec!["one", "two", "two"];
        let reordered = vec!["two", "one", "two"];
        validate_multiset("value", &source, &reordered, "test").expect("same multiset");

        let missing_duplicate = vec!["two", "one"];
        let error = validate_multiset("value", &source, &missing_duplicate, "test")
            .expect_err("missing duplicate must fail");
        assert!(error.contains("missing 1 x \"two\""));
    }
}
