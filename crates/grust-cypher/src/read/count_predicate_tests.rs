use super::*;
use grust_core::{Decimal, Duration, GraphValue, PathValue, TypedGraphIndex};
use std::sync::Arc;
use std::time::{Duration as Timeout, Instant};

fn limits(bytes: usize, work: usize) -> read_budget::ReadExecutionBudgetLimits {
    read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: work,
        max_intermediate_bytes: bytes,
        max_range_items: 100,
        deadline: Instant::now() + Timeout::from_secs(5),
    }
}

#[test]
fn borrowed_scalar_equality_matches_all_reference_value_variants() {
    let actuals = [
        Value::Null,
        Value::Bool(true),
        Value::Bool(false),
        Value::Int(7),
        Value::Int(9_007_199_254_740_993),
        Value::Int(i64::MIN),
        Value::Float(7.0),
        Value::Float(7.5),
        Value::Float(f64::NAN),
        Value::Float(f64::INFINITY),
        Value::Float(f64::NEG_INFINITY),
        Value::from("7"),
        Value::from("7.5"),
        Value::from("true"),
        Value::from(""),
        Value::from("Comment\0suffix"),
        Value::from("é"),
        Value::from("é"),
        Value::datetime("2026-06-12T09:30:00Z").unwrap(),
        Value::decimal("7").unwrap(),
        Value::decimal("7.5").unwrap(),
        Value::decimal("-0.001").unwrap(),
        Value::Decimal(Decimal::from_parts(i128::MIN, 3)),
        Value::duration("P1D").unwrap(),
        Value::Duration(Duration {
            months: i64::MIN,
            days: i64::MAX,
            seconds: i64::MIN,
            nanos: i32::MIN,
        }),
        Value::StringArray(vec!["7".into()]),
        Value::IntArray(vec![7]),
        Value::FloatArray(vec![7.0]),
        Value::Path(PathValue::new(vec![], vec![])),
        Value::Graph(GraphValue::new(vec![], vec![])),
        Value::Json(serde_json::json!(null)),
        Value::Json(serde_json::json!(true)),
        Value::Json(serde_json::json!(7)),
        Value::Json(serde_json::json!(7.0)),
        Value::Json(serde_json::json!(7.5)),
        Value::Json(serde_json::json!(0)),
        Value::Json(serde_json::json!(-0.0)),
        Value::Json(serde_json::json!(u64::MAX)),
        Value::Json(serde_json::json!("7")),
        Value::Json(serde_json::json!("Comment\0suffix")),
        Value::Json(serde_json::json!([])),
        Value::Json(serde_json::json!({"kind":"Comment"})),
    ];
    let mut literals = vec![
        Expr::Null,
        Expr::Boolean(true),
        Expr::Boolean(false),
        Expr::Integer(7),
        Expr::Integer(9_007_199_254_740_992),
        Expr::Integer(i64::MIN),
        Expr::Integer(0),
        Expr::Integer(i64::MAX),
        Expr::Float(0.0),
        Expr::Float(-0.0),
        Expr::Float(7.0),
        Expr::Float(7.5),
        Expr::Float(f64::NAN),
        Expr::Float(f64::INFINITY),
        Expr::Float(f64::NEG_INFINITY),
    ];
    for value in &actuals {
        if let serde_json::Value::String(string) = value.to_json() {
            literals.push(Expr::String(string));
        }
    }
    literals.extend(["Comment", "7.00", "[]", "é", "é"].map(|s| Expr::String(s.into())));
    for actual in &actuals {
        for literal in &literals {
            let expected = eval_constant(literal, &CypherParameters::new()).unwrap();
            assert_eq!(
                literal_equal(actual, literal).unwrap(),
                values_equal(actual, &expected) == Some(true),
                "actual={actual:?}, literal={literal:?}"
            );
        }
    }
}

#[test]
fn json_numbers_keep_representation_equality_and_nonfinite_null_conversion() {
    for (actual, literal, expected) in [
        (serde_json::json!(7), Expr::Integer(7), true),
        (serde_json::json!(7.0), Expr::Integer(7), false),
        (serde_json::json!(7), Expr::Float(7.0), false),
        (serde_json::json!(7.0), Expr::Float(7.0), true),
        (serde_json::json!(0), Expr::Float(-0.0), false),
        (serde_json::json!(-0.0), Expr::Float(0.0), true),
        (serde_json::Value::Null, Expr::Float(f64::NAN), true),
        (serde_json::Value::Null, Expr::Float(f64::INFINITY), true),
        (serde_json::Value::Null, Expr::Float(1.0), false),
    ] {
        assert_eq!(
            literal_equal(&Value::Json(actual), &literal).unwrap(),
            expected
        );
    }
}

