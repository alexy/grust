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
    ReadPushdown, ScalarKind, SqlDialect, SqliteDialect, StrOp, TypeHints, combine_union,
    plan_node_read, plan_node_read_with_hints, plan_read, plan_segment_read,
    plan_segment_read_with_hints, plan_var_length_read,
};
use grust_cypher::read::run_read_query;
use grust_cypher::{CypherParameters, CypherResultTable};

/// A small social/geo graph with varied types: ints, a float, missing props
/// (NULLs), and a name with an apostrophe (to exercise string escaping).
fn fixture() -> Graph {
    let nodes = vec![
        person("p1", "Ada", 36, None, Some(9.5), true),
        person("p2", "Alan", 41, Some("London"), Some(7.0), false),
        person("p3", "Grace", 85, Some("London"), None, true),
        person("p4", "O'Hara", 50, None, Some(7.0), false),
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

fn person(
    id: &str,
    name: &str,
    age: i64,
    city: Option<&str>,
    score: Option<f64>,
    active: bool,
) -> Node {
    let mut props: Vec<(&str, Value)> = vec![
        ("name", Value::from(name)),
        ("age", Value::Int(age)),
        ("active", Value::Bool(active)),
    ];
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
    "MATCH (n:Person) WHERE n.name STARTS WITH 'A' RETURN n.name ORDER BY n.name",
    "MATCH (n:Person) WHERE n.name ENDS WITH 'e' RETURN n.name ORDER BY n.name",
    "MATCH (n:Person) WHERE n.name CONTAINS 'l' RETURN n.name ORDER BY n.name",
    "MATCH (n:Person) WHERE NOT n.name CONTAINS 'a' RETURN n.name ORDER BY n.name",
    "MATCH (n:Person) WHERE n.active = true RETURN n.name ORDER BY n.name",
    "MATCH (n:Person) WHERE n.active <> true RETURN n.name ORDER BY n.name",
    "MATCH (n:Person) WHERE n.active = false RETURN n.name ORDER BY n.name",
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
    // Undirected: both orientations of each edge appear (compared as a multiset).
    "MATCH (a:Person)-[:KNOWS]-(b:Person) RETURN a.name, b.name",
    "MATCH (a:Person {name:'Ada'})-[:KNOWS]-(b) RETURN b.name",
    "MATCH (a:Person)-[:RATED]-(b) RETURN a.name, b.name",
    // Multi-segment (chained) paths.
    "MATCH (a:Person)-[:KNOWS]->(b)-[:KNOWS]->(c) RETURN a.name, b.name, c.name",
    "MATCH (a:Person)-[:KNOWS]->(b)-[:KNOWS]->(c) WHERE c.age >= 80 RETURN a.name, c.name",
    "MATCH (a:Person)-[:KNOWS]->(b)<-[:KNOWS]-(c) RETURN a.name, c.name",
    "MATCH (a:Person)-[:KNOWS]->(b)-[:RATED]->(c) RETURN a.name, c.name",
    "MATCH (:Person)-[:KNOWS]->(b) WHERE b.name STARTS WITH 'A' RETURN b.name",
    "MATCH (a:Person)-[:KNOWS]->(b) WHERE b.name CONTAINS 'r' RETURN b.name",
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
    fn string_predicate(&self, expr: &str, op: StrOp, needle: &str) -> String {
        let n = self.string_literal(needle);
        match op {
            StrOp::StartsWith => format!("instr({expr}, {n}) = 1"),
            StrOp::Contains => format!("instr({expr}, {n}) > 0"),
            StrOp::EndsWith => format!("substr({expr}, -{}) = {n}", needle.chars().count()),
        }
    }
    fn bool_literal_sql(&self, value: bool) -> String {
        if value {
            "1".to_string()
        } else {
            "0".to_string()
        }
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
        assert!(
            sql.contains("CAST(json_extract"),
            "expected a cast in `{sql}`"
        );
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
    assert!(
        sql.contains("CAST(json_extract(e0.props"),
        "expected an edge cast in `{sql}`"
    );
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
async fn arithmetic_pushdown_matches_reference() {
    // `+`/`-`/`*` over typed numeric properties (hints supply the types).
    let graph = fixture();
    let conn = embed(&graph).await;
    let params = CypherParameters::new();
    for cypher in [
        "MATCH (n:Person) WHERE n.age + 1 > 40 RETURN n.name ORDER BY n.name",
        "MATCH (n:Person) WHERE n.age - 6 = 30 RETURN n.name ORDER BY n.name",
        "MATCH (n:Person) WHERE n.score * 2 > 15.0 RETURN n.name ORDER BY n.name",
        "MATCH (n:Person) WHERE n.age * 2 >= 100 RETURN n.name ORDER BY n.name",
        "MATCH (n:Person) WHERE n.age / 2 > 20 RETURN n.name ORDER BY n.name",
        "MATCH (n:Person) WHERE n.score / 2 >= 4.0 RETURN n.name ORDER BY n.name",
    ] {
        let plan = plan_node_read_with_hints(cypher, &params, &OracleHints)
            .unwrap()
            .unwrap_or_else(|| panic!("expected `{cypher}` to be pushable"));
        let sql = plan.to_sql(&SqliteDialect);
        let mut rows = conn
            .query(&sql, ())
            .await
            .unwrap_or_else(|e| panic!("arith query failed for `{sql}`: {e}"));
        let mut nodes = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            nodes.push(Node {
                id: NodeId::new(text(&row, 0)),
                label: Label::new(text(&row, 1)),
                props: parse_props(&text(&row, 2)),
            });
        }
        let actual = plan.project(&SqliteDialect, nodes, &params).unwrap();
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
}

#[tokio::test]
async fn segment_arithmetic_matches_reference() {
    let graph = fixture();
    let conn = embed(&graph).await;
    let params = CypherParameters::new();
    for cypher in [
        "MATCH (a:Person)-[:KNOWS]->(b:Person) WHERE b.age + 1 > 42 RETURN a.name, b.name",
        "MATCH (a)-[r:RATED]->(b) WHERE r.stars * 2 >= 8 RETURN a.name, r.stars",
    ] {
        let plan = plan_segment_read_with_hints(cypher, &params, &OracleHints)
            .unwrap()
            .unwrap_or_else(|| panic!("expected `{cypher}` to be pushable"));
        let sql = plan.to_sql(&SqliteDialect);
        let n = plan.column_count();
        let mut rows = conn.query(&sql, ()).await.unwrap();
        let mut text_rows = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            text_rows.push((0..n).map(|i| opt_text(&row, i)).collect());
        }
        let actual = plan
            .project_text_rows(&SqliteDialect, text_rows, &params)
            .unwrap();
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
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

#[test]
fn var_length_pushdown_matches_reference() {
    // Variable-length lowers to a recursive CTE; the embedded `turso` engine does
    // not support `WITH RECURSIVE`, so this oracle runs against real SQLite
    // (`rusqlite`, bundled) — the dialect `SqliteDialect` targets.
    //
    // A chain p1 -> p11 -> p2 -> p3 plus an alternate p1 -> p2. The ids p1/p11
    // are prefix-colliding to exercise the U+001F-delimited visited check, so the
    // no-repeated-nodes rule matches the reference exactly.
    let nodes = vec![
        node("N", "p1", &[("name", Value::from("A"))]),
        node("N", "p11", &[("name", Value::from("B"))]),
        node("N", "p2", &[("name", Value::from("C"))]),
        node("N", "p3", &[("name", Value::from("D"))]),
    ];
    let edges = vec![
        Edge::new("R", "p1", "p11", Props::new()),
        Edge::new("R", "p11", "p2", Props::new()),
        Edge::new("R", "p2", "p3", Props::new()),
        Edge::new("R", "p1", "p2", Props::new()),
    ];
    let graph = Graph::new(nodes, edges);
    let conn = embed_sqlite(&graph);
    let params = CypherParameters::new();
    for cypher in [
        "MATCH (a:N {name:'A'})-[:R*1..2]->(b) RETURN b.name",
        "MATCH (a:N {name:'A'})-[:R*1..3]->(b) RETURN b.name",
        "MATCH (a:N)-[:R*2..2]->(b) RETURN a.name, b.name",
        "MATCH (a:N)-[:R*1..3]->(b) WHERE b.name <> 'A' RETURN a.name, b.name",
        "MATCH (a:N {name:'A'})-[:R*1..2]-(b) RETURN b.name",
    ] {
        let plan = plan_var_length_read(cypher, &params)
            .unwrap()
            .unwrap_or_else(|| panic!("expected `{cypher}` to be var-length pushable"));
        let sql = plan.to_sql(&SqliteDialect);
        let n = plan.column_count();
        let mut stmt = conn
            .prepare(&sql)
            .unwrap_or_else(|e| panic!("prepare failed for `{sql}`: {e}"));
        let text_rows: Vec<Vec<Option<String>>> = stmt
            .query_map([], |row| {
                (0..n)
                    .map(|i| row.get::<usize, Option<String>>(i))
                    .collect()
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let actual = plan
            .project_text_rows(&SqliteDialect, text_rows, &params)
            .unwrap();
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
}

/// Build a real in-memory SQLite database (rusqlite) populated from the graph.
fn embed_sqlite(graph: &Graph) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE grust_nodes (id TEXT, label TEXT, props TEXT); \
         CREATE TABLE grust_edges (id TEXT, src_id TEXT, dst_id TEXT, edge_type TEXT, props TEXT);",
    )
    .unwrap();
    for n in &graph.nodes {
        conn.execute_batch(&format!(
            "INSERT INTO grust_nodes (id, label, props) VALUES ({}, {}, {});",
            lit(n.id.as_str()),
            lit(n.label.as_str()),
            lit(&untagged_props(&n.props)),
        ))
        .unwrap();
    }
    for e in &graph.edges {
        let id =
            e.id.as_ref()
                .map(|i| lit(i.as_str()))
                .unwrap_or_else(|| "NULL".to_string());
        conn.execute_batch(&format!(
            "INSERT INTO grust_edges (id, src_id, dst_id, edge_type, props) VALUES ({}, {}, {}, {}, {});",
            id,
            lit(e.from.as_str()),
            lit(e.to.as_str()),
            lit(e.label.as_str()),
            lit(&untagged_props(&e.props)),
        ))
        .unwrap();
    }
    conn
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
    assert_eq!(
        sorted(actual),
        sorted(expected),
        "row multiset for `{cypher}`"
    );
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
    plan.project_text_rows(&SqliteDialect, text_rows, params)
        .unwrap()
}

#[tokio::test]
async fn with_pipeline_matches_reference() {
    let graph = fixture();
    let conn = embed(&graph).await;
    let params = CypherParameters::new();
    for cypher in [
        "MATCH (n:Person) WHERE n.age >= 40 WITH n.age AS age RETURN avg(age) AS mean",
        "MATCH (n:Person) WITH n.name AS name WHERE name <> 'Ada' RETURN name ORDER BY name",
        "MATCH (n:Person) WITH n.label AS l, count(*) AS c RETURN l, c",
        "MATCH (n:Person) WHERE n.age >= 40 WITH n ORDER BY n.age DESC LIMIT 1 RETURN n.name",
    ] {
        let plan = plan_read(cypher, &params, &OracleHints)
            .unwrap()
            .unwrap_or_else(|| panic!("expected `{cypher}` to be pushable"));
        let actual = run_pushdown(&conn, &plan, &params).await;
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
}

#[tokio::test]
async fn multi_pattern_pushdown_matches_reference() {
    let graph = fixture();
    let conn = embed(&graph).await;
    let params = CypherParameters::new();
    for cypher in [
        "MATCH (a:Person)-[:KNOWS]->(b), (a)-[:RATED]->(c) RETURN a.name, b.name, c.name",
        "MATCH (a:Person), (c:City) RETURN a.name, c.name",
        "MATCH (a:Person)-[:KNOWS]->(b:Person), (b)-[:KNOWS]->(c:Person) RETURN a.name, c.name",
    ] {
        let plan = plan_read(cypher, &params, &OracleHints)
            .unwrap()
            .unwrap_or_else(|| panic!("expected `{cypher}` to be pushable"));
        let actual = run_pushdown(&conn, &plan, &params).await;
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
}

#[tokio::test]
async fn optional_match_pushdown_matches_reference() {
    let graph = fixture();
    let conn = embed(&graph).await;
    let params = CypherParameters::new();
    for cypher in [
        "MATCH (a:Person) OPTIONAL MATCH (a)-[:KNOWS]->(b:Person) RETURN a.name, b.name",
        "MATCH (a:Person) OPTIONAL MATCH (a)-[r:KNOWS]->(b) WHERE b.age >= 40 RETURN a.name, b.name",
        "MATCH (a:Person) OPTIONAL MATCH (a)<-[:KNOWS]-(b) RETURN a.name, b.name",
        "MATCH (a:Person) WHERE a.age >= 40 OPTIONAL MATCH (a)-[:RATED]->(b) RETURN a.name, b.name",
    ] {
        let plan = plan_read(cypher, &params, &OracleHints)
            .unwrap()
            .unwrap_or_else(|| panic!("expected `{cypher}` to be pushable"));
        let actual = run_pushdown(&conn, &plan, &params).await;
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
}

#[tokio::test]
async fn union_pushdown_matches_reference() {
    let graph = fixture();
    let conn = embed(&graph).await;
    let params = CypherParameters::new();
    for cypher in [
        "MATCH (n:Person) RETURN n.name AS x UNION MATCH (c:City) RETURN c.name AS x",
        "MATCH (n:Person) WHERE n.age >= 50 RETURN n.name AS x \
         UNION ALL MATCH (c:City) RETURN c.name AS x",
        "MATCH (n:Person {name:'Ada'}) RETURN n.name AS x \
         UNION MATCH (:Person)-[:KNOWS]->(b) RETURN b.name AS x",
    ] {
        let plan = plan_read(cypher, &params, &OracleHints)
            .unwrap()
            .unwrap_or_else(|| panic!("expected `{cypher}` to be pushable"));
        let actual = run_pushdown(&conn, &plan, &params).await;
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
}

/// Execute a unified [`ReadPushdown`] the way a backend does: run each leaf's
/// SQL, reconstruct + project, and combine `UNION` arms.
async fn run_pushdown(
    conn: &turso::Connection,
    plan: &ReadPushdown,
    params: &CypherParameters,
) -> CypherResultTable {
    if let Some((arms, distinct)) = plan.union_arms() {
        let mut tables = Vec::with_capacity(arms.len());
        for arm in arms {
            tables.push(run_leaf(conn, arm, params).await);
        }
        return combine_union(tables, distinct).unwrap();
    }
    run_leaf(conn, plan, params).await
}

async fn run_leaf(
    conn: &turso::Connection,
    leaf: &ReadPushdown,
    params: &CypherParameters,
) -> CypherResultTable {
    let sql = leaf.to_sql(&SqliteDialect);
    let n = leaf.column_count();
    let mut rows = conn
        .query(&sql, ())
        .await
        .unwrap_or_else(|e| panic!("leaf query failed for `{sql}`: {e}"));
    let mut text_rows = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        text_rows.push((0..n).map(|i| opt_text(&row, i)).collect());
    }
    leaf.project_text_rows(&SqliteDialect, text_rows, params)
        .unwrap()
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
        let id =
            e.id.as_ref()
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

/// Run a leaf `ReadPushdown` against real SQLite (rusqlite) — for row sources
/// the embedded `turso` engine cannot execute (`WITH RECURSIVE`, `json_each`).
fn run_leaf_sqlite(
    conn: &rusqlite::Connection,
    leaf: &ReadPushdown,
    params: &CypherParameters,
) -> CypherResultTable {
    let sql = leaf.to_sql(&SqliteDialect);
    let n = leaf.column_count();
    let mut stmt = conn
        .prepare(&sql)
        .unwrap_or_else(|e| panic!("prepare failed for `{sql}`: {e}"));
    let text_rows: Vec<Vec<Option<String>>> = stmt
        .query_map([], |row| {
            (0..n)
                .map(|i| {
                    // Decode typed cells (the range CTE yields INTEGER) as text.
                    row.get::<usize, Option<rusqlite::types::Value>>(i)
                        .map(|v| match v {
                            None | Some(rusqlite::types::Value::Null) => None,
                            Some(rusqlite::types::Value::Integer(x)) => Some(x.to_string()),
                            Some(rusqlite::types::Value::Real(x)) => Some(x.to_string()),
                            Some(rusqlite::types::Value::Text(x)) => Some(x),
                            Some(rusqlite::types::Value::Blob(_)) => None,
                        })
                })
                .collect()
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    leaf.project_text_rows(&SqliteDialect, text_rows, params)
        .unwrap()
}

/// PUSHDOWN2 P1: catalog procedures lower to DISTINCT scans (plain SQL — the
/// embedded `turso` engine runs them) and must match the reference exactly,
/// including the YIELD/WHERE tail and aggregate pipelines.
#[tokio::test]
async fn catalog_procedure_pushdown_matches_reference() {
    let graph = fixture();
    let conn = embed(&graph).await;
    let params = CypherParameters::new();
    for cypher in [
        "CALL db.labels()",
        "CALL db.labels() YIELD label RETURN label",
        "CALL db.labels() YIELD label AS l WHERE l <> 'City' RETURN l ORDER BY l",
        "CALL db.relationshipTypes()",
        "CALL db.relationshipTypes() YIELD relationshipType AS t RETURN count(*) AS n",
        "CALL db.labels() YIELD label WITH label AS l RETURN l ORDER BY l DESC",
    ] {
        let plan = plan_read(cypher, &params, &OracleHints)
            .unwrap()
            .unwrap_or_else(|| panic!("expected `{cypher}` to be pushable"));
        assert!(plan.supported_by(&SqliteDialect));
        let actual = run_pushdown(&conn, &plan, &params).await;
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
}

/// PUSHDOWN2 P1/P2: `db.propertyKeys` (json_each) and `tvf.range` (recursive
/// CTE) run against real SQLite and must match the reference exactly.
#[test]
fn property_keys_and_range_pushdown_match_reference() {
    let graph = fixture();
    let conn = embed_sqlite(&graph);
    let mut params = CypherParameters::new();
    params.insert("hi".to_string(), Value::Int(5));
    for cypher in [
        "CALL db.propertyKeys()",
        "CALL db.propertyKeys() YIELD propertyKey RETURN propertyKey",
        "CALL db.propertyKeys() YIELD propertyKey AS k WHERE k STARTS WITH 'a' RETURN k",
        "CALL tvf.range(1, 3) YIELD value RETURN value",
        "CALL tvf.range(1, 3)",
        "CALL tvf.range(3, 1) YIELD value RETURN value",
        "CALL tvf.range(5, 1, -2) YIELD value RETURN value",
        "CALL tvf.range(1, $hi, 2) YIELD value RETURN value",
        "CALL tvf.range(1, 4) YIELD value AS v WHERE v > 1 RETURN v, v * 10 AS tens",
        "CALL tvf.range(1, 3) YIELD value RETURN sum(value) AS total",
    ] {
        let plan = plan_read(cypher, &params, &OracleHints)
            .unwrap()
            .unwrap_or_else(|| panic!("expected `{cypher}` to be pushable"));
        assert!(plan.supported_by(&SqliteDialect), "`{cypher}` gated off");
        let actual = run_leaf_sqlite(&conn, &plan, &params);
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
}

/// PUSHDOWN2 P3: uncorrelated `CALL { … }` subqueries lower to plain scans /
/// a LEFT JOIN of two scans (the embedded `turso` engine runs them) and must
/// match the reference exactly — including the per-outer-row inner aggregate
/// over an empty inner scan, and drop-on-empty for row-producing inners.
#[tokio::test]
async fn uncorrelated_subquery_pushdown_matches_reference() {
    let graph = fixture();
    let conn = embed(&graph).await;
    let params = CypherParameters::new();
    for cypher in [
        // Leading subquery: rows are the inner rows.
        "CALL { MATCH (c:City) RETURN c.name AS n } RETURN n",
        "CALL { MATCH (p:Person) WITH p.name AS n WHERE n STARTS WITH 'A' RETURN n } RETURN n ORDER BY n",
        // MATCH × subquery join, filters pushed on both sides.
        "MATCH (a:Person) CALL { MATCH (c:City) RETURN c.name AS city } RETURN a.name, city ORDER BY a.name",
        "MATCH (a:Person {name:'Ada'}) CALL { MATCH (b:Person) WHERE b.age >= 41 RETURN b.name AS n } RETURN a.name, n ORDER BY n",
        // Inner aggregate: one row per outer row…
        "MATCH (a:Person) CALL { MATCH (c:City) RETURN count(*) AS cities } RETURN a.name, cities ORDER BY a.name",
        // …including over an EMPTY inner scan (LEFT JOIN empty-group row).
        "MATCH (a:Person {name:'Ada'}) CALL { MATCH (x:Nowhere) RETURN count(*) AS n } RETURN a.name, n",
        // Row-producing empty inner drops the outer rows entirely.
        "MATCH (a:Person) CALL { MATCH (x:Nowhere) RETURN x.name AS n } RETURN a.name, n",
        // Inner DISTINCT + outer tail aggregate over the joined rows.
        "MATCH (a:City) CALL { MATCH (p:Person) RETURN DISTINCT p.label AS l } RETURN a.name, l",
        "MATCH (a:Person) CALL { MATCH (c:City) RETURN c.name AS city } WITH a, city RETURN count(*) AS total",
    ] {
        let plan = plan_read(cypher, &params, &OracleHints)
            .unwrap()
            .unwrap_or_else(|| panic!("expected `{cypher}` to be pushable"));
        assert!(plan.supported_by(&SqliteDialect));
        let actual = run_pushdown(&conn, &plan, &params).await;
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
}

/// PUSHDOWN2 P4a: correlated `tvf.keys(n)` lowers to a lateral `json_each`
/// join (real SQLite) and must match the reference exactly, including sorted
/// key order per row, YIELD aliasing + WHERE, and aggregate tails.
#[test]
fn correlated_keys_pushdown_matches_reference() {
    let graph = fixture();
    let conn = embed_sqlite(&graph);
    let params = CypherParameters::new();
    for cypher in [
        "MATCH (n:Person {name:'Ada'}) CALL tvf.keys(n) YIELD key RETURN key",
        "MATCH (n:Person) CALL tvf.keys(n) YIELD key RETURN DISTINCT key ORDER BY key",
        "MATCH (n:Person) CALL tvf.keys(n) YIELD key AS k WHERE k STARTS WITH 'a' RETURN n.name, k ORDER BY n.name, k",
        "MATCH (n:City) CALL tvf.keys(n) YIELD key RETURN n.name, count(*) AS props",
        "MATCH (n:Person {name:'Ada'}) CALL tvf.keys(n) YIELD key",
    ] {
        let plan = plan_read(cypher, &params, &OracleHints)
            .unwrap()
            .unwrap_or_else(|| panic!("expected `{cypher}` to be pushable"));
        assert!(plan.supported_by(&SqliteDialect), "`{cypher}` gated off");
        let actual = run_leaf_sqlite(&conn, &plan, &params);
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
}

/// PUSHDOWN2 P4b: a correlated inner `WHERE` renders into the LEFT JOIN ON
/// clause (numeric cross-scope comparisons under type hints); the inner
/// pipeline — aggregates included — still runs in the reference and must
/// match it exactly, including empty-group and drop-on-empty semantics.
#[tokio::test]
async fn correlated_subquery_pushdown_matches_reference() {
    let graph = fixture();
    let conn = embed(&graph).await;
    let params = CypherParameters::new();
    for cypher in [
        // Correlated aggregate: Grace (85) has an empty inner set -> count 0.
        "MATCH (a:Person) CALL { MATCH (b:Person) WHERE b.age > a.age RETURN count(*) AS older } RETURN a.name, older ORDER BY a.name",
        // Correlated row-producing: empty inner drops the outer row.
        "MATCH (a:Person) CALL { MATCH (b:Person) WHERE b.age > a.age RETURN b.name AS n } RETURN a.name, n ORDER BY a.name, n",
        // Mixed conjunction: the inner-only conjunct rides along in the ON.
        "MATCH (a:Person) CALL { MATCH (b:Person) WHERE b.age > a.age AND b.active = true RETURN count(*) AS n } RETURN a.name, n ORDER BY a.name",
        // Outer-only correlated condition gates the whole inner set per row.
        "MATCH (a:Person) CALL { MATCH (c:City) WHERE a.age >= 41 RETURN c.name AS city } RETURN a.name, city ORDER BY a.name",
        // Inner-tail references to the outer variable are honored by the seeds.
        "MATCH (a:Person) CALL { MATCH (c:City) RETURN c.name AS city, a.name AS me } RETURN me, city ORDER BY me",
    ] {
        let plan = plan_read(cypher, &params, &OracleHints)
            .unwrap()
            .unwrap_or_else(|| panic!("expected `{cypher}` to be pushable"));
        let actual = run_pushdown(&conn, &plan, &params).await;
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
}

/// PUSHDOWN2 P5: endpoint-only `shortestPath` / `allShortestPaths` lower to a
/// recursive walk CTE with per-pair minimal-depth selection (real SQLite —
/// recursive CTEs) and must match the reference exactly: tie multiplicity for
/// `All`, exactly one row per pair for `Single`, shortcut preference, bounds,
/// directions, and multigraph parallel edges.
#[test]
fn shortest_path_pushdown_matches_reference() {
    // Diamond with ties and a shortcut: s -> {m1|m2} -> t (two 2-hop ties),
    // s -> u -> v -> t (a longer route), plus x -> t so an incoming/undirected
    // probe has variety, and parallel edges p -> q (multigraph tie at depth 1).
    let nodes = vec![
        node("N", "s", &[("name", Value::from("S"))]),
        node("N", "m1", &[("name", Value::from("M1"))]),
        node("N", "m2", &[("name", Value::from("M2"))]),
        node("N", "t", &[("name", Value::from("T"))]),
        node("N", "u", &[("name", Value::from("U"))]),
        node("N", "v", &[("name", Value::from("V"))]),
        node("N", "x", &[("name", Value::from("X"))]),
        node("P", "p", &[("name", Value::from("P"))]),
        node("P", "q", &[("name", Value::from("Q"))]),
    ];
    let edges = vec![
        Edge::new("R", "s", "m1", Props::new()),
        Edge::new("R", "s", "m2", Props::new()),
        Edge::new("R", "m1", "t", Props::new()),
        Edge::new("R", "m2", "t", Props::new()),
        Edge::new("R", "s", "u", Props::new()),
        Edge::new("R", "u", "v", Props::new()),
        Edge::new("R", "v", "t", Props::new()),
        Edge::new("R", "x", "t", Props::new()),
        Edge::new("R", "p", "q", Props::new()),
        Edge::new("R", "p", "q", Props::new()),
    ];
    let graph = Graph::new(nodes, edges);
    let conn = embed_sqlite(&graph);
    let params = CypherParameters::new();
    for cypher in [
        // Ties: All keeps both 2-hop paths; Single keeps exactly one.
        "MATCH allShortestPaths((a:N {name:'S'})-[:R*]->(b:N {name:'T'})) RETURN a.name, b.name",
        "MATCH shortestPath((a:N {name:'S'})-[:R*]->(b:N {name:'T'})) RETURN a.name, b.name",
        // Per endpoint pair over an open scan.
        "MATCH shortestPath((a:N {name:'S'})-[:R*]->(b:N)) RETURN b.name",
        "MATCH allShortestPaths((a:N)-[:R*]->(b:N {name:'T'})) RETURN a.name",
        // Bounds: exclude the short routes, force the 3-hop one.
        "MATCH shortestPath((a:N {name:'S'})-[:R*3..]->(b:N {name:'T'})) RETURN a.name, b.name",
        "MATCH allShortestPaths((a:N {name:'S'})-[:R*1..1]->(b:N)) RETURN b.name",
        // Directions.
        "MATCH shortestPath((a:N {name:'T'})<-[:R*]-(b:N)) RETURN b.name",
        "MATCH shortestPath((a:N {name:'X'})-[:R*]-(b:N {name:'S'})) RETURN a.name, b.name",
        // Multigraph parallel edges: two depth-1 ties for All, one for Single.
        "MATCH allShortestPaths((a:P {name:'P'})-[:R*]->(b:P)) RETURN a.name, b.name",
        "MATCH shortestPath((a:P {name:'P'})-[:R*]->(b:P)) RETURN a.name, b.name",
        // Fixed one hop (no `*`), anonymous endpoint, aggregate tail.
        "MATCH shortestPath((a:N {name:'S'})-[:R]->(b:N)) RETURN count(*) AS pairs",
        // Zero-length allowed: start == end when min is 0.
        "MATCH shortestPath((a:N {name:'S'})-[:R*0..1]->(b:N)) RETURN b.name",
    ] {
        let plan = plan_read(cypher, &params, &OracleHints)
            .unwrap()
            .unwrap_or_else(|| panic!("expected `{cypher}` to be pushable"));
        assert!(plan.supported_by(&SqliteDialect), "`{cypher}` gated off");
        let actual = run_leaf_sqlite(&conn, &plan, &params);
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
}
