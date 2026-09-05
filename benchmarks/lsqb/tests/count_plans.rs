//! Offline answer/route integration, not a timing or publication protocol.
//! Each case executes once per backend in its test. PostgreSQL is classified
//! and rendered only; no external service or evidence directory is touched.

use std::path::Path;
use std::sync::OnceLock;

use grust_core::{Graph, Value};
use grust_cypher::CypherParameters;
use grust_cypher::pushdown::{NoTypeHints, plan_read, plan_scalar_count_read};
use grust_lsqb_runner::backend::{
    PreparedBackend, memory_execution_plan, portable_execution_class, scalar_sql_query,
};
use grust_lsqb_runner::dataset::{fingerprint_projected_dataset, load_projected_dataset};
use grust_lsqb_runner::provenance::lsqb_dataset_identity;
use grust_lsqb_runner::queries::{
    DatasetStats, LSQB_EXPECTED_OUTPUT_SHA256, QueryCase, load_adversarial_with_oracle,
    load_baseline, load_baseline_oracle,
};
use grust_lsqb_runner::report::{ExecutionClass, ExecutionPlan};
use grust_postgres_core::{PostgresGraphConfig, PostgresReadDialect};
use grust_turso::TursoReadDialect;
use sha2::{Digest, Sha256};

struct Fixture {
    graph: Graph,
    cases: Vec<QueryCase>,
}

fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"));
        let upstream = directory.join("upstream/lsqb");
        let data = upstream.join("data/social-network-sfexample-projected-fk");
        let graph = load_projected_dataset(&data).expect("load checked-in example graph");
        let stats = DatasetStats::from_graph(&graph);
        let fingerprint = fingerprint_projected_dataset(&data).expect("hash example CSV bytes");
        lsqb_dataset_identity("example", stats, &fingerprint)
            .expect("the example CSV set must match its pinned upstream fingerprint");
        let oracle = load_baseline_oracle(&upstream, "example")
            .expect("verify pinned upstream expected-output.csv");
        assert_eq!(oracle.source_sha256, LSQB_EXPECTED_OUTPUT_SHA256);
        let mut cases = load_baseline(&upstream).expect("verify all nine pinned Cypher sources");
        assert_eq!(cases.len(), 9);
        let attacks = load_adversarial_with_oracle(&directory.join("attacks"), &oracle, stats)
            .expect("derive attack answers from the pinned oracle and verified dataset");
        assert_eq!(attacks.len(), 13);
        cases.extend(attacks);
        Fixture { graph, cases }
    })
}

fn case(id: &str) -> &'static QueryCase {
    fixture().cases.iter().find(|case| case.id == id).unwrap()
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn reference_count(case: &QueryCase) -> i64 {
    let table = grust_cypher::read::run_read_query(
        &fixture().graph,
        &case.executable,
        &CypherParameters::new(),
    )
    .unwrap_or_else(|error| panic!("{} clause reference: {error}", case.id));
    assert_eq!(table.columns.len(), 1, "{} reference columns", case.id);
    assert_eq!(table.rows.len(), 1, "{} reference rows", case.id);
    let [Value::Int(count)] = table.rows[0].as_slice() else {
        panic!("{} did not return one integer: {table:?}", case.id);
    };
    *count
}

fn expected_turso_route(case: &QueryCase) -> (ExecutionClass, ExecutionPlan) {
    let params = CypherParameters::new();
    if plan_scalar_count_read(&case.executable, &params, &NoTypeHints)
        .unwrap()
        .is_some_and(|plan| plan.supported_by(&TursoReadDialect::new("grust")))
    {
        return (
            ExecutionClass::BackendNativeAggregate,
            ExecutionPlan::SqlCount,
        );
    }
    if plan_read(&case.executable, &params, &NoTypeHints)
        .unwrap()
        .is_some_and(|plan| plan.supported_by(&TursoReadDialect::new("grust")))
    {
        (
            ExecutionClass::BackendRowSourceRustProjection,
            ExecutionPlan::SqlRowSource,
        )
    } else {
        (
            ExecutionClass::BackendMaterializeRustReference,
            ExecutionPlan::ClausePipeline,
        )
    }
}

fn check_descriptor(backend: &PreparedBackend, id: &str, case: &QueryCase) {
    let descriptor = backend.execution(case).unwrap();
    let plan = backend.execution_plan(case).unwrap();
    let (expected_class, expected_plan) = match id {
        "memory" => (
            ExecutionClass::InProcessReference,
            memory_execution_plan(case).unwrap(),
        ),
        "turso" => expected_turso_route(case),
        _ => unreachable!(),
    };
    assert_eq!(
        descriptor.class,
        Some(expected_class),
        "{id}/{} class",
        case.id
    );
    assert_eq!(plan, expected_plan, "{id}/{} plan", case.id);
    assert_eq!(portable_execution_class(id, case).unwrap(), expected_class);
    let expected_hash = scalar_sql_query(id, case)
        .unwrap()
        .map(|sql| digest(sql.as_bytes()));
    assert_eq!(
        descriptor.backend_query_sha256, expected_hash,
        "{id}/{} SQL hash",
        case.id
    );

    // All pinned Memory cases must take a proven row-free route. SQL still
    // admits only its separate, narrower scalar subset.
    if id == "memory" {
        assert_eq!(plan, ExecutionPlan::CountFactorized, "memory/{}", case.id);
    }
    if matches!(
        case.id.as_str(),
        "q1" | "q4" | "a1-reversed-chain" | "a7-cartesian-count"
    ) {
        assert_eq!(
            plan,
            if id == "memory" {
                ExecutionPlan::CountFactorized
            } else {
                ExecutionPlan::SqlCount
            }
        );
    }
    if case.id == "a3-split-match" {
        assert_eq!(
            plan,
            if id == "memory" {
                ExecutionPlan::CountFactorized
            } else {
                ExecutionPlan::ClausePipeline
            }
        );
        if id == "turso" {
            assert_eq!(
                descriptor.class,
                Some(ExecutionClass::BackendMaterializeRustReference)
            );
            assert!(descriptor.backend_query_sha256.is_none());
        }
    }
    // The complete pinned optimized set is independently asserted against
    // the manifest in plan_registry.rs.
}

