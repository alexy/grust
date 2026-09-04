//! Cooperative resource accounting for the in-memory read executor.
//!
//! The ordinary reference executor remains available without a caller policy,
//! but it still inherits the process-wide hard cap on `range()` allocation.
//! `run_bounded_read_query` installs one of these budgets for the duration of
//! parsing/execution so deeply nested expression and path helpers can account
//! work without threading a context parameter through the entire evaluator.

use std::cell::RefCell;
use std::time::Instant;

use grust_core::prelude::{Edge, Node, Props, Result, Value};

use crate::gql_execution;

/// Absolute allocation ceiling for `range()` in every reference-executor
/// entrypoint, including the intentionally unrestricted `run_read_query`.
pub const MAX_RANGE_ITEMS: usize = 1_000_000;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ReadExecutionBudgetLimits {
    pub max_candidate_work: usize,
    pub max_intermediate_bytes: usize,
    pub max_range_items: usize,
    pub deadline: Instant,
}

#[derive(Clone, Copy, Debug)]
struct ReadExecutionBudget {
    limits: ReadExecutionBudgetLimits,
    candidate_work: usize,
    intermediate_bytes: usize,
}

thread_local! {
    static BUDGETS: RefCell<Vec<ReadExecutionBudget>> = const { RefCell::new(Vec::new()) };
}

struct BudgetGuard;

impl Drop for BudgetGuard {
    fn drop(&mut self) {
        BUDGETS.with(|budgets| {
            budgets.borrow_mut().pop();
        });
    }
}

pub(crate) fn with_budget<T>(
    limits: ReadExecutionBudgetLimits,
    run: impl FnOnce() -> Result<T>,
) -> Result<T> {
    BUDGETS.with(|budgets| {
        budgets.borrow_mut().push(ReadExecutionBudget {
            limits,
            candidate_work: 0,
            intermediate_bytes: 0,
        });
    });
    let _guard = BudgetGuard;
    checkpoint()?;
    run()
}

/// Account bytes copied into executor-owned intermediate rows and values.
///
/// This is cumulative rather than a live-heap estimate: repeatedly cloning one
/// large binding is bounded even when earlier rows are subsequently dropped.
/// Calls made by the unrestricted executor remain no-ops.
pub(crate) fn charge_intermediate_bytes(bytes: usize, context: &str) -> Result<()> {
    BUDGETS.with(|budgets| {
        let mut budgets = budgets.borrow_mut();
        let Some(budget) = budgets.last_mut() else {
            return Ok(());
        };
        if Instant::now() >= budget.limits.deadline {
            return Err(gql_execution("bounded read execution timed out"));
        }
        let next = budget
            .intermediate_bytes
            .checked_add(bytes)
            .ok_or_else(|| gql_execution("bounded read intermediate-byte counter overflowed"))?;
        if next > budget.limits.max_intermediate_bytes {
            return Err(gql_execution(format!(
                "bounded read exceeded {} cumulative intermediate bytes while {context}",
                budget.limits.max_intermediate_bytes
            )));
        }
        budget.intermediate_bytes = next;
        Ok(())
    })
}

/// Check that an intermediate allocation would fit without consuming the
/// budget yet. Use this immediately before allocations whose requested
/// capacity can be computed up front; the materialized value is charged once
/// it has been produced.
pub(crate) fn check_intermediate_bytes_available(bytes: usize, context: &str) -> Result<()> {
    BUDGETS.with(|budgets| {
        let budgets = budgets.borrow();
        let Some(budget) = budgets.last() else {
            return Ok(());
        };
        if Instant::now() >= budget.limits.deadline {
            return Err(gql_execution("bounded read execution timed out"));
        }
        let next = budget
            .intermediate_bytes
            .checked_add(bytes)
            .ok_or_else(|| gql_execution("bounded read intermediate-byte counter overflowed"))?;
        if next > budget.limits.max_intermediate_bytes {
            return Err(gql_execution(format!(
                "bounded read exceeded {} cumulative intermediate bytes while {context}",
                budget.limits.max_intermediate_bytes
            )));
        }
        Ok(())
    })
}

/// Conservative byte count for cloning one core value, including owned nested
/// buffers. Saturation is intentional: an unrepresentable estimate is rejected
/// by any active finite budget before the clone occurs.
pub(crate) fn value_copy_bytes(value: &Value) -> usize {
    let nested = match value {
        Value::Null
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::Decimal(_)
        | Value::Duration(_) => 0,
        Value::String(value) => value.len(),
        Value::DateTime(value) => value.as_str().len(),
        Value::StringArray(values) => values.iter().fold(
            values.len().saturating_mul(std::mem::size_of::<String>()),
            |bytes, value| bytes.saturating_add(value.len()),
        ),
        Value::IntArray(values) => values.len().saturating_mul(std::mem::size_of::<i64>()),
        Value::FloatArray(values) => values.len().saturating_mul(std::mem::size_of::<f64>()),
        Value::Path(path) => json_values_copy_bytes(&path.nodes)
            .saturating_add(json_values_copy_bytes(&path.relationships)),
        Value::Graph(graph) => json_values_copy_bytes(&graph.nodes)
            .saturating_add(json_values_copy_bytes(&graph.relationships)),
        Value::Json(value) => json_copy_bytes(value),
    };
    std::mem::size_of::<Value>().saturating_add(nested)
}

