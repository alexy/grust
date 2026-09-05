//! Preparation and per-vertex probe accounting for mandatory adjacency views.

use super::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

fn limits(work: usize, bytes: usize) -> read_budget::ReadExecutionBudgetLimits {
    read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: work,
        max_intermediate_bytes: bytes,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    }
}

#[test]
fn preparation_has_exact_resolution_work_and_storage_costs() {
    let query = query("MATCH (b {keep:true})-[:RR]->(), (b)-[:SSS]->() RETURN count(*)");
    let proven = plan(&query).unwrap().unwrap();
    let slot = role(&proven.forest, "b");
    let index = indexed(Graph::default());
    let work = 2 * "RR".len() + 1 + 2 * "SSS".len() + 1;
    let bytes = 2 * std::mem::size_of::<Required<'_>>();

    read_budget::with_budget(limits(work, bytes), || {
        assert!(prepare(&index, &proven.forest, slot)?.is_some());
        let error =
            read_budget::charge_candidate_work(1, "probing exact preparation work").unwrap_err();
        assert!(
            error
                .to_string()
                .ends_with("probing exact preparation work"),
            "{error}"
        );
        let error = read_budget::charge_intermediate_bytes(1, "probing exact preparation bytes")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .ends_with("probing exact preparation bytes"),
            "{error}"
        );
        Ok(())
    })
    .unwrap();

    let error = read_budget::with_budget(limits(work - 1, bytes), || {
        prepare(&index, &proven.forest, slot)
    })
    .err()
    .expect("resolution work should be refused");
    assert!(
        error
            .to_string()
            .ends_with("resolving mandatory count adjacency"),
        "{error}"
    );
    let error = read_budget::with_budget(limits(work, bytes - 1), || {
        prepare(&index, &proven.forest, slot)
    })
    .err()
    .expect("preparation storage should be refused");
    assert!(
        error
            .to_string()
            .ends_with("preparing mandatory count adjacency"),
        "{error}"
    );
}

