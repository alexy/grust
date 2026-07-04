//! grust-cypher test suite, split by area from the former ~17k-line tests.rs.
//!
//! This module is a direct child of the crate root, so `use super::*` here
//! brings every crate-root item — public and private — into scope. Each area
//! submodule re-globs `use super::*`, which chains through this module so the
//! tests keep their original access to crate internals. No test was added,
//! removed, or modified by the split; the original `tests.rs` body is
//! redistributed verbatim across the submodules below.

use super::*;
use grust_memory::MemoryGraphStore;

mod catalog_metadata;
mod ddl_schema;
mod graph_type_ddl;
mod index_ddl;
mod match_misc;
mod mutations;
mod named_graph_selection;
mod predicates1;
mod predicates2;
mod returning1;
mod returning2;

/// Shared helper: true for the structured planning-error variants.
fn is_cypher_planning_error(error: &GrustError) -> bool {
    matches!(
        error,
        GrustError::CypherSyntax(_)
            | GrustError::CypherUnresolvedIdentity(_)
            | GrustError::CypherUnsupportedCardinality(_)
            | GrustError::Unsupported(_)
    )
}
