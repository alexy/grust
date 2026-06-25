//! Differential row-equality oracle for the backend read **pushdown**
//! (Unit 15 of `docs/GQL_GOAL.md`).
//!
//! `grust_cypher::pushdown` lowers a bounded read query's `MATCH`/`WHERE` filter
//! into SQL; the Memory reference (`grust_cypher::read::run_read_query`) is the
//! correctness oracle. This test executes the lowered SQL against an **embedded**
//! SQLite engine (the `turso` crate, in-memory — no server) over a `grust_nodes`
//! table populated with untagged JSON props (the format `grust-cypher`'s
//! `SqliteDialect`/`SparkDialect` assume, matching grust-sail's storage), then
//! runs the shared projection and asserts the result is **byte-identical** to the
//! reference over the same graph.
//!
//! This proves the MATCH/WHERE → SQL lowering equivalent against a real SQL
//! engine without any external service. The Sail/Spark path shares the same IR
//! and rendering logic (only function names / quoting differ), so a green oracle
//! gives high confidence in that path too.

use grust_core::{Edge, Graph, Label, Node, NodeId, Props, Value};
use grust_cypher::pushdown::{
    plan_node_read, plan_node_read_with_hints, plan_segment_read, plan_segment_read_with_hints,
    ScalarKind, SqlDialect, SqliteDialect, TypeHints,
};
use grust_cypher::read::run_read_query;
use grust_cypher::{CypherParameters, CypherResultTable};

/// A small social/geo graph with varied types: ints, a float, missing props
/// (NULLs), and a name with an apostrophe (to exercise string escaping).
fn fixture() -> Graph {
    let nodes = vec![
        person("p1", "Ada", 36, None, Some(9.5)),
        person("p2", "Alan", 41, Some("London"), Some(7.0)),
        person("p3", "Grace", 85, Some("London"), None),
        person("p4", "O'Hara", 50, None, Some(7.0)),
        node("City", "c1", &[("name", Value::from("London"))]),
    ];
    let rated = |stars: i64| -> Props {
        let mut p = Props::new();
        p.insert("stars".into(), Value::Int(stars));
        p
    };
    let edges = vec![
        Edge::new("KNOWS", "p1", "p2", Props::new()),
        Edge::new("KNOWS", "p2", "p3", Props::new()),
        Edge::new("KNOWS", "p1", "p4", Props::new()),
        Edge::new("FOLLOWS", "p3", "p1", Props::new()),
        Edge::new("RATED", "p1", "c1", rated(5)),
        Edge::new("RATED", "p2", "c1", rated(3)),
        Edge::new("RATED", "p3", "c1", rated(4)),
    ];
    Graph::new(nodes, edges)
}

fn person(id: &str, name: &str, age: i64, city: Option<&str>, score: Option<f64>) -> Node {
    let mut props: Vec<(&str, Value)> = vec![("name", Value::from(name)), ("age", Value::Int(age))];
    if let Some(city) = city {
        props.push(("city", Value::from(city)));
    }
    if let Some(score) = score {
        props.push(("score", Value::Float(score)));
    }
    node("Person", id, &props)
}

fn node(label: &str, id: &str, props: &[(&str, Value)]) -> Node {
    let mut p = Props::new();
    for (k, v) in props {
        p.insert((*k).to_string(), v.clone());
    }
    Node::new(label, id, p)
}

