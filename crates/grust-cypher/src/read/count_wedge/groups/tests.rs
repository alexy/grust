use super::*;
use crate::read::count_wedge::tests::{edge, indexed};
use grust_core::{Graph, Node, NodeId, Props};
use std::time::{Duration, Instant};

const PREEXISTING_WORK: usize = 7;

fn limits(work: usize, bytes: usize) -> read_budget::ReadExecutionBudgetLimits {
    read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: work,
        max_intermediate_bytes: bytes,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    }
}

fn parallel(copies: usize) -> TypedGraphIndex {
    let edges = (0..copies).map(|_| edge("T", "b", "x")).collect();
    indexed(Graph::new(
        vec![
            Node::new("N", "b", Props::new()),
            Node::new("N", "x", Props::new()),
        ],
        edges,
    ))
}

fn sparse(count: usize) -> TypedGraphIndex {
    let mut nodes = vec![Node::new("N", "b", Props::new())];
    let mut edges = Vec::new();
    for vertex in 0..count {
        let id = format!("x-{vertex:04}");
        nodes.push(Node::new("N", &id, Props::new()));
        edges.push(edge("T", "b", &id));
    }
    indexed(Graph::new(nodes, edges))
}

fn refused(index: &TypedGraphIndex, work: usize) -> (usize, String) {
    let mut visits = 0;
    let error = read_budget::with_budget(limits(work, 0), || {
        read_budget::charge_candidate_work(PREEXISTING_WORK, "preexisting group work")?;
        groups(index, 0, "T", |_, _| {
            visits += 1;
            Ok(())
        })
    })
    .unwrap_err()
    .to_string();
    (visits, error)
}

fn raw_groups(graph: &Graph, center: usize) -> Vec<(u32, u32)> {
    let slot = |id: &NodeId| graph.nodes.iter().position(|node| &node.id == id).unwrap();
    let mut multiplicities = vec![0u32; graph.nodes.len()];
    let center_id = &graph.nodes[center].id;
    for relationship in &graph.edges {
        if relationship.label.as_str() != "T" {
            continue;
        }
        if &relationship.from == center_id {
            multiplicities[slot(&relationship.to)] += 1;
        }
        if &relationship.to == center_id && relationship.from != relationship.to {
            multiplicities[slot(&relationship.from)] += 1;
        }
    }
    multiplicities
        .into_iter()
        .enumerate()
        .filter(|(_, multiplicity)| *multiplicity != 0)
        .map(|(vertex, multiplicity)| (vertex as u32, multiplicity))
        .collect()
}

#[test]
fn compact_multiplicity_checks_u32_and_usize_boundaries() {
    let max = u32::MAX as usize;
    assert_eq!(compact_multiplicity(0, 0).unwrap(), 0);
    assert_eq!(compact_multiplicity(max, 0).unwrap(), u32::MAX);
    assert_eq!(compact_multiplicity(max - 1, 1).unwrap(), u32::MAX);

    // A 64-bit usize can represent u32::MAX + 1 directly. On 32-bit targets
    // the same mathematical boundary must instead be expressed as a checked
    // split sum.
    let one_too_many = if let Some(above_max) = max.checked_add(1) {
        compact_multiplicity(above_max, 0)
    } else {
        compact_multiplicity(max, 1)
    };
    let error = one_too_many.unwrap_err().to_string();
    assert!(error.contains("index edge bound"), "{error}");

    // Exercise checked_add independently of the u32 conversion without
    // constructing an impossible graph-sized fixture.
    let error = compact_multiplicity(usize::MAX, 1).unwrap_err().to_string();
    assert!(error.contains("index edge bound"), "{error}");
}

#[test]
fn sorted_groups_match_raw_physical_slots_and_empty_rows() {
    let nodes = ["b", "a", "c", "d"]
        .into_iter()
        .map(|id| Node::new("N", id, Props::new()))
        .collect();
    let edges = vec![
        edge("T", "b", "b"),
        edge("T", "b", "b"),
        edge("T", "b", "a"),
        edge("T", "b", "a"),
        edge("T", "a", "b"),
        edge("T", "a", "b"),
        edge("T", "a", "b"),
        edge("T", "b", "c"),
        edge("T", "c", "b"),
        edge("T", "c", "b"),
        edge("T", "c", "b"),
        edge("T", "c", "b"),
        edge("Other", "b", "d"),
    ];
    let index = indexed(Graph::new(nodes, edges));
    let mut actual = Vec::new();
    groups(&index, 0, "T", |vertex, multiplicity| {
        actual.push((vertex, multiplicity));
        Ok(())
    })
    .unwrap();
    assert_eq!(actual, raw_groups(index.graph(), 0));
    assert_eq!(actual, [(0, 2), (1, 5), (2, 5)]);

    let empty = indexed(Graph::new(vec![Node::new("N", "b", Props::new())], vec![]));
    let mut called = false;
    groups(&empty, 0, "T", |_, _| {
        called = true;
        Ok(())
    })
    .unwrap();
    assert!(!called);
}

