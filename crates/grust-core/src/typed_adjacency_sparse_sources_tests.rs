use super::*;
use crate::{Edge, Node, Props};

const VERTICES: usize = 32;

fn nodes(count: usize) -> Vec<Node> {
    (0..count)
        .map(|slot| Node::new("N", format!("n{slot}"), Props::new()))
        .collect()
}

fn relationship(kind: &str, from: usize, to: usize) -> Edge {
    Edge::new(kind, format!("n{from}"), format!("n{to}"), Props::new())
}

fn indexed_fixture() -> TypedGraphIndex {
    let mut edges = vec![
        relationship("Sparse", 5, 2),
        relationship("Sparse", 0, 1),
        relationship("Sparse", 0, 1),
        relationship("Sparse", 2, 0),
        relationship("Sparse", 0, 0),
        relationship("Sparse", 5, 0),
        relationship("Sparse", 2, 5),
    ];
    // ceil(32 / 4) == 8, so this relationship uses dense offsets in both
    // directions while retaining many empty source rows.
    edges.extend((0..8).map(|slot| relationship("Dense", slot % 3, slot)));
    TypedGraphIndex::new(Arc::new(Graph::new(nodes(VERTICES), edges))).unwrap()
}

#[test]
fn sparse_source_slices_are_sorted_unique_and_exact_in_both_directions() {
    let index = indexed_fixture();
    let view = index.adjacency("Sparse");
    let outgoing = view.sparse_outgoing_sources().unwrap();
    let incoming = view.sparse_incoming_sources().unwrap();
    assert_eq!(outgoing, &[0, 2, 5]);
    assert_eq!(incoming, &[0, 1, 2, 5]);

    for (sources, reverse) in [(outgoing, false), (incoming, true)] {
        assert!(sources.windows(2).all(|pair| pair[0] < pair[1]));
        for vertex in 0..VERTICES as u32 {
            let row = if reverse {
                view.incoming(vertex)
            } else {
                view.outgoing(vertex)
            };
            assert_eq!(
                !row.is_empty(),
                sources.binary_search(&vertex).is_ok(),
                "vertex={vertex}, reverse={reverse}"
            );
        }
    }

    let typed = index.adjacency.get("Sparse").unwrap();
    let CsrOffsets::Sparse {
        sources: forward, ..
    } = &typed.forward.offsets
    else {
        panic!("fixture must use sparse forward offsets");
    };
    let CsrOffsets::Sparse {
        sources: reverse, ..
    } = &typed.reverse.offsets
    else {
        panic!("fixture must use sparse reverse offsets");
    };
    assert!(std::ptr::eq(outgoing, forward.as_slice()));
    assert!(std::ptr::eq(incoming, reverse.as_slice()));
}

#[test]
fn source_enumeration_does_not_collapse_physical_multigraph_rows() {
    let index = indexed_fixture();
    let view = index.adjacency("Sparse");
    let outgoing = view.sparse_outgoing_sources().unwrap();
    let incoming = view.sparse_incoming_sources().unwrap();

    assert_eq!(view.outgoing(0).len(), 3); // loop plus two parallel edges
    assert_eq!(
        view.outgoing(0)
            .iter()
            .filter(|neighbor| neighbor.vertex == 1)
            .count(),
        2
    );
    assert_eq!(view.incoming(1).len(), 2);
    assert_eq!(view.outgoing(2).len(), 2);
    assert_eq!(view.incoming(2).len(), 1);
    assert_eq!(
        outgoing
            .iter()
            .map(|&source| view.outgoing(source).len())
            .sum::<usize>(),
        7
    );
    assert_eq!(
        incoming
            .iter()
            .map(|&source| view.incoming(source).len())
            .sum::<usize>(),
        7
    );
}

#[test]
fn dense_csr_reports_none_even_when_some_rows_are_empty() {
    let index = indexed_fixture();
    let view = index.adjacency("Dense");
    assert!(view.sparse_outgoing_sources().is_none());
    assert!(view.sparse_incoming_sources().is_none());
    assert!(view.outgoing(31).is_empty());
    assert!(view.incoming(31).is_empty());

    let typed = index.adjacency.get("Dense").unwrap();
    assert!(matches!(&typed.forward.offsets, CsrOffsets::Dense(_)));
    assert!(matches!(&typed.reverse.offsets, CsrOffsets::Dense(_)));
}

#[test]
fn absent_relationships_and_empty_indexes_report_some_empty() {
    for index in [
        TypedGraphIndex::new(Arc::new(Graph::default())).unwrap(),
        TypedGraphIndex::new(Arc::new(Graph::new(nodes(4), Vec::new()))).unwrap(),
        indexed_fixture(),
    ] {
        for kind in ["", "absent", "unknown:雪/punctuation"] {
            let view = index.adjacency(kind);
            let empty: &[u32] = &[];
            assert_eq!(view.sparse_outgoing_sources(), Some(empty));
            assert_eq!(view.sparse_incoming_sources(), Some(empty));
        }
    }
}

#[test]
fn source_slices_borrow_the_index_not_the_view_or_relationship_name() {
    fn source_slices<'index>(
        index: &'index TypedGraphIndex,
        kind: &str,
    ) -> (&'index [u32], &'index [u32]) {
        let temporary_view = index.adjacency(kind);
        (
            temporary_view.sparse_outgoing_sources().unwrap(),
            temporary_view.sparse_incoming_sources().unwrap(),
        )
    }

    let index = indexed_fixture();
    let (outgoing, incoming) = {
        let temporary_kind = String::from("Sparse");
        source_slices(&index, &temporary_kind)
    };
    assert_eq!(outgoing, &[0, 2, 5]);
    assert_eq!(incoming, &[0, 1, 2, 5]);
    assert!(std::ptr::eq(
        outgoing,
        index.adjacency("Sparse").sparse_outgoing_sources().unwrap()
    ));
}
