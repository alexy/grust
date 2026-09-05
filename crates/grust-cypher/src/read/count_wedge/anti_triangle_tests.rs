//! Public-path regressions independent of the shared triangle/anti kernels.

use std::sync::Arc;

use grust_core::{Edge, Graph, Node, NodeId, Props, TypedGraphIndex, Value};

use crate::CypherParameters;
use crate::parser::parse_query;
use crate::read::{
    IndexedReadPlan, classify_indexed_read_query, run_read_query, run_read_query_indexed,
};

type Domains<'a> = [Option<&'a str>; 4];
const DISTINCT: Domains<'static> = [Some("A"), Some("B"), Some("C"), Some("D")];

fn node(label: &str, id: &str) -> Node {
    Node::new(label, id, Props::new())
}

fn edge(kind: &str, from: &str, to: &str) -> Edge {
    // Public IDs deliberately coincide; vector slots are physical identities.
    Edge::new(kind, from, to, Props::new()).with_id("shared-public-id")
}

fn copies(graph: &mut Graph, kind: &str, from: &str, to: &str, count: usize) {
    graph.edges.extend((0..count).map(|_| edge(kind, from, to)));
}

fn source(domains: Domains<'_>) -> String {
    let pattern = |name: &str, label: Option<&str>| match label {
        Some(label) => format!("({name}:{label})"),
        None => format!("({name})"),
    };
    let [a, b, c, d] = std::array::from_fn(|i| pattern(["a", "b", "c", "d"][i], domains[i]));
    format!(
        "MATCH {a}-[:T]-{b}-[:T]-{c}-[:U]->{d} \
         OPTIONAL MATCH (a)-[k:T]-(c) \
         WITH a,c,d,k WHERE k IS NULL AND a <> c RETURN count(*) AS n"
    )
}

fn orientations(edge: &Edge) -> [Option<(&NodeId, &NodeId)>; 2] {
    [
        Some((&edge.from, &edge.to)),
        (edge.from != edge.to).then_some((&edge.to, &edge.from)),
    ]
}

/// Enumerate raw physical edge triples and test raw anti-edge existence.
/// No index, degree algebra, triangle enumeration, or executor helper is used.
fn raw_count(graph: &Graph, domains: Domains<'_>) -> i64 {
    let matches = |id: &NodeId, label: Option<&str>| {
        let node = graph.nodes.iter().find(|node| &node.id == id).unwrap();
        label.is_none_or(|label| node.label.as_str() == label)
    };
    let mut count = 0;
    for (first_slot, first) in graph.edges.iter().enumerate() {
        if first.label.as_str() != "T" {
            continue;
        }
        for (a, b) in orientations(first).into_iter().flatten() {
            if !matches(a, domains[0]) || !matches(b, domains[1]) {
                continue;
            }
            for (second_slot, second) in graph.edges.iter().enumerate() {
                if second_slot == first_slot || second.label.as_str() != "T" {
                    continue;
                }
                for (from, c) in orientations(second).into_iter().flatten() {
                    if from != b || a == c || !matches(c, domains[2]) {
                        continue;
                    }
                    if graph.edges.iter().any(|closing| {
                        closing.label.as_str() == "T"
                            && ((&closing.from == a && &closing.to == c)
                                || (&closing.from == c && &closing.to == a))
                    }) {
                        continue;
                    }
                    for leaf in &graph.edges {
                        if leaf.label.as_str() == "U"
                            && &leaf.from == c
                            && matches(&leaf.to, domains[3])
                        {
                            count += 1;
                        }
                    }
                }
            }
        }
    }
    count
}

fn assert_public_count(graph: &Graph, domains: Domains<'_>, expected: i64) {
    assert_eq!(
        raw_count(graph, domains),
        expected,
        "independent raw oracle"
    );
    let source = source(domains);
    let query = parse_query(&source).unwrap();
    crate::semantics::analyze(&query).unwrap();
    assert_eq!(
        classify_indexed_read_query(&query).unwrap(),
        IndexedReadPlan::CountFactorized,
        "{source}"
    );
    let params = CypherParameters::new();
    let reference = run_read_query(graph, &source, &params).unwrap();
    assert_eq!(reference.rows, vec![vec![Value::Int(expected)]], "{source}");
    let index = TypedGraphIndex::new(Arc::new(graph.clone())).unwrap();
    assert_eq!(
        run_read_query_indexed(&index, &source, &params).unwrap(),
        reference,
        "{source}"
    );
}

