//! Account JSON conversion only for inline-property equality's fallback.
//! General expression equality keeps its existing pure interface and behavior.

use super::*;

pub(super) enum Decision {
    Known(Option<bool>),
    JsonFallback,
}

/// The single source of truth for equality branches that do not convert JSON.
pub(super) fn decision(a: &Value, b: &Value) -> Decision {
    if matches!(a, Value::Null) || matches!(b, Value::Null) {
        return Decision::Known(None);
    }
    match (a, b) {
        (Value::Decimal(x), Value::Decimal(y)) => return Decision::Known(Some(x == y)),
        (Value::Duration(x), Value::Duration(y)) => return Decision::Known(Some(x == y)),
        _ => {}
    }
    if let (Some(x), Some(y)) = (numeric(a), numeric(b)) {
        return Decision::Known(Some(x == y));
    }
    match (a, b) {
        (Value::String(x), Value::String(y)) => Decision::Known(Some(x == y)),
        (Value::Bool(x), Value::Bool(y)) => Decision::Known(Some(x == y)),
        _ => Decision::JsonFallback,
    }
}

pub(super) fn checked(a: &Value, b: &Value) -> Result<Option<bool>> {
    match decision(a, b) {
        Decision::Known(result) => Ok(result),
        Decision::JsonFallback => {
            charge_intermediate_copy("converting inline property equality to JSON", || {
                conversion_bytes(a).saturating_add(conversion_bytes(b))
            })?;
            Ok(Some(value_to_json(a) == value_to_json(b)))
        }
    }
}

/// Conservative owned-buffer bounds for Value::to_json, measured by borrowing.
/// Typed arrays first clone their raw Vec and then collect JSON-sized slots;
/// Path/Graph clone nested JSON before the object macro serializes it again.
/// Decimal scale must be considered without constructing its formatted value.
fn conversion_bytes(value: &Value) -> usize {
    let json_slot = std::mem::size_of::<serde_json::Value>();
    let nested = match value {
        Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) => 0,
        Value::String(value) => value.len(),
        Value::DateTime(value) => value.as_str().len(),
        Value::Decimal(value) => (value.scale() as usize)
            .saturating_add(42)
            .saturating_mul(4)
            .saturating_add(128),
        Value::Duration(_) => 512,
        Value::StringArray(values) => values.iter().fold(
            values
                .len()
                .saturating_mul(std::mem::size_of::<String>().saturating_add(json_slot)),
            |bytes, value| bytes.saturating_add(value.len()),
        ),
        Value::IntArray(values) => values
            .len()
            .saturating_mul(std::mem::size_of::<i64>().saturating_add(json_slot)),
        Value::FloatArray(values) => values
            .len()
            .saturating_mul(std::mem::size_of::<f64>().saturating_add(json_slot)),
        Value::Path(_) | Value::Graph(_) => read_budget::value_copy_bytes(value)
            .saturating_mul(2)
            .saturating_add(1024),
        Value::Json(value) => read_budget::json_copy_bytes(value),
    };
    json_slot.saturating_add(nested)
}

#[cfg(test)]
mod tests;
