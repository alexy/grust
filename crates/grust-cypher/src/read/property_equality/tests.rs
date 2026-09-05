use super::*;
use grust_core::{Decimal, GraphValue, PathValue};
use std::time::{Duration, Instant};

fn limits(bytes: usize) -> read_budget::ReadExecutionBudgetLimits {
    read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: 10_000,
        max_intermediate_bytes: bytes,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    }
}

fn map(expr: Expr) -> MapLiteral {
    MapLiteral {
        entries: vec![("big".into(), expr)],
        span: crate::lexer::Span::new(0, 0),
    }
}

fn large_values() -> Vec<Value> {
    vec![
        Value::Json(serde_json::json!(["x".repeat(32_000)])),
        Value::StringArray(vec!["x".repeat(32_000)]),
        Value::IntArray(vec![1; 4000]),
        Value::FloatArray(vec![1.0; 4000]),
        Value::Path(PathValue::new(
            vec![serde_json::json!("x".repeat(32_000))],
            vec![],
        )),
        Value::Graph(GraphValue::new(
            vec![],
            vec![serde_json::json!("x".repeat(32_000))],
        )),
    ]
}

#[test]
fn scalar_literal_rejection_borrows_large_reference_properties() {
    for value in large_values() {
        let props = Props::from([("big".into(), value)]);
        for literal in [Expr::Integer(0), Expr::String("[]".into()), Expr::Null] {
            let result = read_budget::with_budget(limits(0), || {
                super::super::props_match(&props, Some(&map(literal)), &CypherParameters::new())
            })
            .unwrap();
            assert!(!result);
        }
    }
}

#[test]
fn literal_shortcuts_retain_per_property_work_and_deadline_checks() {
    let props = Props::from([("big".into(), Value::Int(0))]);
    let pattern = map(Expr::Integer(0));
    let error = read_budget::with_budget(
        read_budget::ReadExecutionBudgetLimits {
            max_candidate_work: 0,
            ..limits(1024)
        },
        || super::super::props_match(&props, Some(&pattern), &CypherParameters::new()),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("checking inline property predicates")
    );
    assert!(
        read_budget::with_budget(
            read_budget::ReadExecutionBudgetLimits {
                deadline: Instant::now(),
                ..limits(1024)
            },
            || super::super::props_match(&props, Some(&pattern), &CypherParameters::new()),
        )
        .is_err()
    );
}

#[test]
fn nonliteral_parameter_and_list_fallbacks_refuse_before_json_conversion() {
    let params = CypherParameters::from([("probe".into(), Value::Int(0))]);
    for value in large_values() {
        let props = Props::from([("big".into(), value)]);
        for expr in [
            Expr::Parameter("probe".into()),
            Expr::List(vec![Expr::Integer(0)]),
        ] {
            let error = read_budget::with_budget(limits(1024), || {
                super::super::props_match(&props, Some(&map(expr)), &params)
            })
            .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("converting inline property equality to JSON"),
                "{error}"
            );
        }
    }
}

#[test]
fn bounded_reference_node_candidates_cannot_clone_before_property_guard() {
    let graph = Graph::new(
        vec![Node::new(
            "N",
            "n",
            Props::from([(
                "big".into(),
                Value::Json(serde_json::json!(["x".repeat(32_000)])),
            )]),
        )],
        vec![],
    );
    let params = CypherParameters::from([("probe".into(), Value::Int(0))]);
    let policy = crate::ReadQueryPolicy {
        max_intermediate_bytes: 4096,
        ..crate::ReadQueryPolicy::default()
    };
    let result = crate::run_bounded_read_query(
        &graph,
        "MATCH (n {big:0}) RETURN count(*) LIMIT 1",
        &params,
        &policy,
    )
    .unwrap();
    assert_eq!(result.rows, vec![vec![Value::Int(0)]]);
    for query in [
        "MATCH (n {big:$probe}) RETURN count(*) LIMIT 1",
        "MATCH (n {big:[0]}) RETURN count(*) LIMIT 1",
    ] {
        let error = crate::run_bounded_read_query(&graph, query, &params, &policy).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("converting inline property equality to JSON"),
            "{error}"
        );
    }
}

#[test]
fn missing_property_still_evaluates_nonliteral_parameter_errors() {
    let error = super::super::props_match(
        &Props::new(),
        Some(&map(Expr::Parameter("missing".into()))),
        &CypherParameters::new(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("parameter $missing was not provided")
    );
}

#[test]
fn checked_equality_matches_pure_equality_for_all_value_kinds() {
    let values = vec![
        Value::Null,
        Value::Bool(true),
        Value::Int(7),
        Value::Int(9_007_199_254_740_993),
        Value::Float(7.0),
        Value::Float(7.5),
        Value::Float(f64::NAN),
        Value::from("7"),
        Value::from("é"),
        Value::datetime("2026-06-12T09:30:00Z").unwrap(),
        Value::decimal("7.0").unwrap(),
        Value::duration("P1D").unwrap(),
        Value::StringArray(vec!["7".into()]),
        Value::IntArray(vec![7]),
        Value::FloatArray(vec![7.0]),
        Value::Json(serde_json::json!(null)),
        Value::Json(serde_json::json!(7)),
        Value::Json(serde_json::json!(7.0)),
        Value::Json(serde_json::json!("7")),
        Value::Json(serde_json::json!([7])),
        Value::Json(serde_json::json!({"x":7})),
        Value::Path(PathValue::new(vec![serde_json::json!(7)], vec![])),
        Value::Graph(GraphValue::new(vec![serde_json::json!(7)], vec![])),
    ];
    for a in &values {
        for b in &values {
            assert_eq!(
                checked(a, b).unwrap(),
                values_equal(a, b),
                "a={a:?},b={b:?}"
            );
        }
    }
    // Only the JSON-fallback branch consumes conversion bytes.
    for (a, b) in [
        (Value::Int(7), Value::Float(7.0)),
        (Value::Null, Value::Json(serde_json::Value::Null)),
        (Value::decimal("7").unwrap(), Value::decimal("7").unwrap()),
        (Value::from("same"), Value::from("same")),
    ] {
        assert_eq!(
            read_budget::with_budget(limits(0), || checked(&a, &b)).unwrap(),
            values_equal(&a, &b)
        );
    }
}

#[test]
fn converted_arrays_and_enormous_decimal_scale_are_precharged_without_formatting() {
    let array = Value::IntArray(vec![1; 32]);
    assert!(
        conversion_bytes(&array)
            >= 32 * (std::mem::size_of::<i64>() + std::mem::size_of::<serde_json::Value>())
    );
    let enormous = Value::Decimal(Decimal::from_parts(1, u32::MAX));
    let error = read_budget::with_budget(limits(1024), || checked(&enormous, &Value::from("0")))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("converting inline property equality to JSON")
    );
    for value in [
        Value::decimal("-0.001").unwrap(),
        Value::duration("P1D").unwrap(),
    ] {
        let error =
            read_budget::with_budget(limits(1), || checked(&value, &Value::from("x"))).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("converting inline property equality to JSON")
        );
    }
}
