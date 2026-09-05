use super::*;
use grust_core::{Edge, Node, Props};
use std::{sync::Arc, time::Duration};

fn index() -> TypedGraphIndex {
    TypedGraphIndex::new(Arc::new(Graph::new(
        vec![
            Node::new("A", "a", Props::new()),
            Node::new("B", "b", Props::new()),
            Node::new("C", "c", Props::new()),
        ],
        vec![
            Edge::new("R", "a", "b", Props::new()),
            Edge::new("R", "a", "b", Props::new()),
            Edge::new("S", "b", "c", Props::new()),
        ],
    )))
    .unwrap()
}

#[test]
fn indexed_classification_matches_the_executed_count_proofs() {
    let index = index();
    let params = CypherParameters::from([("limit".into(), Value::Int(1))]);
    for (source, expected) in [
        (
            "MATCH ()-[:R]->()-[:S]->() RETURN count(*)",
            IndexedReadPlan::CountFactorized,
        ),
        (
            "MATCH ()-[:R]->(b), (b)-[:S]->() RETURN count(*) LIMIT 1",
            IndexedReadPlan::CountFactorized,
        ),
        (
            "MATCH ()-[:R]->(b) MATCH (b)-[:S]->() RETURN count(*) LIMIT 1",
            IndexedReadPlan::CountFactorized,
        ),
        (
            "MATCH (), () RETURN count(*) LIMIT 0",
            IndexedReadPlan::CountFactorized,
        ),
        (
            "MATCH () RETURN count(*) SKIP 1 LIMIT 1",
            IndexedReadPlan::CountFactorized,
        ),
        (
            "MATCH (a)-[:R]->(b), (b)-[:S]->(a) RETURN count(*)",
            IndexedReadPlan::ClausePipeline,
        ),
        (
            "MATCH (a)-[:R]->(a) RETURN count(*)",
            IndexedReadPlan::CountFactorized,
        ),
        (
            "MATCH (a)-[:R]->(b), (a)-[:S]->(b) RETURN count(*)",
            IndexedReadPlan::ClausePipeline,
        ),
        (
            "MATCH ()-[:R]->()-[:R]->() RETURN count(*)",
            IndexedReadPlan::ClausePipeline,
        ),
        (
            "MATCH (n) WHERE n.label = 'A' RETURN count(*)",
            IndexedReadPlan::ClausePipeline,
        ),
        (
            "MATCH (n) RETURN count(*) LIMIT $limit",
            IndexedReadPlan::ClausePipeline,
        ),
        (
            "MATCH (n) RETURN count(n) LIMIT 1",
            IndexedReadPlan::CountFactorized,
        ),
        (
            "MATCH (n) OPTIONAL MATCH (n)-[:R]->() RETURN count(*)",
            IndexedReadPlan::CountFactorized,
        ),
        (
            "MATCH (n) RETURN count(*) UNION ALL MATCH (n) RETURN count(*)",
            IndexedReadPlan::CountFactorized,
        ),
    ] {
        let query = parse_query(source).unwrap();
        crate::semantics::analyze(&query).unwrap();
        assert_eq!(
            classify_indexed_read_query(&query).unwrap(),
            expected,
            "{source}"
        );
        let factorized = count_scan::try_execute(&index, &query, &params)
            .unwrap()
            .or_else(|| count_tree::try_execute(&index, &query, &params).unwrap());
        assert_eq!(
            factorized.is_some(),
            expected == IndexedReadPlan::CountFactorized,
            "{source}"
        );
        let result = run_read_query_indexed(&index, source, &params).unwrap();
        assert_eq!(
            result,
            run_read_query(index.graph(), source, &params).unwrap(),
            "{source}"
        );
        if let Some(factorized) = factorized {
            assert_eq!(result, factorized, "{source}");
        }
    }
}

#[test]
fn indexed_text_entrypoint_retains_parser_semantics_and_graph_selection() {
    let index = index();
    let params = CypherParameters::new();
    for source in [
        "MATCH (",
        "MATCH (n) RETURN missing",
        "USE other MATCH (n) RETURN count(*)",
    ] {
        assert_eq!(
            run_read_query_indexed(&index, source, &params)
                .unwrap_err()
                .to_string(),
            run_read_query(index.graph(), source, &params)
                .unwrap_err()
                .to_string(),
            "{source}"
        );
    }
    for source in [
        "USE default MATCH (n) RETURN count(*)",
        "MATCH (:Missing {key:$missing}) RETURN count(*)",
    ] {
        assert_eq!(
            run_read_query_indexed(&index, source, &params).unwrap(),
            run_read_query(index.graph(), source, &params).unwrap(),
            "{source}"
        );
    }
}

#[test]
fn indexed_classification_and_execution_both_charge_active_budgets() {
    let index = index();
    let source = "MATCH ()-[:R]->()-[:S]->() RETURN count(*) LIMIT 1";
    let query = parse_query(source).unwrap();
    assert_eq!(
        classify_indexed_read_query(&query).unwrap(),
        IndexedReadPlan::CountFactorized
    );
    for (work, bytes, timeout) in [
        (1, 100_000, Duration::from_secs(5)),
        (100_000, 1, Duration::from_secs(5)),
        (100_000, 100_000, Duration::ZERO),
    ] {
        let limits = read_budget::ReadExecutionBudgetLimits {
            max_candidate_work: work,
            max_intermediate_bytes: bytes,
            max_range_items: 100,
            deadline: std::time::Instant::now() + timeout,
        };
        assert!(read_budget::with_budget(limits, || classify_indexed_read_query(&query)).is_err());
        // A successful preflight cannot give this call a cached free plan.
        assert!(
            read_budget::with_budget(limits, || {
                run_read_query_indexed(&index, source, &CypherParameters::new())
            })
            .is_err()
        );
    }
}

#[test]
fn indexed_classification_does_not_override_query_policy() {
    use crate::read_policy::{ReadQueryPolicy, run_bounded_read_query_indexed};
    let index = index();
    let params = CypherParameters::new();
    let policy = ReadQueryPolicy::default();
    for source in [
        "MATCH () RETURN count(*)",
        "MATCH () RETURN count(*) LIMIT 0",
    ] {
        let query = parse_query(source).unwrap();
        assert_eq!(
            classify_indexed_read_query(&query).unwrap(),
            IndexedReadPlan::CountFactorized
        );
        assert!(
            run_bounded_read_query_indexed(&index, source, &params, &policy)
                .unwrap_err()
                .to_string()
                .contains("positive literal LIMIT")
        );
    }
    let source = "MATCH ()-[:R]->()-[:S]->() RETURN count(*) LIMIT 1";
    for policy in [
        ReadQueryPolicy {
            max_candidate_work: 1,
            ..policy
        },
        ReadQueryPolicy {
            max_execution_time: Duration::from_nanos(1),
            ..policy
        },
    ] {
        assert!(run_bounded_read_query_indexed(&index, source, &params, &policy).is_err());
    }
}
