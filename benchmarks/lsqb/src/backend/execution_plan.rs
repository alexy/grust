use super::{Backend, PreparedBackend};
use crate::queries::QueryCase;
use crate::report::{ExecutionClass, ExecutionPlan};
use grust_cypher::CypherParameters;
use grust_cypher::pushdown::{
    NoTypeHints, ScalarCountReadPushdown, SqlDialect, plan_scalar_count_read,
};
use grust_cypher::read::{IndexedReadPlan, classify_indexed_read_query};

impl PreparedBackend {
    /// Declare the current route in the worker, before GO. This uses the same
    /// per-query backend classifier as execution; it does not guess a plan from
    /// the coordinator's catalog or label opaque backend internals.
    pub fn execution_plan(&self, case: &QueryCase) -> Result<ExecutionPlan, String> {
        if matches!(&self.inner, Backend::Memory(_)) {
            return memory_execution_plan(case);
        }
        let scalar = match &self.inner {
            Backend::Turso { .. } => {
                scalar_count_plan(case, &super::TursoReadDialect::new("grust"))?
            }
            Backend::Postgres { .. } => scalar_count_plan(
                case,
                &super::PostgresReadDialect::new(&super::postgres_config()),
            )?,
            _ => None,
        };
        if scalar.is_some() {
            return Ok(ExecutionPlan::SqlCount);
        }
        if matches!(
            &self.inner,
            Backend::Turso { .. } | Backend::Postgres { .. }
        ) && resident_count_plan(case)?
        {
            return Ok(ExecutionPlan::CountFactorized);
        }
        let class = self
            .execution(case)?
            .class
            .ok_or_else(|| "prepared backend omitted its execution class".to_string())?;
        current_execution_plan(class)
    }
}

/// The same structural proof used by indexed execution, without loading data.
/// Execution parses and plans again inside GO; this preflight is not a cache.
pub fn memory_execution_plan(case: &QueryCase) -> Result<ExecutionPlan, String> {
    let query = grust_cypher::parser::parse_query(&case.executable)
        .map_err(|error| error.into_grust(&case.executable).to_string())?;
    grust_cypher::semantics::analyze(&query).map_err(|error| error.to_string())?;
    match classify_indexed_read_query(&query).map_err(|error| error.to_string())? {
        IndexedReadPlan::ClausePipeline => Ok(ExecutionPlan::ClausePipeline),
        IndexedReadPlan::CountFactorized => Ok(ExecutionPlan::CountFactorized),
    }
}

fn scalar_count_plan(
    case: &QueryCase,
    dialect: &dyn SqlDialect,
) -> Result<Option<ScalarCountReadPushdown>, String> {
    plan_scalar_count_read(&case.executable, &CypherParameters::new(), &NoTypeHints)
        .map(|plan| plan.filter(|plan| plan.supported_by(dialect)))
        .map_err(|error| error.to_string())
}

/// Whether the indexed executor proves a non-materializing count plan for
/// `case`: the same structural proof Memory uses, which a durable store with
/// a resident typed index can run over that index.
pub fn resident_count_plan(case: &QueryCase) -> Result<bool, String> {
    Ok(memory_execution_plan(case)? == ExecutionPlan::CountFactorized)
}

/// The route a durable store with a resident index takes for `case`: its own
/// scalar SQL count when the dialect renders one, the resident count plan
/// when the indexed executor proves one, otherwise the store's existing
/// row-source or materialize class.
pub fn resident_store_execution_class(
    case: &QueryCase,
    dialect: &dyn SqlDialect,
) -> Result<ExecutionClass, String> {
    if scalar_count_plan(case, dialect)?.is_some() {
        Ok(ExecutionClass::BackendNativeAggregate)
    } else if resident_count_plan(case)? {
        Ok(ExecutionClass::BackendResidentIndexRustCount)
    } else {
        super::sql_execution_class(case, dialect)
    }
}

/// Exact SQL submitted by the two opt-in scalar-count adapters. Other backends
/// retain their own execution routes; in particular Sail is not opted in.
pub fn scalar_sql_query(backend: &str, case: &QueryCase) -> Result<Option<String>, String> {
    match backend {
        "turso" => render_scalar_sql(case, &super::TursoReadDialect::new("grust")),
        "postgres" => render_scalar_sql(
            case,
            &super::PostgresReadDialect::new(&super::postgres_config()),
        ),
        _ => Ok(None),
    }
}

fn render_scalar_sql(case: &QueryCase, dialect: &dyn SqlDialect) -> Result<Option<String>, String> {
    scalar_count_plan(case, dialect)?
        .map(|plan| plan.to_sql(dialect).map_err(|error| error.to_string()))
        .transpose()
}

