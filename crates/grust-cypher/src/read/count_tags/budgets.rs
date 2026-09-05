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
fn exact_subtraction_and_checked_arithmetic_never_cap_early() {
    let huge = i64::MAX as u128 + 100;
    assert_eq!(contribution(huge, 2, huge * 2 - 1, 1, false).unwrap(), 1);
    assert_eq!(contribution(huge, huge, huge, huge, true).unwrap(), 0);
    assert_eq!(contribution(huge, 2, huge - 1, 1, true).unwrap(), 2);
    assert!(contribution(0, 1, 1, 1, false).is_err());
    assert!(contribution(0, 1, 1, 1, true).is_err());
    assert!(contribution(u128::MAX, 2, 0, 1, false).is_err());
    assert!(contribution(u128::MAX, 2, 0, 1, true).is_err());
    assert!(contribution(u128::MAX, 1, 0, 2, false).is_err());
    let query = parse_query(&format!("{PATH} {COUNT}")).unwrap();
    let tags = plan(&query).unwrap().unwrap();
    assert_eq!(
        scalar_table(i64::MAX as u128, tags.projection)
            .unwrap()
            .rows,
        vec![vec![Value::Int(i64::MAX)]]
    );
    assert!(
        scalar_table(i64::MAX as u128 + 1, tags.projection)
            .unwrap_err()
            .to_string()
            .contains("int64")
    );
    for suffix in ["LIMIT 0", "SKIP 1"] {
        let query = parse_query(&format!("{PATH} {COUNT} {suffix}")).unwrap();
        assert!(
            scalar_table(
                i64::MAX as u128 + 1,
                plan(&query).unwrap().unwrap().projection
            )
            .is_err()
        );
    }
}

#[test]
fn budget_refusals_cover_planning_vectors_scans_intersections_and_deadline() {
    let graph = Graph::new(
        ["x", "y"]
            .map(|id| Node::new("N", id, Props::new()))
            .to_vec(),
        vec![
            edge("R", "x", "x"),
            edge("R", "x", "y"),
            edge("R", "y", "y"),
            edge("U", "y", "x"),
        ],
    );
    let index = indexed(graph);
    let query = parse_query(&format!("{PATH} {COUNT}")).unwrap();
    for (work, bytes, context) in [
        (1, 100_000, "proving directed tag count"),
        (100_000, 1, "allocating tag node masks"),
        (100_000, 2, "allocating tag degrees"),
    ] {
        let error = read_budget::with_budget(limits(work, bytes), || {
            try_execute(&index, &query, &CypherParameters::new())
        })
        .unwrap_err();
        assert!(error.to_string().contains(context), "{error}");
    }
    // Check each phase independently without relying on the numeric work
    // totals of earlier phases. Every iteration starts a fresh bounded call.
    let mut contexts = std::collections::HashSet::new();
    for work in 7..150 {
        if let Err(error) = read_budget::with_budget(limits(work, 100_000), || {
            try_execute(&index, &query, &CypherParameters::new())
        }) {
            contexts.insert(error.to_string());
        }
    }
    for context in [
        "initializing tag node masks",
        "filtering tag vertices",
        "initializing tag degrees",
        "counting tag source degrees",
        "scanning filtered tag edges",
        "visiting tag bridge sources",
        "scanning tag bridge edges",
        "grouping tag bridge endpoints",
        "intersecting tag target groups",
        "scanning tag intersection edges",
    ] {
        assert!(
            contexts.iter().any(|error| error.contains(context)),
            "missing budget charge: {context}"
        );
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
    assert!(read_budget::with_budget(limits(1, 100_000), || supports(&query)).is_err());
}

#[test]
fn zero_degree_work_and_output_alias_allocation_are_charged() {
    let index = indexed(Graph::new(vec![Node::new("N", "x", Props::new())], vec![]));
    let query = parse_query(&format!("{PATH} {COUNT}")).unwrap();
    let error = read_budget::with_budget(limits(13, 100_000), || {
        try_execute(&index, &query, &CypherParameters::new())
    })
    .unwrap_err();
    assert!(
        error.to_string().contains("counting tag source degrees"),
        "{error}"
    );
    let source = format!("{PATH} WHERE a <> b RETURN count(*) AS {}", "a".repeat(512));
    let query = parse_query(&source).unwrap();
    let error = read_budget::with_budget(limits(100_000, 256), || {
        try_execute(&index, &query, &CypherParameters::new())
    })
    .unwrap_err();
    assert!(
        error.to_string().contains("shaping scalar count result"),
        "{error}"
    );
}
