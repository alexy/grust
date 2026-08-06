//! Fixed resource budgets for bounded cognition execution.

use typesec_memory::{CognitionApplyError, CognitionSourceBudget, RecalledMemory};

use super::CognitionError;

#[cfg(feature = "sail")]
pub(super) const MAX_ARROW_BYTES: usize = grust_sail::MAX_ARROW_IPC_PAYLOAD_BYTES;
#[cfg(any(feature = "sail", test))]
pub(super) const MAX_RESULT_CHUNKS: usize = 256;
#[cfg(any(feature = "sail", test))]
pub(super) const MAX_RESULT_ROWS: usize = 100_000;
pub(super) const MAX_RECONCILE_COMPARISONS: usize = 100_000;

/// Enforce the public engine's ID+text budget after TypeSec has released
/// recalled views. TypeSec separately bounds the complete authorized-record
/// envelope before constructing those opaque inputs.
pub(super) fn check_authorized_input(memories: &[RecalledMemory]) -> Result<(), CognitionError> {
    let mut budget = CognitionSourceBudget::new();
    memories.iter().try_for_each(|memory| {
        budget
            .try_add(memory.id.as_str(), &memory.content.text)
            .map_err(source_budget_error)
    })
}

pub(super) fn check_reconcile_work(source_count: usize) -> Result<(), CognitionError> {
    let comparisons = source_count
        .checked_mul(source_count.saturating_sub(1))
        .map(|value| value / 2);
    check(
        comparisons.is_some_and(|value| value <= MAX_RECONCILE_COMPARISONS),
        "local reconcile work",
    )
}

#[cfg(feature = "sail")]
pub(super) fn check_arrow_bytes(bytes: usize) -> Result<(), CognitionError> {
    check(bytes <= MAX_ARROW_BYTES, "Arrow bytes")
}

#[cfg(any(feature = "sail", test))]
pub(super) fn check_result_chunks(chunks: usize) -> Result<(), CognitionError> {
    check(chunks <= MAX_RESULT_CHUNKS, "Sail result chunks")
}

#[cfg(any(feature = "sail", test))]
pub(super) fn check_result_rows(rows: usize) -> Result<(), CognitionError> {
    check(rows <= MAX_RESULT_ROWS, "Sail result rows")
}

fn check(allowed: bool, resource: &'static str) -> Result<(), CognitionError> {
    if allowed {
        Ok(())
    } else {
        Err(CognitionError::ResourceBudgetExceeded(resource))
    }
}

fn source_budget_error(_: CognitionApplyError) -> CognitionError {
    CognitionError::ResourceBudgetExceeded("authorized source input")
}

#[cfg(test)]
mod tests;
