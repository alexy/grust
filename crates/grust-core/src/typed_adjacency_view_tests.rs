use super::*;
use crate::{Edge, Node, Props};

fn fixture(vertices: usize) -> Graph {
    Graph::new(
        (0..vertices)
            .map(|vertex| Node::new("N", format!("n{vertex}"), Props::new()))
            .collect(),
        [
            ("T", "n0", "n2"),
            ("T", "n0", "n1"),
            ("T", "n1", "n0"),
            ("T", "n0", "n1"),
            ("T", "n0", "n0"),
            ("U", "n3", "n0"),
            ("T", "n2", "n2"),
            ("T", "n2", "n0"),
        ]
        .into_iter()
        .map(|(kind, from, to)| Edge::new(kind, from, to, Props::new()))
        .collect(),
    )
}

fn raw_neighbors(graph: &Graph, vertex: u32, kind: &str, reverse: bool) -> Vec<TypedNeighbor> {
    let Some(node) = graph.nodes.get(vertex as usize) else {
        return Vec::new();
    };
    let mut neighbors = Vec::new();
    for (edge, relationship) in graph.edges.iter().enumerate() {
        let (source, target) = if reverse {
            (&relationship.to, &relationship.from)
        } else {
            (&relationship.from, &relationship.to)
        };
        if relationship.label.as_str() == kind && source == &node.id {
            neighbors.push(TypedNeighbor {
                vertex: graph
                    .nodes
                    .iter()
                    .position(|node| &node.id == target)
                    .unwrap() as u32,
                edge: edge as u32,
            });
        }
    }
    neighbors.sort_unstable();
    neighbors
}

fn assert_parity(index: &TypedGraphIndex) {
    for kind in ["T", "U", "absent"] {
        let view = index.adjacency(kind);
        let invalid = [index.graph().nodes.len() as u32, u32::MAX];
        for vertex in (0..index.graph().nodes.len() as u32).chain(invalid) {
            for reverse in [false, true] {
                let (actual, original) = if reverse {
                    (view.incoming(vertex), index.incoming(vertex, kind))
                } else {
                    (view.outgoing(vertex), index.outgoing(vertex, kind))
                };
                assert_eq!(actual, raw_neighbors(index.graph(), vertex, kind, reverse));
                assert_eq!(actual, original);
                assert!(
                    std::ptr::eq(actual, original),
                    "view must borrow the same row"
                );
            }
        }
    }
}

#[test]
fn dense_and_sparse_views_match_direct_scans_and_existing_slices() {
    for (vertices, dense) in [(8, true), (64, false)] {
        let index = TypedGraphIndex::new(Arc::new(fixture(vertices))).unwrap();
        let typed = index.adjacency.get("T").unwrap();
        for csr in [&typed.forward, &typed.reverse] {
            assert_eq!(matches!(&csr.offsets, CsrOffsets::Dense(_)), dense);
        }
        assert_parity(&index);
        let view = index.adjacency("T");
        assert!(view.outgoing(vertices as u32 - 1).is_empty());
        assert!(view.incoming(vertices as u32 - 1).is_empty());
    }
}

#[test]
fn sorted_rows_keep_loops_parallel_and_reciprocal_physical_edge_identities() {
    let index = TypedGraphIndex::new(Arc::new(fixture(8))).unwrap();
    let view = index.adjacency("T");
    assert_eq!(
        view.outgoing(0),
        &[
            TypedNeighbor { vertex: 0, edge: 4 },
            TypedNeighbor { vertex: 1, edge: 1 },
            TypedNeighbor { vertex: 1, edge: 3 },
            TypedNeighbor { vertex: 2, edge: 0 },
        ]
    );
    assert_eq!(
        view.incoming(0),
        &[
            TypedNeighbor { vertex: 0, edge: 4 },
            TypedNeighbor { vertex: 1, edge: 2 },
            TypedNeighbor { vertex: 2, edge: 7 },
        ]
    );
    assert_eq!(
        view.outgoing(2),
        &[
            TypedNeighbor { vertex: 0, edge: 7 },
            TypedNeighbor { vertex: 2, edge: 6 },
        ]
    );
    // The same self-loop slot is present in each direction, not deduplicated
    // across the two independently borrowed rows.
    assert_eq!(view.outgoing(0)[0], view.incoming(0)[0]);
}