#[test]
fn complex_properties_are_rejected_without_copying_their_payloads() {
    let values = [
        Value::Json(serde_json::json!(["x".repeat(32_000)])),
        Value::StringArray(vec!["x".repeat(32_000)]),
        Value::Path(PathValue::new(
            vec![serde_json::json!("x".repeat(32_000))],
            vec![],
        )),
    ];
    for value in values {
        let node = Node::new("N", "n", Props::from([("big".into(), value.clone())]));
        let edge = Edge::new("R", "n", "n", Props::from([("big".into(), value)]));
        let index = TypedGraphIndex::new(Arc::new(Graph::new(vec![node], vec![edge]))).unwrap();
        for source in [
            "MATCH (n {big:0}) RETURN count(n)",
            "MATCH (n)-[r {big:0}]->(n) RETURN count(r)",
            "MATCH (n {big:'[]'})-[*0..0]->(m) RETURN count(m)",
        ] {
            let result = read_budget::with_budget(limits(512, 100), || {
                run_read_query_indexed(&index, source, &CypherParameters::new())
            })
            .unwrap();
            assert_eq!(result.rows, vec![vec![Value::Int(0)]], "{source}");
        }
    }
}

#[test]
fn string_work_and_owned_scalar_formatting_are_budgeted() {
    let large = "x".repeat(1024);
    assert!(
        read_budget::with_budget(limits(1, 100), || {
            literal_equal(&Value::from(large.clone()), &Expr::String(large.clone()))
        })
        .unwrap_err()
        .to_string()
        .contains("comparing literal count strings")
    );
    for actual in [
        Value::decimal("0.001").unwrap(),
        Value::duration("P1D").unwrap(),
    ] {
        let serde_json::Value::String(expected) = actual.to_json() else {
            unreachable!()
        };
        assert!(
            read_budget::with_budget(limits(1, 100), || {
                literal_equal(&actual, &Expr::String(expected))
            })
            .unwrap_err()
            .to_string()
            .contains("formatting")
        );
    }
    // An enormous scale does not allocate an enormous canonical string merely
    // to learn that it cannot equal a short literal.
    assert!(
        !read_budget::with_budget(limits(1, 1), || {
            literal_equal(
                &Value::Decimal(Decimal::from_parts(1, u32::MAX)),
                &Expr::String("0".into()),
            )
        })
        .unwrap()
    );
}

#[test]
fn property_and_label_domains_preserve_conjunction_and_null_semantics() {
    let node = Node::new(
        "N",
        "n",
        Props::from([
            ("kind".into(), Value::from("Comment")),
            ("null".into(), Value::Null),
            ("jsonnull".into(), Value::Json(serde_json::Value::Null)),
        ]),
    );
    for (source, expected) in [
        ("MATCH (n:N:N {kind:'Comment'}) RETURN count(*)", true),
        ("MATCH (n:N:X {kind:'Comment'}) RETURN count(*)", false),
        (
            "MATCH (n {kind:'Comment',kind:'Post'}) RETURN count(*)",
            false,
        ),
        (
            "MATCH (n {kind:'Comment',kind:'Comment'}) RETURN count(*)",
            true,
        ),
        ("MATCH (n {null:null}) RETURN count(*)", false),
        ("MATCH (n {jsonnull:null}) RETURN count(*)", false),
        ("MATCH (n {missing:1}) RETURN count(*)", false),
    ] {
        let query = parse_query(source).unwrap();
        let Clause::Match(m) = &query.parts[0].query.clauses[0] else {
            unreachable!()
        };
        let pattern = &m.patterns[0].start;
        assert_eq!(
            node_matches(&node, pattern, &CypherParameters::new()).unwrap(),
            expected,
            "{source}"
        );
        assert_eq!(
            super::super::node_matches(&node, pattern, &CypherParameters::new()).unwrap(),
            expected,
            "{source}"
        );
    }
}
