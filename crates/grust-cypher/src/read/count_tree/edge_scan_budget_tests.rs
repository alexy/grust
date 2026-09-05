//! Physical-slot prepayment keeps successful totals and bounds partial work.

use super::*;
use grust_core::{Edge, Node, Props};
use std::sync::Arc;
use std::time::{Duration, Instant};

const BOUNDARIES: [usize; 8] = [0, 1, 255, 256, 257, 511, 512, 513];
const CHAIN_BYTES: usize = 3 * 512 + 3 * 3 * 8 + 132;

fn edge(kind: &str, from: &str, to: &str) -> Edge {
    Edge::new(kind, from, to, Props::new())
}

fn chain(slots: usize, incoming: bool, with_s: bool) -> Graph {
    let nodes = [("A", "a"), ("B", "b"), ("C", "c")]
        .into_iter()
        .map(|(label, id)| Node::new(label, id, Props::new()))
        .collect();
    let (from, to) = if incoming { ("b", "a") } else { ("a", "b") };
    let mut edges: Vec<_> = (0..slots).map(|_| edge("R", from, to)).collect();
    if with_s {
        edges.push(edge("S", "b", "c"));
    }
    Graph::new(nodes, edges)
}

fn indexed(graph: Graph) -> TypedGraphIndex {
    TypedGraphIndex::new(Arc::new(graph)).unwrap()
}

fn query(source: &str) -> Query {
    let query = parse_query(source).unwrap();
    crate::semantics::analyze(&query).unwrap();
    query
}

fn limits(work: usize, bytes: usize) -> read_budget::ReadExecutionBudgetLimits {
    read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: work,
        max_intermediate_bytes: bytes,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    }
}

fn exact(index: &TypedGraphIndex, source: &str, expected: i64, work: usize, bytes: usize) {
    let query = query(source);
    let params = CypherParameters::new();
    let actual = read_budget::with_budget(limits(work, bytes), || {
        let table = try_execute(index, &query, &params)?.expect("proven forest");
        let error = read_budget::charge_candidate_work(1, "checking exact edge work").unwrap_err();
        assert!(error.to_string().ends_with("checking exact edge work"));
        let error =
            read_budget::charge_intermediate_bytes(1, "checking exact edge bytes").unwrap_err();
        assert!(error.to_string().ends_with("checking exact edge bytes"));
        Ok(table)
    })
    .unwrap();
    assert_eq!(actual.rows, vec![vec![Value::Int(expected)]], "{source}");
    let reference = execute_read_query(index.graph(), &query, &params).unwrap();
    assert_eq!(actual, reference, "{source}");
    assert_eq!(
        run_read_query_indexed(index, source, &params).unwrap(),
        reference
    );
    assert_eq!(
        read_budget::with_budget(limits(100_000, 100_000), || {
            execute_read_query_indexed(index, &query, &params)
        })
        .unwrap(),
        reference
    );
}

#[test]
fn directed_chunk_boundaries_keep_exact_physical_work_and_no_added_storage() {
    for incoming in [false, true] {
        let relationship = if incoming { "<-[:R]-" } else { "-[:R]->" };
        let source = format!("MATCH (:A){relationship}(:B)-[:S]->(:C) RETURN count(*)");
        for slots in BOUNDARIES {
            let index = indexed(chain(slots, incoming, true));
            // Three pattern units, 12 array/root visits, nine label-lookup
            // units, three candidates, six label predicates, two branch
            // visits, and one S slot: exactly 36 plus the R physical slots.
            exact(&index, &source, slots as i64, 36 + slots, CHAIN_BYTES);
        }
    }
}

#[test]
fn first_second_and_partial_tail_chunks_refuse_before_affordable_prefixes() {
    let index = indexed(chain(513, false, true));
    let query = query("MATCH (:A)-[:R]->(:B)-[:S]->(:C) RETURN count(*)");
    // Postorder S traversal and all preceding node work cost 33 units before
    // the first R chunk. A refused reservation must debit none of its slots.
    for (completed_slots, affordable_prefix) in [(0, 255), (256, 255), (512, 0)] {
        let work = 33 + completed_slots + affordable_prefix;
        read_budget::with_budget(limits(work, CHAIN_BYTES), || {
            let error = try_execute(&index, &query, &CypherParameters::new()).unwrap_err();
            assert!(
                error.to_string().ends_with("scanning typed count edges"),
                "{error}"
            );
            read_budget::charge_candidate_work(
                affordable_prefix,
                "unused partial chunk allowance",
            )?;
            assert!(read_budget::charge_candidate_work(1, "exact refused reservation").is_err());
            Ok(())
        })
        .unwrap();
        assert!(
            read_budget::with_budget(limits(work, CHAIN_BYTES), || {
                execute_read_query_indexed(&index, &query, &CypherParameters::new())
            })
            .is_err(),
            "the indexed entrypoint must not retry an unbounded fallback"
        );
    }
}

