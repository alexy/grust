use super::*;
use grust_core::{Edge, Node, Props};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const WEDGE: &str = "MATCH (a)-[:T]-(b)-[:T]-(c)-[:U]->(d) WHERE a <> c RETURN count(*)";
const Q6: &str = "MATCH (person1:Person)-[:KNOWS]-(person2:Person)-[:KNOWS]-(person3:Person)-[:HAS_INTEREST]->(tag:Tag) WHERE person1 <> person3 RETURN count(*) AS count";

pub(super) fn indexed(graph: Graph) -> TypedGraphIndex {
    TypedGraphIndex::new(Arc::new(graph)).unwrap()
}

pub(super) fn edge(label: &str, from: &str, to: &str) -> Edge {
    Edge::new(label, from, to, Props::new())
}

pub(super) fn compare(index: &TypedGraphIndex, source: &str, expected: i64) {
    let params = CypherParameters::new();
    let query = parse_query(source).unwrap();
    crate::semantics::analyze(&query).unwrap();
    assert_eq!(
        classify_indexed_read_query(&query).unwrap(),
        IndexedReadPlan::CountFactorized
    );
    let result = try_execute(index, &query).unwrap().expect("proven wedge");
    assert_eq!(result.rows, vec![vec![Value::Int(expected)]], "{source}");
    assert_eq!(
        result,
        run_read_query_indexed(index, source, &params).unwrap()
    );
    assert_eq!(
        result,
        run_read_query(index.graph(), source, &params).unwrap()
    );
}

#[test]
fn q6_example_counts_eight() {
    // Exact relevant subgraph of LSQB's pinned sfexample CSV fixture: five
    // people, two tags, six KNOWS edges and the two HAS_INTEREST edges.
    let nodes = (1..=5)
        .map(|id| Node::new("Person", format!("p{id}"), Props::new()))
        .chain((1..=2).map(|id| Node::new("Tag", format!("t{id}"), Props::new())))
        .collect();
    let mut edges: Vec<_> = [(1, 2), (1, 3), (1, 4), (2, 3), (3, 4), (4, 5)]
        .into_iter()
        .map(|(a, b)| edge("KNOWS", &format!("p{a}"), &format!("p{b}")))
        .collect();
    edges.extend([
        edge("HAS_INTEREST", "p2", "t1"),
        edge("HAS_INTEREST", "p4", "t2"),
    ]);
    let index = indexed(Graph::new(nodes, edges));
    compare(&index, Q6, 8);
    compare(&index, &format!("{Q6} LIMIT 1"), 8);
}

#[test]
fn hand_count_parallel_reciprocal_edges_and_self_loops() {
    let nodes = ["a", "b", "c"]
        .into_iter()
        .map(|id| Node::new("N", id, Props::new()))
        .chain([Node::new("D", "d", Props::new())])
        .collect();
    let mut edges = vec![
        edge("T", "a", "b"),
        edge("T", "b", "a"),
        edge("T", "b", "c"),
        edge("T", "c", "b"),
        edge("T", "b", "c"),
        edge("T", "b", "b"),
        edge("T", "b", "b"),
    ];
    for (from, count) in [("a", 5), ("b", 7), ("c", 4)] {
        edges.extend((0..count).map(|_| edge("U", from, "d")));
    }
    let index = indexed(Graph::new(nodes, edges));
    // Only center b contributes: c=a -> (7-2)*2*5 = 50;
    // c=b -> (7-2)*2*7 = 70; c=c -> (7-3)*3*4 = 48.
    let source = "MATCH (a:N)-[:T]-(b:N)-[:T]-(c:N)-[:U]->(:D) WHERE a <> c RETURN count(*)";
    compare(&index, source, 168);
    compare(&index, &source.replace("a <> c", "c <> a"), 168);
    // Restrict a and c to non-overlapping domains: there is no exclusion.
    let mut graph = index.graph().clone();
    graph.nodes[0].label = "A".into();
    graph.nodes[2].label = "C".into();
    compare(
        &indexed(graph),
        &source.replace("a:N", "a:A").replace("c:N", "c:C"),
        24,
    );
}

