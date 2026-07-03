//! Golden snapshots of the strict-write planner (Unit 10a safety net).
//!
//! Before the legacy `cypher_*` write entrypoints are rewired onto the new
//! lexer/parser/semantics pipeline, this pins the *current* behavior: for a
//! corpus of write statements, the serialized `GraphMutationPlan` (or the error)
//! is captured in `tests/golden/write_golden.json`. The compare test then guards
//! that the planner's output stays byte-identical across the rewiring — any
//! drift fails here and the rewiring must be aborted/fixed.
//!
//! To (re)generate the golden after an *intended* change, run:
//!   cargo test -p grust-cypher --test write_golden -- --ignored regenerate
//! and review the diff before committing.

use grust_cypher::cypher_mutation_plan;

/// Representative strict-write statements (explicit-id, the planner default).
const CORPUS: &[&str] = &[
    "CREATE (n:Person {id: 'p1', name: 'Ada', age: 36})",
    "CREATE (n:Person {id: 'p1'})",
    "MERGE (n:Person {id: 'p1'})",
    "MERGE (n:Person {id: 'p1', name: 'Ada'})",
    "MATCH (n:Person {id: 'p1'}) SET n.age = 37",
    "MATCH (n:Person {id: 'p1'}) SET n.age = 37, n.name = 'Ada2'",
    "MATCH (n:Person {id: 'p1'}) SET n += {city: 'London'}",
    "MATCH (n:Person {id: 'p1'}) SET n = {id: 'p1', name: 'Ada'}",
    "MATCH (n:Person {id: 'p1'}) REMOVE n.age",
    "MATCH (n:Person {id: 'p1'}) DELETE n",
    "MATCH (n:Person {id: 'p1'}) DETACH DELETE n",
    "MATCH (a:Person {id: 'p1'}), (b:Person {id: 'p2'}) CREATE (a)-[:KNOWS]->(b)",
    "MATCH (a:Person {id: 'p1'}), (b:Person {id: 'p2'}) MERGE (a)-[:KNOWS]->(b)",
    "CREATE (a:Person {id: 'p1'})-[:KNOWS {since: 2020}]->(b:Person {id: 'p2'})",
    "MATCH (a:Person {id: 'p1'})-[r:KNOWS]->(b) DELETE r",
    "MATCH (n:Person {id: 'p1'}) SET n.age = 37 RETURN n.age",
    "MATCH (n:Person {id: 'p1'}) RETURN n.name",
    "MATCH (n:Person) WHERE n.age > 30 SET n.adult = true",
    "MATCH (n:Person {id: 'p1'}) REMOVE n:Person",
    "MATCH (n:Person {id: 'p1'}) SET n:Admin",
];

/// Snapshot one statement's planner outcome as a stable JSON value
/// (`{cypher, ok: <plan>|null, err: <msg>|null}`).
fn snapshot(cypher: &str) -> serde_json::Value {
    match cypher_mutation_plan(cypher) {
        Ok(plan) => serde_json::json!({
            "cypher": cypher,
            "ok": serde_json::to_value(&plan).unwrap(),
            "err": serde_json::Value::Null,
        }),
        Err(e) => serde_json::json!({
            "cypher": cypher,
            "ok": serde_json::Value::Null,
            "err": e.to_string(),
        }),
    }
}

fn current() -> serde_json::Value {
    serde_json::Value::Array(CORPUS.iter().map(|c| snapshot(c)).collect())
}

const GOLDEN_PATH: &str = "tests/golden/write_golden.json";

#[test]
fn write_planner_matches_golden() {
    let golden = std::fs::read_to_string(GOLDEN_PATH).unwrap_or_else(|e| {
        panic!("missing {GOLDEN_PATH} ({e}); regenerate with `--ignored regenerate`")
    });
    let golden: serde_json::Value = serde_json::from_str(&golden).expect("golden JSON parses");
    assert_eq!(
        current(),
        golden,
        "strict-write planner output drifted from the golden snapshot"
    );
}

/// Run with `--ignored regenerate` to (re)write the golden file.
#[test]
#[ignore = "regenerates the golden snapshot file"]
fn regenerate() {
    let json = serde_json::to_string_pretty(&current()).unwrap();
    std::fs::write(GOLDEN_PATH, json + "\n").expect("write golden");
}
