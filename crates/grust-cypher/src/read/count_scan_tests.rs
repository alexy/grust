use super::*;
use crate::read_budget::{ReadExecutionBudgetLimits, with_budget};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn fixture() -> TypedGraphIndex {
    let nodes = vec![
        Node::new("Person", "a", Props::new()),
        Node::new(
            "Person",
            "b",
            Props::from([("probe".into(), Value::Null), ("id".into(), Value::Null)]),
        ),
        Node::new(
            "Person",
            "c",
            Props::from([
                ("probe".into(), Value::Json(serde_json::Value::Null)),
                ("id".into(), Value::Int(1)),
            ]),
        ),
        Node::new(
            "Place",
            "d",
            Props::from([
                ("probe".into(), Value::from("é")),
                ("label".into(), Value::Null),
            ]),
        ),
    ];
    let edges = [
        ("R", "a", "b"),
        ("R", "a", "b"),
        ("R", "b", "a"),
        ("R", "a", "a"),
        ("S", "c", "a"),
        ("R", "c", "d"),
    ]
    .into_iter()
    .enumerate()
    .map(|(slot, (kind, from, to))| {
        Edge::new(
            kind,
            from,
            to,
            Props::from([(
                "kind".into(),
                Value::from(if slot % 2 == 0 { "even" } else { "odd" }),
            )]),
        )
        .with_id(format!("e{slot}"))
    })
    .collect();
    TypedGraphIndex::new(Arc::new(Graph::new(nodes, edges))).unwrap()
}

fn query(text: &str) -> Query {
    let parsed = parse_query(text).unwrap();
    crate::semantics::analyze(&parsed).unwrap();
    parsed
}

fn run(index: &TypedGraphIndex, text: &str) -> CypherResultTable {
    let parsed = query(text);
    assert!(supports(&parsed).unwrap(), "{text}");
    let table = try_execute(index, &parsed, &CypherParameters::new())
        .unwrap()
        .unwrap();
    assert_eq!(
        table,
        run_read_query(index.graph(), text, &CypherParameters::new()).unwrap(),
        "{text}"
    );
    table
}

#[test]
fn nonnull_counts_and_null_probes_match_hand_counts() {
    let index = fixture();
    for (text, count) in [
        ("MATCH (n) RETURN (((count(n)))) AS c", 4),
        ("MATCH (n:Person) RETURN count(n) AS c", 3),
        ("MATCH (n:Missing) RETURN count(n) AS c", 0),
        ("MATCH (n:Person:Place) RETURN count(*) AS c", 0),
        ("MATCH (n) WHERE n.probe IS NULL RETURN count(n) AS c", 2),
        (
            "MATCH (n) WHERE n.probe IS NOT NULL RETURN count(n) AS c",
            2,
        ),
        // Node::new injects a string id when absent; only the explicit NULL remains.
        ("MATCH (n) WHERE n.id IS NULL RETURN count(n) AS c", 1),
        ("MATCH (n) WHERE n.label IS NULL RETURN count(n) AS c", 0),
        (
            "MATCH (n) WHERE n.label IS NOT NULL RETURN count(n) AS c",
            4,
        ),
        ("MATCH (n {probe:'é'}) RETURN count(n) AS c", 1),
        (
            "MATCH (n) WHERE n.`adversari.al-missing-🧪` IS NULL RETURN count(*) AS c",
            4,
        ),
        ("MATCH (n) WHERE 'é' = '\\u00e9' RETURN count(*) AS c", 4),
        ("MATCH (n) WHERE 'é' <> '\\u00e9' RETURN count(*) AS c", 0),
        ("MATCH (n) WHERE 'é' = 'é' RETURN count(*) AS c", 0),
        ("MATCH (n) WHERE false RETURN count(*) AS c", 0),
        ("MATCH (n) WHERE null RETURN count(*) AS c", 0),
    ] {
        assert_eq!(
            run(&index, text).rows,
            vec![vec![Value::Int(count)]],
            "{text}"
        );
    }
}

#[test]
fn zero_hops_bind_both_nodes_to_the_same_vertex_without_edges() {
    let index = fixture();
    for (text, count) in [
        (
            "MATCH (p:Person)-[:ABSENT*0..0]->(same:Person) RETURN count(*)",
            3,
        ),
        ("MATCH (p)-[:R*0]->(p) RETURN count(p)", 4),
        (
            "MATCH (p)-[:R*0..0]-(same {probe:'é'}) RETURN count(same)",
            1,
        ),
        (
            "MATCH (p:Person)<-[:R*0..0]-(same:Place) RETURN count(p)",
            0,
        ),
        (
            "MATCH (p)-[*0..0]->(same) WHERE same.probe IS NULL RETURN count(p)",
            2,
        ),
    ] {
        assert_eq!(
            run(&index, text).rows,
            vec![vec![Value::Int(count)]],
            "{text}"
        );
    }
}

