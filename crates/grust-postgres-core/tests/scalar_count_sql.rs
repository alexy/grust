//! Offline PostgreSQL count SQL contract. Native execution requires a service
//! and is intentionally not claimed by these rendering/projection tests.

use grust_core::Value;
use grust_cypher::CypherParameters;
use grust_cypher::pushdown::{NoTypeHints, plan_scalar_count_read};
use grust_postgres_core::{PostgresGraphConfig, PostgresReadDialect};

#[test]
fn postgres_scalar_count_uses_qualified_universal_tables() {
    let config = PostgresGraphConfig {
        schema: "tenant".into(),
        table_prefix: "social".into(),
        ..Default::default()
    };
    let dialect = PostgresReadDialect::new(&config);
    let params = [("kind".into(), Value::from("Comment"))]
        .into_iter()
        .collect();
    let plan = plan_scalar_count_read(
        "MATCH (a:Person)-[:R]->(b)-[:S]->(c) WHERE a.kind = $kind RETURN count(*) AS c LIMIT 0",
        &params,
        &NoTypeHints,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        plan.to_sql(&dialect).unwrap(),
        "SELECT COUNT(*) FROM \"tenant\".\"social_nodes\" n0 \
         JOIN \"tenant\".\"social_edges\" e0 ON e0.from_id = n0.id \
         JOIN \"tenant\".\"social_nodes\" n1 ON n1.id = e0.to_id \
         JOIN \"tenant\".\"social_edges\" e1 ON e1.from_id = n1.id \
         JOIN \"tenant\".\"social_nodes\" n2 ON n2.id = e1.to_id \
         WHERE e0.label = 'R' AND e1.label = 'S' AND n0.label = 'Person' \
         AND (jsonb_typeof(n0.props #> ARRAY['kind', 'value']) = 'string' AND (n0.props #>> ARRAY['kind', 'value']) COLLATE \"C\" = 'Comment')"
    );
    assert_eq!(plan.column_count(), 1);
    let table = plan
        .project_text_rows(vec![vec![Some("0".into())]], &params)
        .unwrap();
    assert_eq!(table.columns, vec!["c"]);
    assert!(table.rows.is_empty());
}

#[test]
fn postgres_zero_count_has_one_row_without_pagination() {
    let params = CypherParameters::new();
    let plan = plan_scalar_count_read(
        "MATCH (n:Missing) RETURN count(*) AS total",
        &params,
        &NoTypeHints,
    )
    .unwrap()
    .unwrap();
    let table = plan
        .project_text_rows(vec![vec![Some("0".into())]], &params)
        .unwrap();
    assert_eq!(table.columns, vec!["total"]);
    assert_eq!(table.rows, vec![vec![Value::Int(0)]]);
    assert_eq!(
        plan.to_sql(&PostgresReadDialect::new(&PostgresGraphConfig::default()))
            .unwrap(),
        "SELECT COUNT(*) FROM \"public\".\"grust_nodes\" WHERE label = 'Missing'"
    );
}

#[test]
fn postgres_exact_payload_type_and_byte_comparison_are_required() {
    let params = [("kind".into(), Value::from("O'Hara\\雪"))]
        .into_iter()
        .collect();
    let dialect = PostgresReadDialect::new(&PostgresGraphConfig::default());
    for query in [
        "MATCH (n {kind:$kind}) RETURN count(*)",
        "MATCH ()-[:R {kind:$kind}]->() RETURN count(*)",
        "MATCH (a)-[:R]->(b), (b)-[:S]->(a) WHERE a.kind = $kind RETURN count(*)",
    ] {
        let plan = plan_scalar_count_read(query, &params, &NoTypeHints)
            .unwrap()
            .unwrap();
        assert!(plan.supported_by(&dialect));
        let sql = plan.to_sql(&dialect).unwrap();
        assert!(sql.contains("jsonb_typeof("), "{sql}");
        assert!(
            sql.contains("#> ARRAY['kind', 'value']) = 'string'"),
            "{sql}"
        );
        assert!(sql.contains("COLLATE \"C\""), "{sql}");
        assert!(sql.contains("O''Hara"), "{sql}");
        assert!(!sql.contains("::bigint"), "{sql}");
    }
    for query in [
        "MATCH (n {label:'Person'}) RETURN count(*)",
        "MATCH (n) WHERE n.age = 7 RETURN count(*)",
        "MATCH (n) WHERE n.active = true RETURN count(*)",
    ] {
        assert!(
            plan_scalar_count_read(query, &CypherParameters::new(), &NoTypeHints)
                .unwrap()
                .is_none()
        );
    }
    let params = [("kind".into(), Value::from("not\0sql"))]
        .into_iter()
        .collect();
    let plan = plan_scalar_count_read(
        "MATCH (n {kind:$kind}) RETURN count(*)",
        &params,
        &NoTypeHints,
    )
    .unwrap()
    .unwrap();
    assert!(!plan.supported_by(&dialect));
    assert!(plan.to_sql(&dialect).is_err());
}
