use super::*;
use std::time::{Duration, Instant};

fn limits(work: usize, bytes: usize) -> read_budget::ReadExecutionBudgetLimits {
    read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: work,
        max_intermediate_bytes: bytes,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    }
}

#[test]
fn planner_masks_zero_degree_and_intersection_work_are_budgeted() {
    let index = indexed(simple_graph());
    let query = parse_query(Q).unwrap();
    let mut contexts = std::collections::HashSet::new();
    for work in 0..500 {
        if let Err(error) = read_budget::with_budget(limits(work, 100_000), || {
            try_execute(&index, &query, &CypherParameters::new())
        }) {
            contexts.insert(error.to_string());
        }
    }
    for context in [
        "proving cycle node mentions",
        "proving cycle literal predicates",
        "proving cycle relationship atoms",
        "proving cycle source disjointness",
        "initializing cycle role masks",
        "filtering cycle node mentions",
        "visiting cycle reply sources",
        "grouping cycle adjacency targets",
        "scanning cycle adjacency edges",
        "visiting cycle reply pairs",
        "starting cycle adjacency intersection",
        "intersecting cycle adjacency targets",
    ] {
        assert!(
            contexts.iter().any(|e| e.contains(context)),
            "missing charge: {context}"
        );
    }
    for (bytes, context) in [
        (1, "planning count cycle"),
        (6 * 512, "allocating cycle role masks"),
    ] {
        let error = read_budget::with_budget(limits(100_000, bytes), || {
            try_execute(&index, &query, &CypherParameters::new())
        })
        .unwrap_err();
        assert!(error.to_string().contains(context), "{error}");
    }
    let mut expired = limits(100_000, 100_000);
    expired.deadline = Instant::now();
    assert!(
        read_budget::with_budget(expired, || try_execute(
            &index,
            &query,
            &CypherParameters::new()
        ))
        .unwrap_err()
        .to_string()
        .contains("timed out")
    );
    assert!(read_budget::with_budget(limits(1, 1), || supports(&query)).is_err());
    // Even an empty typed R adjacency still charges its eligible source visit.
    let mut graph = simple_graph();
    graph.edges.clear();
    let empty = indexed(graph);
    let mut errors = Vec::new();
    // Literal predicate checks precede this visit even when no edges exist.
    // Sweep a bounded range instead of coupling the assertion to their cost.
    for work in 0..500 {
        if let Err(e) = read_budget::with_budget(limits(work, 100_000), || {
            try_execute(&empty, &query, &CypherParameters::new())
        }) {
            errors.push(e.to_string());
        }
    }
    assert!(
        errors
            .iter()
            .any(|e| e.contains("visiting cycle reply sources"))
    );
    assert!(
        read_budget::with_budget(limits(1, 1), || execute_read_query_indexed(
            &index,
            &query,
            &CypherParameters::new()
        ))
        .is_err()
    );
}

#[test]
fn empty_index_still_charges_large_proof_strings_before_comparing() {
    let index = indexed(Graph::new(vec![], vec![]));
    let long = "x".repeat(16 * 1024);
    for (source, context) in [
        (
            Q.replace("'C'", &format!("'{long}C'"))
                .replace("'P'", &format!("'{long}P'")),
            "comparing cycle disjointness literals",
        ),
        (
            Q.replace("kind:", &format!("{long}:")),
            "comparing cycle property keys",
        ),
        (
            Q.replace("(u", &format!("({long}")),
            "comparing cycle variable names",
        ),
        (
            Q.replace(":H]", &format!(":{long}]")),
            "comparing cycle relationship types",
        ),
    ] {
        let query = parse_query(&source).unwrap();
        let params = CypherParameters::new();
        for error in [
            read_budget::with_budget(limits(256, 100_000), || supports(&query)).unwrap_err(),
            read_budget::with_budget(limits(256, 100_000), || {
                try_execute(&index, &query, &params)
            })
            .unwrap_err(),
        ] {
            assert!(error.to_string().contains(context), "{error}");
        }
        assert_eq!(
            read_budget::with_budget(limits(1_000_000, 100_000), || {
                try_execute(&index, &query, &params)
            })
            .unwrap()
            .unwrap()
            .rows,
            vec![vec![Value::Int(0)]]
        );
    }
}

#[test]
fn exact_arithmetic_final_overflow_and_alias_allocation() {
    assert_eq!(multiply(u128::MAX, 0).unwrap(), 0);
    assert_eq!(add(u128::MAX - 1, 1).unwrap(), u128::MAX);
    assert!(multiply(u128::MAX, 2).is_err());
    assert!(add(u128::MAX, 1).is_err());
    let query = parse_query(Q).unwrap();
    let cycle = plan::plan(&query).unwrap().unwrap();
    assert_eq!(
        scalar_table(i64::MAX as u128, cycle.projection)
            .unwrap()
            .rows,
        vec![vec![Value::Int(i64::MAX)]]
    );
    for suffix in ["", " LIMIT 0", " SKIP 1"] {
        let query = parse_query(&format!("{Q}{suffix}")).unwrap();
        assert!(
            scalar_table(
                i64::MAX as u128 + 1,
                plan::plan(&query).unwrap().unwrap().projection
            )
            .unwrap_err()
            .to_string()
            .contains("int64")
        );
    }
    let query = parse_query(&Q.replace("AS n", &format!("AS {}", "x".repeat(512)))).unwrap();
    let cycle = plan::plan(&query).unwrap().unwrap();
    let error = read_budget::with_budget(limits(100, 256), || scalar_table(1, cycle.projection))
        .unwrap_err();
    assert!(error.to_string().contains("shaping scalar count result"));
}

#[test]
fn sparse_creator_targets_probe_a_high_degree_knows_neighborhood() {
    let mut graph = simple_graph();
    graph
        .edges
        .retain(|e| e.label.as_str() != "K" && e.from.as_str() != "p");
    for i in 0..128 {
        let id = format!("extra{i}");
        graph.nodes.push(node("N", &id, "X"));
        graph.edges.push(if i % 2 == 0 {
            edge("K", "u", &id)
        } else {
            edge("K", &id, "u")
        });
    }
    graph.edges.extend([
        edge("H", "p", "extra127"),
        edge("K", "u", "extra127"),
        edge("K", "extra127", "u"),
    ]);
    let index = indexed(graph);
    let query = parse_query(Q).unwrap();
    let cycle = plan::plan(&query).unwrap().unwrap();
    let masks = vec![8u8; index.graph().nodes.len()];
    // The sole creator target is at the far end of both sorted K slices.
    // Binary probes fit; scanning either full neighborhood would not.
    let result = read_budget::with_budget(limits(40, 1), || {
        intersect(&index, &cycle, &masks, 1, 2, &CypherParameters::new())
    })
    .unwrap();
    assert_eq!(result, 3);
    compare(&index, Q, 3);
    let mut errors = Vec::new();
    for work in 0..40 {
        if let Err(error) = read_budget::with_budget(limits(work, 1), || {
            intersect(&index, &cycle, &masks, 1, 2, &CypherParameters::new())
        }) {
            errors.push(error.to_string());
        }
    }
    for context in [
        "visiting cycle probe targets",
        "probing cycle adjacency targets",
        "scanning cycle probe edges",
    ] {
        assert!(
            errors.iter().any(|e| e.contains(context)),
            "missing charge: {context}"
        );
    }
}
