//! Borrow scalar pattern predicates from the AST and immutable graph.
//!
//! These helpers are only for planners that have proved scalar-literal maps.
//! They preserve reference equality (including its numeric coercion and JSON
//! scalar compatibility) without cloning a complex property to compare it to
//! a scalar. Formatting the two non-borrowable scalar types is precharged.

use super::*;

fn strings_equal(actual: &str, expected: &str) -> Result<bool> {
    if actual.len() != expected.len() {
        return Ok(false);
    }
    read_budget::charge_candidate_work(actual.len(), "comparing literal count strings")?;
    Ok(actual == expected)
}

fn decimal_string_equal(actual: &grust_core::Decimal, expected: &str) -> Result<bool> {
    let digits = actual
        .mantissa()
        .unsigned_abs()
        .checked_ilog10()
        .unwrap_or(0) as usize
        + 1;
    let scale = actual.scale() as usize;
    let length = if scale == 0 {
        digits
    } else if scale >= digits {
        scale.saturating_add(2)
    } else {
        digits + 1
    }
    .saturating_add(usize::from(actual.mantissa() < 0));
    if length != expected.len() {
        return Ok(false);
    }
    // Canonical formatting owns digit, padded and possibly signed buffers.
    read_budget::charge_intermediate_bytes(
        length.saturating_mul(4).saturating_add(128),
        "formatting decimal count predicate",
    )?;
    strings_equal(&actual.to_canonical_string(), expected)
}

pub(super) fn literal_equal(actual: &Value, expected: &Expr) -> Result<bool> {
    Ok(match expected {
        Expr::Null => false,
        Expr::Boolean(expected) => match actual {
            Value::Bool(actual) | Value::Json(serde_json::Value::Bool(actual)) => {
                actual == expected
            }
            _ => false,
        },
        Expr::Integer(expected) => match numeric(actual) {
            Some(actual) => actual == *expected as f64,
            None => {
                matches!(actual, Value::Json(serde_json::Value::Number(actual))
                    if actual == &serde_json::Number::from(*expected))
            }
        },
        Expr::Float(expected) => match numeric(actual) {
            Some(actual) => actual == *expected,
            // JSON equality is intentionally different from numeric coercion.
            // This also preserves the reference's non-finite float -> JSON null.
            None => match (actual, serde_json::Number::from_f64(*expected)) {
                (Value::Json(serde_json::Value::Number(actual)), Some(expected)) => {
                    actual == &expected
                }
                (Value::Json(serde_json::Value::Null), None) => true,
                _ => false,
            },
        },
        Expr::String(expected) => match actual {
            Value::String(actual) | Value::Json(serde_json::Value::String(actual)) => {
                strings_equal(actual, expected)?
            }
            Value::DateTime(actual) => strings_equal(actual.as_str(), expected)?,
            Value::Decimal(actual) => decimal_string_equal(actual, expected)?,
            Value::Duration(actual) => {
                // Four bounded integer components plus formatting temporaries.
                read_budget::charge_intermediate_bytes(512, "formatting duration count predicate")?;
                strings_equal(&actual.to_iso_string(), expected)?
            }
            _ => false,
        },
        _ => {
            return Err(gql_execution(
                "indexed count predicate requires a scalar literal",
            ));
        }
    })
}

pub(super) fn props_match(
    props: &Props,
    map: Option<&MapLiteral>,
    _params: &CypherParameters,
) -> Result<bool> {
    let Some(map) = map else { return Ok(true) };
    for (key, literal) in &map.entries {
        let comparisons = props.len().checked_ilog2().unwrap_or(0) as usize + 1;
        read_budget::charge_candidate_work(
            key.len().saturating_add(1).saturating_mul(comparisons),
            "looking up literal count properties",
        )?;
        let Some(actual) = props.get(key) else {
            return Ok(false);
        };
        if !literal_equal(actual, literal)? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn node_matches(
    node: &Node,
    pattern: &NodePattern,
    params: &CypherParameters,
) -> Result<bool> {
    for label in &pattern.labels {
        read_budget::charge_candidate_work(1, "checking literal count labels")?;
        if !strings_equal(node.label.as_str(), label)? {
            return Ok(false);
        }
    }
    props_match(&node.props, pattern.properties.as_ref(), params)
}

#[cfg(test)]
#[path = "count_predicate_tests.rs"]
mod tests;
