//! Hash-bucketed equality tracking for heterogeneous graph property values.
//!
//! `Value` intentionally supports floating point and JSON, so deriving `Hash`
//! would be misleading. This module computes an equality-compatible fingerprint
//! and still checks full `PartialEq` inside each bucket. Hash collisions can
//! affect performance, never uniqueness semantics.

use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
};

use crate::Value;

pub(crate) struct UniqueValues<'value, Owner> {
    buckets: HashMap<u64, Vec<(Owner, &'value Value)>>,
}

impl<'value, Owner> UniqueValues<'value, Owner> {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            buckets: HashMap::with_capacity(capacity),
        }
    }

    pub(crate) fn insert<'set>(
        &'set mut self,
        owner: Owner,
        value: &'value Value,
    ) -> Option<&'set Owner> {
        let bucket = self.buckets.entry(value_fingerprint(value)).or_default();
        if let Some(index) = bucket.iter().position(|(_, existing)| *existing == value) {
            return Some(&bucket[index].0);
        }
        bucket.push((owner, value));
        None
    }
}

fn value_fingerprint(value: &Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_value(value, &mut hasher);
    hasher.finish()
}

fn hash_value(value: &Value, state: &mut impl Hasher) {
    std::mem::discriminant(value).hash(state);
    match value {
        Value::Null => {}
        Value::Bool(value) => value.hash(state),
        Value::Int(value) => value.hash(state),
        Value::Float(value) => hash_float(*value, state),
        Value::String(value) => value.hash(state),
        Value::DateTime(value) => value.hash(state),
        Value::Decimal(value) => value.hash(state),
        Value::Duration(value) => value.hash(state),
        Value::StringArray(values) => values.hash(state),
        Value::IntArray(values) => values.hash(state),
        Value::FloatArray(values) => {
            values.len().hash(state);
            for value in values {
                hash_float(*value, state);
            }
        }
        Value::Path(path) => {
            hash_json_slice(&path.nodes, state);
            hash_json_slice(&path.relationships, state);
        }
        Value::Graph(graph) => {
            hash_json_slice(&graph.nodes, state);
            hash_json_slice(&graph.relationships, state);
        }
        Value::Json(value) => hash_json(value, state),
    }
}

fn hash_float(value: f64, state: &mut impl Hasher) {
    // `-0.0 == 0.0`, so equal zeros must share a fingerprint. NaNs are never
    // equal under `PartialEq`; their payload bits may safely remain distinct.
    let bits = if value == 0.0 { 0 } else { value.to_bits() };
    bits.hash(state);
}

fn hash_json_slice(values: &[serde_json::Value], state: &mut impl Hasher) {
    values.len().hash(state);
    for value in values {
        hash_json(value, state);
    }
}

fn hash_json(value: &serde_json::Value, state: &mut impl Hasher) {
    std::mem::discriminant(value).hash(state);
    match value {
        serde_json::Value::Null => {}
        serde_json::Value::Bool(value) => value.hash(state),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                0_u8.hash(state);
                value.hash(state);
            } else if let Some(value) = value.as_u64() {
                1_u8.hash(state);
                value.hash(state);
            } else if let Some(value) = value.as_f64() {
                2_u8.hash(state);
                hash_float(value, state);
            }
        }
        serde_json::Value::String(value) => value.hash(state),
        serde_json::Value::Array(values) => hash_json_slice(values, state),
        serde_json::Value::Object(values) => {
            // JSON object equality is independent of map iteration order. Hash
            // each entry separately and sort the fingerprints to preserve that
            // contract even when serde_json's `preserve_order` feature is active.
            let mut entries = Vec::with_capacity(values.len());
            for (key, value) in values {
                let mut entry = DefaultHasher::new();
                key.hash(&mut entry);
                hash_json(value, &mut entry);
                entries.push(entry.finish());
            }
            entries.sort_unstable();
            entries.hash(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_preserve_value_equality_edge_cases() {
        assert_eq!(
            value_fingerprint(&Value::Float(-0.0)),
            value_fingerprint(&Value::Float(0.0))
        );

        let left = Value::Json(serde_json::json!({"a": 1, "b": [2, 3]}));
        let right = Value::Json(serde_json::json!({"b": [2, 3], "a": 1}));
        assert_eq!(left, right);
        assert_eq!(value_fingerprint(&left), value_fingerprint(&right));
    }

    #[test]
    fn collisions_still_use_full_value_equality() {
        let mut values = UniqueValues::with_capacity(2);
        assert_eq!(values.insert("first", &Value::Int(1)), None);
        assert_eq!(values.insert("second", &Value::Int(2)), None);
        assert_eq!(values.insert("duplicate", &Value::Int(1)), Some(&"first"));
    }
}
