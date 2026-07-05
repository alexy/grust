//! Store-level differential test for `TursoGraphStore::run_read_query`.
//!
//! Unlike the oracle (which exercises the `SqliteDialect` against a mirror
//! schema), this runs the **real** store end-to-end: tagged-JSON props
//! written through `put_node`/`put_edge`, the `TursoReadDialect`
//! (`$.key.value` extraction, `from_id`/`to_id`/`label` edge columns), and
//! the reference fallback for shapes the embedded engine cannot push. Every
//! query must produce exactly what the Memory reference produces over the
//! same graph.

use grust_core::prelude::*;
use grust_cypher::CypherParameters;
use grust_cypher::read::run_read_query;
use grust_turso::{TursoConfig, TursoGraphStore};

fn node(label: &str, id: &str, props: &[(&str, Value)]) -> Node {
    let mut p = Props::new();
    for (k, v) in props {
        p.insert((*k).to_string(), v.clone());
    }
    Node::new(label, id, p)
}

fn fixture() -> Graph {
    let nodes = vec![
        node(
            "Person",
            "p1",
            &[
                ("name", Value::from("Ada")),
                ("age", Value::Int(36)),
                ("active", Value::Bool(true)),
            ],
        ),
        node(
            "Person",
            "p2",
            &[
                ("name", Value::from("Alan")),
                ("age", Value::Int(41)),
                ("active", Value::Bool(false)),
            ],
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

async fn store_with(graph: &Graph) -> TursoGraphStore {
    let store = TursoGraphStore::connect(TursoConfig::default())
        .await
        .expect("in-memory Turso store");
    store.bootstrap().await.expect("bootstrap tables");
    for n in &graph.nodes {
        store.put_node(n).await.expect("put_node");
    }
    for e in &graph.edges {
        store.put_edge(e).await.expect("put_edge");
    }
    store
}

/// Compare as row multisets (Cypher results are unordered without ORDER BY).
fn assert_same(
    cypher: &str,
    actual: &grust_cypher::CypherResultTable,
    expected: &grust_cypher::CypherResultTable,
) {
    assert_eq!(actual.columns, expected.columns, "columns for `{cypher}`");
    let key = |rows: &[Vec<Value>]| {
        let mut keys: Vec<String> = rows.iter().map(|r| format!("{r:?}")).collect();
        keys.sort();
        keys
    };
    assert_eq!(
        key(&actual.rows),
        key(&expected.rows),
        "row multiset for `{cypher}`"
    );
}

#[tokio::test]
async fn pushed_shapes_match_the_reference() {
    let graph = fixture();
    let store = store_with(&graph).await;
    let params = CypherParameters::new();
    for cypher in [
        // Node scans: string / int / bool predicates over tagged props.
        "MATCH (n:Person) RETURN n.name",
        "MATCH (n:Person {name: 'Ada'}) RETURN n.age",
        "MATCH (n:Person) WHERE n.age >= 40 RETURN n.name ORDER BY n.name",
        "MATCH (n:Person) WHERE n.active = true RETURN n.name",
        "MATCH (n:Person) WHERE n.name STARTS WITH 'A' RETURN n.name ORDER BY n.name",
        // Fixed segments over from_id/to_id/label columns.
        "MATCH (a:Person {name:'Ada'})-[:KNOWS]->(b:Person) RETURN b.name",
        "MATCH (a:Person)<-[:KNOWS]-(b:Person {name:'Ada'}) RETURN a.name",
        "MATCH (a:Person {name:'Ada'})-[r]->(b) RETURN b.name ORDER BY b.name",
        // OPTIONAL MATCH null padding.
        "MATCH (a:Person) OPTIONAL MATCH (a)-[:KNOWS]->(b) RETURN a.name, b.name ORDER BY a.name",
        // Multi-pattern join.
        "MATCH (a:Person)-[:KNOWS]->(b), (a)-[:LIVES_IN]->(c) RETURN a.name, b.name, c.name",
        // UNION.
        "MATCH (n:Person) RETURN n.name AS x UNION MATCH (c:City) RETURN c.name AS x",
        // WITH pipeline over a pushed scan.
        "MATCH (n:Person) WITH n.age AS age WHERE age >= 40 RETURN age ORDER BY age",
        // Uncorrelated subquery join.
        "MATCH (a:Person) CALL { MATCH (c:City) RETURN c.name AS city } RETURN a.name, city ORDER BY a.name",
        // Catalog procedures as DISTINCT scans.
        "CALL db.labels()",
        "CALL db.relationshipTypes() YIELD relationshipType AS t RETURN t ORDER BY t",
    ] {
        let actual = store.run_read_query(cypher, &params).await.unwrap();
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
}

#[tokio::test]
async fn gated_shapes_fall_back_to_the_reference() {
    let graph = fixture();
    let store = store_with(&graph).await;
    let params = CypherParameters::new();
    for cypher in [
        // Recursive CTEs are unsupported in the embedded engine.
        "MATCH (a:Person {name:'Ada'})-[:KNOWS*1..2]->(b) RETURN b.name ORDER BY b.name",
        "MATCH shortestPath((a:Person {name:'Ada'})-[:KNOWS*]->(b:Person)) RETURN b.name",
        "CALL tvf.range(1, 3) YIELD value RETURN value",
        // json_each is unavailable: propertyKeys and correlated tvf.keys.
        "CALL db.propertyKeys()",
        "MATCH (n:Person {name:'Ada'}) CALL tvf.keys(n) YIELD key RETURN key ORDER BY key",
        // Path values and correlated pattern subqueries are reference-only.
        "MATCH p = (:Person {name:'Ada'})-[:KNOWS]->(b) RETURN length(p)",
        "MATCH (a:Person) CALL { MATCH (a)-[:KNOWS]->(b) RETURN b.name AS f } RETURN a.name, f",
    ] {
        let actual = store.run_read_query(cypher, &params).await.unwrap();
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
}
