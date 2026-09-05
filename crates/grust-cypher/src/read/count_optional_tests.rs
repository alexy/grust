use super::*;
use grust_core::{Edge, Node, Props};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const Q7: &str = "MATCH (:Tag)<-[:HAS_TAG]-(message:Message)-[:HAS_CREATOR]->(creator:Person) OPTIONAL MATCH (message)<-[:LIKES]-(liker:Person) OPTIONAL MATCH (message)<-[:REPLY_OF]-(comment:Message {kind:'Comment'}) RETURN count(*) AS count";
const A4: &str = "MATCH (:Tag)<-[:HAS_TAG]-(message:Message)-[:HAS_CREATOR]->(creator:Person) WITH message, creator OPTIONAL MATCH (message)<-[:LIKES]-(liker:Person) OPTIONAL MATCH (message)<-[:REPLY_OF]-(comment:Message) RETURN count(*) AS count";

fn edge(kind: &str, from: &str, to: &str) -> Edge {
    Edge::new(kind, from, to, Props::new())
}

fn index(graph: Graph) -> TypedGraphIndex {
    TypedGraphIndex::new(Arc::new(graph)).unwrap()
}

fn compare(index: &TypedGraphIndex, source: &str, expected: i64) {
    let query = parse_query(source).unwrap();
    crate::semantics::analyze(&query).unwrap();
    assert_eq!(
        classify_indexed_read_query(&query).unwrap(),
        IndexedReadPlan::CountFactorized,
        "{source}"
    );
    let result = try_execute(index, &query, &CypherParameters::new())
        .unwrap()
        .expect("proven optional leaves");
    assert_eq!(result.rows, vec![vec![Value::Int(expected)]], "{source}");
    assert_eq!(
        result,
        run_read_query(index.graph(), source, &CypherParameters::new()).unwrap(),
        "{source}"
    );
    assert_eq!(
        result,
        run_read_query_indexed(index, source, &CypherParameters::new()).unwrap()
    );
}

#[path = "count_optional_oracle.rs"]
mod oracle;
use oracle::{Flow, RawEdge, Step, literal_count, raw};

#[test]
fn q7_and_a4_preserve_dropped_tag_multiplicity_but_differ_on_reply_kind() {
    let mut comment = Node::new("Message", "c", Props::new());
    comment
        .props
        .insert("kind".into(), Value::String("Comment".into()));
    let mut post = Node::new("Message", "post", Props::new());
    post.props
        .insert("kind".into(), Value::String("Post".into()));
    let nodes = vec![
        Node::new("Message", "m", Props::new()),
        Node::new("Tag", "t", Props::new()),
        Node::new("Person", "p", Props::new()),
        comment,
        post,
    ];
    let mut edges = Vec::new();
    for (kind, from, to, n) in [
        ("HAS_TAG", "m", "t", 2),
        ("HAS_CREATOR", "m", "p", 2),
        ("LIKES", "p", "m", 3),
        ("REPLY_OF", "c", "m", 1),
        ("REPLY_OF", "post", "m", 2),
    ] {
        edges.extend((0..n).map(|_| edge(kind, from, to)));
    }
    let index = index(Graph::new(nodes, edges));
    for (source, kind, expected) in [(Q7, Some("Comment"), 12), (A4, None, 36)] {
        let steps = [
            Step::Edge(RawEdge {
                anchor_label: Some("Message"),
                leaf_label: Some("Tag"),
                ..raw("HAS_TAG", 0, 1, Flow::Out, false)
            }),
            Step::Edge(RawEdge {
                leaf_label: Some("Person"),
                ..raw("HAS_CREATOR", 0, 2, Flow::Out, false)
            }),
            Step::Keep(&[0, 2]),
            Step::Edge(RawEdge {
                leaf_label: Some("Person"),
                ..raw("LIKES", 0, 3, Flow::In, true)
            }),
            Step::Edge(RawEdge {
                leaf_label: Some("Message"),
                leaf_kind: kind,
                ..raw("REPLY_OF", 0, 4, Flow::In, true)
            }),
        ];
        assert_eq!(literal_count(index.graph(), &steps), expected);
        compare(&index, source, expected);
    }
}