#[test]
fn empty_rows_have_no_scan_charge_and_full_weight_arrays_remain_required() {
    let index = indexed(chain(0, false, true));
    let source = "MATCH (:A)-[:R]->(:B)-[:S]->(:C) RETURN count(*)";
    exact(&index, source, 0, 36, CHAIN_BYTES);
    let query = query(source);
    let error = read_budget::with_budget(limits(35, CHAIN_BYTES), || {
        try_execute(&index, &query, &CypherParameters::new())
    })
    .unwrap_err();
    assert!(error.to_string().ends_with("summing count roots"));
    for (bytes, context) in [
        (3 * 512 + 3 * 8 - 1, "allocating count weights"),
        (CHAIN_BYTES - 1, "shaping scalar count result"),
    ] {
        let error = read_budget::with_budget(limits(1000, bytes), || {
            try_execute(&index, &query, &CypherParameters::new())
        })
        .unwrap_err();
        assert!(error.to_string().ends_with(context), "{error}");
    }
}

#[test]
fn rejected_properties_and_zero_child_weights_still_consume_complete_rows() {
    const SLOTS: usize = 257;
    for with_s in [false, true] {
        for accepted in [false, true] {
            let mut graph = chain(SLOTS, false, with_s);
            for edge in &mut graph.edges {
                if edge.label.as_str() == "R" {
                    edge.props.insert("ok".into(), Value::Bool(accepted));
                }
            }
            let index = indexed(graph);
            let source = "MATCH (:A)-[:R {ok:true}]->(:B)-[:S]->(:C) RETURN count(*)";
            let expected = if with_s && accepted { SLOTS as i64 } else { 0 };
            // A one-entry map costs three units for lookup of "ok" on every
            // R edge, even after a rejected predicate or zero subtree weight.
            let work = 35 + usize::from(with_s) + 4 * SLOTS;
            exact(&index, source, expected, work, CHAIN_BYTES);
        }
        let index = indexed(chain(SLOTS, false, with_s));
        let source = "MATCH (:A)-[:R]->(:B)-[:S]->(:C) RETURN count(*)";
        exact(
            &index,
            source,
            if with_s { SLOTS as i64 } else { 0 },
            35 + usize::from(with_s) + SLOTS,
            CHAIN_BYTES,
        );
    }
}

#[test]
fn predicate_refusal_keeps_the_paid_chunk_and_does_not_retry() {
    let mut graph = chain(257, false, true);
    for edge in &mut graph.edges {
        edge.props.insert("ok".into(), Value::Bool(true));
    }
    let index = indexed(graph);
    let query = query("MATCH (:A)-[:R {ok:true}]->(:B)-[:S]->(:C) RETURN count(*)");
    // The first R chunk consumes 256 units before its first three-unit map
    // lookup. Two remaining units cannot pay that predicate.
    let work = 33 + 256 + 2;
    read_budget::with_budget(limits(work, CHAIN_BYTES), || {
        let error = try_execute(&index, &query, &CypherParameters::new()).unwrap_err();
        assert!(
            error
                .to_string()
                .ends_with("looking up literal count properties"),
            "{error}"
        );
        read_budget::charge_candidate_work(2, "remaining predicate allowance")?;
        assert!(read_budget::charge_candidate_work(1, "no extra allowance").is_err());
        Ok(())
    })
    .unwrap();
    assert!(
        read_budget::with_budget(limits(work, CHAIN_BYTES), || {
            execute_read_query_indexed(&index, &query, &CypherParameters::new())
        })
        .is_err()
    );
}

// Independent raw physical-edge enumeration: a loop has one orientation;
// every nonloop edge has two, and each oriented endpoint joins all S copies.
fn raw_undirected_count(graph: &Graph) -> i64 {
    let mut count = 0;
    for r in graph.edges.iter().filter(|edge| edge.label.as_str() == "R") {
        for reverse in [false, true] {
            if reverse && r.from == r.to {
                continue;
            }
            let end = if reverse { &r.from } else { &r.to };
            count += graph
                .edges
                .iter()
                .filter(|s| s.label.as_str() == "S" && &s.from == end)
                .count() as i64;
        }
    }
    count
}