/// Queries within the pushable subset; each must produce identical rows from the
/// reference and the embedded-SQL pushdown.
const PUSHABLE_QUERIES: &[&str] = &[
    "MATCH (n:Person) RETURN n.name ORDER BY n.name",
    "MATCH (n) RETURN n.label AS label, count(*) AS c ORDER BY label",
    "MATCH (n:Person) WHERE n.age >= 40 RETURN n.name ORDER BY n.name",
    "MATCH (n:Person) WHERE n.age > 30 RETURN n.name, n.age ORDER BY n.age DESC",
    "MATCH (n:Person) WHERE n.score > 8.0 RETURN n.name ORDER BY n.name",
    "MATCH (n:Person {name:'Ada'}) RETURN n.age",
    "MATCH (n:Person) WHERE n.name = 'O\\'Hara' RETURN n.age",
    "MATCH (n:Person) WHERE n.name <> 'Ada' RETURN n.name ORDER BY n.name",
    "MATCH (n:Person) WHERE n.city IS NULL RETURN n.name ORDER BY n.name",
    "MATCH (n:Person) WHERE n.city IS NOT NULL RETURN n.name ORDER BY n.name",
    "MATCH (n:Person) WHERE NOT n.age >= 40 RETURN n.name ORDER BY n.name",
    "MATCH (n:Person) WHERE n.age > 30 AND (n.name = 'Ada' OR n.city IS NULL) RETURN n.name ORDER BY n.name",
    "MATCH (n:Person) WHERE 40 <= n.age RETURN n.name ORDER BY n.name",
    "MATCH (n:Person) WHERE n.score = 7.0 RETURN n.name ORDER BY n.name",
    "MATCH (n:Person) RETURN n.name ORDER BY n.name SKIP 1 LIMIT 2",
    "MATCH (n:Person) WHERE n.age >= 40 RETURN avg(n.age) AS mean",
    "MATCH (n:City) RETURN n.name",
    "MATCH (n:Person) WHERE n.age IN [36, 85] RETURN n.name ORDER BY n.name",
    "MATCH (n:Person) WHERE NOT n.name IN ['Ada'] RETURN n.name ORDER BY n.name",
];

/// Relationship-segment queries within the pushable subset.
const PUSHABLE_SEGMENTS: &[&str] = &[
    "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name",
    "MATCH (a:Person)-[:KNOWS]->(b:Person) WHERE b.age >= 40 RETURN a.name, b.name",
    "MATCH (a:Person)<-[:KNOWS]-(b:Person) RETURN a.name AS who, b.name AS via",
    "MATCH (a:Person {name:'Ada'})-[:KNOWS]->(b) RETURN b.name",
    "MATCH (a)-[r:KNOWS]->(b) RETURN a.name, b.name",
    "MATCH (a)-[:KNOWS|FOLLOWS]->(b:Person) RETURN a.name, b.name",
    "MATCH (a)-[r:RATED]->(b) WHERE r.stars >= 4 RETURN b.name, r.stars",
    "MATCH (a:Person)-[:KNOWS]->(b:Person) WHERE a.name <> 'Ada' RETURN a.name, b.name",
    "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN count(*) AS c",
    "MATCH (a:Person)-[r:KNOWS]->(b:Person) WHERE NOT b.age >= 80 RETURN b.name",
    "MATCH (a:Person)-[:KNOWS]->(b:Person) WHERE b.name IN ['Alan', 'Grace'] RETURN a.name, b.name",
];

#[tokio::test]
async fn pushdown_matches_reference() {
    let graph = fixture();
    let conn = embed(&graph).await;
    let params = CypherParameters::new();
    for cypher in PUSHABLE_QUERIES {
        let expected = run_read_query(&graph, cypher, &params)
            .unwrap_or_else(|e| panic!("reference failed for `{cypher}`: {e}"));
        let actual = pushdown(&conn, cypher, &params).await;
        assert_same(cypher, &actual, &expected);
    }
}

#[tokio::test]
async fn pushed_ordering_preserves_sequence() {
    // ORDER BY / SKIP / LIMIT pushed into SQLite must reproduce the reference's
    // exact row *sequence* (not just multiset). Fixture columns are tie-free.
    let graph = fixture();
    let conn = embed(&graph).await;
    let params = CypherParameters::new();
    for cypher in [
        "MATCH (n:Person) RETURN n.name ORDER BY n.name",
        "MATCH (n:Person) RETURN n.name ORDER BY n.age DESC",
        "MATCH (n:Person) RETURN n.name ORDER BY n.age DESC SKIP 1 LIMIT 2",
        "MATCH (n:Person) WHERE n.city IS NOT NULL RETURN n.name, n.age ORDER BY n.age",
        "MATCH (n:Person) RETURN n.name AS who ORDER BY who SKIP 1",
    ] {
        let plan = plan_node_read(cypher, &params).unwrap().unwrap();
        assert!(
            plan.pushes_ordering(&SqliteDialect),
            "expected pushed ordering for `{cypher}`"
        );
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        let actual = pushdown(&conn, cypher, &params).await;
        assert_eq!(actual, expected, "sequence mismatch for `{cypher}`");
    }
}

