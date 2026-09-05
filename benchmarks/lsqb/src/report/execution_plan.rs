//! Current observed execution routes. These do not change schema-v3 provenance
//! classes or infer physical server plans from the backend's name.

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionPlan {
    ClausePipeline,
    SqlRowSource,
    /// Execution inside the backend; its physical plan is not observed.
    BackendNative,
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
            (ExecutionPlan::SqlRowSource, "sql-row-source"),
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
            serde_json::json!("count-factorized"),
            serde_json::json!("clause_pipeline"),
            serde_json::Value::Null,
            serde_json::json!(1),
        ] {
            let mut value = serde_json::to_value(observation()).unwrap();
            value["plan"] = invalid;
            assert!(serde_json::from_value::<QueryObservationV3>(value).is_err());
        }
    }
}