#[test]
fn physical_edges_directions_and_self_loops_keep_multiplicity() {
    let index = fixture();
    for (text, count) in [
        ("MATCH ()-[edge]->() RETURN count(edge)", 6),
        ("MATCH ()<-[edge]-() RETURN count(edge)", 6),
        ("MATCH ()-[edge]-() RETURN count(edge)", 11),
        ("MATCH (n)-[edge]->(n) RETURN count(edge)", 1),
        ("MATCH (n)-[edge]-(n) RETURN count(n)", 1),
        ("MATCH (:Person)-[edge:R]->(:Person) RETURN count(edge)", 4),
        ("MATCH (:Person)<-[edge:R]-(:Person) RETURN count(edge)", 4),
        (
            "MATCH ()-[edge:R|S {kind:'even'}]->() RETURN count(edge)",
            3,
        ),
        ("MATCH (n)-[edge]->(:Place) RETURN count(n)", 1),
    ] {
        assert_eq!(
            run(&index, text).rows,
            vec![vec![Value::Int(count)]],
            "{text}"
        );
    }
}

#[test]
fn generated_edge_counts_have_an_independent_raw_orientation_oracle() {
    for seed in 0..32 {
        let nodes: Vec<_> = (0..5)
            .map(|i| {
                Node::new(
                    if i % 2 == 0 { "A" } else { "B" },
                    format!("n{i}"),
                    Props::new(),
                )
            })
            .collect();
        let mut edges = Vec::new();
        let mut directed = 0;
        let mut undirected = 0;
        for e in 0..19 {
            let from = (seed + e * 3) % 5;
            let to = (seed * 3 + e * 2) % 5;
            let kind = if (seed + e) % 3 == 0 { "S" } else { "R" };
            if kind == "R" {
                directed += i64::from(from % 2 == 0 && to % 2 == 1);
                undirected += i64::from(from % 2 == 0 && to % 2 == 1)
                    + i64::from(from != to && to % 2 == 0 && from % 2 == 1);
            }
            edges.push(
                Edge::new(kind, format!("n{from}"), format!("n{to}"), Props::new())
                    .with_id(format!("e{e}")),
            );
        }
        let index = TypedGraphIndex::new(Arc::new(Graph::new(nodes, edges))).unwrap();
        for (text, expected) in [
            ("MATCH (:A)-[e:R]->(:B) RETURN count(e)", directed),
            ("MATCH (:A)-[e:R]-(:B) RETURN count(e)", undirected),
        ] {
            assert_eq!(
                run(&index, text).rows,
                vec![vec![Value::Int(expected)]],
                "seed {seed}"
            );
        }
    }
}

#[test]
fn scalar_unions_and_per_arm_pagination_preserve_reference_contract() {
    let index = fixture();
    for (text, counts) in [
        (
            "MATCH (n:Person) RETURN count(n) AS c UNION MATCH (m:Person) RETURN count(*) AS c",
            vec![3],
        ),
        (
            "MATCH (n) RETURN count(n) AS c UNION ALL MATCH (m) RETURN count(*) AS c",
            vec![4, 4],
        ),
        (
            "MATCH (n:Place) RETURN count(n) AS c UNION ALL MATCH (m) RETURN count(*) AS c UNION MATCH (p:Place) RETURN count(p) AS c",
            vec![1, 4],
        ),
        (
            "MATCH (n) RETURN count(n) AS c LIMIT 0 UNION MATCH (m) RETURN count(*) AS c",
            vec![4],
        ),
        ("MATCH (n) RETURN count(n) AS c SKIP 1", vec![]),
    ] {
        assert_eq!(
            run(&index, text).rows,
            counts
                .into_iter()
                .map(|n| vec![Value::Int(n)])
                .collect::<Vec<_>>()
        );
    }
    let mismatched =
        parse_query("MATCH (n) RETURN count(n) AS a UNION MATCH (m) RETURN count(m) AS b").unwrap();
    assert!(!supports(&mismatched).unwrap());
}

