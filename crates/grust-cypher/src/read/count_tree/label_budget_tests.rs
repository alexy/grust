use super::*;
use crate::read_budget::{ReadExecutionBudgetLimits, with_budget};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn limits(work: usize) -> ReadExecutionBudgetLimits {
    ReadExecutionBudgetLimits {
        max_candidate_work: work,
        max_intermediate_bytes: 1_000_000,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    }
}

#[test]
fn borrowed_forest_labels_are_charged_before_lookup_on_hits_misses_and_empty_graphs() {
    let label = "x".repeat(16 * 1024);
    let source = format!("MATCH (:A)-[:R]->(:B)-[:S]->(:{label}) RETURN count(*) AS c");
    let query = parse_query(&source).unwrap();
    crate::semantics::analyze(&query).unwrap();
    let params = CypherParameters::new();
    let graph = |last_label: &str| {
        Graph::new(
            vec![
                Node::new("A", "a", Props::new()),
                Node::new("B", "b", Props::new()),
                Node::new(last_label, "c", Props::new()),
            ],
            vec![
                Edge::new("R", "a", "b", Props::new()),
                Edge::new("S", "b", "c", Props::new()),
            ],
        )
    };
    for (graph, expected) in [
        (Graph::default(), 0),
        (graph("Other"), 0),
        (graph(&label), 1),
    ] {
        let index = TypedGraphIndex::new(Arc::new(graph)).unwrap();
        let error = with_budget(limits(512), || try_execute(&index, &query, &params)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("looking up count forest candidate labels"),
            "{error}"
        );
        let table = with_budget(limits(100_000), || {
            execute_read_query_indexed(&index, &query, &params)
        })
        .unwrap();
        assert_eq!(table.rows, vec![vec![Value::Int(expected)]]);
        assert_eq!(
            table,
            run_read_query(index.graph(), &source, &params).unwrap()
        );
    }
}
