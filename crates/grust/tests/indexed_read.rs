#![cfg(feature = "cypher")]

use std::sync::Arc;

use grust::prelude::*;

#[test]
fn facade_and_prelude_expose_bounded_indexed_reads() {
    let graph = Graph::new(vec![Node::new("Person", "p", Props::new())], vec![]);
    let index = TypedGraphIndex::new(Arc::new(graph)).unwrap();
    let query = "MATCH (:Person) RETURN count(*) AS people LIMIT 1";
    let params = CypherParameters::new();
    let policy = ReadQueryPolicy::default();
    let result = run_bounded_read_query_indexed(&index, query, &params, &policy).unwrap();
    assert_eq!(result.columns, ["people"]);
    assert_eq!(result.rows, vec![vec![Value::Int(1)]]);
    assert_eq!(
        result,
        grust::run_bounded_read_query_indexed(&index, query, &params, &policy).unwrap()
    );
}