#[test]
fn parallel_chunk_boundaries_keep_exact_work_and_zero_bytes() {
    let empty = parallel(0);
    let mut called = false;
    read_budget::with_budget(limits(PREEXISTING_WORK, 0), || {
        read_budget::charge_candidate_work(PREEXISTING_WORK, "preexisting group work")?;
        groups(&empty, 0, "T", |_, _| {
            called = true;
            Ok(())
        })?;
        let error = read_budget::charge_candidate_work(1, "probing exact empty work")
            .unwrap_err()
            .to_string();
        assert!(error.ends_with("while probing exact empty work"), "{error}");
        Ok(())
    })
    .unwrap();
    assert!(!called);

    for slots in [1, SCAN_CHUNK_SIZE - 1, SCAN_CHUNK_SIZE, SCAN_CHUNK_SIZE + 1] {
        let index = parallel(slots);
        let exact_work = PREEXISTING_WORK + slots + 1;
        let mut actual = None;
        read_budget::with_budget(limits(exact_work, 0), || {
            read_budget::charge_candidate_work(PREEXISTING_WORK, "preexisting group work")?;
            groups(&index, 0, "T", |vertex, multiplicity| {
                actual = Some((vertex, multiplicity));
                Ok(())
            })?;
            let error = read_budget::charge_candidate_work(1, "probing exact group work")
                .unwrap_err()
                .to_string();
            assert!(error.ends_with("while probing exact group work"), "{error}");
            let error = read_budget::charge_intermediate_bytes(1, "probing group allocation")
                .unwrap_err()
                .to_string();
            assert!(error.ends_with("while probing group allocation"), "{error}");
            Ok(())
        })
        .unwrap();
        assert_eq!(actual, Some((1, u32::try_from(slots).unwrap())));

        let (visits, error) = refused(&index, exact_work - 1);
        assert_eq!(visits, 0);
        assert!(
            error.ends_with("while grouping count wedge neighbors"),
            "{error}"
        );
    }

    let index = parallel(SCAN_CHUNK_SIZE + 1);
    for work in [
        PREEXISTING_WORK + SCAN_CHUNK_SIZE - 1,
        PREEXISTING_WORK + SCAN_CHUNK_SIZE,
    ] {
        let (visits, error) = refused(&index, work);
        assert_eq!(visits, 0);
        assert!(
            error.ends_with("while scanning count wedge edges"),
            "{error}"
        );
    }
}

