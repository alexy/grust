use super::*;
use std::sync::Arc;

fn indexed(graph: Graph) -> TypedGraphIndex {
    TypedGraphIndex::new(Arc::new(graph)).unwrap()
}

fn query(source: &str) -> Query {
    let query = parse_query(source).unwrap();
    crate::semantics::analyze(&query).unwrap();
    query
}

fn role(forest: &Forest<'_>, name: &str) -> usize {
    forest
        .nodes
        .iter()
        .position(|mentions| {
            mentions
                .iter()
                .any(|node| node.variable.as_deref() == Some(name))
        })
        .unwrap()
}

fn node(label: &str, id: &str, keep: bool) -> Node {
    let mut node = Node::new(label, id, Props::new());
    node.props.insert("keep".into(), Value::Bool(keep));
    node
}

fn edge(kind: &str, from: &str, to: &str) -> Edge {
    Edge::new(kind, from, to, Props::new())
}

fn compare(index: &TypedGraphIndex, source: &str, expected: i64) {
    let query = query(source);
    let params = CypherParameters::new();
    let actual = try_execute(index, &query, &params)
        .unwrap()
        .expect("proven forest");
    assert_eq!(actual.rows, vec![vec![Value::Int(expected)]], "{source}");
    let reference = execute_read_query(index.graph(), &query, &params).unwrap();
    assert_eq!(actual, reference, "{source}");
    assert_eq!(
        execute_read_query_indexed(index, &query, &params).unwrap(),
        reference
    );
}

#[test]
fn only_property_bearing_mandatory_branch_roles_are_enabled() {
    for (source, expected) in [
        ("MATCH (b)-[:R]->(), (b)-[:S]->() RETURN count(*)", false),
        ("MATCH (b {})-[:R]->(), (b)-[:S]->() RETURN count(*)", false),
        ("MATCH (b {keep:true}) RETURN count(*)", false),
        ("MATCH (b {keep:true})-[:R]->() RETURN count(*)", false),
        (
            "MATCH (b {keep:true})-[:R]->() OPTIONAL MATCH (b)-[:O]->() RETURN count(*)",
            false,
        ),
        (
            "MATCH (b)-[:R]->(), (b)-[:S]->() OPTIONAL MATCH (b {keep:true})-[:O]->() RETURN count(*)",
            false,
        ),
        (
            "MATCH (b {keep:true})-[:R]->(), (b)-[:S]->() RETURN count(*)",
            true,
        ),
        (
            "MATCH (b)-[:R]->() MATCH (b {keep:true})-[:S]->() RETURN count(*)",
            true,
        ),
    ] {
        let query = query(source);
        let proven = plan(&query).unwrap().expect("eligible forest");
        assert_eq!(
            enabled(&proven.forest, role(&proven.forest, "b")),
            expected,
            "{source}"
        );
    }
}

#[test]
fn every_mandatory_atom_is_checked_not_just_one_selected_type() {
    let source = "MATCH (a:A)-[:R]->(b:B {keep:true})-[:S]->(c:C), (b)-[:T]->(d:D) RETURN count(*)";
    let query = query(source);
    let proven = plan(&query).unwrap().unwrap();
    let slot = role(&proven.forest, "b");
    let nodes = vec![
        node("A", "a", false),
        node("B", "b", true),
        node("C", "c", false),
        node("D", "d", false),
    ];
    let required = [
        edge("R", "a", "b"),
        edge("S", "b", "c"),
        edge("T", "b", "d"),
    ];
    for missing in 0..=required.len() {
        let mut edges: Vec<_> = required
            .iter()
            .enumerate()
            .filter(|(position, _)| *position != missing)
            .map(|(_, edge)| edge.clone())
            .collect();
        // Types exist elsewhere, but a global type check would be insufficient.
        edges.extend([
            edge("R", "d", "d"),
            edge("S", "d", "d"),
            edge("T", "d", "d"),
        ]);
        let index = indexed(Graph::new(nodes.clone(), edges));
        let present = missing == required.len();
        let prepared = prepare(&index, &proven.forest, slot).unwrap().unwrap();
        assert_eq!(prepared.accepts(1).unwrap(), present);
        compare(&index, source, if present { 1 } else { 0 });
    }
}

#[derive(Clone, Copy, Debug)]
enum Flow {
    Out,
    In,
    Either,
}

fn arm(kind: &str, flow: Flow) -> String {
    match flow {
        Flow::Out => format!("-[:{kind}]->"),
        Flow::In => format!("<-[:{kind}]-"),
        Flow::Either => format!("-[:{kind}]-"),
    }
}

fn reverse(flow: Flow) -> Flow {
    match flow {
        Flow::Out => Flow::In,
        Flow::In => Flow::Out,
        Flow::Either => flow,
    }
}

// Independent physical-edge scan: each undirected loop contributes once.
fn raw_degree(graph: &Graph, center: &Node, kind: &str, flow: Flow) -> i64 {
    graph
        .edges
        .iter()
        .filter(|edge| {
            if edge.label.as_str() != kind {
                return false;
            }
            let neighbor = match flow {
                Flow::Out if edge.from == center.id => Some(&edge.to),
                Flow::In if edge.to == center.id => Some(&edge.from),
                Flow::Either if edge.from == center.id => Some(&edge.to),
                Flow::Either if edge.to == center.id => Some(&edge.from),
                _ => None,
            };
            neighbor.is_some_and(|id| {
                graph
                    .nodes
                    .iter()
                    .any(|node| &node.id == id && node.label.as_str() == "N")
            })
        })
        .count() as i64
}

