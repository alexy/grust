use super::PreparedBackend;
use crate::queries::QueryCase;
use crate::report::{ExecutionClass, ExecutionPlan};

impl PreparedBackend {
    /// Declare the current route in the worker, before GO. This uses the same
    /// per-query backend classifier as execution; it does not guess a plan from
    /// the coordinator's catalog or label opaque backend internals.
    pub fn execution_plan(&self, case: &QueryCase) -> Result<ExecutionPlan, String> {
        let class = self
            .execution(case)?
            .class
            .ok_or_else(|| "prepared backend omitted its execution class".to_string())?;
        current_execution_plan(class)
    }
}

fn current_execution_plan(class: ExecutionClass) -> Result<ExecutionPlan, String> {
    match class {
        ExecutionClass::InProcessReference | ExecutionClass::BackendMaterializeRustReference => {
            Ok(ExecutionPlan::ClausePipeline)
        }
        ExecutionClass::BackendRowSourceRustProjection => Ok(ExecutionPlan::SqlRowSource),
        ExecutionClass::BackendNativeAggregate => Ok(ExecutionPlan::BackendNative),
        ExecutionClass::BackendNeutralPolicy => {
            Err("policy classification is not a query execution plan".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::portable_execution_class;

    fn case(source: &str) -> QueryCase {
        QueryCase {
            id: "test".into(),
            executable: source.into(),
            source_sha256: "test".into(),
            expected_count: 0,
            claim: "test".into(),
        }
    }

    #[test]
    fn current_execution_plan_mapping_covers_only_actual_routes() {
        for (class, expected) in [
            (
                ExecutionClass::InProcessReference,
                ExecutionPlan::ClausePipeline,
            ),
            (
                ExecutionClass::BackendMaterializeRustReference,
                ExecutionPlan::ClausePipeline,
            ),
            (
                ExecutionClass::BackendRowSourceRustProjection,
                ExecutionPlan::SqlRowSource,
            ),
            (
                ExecutionClass::BackendNativeAggregate,
                ExecutionPlan::BackendNative,
            ),
        ] {
            assert_eq!(current_execution_plan(class).unwrap(), expected);
        }
        assert!(current_execution_plan(ExecutionClass::BackendNeutralPolicy).is_err());
    }

    #[tokio::test]
    async fn worker_execution_plan_matches_memory_and_sql_query_selection() {
        let query = case("MATCH (n) RETURN count(*)");
        let memory = PreparedBackend::prepare("memory", &grust_core::Graph::default())
            .await
            .unwrap();
        assert_eq!(
            memory.execution_plan(&query).unwrap(),
            ExecutionPlan::ClausePipeline
        );
        for backend in ["turso", "postgres"] {
            assert_eq!(
                current_execution_plan(portable_execution_class(backend, &query).unwrap()).unwrap(),
                ExecutionPlan::SqlRowSource
            );
            let fallback = case("MATCH (n) WHERE id(n) = 'a' RETURN count(*)");
            assert_eq!(
                current_execution_plan(portable_execution_class(backend, &fallback).unwrap())
                    .unwrap(),
                ExecutionPlan::ClausePipeline
            );
        }
    }
}
