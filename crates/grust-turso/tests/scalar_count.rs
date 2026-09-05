//! Native scalar counts versus the unchanged clause-pipeline reference.
//! Everything runs in an embedded in-memory database, without services.

use grust_core::prelude::*;
use grust_cypher::pushdown::{NoTypeHints, plan_scalar_count_read};
use grust_cypher::{CypherParameters, CypherResultTable};
use grust_sql_core::sql_str;
use grust_turso::{TursoConfig, TursoGraphStore, TursoReadDialect};

const COUNTS: &[&str] = &[
    "MATCH (n) RETURN count(*)",
    "MATCH () RETURN count(*) AS c",
    "MATCH (n:A) RETURN count(*) AS c",
    "MATCH (n:Missing) RETURN count(*) AS c",
    "MATCH (a)-[:R]->(b) RETURN count(*) AS c",
    "MATCH (a)<-[:R]-(b) RETURN count(*) AS c",
    "MATCH (a)-[:R]-(b) RETURN count(*) AS c",
    "MATCH (a)-[]-(b) RETURN count(*) AS c",
    "MATCH (a)-[:R]->(a) RETURN count(*) AS c",
    "MATCH (a)-[:R]->(b)-[:S]->(c) RETURN count(*) AS c",
    "MATCH (a)<-[:R]-(b)-[:S]->(c) RETURN count(*) AS c",
    "MATCH (a)-[:R]-(b)-[:S]-(c) RETURN count(*) AS c",
    "MATCH (a)-[:R|S]->(b)-[:T]->(c) RETURN count(*) AS c",
    "MATCH (a)-[:R]->(b), (a)-[:S]->(c) RETURN count(*) AS c",
    "MATCH (a)-[:R]->(b), (b)-[:S]->(a) RETURN count(*) AS c",
    "MATCH (a:A), (b:B) RETURN count(*) AS c",
    "MATCH (), () RETURN count(*) AS c",
    "MATCH (a)-[:R]->(b), (c:B) RETURN count(*) AS c",
    "MATCH (n) RETURN count(*) AS c SKIP 0 LIMIT 1",
    "MATCH (n) RETURN count(*) AS c LIMIT 0",
    "MATCH (n) RETURN count(*) AS c SKIP 1",
];

fn node(label: &str, id: &str, age: i64, name: &str) -> Node {
    let props: Props = [
        ("age".into(), Value::Int(age)),
        ("name".into(), Value::from(name)),
    ]
    .into_iter()
    .collect();
    Node::new(label, id, props)
}

fn fixture() -> Graph {
    Graph::new(
        vec![
            node("A", "a", 1, "O'Hara\\雪"),
            node("A", "b", 2, "Bee"),
            node("B", "c", 3, "Cee"),
        ],
        vec![
            Edge::new("R", "a", "b", Props::new()),
            Edge::new("R", "b", "a", Props::new()),
            Edge::new("R", "a", "a", Props::new()),
            Edge::new("S", "a", "b", Props::new()),
            Edge::new("S", "b", "c", Props::new()),
            Edge::new("T", "c", "a", Props::new()),
        ],
    )
}

async fn store_with(graph: &Graph) -> TursoGraphStore {
    let store = TursoGraphStore::connect(TursoConfig::default())
        .await
        .unwrap();
    store.bootstrap().await.unwrap();
    store.put_graph(graph).await.unwrap();
    store
}

fn reference(graph: &Graph, query: &str, params: &CypherParameters) -> CypherResultTable {
    grust_cypher::read::run_read_query(graph, query, params)
        .unwrap_or_else(|error| panic!("reference failed for {query}: {error}"))
}

#[tokio::test]
async fn real_store_counts_match_reference_including_empty_and_self_loops() {
    let params = CypherParameters::new();
    for graph in [Graph::default(), fixture()] {
        let store = store_with(&graph).await;
        for query in COUNTS {
            assert!(
                plan_scalar_count_read(query, &params, &NoTypeHints)
                    .unwrap()
                    .is_some(),
                "{query}"
            );
            let actual = store.run_read_query(query, &params).await.unwrap();
            assert_eq!(actual, reference(&graph, query, &params), "{query}");
        }
    }
}

