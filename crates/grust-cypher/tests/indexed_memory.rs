//! End-to-end correctness: mutable Memory store -> cached snapshot -> bounded plan.

use std::sync::Arc;

use futures_executor::block_on;
use grust_core::prelude::*;
use grust_cypher::{
    CypherParameters, ReadQueryPolicy, run_bounded_read_query, run_bounded_read_query_indexed,
};
use grust_memory::MemoryGraphStore;

fn fixture() -> Graph {
    let nodes = vec![
        Node::new("A", "a", Props::new()),
        Node::new("B", "b", Props::new()),
        Node::new("C", "c", Props::new()),
    ];
    let mut edges = Vec::new();
    for i in 0..2 {
        edges.push(Edge::new("R", "a", "b", Props::new()).with_id(format!("r{i}")));
    }
    for i in 0..3 {
        edges.push(Edge::new("S", "b", "c", Props::new()).with_id(format!("s{i}")));
    }
    Graph::new(nodes, edges)
}

#[test]
fn bounded_counts_reuse_snapshot_and_follow_store_mutations() {
    block_on(async {
        let store = MemoryGraphStore::new();
        store.put_graph(&fixture()).await.unwrap();
        let first = store.indexed_snapshot().unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &store.clone().indexed_snapshot().unwrap()
        ));
        let query = "MATCH (:A)-[:R]->(:B)-[:S]->(:C) RETURN count(*) AS n LIMIT 1";
        let params = CypherParameters::new();
        let policy = ReadQueryPolicy::default();
        let old = run_bounded_read_query_indexed(&first, query, &params, &policy).unwrap();
        assert_eq!(old.rows, vec![vec![Value::Int(6)]]);
        assert_eq!(
            old,
            run_bounded_read_query(&store.graph(), query, &params, &policy).unwrap()
        );

        store
            .put_edge(&Edge::new("R", "a", "b", Props::new()).with_id("r2"))
            .await
            .unwrap();
        let second = store.indexed_snapshot().unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(
            run_bounded_read_query_indexed(&second, query, &params, &policy)
                .unwrap()
                .rows,
            vec![vec![Value::Int(9)]]
        );
        assert_eq!(
            run_bounded_read_query_indexed(&first, query, &params, &policy).unwrap(),
            old
        );

        store
            .delete_edge(&"a".into(), &"R".into(), &"b".into())
            .await
            .unwrap();
        let third = store.indexed_snapshot().unwrap();
        assert_eq!(
            run_bounded_read_query_indexed(&third, query, &params, &policy)
                .unwrap()
                .rows,
            vec![vec![Value::Int(0)]]
        );
        assert_eq!(
            third.serialized_graph_bytes(),
            serde_json::to_vec(&store.graph()).unwrap().len()
        );
    });
}

#[test]
fn nonfactorized_queries_keep_reference_results_and_limits() {
    block_on(async {
        let store = MemoryGraphStore::new();
        store.put_graph(&fixture()).await.unwrap();
        let index = store.indexed_snapshot().unwrap();
        let query = "MATCH (n) WHERE n.id = 'a' RETURN n.id LIMIT 1";
        let params = CypherParameters::new();
        let policy = ReadQueryPolicy::default();
        assert_eq!(
            run_bounded_read_query_indexed(&index, query, &params, &policy).unwrap(),
            run_bounded_read_query(index.graph(), query, &params, &policy).unwrap()
        );
        let low = ReadQueryPolicy {
            max_candidate_work: 1,
            ..policy
        };
        assert!(run_bounded_read_query_indexed(&index, query, &params, &low).is_err());
    });
}