fn raw_count(graph: &Graph, r: Flow, s: Flow) -> i64 {
    graph
        .nodes
        .iter()
        .filter(|node| {
            node.label.as_str() == "N" && node.props.get("keep") == Some(&Value::Bool(true))
        })
        .map(|node| raw_degree(graph, node, "R", r) * raw_degree(graph, node, "S", s))
        .sum()
}

#[test]
fn both_pattern_ends_every_direction_and_physical_loops_match_raw_edges() {
    let index = indexed(Graph::new(
        vec![
            node("N", "x", true),
            node("N", "y", false),
            node("N", "z", true),
            node("Other", "o", true),
        ],
        vec![
            edge("R", "x", "x"),
            edge("R", "x", "x"),
            edge("R", "x", "y"),
            edge("R", "y", "x"),
            edge("R", "z", "x"),
            edge("R", "o", "x"),
            edge("S", "x", "x"),
            edge("S", "x", "z"),
            edge("S", "x", "z"),
            edge("S", "y", "x"),
            edge("S", "z", "y"),
            edge("S", "x", "o"),
        ],
    ));
    for r in [Flow::Out, Flow::In, Flow::Either] {
        for s in [Flow::Out, Flow::In, Flow::Either] {
            let expected = raw_count(index.graph(), r, s);
            for reverse_r in [false, true] {
                for reverse_s in [false, true] {
                    let first = if reverse_r {
                        format!("(a:N){}(b:N {{keep:true}})", arm("R", reverse(r)))
                    } else {
                        format!("(b:N {{keep:true}}){}(a:N)", arm("R", r))
                    };
                    let second = if reverse_s {
                        format!("(c:N){}(b)", arm("S", reverse(s)))
                    } else {
                        format!("(b){}(c:N)", arm("S", s))
                    };
                    compare(
                        &index,
                        &format!("MATCH {first}, {second} RETURN count(*)"),
                        expected,
                    );
                }
            }
        }
    }
}

#[test]
fn generated_multigraphs_match_independent_counts_and_reference() {
    for seed in 0..24u64 {
        let mut state = seed + 1;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            state >> 32
        };
        let nodes = (0..4)
            .map(|vertex| {
                node(
                    if vertex == 3 { "Other" } else { "N" },
                    &vertex.to_string(),
                    vertex != 1,
                )
            })
            .collect();
        let mut edges = Vec::new();
        for kind in ["R", "S"] {
            for _ in 0..10 {
                edges.push(edge(
                    kind,
                    &(next() % 4).to_string(),
                    &(next() % 4).to_string(),
                ));
            }
        }
        let index = indexed(Graph::new(nodes, edges));
        for r in [Flow::Out, Flow::In, Flow::Either] {
            for s in [Flow::Out, Flow::In, Flow::Either] {
                let source = format!(
                    "MATCH (b:N {{keep:true}}){}(:N), (b){}(:N) RETURN count(*)",
                    arm("R", r),
                    arm("S", s)
                );
                compare(&index, &source, raw_count(index.graph(), r, s));
            }
        }
    }
}

#[test]
fn original_mentions_edge_and_neighbor_filters_and_optional_padding_remain_required() {
    let mut s = edge("S", "b", "c");
    s.props.insert("ok".into(), Value::Bool(true));
    let index = indexed(Graph::new(
        vec![
            node("N", "a", false),
            node("N", "b", true),
            node("Other", "c", false),
        ],
        vec![edge("R", "b", "a"), edge("R", "b", "a"), s],
    ));
    for (source, expected) in [
        (
            "MATCH (b)-[:R]->(:N) MATCH (b:N {keep:true})-[:S {ok:true}]->(:Other) RETURN count(*)",
            2,
        ),
        (
            "MATCH (b {keep:true})-[:R]->(:N), (b {keep:false})-[:S]->(:Other) RETURN count(*)",
            0,
        ),
        (
            "MATCH (b {keep:true, keep:false})-[:R]->(:N), (b)-[:S]->(:Other) RETURN count(*)",
            0,
        ),
        (
            "MATCH (b {keep:true})-[:R]->(:N), (b:Missing)-[:S]->(:Other) RETURN count(*)",
            0,
        ),
        (
            "MATCH (b {keep:true})-[:R]->(:N), (b)-[:S {ok:false}]->(:Other) RETURN count(*)",
            0,
        ),
        (
            "MATCH (b {keep:true})-[:R]->(:N), (b)-[:S]->(:N) RETURN count(*)",
            0,
        ),
        (
            "MATCH (b {keep:true})-[:R]->(:N), (b)-[:S]->(:Other) OPTIONAL MATCH (b)-[:O]->() RETURN count(*)",
            2,
        ),
        (
            "MATCH (b {keep:true})-[:R]->(:N), (b)-[:S]->(:Other) OPTIONAL MATCH (b:Missing {keep:false})-[:O]->() RETURN count(*)",
            2,
        ),
    ] {
        compare(&index, source, expected);
    }
}

#[test]
fn disabled_leaf_roles_keep_optional_null_padding() {
    let index = indexed(Graph::new(
        vec![node("N", "b", true), node("N", "a", false)],
        vec![edge("R", "b", "a"), edge("R", "b", "a")],
    ));
    for (source, expected) in [
        (
            "MATCH (b:N {keep:true}) OPTIONAL MATCH (b)-[:O]->() RETURN count(*)",
            1,
        ),
        (
            "MATCH (b:N {keep:true})-[:R]->(:N) OPTIONAL MATCH (b)-[:O]->() RETURN count(*)",
            2,
        ),
    ] {
        compare(&index, source, expected);
    }
}
