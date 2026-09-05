use super::*;
use crate::{Edge, Node, Props};

fn fixture() -> Graph {
    Graph {
        nodes: vec![
            Node::new("A", "a", Props::new()),
            Node::new("B", "b", Props::new()),
            Node::new("A", "c", Props::new()),
        ],
        edges: vec![
            Edge::new("T", "a", "c", Props::new()),
            Edge::new("T", "a", "b", Props::new()),
            Edge::new("T", "a", "b", Props::new()),
            Edge::new("T", "a", "a", Props::new()),
            Edge::new("U", "c", "a", Props::new()),
        ],
    }
}

#[test]
fn sorted_typed_slices_preserve_parallel_edges_and_self_loops() {
    let index = TypedGraphIndex::new(Arc::new(fixture())).unwrap();
    assert_eq!(index.vertices_with_label("A"), &[0, 2]);
    assert_eq!(
        index.outgoing(0, "T"),
        &[
            TypedNeighbor { vertex: 0, edge: 3 },
            TypedNeighbor { vertex: 1, edge: 1 },
            TypedNeighbor { vertex: 1, edge: 2 },
            TypedNeighbor { vertex: 2, edge: 0 },
        ]
    );
    assert_eq!(index.incoming(1, "T").len(), 2);
    assert_eq!(
        index.incoming(0, "T"),
        &[TypedNeighbor { vertex: 0, edge: 3 }]
    );
    assert!(index.has_relationship(2, 0, "U"));
    assert!(!index.has_relationship(2, 0, "T"));
    assert!(index.outgoing(u32::MAX, "T").is_empty());
    assert!(index.vertices_with_label("missing").is_empty());
}

#[test]
fn snapshot_cannot_be_invalidated_by_another_arc_owner() {
    let mut graph = Arc::new(fixture());
    let index = TypedGraphIndex::new(graph.clone()).unwrap();
    Arc::make_mut(&mut graph).nodes.clear();
    assert_eq!(index.graph().nodes.len(), 3);
    assert_eq!(index.vertex_index("b"), Some(1));
}

#[test]
fn invalid_graphs_are_rejected() {
    let mut graph = fixture();
    graph.nodes.push(graph.nodes[0].clone());
    assert!(TypedGraphIndex::new(Arc::new(graph)).is_err());
    for missing_source in [false, true] {
        let mut graph = fixture();
        if missing_source {
            graph.edges[0].from = "missing".into();
        } else {
            graph.edges[0].to = "missing".into();
        }
        assert!(TypedGraphIndex::new(Arc::new(graph)).is_err());
    }
}

#[test]
fn typed_index_matches_direct_scans_on_generated_multigraphs() {
    for seed in 0..32 {
        let mut graph = fixture();
        graph.edges.clear();
        for edge in 0..50 {
            let from = (edge * 7 + seed) % 3;
            let to = (edge * 13 + seed / 3) % 3;
            let label = if (edge + seed) % 2 == 0 { "T" } else { "U" };
            graph.edges.push(Edge::new(
                label,
                graph.nodes[from].id.clone(),
                graph.nodes[to].id.clone(),
                Props::new(),
            ));
        }
        let index = TypedGraphIndex::new(Arc::new(graph)).unwrap();
        for vertex in 0..3 {
            for label in ["T", "U", "missing"] {
                for reverse in [false, true] {
                    let mut expected = Vec::new();
                    for (edge_slot, edge) in index.graph().edges.iter().enumerate() {
                        let (from, to) = if reverse {
                            (&edge.to, &edge.from)
                        } else {
                            (&edge.from, &edge.to)
                        };
                        if edge.label.as_str() == label
                            && index.vertex_index(from.as_str()) == Some(vertex)
                        {
                            expected.push(TypedNeighbor {
                                vertex: index.vertex_index(to.as_str()).unwrap(),
                                edge: edge_slot as u32,
                            });
                        }
                    }
                    expected.sort_unstable();
                    let actual = if reverse {
                        index.incoming(vertex, label)
                    } else {
                        index.outgoing(vertex, label)
                    };
                    assert_eq!(actual, expected);
                }
            }
        }
    }
}

#[test]
fn csr_buckets_preserve_empty_source_slots_and_reverse_order() {
    let edges = [(4, 2, 0), (1, 4, 1), (4, 0, 2), (1, 0, 3)];
    for reverse in [false, true] {
        let csr = Csr::build(6, &edges, reverse);
        let CsrOffsets::Dense(offsets) = &csr.offsets else {
            panic!("four edges on six vertices should use dense offsets");
        };
        assert_eq!(offsets.len(), 7);
        assert_eq!(offsets[6], edges.len() as u32);
        for source in 0..6 {
            let mut expected: Vec<_> = edges
                .iter()
                .filter_map(|&(from, to, edge)| {
                    let (from, to) = if reverse { (to, from) } else { (from, to) };
                    (from == source).then_some(TypedNeighbor { vertex: to, edge })
                })
                .collect();
            expected.sort_unstable();
            assert_eq!(csr.at(source), expected);
        }
    }
}

