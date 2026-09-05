//! Exact scalar predicates are opt-in and independent of legacy row coercions.
//! These tests use embedded databases only, with both tagged and plain JSON.

use grust_core::{Edge, Graph, GraphAdminStore, GraphStore, Node, Props, Value};
use grust_cypher::CypherParameters;
use grust_cypher::pushdown::{NoTypeHints, SqliteDialect, plan_scalar_count_read};
use grust_sql_core::sql_str;
use grust_turso::{TursoGraphStore, TursoReadDialect};

const NEEDLES: &[&str] = &[
    "Comment",
    "7",
    "7.0",
    "7.5",
    "true",
    "[]",
    "[7]",
    "{\"x\": 1}",
    "2026-06-12T09:30:00Z",
    "P1D",
    "",
    "O'Hara\\雪",
];

fn fixture() -> Graph {
    let values = vec![
        Value::Null,
        Value::Bool(true),
        Value::Int(7),
        Value::Float(7.0),
        Value::Float(7.5),
        Value::from("Comment"),
        Value::from("Commentary"),
        Value::from("Comment\0suffix"),
        Value::from("Comment\0"),
        Value::from("7\0"),
        Value::from("7"),
        Value::from("7.5"),
        Value::from("true"),
        Value::from("[]"),
        Value::from("[7]"),
        Value::from("{\"x\": 1}"),
        Value::from(""),
        Value::from("O'Hara\\雪"),
        Value::StringArray(vec![]),
        Value::IntArray(vec![7]),
        Value::FloatArray(vec![7.0]),
        Value::Json(serde_json::json!("Comment")),
        Value::Json(serde_json::json!("Comment\0suffix")),
        Value::Json(serde_json::json!(7)),
        Value::Json(serde_json::json!(true)),
        Value::Json(serde_json::json!({"x": 1})),
        Value::Json(serde_json::json!([])),
        Value::datetime("2026-06-12T09:30:00Z").unwrap(),
        Value::decimal("7").unwrap(),
        Value::duration("P1D").unwrap(),
    ];
    let mut graph = Graph::new(vec![Node::new("Leaf", "leaf", Props::new())], vec![]);
    for (position, value) in values.into_iter().enumerate() {
        let id = format!("n{position}");
        let props: Props = [
            ("kind".into(), value.clone()),
            ("guard".into(), Value::from("yes")),
        ]
        .into_iter()
        .collect();
        graph.nodes.push(Node::new("N", &id, props));
        graph.edges.push(Edge::new(
            "R",
            &id,
            "leaf",
            [("kind".into(), value)].into_iter().collect::<Props>(),
        ));
        graph.edges.push(Edge::new("S", "leaf", &id, Props::new()));
    }
    graph.nodes.push(Node::new("N", "missing", Props::new()));
    graph
}

fn params(needle: &str) -> CypherParameters {
    [("kind".into(), Value::from(needle))].into_iter().collect()
}

#[tokio::test]
async fn tagged_node_edge_and_shared_variable_filters_match_reference() {
    let graph = fixture();
    let store = TursoGraphStore::in_memory().await.unwrap();
    store.bootstrap().await.unwrap();
    store.put_graph(&graph).await.unwrap();
    let dialect = TursoReadDialect::new("grust");
    for needle in NEEDLES {
        let params = params(needle);
        for query in [
            "MATCH (n:N {kind:$kind}) RETURN count(*) AS n",
            "MATCH (n:N) WHERE n.kind = $kind AND n.guard = 'yes' RETURN count(*) AS n",
            "MATCH (:N)-[:R {kind:$kind}]->(:Leaf) RETURN count(*) AS n",
            "MATCH (a:N)-[:R]->(b:Leaf), (b)-[:S]->(a) WHERE a.kind = $kind RETURN count(*) AS n",
        ] {
            let plan = plan_scalar_count_read(query, &params, &NoTypeHints)
                .unwrap()
                .unwrap();
            assert!(plan.supported_by(&dialect));
            let sql = plan.to_sql(&dialect).unwrap();
            assert!(sql.contains("json_type("), "{sql}");
            assert!(sql.contains("COLLATE BINARY"), "{sql}");
            let expected = grust_cypher::read::run_read_query(&graph, query, &params).unwrap();
            assert_eq!(
                store.run_read_query(query, &params).await.unwrap(),
                expected,
                "{needle:?}: {query}"
            );
        }
    }
}

#[tokio::test]
async fn plain_json_string_guard_does_not_coerce_other_json_types() {
    let graph = fixture();
    let db = turso::Builder::new_local(":memory:").build().await.unwrap();
    let conn = db.connect().unwrap();
    conn.execute_batch("CREATE TABLE grust_nodes (id TEXT, label TEXT, props TEXT);")
        .await
        .unwrap();
    for node in &graph.nodes {
        let plain: serde_json::Map<String, serde_json::Value> = node
            .props
            .iter()
            .map(|(key, value)| (key.clone(), value.to_json()))
            .collect();
        conn.execute_batch(&format!(
            "INSERT INTO grust_nodes VALUES ({}, {}, {});",
            sql_str(node.id.as_str()),
            sql_str(node.label.as_str()),
            sql_str(&serde_json::to_string(&plain).unwrap()),
        ))
        .await
        .unwrap();
    }
    for needle in NEEDLES {
        let params = params(needle);
        let query = "MATCH (n:N {kind:$kind}) RETURN count(*) AS n";
        let plan = plan_scalar_count_read(query, &params, &NoTypeHints)
            .unwrap()
            .unwrap();
        let sql = plan.to_sql(&SqliteDialect).unwrap();
        let mut rows = conn.query(&sql, ()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let turso::Value::Integer(count) = row.get_value(0).unwrap() else {
            panic!("not an integer")
        };
        assert!(rows.next().await.unwrap().is_none());
        let expected = grust_cypher::read::run_read_query(&graph, query, &params).unwrap();
        assert_eq!(expected.rows, vec![vec![Value::Int(count)]], "{needle:?}");
    }
}

#[test]
fn nul_literal_is_not_admitted_by_the_dialect() {
    let params = params("not\0sql");
    let plan = plan_scalar_count_read(
        "MATCH (n {kind:$kind}) RETURN count(*)",
        &params,
        &NoTypeHints,
    )
    .unwrap()
    .unwrap();
    let dialect = TursoReadDialect::new("grust");
    assert!(!plan.supported_by(&dialect));
    assert!(plan.to_sql(&dialect).is_err());
}