#[test]
fn graph_roles_may_alias_except_the_inequality_endpoints() {
    let index = indexed(Graph::new(
        vec![
            Node::new("N", "x", Props::new()),
            Node::new("N", "y", Props::new()),
        ],
        vec![
            edge("T", "x", "x"),
            edge("T", "x", "y"),
            edge("U", "x", "x"),
            edge("U", "y", "y"),
        ],
    ));
    // (a,b,c,d) = (y,x,x,x), (x,x,y,y). The self-loop is used once.
    compare(&index, WEDGE, 2);
    let index = indexed(Graph::new(
        vec![Node::new("N", "x", Props::new())],
        vec![
            edge("T", "x", "x"),
            edge("T", "x", "x"),
            edge("U", "x", "x"),
        ],
    ));
    compare(&index, WEDGE, 0);
    compare(&indexed(Graph::new(vec![], vec![])), WEDGE, 0);
}

/// Independent literal enumeration: scan raw edges, enumerate both undirected
/// orientations with loops once, and reject identical endpoint vertex slots.
/// No index, grouping, degree subtraction, or executor helpers are used.
fn literal_count(graph: &Graph) -> i64 {
    literal_count_with_anti(graph, false)
}

pub(super) fn literal_count_with_anti(graph: &Graph, anti: bool) -> i64 {
    fn neighbors(graph: &Graph, vertex: usize) -> Vec<usize> {
        let mut neighbors = Vec::new();
        for edge in &graph.edges {
            if edge.label.as_str() != "T" {
                continue;
            }
            let id = &graph.nodes[vertex].id;
            if &edge.from == id {
                neighbors.push(
                    graph
                        .nodes
                        .iter()
                        .position(|node| node.id == edge.to)
                        .unwrap(),
                );
            }
            if &edge.to == id && edge.from != edge.to {
                neighbors.push(
                    graph
                        .nodes
                        .iter()
                        .position(|node| node.id == edge.from)
                        .unwrap(),
                );
            }
        }
        neighbors
    }
    let mut count = 0;
    for b in 0..graph.nodes.len() {
        for a in neighbors(graph, b) {
            for c in neighbors(graph, b) {
                if a == c {
                    continue;
                }
                if anti
                    && graph.edges.iter().any(|edge| {
                        edge.label.as_str() == "T"
                            && ((edge.from == graph.nodes[a].id && edge.to == graph.nodes[c].id)
                                || (edge.from == graph.nodes[c].id && edge.to == graph.nodes[a].id))
                    })
                {
                    continue;
                }
                for edge in &graph.edges {
                    if edge.label.as_str() == "U" && edge.from == graph.nodes[c].id {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

#[test]
fn generated_small_multigraphs_match_literal_enumeration_and_reference() {
    for seed in 0..96u64 {
        let nodes: Vec<_> = (0..4)
            .map(|id| Node::new("N", id.to_string(), Props::new()))
            .collect();
        let mut edges = Vec::new();
        let mut random = seed + 17;
        for _ in 0..18 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let from = (random >> 12) % 4;
            let to = (random >> 24) % 4;
            let label = if random % 3 == 0 { "U" } else { "T" };
            edges.push(edge(label, &from.to_string(), &to.to_string()));
        }
        let graph = Graph::new(nodes, edges);
        let expected = literal_count(&graph);
        compare(&indexed(graph), WEDGE, expected);
    }
}

#[test]
fn unsupported_shapes_keep_the_clause_pipeline() {
    let index = indexed(Graph::new(vec![], vec![]));
    for source in [
        WEDGE.replace("a <> c", "a = c"),
        WEDGE.replace("a <> c", "a <> c AND true"),
        WEDGE.replace("a <> c", "a.id <> c.id"),
        WEDGE.replace("WHERE a <> c ", ""),
        WEDGE.replace("(b)", "(a)"),
        WEDGE.replace("(d)", "(c)"),
        WEDGE.replace("(c)", "(a)"),
        WEDGE.replace("(a)", "(a {})"),
        WEDGE.replace("[:T]", "[:T {}]"),
        WEDGE.replace("[:T]", "[r:T]"),
        WEDGE.replace("[:T]", "[:T*1..2]"),
        WEDGE.replace("[:T]", "[:T|V]"),
        WEDGE.replace("[:T]", "[]"),
        WEDGE.replace("-[:T]-(b)", "-[:T]->(b)"),
        WEDGE.replace("-[:U]->(d)", "<-[:U]-(d)"),
        WEDGE.replace("[:U]", "[:T]"),
        WEDGE.replace("MATCH", "OPTIONAL MATCH"),
        WEDGE.replace("RETURN", "WITH a RETURN"),
        WEDGE.replace("WHERE", ", () WHERE"),
        WEDGE.replace("(a)-", "p=(a)-"),
        WEDGE.replace("count(*)", "count(c)"),
        WEDGE.replace("count(*)", "count(DISTINCT c)"),
        format!("{WEDGE} LIMIT $limit"),
        format!("{WEDGE} ORDER BY a"),
        format!("{WEDGE} UNION ALL {WEDGE}"),
    ] {
        let query = parse_query(&source).unwrap_or_else(|error| panic!("{source}: {error:?}"));
        assert!(!supports(&query).unwrap(), "{source}");
        assert!(try_execute(&index, &query).unwrap().is_none(), "{source}");
        assert_eq!(
            classify_indexed_read_query(&query).unwrap(),
            IndexedReadPlan::ClausePipeline,
            "{source}"
        );
    }
}

#[test]
fn pagination_and_fallback_results_match_the_existing_executor() {
    let index = indexed(Graph::new(
        vec![
            Node::new("N", "x", Props::new()),
            Node::new("N", "y", Props::new()),
        ],
        vec![
            edge("T", "x", "x"),
            edge("T", "x", "y"),
            edge("U", "y", "y"),
        ],
    ));
    let params = CypherParameters::from([("limit".into(), Value::Int(1))]);
    for source in [
        format!("{WEDGE} LIMIT 0"),
        format!("{WEDGE} SKIP 1 LIMIT 1"),
        format!("{WEDGE} AS n SKIP 0 LIMIT 1"),
        format!("{WEDGE} LIMIT $limit"),
        WEDGE.replace("a <> c", "a <> c AND true"),
        WEDGE.replace("(b)", "(b {unused: 1})"),
    ] {
        assert_eq!(
            run_read_query_indexed(&index, &source, &params).unwrap(),
            run_read_query(index.graph(), &source, &params).unwrap(),
            "{source}"
        );
    }
}

#[test]
fn exact_subtraction_overflow_and_output_accounting() {
    let huge = i64::MAX as u128 + 100;
    assert_eq!(weighted_count(huge, huge, huge, u64::MAX).unwrap(), 0);
    assert_eq!(
        weighted_count(huge, huge - 1, huge - 1, 1).unwrap(),
        huge - 1
    );
    assert!(weighted_count(0, 1, 1, 1).is_err());
    assert!(weighted_count(u128::MAX, 0, 2, 1).is_err());
    let query = parse_query(WEDGE).unwrap();
    let wedge = plan(&query).unwrap().unwrap();
    assert_eq!(
        scalar_table(i64::MAX as u128, wedge.projection)
            .unwrap()
            .rows,
        vec![vec![Value::Int(i64::MAX)]]
    );
    assert!(
        scalar_table(i64::MAX as u128 + 1, wedge.projection)
            .unwrap_err()
            .to_string()
            .contains("int64")
    );
    let source = format!("{WEDGE} AS {}", "a".repeat(512));
    let limits = read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: 100,
        max_intermediate_bytes: 256,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    };
    let index = indexed(Graph::new(vec![], vec![]));
    let error = read_budget::with_budget(limits, || {
        run_read_query_indexed(&index, &source, &CypherParameters::new())
    })
    .unwrap_err();
    assert!(error.to_string().contains("shaping scalar count result"));
}

#[test]
fn active_budgets_and_bounded_policy_still_apply() {
    let index = indexed(Graph::new(
        vec![
            Node::new("N", "x", Props::new()),
            Node::new("N", "y", Props::new()),
        ],
        vec![
            edge("T", "x", "x"),
            edge("T", "x", "y"),
            edge("U", "y", "y"),
        ],
    ));
    for (work, bytes, timeout) in [
        (1, 1000, Duration::from_secs(5)),
        (1000, 1, Duration::from_secs(5)),
        (1000, 2, Duration::from_secs(5)),
        (1000, 1000, Duration::ZERO),
    ] {
        let limits = read_budget::ReadExecutionBudgetLimits {
            max_candidate_work: work,
            max_intermediate_bytes: bytes,
            max_range_items: 100,
            deadline: Instant::now() + timeout,
        };
        assert!(
            read_budget::with_budget(limits, || run_read_query_indexed(
                &index,
                WEDGE,
                &CypherParameters::new()
            ))
            .is_err()
        );
    }
    for (work, context) in [
        (14, "scanning count wedge leaf edges"),
        (16, "scanning count wedge edges"),
        (19, "grouping count wedge neighbors"),
    ] {
        // These pin wedge work sites, independently of earlier classifiers.
        // Four role-preparation units replace eight per-vertex predicate units
        // on this two-vertex, entirely unlabeled fixture.
        // All three raw slots are prepaid before the first group charge.
        // The whole indexed entrypoint remains covered by the limits above.
        let query = parse_query(WEDGE).unwrap();
        let limits = read_budget::ReadExecutionBudgetLimits {
            max_candidate_work: work,
            max_intermediate_bytes: 1000,
            max_range_items: 100,
            deadline: Instant::now() + Duration::from_secs(5),
        };
        let error = read_budget::with_budget(limits, || try_execute(&index, &query)).unwrap_err();
        assert!(error.to_string().contains(context), "{error}");
    }
    let policy = crate::ReadQueryPolicy::default();
    for source in [WEDGE.to_string(), format!("{WEDGE} LIMIT 0")] {
        assert!(
            crate::run_bounded_read_query_indexed(
                &index,
                &source,
                &CypherParameters::new(),
                &policy
            )
            .unwrap_err()
            .to_string()
            .contains("positive literal LIMIT")
        );
    }
    assert!(
        crate::run_bounded_read_query_indexed(
            &index,
            &format!("{WEDGE} LIMIT 1"),
            &CypherParameters::new(),
            &policy
        )
        .is_ok()
    );
}

#[test]
fn high_degree_center_stays_inside_a_linear_work_budget() {
    let degree = 256;
    let mut nodes = vec![
        Node::new("Person", "b", Props::new()),
        Node::new("Tag", "d", Props::new()),
    ];
    let mut edges = Vec::new();
    for vertex in 0..degree {
        let id = format!("v{vertex}");
        nodes.push(Node::new("Person", &id, Props::new()));
        edges.extend([edge("T", "b", &id), edge("U", &id, "d")]);
    }
    let index = indexed(Graph::new(nodes, edges));
    let source = "MATCH (a:Person)-[:T]-(b:Person)-[:T]-(c:Person)-[:U]->(:Tag) WHERE a <> c RETURN count(*) LIMIT 1";
    let policy = crate::ReadQueryPolicy {
        max_candidate_work: 10_000,
        max_intermediate_bytes: 4096,
        ..crate::ReadQueryPolicy::default()
    };
    let result =
        crate::run_bounded_read_query_indexed(&index, source, &CypherParameters::new(), &policy)
            .unwrap();
    assert_eq!(result.rows, vec![vec![Value::Int(degree * (degree - 1))]]);
}
