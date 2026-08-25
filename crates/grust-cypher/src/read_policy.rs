//! Parser-backed policy for exposing bounded, read-only GQL/Cypher surfaces.
//!
//! Applications still own authorization and graph projection. This module owns
//! language-level safety so consumers do not scan query text for keywords.

use crate::ast::{Clause, Expr, PathPattern, Query, SingleQuery};
use crate::parser::parse_query;
use crate::read::execute_read_query;
use crate::{CypherParameters, CypherResultTable, gql_execution, gql_syntax};
use grust_core::prelude::{Graph, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadQueryPolicy {
    pub max_query_bytes: usize,
    pub max_result_rows: usize,
    pub max_union_arms: usize,
    pub max_path_length: u64,
    pub allow_graph_selection: bool,
    pub allow_catalog_procedures: bool,
    pub require_match: bool,
}

impl Default for ReadQueryPolicy {
    fn default() -> Self {
        Self {
            max_query_bytes: 2_000,
            max_result_rows: 50,
            max_union_arms: 4,
            max_path_length: 4,
            allow_graph_selection: false,
            allow_catalog_procedures: false,
            require_match: true,
        }
    }
}

/// Parse and validate a single bounded read query using Grust's source-of-truth
/// AST. Every query arm must have a positive literal `LIMIT` no larger than the
/// policy's result ceiling.
pub fn validate_read_query(query_text: &str, policy: &ReadQueryPolicy) -> Result<Query> {
    let text = query_text.trim();
    if text.is_empty() || text.len() > policy.max_query_bytes {
        return Err(gql_syntax(format!(
            "query must contain 1 to {} bytes",
            policy.max_query_bytes
        )));
    }
    let query = parse_query(text).map_err(|error| error.into_grust(text))?;
    validate_query(&query, policy)?;
    Ok(query)
}

/// Execute a parser-validated bounded read query against a projected graph.
pub fn run_bounded_read_query(
    graph: &Graph,
    query_text: &str,
    params: &CypherParameters,
    policy: &ReadQueryPolicy,
) -> Result<CypherResultTable> {
    let query = validate_read_query(query_text, policy)?;
    crate::semantics::analyze(&query)?;
    let table = execute_read_query(graph, &query, params)?;
    if table.rows.len() > policy.max_result_rows {
        return Err(gql_execution(format!(
            "query produced more than {} rows",
            policy.max_result_rows
        )));
    }
    Ok(table)
}

fn validate_query(query: &Query, policy: &ReadQueryPolicy) -> Result<()> {
    if query.parts.is_empty() || query.parts.len() > policy.max_union_arms {
        return Err(gql_syntax(format!(
            "query must contain 1 to {} UNION arms",
            policy.max_union_arms
        )));
    }
    for part in &query.parts {
        validate_single(&part.query, policy)?;
    }
    Ok(())
}

fn validate_single(query: &SingleQuery, policy: &ReadQueryPolicy) -> Result<()> {
    if policy.require_match
        && !query
            .clauses
            .iter()
            .any(|clause| matches!(clause, Clause::Match(_)))
    {
        return Err(gql_syntax("bounded read query requires MATCH"));
    }
    for clause in &query.clauses {
        if clause.is_updating() {
            return Err(gql_syntax("updating clauses are forbidden by read policy"));
        }
        match clause {
            Clause::Use(_) if !policy.allow_graph_selection => {
                return Err(gql_syntax("graph selection is forbidden by read policy"));
            }
            Clause::Call(_) if !policy.allow_catalog_procedures => {
                return Err(gql_syntax("procedure calls are forbidden by read policy"));
            }
            Clause::Subquery(subquery) => validate_query(&subquery.query, policy)?,
            Clause::Match(clause) => {
                for pattern in &clause.patterns {
                    validate_pattern(pattern, policy)?;
                }
            }
            _ => {}
        }
    }
    let Some(Clause::Return(return_clause)) = query.clauses.last() else {
        return Err(gql_syntax("bounded read query must end with RETURN"));
    };
    match return_clause.projection.limit {
        Some(Expr::Integer(limit)) if limit > 0 && limit as usize <= policy.max_result_rows => {
            Ok(())
        }
        _ => Err(gql_syntax(format!(
            "RETURN requires a positive literal LIMIT no larger than {}",
            policy.max_result_rows
        ))),
    }
}

fn validate_pattern(pattern: &PathPattern, policy: &ReadQueryPolicy) -> Result<()> {
    if pattern.segments.len() as u64 > policy.max_path_length {
        return Err(gql_syntax("fixed path exceeds the read policy bound"));
    }
    for segment in &pattern.segments {
        if let Some(length) = segment.relationship.length {
            let Some(maximum) = length.max else {
                return Err(gql_syntax("unbounded variable-length paths are forbidden"));
            };
            if maximum > policy.max_path_length {
                return Err(gql_syntax(
                    "variable-length path exceeds the read policy bound",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use grust_core::prelude::GraphBuilder;

    #[test]
    fn parser_backed_policy_executes_bounded_reads() {
        let mut builder = GraphBuilder::new();
        let _ = builder.node("Person", "ada").prop("name", "Ada").finish();
        let graph = builder.build();
        let result = run_bounded_read_query(
            &graph,
            "MATCH (n:Person) RETURN n.name AS name LIMIT 5",
            &CypherParameters::new(),
            &ReadQueryPolicy::default(),
        )
        .unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn policy_rejects_updates_and_nonliteral_or_unbounded_limits() {
        let policy = ReadQueryPolicy::default();
        assert!(validate_read_query("MATCH (n) DELETE n RETURN n LIMIT 1", &policy).is_err());
        assert!(validate_read_query("MATCH (n) RETURN n", &policy).is_err());
        assert!(validate_read_query("MATCH (n) RETURN n LIMIT $limit", &policy).is_err());
        assert!(validate_read_query("MATCH (n)-[*]->(m) RETURN m LIMIT 5", &policy).is_err());
    }

    #[test]
    fn keywords_inside_values_do_not_confuse_the_policy() {
        let policy = ReadQueryPolicy::default();
        assert!(
            validate_read_query(
                "MATCH (n {label: 'DELETE USE CREATE'}) RETURN n LIMIT 1",
                &policy
            )
            .is_ok()
        );
    }
}