#[test]
fn empty_matches_and_independent_padding() {
    let source =
        "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b) OPTIONAL MATCH (a)<-[:S]-(c) RETURN count(*)";
    compare(&index(Graph::new(vec![], vec![])), source, 0);
    let nodes = vec![
        Node::new("N", "x", Props::new()),
        Node::new("N", "y", Props::new()),
    ];
    compare(&index(Graph::new(nodes.clone(), vec![])), source, 2);
    let edges = vec![
        edge("S", "x", "x"),
        edge("S", "x", "x"),
        edge("S", "y", "x"),
    ];
    let index = index(Graph::new(nodes, edges));
    // R has no matches, but its separate OPTIONAL does not suppress S's three.
    compare(&index, source, 4);
    assert_eq!(
        literal_count(
            index.graph(),
            &[
                Step::Edge(raw("R", 0, 1, Flow::Out, true)),
                Step::Edge(raw("S", 0, 2, Flow::In, true)),
            ]
        ),
        4
    );
}

#[test]
fn anchor_leaf_and_relationship_predicates_pad_inside_the_optional() {
    let mut node = Node::new("N", "x", Props::new());
    node.props.insert("active".into(), Value::Bool(true));
    node.props
        .insert("label".into(), Value::String("actual".into()));
    let mut edges = vec![
        edge("R", "x", "x"),
        edge("R", "x", "x"),
        edge("R", "x", "x"),
    ];
    edges[0].props.insert("weight".into(), Value::Int(1));
    edges[1].props.insert("weight".into(), Value::Int(1));
    let index = index(Graph::new(vec![node], edges));
    for (pattern, expected) in [
        ("(a:N {active:true})-[:R]->(b:N)", 3),
        ("(a:Missing)-[:R]->(b)", 1),
        ("(a:N:Missing)-[:R]->(b)", 1),
        ("(a {active:false})-[:R]->(b)", 1),
        ("(a {missing:null})-[:R]->(b)", 1),
        ("(a {label:'N'})-[:R]->(b)", 1),
        ("(a {label:'actual'})-[:R]->(b)", 3),
        ("(a)-[:R]->(b:Missing)", 1),
        ("(a)-[:R]->(b {active:false})", 1),
        ("(a)-[:R {weight:1}]->(b)", 2),
        ("(a)-[:R {weight:2}]->(b)", 1),
        ("(b:N)<-[:R]-(a {active:true})", 3),
        ("(b:N)-[:R]->(a:Missing)", 1),
    ] {
        compare(
            &index,
            &format!("MATCH (a:N) OPTIONAL MATCH {pattern} RETURN count(*)"),
            expected,
        );
    }
}

#[test]
fn undirected_parallel_reciprocal_and_self_edges_are_physical_matches() {
    let graph = Graph::new(
        vec![
            Node::new("N", "x", Props::new()),
            Node::new("N", "y", Props::new()),
        ],
        vec![
            edge("R", "x", "x"),
            edge("R", "x", "x"),
            edge("R", "x", "y"),
            edge("R", "x", "y"),
            edge("R", "y", "x"),
        ],
    );
    let expected = literal_count(&graph, &[Step::Edge(raw("R", 0, 1, Flow::Either, true))]);
    assert_eq!(expected, 8);
    let index = index(graph);
    for pattern in ["(a)-[:R]-(b)", "(b)-[:R]-(a)", "()-[:R]-(a)"] {
        compare(
            &index,
            &format!("MATCH (a) OPTIONAL MATCH {pattern} RETURN count(*)"),
            expected,
        );
    }
    let mut graph = index.graph().clone();
    graph.nodes[0].label = "A".into();
    graph.nodes[1].label = "B".into();
    let index = TypedGraphIndex::new(Arc::new(graph)).unwrap();
    for (pattern, expected) in [
        ("(a)-[:R]->(b)", 4),
        ("(b)<-[:R]-(a)", 4),
        ("(a)<-[:R]-(b)", 3),
        ("(b)-[:R]->(a)", 3),
        ("(a)-[:R]-(b)", 5),
        ("(b)-[:R]-(a)", 5),
    ] {
        compare(
            &index,
            &format!("MATCH (a:A) OPTIONAL MATCH {pattern} RETURN count(*)"),
            expected,
        );
    }
}

