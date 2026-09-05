use super::*;
use grust_core::{Edge, Node, Props};
use std::sync::Arc;

#[path = "oracle.rs"]
mod oracle;
use oracle::{Filters, literal_count};

#[path = "budgets.rs"]
mod budgets;

const PATH: &str = "MATCH (a)<-[:R]-(m)<-[:U]-(c)-[:R]->(b)";
const COUNT: &str = "WHERE a <> b RETURN count(*) AS n";
const ANTI: &str =
    "OPTIONAL MATCH (c)-[h:R]->(a) WITH a, b, h WHERE h IS NULL AND a <> b RETURN count(*) AS n";
const Q5: &str = "MATCH (tag1:Tag)<-[:HAS_TAG]-(message:Message)<-[:REPLY_OF]-(comment:Message {kind:'Comment'})-[:HAS_TAG]->(tag2:Tag) WHERE tag1 <> tag2 RETURN count(*) AS count";
const Q8: &str = "MATCH (tag1:Tag)<-[:HAS_TAG]-(message:Message)<-[:REPLY_OF]-(comment:Message {kind:'Comment'})-[:HAS_TAG]->(tag2:Tag) OPTIONAL MATCH (comment)-[h:HAS_TAG]->(tag1) WITH tag1, tag2, h WHERE h IS NULL AND tag1 <> tag2 RETURN count(*) AS count";

fn edge(kind: &str, from: &str, to: &str) -> Edge {
    Edge::new(kind, from, to, Props::new())
}

fn indexed(graph: Graph) -> TypedGraphIndex {
    TypedGraphIndex::new(Arc::new(graph)).unwrap()
}

fn compare(index: &TypedGraphIndex, source: &str, expected: i64) {
    let params = CypherParameters::new();
    let query = parse_query(source).unwrap();
    crate::semantics::analyze(&query).unwrap();
    assert!(supports(&query).unwrap(), "{source}");
    assert_eq!(
        classify_indexed_read_query(&query).unwrap(),
        IndexedReadPlan::CountFactorized,
        "{source}"
    );
    let result = try_execute(index, &query, &params)
        .unwrap()
        .expect("proven tag count");
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

#[test]
fn q5_q8_and_reordered_anti_fixture_counts() {
    // Exact relevant sfexample CSV rows: 2 Tags, 2 Posts, 6 Comments;
    // Post/Comment inheritance is represented exactly as the Grust adapter.
    let nodes = (1..=2)
        .map(|id| Node::new("Tag", format!("t{id}"), Props::new()))
        .chain([10, 20].map(|id| {
            let mut node = Node::new("Message", format!("p{id}"), Props::new());
            node.props
                .insert("kind".into(), Value::String("Post".into()));
            node
        }))
        .chain((1..=6).map(|id| {
            let mut node = Node::new("Message", format!("c{id}"), Props::new());
            node.props
                .insert("kind".into(), Value::String("Comment".into()));
            node
        }))
        .collect();
    let mut edges: Vec<_> = [
        ("p10", "t1"),
        ("p20", "t2"),
        ("c1", "t1"),
        ("c3", "t1"),
        ("c2", "t2"),
        ("c3", "t2"),
        ("c4", "t2"),
        ("c6", "t2"),
    ]
    .into_iter()
    .map(|(from, to)| edge("HAS_TAG", from, to))
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
        .map(|(from, to)| edge("REPLY_OF", from, to)),
    );
    let index = indexed(Graph::new(nodes, edges));
    let filters = Filters {
        nodes: [
            |node| node.label.as_str() == "Tag",
            |node| node.label.as_str() == "Message",
            |node| {
                node.label.as_str() == "Message"
                    && node.props.get("kind") == Some(&Value::String("Comment".into()))
            },
            |node| node.label.as_str() == "Tag",
        ],
        ..Filters::default()
    };
    for (source, anti, expected) in [(Q5, false, 3), (Q8, true, 2)] {
        assert_eq!(
            literal_count(index.graph(), "HAS_TAG", "REPLY_OF", filters, anti),
            expected
        );
        compare(&index, source, expected);
    }
    compare(
        &index,
        &Q8.replace("h IS NULL AND tag1 <> tag2", "tag2 <> tag1 AND h IS NULL"),
        2,
    );
}

#[test]
fn parallel_bridges_tags_and_anti_edges_keep_bag_semantics() {
    let nodes = ["m", "c", "a", "b"]
        .map(|id| Node::new("N", id, Props::new()))
        .to_vec();
    let mut edges = Vec::new();
    for (kind, from, to, n) in [
        ("U", "c", "m", 2),
        ("R", "m", "a", 2),
        ("R", "m", "b", 3),
        ("R", "c", "a", 4),
        ("R", "c", "b", 5),
    ] {
        edges.extend((0..n).map(|_| edge(kind, from, to)));
    }
    let index = indexed(Graph::new(nodes, edges));
    // Two bridge copies times (2*5 + 3*4). All left targets have anti witnesses.
    compare(&index, &format!("{PATH} {COUNT}"), 44);
    compare(&index, &format!("{PATH} {ANTI}"), 0);
    for (anti, expected) in [(false, 44), (true, 0)] {
        assert_eq!(
            literal_count(index.graph(), "R", "U", Filters::default(), anti),
            expected
        );
    }
}