#[test]
fn cached_serialized_size_matches_exact_json_and_stays_on_its_snapshot() {
    let mut graph = fixture();
    graph.nodes[0]
        .props
        .insert("escaped".into(), crate::Value::String("雪\n\"\\".into()));
    graph.edges[0].props.insert(
        "nested".into(),
        crate::Value::Json(serde_json::json!({"items": [null, true, 1.25, "é"]})),
    );
    let mut graph = Arc::new(graph);
    let expected = serde_json::to_vec(graph.as_ref()).unwrap().len();
    let index = TypedGraphIndex::new(graph.clone()).unwrap();
    assert_eq!(index.serialized_graph_bytes(), expected);
    Arc::make_mut(&mut graph).edges.clear();
    assert_eq!(index.serialized_graph_bytes(), expected);
    assert_ne!(serde_json::to_vec(graph.as_ref()).unwrap().len(), expected);
    let empty = TypedGraphIndex::new(Arc::new(Graph::new(vec![], vec![]))).unwrap();
    assert_eq!(
        empty.serialized_graph_bytes(),
        serde_json::to_vec(empty.graph()).unwrap().len()
    );
}

#[test]
fn serialized_size_counter_rejects_overflow() {
    use std::io::Write;
    assert!(SerializedSize(usize::MAX).write(b"x").is_err());
}

fn assert_direct_scans(index: &TypedGraphIndex, labels: &[&str]) {
    for vertex in 0..index.graph().nodes.len() as u32 {
        for &label in labels {
            for reverse in [false, true] {
                let mut expected: Vec<_> = index
                    .graph()
                    .edges
                    .iter()
                    .enumerate()
                    .filter_map(|(slot, edge)| {
                        let (source, target) = if reverse {
                            (&edge.to, &edge.from)
                        } else {
                            (&edge.from, &edge.to)
                        };
                        (edge.label.as_str() == label
                            && index.vertex_index(source.as_str()) == Some(vertex))
                        .then(|| TypedNeighbor {
                            vertex: index.vertex_index(target.as_str()).unwrap(),
                            edge: slot as u32,
                        })
                    })
                    .collect();
                expected.sort_unstable();
                let actual = if reverse {
                    index.incoming(vertex, label)
                } else {
                    index.outgoing(vertex, label)
                };
                assert_eq!(
                    actual, expected,
                    "vertex={vertex}, type={label}, reverse={reverse}"
                );
            }
        }
    }
}

#[test]
fn hybrid_offsets_preserve_parallel_reciprocal_self_loops_and_isolated_vertices() {
    let mut graph = fixture();
    graph
        .nodes
        .extend((3..64).map(|slot| Node::new("Isolated", format!("n{slot}"), Props::new())));
    graph.edges.push(Edge::new("T", "b", "a", Props::new()));
    for edge in 0..20 {
        graph.edges.push(Edge::new(
            "Dense",
            graph.nodes[edge % 3].id.clone(),
            graph.nodes[(edge / 3) % 3].id.clone(),
            Props::new(),
        ));
    }
    let expected_bytes = serde_json::to_vec(&graph).unwrap().len();
    let index = TypedGraphIndex::new(Arc::new(graph)).unwrap();
    for (label, dense) in [("Dense", true), ("T", false), ("U", false)] {
        let adjacency = &index.adjacency[label];
        for csr in [&adjacency.forward, &adjacency.reverse] {
            assert_eq!(
                matches!(csr.offsets, CsrOffsets::Dense(_)),
                dense,
                "{label}"
            );
            assert!(csr.at(64).is_empty());
            assert!(csr.at(u32::MAX).is_empty());
        }
    }
    assert_direct_scans(&index, &["Dense", "T", "U", "missing"]);
    assert_eq!(
        index.vertices_with_label("Isolated"),
        (3..64).collect::<Vec<_>>()
    );
    assert!(index.outgoing(63, "Dense").is_empty());
    assert!(index.incoming(63, "T").is_empty());
    assert!(index.has_relationship(0, 1, "T"));
    assert!(index.has_relationship(1, 0, "T"));
    assert!(!index.has_relationship(63, 0, "T"));
    assert!(!index.has_relationship(u32::MAX, 0, "T"));
    assert!(!index.has_relationship(0, u32::MAX, "T"));
    assert_eq!(index.serialized_graph_bytes(), expected_bytes);
}