#[test]
fn generated_multigraphs_match_padded_rows_and_reference() {
    for seed in 0..64u64 {
        let nodes = (0..4)
            .map(|i| Node::new("N", i.to_string(), Props::new()))
            .collect();
        let mut random = seed + 17;
        let mut edges = Vec::new();
        for _ in 0..18 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            edges.push(edge(
                ["R", "S", "T", "U"][(random >> 6) as usize % 4],
                &((random >> 12) % 4).to_string(),
                &((random >> 24) % 4).to_string(),
            ));
        }
        let graph = Graph::new(nodes, edges);
        for (source, first, second) in [
            (
                "MATCH (a)-[:R]->(b), (a)-[:S]->(c) WITH a, b OPTIONAL MATCH (a)-[:T]->(d) OPTIONAL MATCH (e)-[:U]->(b) RETURN count(*)",
                Flow::Out,
                Flow::In,
            ),
            (
                "MATCH (a)-[:R]->(b), (a)-[:S]->(c) OPTIONAL MATCH (d)-[:T]-(a) OPTIONAL MATCH (b)-[:U]-(e) RETURN count(*)",
                Flow::Either,
                Flow::Either,
            ),
        ] {
            let expected = literal_count(
                &graph,
                &[
                    Step::Edge(raw("R", 0, 1, Flow::Out, false)),
                    Step::Edge(raw("S", 0, 2, Flow::Out, false)),
                    Step::Keep(&[0, 1]),
                    Step::Edge(raw("T", 0, 3, first, true)),
                    Step::Edge(raw("U", 1, 4, second, true)),
                ],
            );
            compare(&index(graph.clone()), source, expected);
        }
    }
}

#[test]
fn unsupported_scope_correlation_and_relationship_reuse_fall_back() {
    let index = index(Graph::new(vec![], vec![]));
    for source in [
        "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b), (a)-[:S]->(c) RETURN count(*)",
        "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b)-[:S]->(c) RETURN count(*)",
        "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b) WHERE b.x = 1 RETURN count(*)",
        "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b) WITH a, b WHERE b IS NULL RETURN count(*)",
        "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b) OPTIONAL MATCH (b)-[:S]->(c) RETURN count(*)",
        "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b) OPTIONAL MATCH (a)-[:S]->(b) RETURN count(*)",
        "MATCH (a)-[:R]->(b) OPTIONAL MATCH (a)-[:R]->(c) RETURN count(*)",
        "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b) OPTIONAL MATCH (a)-[:R]->(c) RETURN count(*)",
        "MATCH (a) OPTIONAL MATCH (a)-[:R]->(a) RETURN count(*)",
        "MATCH (a), (b) OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*)",
        "MATCH (a)-[:R]->(b)-[:S]->(a) OPTIONAL MATCH (a)-[:T]->(c) RETURN count(*)",
        "MATCH (a), (b) WITH a OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*)",
        "MATCH (a), (b) WITH a OPTIONAL MATCH (b)-[:R]->(c) RETURN count(*)",
        "MATCH (a) WITH DISTINCT a OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*)",
        "MATCH (a) WITH a AS x OPTIONAL MATCH (x)-[:R]->(b) RETURN count(*)",
        "MATCH (a) WITH * OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*)",
        "MATCH (a) WITH a, a OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*)",
        "MATCH (a) WITH a LIMIT 1 OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*)",
        "MATCH (a) WITH a WHERE true OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*)",
        "MATCH (a) OPTIONAL MATCH (a)-[r:R]->(b) RETURN count(*)",
        "MATCH (a) OPTIONAL MATCH (a)-[:R|S]->(b) RETURN count(*)",
        "MATCH (a) OPTIONAL MATCH (a)-[:R*1..2]->(b) RETURN count(*)",
        "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b {x:$x}) RETURN count(*)",
        "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b) MATCH (c) RETURN count(*)",
        "OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*)",
    ] {
        let query = parse_query(source).unwrap();
        assert_eq!(
            classify_indexed_read_query(&query).unwrap(),
            IndexedReadPlan::ClausePipeline,
            "{source}"
        );
        assert!(
            try_execute(&index, &query, &CypherParameters::new())
                .unwrap()
                .is_none(),
            "{source}"
        );
    }
}

