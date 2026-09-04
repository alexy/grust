use std::fs;
use std::path::Path;
use std::time::Instant;

use grust_core::Graph;
use grust_cypher::{CypherParameters, ReadQueryPolicy, run_bounded_read_query};

use crate::queries::sha256;
use crate::report::{PolicyCaseOverrides, PolicyResult};

pub const POLICY_MAX_CANDIDATE_WORK: usize = 10_000;
pub const INTERMEDIATE_ATTACK_MAX_CANDIDATE_WORK: usize = 50_000;
pub const INTERMEDIATE_ATTACK_PARAMETER_BYTES: usize = 48 * 1024;

struct PolicyCase {
    id: &'static str,
    expected_rejection: &'static str,
    max_candidate_work: Option<usize>,
    allow_catalog_procedures: Option<bool>,
    payload_bytes: Option<usize>,
}

const CASES: [PolicyCase; 14] = [
    PolicyCase {
        id: "p1-unbounded-path",
        expected_rejection: "syntax.unbounded-path",
        max_candidate_work: None,
        allow_catalog_procedures: None,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p2-range-bomb",
        expected_rejection: "execution.range-limit",
        max_candidate_work: None,
        allow_catalog_procedures: None,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p3-cartesian-work",
        expected_rejection: "execution.candidate-work",
        max_candidate_work: None,
        allow_catalog_procedures: None,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p4-updating-smuggle",
        expected_rejection: "syntax.updating-clause",
        max_candidate_work: None,
        allow_catalog_procedures: None,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p5-forbidden-procedure",
        expected_rejection: "syntax.forbidden-procedure",
        max_candidate_work: None,
        allow_catalog_procedures: None,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p6-union-arms",
        expected_rejection: "syntax.union-arms",
        max_candidate_work: None,
        allow_catalog_procedures: None,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p7-intermediate-projection",
        expected_rejection: "execution.intermediate-bytes",
        max_candidate_work: Some(INTERMEDIATE_ATTACK_MAX_CANDIDATE_WORK),
        allow_catalog_procedures: None,
        payload_bytes: Some(INTERMEDIATE_ATTACK_PARAMETER_BYTES),
    },
    PolicyCase {
        id: "p8-correlated-replan",
        expected_rejection: "execution.candidate-work",
        max_candidate_work: None,
        allow_catalog_procedures: None,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p9-catalog-rescan",
        expected_rejection: "execution.candidate-work",
        max_candidate_work: None,
        allow_catalog_procedures: Some(true),
        payload_bytes: None,
    },
    PolicyCase {
        id: "p10-resource-query-bytes",
        expected_rejection: "syntax.query-bytes",
        max_candidate_work: None,
        allow_catalog_procedures: None,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p11-path-hop-limit",
        expected_rejection: "syntax.path-limit",
        max_candidate_work: None,
        allow_catalog_procedures: None,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p12-unicode-invalid-scalar",
        expected_rejection: "syntax.invalid-unicode-scalar",
        max_candidate_work: None,
        allow_catalog_procedures: None,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p13-schema-graph-selection",
        expected_rejection: "syntax.graph-selection",
        max_candidate_work: None,
        allow_catalog_procedures: None,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p14-parser-unterminated-comment",
        expected_rejection: "syntax.unterminated-comment",
        max_candidate_work: None,
        allow_catalog_procedures: None,
        payload_bytes: None,
    },
];

/// The effective base policy shared by every attack before documented case
/// overrides are applied.
pub fn benchmark_policy() -> ReadQueryPolicy {
    ReadQueryPolicy {
        max_candidate_work: POLICY_MAX_CANDIDATE_WORK,
        ..ReadQueryPolicy::default()
    }
}

pub fn run_policy_attacks(graph: &Graph, directory: &Path) -> Result<Vec<PolicyResult>, String> {
    CASES
        .iter()
        .map(|case| {
            let mut policy = benchmark_policy();
            if let Some(max_candidate_work) = case.max_candidate_work {
                policy.max_candidate_work = max_candidate_work;
            }
            if let Some(allow_catalog_procedures) = case.allow_catalog_procedures {
                policy.allow_catalog_procedures = allow_catalog_procedures;
            }
            let mut params = CypherParameters::new();
            if let Some(payload_bytes) = case.payload_bytes {
                params.insert(
                    "payload".to_string(),
                    grust_core::Value::from("x".repeat(payload_bytes)),
                );
            }
            let path = directory.join(format!("{}.cypher", case.id));
            let source = fs::read_to_string(&path)
                .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
            let started = Instant::now();
            let outcome = run_bounded_read_query(graph, &source, &params, &policy);
            let elapsed_ns = started.elapsed().as_nanos();
            let (actual_rejection, error) = match outcome {
                Ok(_) => ("accepted".to_string(), None),
                Err(error) => {
                    let message = error.to_string();
                    (classify_rejection(&message).to_string(), Some(message))
                }
            };
            Ok(PolicyResult {
                id: case.id.to_string(),
                source_sha256: sha256(source.as_bytes()),
                overrides: case.overrides(),
                expected_rejection: case.expected_rejection.to_string(),
                status: if actual_rejection == case.expected_rejection {
                    "pass".to_string()
                } else {
                    "fail".to_string()
                },
                actual_rejection,
                elapsed_ns,
                error,
            })
        })
        .collect()
}

