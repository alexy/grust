//! Internal WITH keys preserve the identity of bare relationship bindings.
//! Computed expressions and graph-free bindings retain their existing JSON
//! value keys; no physical-slot marker is exposed in public result values.

use super::*;

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub(super) struct Key(Vec<Part>);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Part {
    EdgeSlot(usize),
    Value(String),
}

impl Key {
    fn with_capacity(count: usize, context: &str) -> Result<Self> {
        read_budget::charge_candidate_work(count, context)?;
        read_budget::charge_intermediate_bytes(
            count.saturating_mul(std::mem::size_of::<Part>()),
            context,
        )?;
        Ok(Self(Vec::with_capacity(count)))
    }

    pub(super) fn copy(&self, context: &str) -> Result<Self> {
        read_budget::charge_intermediate_bytes(
            self.0.iter().fold(
                self.0.len().saturating_mul(std::mem::size_of::<Part>()),
                |bytes, part| match part {
                    Part::EdgeSlot(_) => bytes,
                    Part::Value(value) => bytes.saturating_add(value.len()),
                },
            ),
            context,
        )?;
        Ok(self.clone())
    }
}

fn value_part(value: Value, context: &str) -> Result<Part> {
    Ok(Part::Value(return_row_key(&[value], context)?))
}

fn edge_part(bound: Option<&Bound>) -> Option<Part> {
    match bound {
        Some(Bound::Edge(_, Some(slot))) => Some(Part::EdgeSlot(*slot)),
        _ => None,
    }
}

pub(super) fn grouping(items: &[&ReturnItem], row: &Row, params: &CypherParameters) -> Result<Key> {
    let mut key = Key::with_capacity(items.len(), "building WITH grouping keys")?;
    for item in items {
        let part = match &item.expr {
            Expr::Variable(name) => edge_part(row.get(name)),
            _ => None,
        };
        key.0.push(match part {
            Some(part) => part,
            None => value_part(eval(&item.expr, row, params)?, "WITH GROUP BY")?,
        });
    }
    Ok(key)
}

pub(super) fn bindings(row: &Row) -> Result<Key> {
    let mut key = Key::with_capacity(row.len(), "building WITH DISTINCT keys")?;
    for bound in row.values() {
        key.0.push(match edge_part(Some(bound)) {
            Some(part) => part,
            None => value_part(bound_value(bound)?, "WITH DISTINCT")?,
        });
    }
    Ok(key)
}

#[cfg(test)]
mod tests;
