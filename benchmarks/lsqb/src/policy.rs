use std::fs;
use std::path::Path;
use std::time::Instant;

use grust_core::Graph;
use grust_cypher::{CypherParameters, ReadQueryPolicy, run_bounded_read_query};

use crate::queries::sha256;
use crate::report::PolicyResult;

pub const POLICY_MAX_CANDIDATE_WORK: usize = 10_000;
pub const INTERMEDIATE_ATTACK_MAX_CANDIDATE_WORK: usize = 50_000;
pub const INTERMEDIATE_ATTACK_PARAMETER_BYTES: usize = 48 * 1024;

struct PolicyCase {
    id: &'static str,
    expected_rejection: &'static str,
    max_candidate_work: Option<usize>,
    allow_catalog_procedures: bool,
    payload_bytes: Option<usize>,
}

const CASES: [PolicyCase; 9] = [
    PolicyCase {
        id: "p1-unbounded-path",
        expected_rejection: "syntax.unbounded-path",
        max_candidate_work: None,
        allow_catalog_procedures: false,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p2-range-bomb",
        expected_rejection: "execution.range-limit",
        max_candidate_work: None,
        allow_catalog_procedures: false,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p3-cartesian-work",
        expected_rejection: "execution.candidate-work",
        max_candidate_work: None,
        allow_catalog_procedures: false,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p4-updating-smuggle",
        expected_rejection: "syntax.updating-clause",
        max_candidate_work: None,
        allow_catalog_procedures: false,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p5-forbidden-procedure",
        expected_rejection: "syntax.forbidden-procedure",
        max_candidate_work: None,
        allow_catalog_procedures: false,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p6-union-arms",
        expected_rejection: "syntax.union-arms",
        max_candidate_work: None,
        allow_catalog_procedures: false,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p7-intermediate-projection",
        expected_rejection: "execution.intermediate-bytes",
        max_candidate_work: Some(INTERMEDIATE_ATTACK_MAX_CANDIDATE_WORK),
        allow_catalog_procedures: false,
        payload_bytes: Some(INTERMEDIATE_ATTACK_PARAMETER_BYTES),
    },
    PolicyCase {
        id: "p8-correlated-replan",
        expected_rejection: "execution.candidate-work",
        max_candidate_work: None,
        allow_catalog_procedures: false,
        payload_bytes: None,
    },
    PolicyCase {
        id: "p9-catalog-rescan",
        expected_rejection: "execution.candidate-work",
        max_candidate_work: None,
        allow_catalog_procedures: true,
        payload_bytes: None,
    },
];

pub fn run_policy_attacks(graph: &Graph, directory: &Path) -> Result<Vec<PolicyResult>, String> {
    CASES
        .iter()
        .map(|case| {
            let mut policy = ReadQueryPolicy {
                max_candidate_work: POLICY_MAX_CANDIDATE_WORK,
                ..ReadQueryPolicy::default()
            };
            if let Some(max_candidate_work) = case.max_candidate_work {
                policy.max_candidate_work = max_candidate_work;
            }
            policy.allow_catalog_procedures = case.allow_catalog_procedures;
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

fn classify_rejection(message: &str) -> &'static str {
    if message.contains("unbounded variable-length paths") {
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
}
