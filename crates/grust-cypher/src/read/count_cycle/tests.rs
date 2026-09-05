use super::*;
use grust_core::{Edge, Node, Props};
use std::sync::Arc;

#[path = "oracle.rs"]
mod oracle;
use oracle::{Filters, literal_count};
#[path = "budgets.rs"]
mod budgets;
#[path = "role_masks.rs"]
mod role_masks;

const Q: &str = "MATCH (u:N)-[:K]-(v:N), (u)<-[:H]-(c:N {kind:'C'})-[:R]->(p:N {kind:'P'})-[:H]->(v) RETURN count(*) AS n";
const Q2: &str = "MATCH (person1:Person)-[:KNOWS]-(person2:Person), (person1)<-[:HAS_CREATOR]-(comment:Message {kind:'Comment'})-[:REPLY_OF]->(post:Message {kind:'Post'})-[:HAS_CREATOR]->(person2) RETURN count(*) AS count";
const A2: &str = "MATCH (post:Message {kind:'Post'})-[:HAS_CREATOR]->(person2:Person), (comment:Message {kind:'Comment'})-[:REPLY_OF]->(post), (comment)-[:HAS_CREATOR]->(person1:Person), (person2)-[:KNOWS]-(person1) RETURN count(*) AS count";

fn node(label: &str, id: &str, kind: &str) -> Node {
    let mut node = Node::new(label, id, Props::new());
    node.props.insert("kind".into(), Value::String(kind.into()));
    node
}

fn edge(kind: &str, from: &str, to: &str) -> Edge {
    Edge::new(kind, from, to, Props::new())
}
fn indexed(graph: Graph) -> TypedGraphIndex {
    TypedGraphIndex::new(Arc::new(graph)).unwrap()
}

fn compare(index: &TypedGraphIndex, source: &str, expected: i64) {
    let query = parse_query(source).unwrap();
    crate::semantics::analyze(&query).unwrap();
    assert!(supports(&query).unwrap(), "{source}");
    assert_eq!(
        classify_indexed_read_query(&query).unwrap(),
        IndexedReadPlan::CountFactorized,
        "{source}"
    );
    let params = CypherParameters::new();
    let result = try_execute(index, &query, &params)
        .unwrap()
        .expect("proven count cycle");
    assert_eq!(result.rows, vec![vec![Value::Int(expected)]], "{source}");
    assert_eq!(
        result,
        run_read_query(index.graph(), source, &params).unwrap(),
        "{source}"
    );
    assert_eq!(
        result,
        run_read_query_indexed(index, source, &params).unwrap(),
        "{source}"
    );
}

fn simple_graph() -> Graph {
    Graph::new(
        vec![
            node("N", "c", "C"),
            node("N", "p", "P"),
            node("N", "u", "X"),
            node("N", "v", "X"),
        ],
        vec![
            edge("R", "c", "p"),
            edge("H", "c", "u"),
            edge("H", "p", "v"),
            edge("K", "u", "v"),
        ],
    )
}

#[test]
fn q2_and_reordered_a2_match_three_in_the_example_fixture() {
    // Exact relevant sfexample rows, including comment->comment replies that
    // must fail the Post property constraint rather than a schema shortcut.
    let nodes = (1..=5)
        .map(|i| node("Person", &format!("u{i}"), "Person"))
        .chain([10, 20].map(|i| node("Message", &format!("p{i}"), "Post")))
        .chain((1..=6).map(|i| node("Message", &format!("c{i}"), "Comment")))
        .collect();
    let mut edges: Vec<_> = [
        ("p10", "u2"),
        ("p20", "u3"),
        ("c1", "u3"),
        ("c2", "u1"),
        ("c3", "u3"),
        ("c4", "u1"),
        ("c5", "u4"),
        ("c6", "u1"),
    ]
    .into_iter()
    .map(|(a, b)| edge("HAS_CREATOR", a, b))
    .collect();
    edges.extend(
        [
            ("c1", "p10"),
            ("c2", "p10"),
            ("c6", "p20"),
            ("c3", "c2"),
            ("c4", "c3"),
            ("c5", "c4"),
        ]
        .into_iter()
        .map(|(a, b)| edge("REPLY_OF", a, b)),
    );
    edges.extend(
        [(1, 2), (1, 3), (1, 4), (2, 3), (3, 4), (4, 5)]
            .into_iter()
            .map(|(a, b)| edge("KNOWS", &format!("u{a}"), &format!("u{b}"))),
    );
    let index = indexed(Graph::new(nodes, edges));
    let filters = Filters {
        nodes: [
            |n| {
                n.label.as_str() == "Message"
                    && n.props.get("kind") == Some(&Value::String("Comment".into()))
            },
            |n| {
                n.label.as_str() == "Message"
                    && n.props.get("kind") == Some(&Value::String("Post".into()))
            },
            |n| n.label.as_str() == "Person",
            |n| n.label.as_str() == "Person",
        ],
        ..Filters::default()
    };
    assert_eq!(
        literal_count(index.graph(), ["REPLY_OF", "HAS_CREATOR", "KNOWS"], filters),
        3
    );
    compare(&index, Q2, 3);
    compare(&index, A2, 3);
}