#[test]
fn sparse_rows_refill_before_work_and_charge_each_callback() {
    let index = sparse(SCAN_CHUNK_SIZE + 1);
    for (work, visits, context) in [
        (
            PREEXISTING_WORK + SCAN_CHUNK_SIZE - 1,
            0,
            "scanning count wedge edges",
        ),
        (
            PREEXISTING_WORK + SCAN_CHUNK_SIZE,
            0,
            "grouping count wedge neighbors",
        ),
        (
            PREEXISTING_WORK + 2 * SCAN_CHUNK_SIZE - 1,
            SCAN_CHUNK_SIZE - 1,
            "grouping count wedge neighbors",
        ),
        (
            PREEXISTING_WORK + 2 * SCAN_CHUNK_SIZE,
            SCAN_CHUNK_SIZE,
            "scanning count wedge edges",
        ),
        (
            PREEXISTING_WORK + 2 * SCAN_CHUNK_SIZE + 1,
            SCAN_CHUNK_SIZE,
            "grouping count wedge neighbors",
        ),
    ] {
        let (actual_visits, error) = refused(&index, work);
        assert_eq!(actual_visits, visits, "{work}: {error}");
        assert!(error.ends_with(context), "{work}: {error}");
    }

    let exact_work = PREEXISTING_WORK + 2 * (SCAN_CHUNK_SIZE + 1);
    let mut visits = 0;
    read_budget::with_budget(limits(exact_work, 0), || {
        read_budget::charge_candidate_work(PREEXISTING_WORK, "preexisting group work")?;
        groups(&index, 0, "T", |_, multiplicity| {
            assert_eq!(multiplicity, 1);
            visits += 1;
            Ok(())
        })?;
        let error = read_budget::charge_candidate_work(1, "probing exact sparse work")
            .unwrap_err()
            .to_string();
        assert!(
            error.ends_with("while probing exact sparse work"),
            "{error}"
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(visits, SCAN_CHUNK_SIZE + 1);
}

#[test]
fn chunk_carry_never_splits_a_group_direction_or_self_loop() {
    let mut index = sparse(SCAN_CHUNK_SIZE);
    let mut graph = index.graph().clone();
    let final_id = graph.nodes.last().unwrap().id.clone();
    graph.edges.push(edge("T", "b", final_id.as_str()));
    index = indexed(graph);
    let mut actual = Vec::new();
    groups(&index, 0, "T", |vertex, multiplicity| {
        actual.push((vertex, multiplicity));
        Ok(())
    })
    .unwrap();
    assert_eq!(actual.len(), SCAN_CHUNK_SIZE);
    assert_eq!(
        actual.last(),
        Some(&(u32::try_from(SCAN_CHUNK_SIZE).unwrap(), 2))
    );

    let mut edges: Vec<_> = (0..SCAN_CHUNK_SIZE).map(|_| edge("T", "b", "x")).collect();
    edges.push(edge("T", "x", "b"));
    let reciprocal = indexed(Graph::new(
        vec![
            Node::new("N", "b", Props::new()),
            Node::new("N", "x", Props::new()),
        ],
        edges,
    ));
    let mut value = None;
    groups(&reciprocal, 0, "T", |vertex, multiplicity| {
        value = Some((vertex, multiplicity));
        Ok(())
    })
    .unwrap();
    assert_eq!(
        value,
        Some((1, u32::try_from(SCAN_CHUNK_SIZE + 1).unwrap()))
    );

    let loops = (0..SCAN_CHUNK_SIZE).map(|_| edge("T", "b", "b")).collect();
    let loops = indexed(Graph::new(vec![Node::new("N", "b", Props::new())], loops));
    let mut value = None;
    read_budget::with_budget(limits(2 * SCAN_CHUNK_SIZE + 1, 0), || {
        groups(&loops, 0, "T", |vertex, multiplicity| {
            value = Some((vertex, multiplicity));
            Ok(())
        })
    })
    .unwrap();
    assert_eq!(value, Some((0, u32::try_from(SCAN_CHUNK_SIZE).unwrap())));
}

#[test]
fn chunk_prepayment_precedes_the_first_visitors_own_work() {
    let index = sparse(SCAN_CHUNK_SIZE);
    let mut visits = 0;
    let error = read_budget::with_budget(limits(SCAN_CHUNK_SIZE + 1, 0), || {
        groups(&index, 0, "T", |_, _| {
            visits += 1;
            read_budget::charge_candidate_work(1, "visitor after prepaid chunk")
        })
    })
    .unwrap_err()
    .to_string();
    assert_eq!(visits, 1);
    assert!(
        error.ends_with("while visitor after prepaid chunk"),
        "{error}"
    );
}

#[test]
fn visitor_budget_calls_are_reentrant_and_finish_checks_deadlines() {
    let index = parallel(1);
    let mut visits = 0;
    read_budget::with_budget(limits(9, 0), || {
        groups(&index, 0, "T", |_, _| {
            visits += 1;
            read_budget::with_budget(limits(3, 0), || {
                read_budget::charge_candidate_work(3, "nested group visitor budget")
            })?;
            read_budget::charge_candidate_work(7, "charging inside group visitor")
        })
    })
    .unwrap();
    assert_eq!(visits, 1);
    let mut refused_visits = 0;
    let error = read_budget::with_budget(limits(8, 0), || {
        groups(&index, 0, "T", |_, _| {
            refused_visits += 1;
            read_budget::charge_candidate_work(7, "charging inside group visitor")
        })
    })
    .unwrap_err()
    .to_string();
    assert_eq!(refused_visits, 1);
    assert!(
        error.ends_with("while charging inside group visitor"),
        "{error}"
    );

    let empty = parallel(0);
    let mut deadline_visit = false;
    read_budget::with_budget(limits(10, 0), || {
        let error = groups(&index, 0, "T", |_, _| {
            deadline_visit = true;
            read_budget::expire_deadline_for_test();
            Ok(())
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("timed out"), "{error}");
        let error = groups(&empty, 0, "T", |_, _| Ok(()))
            .unwrap_err()
            .to_string();
        assert!(error.contains("timed out"), "{error}");
        Ok(())
    })
    .unwrap();
    assert!(deadline_visit);
}
