//! Physical relationship identity carried only during one MATCH expansion.
//! It is never a binding, never visible to RETURN *, and resets at each MATCH.

use super::*;
use std::ops::{Deref, DerefMut};

pub(super) struct MatchRow {
    bindings: Row,
    slots: Vec<usize>,
}

impl Deref for MatchRow {
    type Target = Row;
    fn deref(&self) -> &Row {
        &self.bindings
    }
}

impl DerefMut for MatchRow {
    fn deref_mut(&mut self) -> &mut Row {
        &mut self.bindings
    }
}

impl MatchRow {
    pub(super) fn copy(&self, context: &str) -> Result<Self> {
        charge_slots(self.slots.len(), context)?;
        Ok(Self {
            bindings: clone_row(&self.bindings, context)?,
            slots: self.slots.clone(),
        })
    }

    pub(super) fn slots(&self) -> &[usize] {
        &self.slots
    }

    pub(super) fn contains(&self, slot: usize) -> Result<bool> {
        contains(&self.slots, slot)
    }

    pub(super) fn disjoint(&self, slots: &[usize]) -> Result<bool> {
        for &slot in slots {
            if self.contains(slot)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn record(&mut self, slots: &[usize]) -> Result<()> {
        reserve_slot_copy(
            &mut self.slots,
            slots.len(),
            "recording MATCH relationship identities",
        )?;
        self.slots.extend_from_slice(slots);
        Ok(())
    }
}

pub(super) fn contains(slots: &[usize], slot: usize) -> Result<bool> {
    read_budget::charge_candidate_work(slots.len(), "checking MATCH relationship uniqueness")?;
    Ok(slots.contains(&slot))
}

/// Reusing a named fixed relationship preserves physical identity across
/// MATCH/WITH, even when IDs are missing or duplicated. A NULL binding cannot
/// match an edge; graph-free pushed bindings cannot safely be rebound.
pub(super) fn fixed_binding_matches(row: &Row, name: Option<&str>, slot: usize) -> Result<bool> {
    let Some(name) = name else { return Ok(true) };
    read_budget::charge_candidate_work(1, "checking bound relationship identity")?;
    match row.get(name) {
        None => Ok(true),
        Some(Bound::Edge(_, Some(bound))) => Ok(*bound == slot),
        Some(Bound::Value(Value::Null)) => Ok(false),
        _ => Err(gql_execution(
            "cannot rebind a relationship without physical edge provenance",
        )),
    }
}

/// Existing list-shaped relationship values do not retain physical slot
/// provenance. Refuse repeated bindings rather than silently overwriting them.
pub(super) fn require_unbound_trail(row: &Row, name: Option<&str>) -> Result<()> {
    if let Some(name) = name {
        read_budget::charge_candidate_work(1, "checking variable-length relationship binding")?;
        if row.contains_key(name) {
            return Err(gql_execution(
                "rebinding a variable-length or shortest-path relationship list is unsupported",
            ));
        }
    }
    Ok(())
}

fn charge_slots(count: usize, context: &str) -> Result<()> {
    read_budget::charge_intermediate_bytes(
        count.saturating_mul(std::mem::size_of::<usize>()),
        context,
    )
}

fn reserve_slot_copy(slots: &mut Vec<usize>, additional: usize, context: &str) -> Result<()> {
    // Charge each copied slot once, before allocation or mutation. Request only
    // the necessary growth, avoiding Vec::push's geometric spare capacity.
    charge_slots(additional, context)?;
    slots.reserve_exact(additional);
    Ok(())
}

pub(super) fn begin(rows: Vec<Row>) -> Result<Vec<MatchRow>> {
    read_budget::charge_candidate_work(rows.len(), "starting MATCH relationship scope")?;
    read_budget::charge_intermediate_bytes(
        rows.len().saturating_mul(std::mem::size_of::<MatchRow>()),
        "starting MATCH relationship scope",
    )?;
    Ok(rows
        .into_iter()
        .map(|bindings| MatchRow {
            bindings,
            slots: Vec::new(),
        })
        .collect())
}

pub(super) fn finish(rows: Vec<MatchRow>) -> Result<Vec<Row>> {
    read_budget::charge_candidate_work(rows.len(), "finishing MATCH relationship scope")?;
    read_budget::charge_intermediate_bytes(
        rows.len().saturating_mul(std::mem::size_of::<Row>()),
        "finishing MATCH relationship scope",
    )?;
    Ok(rows.into_iter().map(|row| row.bindings).collect())
}

#[derive(Default)]
pub(super) struct EdgeTrail {
    pub(super) edges: Vec<Edge>,
    pub(super) slots: Vec<usize>,
}

impl EdgeTrail {
    pub(super) fn push(&mut self, slot: usize, edge: &Edge) -> Result<()> {
        reserve_slot_copy(
            &mut self.slots,
            1,
            "tracking variable-length relationship identities",
        )?;
        self.edges
            .push(clone_edge(edge, "searching variable-length paths")?);
        self.slots.push(slot);
        Ok(())
    }

    pub(super) fn pop(&mut self) {
        self.edges.pop();
        self.slots.pop();
    }

    pub(super) fn copy(&self) -> Result<Self> {
        charge_slots(
            self.slots.len(),
            "collecting variable-length relationship identities",
        )?;
        Ok(Self {
            edges: clone_edges(&self.edges, "collecting variable-length path edges")?,
            slots: self.slots.clone(),
        })
    }
}

#[cfg(test)]
mod tests;