#[test]
fn final_projection_keeps_alias_and_pagination_semantics() {
    let index = index(Graph::new(vec![Node::new("N", "x", Props::new())], vec![]));
    for suffix in ["AS total", "AS total SKIP 0 LIMIT 1", "LIMIT 0", "SKIP 1"] {
        let source = format!("MATCH (a) OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*) {suffix}");
        let query = parse_query(&source).unwrap();
        assert_eq!(
            try_execute(&index, &query, &CypherParameters::new())
                .unwrap()
                .unwrap(),
            run_read_query(index.graph(), &source, &CypherParameters::new()).unwrap()
        );
    }
}

#[test]
fn capped_optional_products_keep_zero_annihilation_and_final_overflow() {
    let mut source = "MATCH (a)".to_string();
    let mut edges = Vec::new();
    for i in 0..63 {
        source.push_str(&format!(" OPTIONAL MATCH (a)-[:R{i}]->()"));
        edges.extend([
            edge(&format!("R{i}"), "x", "x"),
            edge(&format!("R{i}"), "x", "x"),
        ]);
    }
    let index = index(Graph::new(vec![Node::new("N", "x", Props::new())], edges));
    for suffix in ["", " LIMIT 0", " SKIP 1"] {
        let query = parse_query(&format!("{source} RETURN count(*){suffix}")).unwrap();
        assert!(
            try_execute(&index, &query, &CypherParameters::new())
                .unwrap_err()
                .to_string()
                .contains("int64")
        );
    }
    for mandatory in ["MATCH (a), (:Missing)", "MATCH (a)-[:Absent]->()"] {
        let query = parse_query(&format!(
            "{} RETURN count(*)",
            source.replacen("MATCH (a)", mandatory, 1)
        ))
        .unwrap();
        assert_eq!(
            try_execute(&index, &query, &CypherParameters::new())
                .unwrap()
                .unwrap()
                .rows,
            vec![vec![Value::Int(0)]]
        );
    }
}

#[test]
fn budgets_charge_planning_allocation_zero_degree_and_edge_scans() {
    let index = index(Graph::new(
        vec![Node::new("N", "x", Props::new())],
        vec![edge("R", "x", "x")],
    ));
    for (source, work, bytes, expired, context) in [
        (
            "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*)",
            100,
            1,
            false,
            "planning count forest",
        ),
        (
            "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*)",
            100,
            3 * 512,
            false,
            "allocating count weights",
        ),
        (
            "MATCH (a) OPTIONAL MATCH (a)-[:Missing]->(b) RETURN count(*)",
            4,
            100_000,
            false,
            "combining optional count leaves",
        ),
        (
            "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*)",
            5,
            100_000,
            false,
            "scanning optional count edges",
        ),
        (
            "MATCH (a) WITH a OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*)",
            1,
            100_000,
            false,
            "planning count WITH bindings",
        ),
        (
            "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*)",
            100,
            100_000,
            true,
            "timed out",
        ),
    ] {
        let query = parse_query(source).unwrap();
        let limits = read_budget::ReadExecutionBudgetLimits {
            max_candidate_work: work,
            max_intermediate_bytes: bytes,
            max_range_items: 100,
            deadline: Instant::now()
                + if expired {
                    Duration::ZERO
                } else {
                    Duration::from_secs(5)
                },
        };
        let error = read_budget::with_budget(limits, || {
            try_execute(&index, &query, &CypherParameters::new())
        })
        .unwrap_err();
        assert!(error.to_string().contains(context), "{error}: {source}");
        assert!(
            read_budget::with_budget(limits, || execute_read_query_indexed(
                &index,
                &query,
                &CypherParameters::new()
            ))
            .is_err()
        );
    }
}
