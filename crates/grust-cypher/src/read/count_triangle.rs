//! Exact weighted triangle counts for three symmetric location arms.
//!
//! The two-hop arms are deliberately required to be separate `MATCH` clauses:
//! relationship uniqueness resets at that boundary, so a repeated person may
//! choose the same physical location path independently in each arm. The
//! closing undirected triangle is one path, and its three relationship slots
//! must therefore be distinct. Counts are grouped by endpoints and never
//! materialize one row per match.

use super::count_support::WeightedSupport;
use super::*;
use grust_core::TypedGraphIndex;
use std::mem::size_of;

mod count;
mod location;
mod plan;

use plan::Triangle;

fn arithmetic_overflow(context: &str) -> GrustError {
    gql_execution(format!(
        "count triangle arithmetic exceeds u128 while {context}"
    ))
}

pub(super) fn add(left: u128, right: u128, context: &str) -> Result<u128> {
    left.checked_add(right)
        .ok_or_else(|| arithmetic_overflow(context))
}

pub(super) fn multiply(left: u128, right: u128, context: &str) -> Result<u128> {
    left.checked_mul(right)
        .ok_or_else(|| arithmetic_overflow(context))
}

/// Compute an exact requested vector payload size. An impossible size is an
/// execution error even without a caller budget; it must not wrap into a small
/// admission charge before `Vec::with_capacity`.
pub(super) fn allocation_bytes<T>(items: usize, context: &str) -> Result<usize> {
    items.checked_mul(size_of::<T>()).ok_or_else(|| {
        gql_execution(format!(
            "count triangle allocation overflowed while {context}"
        ))
    })
}

pub(super) fn reserved_vec<T>(items: usize, context: &str) -> Result<Vec<T>> {
    read_budget::charge_intermediate_bytes(allocation_bytes::<T>(items, context)?, context)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(items)
        .map_err(|_| gql_execution(format!("count triangle allocation failed while {context}")))?;
    Ok(values)
}

/// Conservatively model comparison sorting before entering an infallible
/// standard-library comparator. `4n(log2(n) + 1)` is an intentionally padded
/// O(n log n) accounting unit, not a claim about the library's exact number of
/// comparisons. Saturation makes an unrepresentable estimate fail any finite
/// budget; checkpoints bracket the non-cooperative sort itself.
pub(super) fn sort_work(items: usize) -> usize {
    if items < 2 {
        return items;
    }
    let levels = items.checked_ilog2().map_or(1, |level| level as usize + 1);
    items.saturating_mul(levels).saturating_mul(4)
}

pub(super) fn supports(query: &Query) -> Result<bool> {
    read_budget::checkpoint()?;
    Ok(plan::plan(query)?.is_some())
}

fn scalar_table(count: u128, triangle: &Triangle<'_>) -> Result<CypherResultTable> {
    let count =
        i64::try_from(count).map_err(|_| gql_execution("count triangle result exceeds int64"))?;
    let projection = triangle.projection;
    let suppressed = plan::scalar_bound(projection.skip.as_ref()).unwrap() > 0
        || projection.limit == Some(Expr::Integer(0));
    read_budget::charge_intermediate_bytes(
        128usize.saturating_add(projection.items[0].alias.as_ref().map_or(4, String::len)),
        "shaping scalar triangle count result",
    )?;
    Ok(CypherResultTable {
        columns: vec![
            projection.items[0]
                .alias
                .clone()
                .unwrap_or_else(|| "expr".into()),
        ],
        rows: if suppressed {
            Vec::new()
        } else {
            vec![vec![Value::Int(count)]]
        },
    })
}

pub(super) fn try_execute(
    index: &TypedGraphIndex,
    query: &Query,
    _params: &CypherParameters,
) -> Result<Option<CypherResultTable>> {
    let Some(triangle) = plan::plan(query)? else {
        return Ok(None);
    };
    let graph = index.graph();
    let people = index.vertices_with_label(triangle.person_label);
    read_budget::charge_candidate_work(graph.nodes.len(), "initializing triangle person slots")?;
    let mut person_slot = reserved_vec(graph.nodes.len(), "allocating triangle person slots")?;
    person_slot.resize(graph.nodes.len(), u32::MAX);
    read_budget::charge_candidate_work(people.len(), "indexing triangle people")?;
    for (ordinal, &vertex) in people.iter().enumerate() {
        // TypedGraphIndex construction proves both vertex and label-list
        // cardinality fit u32.
        person_slot[vertex as usize] = ordinal as u32;
    }

    let locations = location::build(index, &triangle, &person_slot, people.len())?;
    let (support, loops) =
        WeightedSupport::build_with_loops(index, triangle.knows_type, people, &person_slot)?;
    let forward = support.orient(people)?;
    let mut count = 0;
    count::add_distinct(&forward, &locations, &mut count)?;
    count::add_repeated(support.edges(), &loops, &locations, &mut count)?;
    scalar_table(count, &triangle).map(Some)
}

#[cfg(test)]
#[path = "count_triangle_tests.rs"]
mod tests;