#[tokio::test]
async fn real_store_filters_parameters_aliases_and_pagination_match() {
    let graph = fixture();
    let store = store_with(&graph).await;
    let params = [
        ("name".into(), Value::from("O'Hara\\雪")),
        ("age".into(), Value::Int(2)),
        ("skip".into(), Value::Int(0)),
        ("limit".into(), Value::Int(1)),
    ]
    .into_iter()
    .collect();
    for query in [
        "MATCH (n {name: $name}) RETURN COUNT(*) AS `total count`",
        "MATCH (a {name: $name})-[:R]->(b) RETURN count(*) AS c",
        "MATCH (n) RETURN count(*) AS c SKIP $skip LIMIT $limit",
        "MATCH (n) RETURN count(*) AS c SKIP 2 - 1 LIMIT 1 + 1",
    ] {
        let plan = plan_scalar_count_read(query, &params, &NoTypeHints)
            .unwrap()
            .unwrap();
        assert!(
            plan.to_sql(&TursoReadDialect::new("grust"))
                .unwrap()
                .starts_with("SELECT COUNT(*)")
        );
        assert_eq!(
            store.run_read_query(query, &params).await.unwrap(),
            reference(&graph, query, &params),
            "{query}"
        );
    }
    for suffix in [
        " SKIP -1",
        " LIMIT 'bad'",
        " SKIP $missing",
        " SKIP 1 LIMIT $missing",
    ] {
        let query = format!("MATCH (n) RETURN count(*) AS c{suffix}");
        let expected = grust_cypher::read::run_read_query(&graph, &query, &params).unwrap_err();
        let actual = store.run_read_query(&query, &params).await.unwrap_err();
        assert_eq!(actual.to_string(), expected.to_string(), "{query}");
    }
}

#[tokio::test]
async fn unproven_counts_keep_existing_execution_paths() {
    let graph = fixture();
    let store = store_with(&graph).await;
    let params = CypherParameters::new();
    for query in [
        "MATCH (n) WHERE n.age >= 2 RETURN count(*) AS c",
        "MATCH (n) WHERE n.missing IS NULL RETURN count(*) AS c",
        "MATCH (n) WHERE n.missing IS NOT NULL RETURN count(*) AS c",
        "MATCH (a)-[:R]->(b) WHERE a.age >= 2 RETURN count(*) AS c",
        "MATCH (n) WHERE n.age IN [1, 3] RETURN count(*) AS c",
        "MATCH (a)-[:R]->(b)-[:R]->(c) RETURN count(*) AS c",
        "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*) AS c",
        "MATCH (a) MATCH (b) RETURN count(*) AS c",
        "MATCH (a)-[:R]->(b) WHERE a.age = b.age RETURN count(*) AS c",
        "MATCH (a) RETURN count(a) AS c",
        "MATCH (a) RETURN count(DISTINCT a) AS c",
        "MATCH (a) RETURN count(*) AS c ORDER BY c",
        "MATCH (a) RETURN count(*) AS c UNION MATCH (a) RETURN count(*) AS c",
        "MATCH (a) WITH a RETURN count(*) AS c",
    ] {
        assert!(
            plan_scalar_count_read(query, &params, &NoTypeHints)
                .unwrap()
                .is_none(),
            "{query}"
        );
        assert_eq!(
            store.run_read_query(query, &params).await.unwrap(),
            reference(&graph, query, &params),
            "{query}"
        );
    }
}