pub(crate) fn node_copy_bytes(node: &Node) -> usize {
    std::mem::size_of::<Node>()
        .saturating_add(node.id.as_str().len())
        .saturating_add(node.label.as_str().len())
        .saturating_add(props_copy_bytes(&node.props))
}

pub(crate) fn edge_copy_bytes(edge: &Edge) -> usize {
    std::mem::size_of::<Edge>()
        .saturating_add(edge.id.as_ref().map_or(0, |id| id.as_str().len()))
        .saturating_add(edge.from.as_str().len())
        .saturating_add(edge.to.as_str().len())
        .saturating_add(edge.label.as_str().len())
        .saturating_add(props_copy_bytes(&edge.props))
}

fn props_copy_bytes(props: &Props) -> usize {
    props.iter().fold(
        props
            .len()
            .saturating_mul(std::mem::size_of::<(String, Value)>()),
        |bytes, (key, value)| {
            bytes
                .saturating_add(key.len())
                .saturating_add(value_copy_bytes(value))
        },
    )
}

fn json_values_copy_bytes(values: &[serde_json::Value]) -> usize {
    values.iter().fold(
        values
            .len()
            .saturating_mul(std::mem::size_of::<serde_json::Value>()),
        |bytes, value| bytes.saturating_add(json_copy_bytes(value)),
    )
}

pub(crate) fn json_copy_bytes(value: &serde_json::Value) -> usize {
    let nested = match value {
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => 0,
        serde_json::Value::String(value) => value.len(),
        serde_json::Value::Array(values) => json_values_copy_bytes(values),
        serde_json::Value::Object(values) => values.iter().fold(
            values
                .len()
                .saturating_mul(std::mem::size_of::<(String, serde_json::Value)>()),
            |bytes, (key, value)| {
                bytes
                    .saturating_add(key.len())
                    .saturating_add(json_copy_bytes(value))
            },
        ),
    };
    std::mem::size_of::<serde_json::Value>().saturating_add(nested)
}

/// Check the cooperative wall-clock deadline at expression and pipeline
/// boundaries. An inactive (ordinary read) executor has no deadline.
pub(crate) fn checkpoint() -> Result<()> {
    BUDGETS.with(|budgets| {
        let budgets = budgets.borrow();
        let Some(budget) = budgets.last() else {
            return Ok(());
        };
        if Instant::now() >= budget.limits.deadline {
            return Err(gql_execution("bounded read execution timed out"));
        }
        Ok(())
    })
}

pub(crate) fn intermediate_accounting_active() -> bool {
    BUDGETS.with(|budgets| !budgets.borrow().is_empty())
}

/// Account candidate rows, scanned graph elements, expanded list items, and
/// path-search steps before they are added to an intermediate allocation.
pub(crate) fn charge_candidate_work(units: usize, context: &str) -> Result<()> {
    BUDGETS.with(|budgets| {
        let mut budgets = budgets.borrow_mut();
        let Some(budget) = budgets.last_mut() else {
            return Ok(());
        };
        if Instant::now() >= budget.limits.deadline {
            return Err(gql_execution("bounded read execution timed out"));
        }
        let next = budget
            .candidate_work
            .checked_add(units)
            .ok_or_else(|| gql_execution("bounded read candidate-work counter overflowed"))?;
        if next > budget.limits.max_candidate_work {
            return Err(gql_execution(format!(
                "bounded read exceeded {} candidate-work units while {context}",
                budget.limits.max_candidate_work
            )));
        }
        budget.candidate_work = next;
        Ok(())
    })
}

/// Enforce both the universal allocation ceiling and the active caller policy
/// before a `range()` vector is allocated.
pub(crate) fn check_range_items(items: usize) -> Result<()> {
    if items > MAX_RANGE_ITEMS {
        return Err(gql_execution(format!(
            "range() would produce {items} items; the executor maximum is {MAX_RANGE_ITEMS}"
        )));
    }
    BUDGETS.with(|budgets| {
        let budgets = budgets.borrow();
        let Some(budget) = budgets.last() else {
            return Ok(());
        };
        if items > budget.limits.max_range_items {
            return Err(gql_execution(format!(
                "range() would produce {items} items; the read policy maximum is {}",
                budget.limits.max_range_items
            )));
        }
        Ok(())
    })?;
    charge_candidate_work(items, "materializing range()")
}