#[test]
fn anti_existence_ignores_right_target_mask_and_right_edge_filters() {
    let nodes = [
        ("Source", "m"),
        ("Source", "c"),
        ("A", "a"),
        ("B", "b"),
        ("A", "free"),
    ]
    .map(|(label, id)| Node::new(label, id, Props::new()))
    .to_vec();
    let mut edges = vec![
        edge("U", "c", "m"),
        edge("U", "c", "m"),
        edge("R", "m", "a"),
        edge("R", "m", "free"),
    ];
    // These witnesses fail both b's label mask and right-edge property filter.
    edges.extend((0..5).map(|_| edge("R", "c", "a")));
    for _ in 0..3 {
        let mut e = edge("R", "c", "b");
        e.props.insert("accept".into(), Value::Bool(true));
        edges.push(e);
    }
    let index = indexed(Graph::new(nodes, edges));
    let path = "MATCH (a:A)<-[:R]-(m:Source)<-[:U]-(c:Source)-[:R {accept:true}]->(b:B)";
    let filters = Filters {
        nodes: [
            |n| n.label.as_str() == "A",
            |n| n.label.as_str() == "Source",
            |n| n.label.as_str() == "Source",
            |n| n.label.as_str() == "B",
        ],
        relationships: [
            |_| true,
            |_| true,
            |e| e.props.get("accept") == Some(&Value::Bool(true)),
        ],
    };
    for (anti, suffix, expected) in [(false, COUNT, 12), (true, ANTI, 6)] {
        assert_eq!(
            literal_count(index.graph(), "R", "U", filters, anti),
            expected
        );
        compare(&index, &format!("{path} {suffix}"), expected);
    }
}

#[test]
fn self_loops_and_coincident_source_roles_do_not_reuse_edges() {
    let nodes = ["x", "y"]
        .map(|id| Node::new("N", id, Props::new()))
        .to_vec();
    let edges = vec![
        edge("U", "x", "x"),
        edge("U", "x", "x"),
        edge("R", "x", "x"),
        edge("R", "x", "x"),
        edge("R", "x", "y"),
        edge("R", "y", "x"),
    ];
    let index = indexed(Graph::new(nodes, edges));
    compare(&index, &format!("{PATH} {COUNT}"), 8);
    compare(&index, &format!("{PATH} {ANTI}"), 0);
    for anti in [false, true] {
        let expected = literal_count(index.graph(), "R", "U", Filters::default(), anti);
        compare(
            &index,
            &format!("{PATH} {}", if anti { ANTI } else { COUNT }),
            expected,
        );
    }
}

#[test]
fn generated_multigraphs_match_raw_edge_tuples_and_reference() {
    for seed in 0..96u64 {
        let nodes = (0..4)
            .map(|i| {
                let mut n = Node::new(
                    if i % 2 == 0 { "A" } else { "B" },
                    i.to_string(),
                    Props::new(),
                );
                n.props.insert("on".into(), Value::Bool(i < 3));
                n
            })
            .collect();
        let mut edges = Vec::new();
        let mut random = seed + 17;
        for _ in 0..20 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let mut e = edge(
                if (random >> 6) % 3 == 0 { "U" } else { "R" },
                &((random >> 12) % 4).to_string(),
                &((random >> 24) % 4).to_string(),
            );
            e.props.insert("on".into(), Value::Bool(random & 8 != 0));
            edges.push(e);
        }
        let index = indexed(Graph::new(nodes, edges));
        for (path, filters) in [
            (PATH, Filters::default()),
            (
                "MATCH (a {on:true})<-[:R {on:true}]-(m)<-[:U {on:true}]-(c {on:true})-[:R]->(b:A)",
                Filters {
                    nodes: [
                        |n| n.props.get("on") == Some(&Value::Bool(true)),
                        |_| true,
                        |n| n.props.get("on") == Some(&Value::Bool(true)),
                        |n| n.label.as_str() == "A",
                    ],
                    relationships: [
                        |e| e.props.get("on") == Some(&Value::Bool(true)),
                        |e| e.props.get("on") == Some(&Value::Bool(true)),
                        |_| true,
                    ],
                },
            ),
        ] {
            for (anti, suffix) in [(false, COUNT), (true, ANTI)] {
                let expected = literal_count(index.graph(), "R", "U", filters, anti);
                compare(&index, &format!("{path} {suffix}"), expected);
            }
        }
    }
}