// An unconstrained mirror proves SQL bag multiplicity independently of the
// universal store's endpoint/type primary key. Duplicate physical edge rows
// must contribute individually to COUNT(*), even with absent/equal edge IDs.
async fn mirror(graph: &Graph) -> (turso::Database, turso::Connection) {
    let db = turso::Builder::new_local(":memory:").build().await.unwrap();
    let conn = db.connect().unwrap();
    conn.execute_batch(
        "CREATE TABLE grust_nodes (id TEXT, label TEXT, props TEXT); \
         CREATE TABLE grust_edges (id TEXT, from_id TEXT, to_id TEXT, label TEXT, props TEXT);",
    )
    .await
    .unwrap();
    for node in &graph.nodes {
        conn.execute_batch(&format!(
            "INSERT INTO grust_nodes VALUES ({}, {}, {});",
            sql_str(node.id.as_str()),
            sql_str(node.label.as_str()),
            sql_str(&serde_json::to_string(&node.props).unwrap()),
        ))
        .await
        .unwrap();
    }
    for edge in &graph.edges {
        conn.execute_batch(&format!(
            "INSERT INTO grust_edges VALUES (NULL, {}, {}, {}, {});",
            sql_str(edge.from.as_str()),
            sql_str(edge.to.as_str()),
            sql_str(edge.label.as_str()),
            sql_str(&serde_json::to_string(&edge.props).unwrap()),
        ))
        .await
        .unwrap();
    }
    (db, conn)
}

async fn native_count(conn: &turso::Connection, query: &str) -> CypherResultTable {
    let params = CypherParameters::new();
    let plan = plan_scalar_count_read(query, &params, &NoTypeHints)
        .unwrap()
        .unwrap();
    let sql = plan.to_sql(&TursoReadDialect::new("grust")).unwrap();
    let mut rows = conn.query(&sql, ()).await.unwrap();
    let row = rows
        .next()
        .await
        .unwrap()
        .expect("COUNT emits one row even on empty input");
    let turso::Value::Integer(count) = row.get_value(0).unwrap() else {
        panic!("native COUNT did not emit an integer: {sql}");
    };
    assert!(
        rows.next().await.unwrap().is_none(),
        "native COUNT transported match rows: {sql}"
    );
    plan.project_text_rows(vec![vec![Some(count.to_string())]], &params)
        .unwrap()
}

#[tokio::test]
async fn parallel_edges_preserve_bag_multiplicity_without_deduplication() {
    let mut graph = fixture();
    graph.edges.push(graph.edges[0].clone());
    graph.edges.push(graph.edges[2].clone()); // parallel self-loop
    let (_db, conn) = mirror(&graph).await;
    for query in COUNTS {
        assert_eq!(
            native_count(&conn, query).await,
            reference(&graph, query, &CypherParameters::new()),
            "{query}"
        );
    }
    assert_eq!(
        native_count(&conn, "MATCH (a)-[:R]-(b) RETURN count(*) AS c")
            .await
            .rows,
        vec![vec![Value::Int(8)]]
    );
}

#[tokio::test]
async fn deterministic_small_graph_differential() {
    let mut seed = 0x5eed_u64;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        seed >> 32
    };
    for size in 0..=5 {
        for _ in 0..3 {
            let nodes: Vec<_> = (0..size)
                .map(|index| {
                    node(
                        if index % 2 == 0 { "A" } else { "B" },
                        &format!("n{index}"),
                        index as i64,
                        "name",
                    )
                })
                .collect();
            let mut edges = vec![];
            for from in 0..size {
                for to in 0..size {
                    for edge_type in ["R", "S", "T"] {
                        // Includes parallel edges, cycles, and self-loops.
                        for _ in 0..(next() % 3) {
                            edges.push(Edge::new(
                                edge_type,
                                format!("n{from}"),
                                format!("n{to}"),
                                Props::new(),
                            ));
                        }
                    }
                }
            }
            let graph = Graph::new(nodes, edges);
            let (_db, conn) = mirror(&graph).await;
            for query in COUNTS {
                assert_eq!(
                    native_count(&conn, query).await,
                    reference(&graph, query, &CypherParameters::new()),
                    "size={size}: {query}"
                );
            }
        }
    }
}