#[test]
fn prepared_views_reuse_type_resolution_and_charge_only_actual_probes() {
    let long_r = "R".repeat(8 * 1024);
    let long_s = "S".repeat(8 * 1024);
    let source =
        format!("MATCH (b {{keep:true}})-[:{long_r}]-(), (b)-[:{long_s}]-() RETURN count(*)");
    let query = query(&source);
    let proven = plan(&query).unwrap().unwrap();
    let slot = role(&proven.forest, "b");

    for (incoming, copies, probes_per_accept) in [(false, 1, 2), (false, 1024, 2), (true, 1, 4)] {
        let mut edges = Vec::new();
        for kind in [&long_r, &long_s] {
            edges.extend((0..copies).map(|_| {
                if incoming {
                    edge(kind, "x", "b")
                } else {
                    edge(kind, "b", "x")
                }
            }));
        }
        let index = indexed(Graph::new(
            vec![node("N", "b", true), node("N", "x", false)],
            edges,
        ));
        let prepared = prepare(&index, &proven.forest, slot).unwrap().unwrap();
        let repetitions = 4;
        let exact_work = probes_per_accept * repetitions;
        read_budget::with_budget(limits(exact_work, 0), || {
            for _ in 0..repetitions {
                assert!(prepared.accepts(0)?);
            }
            let error =
                read_budget::charge_candidate_work(1, "probing exact prepared work").unwrap_err();
            assert!(
                error.to_string().ends_with("probing exact prepared work"),
                "{error}"
            );
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn a_missing_second_atom_is_probed_and_short_circuits_later_atoms() {
    let query = query("MATCH (b {keep:true})-[:R]->(), (b)-[:S]->(), (b)-[:T]->() RETURN count(*)");
    let proven = plan(&query).unwrap().unwrap();
    let slot = role(&proven.forest, "b");
    let index = indexed(Graph::new(
        vec![node("N", "b", true), node("N", "x", false)],
        vec![edge("R", "b", "x"), edge("T", "b", "x")],
    ));
    let prepared = prepare(&index, &proven.forest, slot).unwrap().unwrap();

    read_budget::with_budget(limits(2, 0), || {
        assert!(!prepared.accepts(0)?);
        let error =
            read_budget::charge_candidate_work(1, "probing missing-second work").unwrap_err();
        assert!(
            error.to_string().ends_with("probing missing-second work"),
            "{error}"
        );
        Ok(())
    })
    .unwrap();
    let error = read_budget::with_budget(limits(1, 0), || prepared.accepts(0)).unwrap_err();
    assert!(
        error
            .to_string()
            .ends_with("probing mandatory count adjacency"),
        "{error}"
    );
}

#[test]
fn disabled_and_empty_candidate_paths_do_not_prepare_views() {
    let disabled = query("MATCH (b {keep:true})-[:R]->() RETURN count(*)");
    let proven = plan(&disabled).unwrap().unwrap();
    let slot = role(&proven.forest, "b");
    let index = indexed(Graph::default());
    read_budget::with_budget(limits(0, 0), || {
        assert!(prepare(&index, &proven.forest, slot)?.is_none());
        Ok(())
    })
    .unwrap();

    let empty_candidates =
        query("MATCH (b:Missing {keep:true})-[:R]->(:A), (b)-[:S]->(:C) RETURN count(*)");
    let index = indexed(Graph::new(vec![node("Other", "other", false)], Vec::new()));
    // Four syntax occurrences reserve 4 * 512 planner bytes, three roles
    // allocate one u64 each, and the scalar output costs 128 + 4 bytes. There
    // is deliberately no Required storage because b has no candidates.
    let exact_bytes = 4 * 512 + 3 * 8 + 128 + 4;
    let result = read_budget::with_budget(limits(100_000, exact_bytes), || {
        let result = try_execute(&index, &empty_candidates, &CypherParameters::new())?.unwrap();
        let error =
            read_budget::charge_intermediate_bytes(1, "probing empty candidate bytes").unwrap_err();
        assert!(
            error.to_string().ends_with("probing empty candidate bytes"),
            "{error}"
        );
        Ok(result)
    })
    .unwrap();
    assert_eq!(result.rows, vec![vec![Value::Int(0)]]);
}

#[test]
fn preparation_and_probes_observe_expired_deadlines() {
    let query = query("MATCH (b {keep:true})-[:R]->(), (b)-[:S]->() RETURN count(*)");
    let proven = plan(&query).unwrap().unwrap();
    let slot = role(&proven.forest, "b");
    let index = indexed(Graph::new(
        vec![node("N", "b", true)],
        vec![edge("R", "b", "b"), edge("S", "b", "b")],
    ));

    let error = read_budget::with_budget(limits(100, 100), || {
        read_budget::expire_deadline_for_test();
        prepare(&index, &proven.forest, slot)
    })
    .err()
    .expect("expired preparation should fail");
    assert!(error.to_string().contains("timed out"), "{error}");

    let prepared = prepare(&index, &proven.forest, slot).unwrap().unwrap();
    let error = read_budget::with_budget(limits(100, 0), || {
        read_budget::expire_deadline_for_test();
        prepared.accepts(0)
    })
    .unwrap_err();
    assert!(error.to_string().contains("timed out"), "{error}");
}

#[test]
fn long_types_resolve_on_hits_misses_and_empty_indexes() {
    let kind = "T".repeat(16 * 1024);
    let query = query(&format!(
        "MATCH (b {{keep:true}})-[:{kind}]->(), (b)-[:S]->() RETURN count(*)"
    ));
    let proven = plan(&query).unwrap().unwrap();
    let slot = role(&proven.forest, "b");
    let resolution_work = kind.len() * 2 + 1 + 3;
    let bytes = 2 * std::mem::size_of::<Required<'_>>();

    for (graph, expected, probe_work) in [
        (Graph::default(), false, 1),
        (Graph::new(vec![node("N", "b", true)], Vec::new()), false, 1),
        (
            Graph::new(
                vec![node("N", "b", true)],
                vec![edge(&kind, "b", "b"), edge("S", "b", "b")],
            ),
            true,
            2,
        ),
    ] {
        let index = indexed(graph);
        let error = read_budget::with_budget(limits(resolution_work - 1, bytes), || {
            prepare(&index, &proven.forest, slot)
        })
        .err()
        .expect("long type resolution should be refused");
        assert!(
            error
                .to_string()
                .ends_with("resolving mandatory count adjacency"),
            "{error}"
        );

        let prepared = read_budget::with_budget(limits(resolution_work, bytes), || {
            prepare(&index, &proven.forest, slot)
        })
        .unwrap()
        .unwrap();
        assert_eq!(
            read_budget::with_budget(limits(probe_work, 0), || prepared.accepts(0)).unwrap(),
            expected
        );
    }
}

#[test]
fn absent_adjacency_skips_long_properties_but_not_weight_initialization() {
    let text = "x".repeat(16 * 1024);
    let mut b = node("B", "b", true);
    b.props.insert("text".into(), Value::String(text.clone()));
    let index = indexed(Graph::new(vec![b], Vec::new()));
    let source =
        format!("MATCH (b:B {{text:'{text}'}})-[:R]->(:A), (b)-[:S]->(:C) RETURN count(*)");
    let query = query(&source);
    let params = CypherParameters::new();
    let proven = plan(&query).unwrap().unwrap();
    let pattern = proven.forest.nodes[role(&proven.forest, "b")][0];
    let error = read_budget::with_budget(limits(512, 100_000), || {
        node_matches(&index.graph().nodes[0], pattern, &params)
    })
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("comparing literal count strings"),
        "{error}"
    );
    let actual = read_budget::with_budget(limits(512, 100_000), || {
        try_execute(&index, &query, &params)
    })
    .unwrap()
    .unwrap();
    assert_eq!(actual.rows, vec![vec![Value::Int(0)]]);
    assert_eq!(
        actual,
        execute_read_query(index.graph(), &query, &params).unwrap()
    );

    // Four syntactic node occurrences reserve the same planner bytes; even a
    // pruned one-vertex role still allocates its complete eight-byte vector.
    let error = read_budget::with_budget(limits(512, 4 * 512 + 7), || {
        try_execute(&index, &query, &params)
    })
    .unwrap_err();
    assert!(
        error.to_string().ends_with("allocating count weights"),
        "{error}"
    );
    let mut selection_refused = false;
    let mut initialization_refused = false;
    for work in 0..128 {
        if let Err(error) = read_budget::with_budget(limits(work, 100_000), || {
            try_execute(&index, &query, &params)
        }) {
            selection_refused |= error
                .to_string()
                .ends_with("selecting mandatory count candidates");
            initialization_refused |= error.to_string().ends_with("initializing count weights");
        }
    }
    assert!(selection_refused && initialization_refused);
}
