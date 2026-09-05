use super::*;
use crate::read_budget::{ReadExecutionBudgetLimits, with_budget};
use std::time::{Duration, Instant};

fn node(id: &str) -> Node {
    Node::new("N", id, Props::new())
}
fn edge(from: &str, to: &str) -> Edge {
    Edge::new("T", from, to, Props::new()).with_id("duplicate")
}
fn count(graph: &Graph, source: &str) -> i64 {
    let result = run_read_query(graph, source, &CypherParameters::new()).unwrap();
    let [row] = result.rows.as_slice() else {
        panic!("{source}: {result:?}")
    };
    let [Value::Int(value)] = row.as_slice() else {
        panic!("{source}: {result:?}")
    };
    *value
}

#[test]
fn one_loop_cannot_fill_triangle_but_three_physical_loops_have_six_permutations() {
    let source = "MATCH (a)-[:T]->(b)-[:T]->(c)-[:T]->(a) RETURN count(*)";
    for (loops, expected) in [(1, 0), (2, 0), (3, 6)] {
        let graph = Graph::new(
            vec![node("x")],
            (0..loops).map(|_| edge("x", "x")).collect(),
        );
        assert_eq!(count(&graph, source), expected);
        assert_eq!(count(&graph, &source.replace("->", "-")), expected);
        assert_eq!(
            count(&graph, &source.replace("MATCH (a)", "MATCH p=(a)")),
            expected
        );
    }
}

#[test]
fn comma_paths_share_scope_and_separate_matches_reset_it() {
    for (parallel, joined, split) in [(1, 0, 1), (2, 2, 4)] {
        let graph = Graph::new(
            vec![node("a"), node("b")],
            (0..parallel).map(|_| edge("a", "b")).collect(),
        );
        for source in [
            "MATCH (a)-[:T]->(b), (a)-[:T]->(b) RETURN count(*)",
            "MATCH (a)-[r:T]->(b), (a)-[s:T]->(b) RETURN count(*)",
            "MATCH p=(a)-[:T]->(b), q=(a)-[:T]->(b) RETURN count(*)",
            "MATCH ()-[:T]->(), ()-[:T]->() RETURN count(*)",
        ] {
            assert_eq!(count(&graph, source), joined, "{source}");
        }
        for source in [
            "MATCH (a)-[:T]->(b) MATCH (a)-[:T]->(b) RETURN count(*)",
            "MATCH (a)-[r:T]->(b) WITH a,b MATCH (a)-[s:T]->(b) RETURN count(*)",
        ] {
            assert_eq!(count(&graph, source), split, "{source}");
        }
    }
}

#[test]
fn named_relationships_preserve_slot_identity_through_match_and_with_aliases() {
    for explicit_ids in [false, true] {
        let edges = (0..2)
            .map(|_| {
                let edge = Edge::new("T", "a", "b", Props::new());
                if explicit_ids {
                    edge.with_id("duplicate")
                } else {
                    edge
                }
            })
            .collect();
        let graph = Graph::new(vec![node("a"), node("b")], edges);
        assert_eq!(
            count(
                &graph,
                "MATCH (a)-[r:T]->(b), (a)-[r:T]->(b) RETURN count(*)"
            ),
            0
        );
        for source in [
            "MATCH (a)-[r:T]->(b) MATCH (a)-[r:T]->(b) RETURN count(*)",
            "MATCH (a)-[r:T]->(b) WITH a,b,r AS s MATCH (a)-[s:T]->(b) RETURN count(*)",
            "MATCH (a)-[r:T]->(b) WITH r MATCH ()-[r:T]->() RETURN count(*)",
        ] {
            assert_eq!(count(&graph, source), 2, "{source}");
        }
        assert_eq!(
            count(
                &graph,
                "MATCH (a {id:'b'}) OPTIONAL MATCH (a)-[r:T]->() WITH r MATCH ()-[r:T]->() RETURN count(*)"
            ),
            0
        );
    }
}

#[test]
fn repeated_relationship_lists_fail_closed_instead_of_overwriting_identity() {
    let graph = Graph::new(vec![node("a"), node("b")], vec![edge("a", "b")]);
    for source in [
        "MATCH (a)-[r:T*1..1]->(b) MATCH (a)-[r:T*1..1]->(b) RETURN count(*)",
        "MATCH (a)-[r:T*1..1]->(b), (a)-[r:T*1..1]->(b) RETURN count(*)",
        "MATCH (a)-[r:T*1..1]->(b) WITH a,b,r AS s MATCH (a)-[s:T*1..1]->(b) RETURN count(*)",
        "MATCH (a)-[r:T]->(b) MATCH shortestPath((a)-[r:T*1..1]->(b)) RETURN count(*)",
    ] {
        let error = run_read_query(&graph, source, &CypherParameters::new()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("relationship list is unsupported"),
            "{source}: {error}"
        );
    }
    assert_eq!(
        count(&graph, "MATCH (a)-[r:T*1..1]->(b) RETURN count(r)"),
        1
    );
}