/// An SQLite dialect that reports **untyped** JSON extraction, to simulate
/// Spark's `GET_JSON_OBJECT` (text) cast path against the embedded engine: numeric
/// `ORDER BY` keys are cast via `TypeHints`. SQLite executes the same `CAST(...)`,
/// so this verifies the schema-aware ordering Spark relies on without a server.
struct UntypedSqlite;

impl SqlDialect for UntypedSqlite {
    fn nodes_table(&self) -> &str {
        "grust_nodes"
    }
    fn quote_ident(&self, ident: &str) -> String {
        format!("\"{ident}\"")
    }
    fn json_property(&self, props_column: &str, key: &str) -> String {
        format!("json_extract({props_column}, '$.{key}')")
    }
    fn cast_int(&self, expr: &str) -> String {
        format!("CAST({expr} AS INTEGER)")
    }
    fn cast_float(&self, expr: &str) -> String {
        format!("CAST({expr} AS REAL)")
    }
    fn string_literal(&self, value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }
    // orders_json_typed defaults to false → ordering casts via TypeHints.
}

struct OracleHints;
impl TypeHints for OracleHints {
    fn node_property_kind(&self, label: Option<&str>, key: &str) -> Option<ScalarKind> {
        match (label, key) {
            (Some("Person"), "age") => Some(ScalarKind::Int),
            (Some("Person"), "score") => Some(ScalarKind::Float),
            (Some("Person"), "name") => Some(ScalarKind::Str),
            _ => None,
        }
    }
    fn edge_property_kind(&self, edge_type: Option<&str>, key: &str) -> Option<ScalarKind> {
        match (edge_type, key) {
            (Some("RATED"), "stars") => Some(ScalarKind::Int),
            _ => None,
        }
    }
}

#[tokio::test]
async fn schema_aware_ordering_casts_match_reference() {
    // Simulates the Spark path: untyped JSON dialect + schema hints → numeric
    // ORDER BY keys are cast, executed by SQLite, compared to the reference.
    let graph = fixture();
    let conn = embed(&graph).await;
    let params = CypherParameters::new();
    for cypher in [
        "MATCH (n:Person) RETURN n.name ORDER BY n.age",
        "MATCH (n:Person) RETURN n.name ORDER BY n.age DESC SKIP 1 LIMIT 2",
        "MATCH (n:Person) WHERE n.score IS NOT NULL RETURN n.name ORDER BY n.score, n.name",
    ] {
        let plan = plan_node_read_with_hints(cypher, &params, &OracleHints)
            .unwrap()
            .unwrap();
        assert!(
            plan.pushes_ordering(&UntypedSqlite),
            "hints should make `{cypher}` pushable on an untyped dialect"
        );
        let sql = plan.to_sql(&UntypedSqlite);
        assert!(sql.contains("CAST(json_extract"), "expected a cast in `{sql}`");
        let mut rows = conn.query(&sql, ()).await.unwrap();
        let mut nodes = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            nodes.push(Node {
                id: NodeId::new(text(&row, 0)),
                label: Label::new(text(&row, 1)),
                props: parse_props(&text(&row, 2)),
            });
        }
        let actual = plan.project(&UntypedSqlite, nodes, &params).unwrap();
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_eq!(actual, expected, "sequence mismatch for `{cypher}`");
    }
}