fn current_execution_plan(class: ExecutionClass) -> Result<ExecutionPlan, String> {
    match class {
        ExecutionClass::InProcessReference | ExecutionClass::BackendMaterializeRustReference => {
            Ok(ExecutionPlan::ClausePipeline)
        }
        ExecutionClass::BackendRowSourceRustProjection => Ok(ExecutionPlan::SqlRowSource),
        ExecutionClass::BackendResidentIndexRustCount => Ok(ExecutionPlan::CountFactorized),
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
                ExecutionClass::BackendResidentIndexRustCount,
                ExecutionPlan::CountFactorized,
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
            ExecutionPlan::CountFactorized
        );
        for backend in ["turso", "postgres"] {
            assert_eq!(
                portable_execution_class(backend, &query).unwrap(),
                ExecutionClass::BackendNativeAggregate
            );
            let fallback = case("MATCH (n) WHERE id(n) = 'a' RETURN count(*)");
            assert_eq!(
                current_execution_plan(portable_execution_class(backend, &fallback).unwrap())
                    .unwrap(),
                ExecutionPlan::ClausePipeline
            );
        }
    }

    #[tokio::test]
    async fn scalar_capability_and_filter_proof_gate_all_metadata() {
        let exact = case("MATCH (n {kind:'Comment'}) RETURN count(*)");
        let unsupported = grust_cypher::pushdown::SparkDialect;
        assert!(scalar_count_plan(&exact, &unsupported).unwrap().is_none());
        // Without a scalar SQL count the resident-store route falls through to
        // the structural proof, which admits this inline property scan.
        assert_eq!(
            resident_store_execution_class(&exact, &unsupported).unwrap(),
            ExecutionClass::BackendResidentIndexRustCount
        );

        let prepared = PreparedBackend::prepare("turso", &grust_core::Graph::default())
            .await
            .unwrap();
        // Counts the scalar SQL opt-in declines. Where the indexed executor
        // proves a plan (inline property scans), Turso and PostgreSQL run it
        // over their resident index under the distinct class; where it does
        // not (a general WHERE), both keep SQL row-source.
        let mut proven = 0;
        for query in [
            "MATCH (n) WHERE n.age = 7 RETURN count(*)",
            "MATCH (n {label:'Person'}) RETURN count(*)",
            "MATCH (n {kind:'not\0sql'}) RETURN count(*)",
        ] {
            let query = case(query);
            assert!(scalar_sql_query("turso", &query).unwrap().is_none());
            assert!(scalar_sql_query("postgres", &query).unwrap().is_none());
            let (expected_plan, expected_class) = if resident_count_plan(&query).unwrap() {
                proven += 1;
                (
                    ExecutionPlan::CountFactorized,
                    ExecutionClass::BackendResidentIndexRustCount,
                )
            } else {
                (
                    ExecutionPlan::SqlRowSource,
                    ExecutionClass::BackendRowSourceRustProjection,
                )
            };
            assert_eq!(prepared.execution_plan(&query).unwrap(), expected_plan);
            let execution = prepared.execution(&query).unwrap();
            assert_eq!(execution.class, Some(expected_class));
            assert!(execution.backend_query_sha256.is_none());
            assert_eq!(
                portable_execution_class("turso", &query).unwrap(),
                expected_class
            );
            assert_eq!(
                portable_execution_class("postgres", &query).unwrap(),
                expected_class
            );
        }
        assert!(
            proven > 0 && proven < 3,
            "both routes are exercised: {proven} proven"
        );
        // A projection is not a count plan: the store's row-source route stays.
        let projection = case("MATCH (n) WHERE n.age = 7 RETURN n.age");
        assert!(!resident_count_plan(&projection).unwrap());
        assert_eq!(
            prepared.execution_plan(&projection).unwrap(),
            ExecutionPlan::SqlRowSource
        );
        assert_eq!(
            prepared.execution(&projection).unwrap().class,
            Some(ExecutionClass::BackendRowSourceRustProjection)
        );
        assert_eq!(
            prepared.execution_plan(&exact).unwrap(),
            ExecutionPlan::SqlCount
        );
        for backend in ["turso", "postgres"] {
            assert!(scalar_sql_query(backend, &exact).unwrap().is_some());
            assert_eq!(
                portable_execution_class(backend, &exact).unwrap(),
                ExecutionClass::BackendNativeAggregate
            );
        }
        prepared.finish().await.unwrap();
    }
}
