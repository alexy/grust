use super::*;
use grust_core::{Edge, Node, Props};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn graph() -> Graph {
    Graph {
        nodes: vec![
            Node::new("A", "a", Props::new()),
            Node::new("B", "b", Props::new()),
            Node::new("C", "c", Props::new()),
        ],
        edges: vec![
            Edge::new("R", "a", "b", Props::new()),
            Edge::new("R", "a", "b", Props::new()),
            Edge::new("S", "b", "c", Props::new()),
            Edge::new("S", "b", "c", Props::new()),
            Edge::new("S", "b", "c", Props::new()),
        ],
    }
}

fn compare(graph: Graph, source: &str, expected: i64) {
    let params = CypherParameters::new();
    let query = parse_query(source).unwrap();
    crate::semantics::analyze(&query).unwrap();
    assert_eq!(
        classify_indexed_read_query(&query).unwrap(),
        IndexedReadPlan::CountFactorized,
        "{source}"
    );
    let index = TypedGraphIndex::new(Arc::new(graph)).unwrap();
    let actual = try_execute(&index, &query, &params)
        .unwrap()
        .expect("eligible count forest");
    assert_eq!(actual.rows, vec![vec![Value::Int(expected)]], "{source}");
    let reference = execute_read_query(index.graph(), &query, &params).unwrap();
    assert_eq!(actual, reference, "{source}");
    assert_eq!(
        execute_read_query_indexed(&index, &query, &params).unwrap(),
        reference
    );
}

#[test]
fn chain_branches_reversal_and_match_boundaries() {
    for source in [
        "MATCH (:A)-[:R]->(:B)-[:S]->(:C) RETURN count(*) AS n",
        "MATCH (:C)<-[:S]-(:B)<-[:R]-(:A) RETURN count(*) AS n",
        "MATCH (:A)-[:R]->(b:B), (b)-[:S]->(:C) RETURN count(*)",
        "MATCH (:A)-[:R]->(b:B) MATCH (b)-[:S]->(:C) RETURN count(*)",
    ] {
        compare(graph(), source, 6);
    }
}

#[test]
fn forest_products_and_empty_domains() {
    compare(graph(), "MATCH (), () RETURN count(*)", 9);
    compare(graph(), "MATCH (x), (x) RETURN count(*)", 3);
    compare(graph(), "MATCH (:Missing), () RETURN count(*) LIMIT 1", 0);
    compare(graph(), "MATCH (:A)-[:Missing]->(:B) RETURN count(*)", 0);
    compare(graph(), "MATCH (a:A)-[:R]->(b:B), (b:C) RETURN count(*)", 0);
    compare(Graph::new(vec![], vec![]), "MATCH () RETURN count(*)", 0);
}

#[test]
fn q4_star_counts_degrees_without_materializing_the_product() {
    let nodes = vec![
        Node::new("Message", "m", Props::new()),
        Node::new("Tag", "t", Props::new()),
        Node::new("Person", "p", Props::new()),
        Node::new("Comment", "c", Props::new()),
    ];
    let mut edges = Vec::new();
    for (label, from, to, degree) in [
        ("HAS_TAG", "m", "t", 2),
        ("HAS_CREATOR", "m", "p", 2),
        ("LIKES", "p", "m", 3),
        ("REPLY_OF", "c", "m", 4),
        ("REPLY_OF", "p", "m", 7),
    ] {
        for _ in 0..degree {
            edges.push(Edge::new(label, from, to, Props::new()));
        }
    }
    compare(
        Graph::new(nodes, edges),
        "MATCH (:Tag)<-[:HAS_TAG]-(m:Message)-[:HAS_CREATOR]->(:Person), (m)<-[:LIKES]-(:Person), (m)<-[:REPLY_OF]-(:Comment) RETURN count(*) AS count",
        48,
    );
}

