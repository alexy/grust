//! Parser-backed policy for exposing bounded, read-only GQL/Cypher surfaces.
//!
//! Applications still own authorization and graph projection. This module owns
//! language-level safety so consumers do not scan query text for keywords.

use crate::ast::{Clause, Expr, PathPattern, Query, SingleQuery};
use crate::parser::parse_query;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::read::{execute_read_query, execute_read_query_indexed};
use crate::read_budget::{MAX_RANGE_ITEMS, ReadExecutionBudgetLimits, with_budget};
use crate::{CypherParameters, CypherResultTable, gql_execution, gql_syntax};
use grust_core::TypedGraphIndex;
use grust_core::prelude::{Graph, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadQueryPolicy {
    pub max_query_bytes: usize,
    /// Maximum serialized size of the complete parameter map.
    pub max_parameter_bytes: usize,
    /// Maximum node and edge counts accepted in the projected input graph.
    pub max_graph_nodes: usize,
    pub max_graph_edges: usize,
    /// Maximum serialized size of the projected input graph.
    pub max_graph_bytes: usize,
    /// Cumulative work units spent scanning or producing candidates.
    pub max_candidate_work: usize,
    /// Cumulative bytes copied into executor-owned intermediate rows, graph
    /// bindings, and values. This limits amplification before final `LIMIT`
    /// and output-size checks run.
    pub max_intermediate_bytes: usize,
    pub max_result_rows: usize,
    /// Maximum serialized size of the returned columns and rows.
    pub max_output_bytes: usize,
    /// Maximum number of integers materialized by one `range()` invocation.
    pub max_range_items: usize,
    pub max_union_arms: usize,
    /// Maximum cumulative hop count of every path pattern.
    pub max_path_length: u64,
    /// Cooperative wall-clock deadline for parsing, execution, and encoding.
    pub max_execution_time: Duration,
    pub allow_graph_selection: bool,
    pub allow_catalog_procedures: bool,
    pub require_match: bool,
}

impl Default for ReadQueryPolicy {
    fn default() -> Self {
        Self {
            max_query_bytes: 2_000,
            max_parameter_bytes: 64 * 1024,
            max_graph_nodes: 100_000,
            max_graph_edges: 500_000,
            max_graph_bytes: 64 * 1024 * 1024,
            max_candidate_work: 1_000_000,
            max_intermediate_bytes: 256 * 1024 * 1024,
            max_result_rows: 50,
            max_output_bytes: 1024 * 1024,
            max_range_items: 10_000,
            max_union_arms: 4,
            max_path_length: 4,
            max_execution_time: Duration::from_secs(2),
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
    validate_policy(policy)?;
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
    run_bounded_read_query_with_executor(graph, None, query_text, params, policy, |query| {
        execute_read_query(graph, query, params)
    })
}

/// Execute a bounded read against the index's immutable projected graph.
///
/// Applies the same validation, input-size checks, execution budgets and output
/// checks as [`run_bounded_read_query`], including when execution falls back to
/// the reference executor. The exact graph-byte limit uses the size measured
/// when the index acquired its immutable snapshot.
/// `USE` retains the reference entrypoint's behavior: the policy controls whether
/// it is permitted; this wrapper does not resolve a different graph snapshot.
pub fn run_bounded_read_query_indexed(
    index: &TypedGraphIndex,
    query_text: &str,
    params: &CypherParameters,
    policy: &ReadQueryPolicy,
) -> Result<CypherResultTable> {
    run_bounded_read_query_with_executor(
        index.graph(),
        Some(index.serialized_graph_bytes()),
        query_text,
        params,
        policy,
        |query| execute_read_query_indexed(index, query, params),
    )
}

fn run_bounded_read_query_with_executor(
    graph: &Graph,
    indexed_graph_bytes: Option<usize>,
    query_text: &str,
    params: &CypherParameters,
    policy: &ReadQueryPolicy,
    execute: impl FnOnce(&Query) -> Result<CypherResultTable>,
) -> Result<CypherResultTable> {
    let started = Instant::now();
    let deadline = started
        .checked_add(policy.max_execution_time)
        .ok_or_else(|| gql_execution("read policy execution timeout is too large"))?;
    let query = validate_read_query(query_text, policy)?;
    ensure_before_deadline(deadline)?;
    crate::semantics::analyze(&query)?;
    ensure_graph_bounds(graph, policy)?;
    ensure_serialized_size("parameters", params, policy.max_parameter_bytes, deadline)?;
    if let Some(bytes) = indexed_graph_bytes {
        ensure_before_deadline(deadline)?;
        if bytes > policy.max_graph_bytes {
            return Err(gql_execution(format!(
                "bounded read graph exceeds {} serialized bytes",
                policy.max_graph_bytes
            )));
        }
    } else {
        ensure_serialized_size("graph", graph, policy.max_graph_bytes, deadline)?;
    }

    let limits = ReadExecutionBudgetLimits {
        max_candidate_work: policy.max_candidate_work,
        max_intermediate_bytes: policy.max_intermediate_bytes,
        max_range_items: policy.max_range_items,
        deadline,
    };
    with_budget(limits, || {
        let table = execute(&query)?;
        if table.rows.len() > policy.max_result_rows {
            return Err(gql_execution(format!(
                "query produced more than {} rows",
                policy.max_result_rows
            )));
        }
        let output = EncodedResultSize {
            columns: &table.columns,
            rows: &table.rows,
        };
        ensure_serialized_size("query output", &output, policy.max_output_bytes, deadline)?;
        Ok(table)
    })
}

#[cfg(test)]
#[path = "read_policy/indexed_tests.rs"]
mod indexed_tests;

fn validate_policy(policy: &ReadQueryPolicy) -> Result<()> {
    let positive = [
        ("max_query_bytes", policy.max_query_bytes),
        ("max_parameter_bytes", policy.max_parameter_bytes),
        ("max_graph_nodes", policy.max_graph_nodes),
        ("max_graph_edges", policy.max_graph_edges),
        ("max_graph_bytes", policy.max_graph_bytes),
        ("max_candidate_work", policy.max_candidate_work),
        ("max_intermediate_bytes", policy.max_intermediate_bytes),
        ("max_result_rows", policy.max_result_rows),
        ("max_output_bytes", policy.max_output_bytes),
        ("max_range_items", policy.max_range_items),
        ("max_union_arms", policy.max_union_arms),
    ];
    if let Some((name, _)) = positive.into_iter().find(|(_, value)| *value == 0) {
        return Err(gql_syntax(format!("read policy {name} must be positive")));
    }
    if policy.max_path_length == 0 {
        return Err(gql_syntax("read policy max_path_length must be positive"));
    }
    if policy.max_execution_time.is_zero() {
        return Err(gql_syntax(
            "read policy max_execution_time must be positive",
        ));
    }
    if policy.max_range_items > MAX_RANGE_ITEMS {
        return Err(gql_syntax(format!(
            "read policy max_range_items cannot exceed the executor maximum of {MAX_RANGE_ITEMS}"
        )));
    }
    Ok(())
}

fn ensure_graph_bounds(graph: &Graph, policy: &ReadQueryPolicy) -> Result<()> {
    if graph.nodes.len() > policy.max_graph_nodes {
        return Err(gql_execution(format!(
            "bounded read graph contains {} nodes; policy maximum is {}",
            graph.nodes.len(),
            policy.max_graph_nodes
        )));
    }
    if graph.edges.len() > policy.max_graph_edges {
        return Err(gql_execution(format!(
            "bounded read graph contains {} edges; policy maximum is {}",
            graph.edges.len(),
            policy.max_graph_edges
        )));
    }
    Ok(())
}

fn ensure_before_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        Err(gql_execution("bounded read execution timed out"))
    } else {
        Ok(())
    }
}

#[derive(Serialize)]
struct EncodedResultSize<'a> {
    columns: &'a [String],
    rows: &'a [Vec<grust_core::Value>],
}