#[test]
fn indexed_and_contiguous_edge_iteration_use_the_same_physical_identity() {
    let graph = Graph::new(
        vec![node("a"), node("b")],
        vec![edge("a", "b"), edge("a", "b")],
    );
    // Inline start properties keep this selective fixed query on the raw scan
    // branch; the unfiltered query builds adjacency instead.
    for source in [
        "MATCH (a {id:'a'})-[:T]->(b)-[:T]-(a) RETURN count(*)",
        "MATCH (a)-[:T]->(b)-[:T]-(a) RETURN count(*)",
    ] {
        assert_eq!(count(&graph, source), 2);
    }
}

#[test]
fn fixed_and_variable_segments_and_comma_paths_share_identity() {
    let graph = Graph::new(vec![node("a"), node("b")], vec![edge("a", "b")]);
    for source in [
        "MATCH (a)-[:T]->(b)-[:T*1..1]-(c) RETURN count(*)",
        "MATCH (a)-[:T*1..1]-(b)-[:T]->(c) RETURN count(*)",
        "MATCH (a)-[:T]->(b), (a)-[:T*1..1]->(b) RETURN count(*)",
        "MATCH (a)-[:T*1..1]->(b), (a)-[:T]->(b) RETURN count(*)",
        "MATCH (a)-[:T*1..1]->(b), (a)-[:T*1..1]->(b) RETURN count(*)",
    ] {
        assert_eq!(count(&graph, source), 0, "{source}");
    }
    for source in [
        "MATCH (a)-[:T]->(b) MATCH (a)-[:T*1..1]->(b) RETURN count(*)",
        "MATCH (a)-[:T*0..0]->(same), (a)-[:T]->(b) RETURN count(*)",
    ] {
        assert_eq!(count(&graph, source), 1, "{source}");
    }
    // Existing variable-length node-simple semantics are unchanged.
    assert_eq!(count(&graph, "MATCH (a)-[:T*1..2]-(b) RETURN count(*)"), 2);
}

#[test]
fn optional_failed_uniqueness_null_pads_and_starts_a_new_scope() {
    let graph = Graph::new(vec![node("a"), node("b")], vec![edge("a", "b")]);
    assert_eq!(
        count(
            &graph,
            "MATCH (a {id:'a'}) OPTIONAL MATCH (a)-[r:T]->(b), (a)-[s:T]->(b) RETURN count(*)"
        ),
        1
    );
    assert_eq!(
        count(
            &graph,
            "MATCH (a {id:'a'}) OPTIONAL MATCH (a)-[r:T]->(b), (a)-[s:T]->(b) RETURN count(r)"
        ),
        0
    );
    assert_eq!(
        count(
            &graph,
            "MATCH (a)-[:T]->(b) OPTIONAL MATCH (a)-[r:T]->(b) RETURN count(r)"
        ),
        1
    );
    assert_eq!(
        count(
            &graph,
            "MATCH (a {id:'a'}) OPTIONAL MATCH (a)-[r:T]->(b), (a)-[s:T]->(b) WITH a,r WHERE r IS NULL MATCH (a)-[:T]->(b) RETURN count(*)"
        ),
        1
    );
    let result = run_read_query(
        &graph,
        "MATCH (a)-[r:T]->(b) RETURN *",
        &CypherParameters::new(),
    )
    .unwrap();
    assert_eq!(result.columns, vec!["a", "b", "r"]);
}