#[test]
fn undirected_self_loops_are_not_doubled() {
    let g = Graph::new(
        vec![Node::new("X", "x", Props::new())],
        vec![
            Edge::new("R", "x", "x", Props::new()),
            Edge::new("R", "x", "x", Props::new()),
            Edge::new("S", "x", "x", Props::new()),
            Edge::new("S", "x", "x", Props::new()),
            Edge::new("S", "x", "x", Props::new()),
        ],
    );
    compare(g, "MATCH ()-[:R]-()-[:S]-() RETURN count(*)", 6);
    compare(graph(), "MATCH ()-[:R]-() RETURN count(*)", 4);
}

#[test]
fn inline_filters_use_actual_properties() {
    let mut g = graph();
    g.nodes[1]
        .props
        .insert("kind".into(), Value::String("Comment".into()));
    g.edges[2].props.insert("weight".into(), Value::Int(2));
    compare(
        g.clone(),
        "MATCH (:A)-[:R]->(b:B {kind:'Comment'})-[:S {weight:2}]->(:C) RETURN count(*)",
        2,
    );
    compare(
        g,
        "MATCH (:A)-[:R]->(b:B {kind:'Post'})-[:S]->(:C) RETURN count(*)",
        0,
    );
}

#[test]
fn unsupported_forest_shapes_preserve_reference_results() {
    let index = TypedGraphIndex::new(Arc::new(graph())).unwrap();
    for source in [
        "MATCH ()-[:R]->()-[:R]->() RETURN count(*)",
        "MATCH (a)-[:R]->(b), (b)-[:S]->(a) RETURN count(*)",
        "MATCH ()-[r:R]->() RETURN count(*)",
        "MATCH ()-[:R*1..2]->() RETURN count(*)",
        "MATCH (n) WHERE n.id = 'a' RETURN count(*)",
        "MATCH (n {id:$missing}) RETURN count(*)",
        "OPTIONAL MATCH (n) RETURN count(*)",
        "MATCH (n) RETURN count(n)",
        "MATCH (n) RETURN n",
    ] {
        let query = parse_query(source).unwrap();
        assert!(!supports(&query).unwrap(), "{source}");
        assert!(
            try_execute(&index, &query, &CypherParameters::new())
                .unwrap()
                .is_none(),
            "{source}"
        );
        assert_eq!(
            execute_read_query_indexed(&index, &query, &CypherParameters::new())
                .map_err(|error| error.to_string()),
            execute_read_query(index.graph(), &query, &CypherParameters::new())
                .map_err(|error| error.to_string()),
            "{source}"
        );
    }
}

#[test]
fn scalar_limit_and_skip_are_after_aggregation() {
    let index = TypedGraphIndex::new(Arc::new(graph())).unwrap();
    for suffix in ["LIMIT 0", "SKIP 1", "SKIP 0 LIMIT 1"] {
        let query = parse_query(&format!("MATCH () RETURN count(*) {suffix}")).unwrap();
        let actual = try_execute(&index, &query, &CypherParameters::new())
            .unwrap()
            .unwrap();
        assert_eq!(
            actual,
            execute_read_query(index.graph(), &query, &CypherParameters::new()).unwrap()
        );
    }
}

#[test]
fn cap_preserves_zero_annihilation_and_rejects_final_overflow() {
    let mut edges = Vec::new();
    let mut source = "MATCH ()".to_string();
    for i in 0..63 {
        for _ in 0..2 {
            edges.push(Edge::new(format!("R{i}"), "x", "x", Props::new()));
        }
        source.push_str(&format!("-[:R{i}]->()"));
    }
    let index = TypedGraphIndex::new(Arc::new(Graph::new(
        vec![Node::new("X", "x", Props::new())],
        edges,
    )))
    .unwrap();
    let query = parse_query(&format!("{source} RETURN count(*)")).unwrap();
    assert!(
        try_execute(&index, &query, &CypherParameters::new())
            .unwrap_err()
            .to_string()
            .contains("int64")
    );
    let query = parse_query(&format!("{source}, (:Missing) RETURN count(*)")).unwrap();
    assert_eq!(
        try_execute(&index, &query, &CypherParameters::new())
            .unwrap()
            .unwrap()
            .rows,
        vec![vec![Value::Int(0)]]
    );
}