struct LimitWriter {
    written: usize,
    maximum: usize,
    deadline: Instant,
    exceeded: bool,
    timed_out: bool,
}

impl Write for LimitWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if Instant::now() >= self.deadline {
            self.timed_out = true;
            return Err(io::Error::new(io::ErrorKind::TimedOut, "deadline elapsed"));
        }
        let Some(next) = self.written.checked_add(bytes.len()) else {
            self.exceeded = true;
            return Err(io::Error::other("serialized-size counter overflowed"));
        };
        if next > self.maximum {
            self.exceeded = true;
            return Err(io::Error::other("serialized-size limit exceeded"));
        }
        self.written = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn ensure_serialized_size<T: Serialize + ?Sized>(
    what: &str,
    value: &T,
    maximum: usize,
    deadline: Instant,
) -> Result<()> {
    let mut writer = LimitWriter {
        written: 0,
        maximum,
        deadline,
        exceeded: false,
        timed_out: false,
    };
    let encoded = serde_json::to_writer(&mut writer, value);
    if writer.timed_out {
        return Err(gql_execution("bounded read execution timed out"));
    }
    if writer.exceeded {
        return Err(gql_execution(format!(
            "bounded read {what} exceeds {maximum} serialized bytes"
        )));
    }
    encoded
        .map_err(|error| gql_execution(format!("could not measure bounded read {what}: {error}")))
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
        Some(Expr::Integer(limit))
            if limit > 0
                && usize::try_from(limit).is_ok_and(|limit| limit <= policy.max_result_rows) =>
        {
            Ok(())
        }
        _ => Err(gql_syntax(format!(
            "RETURN requires a positive literal LIMIT no larger than {}",
            policy.max_result_rows
        ))),
    }
}

