//! Sparse seeds are necessary vertex sets, never precomputed match rows.

use super::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn node(label: &str, slot: usize, keep: bool) -> Node {
    let mut node = Node::new(label, format!("n{slot}"), Props::new());
    node.props.insert("keep".into(), Value::Bool(keep));
    node.props.insert("leaf".into(), Value::Bool(slot != 2));
    node.props.insert("anchor".into(), Value::Bool(false));
    node
}

fn edge(kind: &str, from: usize, to: usize, ok: bool) -> Edge {
    let mut edge = Edge::new(kind, format!("n{from}"), format!("n{to}"), Props::new());
    edge.props.insert("ok".into(), Value::Bool(ok));
    edge
}

fn indexed(nodes: Vec<Node>, edges: Vec<Edge>) -> TypedGraphIndex {
    TypedGraphIndex::new(Arc::new(Graph::new(nodes, edges))).unwrap()
}

fn query(source: &str) -> Query {
    let query = parse_query(source).unwrap();
    crate::semantics::analyze(&query).unwrap();
    query
}

fn prepared<'index>(index: &'index TypedGraphIndex, source: &str) -> Prepared<'index> {
    let query = query(source);
    let proven = plan(&query).unwrap().unwrap();
    let slot = proven
        .forest
        .nodes
        .iter()
        .position(|mentions| {
            mentions
                .iter()
                .any(|node| node.variable.as_deref() == Some("b"))
        })
        .unwrap();
    prepare(index, &proven.forest, slot).unwrap().unwrap()
}

fn compare(index: &TypedGraphIndex, source: &str, expected: i64) {
    let query = query(source);
    let params = CypherParameters::new();
    let actual = try_execute(index, &query, &params).unwrap().unwrap();
    assert_eq!(actual.rows, vec![vec![Value::Int(expected)]], "{source}");
    let reference = execute_read_query(index.graph(), &query, &params).unwrap();
    assert_eq!(actual, reference, "{source}");
    assert_eq!(
        execute_read_query_indexed(index, &query, &params).unwrap(),
        reference
    );
}

fn selection_fixture() -> TypedGraphIndex {
    let nodes = (0..64)
        .map(|slot| {
            let label = match slot {
                0..=7 => "B",
                8 => "One",
                9..=10 => "Tie",
                _ => "Other",
            };
            node(label, slot, true)
        })
        .collect();
    let edges = [
        ("R", 0),
        ("R", 2),
        ("R", 4),
        ("S", 0),
        ("S", 1),
        ("T", 0),
        ("T", 3),
    ]
    .into_iter()
    .map(|(kind, from)| edge(kind, from, 12, true))
    .collect();
    indexed(nodes, edges)
}

#[test]
fn strictly_shorter_sets_win_and_ties_keep_the_existing_borrow() {
    let index = selection_fixture();
    let source = "MATCH (b {keep:true})-[:R]->(), (b)-[:S]->(), (b)-[:T]->() RETURN count(*)";
    let prepared = prepared(&index, source);
    let selected = prepared
        .narrow_candidates(Some(index.vertices_with_label("B")), 64)
        .unwrap()
        .unwrap();
    assert_eq!(selected, &[0, 1]);
    assert!(std::ptr::eq(
        selected,
        index.adjacency("S").sparse_outgoing_sources().unwrap()
    ));
    assert!(std::ptr::eq(
        selected,
        prepared.narrow_candidates(None, 64).unwrap().unwrap()
    ));
    for label in ["One", "Tie", "Missing"] {
        let original = index.vertices_with_label(label);
        let selected = prepared
            .narrow_candidates(Some(original), 64)
            .unwrap()
            .unwrap();
        assert!(std::ptr::eq(selected, original), "{label}");
    }
    // S and T have equal cardinality but different slots: the earlier S wins.
    assert_ne!(
        selected,
        index.adjacency("T").sparse_outgoing_sources().unwrap()
    );
}

