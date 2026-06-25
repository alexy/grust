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
use grust_cypher::pushdown::{plan_node_read, SqliteDialect};
use grust_cypher::read::run_read_query;
use grust_cypher::CypherParameters;

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
    let edges = vec![Edge::new("KNOWS", "p1", "p2", Props::new())];
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
        assert_eq!(actual, expected, "row mismatch for `{cypher}`");
    }
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
    plan.project(nodes, params).unwrap()
}

/// Build an in-memory SQLite database with a `grust_nodes(id, label, props)`
/// table populated from the graph, props stored as untagged JSON.
async fn embed(graph: &Graph) -> turso::Connection {
    let db = turso::Builder::new_local(":memory:").build().await.unwrap();
    let conn = db.connect().unwrap();
    conn.execute_batch("CREATE TABLE grust_nodes (id TEXT, label TEXT, props TEXT);")
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

/// SQLite string literal (single-quote doubling).
fn lit(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