fn validate_pattern(pattern: &PathPattern, policy: &ReadQueryPolicy) -> Result<()> {
    let mut total_hops = 0_u64;
    for segment in &pattern.segments {
        let segment_hops = if let Some(length) = segment.relationship.length {
            let Some(maximum) = length.max else {
                return Err(gql_syntax("unbounded variable-length paths are forbidden"));
            };
            maximum
        } else {
            1
        };
        total_hops = total_hops
            .checked_add(segment_hops)
            .ok_or_else(|| gql_syntax("path length overflowed the read policy bound"))?;
        if total_hops > policy.max_path_length {
            return Err(gql_syntax(format!(
                "path can traverse {total_hops} hops; read policy maximum is {}",
                policy.max_path_length
            )));
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
        assert!(validate_read_query("MATCH (n) RETURN n LIMIT 4294967297", &policy).is_err());
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

    #[test]
    fn path_limit_is_cumulative_across_segments() {
        let policy = ReadQueryPolicy::default();
        assert!(
            validate_read_query(
                "MATCH (a)-[*1..3]->(b)-[*1..2]->(c) RETURN c LIMIT 1",
                &policy,
            )
            .is_err()
        );
    }

    #[test]
    fn candidate_work_stops_cartesian_expansion_before_return_limit() {
        let mut builder = GraphBuilder::new();
        for index in 0..8 {
            let _ = builder.node("N", format!("n{index}")).finish();
        }
        let graph = builder.build();
        let policy = ReadQueryPolicy {
            max_candidate_work: 60,
            ..ReadQueryPolicy::default()
        };
        let error = run_bounded_read_query(
            &graph,
            "MATCH (a), (b), (c) RETURN a LIMIT 1",
            &CypherParameters::new(),
            &policy,
        )
        .unwrap_err();
        assert!(error.to_string().contains("candidate-work"));
    }

    #[test]
    fn intermediate_byte_budget_stops_deep_binding_clone_amplification() {
        let mut builder = GraphBuilder::new();
        let _ = builder
            .node("N", "large")
            .prop("payload", "x".repeat(2 * 1024))
            .finish();
        let graph = builder.build();
        let query = "MATCH (n:N) UNWIND range(1, 100) AS item RETURN item LIMIT 1";
        let policy = ReadQueryPolicy {
            max_graph_bytes: 16 * 1024,
            max_candidate_work: 10_000,
            max_intermediate_bytes: 8 * 1024,
            max_range_items: 100,
            ..ReadQueryPolicy::default()
        };

        let error = run_bounded_read_query(&graph, query, &CypherParameters::new(), &policy)
            .expect_err("deep row copies must exhaust the cumulative byte budget");
        assert!(error.to_string().contains("cumulative intermediate bytes"));
        assert!(error.to_string().contains("expanding UNWIND rows"));

        let result = crate::read::run_read_query(&graph, query, &CypherParameters::new())
            .expect("the unrestricted executor retains its existing behavior");
        assert_eq!(result.rows, vec![vec![grust_core::Value::Int(1)]]);
    }

    #[test]
    fn intermediate_byte_budget_stops_literal_projection_amplification_before_limit() {
        let mut builder = GraphBuilder::new();
        for index in 0..128 {
            let _ = builder.node("N", format!("n{index}")).finish();
        }
        let graph = builder.build();
        let literal = "x".repeat(512);
        let query = format!("MATCH (n:N) RETURN '{literal}' AS payload LIMIT 1");
        let policy = ReadQueryPolicy {
            max_query_bytes: 2_000,
            max_candidate_work: 10_000,
            // Leave room for MATCH bindings and relationship-scope framing;
            // the repeated 512-byte projection must still exhaust the budget.
            max_intermediate_bytes: 96 * 1024,
            ..ReadQueryPolicy::default()
        };

        let error = run_bounded_read_query(&graph, &query, &CypherParameters::new(), &policy)
            .expect_err("repeated literal results must exhaust the cumulative byte budget");
        assert!(
            error.to_string().contains("cumulative intermediate bytes"),
            "{error}"
        );
        assert!(
            error
                .to_string()
                .contains("materializing expression results"),
            "{error}"
        );
    }

    #[test]
    fn candidate_work_accounts_for_correlated_subquery_index_builds() {
        let mut builder = GraphBuilder::new();
        for index in 0..64 {
            let _ = builder.node("N", format!("n{index}")).finish();
        }
        let graph = builder.build();
        let policy = ReadQueryPolicy {
            max_candidate_work: 2_000,
            ..ReadQueryPolicy::default()
        };
        let error = run_bounded_read_query(
            &graph,
            "MATCH (n) CALL { MATCH (n) RETURN n AS m LIMIT 1 } RETURN n LIMIT 1",
            &CypherParameters::new(),
            &policy,
        )
        .unwrap_err();
        assert!(error.to_string().contains("candidate-work"));
        assert!(error.to_string().contains("subquery node index"));
    }

    #[test]
    fn candidate_work_accounts_for_correlated_catalog_scans() {
        let mut builder = GraphBuilder::new();
        for index in 0..64 {
            let _ = builder
                .node("N", format!("n{index}"))
                .prop("name", format!("node {index}"))
                .finish();
        }
        let graph = builder.build();
        let policy = ReadQueryPolicy {
            max_candidate_work: 2_000,
            allow_catalog_procedures: true,
            ..ReadQueryPolicy::default()
        };
        let error = run_bounded_read_query(
            &graph,
            "MATCH (n) CALL db.propertyKeys() YIELD propertyKey RETURN n LIMIT 1",
            &CypherParameters::new(),
            &policy,
        )
        .unwrap_err();
        assert!(error.to_string().contains("candidate-work"));
        assert!(error.to_string().contains("db.propertyKeys()"));
    }

    #[test]
    fn parameter_graph_output_and_range_budgets_are_enforced() {
        let mut builder = GraphBuilder::new();
        let _ = builder
            .node("N", "n")
            .prop("payload", "x".repeat(256))
            .finish();
        let graph = builder.build();
        let query = "MATCH (n) RETURN n LIMIT 1";

        let mut params = CypherParameters::new();
        params.insert("payload".into(), grust_core::Value::String("x".repeat(256)));
        let parameter_policy = ReadQueryPolicy {
            max_parameter_bytes: 32,
            ..ReadQueryPolicy::default()
        };
        assert!(
            run_bounded_read_query(&graph, query, &params, &parameter_policy)
                .unwrap_err()
                .to_string()
                .contains("parameters")
        );

        let graph_policy = ReadQueryPolicy {
            max_graph_nodes: 1,
            max_graph_bytes: 32,
            ..ReadQueryPolicy::default()
        };
        assert!(
            run_bounded_read_query(&graph, query, &CypherParameters::new(), &graph_policy,)
                .unwrap_err()
                .to_string()
                .contains("graph")
        );

        let output_policy = ReadQueryPolicy {
            max_output_bytes: 32,
            ..ReadQueryPolicy::default()
        };
        assert!(
            run_bounded_read_query(&graph, query, &CypherParameters::new(), &output_policy,)
                .unwrap_err()
                .to_string()
                .contains("query output")
        );

        let range_policy = ReadQueryPolicy {
            max_range_items: 3,
            ..ReadQueryPolicy::default()
        };
        assert!(
            run_bounded_read_query(
                &graph,
                "MATCH (n) RETURN range(1, 4) AS values LIMIT 1",
                &CypherParameters::new(),
                &range_policy,
            )
            .unwrap_err()
            .to_string()
            .contains("read policy maximum")
        );
    }

    #[test]
    fn execution_deadline_is_enforced() {
        let mut builder = GraphBuilder::new();
        let _ = builder.node("N", "n").finish();
        let policy = ReadQueryPolicy {
            max_execution_time: Duration::from_nanos(1),
            ..ReadQueryPolicy::default()
        };
        let error = run_bounded_read_query(
            &builder.build(),
            "MATCH (n) RETURN n LIMIT 1",
            &CypherParameters::new(),
            &policy,
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
    }
}
