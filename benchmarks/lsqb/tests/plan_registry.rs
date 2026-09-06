//! Keep optimized admission synchronized with actual planners and SQL bytes.
//! This test parses/renders pinned queries only; it executes no queries/services.

use std::path::Path;

use grust_lsqb_runner::{backend, queries, report::ExecutionPlan};
use serde_json::{Value, json};

#[test]
fn registry_matches_every_optimized_plan_without_authorizing_fallbacks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut cases = queries::load_baseline(&root.join("upstream/lsqb")).unwrap();
    cases.extend(queries::load_adversarial(&root.join("attacks")).unwrap());
    assert_eq!(cases.len(), 22);
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(root.join("evidence-manifest-v2.json")).unwrap())
            .unwrap();
    let registry = &manifest["execution_plans"];
    assert_eq!(registry["schema"], "grust-lsqb-execution-plan-registry-v1");
    let entries = registry["entries"].as_object().unwrap();
    assert_eq!(entries.len(), 3);

    for id in ["memory", "turso", "postgres"] {
        let registered = entries[id].as_object().unwrap();
        let mut optimized = 0;
        for case in &cases {
            let memory = id == "memory";
            let sql = backend::scalar_sql_query(id, case).unwrap();
            let resident = !memory && sql.is_none() && backend::resident_count_plan(case).unwrap();
            let eligible = if memory {
                backend::memory_execution_plan(case).unwrap() == ExecutionPlan::CountFactorized
            } else {
                sql.is_some() || resident
            };
            // Widening this contract is deliberate work, not an incidental
            // classifier change hidden by automatic inventory regeneration.
            let expected = matches!(
                case.id.as_str(),
                "q1" | "q4" | "a1-reversed-chain" | "a7-cartesian-count"
            ) || matches!(
                case.id.as_str(),
                "q2" | "q3"
                    | "q5"
                    | "q6"
                    | "q7"
                    | "q8"
                    | "q9"
                    | "a2-reordered-join"
                    | "a3-split-match"
                    | "a4-optional-fanout"
                    | "a5-negated-pattern"
                    | "a6-range-expansion"
                    | "a8-union-dedup"
                    | "a9-path-zero-hop"
                    | "a10-unicode-literal"
                    | "a11-schema-null-probe"
                    | "a12-parser-comment-trivia"
                    | "a13-resource-edge-scan"
            );
            assert_eq!(eligible, expected, "{id}/{} eligibility", case.id);
            if !eligible {
                assert!(
                    !registered.contains_key(&case.id),
                    "{id}/{} fallback must not have an optimized exemption",
                    case.id
                );
                continue;
            }
            optimized += 1;
            let expected_entry = json!({
                "plan": if memory || resident { "count-factorized" } else { "sql-count" },
                "source_sha256": case.source_sha256,
                "adapter_sha256": queries::sha256(case.executable.as_bytes()),
                "execution_class": backend::portable_execution_class(id, case).unwrap(),
                "rust_rows": if memory || resident {
                    json!({"kind": "not-materialized", "rows": 0})
                } else {
                    Value::Null
                },
                "backend_query_sha256": sql.map(|sql| queries::sha256(sql.as_bytes())),
            });
            assert_eq!(
                registered[&case.id], expected_entry,
                "{id}/{} registry",
                case.id
            );
        }
        assert_eq!(
            registered.len(),
            optimized,
            "{id}: no unknown registry entries"
        );
    }
}