impl PolicyCase {
    fn overrides(&self) -> PolicyCaseOverrides {
        PolicyCaseOverrides {
            max_candidate_work: self.max_candidate_work,
            allow_catalog_procedures: self.allow_catalog_procedures,
            parameter_payload_bytes: self.payload_bytes,
        }
    }
}

fn classify_rejection(message: &str) -> &'static str {
    if message.contains("query must contain 1 to") && message.contains("bytes") {
        "syntax.query-bytes"
    } else if message.contains("invalid unicode scalar value") {
        "syntax.invalid-unicode-scalar"
    } else if message.contains("unterminated block comment") {
        "syntax.unterminated-comment"
    } else if message.contains("graph selection is forbidden") {
        "syntax.graph-selection"
    } else if message.contains("path can traverse") && message.contains("read policy maximum") {
        "syntax.path-limit"
    } else if message.contains("unbounded variable-length paths") {
        "syntax.unbounded-path"
    } else if message.contains("read policy maximum") && message.contains("range()") {
        "execution.range-limit"
    } else if message.contains("candidate-work") {
        "execution.candidate-work"
    } else if message.contains("cumulative intermediate bytes") {
        "execution.intermediate-bytes"
    } else if message.contains("updating clauses are forbidden") {
        "syntax.updating-clause"
    } else if message.contains("procedure calls are forbidden") {
        "syntax.forbidden-procedure"
    } else if message.contains("UNION arms") {
        "syntax.union-arms"
    } else if message.contains("GQL.SYNTAX") {
        "syntax.other"
    } else if message.contains("GQL.EXECUTION") {
        "execution.other"
    } else {
        "unclassified"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::PolicyLimits;

    const EXTENDED_CASES: [(&str, &str); 5] = [
        (
            include_str!("../attacks/p10-resource-query-bytes.cypher"),
            "syntax.query-bytes",
        ),
        (
            include_str!("../attacks/p11-path-hop-limit.cypher"),
            "syntax.path-limit",
        ),
        (
            include_str!("../attacks/p12-unicode-invalid-scalar.cypher"),
            "syntax.invalid-unicode-scalar",
        ),
        (
            include_str!("../attacks/p13-schema-graph-selection.cypher"),
            "syntax.graph-selection",
        ),
        (
            include_str!("../attacks/p14-parser-unterminated-comment.cypher"),
            "syntax.unterminated-comment",
        ),
    ];

    #[test]
    fn rejection_classifier_is_stable() {
        assert_eq!(
            classify_rejection("[GQL.SYNTAX] unbounded variable-length paths are forbidden"),
            "syntax.unbounded-path"
        );
        assert_eq!(
            classify_rejection("[GQL.EXECUTION] bounded read exceeded candidate-work"),
            "execution.candidate-work"
        );
        assert_eq!(
            classify_rejection(
                "[GQL.EXECUTION] bounded read exceeded cumulative intermediate bytes"
            ),
            "execution.intermediate-bytes"
        );
    }

    #[test]
    fn extended_policy_fixtures_hit_exact_rejection_classes() {
        let graph = Graph::new(Vec::new(), Vec::new());
        for (source, expected) in EXTENDED_CASES {
            let error = run_bounded_read_query(
                &graph,
                source,
                &CypherParameters::new(),
                &ReadQueryPolicy::default(),
            )
            .expect_err("an adversarial policy fixture must be rejected");
            assert_eq!(classify_rejection(&error.to_string()), expected, "{error}");
        }
    }

    #[test]
    fn report_policy_is_the_full_effective_base_policy() {
        let policy = benchmark_policy();
        let reported = PolicyLimits::from(&policy);

        assert_eq!(reported.max_query_bytes, 2_000);
        assert_eq!(reported.max_parameter_bytes, 64 * 1024);
        assert_eq!(reported.max_graph_nodes, 100_000);
        assert_eq!(reported.max_graph_edges, 500_000);
        assert_eq!(reported.max_graph_bytes, 64 * 1024 * 1024);
        assert_eq!(reported.max_candidate_work, POLICY_MAX_CANDIDATE_WORK);
        assert_eq!(reported.max_intermediate_bytes, 256 * 1024 * 1024);
        assert_eq!(reported.max_result_rows, 50);
        assert_eq!(reported.max_output_bytes, 1024 * 1024);
        assert_eq!(reported.max_range_items, 10_000);
        assert_eq!(reported.max_union_arms, 4);
        assert_eq!(reported.max_path_length, 4);
        assert_eq!(reported.max_execution_time_ms, 2_000);
        assert!(!reported.allow_graph_selection);
        assert!(!reported.allow_catalog_procedures);
        assert!(reported.require_match);
    }

    #[test]
    fn per_case_overrides_are_explicit_and_exhaustive() {
        for case in &CASES {
            let overrides = case.overrides();
            match case.id {
                "p7-intermediate-projection" => {
                    assert_eq!(
                        overrides.max_candidate_work,
                        Some(INTERMEDIATE_ATTACK_MAX_CANDIDATE_WORK)
                    );
                    assert_eq!(
                        overrides.parameter_payload_bytes,
                        Some(INTERMEDIATE_ATTACK_PARAMETER_BYTES)
                    );
                    assert_eq!(overrides.allow_catalog_procedures, None);
                }
                "p9-catalog-rescan" => {
                    assert_eq!(overrides.allow_catalog_procedures, Some(true));
                    assert_eq!(overrides.max_candidate_work, None);
                    assert_eq!(overrides.parameter_payload_bytes, None);
                }
                _ => assert_eq!(overrides, PolicyCaseOverrides::default()),
            }
        }
    }
}