#[test]
fn range_count_retains_inclusive_signed_and_error_semantics() {
    let index = fixture();
    for (arguments, expected) in [
        ("1, 10000", 10000),
        ("1, 5, 2", 3),
        ("5, 1, -2", 3),
        ("1, -3, -2", 3),
        ("5, 1", 0),
        ("1, 5, -1", 0),
        ("-2, -2", 1),
        ("9223372036854775806, 9223372036854775807", 2),
    ] {
        let text = format!("UNWIND range({arguments}) AS i RETURN count(i)");
        assert_eq!(run(&index, &text).rows, vec![vec![Value::Int(expected)]]);
    }
    for text in [
        "UNWIND range(1, 2, 0) AS i RETURN count(*) LIMIT 0",
        "UNWIND range(0, 1000000) AS i RETURN count(*)",
        "UNWIND range(0, 9223372036854775807) AS i RETURN count(*)",
    ] {
        let parsed = query(text);
        assert!(supports(&parsed).unwrap());
        assert!(try_execute(&index, &parsed, &CypherParameters::new()).is_err());
        assert!(run_read_query(index.graph(), text, &CypherParameters::new()).is_err());
    }
    assert_eq!(count_range(i64::MAX, i64::MIN, i64::MIN).unwrap(), 2);
}

#[test]
fn unsupported_nullable_and_expression_shapes_do_not_get_scan_plans() {
    for text in [
        "OPTIONAL MATCH (n) RETURN count(n)",
        "MATCH (n) RETURN count(n.probe)",
        "MATCH (n) RETURN count(DISTINCT n)",
        "MATCH (n) WHERE n.probe = 7 RETURN count(n)",
        "MATCH (n) WHERE n.probe IS NULL OR true RETURN count(n)",
        "MATCH (n) WHERE n.probe.x IS NULL RETURN count(n)",
        "MATCH (n) WITH n RETURN count(n)",
        "MATCH (a), (b) RETURN count(a)",
        "MATCH (a)-[r*0..0]->(b) RETURN count(r)",
        "MATCH (a)-[*0..1]->(b) RETURN count(*)",
        "MATCH p=(a)-[r]->(b) RETURN count(r)",
        "UNWIND range($start, 10) AS i RETURN count(*)",
        "MATCH (n) RETURN count(n) LIMIT $limit",
    ] {
        assert!(!supports(&query(text)).unwrap(), "{text}");
    }
}

fn limits() -> ReadExecutionBudgetLimits {
    ReadExecutionBudgetLimits {
        max_candidate_work: 1_000_000,
        max_intermediate_bytes: 1_000_000,
        max_range_items: 10000,
        deadline: Instant::now() + Duration::from_secs(5),
    }
}

#[test]
fn scan_work_range_memory_and_deadline_limits_are_not_bypassed() {
    let index = fixture();
    let params = CypherParameters::new();
    let scanned = query("MATCH (n) WHERE n.probe IS NULL RETURN count(n)");
    assert!(
        with_budget(
            ReadExecutionBudgetLimits {
                max_candidate_work: 2,
                ..limits()
            },
            || try_execute(&index, &scanned, &params)
        )
        .is_err()
    );
    assert!(
        with_budget(
            ReadExecutionBudgetLimits {
                max_intermediate_bytes: 1,
                ..limits()
            },
            || try_execute(&index, &scanned, &params)
        )
        .is_err()
    );
    assert!(
        with_budget(
            ReadExecutionBudgetLimits {
                deadline: Instant::now(),
                ..limits()
            },
            || try_execute(&index, &scanned, &params)
        )
        .is_err()
    );
    let ranged = query("UNWIND range(1, 100) AS i RETURN count(*)");
    assert!(
        with_budget(
            ReadExecutionBudgetLimits {
                max_range_items: 99,
                ..limits()
            },
            || try_execute(&index, &ranged, &params)
        )
        .is_err()
    );
    assert!(
        with_budget(
            ReadExecutionBudgetLimits {
                max_candidate_work: 99,
                ..limits()
            },
            || try_execute(&index, &ranged, &params)
        )
        .is_err()
    );
    // Counting the range needs no list or row buffer, but its cardinality/work
    // ceiling is retained. This memory budget is smaller than 100 i64 values.
    assert!(
        with_budget(
            ReadExecutionBudgetLimits {
                max_intermediate_bytes: 512,
                ..limits()
            },
            || try_execute(&index, &ranged, &params)
        )
        .unwrap()
        .is_some()
    );
}