#[test]
fn dense_and_undirected_atoms_are_not_seeds_but_directed_absence_is_empty() {
    let index = selection_fixture();
    let candidates = index.vertices_with_label("B");
    let undirected = prepared(
        &index,
        "MATCH (b {keep:true})-[:R]-(), (b)-[:S]-() RETURN count(*)",
    );
    assert!(std::ptr::eq(
        undirected
            .narrow_candidates(Some(candidates), 64)
            .unwrap()
            .unwrap(),
        candidates
    ));
    assert!(undirected.narrow_candidates(None, 64).unwrap().is_none());
    let mixed = prepared(
        &index,
        "MATCH (b {keep:true})-[:R]-(), (b)-[:S]->() RETURN count(*)",
    );
    assert_eq!(
        mixed.narrow_candidates(Some(candidates), 64).unwrap(),
        Some(&[0, 1][..])
    );
    for arm in ["-[:Absent]->", "<-[:Absent]-"] {
        let source = format!("MATCH (b {{keep:true}}){arm}(), (b)-[:R]->() RETURN count(*)");
        assert!(
            prepared(&index, &source)
                .narrow_candidates(Some(candidates), 64)
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }
    let mut edges = Vec::new();
    for kind in ["R", "S"] {
        edges.extend((0..16).map(|_| edge(kind, 0, 1, true)));
    }
    let dense = indexed(index.graph().nodes.clone(), edges);
    assert!(dense.adjacency("R").sparse_outgoing_sources().is_none());
    let prepared = prepared(
        &dense,
        "MATCH (b {keep:true})-[:R]->(), (b)-[:S]->() RETURN count(*)",
    );
    let candidates = dense.vertices_with_label("B");
    assert!(std::ptr::eq(
        prepared
            .narrow_candidates(Some(candidates), 64)
            .unwrap()
            .unwrap(),
        candidates
    ));
    assert!(prepared.narrow_candidates(None, 64).unwrap().is_none());
}

#[test]
fn role_relative_direction_at_either_pattern_end_selects_the_correct_sources() {
    for loop_edge in [false, true] {
        let target = usize::from(!loop_edge);
        let index = indexed(
            (0..64).map(|slot| node("N", slot, slot < 3)).collect(),
            vec![
                edge("R", 0, target, true),
                edge("R", 0, target, true),
                edge("S", 0, 0, true),
                edge("S", 1, 1, true),
                edge("S", 2, 2, true),
            ],
        );
        for (first, expected_slot) in [
            ("(b {keep:true})-[:R]->()", 0),
            ("()<-[:R]-(b {keep:true})", 0),
            ("(b {keep:true})<-[:R]-()", target),
            ("()-[:R]->(b {keep:true})", target),
        ] {
            let source = format!("MATCH {first}, (b)-[:S]->() RETURN count(*)");
            let selected = prepared(&index, &source)
                .narrow_candidates(None, 64)
                .unwrap()
                .unwrap();
            assert_eq!(selected, &[expected_slot as u32], "{source}");
            compare(&index, &source, 2);
        }
    }
}

#[test]
fn a_seed_does_not_replace_other_mandatory_checks() {
    let index = indexed(
        (0..64).map(|slot| node("N", slot, true)).collect(),
        vec![
            edge("R", 0, 1, true),
            edge("S", 1, 2, true),
            edge("S", 2, 1, true),
        ],
    );
    let source = "MATCH (b {keep:true})-[:R]->(), (b)-[:S]->() RETURN count(*)";
    let prepared = prepared(&index, source);
    assert_eq!(
        prepared.narrow_candidates(None, 64).unwrap(),
        Some(&[0][..])
    );
    assert!(!prepared.accepts(0).unwrap());
    compare(&index, source, 0);
}

#[test]
fn shorter_sources_preserve_later_mentions_edge_filters_and_optional_padding() {
    let index = indexed(
        (0..64)
            .map(|slot| node(if slot == 2 { "Other" } else { "N" }, slot, slot != 1))
            .collect(),
        vec![
            edge("R", 0, 1, true),
            edge("R", 0, 1, true),
            edge("R", 0, 1, false),
            edge("R", 2, 1, true),
            edge("S", 0, 1, true),
            edge("S", 2, 1, true),
            edge("O", 2, 1, true),
            edge("O", 2, 1, true),
        ],
    );
    let base = "MATCH (b)-[:R {ok:true}]->(:N {leaf:true}) MATCH (b:N {keep:true})-[:S]->(:N)";
    for suffix in [
        "",
        " OPTIONAL MATCH (b)-[:O]->()",
        " OPTIONAL MATCH (b:Missing {anchor:true})-[:O]->()",
    ] {
        compare(&index, &format!("{base}{suffix} RETURN count(*)"), 2);
    }
    for source in [
        "MATCH (b:N {keep:true})-[:R]->(), (b {keep:false})-[:S]->() RETURN count(*)",
        "MATCH (b:N {keep:true,keep:false})-[:R]->(), (b)-[:S]->() RETURN count(*)",
        "MATCH (b:N {keep:true})-[:R]->(), (b:Other)-[:S]->() RETURN count(*)",
        "MATCH (b:N {keep:true})-[:R {ok:true}]->(:Other), (b)-[:S]->() RETURN count(*)",
        "MATCH (b:N {keep:true})-[:R {ok:true}]->(), (b)-[:S {ok:false}]->() RETURN count(*)",
    ] {
        compare(&index, source, 0);
    }
}

#[derive(Clone, Copy)]
enum Flow {
    Out,
    In,
    Either,
}

fn arm(kind: &str, flow: Flow) -> String {
    match flow {
        Flow::Out => format!("-[:{kind} {{ok:true}}]->"),
        Flow::In => format!("<-[:{kind} {{ok:true}}]-"),
        Flow::Either => format!("-[:{kind} {{ok:true}}]-"),
    }
}

fn raw_degree(graph: &Graph, center: &Node, kind: &str, flow: Flow) -> i64 {
    graph
        .edges
        .iter()
        .filter(|edge| {
            if edge.label.as_str() != kind || edge.props.get("ok") != Some(&Value::Bool(true)) {
                return false;
            }
            let target = match flow {
                Flow::Out | Flow::Either if edge.from == center.id => &edge.to,
                Flow::In | Flow::Either if edge.to == center.id => &edge.from,
                _ => return false,
            };
            graph.nodes.iter().any(|node| {
                &node.id == target
                    && node.label.as_str() == "N"
                    && node.props.get("leaf") == Some(&Value::Bool(true))
            })
        })
        .count() as i64
}

#[test]
fn generated_sparse_multigraphs_match_raw_weighted_counts_and_reference() {
    for seed in 0..12u64 {
        let mut state = seed + 1;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            state >> 32
        };
        let mut nodes: Vec<_> = (0..128)
            .map(|slot| {
                node(
                    if slot == 3 { "Other" } else { "N" },
                    slot,
                    slot == 0 || slot == 2 || slot == 3,
                )
            })
            .collect();
        nodes[0]
            .props
            .insert("anchor".into(), Value::Bool(seed % 2 == 0));
        let mut edges = Vec::new();
        for kind in ["R", "S", "O"] {
            edges.extend([
                edge(kind, 0, 0, true),
                edge(kind, 0, 1, true),
                edge(kind, 0, 1, true),
                edge(kind, 1, 0, true),
            ]);
            for _ in 0..4 {
                edges.push(edge(
                    kind,
                    (next() % 4) as usize,
                    (next() % 4) as usize,
                    next() % 3 != 0,
                ));
            }
        }
        let index = indexed(nodes, edges);
        for r in [Flow::Out, Flow::In] {
            for s in [Flow::Out, Flow::In, Flow::Either] {
                let source = format!(
                    "MATCH (b:N {{keep:true}}){}(:N {{leaf:true}}), (b){}(:N {{leaf:true}}) OPTIONAL MATCH (b {{anchor:true}})-[:O {{ok:true}}]->(:N {{leaf:true}}) RETURN count(*)",
                    arm("R", r),
                    arm("S", s)
                );
                let expected = index
                    .graph()
                    .nodes
                    .iter()
                    .filter(|node| {
                        node.label.as_str() == "N"
                            && node.props.get("keep") == Some(&Value::Bool(true))
                    })
                    .map(|node| {
                        let optional = if node.props.get("anchor") == Some(&Value::Bool(true)) {
                            raw_degree(index.graph(), node, "O", Flow::Out).max(1)
                        } else {
                            1
                        };
                        raw_degree(index.graph(), node, "R", r)
                            * raw_degree(index.graph(), node, "S", s)
                            * optional
                    })
                    .sum();
                let seed = prepared(&index, &source)
                    .narrow_candidates(Some(index.vertices_with_label("N")), 128)
                    .unwrap()
                    .unwrap();
                assert!(seed.len() <= 4);
                compare(&index, &source, expected);
            }
        }
    }
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
fn selection_charges_every_atom_even_after_empty_and_allocates_no_storage() {
    let index = selection_fixture();
    let source = "MATCH (b {keep:true})-[:Absent]->(), (b)-[:R]-(), (b)-[:S]->() RETURN count(*)";
    let prepared = prepared(&index, source);
    read_budget::with_budget(limits(3, 0), || {
        assert!(prepared.narrow_candidates(None, 64)?.unwrap().is_empty());
        let error =
            read_budget::charge_candidate_work(1, "checking exact selection work").unwrap_err();
        assert!(error.to_string().ends_with("checking exact selection work"));
        Ok(())
    })
    .unwrap();
    for work in 0..3 {
        let error =
            read_budget::with_budget(limits(work, 0), || prepared.narrow_candidates(None, 64))
                .unwrap_err();
        assert!(
            error
                .to_string()
                .ends_with("selecting mandatory count candidates")
        );
    }
    let error = read_budget::with_budget(limits(3, 0), || {
        read_budget::expire_deadline_for_test();
        prepared.narrow_candidates(None, 64)
    })
    .unwrap_err();
    assert!(error.to_string().contains("timed out"));
}

#[test]
fn long_type_resolution_is_not_recharged_during_candidate_selection() {
    let kind = "LongType".repeat(2048);
    let index = indexed(
        (0..64).map(|slot| node("N", slot, true)).collect(),
        vec![edge(&kind, 0, 1, true), edge("S", 0, 1, true)],
    );
    let source = format!("MATCH (b {{keep:true}})-[:{kind}]->(), (b)-[:S]->() RETURN count(*)");
    let prepared = prepared(&index, &source);
    read_budget::with_budget(limits(2, 0), || {
        assert_eq!(prepared.narrow_candidates(None, 64)?, Some(&[0][..]));
        Ok(())
    })
    .unwrap();
    // Selection does not waive either mandatory row probe for the retained vertex.
    let error = read_budget::with_budget(limits(1, 0), || prepared.accepts(0)).unwrap_err();
    assert!(
        error
            .to_string()
            .ends_with("probing mandatory count adjacency")
    );
}

#[test]
fn initial_empty_labels_keep_the_exact_allocation_and_work_bypass() {
    let vertices = 64;
    let index = indexed(
        (0..vertices)
            .map(|slot| node("Other", slot, true))
            .collect(),
        Vec::new(),
    );
    let query = query("MATCH (b:Missing {keep:true})-[:R]->(:A), (b)-[:S]->(:C) RETURN count(*)");
    let work = 4 * vertices + 4 + (2 * "Missing".len() + 1) + 3 + 3;
    let bytes = 4 * 512 + 3 * 8 * vertices + 132;
    read_budget::with_budget(limits(work, bytes), || {
        let table = try_execute(&index, &query, &CypherParameters::new())?.unwrap();
        assert_eq!(table.rows, vec![vec![Value::Int(0)]]);
        assert!(read_budget::charge_candidate_work(1, "exact empty work").is_err());
        assert!(read_budget::charge_intermediate_bytes(1, "exact empty bytes").is_err());
        Ok(())
    })
    .unwrap();
}

#[test]
fn nonempty_seed_keeps_candidate_branch_and_full_vertex_accounting() {
    let query = query("MATCH (b:B {keep:true})-[:R]->(:A), (b)-[:S]->(:C) RETURN count(*)");
    let mut successful_work = Vec::new();
    for vertices in [32, 64] {
        let index = indexed(
            (0..vertices)
                .map(|slot| {
                    node(
                        match slot {
                            1 => "A",
                            2 => "C",
                            _ => "B",
                        },
                        slot,
                        slot == 0,
                    )
                })
                .collect(),
            vec![edge("R", 0, 1, true), edge("S", 0, 2, true)],
        );
        let bytes = 4 * 512 + 3 * 8 * vertices + 2 * std::mem::size_of::<Required<'_>>() + 132;
        let contexts = [
            "initializing count weights",
            "filtering count vertices",
            "combining count branches",
            "selecting mandatory count candidates",
            "probing mandatory count adjacency",
            "summing count roots",
        ];
        let mut seen = [false; 6];
        for work in 0..2048 {
            match read_budget::with_budget(limits(work, bytes), || {
                try_execute(&index, &query, &CypherParameters::new())
            }) {
                Err(error) => {
                    for (seen, context) in seen.iter_mut().zip(contexts) {
                        *seen |= error.to_string().ends_with(context);
                    }
                }
                Ok(Some(table)) => {
                    assert_eq!(table.rows, vec![vec![Value::Int(1)]]);
                    successful_work.push(work);
                    break;
                }
                Ok(None) => panic!("expected proven forest"),
            }
        }
        assert!(seen.into_iter().all(|seen| seen));
        let error = read_budget::with_budget(limits(2048, 4 * 512 + 8 * vertices - 1), || {
            try_execute(&index, &query, &CypherParameters::new())
        })
        .unwrap_err();
        assert!(error.to_string().ends_with("allocating count weights"));
        assert!(
            read_budget::with_budget(limits(2048, bytes - 1), || {
                try_execute(&index, &query, &CypherParameters::new())
            })
            .unwrap_err()
            .to_string()
            .ends_with("shaping scalar count result")
        );
    }
    assert_eq!(successful_work.len(), 2);
    // Extra isolated B vertices pay three full arrays and one root sum, but
    // no additional filtering/branch visits after the one-vertex sparse seed.
    assert_eq!(successful_work[1] - successful_work[0], 4 * (64 - 32));
}