#[test]
fn shortest_selection_is_preserved_then_checked_for_scope_reuse() {
    let graph = Graph::new(
        vec![node("a"), node("b"), node("c")],
        vec![edge("a", "b"), edge("a", "c"), edge("c", "b")],
    );
    let source =
        "MATCH (a {id:'a'})-[:T]->(b {id:'b'}), p=shortestPath((a)-[:T*1..2]->(b)) RETURN count(*)";
    // Do not replace the selected a->b shortest path with longer a->c->b.
    assert_eq!(count(&graph, source), 0);
    assert_eq!(count(&graph, &source.replace(", p=", " MATCH p=")), 1);
    assert_eq!(
        count(
            &graph,
            "MATCH p=shortestPath((a {id:'a'})-[:T*1..2]->(b {id:'b'})), (a)-[:T]->(b) RETURN count(*)"
        ),
        0
    );
    let parallel = Graph::new(
        vec![node("a"), node("b")],
        vec![edge("a", "b"), edge("a", "b")],
    );
    let source = "MATCH (a)-[:T]->(b), p=allShortestPaths((a)-[:T*1..1]->(b)) RETURN count(*)";
    assert_eq!(count(&parallel, source), 2);
    // Single keeps the original first-found tie before applying uniqueness.
    assert_eq!(
        count(
            &parallel,
            &source.replace("allShortestPaths", "shortestPath")
        ),
        1
    );
}

#[test]
fn generated_three_hop_walks_match_an_independent_physical_slot_oracle() {
    for seed in 0..40usize {
        let graph = Graph::new(
            (0..3).map(|i| node(&i.to_string())).collect(),
            (0..7)
                .map(|e| {
                    edge(
                        &((seed + e * 2) % 3).to_string(),
                        &((seed * 2 + e) % 3).to_string(),
                    )
                })
                .collect(),
        );
        let mut expected = 0;
        for (i, first) in graph.edges.iter().enumerate() {
            for (j, second) in graph.edges.iter().enumerate() {
                for (k, third) in graph.edges.iter().enumerate() {
                    if i != j
                        && i != k
                        && j != k
                        && first.to == second.from
                        && second.to == third.from
                        && third.to == first.from
                    {
                        expected += 1;
                    }
                }
            }
        }
        assert_eq!(
            count(
                &graph,
                "MATCH (a)-[:T]->(b)-[:T]->(c)-[:T]->(a) RETURN count(*)"
            ),
            expected
        );
    }
}

fn limits() -> ReadExecutionBudgetLimits {
    ReadExecutionBudgetLimits {
        max_candidate_work: 1000,
        max_intermediate_bytes: 1000,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    }
}

#[test]
fn scope_memory_work_and_deadline_are_accounted_before_copying_or_searching() {
    let mut empty = MatchRow {
        bindings: Row::new(),
        slots: Vec::new(),
    };
    assert!(
        with_budget(
            ReadExecutionBudgetLimits {
                max_intermediate_bytes: std::mem::size_of::<usize>() - 1,
                ..limits()
            },
            || empty.record(&[0])
        )
        .is_err()
    );
    assert_eq!(empty.slots.capacity(), 0);
    with_budget(
        ReadExecutionBudgetLimits {
            max_intermediate_bytes: std::mem::size_of::<usize>(),
            ..limits()
        },
        || empty.record(&[0]),
    )
    .unwrap();
    assert_eq!(empty.slots(), &[0]);
    let mut row = MatchRow {
        bindings: Row::new(),
        slots: vec![0, 1, 2],
    };
    assert!(
        with_budget(
            ReadExecutionBudgetLimits {
                max_candidate_work: 2,
                ..limits()
            },
            || row.contains(3)
        )
        .is_err()
    );
    assert!(
        with_budget(
            ReadExecutionBudgetLimits {
                max_intermediate_bytes: 1,
                ..limits()
            },
            || row.copy("copying test scope")
        )
        .is_err()
    );
    assert!(
        with_budget(
            ReadExecutionBudgetLimits {
                max_intermediate_bytes: 1,
                ..limits()
            },
            || row.record(&[3])
        )
        .is_err()
    );
    assert_eq!(row.slots(), &[0, 1, 2]);
    assert!(
        with_budget(
            ReadExecutionBudgetLimits {
                deadline: Instant::now(),
                ..limits()
            },
            || row.contains(3)
        )
        .is_err()
    );
    let graph = Graph::new(
        vec![node("a"), node("b")],
        vec![edge("a", "b"), edge("a", "b")],
    );
    let query = "MATCH (a)-[:T]->(b), (a)-[:T]->(b) RETURN count(*) LIMIT 1";
    for (work, bytes) in [(1, 10000), (10000, 1)] {
        let policy = crate::ReadQueryPolicy {
            max_candidate_work: work,
            max_intermediate_bytes: bytes,
            ..crate::ReadQueryPolicy::default()
        };
        assert!(
            crate::run_bounded_read_query(&graph, query, &CypherParameters::new(), &policy)
                .is_err()
        );
    }
}