#[test]
fn undirected_loops_parallel_and_reciprocal_edges_keep_counts_and_scan_debits() {
    for slots in BOUNDARIES {
        let nodes = vec![
            Node::new("X", "x", Props::new()),
            Node::new("X", "y", Props::new()),
        ];
        let mut edges: Vec<_> = (0..slots).map(|_| edge("R", "x", "x")).collect();
        edges.extend([
            edge("R", "y", "y"),
            edge("R", "x", "y"),
            edge("R", "x", "y"),
            edge("R", "y", "x"),
            edge("R", "y", "x"),
            edge("R", "y", "x"),
            edge("S", "x", "x"),
            edge("S", "y", "y"),
        ]);
        let index = indexed(Graph::new(nodes, edges));
        let expected = raw_undirected_count(index.graph());
        assert_eq!(expected, slots as i64 + 11);
        // R appears in both physical directions, even for the slots whose
        // incoming self-loop contribution is skipped. Other work is 44 units.
        exact(
            &index,
            "MATCH (:X)-[:R]-(:X)-[:S]->(:X) RETURN count(*)",
            expected,
            44 + 2 * (slots + 6),
            3 * 512 + 3 * 2 * 8 + 132,
        );
    }
}

#[test]
fn optional_edges_keep_per_edge_charges_and_null_padding() {
    const SLOTS: usize = 257;
    let source = "MATCH (:A)-[:R]->(b:B)-[:S]->(:C) OPTIONAL MATCH (b)-[:O]->(:C) RETURN count(*)";
    for copies in [0, 2] {
        let mut graph = chain(SLOTS, false, true);
        graph.edges.extend((0..copies).map(|_| edge("O", "b", "c")));
        let index = indexed(graph);
        exact(
            &index,
            source,
            (SLOTS * copies.max(1)) as i64,
            40 + SLOTS + 3 * copies,
            5 * 512 + 3 * 3 * 8 + 132,
        );
        let failed_anchor = query(
            "MATCH (:A)-[:R]->(b:B)-[:S]->(:C) OPTIONAL MATCH (b {missing:true})-[:O]->(:C) RETURN count(*)",
        );
        let reference =
            execute_read_query(index.graph(), &failed_anchor, &CypherParameters::new()).unwrap();
        assert_eq!(reference.rows, vec![vec![Value::Int(SLOTS as i64)]]);
        assert_eq!(
            execute_read_query_indexed(&index, &failed_anchor, &CypherParameters::new()).unwrap(),
            reference
        );
        if copies == 2 {
            // 24 units precede the first O edge. With one additional unit,
            // its own scan succeeds and its leaf predicate refuses: OPTIONAL
            // did not inherit mandatory whole-chunk prepayment.
            let query = query(source);
            let error = read_budget::with_budget(limits(25, 100_000), || {
                try_execute(&index, &query, &CypherParameters::new())
            })
            .unwrap_err();
            assert!(
                error.to_string().ends_with("checking literal count labels"),
                "{error}"
            );
        }
    }
}

#[test]
fn active_deadline_errors_propagate_and_do_not_leave_a_stale_budget() {
    let index = indexed(chain(513, false, true));
    let query = query("MATCH (:A)-[:R]->(:B)-[:S]->(:C) RETURN count(*)");
    for full_entrypoint in [false, true] {
        let error = read_budget::with_budget(limits(100_000, 100_000), || {
            read_budget::expire_deadline_for_test();
            if full_entrypoint {
                execute_read_query_indexed(&index, &query, &CypherParameters::new()).map(Some)
            } else {
                try_execute(&index, &query, &CypherParameters::new())
            }
        })
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
    }
    assert_eq!(
        execute_read_query_indexed(&index, &query, &CypherParameters::new())
            .unwrap()
            .rows,
        vec![vec![Value::Int(513)]]
    );
}

#[test]
fn scalar_alias_skip_and_limit_semantics_remain_after_complete_scanning() {
    let index = indexed(chain(257, false, true));
    for (suffix, expected_rows) in [("", 1), (" LIMIT 1", 1), (" LIMIT 0", 0), (" SKIP 1", 0)] {
        let source = format!("MATCH (:A)-[:R]->(:B)-[:S]->(:C) RETURN count(*) AS total{suffix}");
        let query = query(&source);
        let actual = read_budget::with_budget(limits(36 + 257, CHAIN_BYTES + 1), || {
            let table = try_execute(&index, &query, &CypherParameters::new())?.unwrap();
            assert!(
                read_budget::charge_candidate_work(1, "complete scan before pagination").is_err()
            );
            Ok(table)
        })
        .unwrap();
        assert_eq!(actual.columns, vec!["total"]);
        assert_eq!(actual.rows.len(), expected_rows);
        assert_eq!(
            actual,
            execute_read_query(index.graph(), &query, &CypherParameters::new()).unwrap()
        );
        assert_eq!(
            actual,
            run_read_query_indexed(&index, &source, &CypherParameters::new()).unwrap()
        );
    }
}