#[test]
fn sparse_offsets_only_describe_sorted_active_sources() {
    let edges = [(4, 2, 0), (1, 4, 1), (4, 0, 2), (1, 0, 3)];
    for reverse in [false, true] {
        let csr = Csr::build(24, &edges, reverse);
        let CsrOffsets::Sparse { sources, offsets } = &csr.offsets else {
            panic!("four edges on 24 vertices should use sparse offsets");
        };
        if reverse {
            assert_eq!(sources, &[0, 2, 4]);
            assert_eq!(offsets, &[0, 2, 3, 4]);
        } else {
            assert_eq!(sources, &[1, 4]);
            assert_eq!(offsets, &[0, 2, 4]);
        }
        assert_eq!(offsets.len(), sources.len() + 1);
        for source in 0..24 {
            let mut expected: Vec<_> = edges
                .iter()
                .filter_map(|&(from, to, edge)| {
                    let (from, to) = if reverse { (to, from) } else { (from, to) };
                    (from == source).then_some(TypedNeighbor { vertex: to, edge })
                })
                .collect();
            expected.sort_unstable();
            assert_eq!(csr.at(source), expected);
        }
        assert!(csr.at(u32::MAX).is_empty());
    }
}

#[test]
fn density_threshold_rounds_up_and_empty_offsets_are_safe() {
    let edges = [(0, 1, 0), (1, 0, 1)];
    for (vertices, edge_count, dense) in [(4, 1, true), (5, 1, false), (8, 2, true), (9, 2, false)]
    {
        let csr = Csr::build(vertices, &edges[..edge_count], false);
        assert_eq!(matches!(csr.offsets, CsrOffsets::Dense(_)), dense);
        assert!(csr.at(vertices as u32).is_empty());
        assert!(csr.at(u32::MAX).is_empty());
    }
    for vertices in [0, 10] {
        let csr = Csr::build(vertices, &[], false);
        assert!(csr.at(0).is_empty());
        assert!(csr.at(u32::MAX).is_empty());
    }
}

fn csr_heap_bytes(csr: &Csr) -> usize {
    let offset_capacity = match &csr.offsets {
        CsrOffsets::Dense(offsets) => offsets.capacity(),
        CsrOffsets::Sparse { sources, offsets } => sources.capacity() + offsets.capacity(),
    };
    offset_capacity * std::mem::size_of::<u32>()
        + csr.neighbors.capacity() * std::mem::size_of::<TypedNeighbor>()
}

#[test]
fn many_relationship_types_have_edge_bounded_adjacency_allocations() {
    let nodes: Vec<_> = (0..512)
        .map(|slot| Node::new("N", format!("n{slot}"), Props::new()))
        .collect();
    let mut edges = Vec::new();
    for slot in 0..128 {
        edges.push(Edge::new(
            format!("Sparse{slot}"),
            nodes[slot].id.clone(),
            nodes[(slot * 17) % 512].id.clone(),
            Props::new(),
        ));
        edges.push(Edge::new(
            "Dense",
            nodes[slot].id.clone(),
            nodes[(slot + 1) % 512].id.clone(),
            Props::new(),
        ));
    }
    let edge_count = edges.len();
    let index = TypedGraphIndex::new(Arc::new(Graph::new(nodes, edges))).unwrap();
    let mut heap_bytes = 0;
    let mut entries = 0;
    for (label, adjacency) in &index.adjacency {
        for csr in [&adjacency.forward, &adjacency.reverse] {
            assert_eq!(
                matches!(csr.offsets, CsrOffsets::Dense(_)),
                label.as_str() == "Dense"
            );
            heap_bytes += csr_heap_bytes(csr);
            entries += csr.neighbors.len();
            assert!(csr.at(u32::MAX).is_empty());
        }
    }
    assert_eq!(entries, edge_count * 2);
    // Includes actual Vec capacities, not just populated entries: reserving V
    // slots for every sparse type would violate this modest-fixture bound.
    assert!(
        heap_bytes <= 128 * edge_count,
        "{heap_bytes} adjacency bytes for {edge_count} edges"
    );
    assert_eq!(
        index.serialized_graph_bytes(),
        serde_json::to_vec(index.graph()).unwrap().len()
    );
}

#[test]
fn hybrid_generated_graphs_match_scans_in_both_directions() {
    for vertices in [1, 3, 17, 64] {
        for seed in 0..8 {
            let nodes: Vec<_> = (0..vertices)
                .map(|slot| Node::new("N", format!("n{slot}"), Props::new()))
                .collect();
            let edges = (0..vertices * 2 + 5)
                .map(|edge| {
                    Edge::new(
                        if edge % 13 == 0 { "Sparse" } else { "Dense" },
                        nodes[(edge * 7 + seed) % vertices].id.clone(),
                        nodes[(edge * 11 + seed / 2) % vertices].id.clone(),
                        Props::new(),
                    )
                })
                .collect();
            let index = TypedGraphIndex::new(Arc::new(Graph::new(nodes, edges))).unwrap();
            assert_direct_scans(&index, &["Dense", "Sparse", "missing"]);
        }
    }
}
