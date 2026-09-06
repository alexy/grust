//! Derive the optimized-plan registry from pinned queries and actual planners.
//! This only parses/classifies/renders: it runs no queries or backend services.

use std::collections::BTreeMap;
use std::path::Path;

use grust_lsqb_runner::{backend, queries, report::ExecutionPlan};
use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut cases = queries::load_baseline(&root.join("upstream/lsqb"))?;
    cases.extend(queries::load_adversarial(&root.join("attacks"))?);
    let mut entries = BTreeMap::new();
    for id in ["memory", "turso", "postgres"] {
        let mut plans: BTreeMap<String, Value> = BTreeMap::new();
        for case in &cases {
            let (plan, class, rows, sql_hash) = if id == "memory" {
                if backend::memory_execution_plan(case)? != ExecutionPlan::CountFactorized {
                    continue;
                }
                (
                    "count-factorized",
                    "in-process-reference",
                    json!({"kind": "not-materialized", "rows": 0}),
                    None,
                )
            } else if backend::resident_count_plan(case)? {
                (
                    "count-factorized",
                    "backend-resident-index-rust-count",
                    json!({"kind": "not-materialized", "rows": 0}),
                    None,
                )
            } else if let Some(sql) = backend::scalar_sql_query(id, case)? {
                (
                    "sql-count",
                    "backend-native-aggregate",
                    Value::Null,
                    Some(queries::sha256(sql.as_bytes())),
                )
            } else {
                continue;
            };
            plans.insert(
                case.id.clone(),
                json!({
                    "plan": plan,
                    "source_sha256": case.source_sha256,
                    "adapter_sha256": queries::sha256(case.executable.as_bytes()),
                    "execution_class": class,
                    "rust_rows": rows,
                    "backend_query_sha256": sql_hash,
                }),
            );
        }
        entries.insert(id, plans);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "grust-lsqb-execution-plan-registry-v1",
            "entries": entries,
        }))?
    );
    Ok(())
}
