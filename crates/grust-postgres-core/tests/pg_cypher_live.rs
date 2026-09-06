//! Live differential suite for the PostgreSQL Cypher executor
//! (`docs/GQL_POSTGRES_EXECUTOR_GOAL.md` Q4).
//!
//! Requires a reachable PostgreSQL (`GRUST_PG_URL`, default
//! `host=127.0.0.1 user=postgres dbname=grust_test`), so every test is
//! `#[ignore]`d — run with `cargo test -p grust-postgres-core -- --ignored`.
//! Reads assert **row-multiset equality against the Memory reference** over
//! the same graph (the same oracle discipline as `grust-turso`); writes
//! assert store state after executing planned Cypher mutations and atomic
//! transaction scripts.

use grust_core::prelude::*;
use grust_cypher::read::run_read_query;
use grust_cypher::{
    CypherMutationOptions, CypherParameters, CypherResultTable,
    run_cypher_transaction_script_on_store, sail_cypher_mutation_plan_with_options,
};
use grust_postgres_core::{PostgresGraphConfig, PostgresGraphStore};

fn pg_url() -> String {
    std::env::var("GRUST_PG_URL")
        .unwrap_or_else(|_| "host=127.0.0.1 user=postgres dbname=grust_test".to_string())
}

/// A fresh store on an isolated table prefix (tests run concurrently).
async fn store(prefix: &str) -> PostgresGraphStore {
    let store = PostgresGraphStore::connect(PostgresGraphConfig {
        connection_string: pg_url(),
        schema: "public".to_string(),
        table_prefix: prefix.to_string(),
        batch_size: 500,
    })
    .await
    .expect("live PostgreSQL (set GRUST_PG_URL)");
    store.bootstrap().await.expect("bootstrap");
    store.clear().await.expect("clear");
    store
}

fn node(label: &str, id: &str, props: &[(&str, Value)]) -> Node {
    let mut p = Props::new();
    for (k, v) in props {
        p.insert((*k).to_string(), v.clone());
    }
    Node::new(label, id, p)
}