#[tokio::test]
async fn untyped_segment_edge_ordering_matches_reference() {
    // Spark-path simulation: untyped dialect + edge hints cast the edge-property
    // sort key; SQLite executes it and must match the reference sequence.
    let graph = fixture();
    let conn = embed(&graph).await;
    let params = CypherParameters::new();
    let cypher = "MATCH (a:Person)-[r:RATED]->(b) RETURN a.name ORDER BY r.stars DESC";
    let plan = plan_segment_read_with_hints(cypher, &params, &OracleHints)
        .unwrap()
        .unwrap();
    assert!(plan.pushes_ordering(&UntypedSqlite));
    let sql = plan.to_sql(&UntypedSqlite);
    assert!(sql.contains("CAST(json_extract(re.props"), "expected an edge cast in `{sql}`");
    let n = plan.column_count();
    let mut rows = conn.query(&sql, ()).await.unwrap();
    let mut text_rows = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        text_rows.push((0..n).map(|i| opt_text(&row, i)).collect());
    }
    let actual = plan
        .project_text_rows(&UntypedSqlite, text_rows, &params)
        .unwrap();
    let expected = run_read_query(&graph, cypher, &params).unwrap();
    assert_eq!(actual, expected, "untyped segment edge ordering mismatch");
}

#[tokio::test]
async fn segment_pushdown_matches_reference() {
    let graph = fixture();
    let conn = embed(&graph).await;
    let params = CypherParameters::new();
    for cypher in PUSHABLE_SEGMENTS {
        let expected = run_read_query(&graph, cypher, &params)
            .unwrap_or_else(|e| panic!("reference failed for `{cypher}`: {e}"));
        let actual = segment_pushdown(&conn, cypher, &params).await;
        assert_same(cypher, &actual, &expected);
    }
}

#[tokio::test]
async fn segment_pushed_ordering_preserves_sequence() {
    // Segment ORDER BY / SKIP / LIMIT pushed into SQLite must reproduce the
    // reference's exact sequence (tie-free b.age in the fixture).
    let graph = fixture();
    let conn = embed(&graph).await;
    let params = CypherParameters::new();
    for cypher in [
        "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name ORDER BY b.age DESC",
        "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN b.name ORDER BY b.age SKIP 1",
        // Edge-property ordering on a typed-JSON dialect (no hints needed).
        "MATCH (a:Person)-[r:RATED]->(b) RETURN a.name ORDER BY r.stars",
    ] {
        let plan = plan_segment_read(cypher, &params).unwrap().unwrap();
        assert!(
            plan.pushes_ordering(&SqliteDialect),
            "expected pushed ordering for `{cypher}`"
        );
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        let actual = segment_pushdown(&conn, cypher, &params).await;
        assert_eq!(actual, expected, "segment sequence mismatch for `{cypher}`");
    }
}

/// Compare result tables by column names and **row multiset** — Cypher results
/// are unordered without a total `ORDER BY`, and a SQL join's row order is not
/// guaranteed, so set equality (not sequence) is the correctness criterion.
fn assert_same(cypher: &str, actual: &CypherResultTable, expected: &CypherResultTable) {
    assert_eq!(actual.columns, expected.columns, "columns for `{cypher}`");
    let sorted = |t: &CypherResultTable| {
        let mut rows: Vec<String> = t.rows.iter().map(|r| format!("{r:?}")).collect();
        rows.sort();
        rows
    };
    assert_eq!(sorted(actual), sorted(expected), "row multiset for `{cypher}`");
}

#[tokio::test]
async fn pushdown_matches_reference_with_parameters() {
    let graph = fixture();
    let conn = embed(&graph).await;
    let mut params = CypherParameters::new();
    params.insert("min".to_string(), Value::Int(50));
    let cypher = "MATCH (n:Person) WHERE n.age >= $min RETURN n.name ORDER BY n.name";
    let expected = run_read_query(&graph, cypher, &params).unwrap();
    let actual = pushdown(&conn, cypher, &params).await;
    assert_eq!(actual, expected);
}

/// Run a query via the pushdown path: lower to SQLite SQL, execute against the
/// embedded engine, reconstruct the surviving nodes, and project.
async fn pushdown(
    conn: &turso::Connection,
    cypher: &str,
    params: &CypherParameters,
) -> grust_cypher::CypherResultTable {
    let plan = plan_node_read(cypher, params)
        .unwrap()
        .unwrap_or_else(|| panic!("expected `{cypher}` to be pushable"));
    let sql = plan.to_sql(&SqliteDialect);
    let mut rows = conn
        .query(&sql, ())
        .await
        .unwrap_or_else(|e| panic!("embedded query failed for `{sql}`: {e}"));
    let mut nodes = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        nodes.push(Node {
            id: NodeId::new(text(&row, 0)),
            label: Label::new(text(&row, 1)),
            props: parse_props(&text(&row, 2)),
        });
    }
    plan.project(&SqliteDialect, nodes, params).unwrap()
}