#[test]
fn literal_predicates_use_real_properties_and_reference_equality() {
    let mut m = Node::new("N", "m", Props::new());
    m.props.insert("kind".into(), Value::String("ok".into()));
    let mut c = Node::new("N", "c", Props::new());
    c.props
        .insert("label".into(), Value::String("stored".into()));
    let mut a = Node::new("T", "a", Props::new());
    a.props.insert("v".into(), Value::Float(1.0));
    let mut b = Node::new("T", "b", Props::new());
    b.props.insert("v".into(), Value::Int(2));
    let mut edges = vec![
        edge("U", "c", "m"),
        edge("R", "m", "a"),
        edge("R", "c", "b"),
    ];
    edges[0].props.insert("v".into(), Value::Float(3.0));
    let index = indexed(Graph::new(vec![m, c, a, b], edges));
    for (path, expected) in [
        (
            "MATCH (a:T {v:1})<-[:R]-(m:N {kind:'ok'})<-[:U {v:3}]-(c:N {label:'stored'})-[:R]->(b:T {v:2.0})",
            1,
        ),
        ("MATCH (a)<-[:R]-(m)<-[:U]-(c {label:'N'})-[:R]->(b)", 0),
        ("MATCH (a {missing:null})<-[:R]-(m)<-[:U]-(c)-[:R]->(b)", 0),
        ("MATCH (a)<-[:R {missing:null}]-(m)<-[:U]-(c)-[:R]->(b)", 0),
        ("MATCH (a:T:Absent)<-[:R]-(m)<-[:U]-(c)-[:R]->(b)", 0),
    ] {
        for suffix in [COUNT, ANTI] {
            compare(&index, &format!("{path} {suffix}"), expected);
        }
    }
}

#[test]
fn unsupported_shapes_keep_the_complete_proof_conservative() {
    let index = indexed(Graph::new(vec![], vec![]));
    let base = format!("{PATH} {COUNT}");
    let anti = format!("{PATH} {ANTI}");
    let sources = vec![
        base.replace("a <> b", "a.id <> b.id"),
        base.replace("a <> b", "id(a) <> id(b)"),
        base.replace("a <> b", "a <> b AND true"),
        base.replace("a <> b", "a = b"),
        base.replace("<-[:U]-", "<-[:R]-"),
        base.replace("<-[:U]-", "-[:U]-"),
        base.replace("<-[:R]-", "<-[:R|S]-"),
        base.replace("<-[:R]-", "<-[r:R]-"),
        base.replace("<-[:R]-", "<-[:R*1..2]-"),
        base.replace("(a)", "()"),
        base.replace("(c)", "(m)"),
        base.replace("(a)", "(a {v:$x})"),
        base.replace("count(*)", "count(a)"),
        format!("{base} ORDER BY n"),
        anti.replace("(c)-[h:R]->(a)", "(c:N)-[h:R]->(a)"),
        anti.replace("(c)-[h:R]->(a)", "(c)-[h:R {on:true}]->(a)"),
        anti.replace("(c)-[h:R]->(a)", "(c)-[h:R]-(a)"),
        anti.replace("(c)-[h:R]->(a)", "(m)-[h:R]->(a)"),
        anti.replace("(c)-[h:R]->(a)", "(c)-[h:U]->(a)"),
        anti.replace("(c)-[h:R]->(a)", "(c)-[m:R]->(a)"),
        anti.replace("WITH a, b, h", "WITH DISTINCT a, b, h"),
        anti.replace("WITH a, b, h", "WITH a AS a, b, h"),
        anti.replace("WITH a, b, h", "WITH a, b, h LIMIT 1"),
        anti.replace("WITH a, b, h", "WITH a, b, m"),
        anti.replace("h IS NULL AND a <> b", "a <> b"),
        anti.replace("h IS NULL AND a <> b", "h IS NOT NULL AND a <> b"),
        anti.replace("h IS NULL AND a <> b", "h IS NULL OR a <> b"),
        anti.replace("OPTIONAL MATCH", "WHERE a <> b OPTIONAL MATCH"),
        anti.replace("WITH a, b, h", "WHERE true WITH a, b, h"),
    ];
    for source in sources {
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
fn empty_graph_and_final_pagination_match_reference() {
    let index = indexed(Graph::new(vec![], vec![]));
    for suffix in [COUNT, ANTI] {
        compare(&index, &format!("{PATH} {suffix}"), 0);
        for tail in ["LIMIT 0", "SKIP 1", "SKIP 0 LIMIT 1"] {
            let source = format!("{PATH} {suffix} {tail}");
            let query = parse_query(&source).unwrap();
            assert_eq!(
                try_execute(&index, &query, &CypherParameters::new())
                    .unwrap()
                    .unwrap(),
                run_read_query(index.graph(), &source, &CypherParameters::new()).unwrap()
            );
        }
    }
}