#[test]
fn empty_and_missing_types_accept_every_invalid_slot_as_an_empty_row() {
    for graph in [Graph::default(), Graph::new(fixture(8).nodes, Vec::new())] {
        let index = TypedGraphIndex::new(Arc::new(graph)).unwrap();
        for kind in ["", "T", "unknown:雪/punctuation"] {
            let view = index.adjacency(kind);
            for vertex in [0, index.graph().nodes.len() as u32, u32::MAX] {
                assert!(view.outgoing(vertex).is_empty());
                assert!(view.incoming(vertex).is_empty());
            }
        }
        assert_parity(&index);
    }
}

#[test]
fn views_do_not_borrow_type_strings_and_slices_outlive_temporary_views() {
    fn row(index: &TypedGraphIndex) -> &[TypedNeighbor] {
        let temporary_view = index.adjacency("T");
        temporary_view.outgoing(0)
    }

    let index = TypedGraphIndex::new(Arc::new(fixture(8))).unwrap();
    let view = {
        let relationship = String::from("T");
        index.adjacency(&relationship)
    };
    let copied = view;
    assert!(std::ptr::eq(row(&index), view.outgoing(0)));
    assert!(std::ptr::eq(copied.outgoing(0), view.outgoing(0)));
    assert!(std::ptr::eq(index.adjacency("T").outgoing(0), row(&index)));
}

#[test]
fn another_snapshot_owner_cannot_invalidate_a_borrowed_view() {
    let mut snapshot = Arc::new(fixture(8));
    let first_owner = Arc::new(TypedGraphIndex::new(snapshot.clone()).unwrap());
    let owner = first_owner.clone();
    let snapshot_owners = Arc::strong_count(&snapshot);
    let index_owners = Arc::strong_count(&owner);
    let view = owner.adjacency("T");
    let copied = view;
    assert_eq!(Arc::strong_count(&snapshot), snapshot_owners);
    assert_eq!(Arc::strong_count(&owner), index_owners);
    drop(first_owner);

    Arc::make_mut(&mut snapshot).edges.clear();
    let changed = TypedGraphIndex::new(snapshot.clone()).unwrap();
    assert!(changed.adjacency("T").outgoing(0).is_empty());
    assert_eq!(view.outgoing(0).len(), 4);
    assert_eq!(copied.incoming(0).len(), 3);
    assert_eq!(owner.graph().edges.len(), 8);
    drop(snapshot);
    assert_parity(&owner);
    assert_eq!(view.outgoing(0).len(), 4);
}

#[test]
fn an_absent_type_view_stays_on_its_snapshot_after_copy_on_write() {
    let mut snapshot = Arc::new(Graph::new(fixture(8).nodes, Vec::new()));
    let index = TypedGraphIndex::new(snapshot.clone()).unwrap();
    let missing = index.adjacency("T");
    Arc::make_mut(&mut snapshot)
        .edges
        .push(Edge::new("T", "n0", "n1", Props::new()));
    let newer = TypedGraphIndex::new(snapshot).unwrap();
    assert!(missing.outgoing(0).is_empty());
    assert!(missing.incoming(1).is_empty());
    assert_eq!(
        newer.adjacency("T").outgoing(0),
        &[TypedNeighbor { vertex: 1, edge: 0 }]
    );
}

#[test]
fn view_is_exported_at_the_crate_root_and_in_the_prelude() {
    fn root_view(index: &TypedGraphIndex) -> crate::TypedAdjacencyView<'_> {
        index.adjacency("T")
    }
    fn prelude_view(index: &TypedGraphIndex) -> crate::prelude::TypedAdjacencyView<'_> {
        index.adjacency("T")
    }
    fn traits<T: Copy + Clone + std::fmt::Debug>() {}
    traits::<crate::TypedAdjacencyView<'_>>();
    let index = TypedGraphIndex::new(Arc::new(fixture(8))).unwrap();
    assert!(std::ptr::eq(
        root_view(&index).outgoing(0),
        prelude_view(&index).outgoing(0)
    ));
}

#[test]
fn generated_multigraph_views_match_raw_edges_for_every_vertex_and_direction() {
    for seed in 0..12usize {
        let vertices = if seed % 2 == 0 { 8 } else { 64 };
        let mut graph = fixture(vertices);
        graph.edges.clear();
        for ordinal in 0..32 {
            let from = (ordinal * 7 + seed) % vertices;
            let to = (ordinal * 13 + seed / 3) % vertices;
            let kind = if ordinal % 5 == 0 { "T" } else { "U" };
            graph.edges.push(Edge::new(
                kind,
                format!("n{from}"),
                format!("n{to}"),
                Props::new(),
            ));
        }
        let index = TypedGraphIndex::new(Arc::new(graph)).unwrap();
        assert_parity(&index);
    }
}
