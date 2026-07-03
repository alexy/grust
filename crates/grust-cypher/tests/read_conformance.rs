//! End-to-end read-query conformance (Unit 6-9 of `docs/GQL_GOAL.md`).
//!
//! Exercises the public `grust_cypher::read::run_read_query` entrypoint as an
//! external consumer would — parse → semantics → Memory reference execution over
//! a fixed graph — across the supported read surface. Complements the in-crate
//! unit tests by pinning the public API boundary and serving as living
//! documentation of what the portable read profile executes today.

use grust_core::{Edge, Graph, Node, Props, Value};
use grust_cypher::CypherParameters;
use grust_cypher::read::run_read_query;

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
        node(
            "Person",
            "p1",
            &[("name", Value::from("Ada")), ("age", Value::Int(36))],
        ),
        node(
            "Person",
            "p2",
            &[("name", Value::from("Alan")), ("age", Value::Int(41))],
        ),
        node(
            "Person",
            "p3",
            &[("name", Value::from("Grace")), ("age", Value::Int(85))],
        ),
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
        rows(
            "MATCH (a:Person) OPTIONAL MATCH (a)-[:KNOWS]->(b) RETURN a.name, b.name ORDER BY a.name"
        ),
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
        col0(
            "MATCH (n:Person {name:'Ada'}) RETURN n.label AS l UNION MATCH (m:Person {name:'Alan'}) RETURN m.label AS l"
        ),
        vec![Value::from("Person")]
    );
}

