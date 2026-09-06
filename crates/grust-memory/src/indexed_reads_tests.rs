use futures_executor::block_on;
use grust_core::{Direction, EdgeQuery, Graph, GraphBuilder, Start, Step, Traversal, prelude::*};

use crate::MemoryGraphStore;

/// A small multigraph with a self-loop, reciprocal edges, two relationship
/// types between the same pair, a label the traversal can filter on, and a
/// vertex with no edges at all.
fn fixture() -> Graph {
    let mut builder = GraphBuilder::new();
    let a = builder.node("Person", "a").finish();
    let b = builder.node("Person", "b").finish();
    let c = builder.node("City", "c").finish();
    let _d = builder.node("City", "d").finish();
    let _ = builder.edge("KNOWS", &a, &b).finish();
    let _ = builder.edge("KNOWS", &b, &a).finish();
    let _ = builder.edge("LIVES_IN", &a, &c).finish();
    let _ = builder.edge("LIVES_IN", &b, &c).finish();
    let _ = builder.edge("KNOWS", &a, &a).finish();
    let _ = builder.edge("NEAR", &c, &c).finish();
    builder.build()
}

/// The same contents loaded two ways: one bulk load, which builds the
/// snapshot, and one edge at a time, which never does.
fn indexed_and_plain() -> (MemoryGraphStore, MemoryGraphStore) {
    let graph = fixture();
    let indexed = MemoryGraphStore::new();
    block_on(indexed.put_graph(&graph)).unwrap();
    assert!(
        indexed.cached_index().is_some(),
        "a load into an empty store builds the snapshot"
    );

    let plain = MemoryGraphStore::new();
    for node in &graph.nodes {
        block_on(plain.put_node(node)).unwrap();
    }
    for edge in &graph.edges {
        block_on(plain.put_edge(edge)).unwrap();
    }
    assert!(
        plain.cached_index().is_none(),
        "point writes never build a snapshot"
    );
    (indexed, plain)
}

fn by_label(label: &str) -> Traversal {
    Traversal {
        start: Start::NodesByLabel(label.into()),
        steps: Vec::new(),
        limit: None,
    }
}

fn traversals() -> Vec<Traversal> {
    let both = |edge: Option<&str>| Step {
        direction: Direction::Both,
        edge: edge.map(Label::from),
        node: None,
    };
    let mut cases = vec![
        Traversal::from_node("a").out("KNOWS"),
        Traversal::from_node("a").in_("KNOWS"),
        Traversal::from_node("a").out("LIVES_IN").to("City"),
        Traversal::from_node("a").out("KNOWS").out("LIVES_IN"),
        Traversal::from_node("a").out("KNOWS").limit(1),
        Traversal::from_node("missing").out("KNOWS"),
        Traversal::from_node("d").out("KNOWS"),
        by_label("Person").out("LIVES_IN"),
        by_label("City").in_("LIVES_IN").to("Person"),
    ];
    for start in ["a", "c"] {
        let mut named = Traversal::from_node(start);
        named.steps.push(both(Some("KNOWS")));
        cases.push(named);
        let mut any = Traversal::from_node(start);
        any.steps.push(both(None));
        cases.push(any);
    }
    let mut any_out = Traversal::from_node("a");
    any_out.steps.push(Step {
        direction: Direction::Out,
        edge: None,
        node: None,
    });
    cases.push(any_out);
    cases
}

#[test]
fn indexed_traversal_matches_the_map_walk_except_both_order() {
    let (indexed, plain) = indexed_and_plain();
    for traversal in traversals() {
        let fast = block_on(indexed.traverse(traversal.clone())).unwrap();
        let slow = block_on(plain.traverse(traversal.clone())).unwrap();
        let mixes_directions = traversal
            .steps
            .iter()
            .any(|s| s.direction == Direction::Both);
        if mixes_directions {
            let mut fast_ids: Vec<_> = fast.iter().map(|n| n.id.clone()).collect();
            let mut slow_ids: Vec<_> = slow.iter().map(|n| n.id.clone()).collect();
            fast_ids.sort();
            slow_ids.sort();
            assert_eq!(fast_ids, slow_ids, "{traversal:?}");
        } else {
            assert_eq!(fast, slow, "{traversal:?}");
        }
    }
}