async fn check_case(id: &str) {
    let case = case(id);
    let expected = reference_count(case);
    assert_eq!(
        expected, case.expected_count,
        "{id} pinned oracle/reference"
    );
    for backend_id in ["memory", "turso"] {
        let prepared = PreparedBackend::prepare(backend_id, &fixture().graph)
            .await
            .unwrap_or_else(|error| panic!("{backend_id}/{id} prepare: {error}"));
        check_descriptor(&prepared, backend_id, case);
        // Both prepared backends own their loaded graph. An empty caller
        // source catches an accidental route back to external graph contents.
        let actual = prepared
            .execute_count(case, &Graph::default(), 10_000)
            .await
            .unwrap_or_else(|error| panic!("{backend_id}/{id} execute: {error:?}"));
        assert_eq!(actual, expected, "{backend_id}/{id} reference equality");
        assert_eq!(
            actual, case.expected_count,
            "{backend_id}/{id} pinned answer"
        );
        check_descriptor(&prepared, backend_id, case);
        eprintln!(
            "{backend_id}/{id}: count={actual}, plan={:?}, class={:?}",
            prepared.execution_plan(case).unwrap(),
            prepared.execution(case).unwrap().class
        );
        prepared.finish().await.unwrap();
    }
}

macro_rules! count_cases {
    ($($name:ident => $id:literal),* $(,)?) => {
        $(#[tokio::test]
        async fn $name() { check_case($id).await; })*
    };
}

count_cases! {
    q1 => "q1",
    q2 => "q2",
    q3 => "q3",
    q4 => "q4",
    q5 => "q5",
    q6 => "q6",
    q7 => "q7",
    q8 => "q8",
    q9 => "q9",
    a1_reversed_chain => "a1-reversed-chain",
    a2_reordered_join => "a2-reordered-join",
    a3_split_match => "a3-split-match",
    a4_optional_fanout => "a4-optional-fanout",
    a5_negated_pattern => "a5-negated-pattern",
    a6_range_expansion => "a6-range-expansion",
    a7_cartesian_count => "a7-cartesian-count",
    a8_union_dedup => "a8-union-dedup",
    a9_path_zero_hop => "a9-path-zero-hop",
    a10_unicode_literal => "a10-unicode-literal",
    a11_schema_null_probe => "a11-schema-null-probe",
    a12_parser_comment_trivia => "a12-parser-comment-trivia",
    a13_resource_edge_scan => "a13-resource-edge-scan",
}

#[test]
fn native_sql_hashes_match_the_exact_adapter_renderers_without_services() {
    let params = CypherParameters::new();
    let postgres = PostgresReadDialect::new(&PostgresGraphConfig {
        table_prefix: "lsqb_matrix".into(),
        ..PostgresGraphConfig::default()
    });
    for case in &fixture().cases {
        let plan = plan_scalar_count_read(&case.executable, &params, &NoTypeHints).unwrap();
        for (backend, dialect) in [
            (
                "turso",
                &TursoReadDialect::new("grust") as &dyn grust_cypher::pushdown::SqlDialect,
            ),
            (
                "postgres",
                &postgres as &dyn grust_cypher::pushdown::SqlDialect,
            ),
        ] {
            let actual = scalar_sql_query(backend, case).unwrap();
            let expected = plan
                .as_ref()
                .filter(|plan| plan.supported_by(dialect))
                .map(|plan| plan.to_sql(dialect).unwrap());
            assert_eq!(actual, expected, "{backend}/{} exact SQL", case.id);
            if let Some(sql) = actual {
                assert!(sql.starts_with("SELECT COUNT(*) FROM "), "{sql}");
                let expected_sha = digest(expected.unwrap().as_bytes());
                assert_eq!(
                    grust_lsqb_runner::queries::sha256(sql.as_bytes()),
                    expected_sha
                );
                assert_ne!(
                    expected_sha, case.source_sha256,
                    "SQL digest must not be the Cypher source digest"
                );
                assert_eq!(
                    portable_execution_class(backend, case).unwrap(),
                    ExecutionClass::BackendNativeAggregate
                );
            } else {
                assert_ne!(
                    portable_execution_class(backend, case).unwrap(),
                    ExecutionClass::BackendNativeAggregate
                );
            }
        }
        assert_eq!(
            scalar_sql_query("sail", case).unwrap(),
            None,
            "Sail must remain opt-out"
        );
    }
}

#[tokio::test]
async fn prepared_backends_keep_the_loaded_graph_across_distinct_queries() {
    for id in ["memory", "turso"] {
        let source = fixture().graph.clone();
        let prepared = PreparedBackend::prepare(id, &source).await.unwrap();
        drop(source);
        for query_id in ["q1", "q4", "a3-split-match", "a7-cartesian-count"] {
            let case = case(query_id);
            check_descriptor(&prepared, id, case);
            let count = prepared
                .execute_count(case, &Graph::default(), 10_000)
                .await
                .unwrap();
            assert_eq!(count, case.expected_count, "{id}/{query_id} retained graph");
        }
        prepared.finish().await.unwrap();
    }
}