/// Run a relationship-segment query via the pushdown path: lower to SQLite SQL,
/// execute, read the selected columns as text cells, reconstruct + project.
async fn segment_pushdown(
    conn: &turso::Connection,
    cypher: &str,
    params: &CypherParameters,
) -> CypherResultTable {
    let plan = plan_segment_read(cypher, params)
        .unwrap()
        .unwrap_or_else(|| panic!("expected `{cypher}` to be pushable as a segment"));
    let sql = plan.to_sql(&SqliteDialect);
    let n = plan.column_count();
    let mut rows = conn
        .query(&sql, ())
        .await
        .unwrap_or_else(|e| panic!("embedded segment query failed for `{sql}`: {e}"));
    let mut text_rows = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        text_rows.push((0..n).map(|i| opt_text(&row, i)).collect());
    }
    plan.project_text_rows(&SqliteDialect, text_rows, params).unwrap()
}

/// Build an in-memory SQLite database with `grust_nodes(id, label, props)` and
/// `grust_edges(id, src_id, dst_id, edge_type, props)` populated from the graph,
/// props stored as untagged JSON.
async fn embed(graph: &Graph) -> turso::Connection {
    let db = turso::Builder::new_local(":memory:").build().await.unwrap();
    let conn = db.connect().unwrap();
    conn.execute_batch(
        "CREATE TABLE grust_nodes (id TEXT, label TEXT, props TEXT); \
         CREATE TABLE grust_edges (id TEXT, src_id TEXT, dst_id TEXT, edge_type TEXT, props TEXT);",
    )
    .await
    .unwrap();
    for n in &graph.nodes {
        let sql = format!(
            "INSERT INTO grust_nodes (id, label, props) VALUES ({}, {}, {});",
            lit(n.id.as_str()),
            lit(n.label.as_str()),
            lit(&untagged_props(&n.props)),
        );
        conn.execute_batch(&sql).await.unwrap();
    }
    for e in &graph.edges {
        let id = e
            .id
            .as_ref()
            .map(|i| lit(i.as_str()))
            .unwrap_or_else(|| "NULL".to_string());
        let sql = format!(
            "INSERT INTO grust_edges (id, src_id, dst_id, edge_type, props) VALUES ({}, {}, {}, {}, {});",
            id,
            lit(e.from.as_str()),
            lit(e.to.as_str()),
            lit(e.label.as_str()),
            lit(&untagged_props(&e.props)),
        );
        conn.execute_batch(&sql).await.unwrap();
    }
    // Keep the database alive for the lifetime of the connection by leaking it;
    // the process is a single test binary, so this is fine.
    Box::leak(Box::new(db));
    conn
}

/// Serialize props to plain (untagged) JSON, matching `SqliteDialect`'s `$.key`
/// extraction (and grust-sail's `props_to_json`).
fn untagged_props(props: &Props) -> String {
    let mut map = serde_json::Map::new();
    for (key, value) in props {
        map.insert(key.clone(), value.to_json());
    }
    serde_json::to_string(&serde_json::Value::Object(map)).unwrap()
}

fn parse_props(json: &str) -> Props {
    let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(json).unwrap();
    map.into_iter()
        .map(|(k, v)| (k, Value::from_json(v)))
        .collect()
}

fn text(row: &turso::Row, idx: usize) -> String {
    match row.get_value(idx).unwrap() {
        turso::Value::Text(s) => s,
        other => panic!("expected text at column {idx}, got {other:?}"),
    }
}

fn opt_text(row: &turso::Row, idx: usize) -> Option<String> {
    match row.get_value(idx).unwrap() {
        turso::Value::Text(s) => Some(s),
        turso::Value::Null => None,
        other => panic!("expected text/null at column {idx}, got {other:?}"),
    }
}

/// SQLite string literal (single-quote doubling).
fn lit(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