/// The shared read fixture: mixed types, missing props, an apostrophe name.
fn fixture() -> Graph {
    let nodes = vec![
        node(
            "Person",
            "p1",
            &[
                ("name", Value::from("Ada")),
                ("age", Value::Int(36)),
                ("active", Value::Bool(true)),
                ("score", Value::Float(9.5)),
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
        node(
            "Person",
            "p4",
            &[("name", Value::from("O'Hara")), ("age", Value::Int(50))],
        ),
        node("City", "c1", &[("name", Value::from("London"))]),
    ];
    let edges = vec![
        Edge::new("KNOWS", "p1", "p2", Props::new()),
        Edge::new("KNOWS", "p2", "p3", Props::new()),
        Edge::new("KNOWS", "p1", "p4", Props::new()),
        Edge::new("FOLLOWS", "p3", "p1", Props::new()),
        Edge::new("LIVES_IN", "p1", "c1", Props::new()),
    ];
    Graph::new(nodes, edges)
}

async fn store_with(prefix: &str, graph: &Graph) -> PostgresGraphStore {
    let store = store(prefix).await;
    store.put_graph(graph).await.expect("put_graph");
    store
}

fn assert_same(cypher: &str, actual: &CypherResultTable, expected: &CypherResultTable) {
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
#[ignore = "requires live PostgreSQL (GRUST_PG_URL)"]
async fn pushed_reads_match_the_reference() {
    let graph = fixture();
    let store = store_with("pgread", &graph).await;
    let params = CypherParameters::new();
    for cypher in [
        // Node scans: string / int / float / bool predicates over tagged jsonb.
        "MATCH (n:Person) RETURN n.name",
        "MATCH (n:Person {name: 'Ada'}) RETURN n.age",
        "MATCH (n:Person {name: \"O'Hara\"}) RETURN n.age",
        "MATCH (n:Person) WHERE n.age >= 41 RETURN n.name ORDER BY n.name",
        "MATCH (n:Person) WHERE n.score > 9.0 RETURN n.name",
        "MATCH (n:Person) WHERE n.active = true RETURN n.name",
        "MATCH (n:Person) WHERE n.name STARTS WITH 'A' RETURN n.name ORDER BY n.name",
        "MATCH (n:Person) WHERE n.name CONTAINS 'ra' RETURN n.name ORDER BY n.name",
        "MATCH (n:Person) WHERE n.name ENDS WITH 'a' RETURN n.name ORDER BY n.name",
        "MATCH (n:Person) WHERE n.city IS NULL RETURN n.name",
        "MATCH (n:Person) WHERE n.age IN [36, 85] RETURN n.name",
        // Fixed segments over from_id/to_id/label edge columns.
        "MATCH (a:Person {name:'Ada'})-[:KNOWS]->(b:Person) RETURN b.name ORDER BY b.name",
        "MATCH (a:Person)<-[:KNOWS]-(b:Person {name:'Ada'}) RETURN a.name ORDER BY a.name",
        "MATCH (a:Person {name:'Grace'})-[:FOLLOWS]-(b) RETURN b.name",
        "MATCH (a)-[r:KNOWS]->(b) RETURN a.name, b.name ORDER BY a.name, b.name",
        // OPTIONAL MATCH null padding.
        "MATCH (a:Person) OPTIONAL MATCH (a)-[:KNOWS]->(b) RETURN a.name, b.name ORDER BY a.name",
        // Multi-pattern join.
        "MATCH (a:Person)-[:KNOWS]->(b), (a)-[:LIVES_IN]->(c) RETURN a.name, b.name, c.name",
        // UNION.
        "MATCH (n:Person) RETURN n.name AS x UNION MATCH (c:City) RETURN c.name AS x",
        // WITH pipeline + aggregates over a pushed scan.
        "MATCH (n:Person) WITH n.age AS age WHERE age >= 41 RETURN age ORDER BY age",
        "MATCH (n) RETURN n.label AS label, count(*) AS c ORDER BY label",
        // Variable-length via WITH RECURSIVE — the first non-SQLite engine.
        "MATCH (a:Person {name:'Ada'})-[:KNOWS*1..2]->(b) RETURN b.name ORDER BY b.name",
        "MATCH (a:Person {name:'Ada'})-[:KNOWS*2..2]->(b) RETURN b.name",
        // Subqueries: uncorrelated join + correlated WHERE in the JOIN ON is
        // hint-gated (falls back under NoTypeHints) — the uncorrelated forms push.
        "MATCH (a:Person) CALL { MATCH (c:City) RETURN c.name AS city } RETURN a.name, city ORDER BY a.name",
        "CALL { MATCH (p:Person) WITH p.name AS n WHERE n STARTS WITH 'A' RETURN n } RETURN n ORDER BY n",
        // Catalog procedures + tvf.range via generate_series.
        "CALL db.labels()",
        "CALL db.relationshipTypes() YIELD relationshipType AS t RETURN t ORDER BY t",
        "CALL db.propertyKeys()",
        "CALL tvf.range(1, 4) YIELD value RETURN sum(value) AS total",
        "CALL tvf.range(5, 1, -2) YIELD value RETURN value",
    ] {
        let actual = store.run_read_query(cypher, &params).await.unwrap();
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
}

#[tokio::test]
#[ignore = "requires live PostgreSQL (GRUST_PG_URL)"]
async fn gated_reads_fall_back_to_the_reference() {
    let graph = fixture();
    let store = store_with("pgfall", &graph).await;
    let params = CypherParameters::new();
    for cypher in [
        // Shortest paths: no insertion-ordered rowid — reference fallback.
        "MATCH shortestPath((a:Person {name:'Ada'})-[:KNOWS*]->(b:Person)) RETURN b.name ORDER BY b.name",
        "MATCH p = shortestPath((a:Person {name:'Ada'})-[:KNOWS*]->(b:Person {name:'Grace'})) RETURN length(p)",
        // Correlated tvf.keys: jsonb_object_keys order is not sorted — fallback.
        "MATCH (n:Person {name:'Ada'}) CALL tvf.keys(n) YIELD key RETURN key ORDER BY key",
        // Path values and correlated pattern subqueries are reference-only.
        "MATCH p = (:Person {name:'Ada'})-[:KNOWS]->(b) RETURN length(p)",
        "MATCH (a:Person) CALL { MATCH (a)-[:KNOWS]->(b) RETURN b.name AS f } RETURN a.name, f ORDER BY a.name, f",
        // Graph values, CASE buckets — arbitrary reference shapes still work.
        "MATCH (a:Person)-[r:KNOWS]->(b) WITH collect(a) AS ns, collect(r) AS rs RETURN size(nodes(graph(ns, rs))) AS n",
        "MATCH (n:Person) RETURN CASE WHEN n.age >= 80 THEN 'senior' ELSE 'other' END AS bucket, count(*) AS c ORDER BY bucket",
    ] {
        let actual = store.run_read_query(cypher, &params).await.unwrap();
        let expected = run_read_query(&graph, cypher, &params).unwrap();
        assert_same(cypher, &actual, &expected);
    }
}

#[tokio::test]
#[ignore = "requires live PostgreSQL (GRUST_PG_URL)"]
async fn cypher_writes_execute_on_postgres() {
    let store = store("pgwrite").await;

    async fn run_write(store: &PostgresGraphStore, cypher: &str) {
        let (plan, _) =
            sail_cypher_mutation_plan_with_options(cypher, CypherMutationOptions::default())
                .unwrap_or_else(|e| panic!("plan failed: {e}\n  {cypher}"));
        store
            .execute_cypher_mutation_plan(&plan)
            .await
            .unwrap_or_else(|e| panic!("write failed: {e}\n  {cypher}"));
    }

    // Resolved creates, then matched relationship + property writes, then a
    // broad matched patch (the bounded matched-node support).
    for cypher in [
        "CREATE (:Person {id: 'p1', name: 'Ada', age: 36})",
        "CREATE (:Person {id: 'p2', name: 'Alan', age: 41})",
        // MERGE is an upsert: it must introduce a NEW node here (an existing
        // id would have its props replaced — the contract on every backend).
        "MERGE (:Person {id: 'p3', name: 'Grace'})",
        "MATCH (a:Person {id: 'p1'}), (b:Person {id: 'p2'}) CREATE (a)-[:KNOWS]->(b)",
        "MATCH (n:Person {id: 'p2'}) SET n.age = 42",
        "MATCH (n:Person) SET n.checked = true",
    ] {
        run_write(&store, cypher).await;
    }

    // Verify through the read path (pushdown over the same store).
    let table = store
        .run_read_query(
            "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name, b.age",
            &CypherParameters::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        table.rows,
        vec![vec![
            Value::from("Ada"),
            Value::from("Alan"),
            Value::Int(42)
        ]]
    );
    let checked = store
        .run_read_query(
            "MATCH (n:Person) WHERE n.checked = true RETURN count(*) AS c",
            &CypherParameters::new(),
        )
        .await
        .unwrap();
    assert_eq!(checked.rows, vec![vec![Value::Int(3)]]);

    // Strict-write rejections stay structured (at planning, before the store).
    let err = sail_cypher_mutation_plan_with_options(
        "CREATE (:Person {name: 'no id'})",
        CypherMutationOptions::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("explicit"), "unexpected: {err}");
}

#[tokio::test]
#[ignore = "requires live PostgreSQL (GRUST_PG_URL)"]
async fn transaction_scripts_commit_and_roll_back_atomically() {
    let store = store("pgtxn").await;

    // COMMIT applies the batch in one apply_mutations call (Transactional).
    let report = run_cypher_transaction_script_on_store(
        &store,
        "BEGIN; \
         CREATE (:Person {id: 'p1', name: 'Ada'}); \
         CREATE (:Person {id: 'p2', name: 'Alan'}); \
         COMMIT",
        CypherMutationOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(report.node_upserts, 2);
    assert!(store.get_node(&NodeId::from("p1")).await.unwrap().is_some());

    // ROLLBACK never touches the store.
    run_cypher_transaction_script_on_store(
        &store,
        "BEGIN; CREATE (:Person {id: 'p9'}); ROLLBACK",
        CypherMutationOptions::default(),
    )
    .await
    .unwrap();
    assert!(store.get_node(&NodeId::from("p9")).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "requires live PostgreSQL (GRUST_PG_URL)"]
async fn resident_snapshot_reflects_the_store_and_every_write_drops_it() {
    let store = store_with("pgsnap", &fixture()).await;
    let first = store.indexed_snapshot().await.unwrap();
    let again = store.indexed_snapshot().await.unwrap();
    assert!(
        std::sync::Arc::ptr_eq(&first, &again),
        "an unchanged store shares one snapshot"
    );
    let count = |index: &grust_core::TypedGraphIndex, cypher: &str| {
        let table =
            grust_cypher::read::run_read_query_indexed(index, cypher, &CypherParameters::new())
                .unwrap();
        match table.rows[0].as_slice() {
            [Value::Int(count)] => *count,
            other => panic!("expected one integer, got {other:?}"),
        }
    };
    let people = count(&first, "MATCH (n:Person) RETURN count(n)");
    assert_eq!(
        people,
        store
            .read_graph()
            .await
            .unwrap()
            .nodes
            .iter()
            .filter(|node| node.label.as_str() == "Person")
            .count() as i64
    );

    store
        .put_node(&node("Person", "p-new", &[("name", Value::from("New"))]))
        .await
        .unwrap();
    let after_write = store.indexed_snapshot().await.unwrap();
    assert!(
        !std::sync::Arc::ptr_eq(&first, &after_write),
        "a write drops the snapshot"
    );
    assert_eq!(
        count(&after_write, "MATCH (n:Person) RETURN count(n)"),
        people + 1
    );

    // A whole-store clear is a write like any other.
    store.clear().await.unwrap();
    let after_clear = store.indexed_snapshot().await.unwrap();
    assert_eq!(count(&after_clear, "MATCH (n) RETURN count(n)"), 0);
}
