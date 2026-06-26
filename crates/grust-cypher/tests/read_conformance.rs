//! End-to-end read-query conformance (Unit 6-9 of `docs/GQL_GOAL.md`).
//!
//! Exercises the public `grust_cypher::read::run_read_query` entrypoint as an
//! external consumer would — parse → semantics → Memory reference execution over
//! a fixed graph — across the supported read surface. Complements the in-crate
//! unit tests by pinning the public API boundary and serving as living
//! documentation of what the portable read profile executes today.

use grust_core::{Edge, Graph, Node, Props, Value};
use grust_cypher::read::run_read_query;
use grust_cypher::CypherParameters;

fn node(label: &str, id: &str, props: &[(&str, Value)]) -> Node {
    let mut p = Props::new();
    for (k, v) in props {
        p.insert((*k).to_string(), v.clone());
    }
    Node::new(label, id, p)
}

/// A small social/geo graph: Ada -KNOWS-> Alan -KNOWS-> Grace; Ada -LIVES_IN-> London.
fn fixture() -> Graph {
    let nodes = vec![
        node("Person", "p1", &[("name", Value::from("Ada")), ("age", Value::Int(36))]),
        node("Person", "p2", &[("name", Value::from("Alan")), ("age", Value::Int(41))]),
        node("Person", "p3", &[("name", Value::from("Grace")), ("age", Value::Int(85))]),
        node("City", "c1", &[("name", Value::from("London"))]),
    ];
    let edges = vec![
        Edge::new("KNOWS", "p1", "p2", Props::new()),
        Edge::new("KNOWS", "p2", "p3", Props::new()),
        Edge::new("LIVES_IN", "p1", "c1", Props::new()),
    ];
    Graph::new(nodes, edges)
}

fn rows(cypher: &str) -> Vec<Vec<Value>> {
    run_read_query(&fixture(), cypher, &CypherParameters::new())
        .unwrap_or_else(|e| panic!("query failed: {e}\n  query: {cypher}"))
        .rows
}

fn col0(cypher: &str) -> Vec<Value> {
    rows(cypher).into_iter().map(|mut r| r.remove(0)).collect()
}

#[test]
fn node_scan_and_filter() {
    assert_eq!(
        col0("MATCH (n:Person) WHERE n.age >= 40 RETURN n.name ORDER BY n.name"),
        vec![Value::from("Alan"), Value::from("Grace")]
    );
}

#[test]
fn relationship_and_var_length() {
    assert_eq!(
        col0("MATCH (:Person {name:'Ada'})-[:KNOWS]->(b) RETURN b.name"),
        vec![Value::from("Alan")]
    );
    assert_eq!(
        col0("MATCH (:Person {name:'Ada'})-[:KNOWS*1..2]->(b) RETURN b.name ORDER BY b.name"),
        vec![Value::from("Alan"), Value::from("Grace")]
    );
}

#[test]
fn optional_match_null_pads() {
    assert_eq!(
        rows("MATCH (a:Person) OPTIONAL MATCH (a)-[:KNOWS]->(b) RETURN a.name, b.name ORDER BY a.name"),
        vec![
            vec![Value::from("Ada"), Value::from("Alan")],
            vec![Value::from("Alan"), Value::from("Grace")],
            vec![Value::from("Grace"), Value::Null],
        ]
    );
}

#[test]
fn aggregation_group_by() {
    assert_eq!(
        rows("MATCH (n) RETURN n.label AS label, count(*) AS c ORDER BY label"),
        vec![
            vec![Value::from("City"), Value::Int(1)],
            vec![Value::from("Person"), Value::Int(3)],
        ]
    );
}

#[test]
fn with_horizon_and_aggregate() {
    assert_eq!(
        rows("MATCH (n:Person) WITH n.age AS age WHERE age >= 40 RETURN avg(age) AS mean"),
        vec![vec![Value::Float(63.0)]]
    );
}

#[test]
fn unwind_and_scalar_functions() {
    assert_eq!(
        col0("UNWIND [1, 2, 3] AS x RETURN x * x ORDER BY x"),
        vec![Value::Int(1), Value::Int(4), Value::Int(9)]
    );
    assert_eq!(
        col0("MATCH (n:Person {name:'Ada'}) RETURN toUpper(n.name)"),
        vec![Value::from("ADA")]
    );
}

#[test]
fn union_distinct() {
    assert_eq!(
        col0("MATCH (n:Person {name:'Ada'}) RETURN n.label AS l UNION MATCH (m:Person {name:'Alan'}) RETURN m.label AS l"),
        vec![Value::from("Person")]
    );
}

#[test]
fn case_expression() {
    assert_eq!(
        col0("MATCH (n:Person) RETURN CASE WHEN n.age >= 80 THEN 'senior' ELSE 'other' END AS bucket ORDER BY n.name"),
        vec![Value::from("other"), Value::from("other"), Value::from("senior")]
    );
}

#[test]
fn parameters() {
    let mut params = CypherParameters::new();
    params.insert("min".to_string(), Value::Int(40));
    let table = run_read_query(
        &fixture(),
        "MATCH (n:Person) WHERE n.age >= $min RETURN n.name ORDER BY n.name",
        &params,
    )
    .unwrap();
    assert_eq!(
        table.rows.into_iter().map(|mut r| r.remove(0)).collect::<Vec<_>>(),
        vec![Value::from("Alan"), Value::from("Grace")]
    );
}

#[test]
fn path_variable_and_functions() {
    // p binds a path; length(p)/nodes(p)/relationships(p) read it.
    assert_eq!(
        col0("MATCH p = (:Person {name:'Ada'})-[:KNOWS]->(:Person) RETURN length(p)"),
        vec![Value::Int(1)]
    );
}

#[test]
fn datetime_ordering() {
    // Temporal values order chronologically (RFC 3339 form), not as equal.
    let nodes = vec![
        node("Event", "e1", &[("at", Value::datetime("2026-01-03T00:00:00Z").unwrap())]),
        node("Event", "e2", &[("at", Value::datetime("2026-01-01T00:00:00Z").unwrap())]),
        node("Event", "e3", &[("at", Value::datetime("2026-01-02T00:00:00Z").unwrap())]),
    ];
    let graph = Graph::new(nodes, vec![]);
    let table = run_read_query(
        &graph,
        "MATCH (n:Event) RETURN n.id AS id ORDER BY n.at",
        &CypherParameters::new(),
    )
    .unwrap();
    assert_eq!(
        table.rows.into_iter().map(|mut r| r.remove(0)).collect::<Vec<_>>(),
        vec![Value::from("e2"), Value::from("e3"), Value::from("e1")]
    );
    // DESC reverses chronologically.
    let desc = run_read_query(
        &graph,
        "MATCH (n:Event) RETURN n.id AS id ORDER BY n.at DESC",
        &CypherParameters::new(),
    )
    .unwrap();
    assert_eq!(
        desc.rows.into_iter().map(|mut r| r.remove(0)).collect::<Vec<_>>(),
        vec![Value::from("e1"), Value::from("e3"), Value::from("e2")]
    );
}

#[test]
fn unsupported_read_shapes_error() {
    // Writes are rejected by the read executor.
    assert!(run_read_query(
        &fixture(),
        "CREATE (:Person {id:'x'})",
        &CypherParameters::new()
    )
    .is_err());
    // Path variables over variable-length relationships are not supported yet.
    assert!(run_read_query(
        &fixture(),
        "MATCH p = (:Person)-[:KNOWS*1..2]->(:Person) RETURN p",
        &CypherParameters::new()
    )
    .is_err());
}