#[test]
fn traverse_ids_matches_traverse_on_both_paths() {
    let (indexed, plain) = indexed_and_plain();
    for traversal in traversals() {
        for store in [&indexed, &plain] {
            let nodes: Vec<_> = block_on(store.traverse(traversal.clone()))
                .unwrap()
                .into_iter()
                .map(|n| n.id)
                .collect();
            let ids = block_on(store.traverse_ids(traversal.clone())).unwrap();
            assert_eq!(ids, nodes, "{traversal:?}");
        }
    }
}

#[test]
fn indexed_edge_queries_match_the_map_walk() {
    let (indexed, plain) = indexed_and_plain();
    let queries = [
        EdgeQuery {
            from: Some("a".into()),
            to: None,
            label: None,
        },
        EdgeQuery {
            from: Some("a".into()),
            to: None,
            label: Some("KNOWS".into()),
        },
        EdgeQuery {
            from: Some("a".into()),
            to: Some("a".into()),
            label: None,
        },
        EdgeQuery {
            from: None,
            to: Some("c".into()),
            label: None,
        },
        EdgeQuery {
            from: None,
            to: Some("c".into()),
            label: Some("NEAR".into()),
        },
        EdgeQuery {
            from: Some("missing".into()),
            to: None,
            label: None,
        },
        EdgeQuery {
            from: Some("d".into()),
            to: None,
            label: None,
        },
        EdgeQuery {
            from: None,
            to: None,
            label: Some("KNOWS".into()),
        },
        EdgeQuery::default(),
    ];
    for query in queries {
        let fast = block_on(indexed.get_edges(query.clone())).unwrap();
        let slow = block_on(plain.get_edges(query.clone())).unwrap();
        assert_eq!(fast, slow, "{query:?}");
    }
}

#[test]
fn a_write_after_the_load_returns_reads_to_the_maps() {
    let store = MemoryGraphStore::new();
    block_on(store.put_graph(&fixture())).unwrap();
    let edge = Edge::new("KNOWS", "a", "c", Props::default());
    block_on(store.put_edge(&edge)).unwrap();
    assert!(store.cached_index().is_none());
    let ids: Vec<_> = block_on(store.traverse(Traversal::from_node("a").out("KNOWS")))
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(
        ids,
        vec![NodeId::from("a"), NodeId::from("b"), NodeId::from("c")]
    );
    let rebuilt = store.indexed_snapshot().unwrap();
    assert_eq!(rebuilt.graph().edges.len(), 7);
    assert!(store.cached_index().is_some());
}

#[test]
fn a_load_into_a_populated_store_leaves_the_maps_in_charge() {
    let store = MemoryGraphStore::new();
    block_on(store.put_graph(&fixture())).unwrap();
    let mut more = GraphBuilder::new();
    let e = more.node("Person", "e").finish();
    let a = more.node("Person", "a").finish();
    let _ = more.edge("KNOWS", &e, &a).finish();
    block_on(store.put_graph(&more.build())).unwrap();
    assert!(store.cached_index().is_none());
    let sources: Vec<_> = block_on(store.traverse(Traversal::from_node("a").in_("KNOWS")))
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(
        sources,
        vec![NodeId::from("a"), NodeId::from("b"), NodeId::from("e")]
    );
}

#[test]
fn a_graph_the_index_rejects_keeps_the_map_path() {
    let store = MemoryGraphStore::new();
    let dangling = Graph {
        nodes: vec![Node::new("Person", "a", Props::default())],
        edges: vec![Edge::new("KNOWS", "a", "ghost", Props::default())],
    };
    block_on(store.put_graph(&dangling)).unwrap();
    assert!(store.cached_index().is_none());
    let edges = block_on(store.get_edges(EdgeQuery {
        from: Some("a".into()),
        to: None,
        label: None,
    }))
    .unwrap();
    assert_eq!(edges.len(), 1);
}