fn asymmetric_fixture(placement: [usize; 3]) -> Graph {
    let labels = ["A", "B", "C"];
    let mut graph = Graph::new(
        vec![
            node(labels[placement[0]], "x0"),
            node(labels[placement[1]], "x1"),
            node(labels[placement[2]], "x2"),
            node("A", "open-a"),
            node("A", "second-a"),
            node("B", "open-b"),
            node("C", "open-c"),
            node("C", "isolated-c1"),
            node("C", "isolated-c2"),
            node("D", "d"),
        ],
        vec![edge("T", "x0", "x1"), edge("T", "x1", "x0")],
    );
    // Weighted closed triangle: edge weights 2, 3, 5 and different U weights.
    copies(&mut graph, "T", "x1", "x2", 2);
    copies(&mut graph, "T", "x2", "x1", 1);
    copies(&mut graph, "T", "x0", "x2", 3);
    copies(&mut graph, "T", "x2", "x0", 2);
    for (from, count) in [("x0", 2), ("x1", 7), ("x2", 11)] {
        copies(&mut graph, "U", from, "d", count);
    }
    // Positive open component: (4 + 1) * 6 * 3 = 90. A/B/C domain sizes
    // are genuinely different (3/2/4), not renamed identical domains.
    copies(&mut graph, "T", "open-a", "open-b", 4);
    copies(&mut graph, "T", "second-a", "open-b", 1);
    copies(&mut graph, "T", "open-b", "open-c", 6);
    copies(&mut graph, "U", "open-c", "d", 3);
    graph
}

#[test]
fn all_six_asymmetric_role_placements_remove_weighted_closed_triangles() {
    for placement in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        assert_public_count(&asymmetric_fixture(placement), DISTINCT, 90);
    }
}

#[test]
fn t_self_loops_do_not_change_anti_counts_in_distinct_or_overlapping_domains() {
    let graph = asymmetric_fixture([2, 0, 1]);
    let mut loops = graph.clone();
    for node in &graph.nodes {
        copies(&mut loops, "T", node.id.as_str(), node.id.as_str(), 3);
    }
    for domains in [
        DISTINCT,
        [None, Some("B"), Some("C"), None],
        [Some("A"), None, None, Some("D")],
        [None; 4],
    ] {
        let expected = raw_count(&graph, domains);
        assert_public_count(&graph, domains, expected);
        assert_public_count(&loops, domains, expected);
    }
}

fn open_chain() -> Graph {
    Graph::new(
        vec![
            node("A", "a"),
            node("B", "b"),
            node("C", "c"),
            node("D", "d"),
        ],
        vec![
            edge("T", "a", "b"),
            edge("T", "b", "a"),
            edge("T", "b", "c"),
            edge("T", "c", "b"),
            edge("T", "b", "c"),
        ],
    )
}

#[test]
fn closure_direction_and_parallel_copies_only_change_existence() {
    let mut open = open_chain();
    copies(&mut open, "U", "c", "d", 5);
    assert_public_count(&open, DISTINCT, 30);
    for (forward, reverse) in [(1, 0), (0, 1), (7, 0), (0, 9), (3, 5)] {
        let mut closed = open.clone();
        copies(&mut closed, "T", "a", "c", forward);
        copies(&mut closed, "T", "c", "a", reverse);
        assert_public_count(&closed, DISTINCT, 0);
    }
}

#[test]
fn u_edges_keep_parallel_self_loop_and_outgoing_only_leaf_semantics() {
    let mut graph = open_chain();
    copies(&mut graph, "U", "d", "c", 7);
    assert_public_count(&graph, DISTINCT, 0); // Incoming-only does not qualify.
    copies(&mut graph, "U", "c", "d", 3);
    assert_public_count(&graph, DISTINCT, 18);
    copies(&mut graph, "U", "c", "c", 2);
    assert_public_count(&graph, DISTINCT, 18); // c is not in d's label domain.
    let any_leaf = [Some("A"), Some("B"), Some("C"), None];
    assert_public_count(&graph, any_leaf, 30); // U self-loops counted once.
    graph.nodes.push(node("Other", "other"));
    copies(&mut graph, "U", "c", "other", 4);
    assert_public_count(&graph, DISTINCT, 18);
    assert_public_count(&graph, any_leaf, 54);
    copies(&mut graph, "U", "c", "a", 1);
    assert_public_count(&graph, any_leaf, 60); // d may alias a.
}

#[test]
fn generated_asymmetric_multigraphs_match_both_independent_oracles() {
    let mut nonzero = 0;
    for seed in 0..32u64 {
        let mut graph = open_chain();
        graph.nodes.extend([
            node("A", "a2"),
            node("B", "b2"),
            node("C", "c2"),
            node("D", "d2"),
        ]);
        copies(&mut graph, "U", "c", "d", 1);
        let mut random = seed + 91;
        for _ in 0..18 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            graph.edges.push(edge(
                if random % 3 == 0 { "U" } else { "T" },
                graph.nodes[(random >> 12) as usize % 8].id.as_str(),
                graph.nodes[(random >> 24) as usize % 8].id.as_str(),
            ));
        }
        for domains in [
            DISTINCT,
            [None, Some("B"), Some("C"), None],
            [Some("A"), None, None, Some("D")],
        ] {
            let expected = raw_count(&graph, domains);
            nonzero += usize::from(expected > 0);
            assert_public_count(&graph, domains, expected);
        }
    }
    assert!(nonzero > 0);
}
