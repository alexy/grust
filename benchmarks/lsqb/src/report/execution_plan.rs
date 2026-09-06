//! Current observed execution routes. These do not change schema-v3 provenance
//! classes or infer physical server plans from the backend's name.

use super::{ExecutionClass, ExecutionDescriptorV2};
use crate::queries::{RustRowCardinality, RustRowEstimate};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionPlan {
    ClausePipeline,
    /// Proven fixed-length forest evaluated without materializing match rows.
    CountFactorized,
    SqlRowSource,
    /// Grust lowers the aggregate itself to a backend SQL scalar query.
    SqlCount,
    /// Execution inside the backend; its physical plan is not observed.
    BackendNative,
}

impl ExecutionPlan {
    /// Check worker metadata against the coordinator's admission decision.
    /// A fallback cannot keep a non-materializing plan's row-limit exemption.
    pub fn validate_execution(
        self,
        execution: &ExecutionDescriptorV2,
        rust_rows: Option<RustRowEstimate>,
    ) -> Result<(), String> {
        let materialized =
            rust_rows.is_some_and(|rows| rows.kind != RustRowCardinality::NotMaterialized);
        let native = execution.class == Some(ExecutionClass::BackendNativeAggregate)
            && execution.backend_query_sha256.is_some()
            && rust_rows.is_none();
        let valid = match self {
            Self::ClausePipeline => {
                materialized
                    && matches!(
                        execution.class,
                        Some(
                            ExecutionClass::InProcessReference
                                | ExecutionClass::BackendMaterializeRustReference
                        )
                    )
            }
            Self::CountFactorized => {
                matches!(
                    execution.class,
                    Some(
                        ExecutionClass::InProcessReference
                            | ExecutionClass::BackendResidentIndexRustCount
                    )
                ) && rust_rows
                    == Some(RustRowEstimate {
                        kind: RustRowCardinality::NotMaterialized,
                        rows: 0,
                    })
            }
            Self::SqlRowSource => {
                materialized
                    && execution.class == Some(ExecutionClass::BackendRowSourceRustProjection)
            }
            Self::SqlCount | Self::BackendNative => native,
        };
        if valid {
            Ok(())
        } else {
            Err("worker execution plan disagrees with query admission metadata".to_string())
        }
    }
}

/// An absent field is legacy data. A present null is malformed, not legacy.
pub(crate) fn deserialize_present_execution_plan<'de, D>(
    deserializer: D,
) -> Result<Option<ExecutionPlan>, D::Error>
where
    D: Deserializer<'de>,
{
    ExecutionPlan::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{ObservationTerminationV3, OutcomeStatus, QueryObservationV3};

    fn observation() -> QueryObservationV3 {
        QueryObservationV3 {
            iteration: 1,
            query_position: 1,
            plan: Some(ExecutionPlan::ClausePipeline),
            setup_ns: 7,
            elapsed_ns: 10,
            recovery_ns: 3,
            termination: ObservationTerminationV3::NormalExit,
            actual_count: Some(42),
            outcome: OutcomeStatus::Pass,
            detail: None,
        }
    }

    #[test]
    fn current_execution_plan_values_roundtrip_without_future_claims() {
        for (plan, name) in [
            (ExecutionPlan::ClausePipeline, "clause-pipeline"),
            (ExecutionPlan::CountFactorized, "count-factorized"),
            (ExecutionPlan::SqlRowSource, "sql-row-source"),
            (ExecutionPlan::SqlCount, "sql-count"),
            (ExecutionPlan::BackendNative, "backend-native"),
        ] {
            let mut expected = observation();
            expected.plan = Some(plan);
            let value = serde_json::to_value(&expected).unwrap();
            assert_eq!(value["plan"], name);
            assert_eq!(
                serde_json::from_value::<QueryObservationV3>(value).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn legacy_observations_keep_their_absent_plan() {
        let mut value = serde_json::to_value(observation()).unwrap();
        value.as_object_mut().unwrap().remove("plan");
        let legacy: QueryObservationV3 = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(legacy.plan, None);
        assert_eq!(serde_json::to_value(legacy).unwrap(), value);
    }

    #[test]
    fn unknown_and_explicit_null_plans_are_rejected() {
        for invalid in [
            serde_json::json!("count-intersection"),
            serde_json::json!("clause_pipeline"),
            serde_json::Value::Null,
            serde_json::json!(1),
        ] {
            let mut value = serde_json::to_value(observation()).unwrap();
            value["plan"] = invalid;
            assert!(serde_json::from_value::<QueryObservationV3>(value).is_err());
        }
    }

    #[test]
    fn a_fallback_cannot_keep_the_non_materializing_row_exemption() {
        let execution = ExecutionDescriptorV2 {
            class: Some(ExecutionClass::InProcessReference),
            language: "Grust portable Cypher".into(),
            transport: "in-process".into(),
            backend_query_sha256: None,
        };
        let no_rows = Some(RustRowEstimate {
            kind: RustRowCardinality::NotMaterialized,
            rows: 0,
        });
        assert!(
            ExecutionPlan::CountFactorized
                .validate_execution(&execution, no_rows)
                .is_ok()
        );
        assert!(
            ExecutionPlan::ClausePipeline
                .validate_execution(&execution, no_rows)
                .is_err()
        );
        let empty_match = Some(RustRowEstimate {
            kind: RustRowCardinality::Exact,
            rows: 0,
        });
        assert!(
            ExecutionPlan::CountFactorized
                .validate_execution(&execution, empty_match)
                .is_err()
        );
        assert!(
            ExecutionPlan::ClausePipeline
                .validate_execution(&execution, empty_match)
                .is_ok()
        );
        assert!(
            ExecutionPlan::CountFactorized
                .validate_execution(&execution, None)
                .is_err()
        );
    }
}
