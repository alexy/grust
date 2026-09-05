//! SQL row identity is not a nullable public EdgeId. The portable planner must
//! decline overlapping type positions until physical identity is guaranteed.

use grust_core::{Edge, Graph, Node, Props, Value};
use grust_cypher::CypherParameters;
use grust_cypher::pushdown::{NoTypeHints, plan_read, plan_scalar_count_read, plan_segment_read};
use grust_sql_core::sql_str;

const CHAIN: &str = "MATCH (a)-[:R]->(b)<-[:R]-(c) RETURN count(*) AS n";
const COMMA: &str = "MATCH (a)-[:R]->(b), (c)-[:R]->(d) RETURN count(*) AS n";

async fn scalar(conn: &turso::Connection, sql: &str) -> i64 {
    let mut rows = conn.query(sql, ()).await.unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let turso::Value::Integer(value) = row.get_value(0).unwrap() else {
        panic!("expected SQL count integer");
    };
    assert!(rows.next().await.unwrap().is_none());
    value
}

#[tokio::test]
async fn embedded_counterexamples_preserve_physical_parallel_edges_not_public_ids() {
    // A single physical edge has zero two-position matches; two parallel
    // physical edges have exactly two, whether IDs are null, equal, or distinct.
    for ids in [
        vec![None],
        vec![None, None],
        vec![Some("same"), Some("same")],
        vec![Some("one"), Some("two")],
    ] {
        let edges: Vec<_> = ids
            .iter()
            .map(|id| {
                let edge = Edge::new("R", "a", "b", Props::new());
                match id {
                    Some(id) => edge.with_id(*id),
                    None => edge,
                }
            })
            .collect();
        let graph = Graph::new(
            vec![
                Node::new("N", "a", Props::new()),
                Node::new("N", "b", Props::new()),
            ],
            edges,
        );
        let db = turso::Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute_batch("CREATE TABLE edges (id TEXT, source TEXT, target TEXT, kind TEXT);")
            .await
            .unwrap();
        for id in &ids {
            conn.execute_batch(&format!(
                "INSERT INTO edges VALUES ({}, 'a', 'b', 'R');",
                id.map(sql_str).unwrap_or_else(|| "NULL".into())
            ))
            .await
            .unwrap();
        }
        let count = ids.len() as i64;
        let expected = count * (count - 1);
        // This reproduces the old two-alias row source's invalid self-pairs.
        let old_join = "SELECT COUNT(*) FROM edges e0 JOIN edges e1 ON e0.target=e1.target WHERE e0.kind='R' AND e1.kind='R'";
        assert_eq!(scalar(&conn, old_join).await, count * count);
        // SQLite rowid is a valid independent oracle here, but is deliberately
        // not assumed by the portable production dialect/Sail contract.
        assert_eq!(
            scalar(&conn, &format!("{old_join} AND e0.rowid<>e1.rowid")).await,
            expected
        );
        if ids[0].is_none() || (ids.len() == 2 && ids[0] == ids[1]) {
            assert_eq!(
                scalar(&conn, &format!("{old_join} AND e0.id<>e1.id")).await,
                0
            );
        }
        for source in [CHAIN, COMMA] {
            let params = CypherParameters::new();
            assert!(plan_segment_read(source, &params).unwrap().is_none());
            assert!(plan_read(source, &params, &NoTypeHints).unwrap().is_none());
            assert!(
                plan_scalar_count_read(source, &params, &NoTypeHints)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                grust_cypher::read::run_read_query(&graph, source, &params)
                    .unwrap()
                    .rows,
                vec![vec![Value::Int(expected)]]
            );
        }
    }
}

#[tokio::test]
async fn separate_match_clauses_reset_relationship_uniqueness() {
    use grust_core::{GraphAdminStore, GraphStore};
    let graph = Graph::new(
        vec![
            Node::new("N", "a", Props::new()),
            Node::new("N", "b", Props::new()),
        ],
        vec![Edge::new("R", "a", "b", Props::new())],
    );
    let store = grust_turso::TursoGraphStore::in_memory().await.unwrap();
    store.bootstrap().await.unwrap();
    store.put_graph(&graph).await.unwrap();
    for source in [
        "MATCH (a)-[:R]->(b) MATCH (a)-[:R]->(b) RETURN count(*) AS n",
        "MATCH (a)-[:R]->(b) OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*) AS n",
    ] {
        let expected =
            grust_cypher::read::run_read_query(&graph, source, &CypherParameters::new()).unwrap();
        assert_eq!(expected.rows, vec![vec![Value::Int(1)]]);
        assert_eq!(
            store
                .run_read_query(source, &CypherParameters::new())
                .await
                .unwrap(),
            expected
        );
    }
}