#[test]
fn all_comma_atom_orders_and_arrow_spellings_have_the_same_proof() {
    let atoms = [
        "(p:N {kind:'P'})-[:H]->(v:N)",
        "(p)<-[:R]-(c:N {kind:'C'})",
        "(u:N)<-[:H]-(c)",
        "(v)-[:K]-(u)",
    ];
    let index = indexed(simple_graph());
    for a in 0..4 {
        for b in 0..4 {
            for c in 0..4 {
                for d in 0..4 {
                    let order = [a, b, c, d];
                    if order
                        .iter()
                        .enumerate()
                        .any(|(i, x)| order[..i].contains(x))
                    {
                        continue;
                    }
                    let source = format!(
                        "MATCH {} RETURN count(*)",
                        order.map(|i| atoms[i]).join(", ")
                    );
                    compare(&index, &source, 1);
                }
            }
        }
    }
}

#[test]
fn nonfunctional_creators_parallel_reciprocal_edges_and_knows_loops() {
    let nodes = vec![
        node("N", "c", "C"),
        node("N", "p", "P"),
        node("N", "x", "X"),
        node("N", "y", "X"),
    ];
    let mut edges = Vec::new();
    for (kind, from, to, n) in [
        ("R", "c", "p", 2),
        ("H", "c", "x", 2),
        ("H", "c", "y", 1),
        ("H", "p", "x", 3),
        ("H", "p", "y", 4),
        ("K", "x", "x", 2),
        ("K", "x", "y", 3),
        ("K", "y", "x", 1),
        ("K", "y", "y", 1),
    ] {
        edges.extend((0..n).map(|_| edge(kind, from, to)));
    }
    // 2 replies * [2*(3*2 + 4*4) + 1*(3*4 + 4*1)] = 120.
    let index = indexed(Graph::new(nodes, edges));
    assert_eq!(
        literal_count(index.graph(), ["R", "H", "K"], Filters::default()),
        120
    );
    compare(&index, Q, 120);
    // A creator can also be its message's own physical node; only c/p are
    // proved distinct. No blanket node-isomorphism restriction is introduced.
    let index = indexed(Graph::new(
        vec![node("N", "c", "C"), node("N", "p", "P")],
        vec![
            edge("R", "c", "p"),
            edge("H", "c", "c"),
            edge("H", "p", "c"),
            edge("K", "c", "c"),
        ],
    ));
    compare(&index, Q, 1);
}

#[test]
fn every_node_mention_and_duplicate_key_conjunct_is_preserved() {
    let index = indexed(simple_graph());
    for source in [
        Q.replace("(u)<-", "(u:Missing)<-"),
        Q.replace("(v) RETURN", "(v {absent:true}) RETURN"),
        Q.replace("kind:'C'", "kind:'C',kind:'P'"),
    ] {
        compare(&index, &source, 0);
    }
    let source = "MATCH (c:N {kind:'C'})-[:H]->(u:N), (p:N {kind:'P'})-[:H]->(v:N), (c {kind:'P'})-[:R]->(p), (u)-[:K]-(v) RETURN count(*)";
    compare(&index, source, 0);
    compare(
        &index,
        &source.replace("(c {kind:'P'})-[:R]->(p)", "(p)<-[:R]-(c {kind:'P'})"),
        0,
    );
}

