//! Reusing mandatory-label candidates must not weaken predicates or padding.

use super::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn index(nodes: usize) -> TypedGraphIndex {
    let nodes = (0..nodes)
        .map(|vertex| Node::new("Other", vertex.to_string(), Props::new()))
        .collect();
    TypedGraphIndex::new(Arc::new(Graph::new(nodes, Vec::new()))).unwrap()
}

fn limits(work: usize, bytes: usize) -> read_budget::ReadExecutionBudgetLimits {
    read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: work,
        max_intermediate_bytes: bytes,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    }
}

fn query(source: &str) -> Query {
    let query = parse_query(source).unwrap();
    crate::semantics::analyze(&query).unwrap();
    query
}

#[test]
fn missing_labels_skip_branch_visits_but_keep_full_weight_initialization() {
    const VERTICES: usize = 4096;
    let index = index(VERTICES);
    let query = query("MATCH (:Missing)-[:R]->(:Missing)-[:S]->(:Missing) RETURN count(*)");
    let params = CypherParameters::new();
    // One pattern + two relationships; each of three roles pays a label
    // lookup and initializes V weights; the single root still sums all V.
    let fixed = 3 + 3 * (2 * "Missing".len() + 1);
    let work = 4 * VERTICES + fixed;
    let table = read_budget::with_budget(limits(work, 1_000_000), || {
        let table = try_execute(&index, &query, &params)?.unwrap();
        let error =
            read_budget::charge_candidate_work(1, "probing branch candidate work").unwrap_err();
        assert!(
            error
                .to_string()
                .ends_with("while probing branch candidate work")
        );
        Ok(table)
    })
    .unwrap();
    assert_eq!(table.rows, vec![vec![Value::Int(0)]]);
    for (work, bytes, context) in [
        (3 + VERTICES - 1, 1_000_000, "initializing count weights"),
        (work, 3 * 512 + 8 * VERTICES - 1, "allocating count weights"),
        (work - 1, 1_000_000, "summing count roots"),
    ] {
        let error =
            read_budget::with_budget(limits(work, bytes), || try_execute(&index, &query, &params))
                .unwrap_err();
        assert!(error.to_string().ends_with(context), "{error}");
    }
}

#[test]
fn present_and_unlabeled_candidates_are_still_charged_even_with_zero_degree() {
    let index = index(1);
    let params = CypherParameters::new();
    for source in [
        "MATCH (:Other)-[:R]->(:Other) RETURN count(*)",
        "MATCH ()-[:R]->() RETURN count(*)",
    ] {
        let query = query(source);
        let mut found_branch_refusal = false;
        let mut found_success = false;
        for work in 0..128 {
            match read_budget::with_budget(limits(work, 100_000), || {
                try_execute(&index, &query, &params)
            }) {
                Err(error) => {
                    found_branch_refusal |= error.to_string().ends_with("combining count branches");
                }
                Ok(Some(table)) => {
                    assert_eq!(table.rows, vec![vec![Value::Int(0)]]);
                    found_success = true;
                    break;
                }
                Ok(None) => panic!("forest was not admitted: {source}"),
            }
        }
        assert!(found_branch_refusal && found_success, "{source}");
    }
}

#[test]
fn reused_candidates_keep_later_labels_properties_and_optional_null_padding() {
    let mut a = Node::new("A", "a", Props::new());
    a.props.insert("ok".into(), Value::Bool(true));
    let mut nodes = vec![
        a,
        Node::new("B", "b", Props::new()),
        Node::new("C", "c", Props::new()),
    ];
    nodes.extend((0..16).map(|id| Node::new("Other", format!("other-{id}"), Props::new())));
    let graph = Graph::new(
        nodes,
        vec![
            Edge::new("R", "a", "b", Props::new()),
            Edge::new("R", "a", "b", Props::new()),
            Edge::new("S", "a", "c", Props::new()),
            Edge::new("O", "other-0", "b", Props::new()),
        ],
    );
    let index = TypedGraphIndex::new(Arc::new(graph)).unwrap();
    let params = CypherParameters::new();
    for (source, expected) in [
        (
            "MATCH (a)-[:R]->(:B) MATCH (a:A {ok:true})-[:S]->(:C) RETURN count(*)",
            2,
        ),
        (
            "MATCH (a:A)-[:R]->(:B) MATCH (a:Missing)-[:S]->(:C) RETURN count(*)",
            0,
        ),
        (
            "MATCH (a:A {ok:false})-[:R]->(:B), (a)-[:S]->(:C) RETURN count(*)",
            0,
        ),
        (
            "MATCH (a:A)-[:R]->(:B), (a)-[:S]->(:C) OPTIONAL MATCH (a)-[:O]->() RETURN count(*)",
            2,
        ),
        (
            "MATCH (a:A)-[:R]->(:B), (a)-[:S]->(:C) OPTIONAL MATCH (a:Missing)-[:O]->() RETURN count(*)",
            2,
        ),
        (
            "MATCH (a:A {ok:false})-[:R]->(:B), (a)-[:S]->(:C) OPTIONAL MATCH (a)-[:O]->() RETURN count(*)",
            0,
        ),
    ] {
        let query = query(source);
        let actual = try_execute(&index, &query, &params).unwrap().unwrap();
        assert_eq!(actual.rows, vec![vec![Value::Int(expected)]], "{source}");
        assert_eq!(
            actual,
            execute_read_query(index.graph(), &query, &params).unwrap(),
            "{source}"
        );
    }
}

#[test]
fn labeled_multigraph_branches_match_reference_for_every_direction() {
    let nodes = vec![
        Node::new("X", "x", Props::new()),
        Node::new("X", "y", Props::new()),
        Node::new("Other", "z", Props::new()),
    ];
    let edges = [
        ("R", "x", "x"),
        ("R", "x", "y"),
        ("R", "x", "y"),
        ("R", "y", "x"),
        ("R", "z", "x"),
        ("S", "x", "x"),
        ("S", "y", "x"),
        ("S", "x", "y"),
        ("S", "z", "y"),
    ]
    .into_iter()
    .map(|(label, from, to)| Edge::new(label, from, to, Props::new()))
    .collect();
    let index = TypedGraphIndex::new(Arc::new(Graph::new(nodes, edges))).unwrap();
    let params = CypherParameters::new();
    for first in ["-[:R]->", "<-[:R]-", "-[:R]-"] {
        for second in ["-[:S]->", "<-[:S]-", "-[:S]-"] {
            let source = format!("MATCH (:X){first}(:X){second}(:X) RETURN count(*)");
            let query = query(&source);
            assert_eq!(
                try_execute(&index, &query, &params).unwrap().unwrap(),
                execute_read_query(index.graph(), &query, &params).unwrap(),
                "{source}"
            );
        }
    }
}