#[test]
fn work_memory_and_deadline_remain_enforced() {
    let index = TypedGraphIndex::new(Arc::new(graph())).unwrap();
    let query = parse_query("MATCH (:A)-[:R]->(:B)-[:S]->(:C) RETURN count(*) LIMIT 1").unwrap();
    for (work, bytes, expired) in [
        (1, 100_000, false),
        (100_000, 1, false),
        (100_000, 100_000, true),
    ] {
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

#[test]
fn generated_small_multigraphs_match_the_old_executor() {
    for seed in 0..32 {
        let mut g = graph();
        g.edges.clear();
        let mut random = seed + 17u64;
        for _ in 0..14 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let from = (random % 3) as usize;
            let to = ((random >> 12) % 3) as usize;
            let label = if random & 8 == 0 { "R" } else { "S" };
            g.edges.push(Edge::new(
                label,
                g.nodes[from].id.clone(),
                g.nodes[to].id.clone(),
                Props::new(),
            ));
        }
        let index = TypedGraphIndex::new(Arc::new(g)).unwrap();
        for source in [
            "MATCH ()-[:R]->()-[:S]->() RETURN count(*)",
            "MATCH ()-[:R]-(b), (b)-[:S]-() RETURN count(*)",
        ] {
            let query = parse_query(source).unwrap();
            assert_eq!(
                try_execute(&index, &query, &CypherParameters::new())
                    .unwrap()
                    .unwrap(),
                execute_read_query(index.graph(), &query, &CypherParameters::new()).unwrap(),
                "seed {seed}: {source}"
            );
        }
    }
}

#[test]
fn output_alias_is_charged_before_cloning() {
    let index = TypedGraphIndex::new(Arc::new(Graph::new(vec![], vec![]))).unwrap();
    let query = parse_query(&format!("RETURN count(*) AS {}", "a".repeat(512))).unwrap();
    let limits = read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: 100,
        max_intermediate_bytes: 256,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    };
    let error = read_budget::with_budget(limits, || {
        execute_read_query_indexed(&index, &query, &CypherParameters::new())
    })
    .unwrap_err();
    assert!(error.to_string().contains("shaping scalar count result"));
}

#[test]
fn budgets_refuse_dp_allocation_and_typed_edge_scanning() {
    let index = TypedGraphIndex::new(Arc::new(graph())).unwrap();
    let query = parse_query("MATCH (:A)-[:R]->(:B)-[:S]->(:C) RETURN count(*)").unwrap();
    let limits = |work, bytes| read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: work,
        max_intermediate_bytes: bytes,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    };
    let allocation_limit = limits(100_000, 3 * 512);
    let error = read_budget::with_budget(allocation_limit, || {
        try_execute(&index, &query, &CypherParameters::new())
    })
    .unwrap_err();
    assert!(
        error.to_string().contains("allocating count weights"),
        "{error}"
    );
    assert!(
        read_budget::with_budget(allocation_limit, || execute_read_query_indexed(
            &index,
            &query,
            &CypherParameters::new()
        ))
        .is_err()
    );

    // Predicate work can grow independently of the adjacency traversal. Find
    // the scan phase itself instead of pinning an unrelated prefix's total.
    let mut reached_scan = false;
    for work in 0..256 {
        let limits = limits(work, 100_000);
        let result = read_budget::with_budget(limits, || {
            try_execute(&index, &query, &CypherParameters::new())
        });
        if let Err(error) = result
            && error.to_string().contains("scanning typed count edges")
        {
            reached_scan = true;
            assert!(
                read_budget::with_budget(limits, || execute_read_query_indexed(
                    &index,
                    &query,
                    &CypherParameters::new()
                ))
                .is_err()
            );
            break;
        }
    }
    assert!(reached_scan, "typed-edge scans must charge candidate work");
}
