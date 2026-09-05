//! Indexed entrypoints and the structural plan classification shared with them.

use super::*;
use grust_core::TypedGraphIndex;

/// The execution route selected for an indexed read query.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexedReadPlan {
    /// The existing reference executor handles the query.
    ClausePipeline,
    /// Proven count algebra executes without materializing matching rows.
    CountFactorized,
}

/// Classify an already-parsed, semantically valid query using the executor's
/// complete eligibility proof, including cycle detection.
///
/// This is structural classification, not policy validation or execution. It
/// does not cache a plan or waive later planning costs. Any active read budget
/// charges this classification; execution independently plans under its own
/// active budget. Unsupported shapes select the existing clause pipeline.
pub fn classify_indexed_read_query(query: &Query) -> Result<IndexedReadPlan> {
    read_budget::checkpoint()?;
    Ok(
        if count_scan::supports(query)?
            || count_tree::supports(query)?
            || count_wedge::supports(query)?
            || count_tags::supports(query)?
            || count_cycle::supports(query)?
            || count_triangle::supports(query)?
        {
            IndexedReadPlan::CountFactorized
        } else {
            IndexedReadPlan::ClausePipeline
        },
    )
}

/// Parse, analyze, and execute a read query against the index's immutable graph.
///
/// Like [`run_read_query`], this validates default graph selection and semantic
/// bindings. Parsing, analysis, planning, and execution all happen inside this
/// call, even if a caller classified the query earlier. Use the bounded indexed
/// entrypoint in [`crate::read_policy`] when a caller policy is required.
pub fn run_read_query_indexed(
    index: &TypedGraphIndex,
    cypher: &str,
    params: &CypherParameters,
) -> Result<CypherResultTable> {
    let query = parse_query(cypher).map_err(|error| error.into_grust(cypher))?;
    ensure_query_uses_graph(&query, "default")?;
    crate::semantics::analyze(&query)?;
    execute_read_query_indexed(index, &query, params)
}

/// Execute a parsed query using a reusable immutable snapshot index.
/// Unsupported fast-plan shapes retain the existing reference execution path.
/// Like [`execute_read_query`], this is not a policy-validation entrypoint.
pub fn execute_read_query_indexed(
    index: &TypedGraphIndex,
    query: &Query,
    params: &CypherParameters,
) -> Result<CypherResultTable> {
    read_budget::checkpoint()?;
    if let Some(table) = count_scan::try_execute(index, query, params)? {
        return Ok(table);
    }
    if let Some(table) = count_tree::try_execute(index, query, params)? {
        return Ok(table);
    }
    if let Some(table) = count_wedge::try_execute(index, query)? {
        return Ok(table);
    }
    if let Some(table) = count_tags::try_execute(index, query, params)? {
        return Ok(table);
    }
    if let Some(table) = count_cycle::try_execute(index, query, params)? {
        return Ok(table);
    }
    if let Some(table) = count_triangle::try_execute(index, query, params)? {
        return Ok(table);
    }
    execute_read_query(index.graph(), query, params)
}

#[cfg(test)]
#[path = "indexed_tests.rs"]
mod tests;
