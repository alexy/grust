//! `grust-beads [issues.jsonl]` — load a beads `bd export` JSONL file into a
//! grust property graph and report on it, including a grust Cypher query.
//!
//! With no argument it loads the bundled `sample-issues.jsonl` fixture so the
//! example runs standalone:
//!
//! ```sh
//! cargo run -p grust-beads
//! cargo run -p grust-beads -- path/to/your-export.jsonl
//! ```

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;

use grust_beads::load_jsonl;
use grust_cypher::read::run_read_query;
use grust_cypher::CypherParameters;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/sample-issues.jsonl").to_string());
    let graph = load_jsonl(BufReader::new(File::open(&path)?))?;

    println!("beads graph from {path}:");
    println!("  issues (nodes):       {}", graph.nodes.len());
    println!("  dependencies (edges): {}", graph.edges.len());

    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    for e in &graph.edges {
        *by_type.entry(e.label.as_str().to_string()).or_default() += 1;
    }
    if !by_type.is_empty() {
        println!("  edges by type:");
        for (t, c) in &by_type {
            println!("    {t}: {c}");
        }
    }

    // grust is the graph: query the issue graph with Cypher.
    let params = CypherParameters::new();
    let by_status =
        "MATCH (n:Issue) RETURN n.status AS status, count(*) AS count ORDER BY count DESC";
    match run_read_query(&graph, by_status, &params) {
        Ok(table) => {
            println!("  issues by status (grust Cypher):");
            for row in &table.rows {
                println!(
                    "    {}",
                    row.iter().map(value_str).collect::<Vec<_>>().join("\t")
                );
            }
        }
        Err(e) => eprintln!("  status query failed: {e}"),
    }

    let blocks = "MATCH (a:Issue)-[:BLOCKS]->(b:Issue) RETURN a.id AS issue, b.id AS depends_on";
    if let Ok(table) = run_read_query(&graph, blocks, &params) {
        if !table.rows.is_empty() {
            println!("  blocking dependencies (grust Cypher):");
            for row in &table.rows {
                println!(
                    "    {}",
                    row.iter().map(value_str).collect::<Vec<_>>().join(" -> ")
                );
            }
        }
    }

    Ok(())
}

fn value_str(v: &grust_core::Value) -> String {
    match v {
        grust_core::Value::String(s) => s.clone(),
        grust_core::Value::Int(n) => n.to_string(),
        grust_core::Value::Null => "(null)".to_string(),
        other => format!("{other:?}"),
    }
}
