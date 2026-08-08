//! Hash-bucketed equality tracking for heterogeneous graph property values.
//!
//! `Value` intentionally supports floating point and JSON, so deriving `Hash`
//! would be misleading. This module computes an equality-compatible fingerprint
//! and still checks full `PartialEq` inside each bucket. Hash collisions can
//! affect performance, never uniqueness semantics.

use std::{
    borrow::Borrow,
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
};

use crate::Value;

/// Collision-safe owner index for graph [`Value`]s.
///
/// `Value` deliberately cannot implement `Eq + Hash` because floats and JSON
/// retain their native equality semantics. This index uses an
/// equality-compatible fingerprint only to select a small bucket, then always
/// confirms uniqueness with full `Value::eq`. `StoredValue` may be either an
/// owned [`Value`] for a persistent index or `&Value` for one validation pass.
#[derive(Clone, Debug)]
pub struct UniqueValueIndex<Owner, StoredValue> {
    buckets: HashMap<u64, Vec<(Owner, StoredValue)>>,
}

impl<Owner, StoredValue> UniqueValueIndex<Owner, StoredValue>
where
    StoredValue: Borrow<Value>,
{
    /// Create an empty index sized for approximately `capacity` distinct
    /// values.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buckets: HashMap::with_capacity(capacity),
        }
    }

    /// Return the owner of an equal value, if one is already indexed.
    pub fn get(&self, value: &Value) -> Option<&Owner> {
        self.buckets
            .get(&value_fingerprint(value))?
            .iter()
            .find_map(|(owner, existing)| (existing.borrow() == value).then_some(owner))
    }

    /// Index one owner/value pair and return a previously indexed owner of an
    /// equal value, if any.
    ///
    /// The new pair is retained even when a conflict exists. Keeping the index
    /// structurally complete lets mutable stores recover correctly after one
    /// owner of a temporarily duplicated value is replaced or removed.
    pub fn insert(&mut self, owner: Owner, value: StoredValue) -> Option<&Owner> {
        let bucket = self
            .buckets
            .entry(value_fingerprint(value.borrow()))
            .or_default();
        let existing = bucket
            .iter()
            .position(|(_, existing)| existing.borrow() == value.borrow());
        bucket.push((owner, value));
        existing.map(|index| &bucket[index].0)
    }
}

impl<Owner, StoredValue> UniqueValueIndex<Owner, StoredValue>
where
    Owner: PartialEq,
    StoredValue: Borrow<Value>,
{
    /// Return an equal value's owner other than `owner`, if one is indexed.
    pub fn conflicting_owner(&self, owner: &Owner, value: &Value) -> Option<&Owner> {
        self.buckets
            .get(&value_fingerprint(value))?
            .iter()
            .find_map(|(existing_owner, existing)| {
                (existing_owner != owner && existing.borrow() == value).then_some(existing_owner)
            })
    }

    /// Remove `owner` from the fingerprint bucket for `value`.
    ///
    /// An owner represents at most one value in a set. Matching the owner is
    /// intentional: IEEE NaNs are not equal to themselves but must still be
    /// removable when an indexed record is replaced or deleted.
    pub fn remove(&mut self, owner: &Owner, value: &Value) -> bool {
        let fingerprint = value_fingerprint(value);
        let Some(bucket) = self.buckets.get_mut(&fingerprint) else {
            return false;
        };
        let Some(index) = bucket
            .iter()
            .position(|(existing_owner, _)| existing_owner == owner)
        else {
            return false;
        };
        bucket.swap_remove(index);
        if bucket.is_empty() {
            self.buckets.remove(&fingerprint);
        }
        true
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
        let mut values = UniqueValueIndex::with_capacity(2);
        assert_eq!(values.insert("first", &Value::Int(1)), None);
        assert_eq!(values.insert("second", &Value::Int(2)), None);
        assert_eq!(values.insert("duplicate", &Value::Int(1)), Some(&"first"));
        assert!(values.remove(&"first", &Value::Int(1)));
        assert_eq!(values.get(&Value::Int(1)), Some(&"duplicate"));
    }

    #[test]
    fn owned_values_can_be_removed_by_owner_even_when_not_self_equal() {
        let nan = Value::Float(f64::NAN);
        let mut values = UniqueValueIndex::with_capacity(1);
        assert_eq!(values.insert("owner", nan.clone()), None);
        assert!(values.remove(&"owner", &nan));
        assert_eq!(values.insert("replacement", nan), None);
    }
}