#[test]
fn case_expression() {
    assert_eq!(
        col0(
            "MATCH (n:Person) RETURN CASE WHEN n.age >= 80 THEN 'senior' ELSE 'other' END AS bucket ORDER BY n.name"
        ),
        vec![
            Value::from("other"),
            Value::from("other"),
            Value::from("senior")
        ]
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
        table
            .rows
            .into_iter()
            .map(|mut r| r.remove(0))
            .collect::<Vec<_>>(),
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
fn numeric_scalar_functions() {
    // Unary f64 math functions (null-propagating, numeric operands).
    assert_eq!(
        col0("UNWIND [16.0] AS x RETURN sqrt(x)"),
        vec![Value::Float(4.0)]
    );
    assert_eq!(
        col0("UNWIND [0.0] AS x RETURN cos(x)"),
        vec![Value::Float(1.0)]
    );
    assert_eq!(
        col0("UNWIND [0.0] AS x RETURN sin(x)"),
        vec![Value::Float(0.0)]
    );
    assert_eq!(
        col0("UNWIND [1.0] AS x RETURN ln(x)"),
        vec![Value::Float(0.0)]
    );
    assert_eq!(
        col0("UNWIND [0.0] AS x RETURN exp(x)"),
        vec![Value::Float(1.0)]
    );
    // Integer operand widens; null propagates.
    assert_eq!(
        col0("UNWIND [9] AS x RETURN sqrt(x)"),
        vec![Value::Float(3.0)]
    );
    assert_eq!(col0("UNWIND [null] AS x RETURN sqrt(x)"), vec![Value::Null]);
}

#[test]
fn datetime_ordering() {
    // Temporal values order chronologically (RFC 3339 form), not as equal.
    let nodes = vec![
        node(
            "Event",
            "e1",
            &[("at", Value::datetime("2026-01-03T00:00:00Z").unwrap())],
        ),
        node(
            "Event",
            "e2",
            &[("at", Value::datetime("2026-01-01T00:00:00Z").unwrap())],
        ),
        node(
            "Event",
            "e3",
            &[("at", Value::datetime("2026-01-02T00:00:00Z").unwrap())],
        ),
    ];
    let graph = Graph::new(nodes, vec![]);
    let table = run_read_query(
        &graph,
        "MATCH (n:Event) RETURN n.id AS id ORDER BY n.at",
        &CypherParameters::new(),
    )
    .unwrap();
    assert_eq!(
        table
            .rows
            .into_iter()
            .map(|mut r| r.remove(0))
            .collect::<Vec<_>>(),
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
        desc.rows
            .into_iter()
            .map(|mut r| r.remove(0))
            .collect::<Vec<_>>(),
        vec![Value::from("e1"), Value::from("e3"), Value::from("e2")]
    );
}

#[test]
fn catalog_procedures() {
    // db.labels(): distinct node labels, sorted (standalone CALL → result table).
    assert_eq!(
        col0("CALL db.labels()"),
        vec![Value::from("City"), Value::from("Person")]
    );
    // db.relationshipTypes(): distinct edge labels.
    assert_eq!(
        col0("CALL db.relationshipTypes()"),
        vec![Value::from("KNOWS"), Value::from("LIVES_IN")]
    );
    // db.propertyKeys(): distinct property keys across nodes and edges.
    assert_eq!(
        col0("CALL db.propertyKeys()"),
        vec![Value::from("age"), Value::from("id"), Value::from("name")]
    );
}

#[test]
fn call_yield_into_pipeline() {
    // YIELD binds the column for downstream clauses; alias + WHERE + RETURN.
    assert_eq!(
        col0("CALL db.labels() YIELD label AS l WHERE l <> 'City' RETURN l ORDER BY l"),
        vec![Value::from("Person")]
    );
    // Count via aggregation over the yielded rows.
    assert_eq!(
        rows("CALL db.relationshipTypes() YIELD relationshipType AS t RETURN count(*) AS n"),
        vec![vec![Value::Int(2)]]
    );
}

#[test]
fn decimal_values_and_arithmetic() {
    // Constructor + lossless arithmetic (f64 would drift on 0.1 + 0.2).
    assert_eq!(
        col0("RETURN decimal('0.1') + decimal('0.2') AS d"),
        vec![Value::decimal("0.3").unwrap()]
    );
    assert_eq!(
        col0("RETURN decimal('1.5') * decimal('2') AS d"),
        vec![Value::decimal("3").unwrap()]
    );
    // Int coerces exactly into a decimal under +/-/*.
    assert_eq!(
        col0("RETURN decimal('2.5') + 3 AS d"),
        vec![Value::decimal("5.5").unwrap()]
    );
    // Comparison/ordering across decimals.
    assert_eq!(
        col0("UNWIND [decimal('1.50'), decimal('1.05'), decimal('1.5')] AS d RETURN d ORDER BY d"),
        vec![
            Value::decimal("1.05").unwrap(),
            Value::decimal("1.5").unwrap(),
            Value::decimal("1.5").unwrap(),
        ]
    );
    assert_eq!(
        col0("RETURN decimal('2.50') = decimal('2.5') AS eq"),
        vec![Value::Bool(true)]
    );
}

#[test]
fn duration_values_and_arithmetic() {
    // Constructor normalizes (P1Y2M -> 14 months); years/weeks fold in.
    assert_eq!(
        col0("RETURN duration('P1Y2M') AS d"),
        vec![Value::duration("P14M").unwrap()]
    );
    // Component-wise addition stays a duration.
    assert_eq!(
        col0("RETURN duration('P1D') + duration('P2D') AS d"),
        vec![Value::duration("P3D").unwrap()]
    );
    assert_eq!(
        col0("RETURN duration('P1MT1H') - duration('PT30M') AS d"),
        vec![Value::duration("P1MT1800S").unwrap()]
    );
    // Ordering is structural and deterministic.
    assert_eq!(
        col0("UNWIND [duration('P2D'), duration('P1D'), duration('P1M')] AS d RETURN d ORDER BY d"),
        vec![
            Value::duration("P1D").unwrap(),
            Value::duration("P2D").unwrap(),
            Value::duration("P1M").unwrap(),
        ]
    );
}

#[test]
fn unsupported_read_shapes_error() {
    // Writes are rejected by the read executor.
    assert!(
        run_read_query(
            &fixture(),
            "CREATE (:Person {id:'x'})",
            &CypherParameters::new()
        )
        .is_err()
    );
    // Path variables over variable-length relationships are not supported yet.
    assert!(
        run_read_query(
            &fixture(),
            "MATCH p = (:Person)-[:KNOWS*1..2]->(:Person) RETURN p",
            &CypherParameters::new()
        )
        .is_err()
    );
}

/// The portable-read conformance corpus is *executable*, not just
/// integrity-checked: every case in `tests/gql/portable_read.json` runs
/// against the shared fixture graph, and its outcome must match the case's
/// expectation (including the structured error kind for rejected cases). This
/// closes the loop the corpus left open ("execution deferred to Units 6/12")
/// and would have caught the stale pre-F8 `subquery-future` rejection.
#[test]
fn portable_corpus_executes_as_expected() {
    use grust_cypher::{GqlExpectation, load_manifest};

    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("gql")
        .join("portable_read.json");
    let json = std::fs::read_to_string(&path).expect("portable_read.json must be readable");
    let manifest = load_manifest(&json).expect("portable_read.json must load");
    assert!(manifest.cases.len() >= 20, "corpus should stay meaningful");

    let graph = fixture();
    for case in &manifest.cases {
        let result = run_read_query(&graph, &case.statement, &CypherParameters::new());
        match case.expectation {
            GqlExpectation::Supported => {
                let table = result.unwrap_or_else(|e| {
                    panic!(
                        "case `{}` should execute on the reference: {e}\n  statement: {}",
                        case.id, case.statement
                    )
                });
                assert!(
                    !table.columns.is_empty(),
                    "case `{}` produced no columns",
                    case.id
                );
            }
            GqlExpectation::Rejected => {
                let err = result.err().unwrap_or_else(|| {
                    panic!(
                        "case `{}` should be rejected but executed\n  statement: {}",
                        case.id, case.statement
                    )
                });
                let kind = case
                    .error_kind
                    .expect("rejected cases carry an errorKind (pinned by load_manifest)");
                assert!(
                    error_matches_kind(&err, kind),
                    "case `{}` rejected with the wrong error kind: expected {kind:?}, got {err}",
                    case.id
                );
            }
        }
    }
}

/// Mirror of `GqlError`'s kind → `GrustError` transport mapping.
fn error_matches_kind(err: &grust_core::GrustError, kind: grust_cypher::GqlErrorKind) -> bool {
    use grust_core::GrustError;
    use grust_cypher::GqlErrorKind;
    matches!(
        (err, kind),
        (GrustError::CypherSyntax(_), GqlErrorKind::Syntax)
            | (GrustError::CypherUnresolvedIdentity(_), GqlErrorKind::Name)
            | (
                GrustError::CypherUnsupportedCardinality(_),
                GqlErrorKind::Cardinality
            )
            | (GrustError::Unsupported(_), GqlErrorKind::UnsupportedFeature)
            | (
                GrustError::CypherExecution(_),
                GqlErrorKind::Type | GqlErrorKind::Execution
            )
    )
}