#[test]
fn literal_edge_filters_and_numeric_property_equality_match_reference() {
    let mut graph = simple_graph();
    graph.nodes[0]
        .props
        .insert("number".into(), Value::Float(2.0));
    for e in &mut graph.edges {
        e.props.insert("weight".into(), Value::Float(3.0));
    }
    let index = indexed(graph);
    let source = Q
        .replace("kind:'C'", "kind:'C',number:2")
        .replace("[:R]", "[:R {weight:3}]")
        .replace("[:H]", "[:H {weight:3}]")
        .replace("[:K]", "[:K {weight:3}]");
    compare(&index, &source, 1);
    compare(
        &index,
        &source.replace("[:K {weight:3}]", "[:K {weight:4}]"),
        0,
    );
    compare(&index, &source.replace("number:2", "missing:null"), 0);

    let mut graph = simple_graph();
    for (from, to, weight, copies) in [("c", "u", 4, 2), ("p", "v", 5, 3)] {
        for _ in 0..copies {
            let mut e = edge("H", from, to);
            e.props.insert("weight".into(), Value::Int(weight));
            graph.edges.push(e);
        }
    }
    let index = indexed(graph);
    let source = Q
        .replace("<-[:H]-", "<-[:H {weight:4}]-")
        .replace("-[:H]->", "-[:H {weight:5}]->");
    compare(&index, &source, 6);
    compare(
        &index,
        &source
            .replace("weight:4", "weight:5")
            .replace("weight:5}]->", "weight:4}]->"),
        0,
    );
}

#[test]
fn generated_multigraphs_match_independent_edge_quadruples_and_reference() {
    for seed in 0..96u64 {
        let mut graph = simple_graph();
        let mut random = seed + 17;
        for _ in 0..20 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let labels = ["R", "H", "H", "K"];
            let mut e = edge(
                labels[(random >> 6) as usize % 4],
                graph.nodes[(random >> 12) as usize % 4].id.as_str(),
                graph.nodes[(random >> 24) as usize % 4].id.as_str(),
            );
            e.props.insert("on".into(), Value::Bool(random & 8 != 0));
            graph.edges.push(e);
        }
        let index = indexed(graph);
        for filtered in [false, true] {
            let filters = if filtered {
                Filters {
                    edges: [
                        |e| e.props.get("on") == Some(&Value::Bool(true)),
                        |_| true,
                        |_| true,
                        |e| e.props.get("on") == Some(&Value::Bool(true)),
                    ],
                    ..Filters::default()
                }
            } else {
                Filters::default()
            };
            let source = if filtered {
                Q.replace("[:R]", "[:R {on:true}]")
                    .replace("[:K]", "[:K {on:true}]")
            } else {
                Q.into()
            };
            let expected = literal_count(index.graph(), ["R", "H", "K"], filters);
            compare(&index, &source, expected);
        }
    }
}

#[test]
fn insufficient_disjointness_and_other_unproven_shapes_are_rejected() {
    let index = indexed(simple_graph());
    for source in [
        Q.replace("kind:'P'", "kind:'C'"),
        Q.replace("kind:'P'", "other:'P'"),
        Q.replace("kind:'C'", "kind:1")
            .replace("kind:'P'", "kind:1.0"),
        Q.replace("kind:'C'", "kind:9007199254740992")
            .replace("kind:'P'", "kind:9007199254740993"),
        Q.replace("kind:'C'", "kind:1")
            .replace("kind:'P'", "kind:2"),
        Q.replace("[:K]-", "[:K]->"),
        Q.replace("[:R]", "[:H]"),
        Q.replace("[:K]", "[:H]"),
        Q.replace("[:R]", "[r:R]"),
        Q.replace("[:R]", "[:R*1..2]"),
        Q.replace("[:H]", "[:H|Other]"),
        Q.replace("(v) RETURN", "(u) RETURN"),
        Q.replace("kind:'C'", "kind:$c"),
        Q.replace("RETURN count(*)", "WHERE c <> p RETURN count(*)"),
        Q.replace("MATCH ", "OPTIONAL MATCH "),
        Q.replace("count(*)", "count(c)"),
        format!("{Q} ORDER BY n"),
    ] {
        let query = parse_query(&source).unwrap();
        assert!(!supports(&query).unwrap(), "{source}");
        assert!(
            try_execute(&index, &query, &CypherParameters::new())
                .unwrap()
                .is_none(),
            "{source}"
        );
    }
}

#[test]
fn empty_graph_and_pagination_preserve_scalar_semantics() {
    let index = indexed(Graph::new(vec![], vec![]));
    compare(&index, Q, 0);
    let index = indexed(simple_graph());
    for suffix in ["LIMIT 0", "SKIP 1", "SKIP 0 LIMIT 1"] {
        let source = format!("{Q} {suffix}");
        let query = parse_query(&source).unwrap();
        assert_eq!(
            try_execute(&index, &query, &CypherParameters::new())
                .unwrap()
                .unwrap(),
            run_read_query(index.graph(), &source, &CypherParameters::new()).unwrap()
        );
    }
}
