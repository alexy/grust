//! Result-row materialization and grouped-aggregate evaluation (extracted from lib.rs).

use crate::*;

pub(crate) fn cypher_mutation_result_from_plan(
    report: CypherMutationReport,
    generated_node_ids: Vec<CypherGeneratedNodeId>,
    plan: &GraphMutationPlan,
    options: &CypherMutationOptions,
    row_edge_bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
) -> Result<CypherMutationResult> {
    let mut written_node_identities = Vec::new();
    let mut written_edge_identities = Vec::new();
    for operation in &plan.operations {
        if options.collect_written_node_identities
            && let GraphMutationPlanOp::UpsertNode { kind, node } = operation
        {
            written_node_identities.push(cypher_written_node_identity(*kind, node));
        }
        if options.collect_written_edge_identities {
            match operation {
                GraphMutationPlanOp::UpsertEdge { kind, edge } => {
                    written_edge_identities.push(cypher_written_edge_identity(*kind, edge));
                }
                GraphMutationPlanOp::UpsertEdgesFromNodeMatches { .. } => {}
                _ => {}
            }
        }
    }
    if options.collect_written_edge_identities {
        for (variable, binding) in row_edge_bindings {
            let Some(edges) = row_edge_values.get(variable) else {
                continue;
            };
            written_edge_identities.extend(
                edges
                    .iter()
                    .map(|edge| cypher_written_edge_identity(binding.kind, edge)),
            );
        }
    }
    Ok(CypherMutationResult {
        report,
        generated_node_ids,
        written_node_identities,
        written_edge_identities,
    })
}

/// Restricted row table produced by writable Cypher execution before `RETURN`
/// projection.
///
/// This model deliberately contains only variables whose values are already
/// owned by the write path:
///
/// - concrete node and relationship variables are tracked separately by the
///   resolved mutation plan;
/// - row-node variables come from broad `MATCH ... SET/REMOVE/DELETE` rows or
///   endpoint-aligned row-producing relationship writes;
/// - row-edge variables come from broad relationship mutations or
///   row-producing relationship writes.
/// - row-path variables are assembled from aligned row-node and row-edge
///   variables produced by one row-producing relationship write.
///
/// It is not a general Cypher read-query row model. Keeping this vocabulary
/// explicit prevents restricted writable `RETURN` from growing path or
/// arbitrary read-query semantics accidentally.
pub(crate) struct CypherWriteResultRows<'a> {
    pub(crate) row_nodes: &'a HashMap<String, Vec<Node>>,
    pub(crate) row_edges: &'a HashMap<String, Vec<Edge>>,
    pub(crate) row_paths: &'a HashMap<String, CypherRowProducedPathBinding>,
}

/// Shared inputs and identity caches for one writable `RETURN` evaluation.
///
/// Keeping these values together makes the aggregate helpers consume one
/// coherent row scope and prevents their signatures from drifting as new
/// projection sources are added.
pub(crate) struct CypherReturnEvaluation<'a, S> {
    pub(crate) store: &'a S,
    pub(crate) node_bindings: &'a HashMap<String, NodeId>,
    pub(crate) edge_bindings: &'a HashMap<String, CypherBoundEdgeIdentity>,
    pub(crate) row_node_values: &'a HashMap<String, Vec<Node>>,
    pub(crate) row_edge_values: &'a HashMap<String, Vec<Edge>>,
    pub(crate) row_path_bindings: &'a HashMap<String, CypherRowProducedPathBinding>,
    pub(crate) nodes: &'a mut HashMap<String, Node>,
    pub(crate) edges: &'a mut HashMap<String, Edge>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CypherWriteResultBindingKind {
    Node,
    Edge,
    Path,
}

impl<'a> CypherWriteResultRows<'a> {
    pub(crate) fn new(
        row_nodes: &'a HashMap<String, Vec<Node>>,
        row_edges: &'a HashMap<String, Vec<Edge>>,
        row_paths: &'a HashMap<String, CypherRowProducedPathBinding>,
    ) -> Self {
        Self {
            row_nodes,
            row_edges,
            row_paths,
        }
    }

    pub(crate) fn row_nodes(&self, variable: &str) -> Option<&'a [Node]> {
        self.row_nodes.get(variable).map(Vec::as_slice)
    }

    pub(crate) fn row_edges(&self, variable: &str) -> Option<&'a [Edge]> {
        self.row_edges.get(variable).map(Vec::as_slice)
    }

    pub(crate) fn path_row_count(&self, variable: &str) -> Result<usize> {
        let path = self.row_paths.get(variable).ok_or_else(|| {
            cypher_unsupported_cardinality(format!(
                "writable Cypher RETURN cannot materialize path variable '{variable}'"
            ))
        })?;
        let mut row_count = None;
        if let Some(edges) = self.row_edges(&path.edge_variable) {
            Self::merge_row_count(&mut row_count, edges.len())?;
        }
        for endpoint in [&path.from_variable, &path.to_variable] {
            if let Some(nodes) = self.row_nodes(endpoint) {
                Self::merge_row_count(&mut row_count, nodes.len())?;
            }
        }
        Ok(row_count.unwrap_or(1))
    }

    pub(crate) fn binding_kind(&self, variable: &str) -> Option<CypherWriteResultBindingKind> {
        if self.row_nodes.contains_key(variable) {
            Some(CypherWriteResultBindingKind::Node)
        } else if self.row_edges.contains_key(variable) {
            Some(CypherWriteResultBindingKind::Edge)
        } else if self.row_paths.contains_key(variable) {
            Some(CypherWriteResultBindingKind::Path)
        } else {
            None
        }
    }

    pub(crate) fn variable_names(&self) -> BTreeSet<String> {
        self.row_nodes
            .keys()
            .chain(self.row_edges.keys())
            .chain(self.row_paths.keys())
            .cloned()
            .collect()
    }

    pub(crate) fn row_count_for_return(&self, return_clause: &CypherReturnClause) -> Result<usize> {
        let mut row_count = None;
        for projection in &return_clause.projections {
            let count = match projection.element {
                CypherReturnElement::RowNode => self
                    .row_nodes(&projection.variable)
                    .map(<[Node]>::len)
                    .ok_or_else(|| {
                        cypher_unsupported_cardinality(format!(
                            "writable Cypher RETURN cannot materialize matched node variable '{}'",
                            projection.variable
                        ))
                    })?,
                CypherReturnElement::RowEdge => self
                    .row_edges(&projection.variable)
                    .map(<[Edge]>::len)
                    .ok_or_else(|| {
                        cypher_unsupported_cardinality(format!(
                            "writable Cypher RETURN cannot materialize row-producing relationship variable '{}'",
                            projection.variable
                        ))
                    })?,
                CypherReturnElement::RowPath => {
                    self.path_row_count(&projection.variable)?
                }
                CypherReturnElement::Node
                | CypherReturnElement::Edge
                | CypherReturnElement::Literal
                | CypherReturnElement::Aggregate => continue,
            };
            Self::merge_row_count(&mut row_count, count)?;
        }
        if let Some(count) = row_count {
            return Ok(count);
        }
        if let Some(count) = self.materialized_row_count()? {
            return Ok(count);
        }
        Ok(1)
    }

    fn materialized_row_count(&self) -> Result<Option<usize>> {
        let mut row_count = None;
        for count in self
            .row_nodes
            .values()
            .map(Vec::len)
            .chain(self.row_edges.values().map(Vec::len))
        {
            Self::merge_row_count(&mut row_count, count)?;
        }
        Ok(row_count)
    }

    fn merge_row_count(row_count: &mut Option<usize>, count: usize) -> Result<()> {
        if row_count.is_some_and(|current| current != count) {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN does not support mixing row-producing variables with different row counts",
            ));
        }
        *row_count = Some(count);
        Ok(())
    }
}

pub async fn evaluate_cypher_return_table<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    return_clause: &CypherReturnClause,
) -> Result<CypherResultTable>
where
    S: GraphStore + Sync,
{
    let columns = return_clause
        .projections
        .iter()
        .map(|projection| projection.column.clone())
        .collect::<Vec<_>>();
    let write_rows =
        CypherWriteResultRows::new(row_node_values, row_edge_values, row_path_bindings);
    let row_count = write_rows.row_count_for_return(return_clause)?;
    let mut nodes = HashMap::new();
    let mut edges = HashMap::new();
    if return_clause
        .projections
        .iter()
        .all(|projection| projection.element == CypherReturnElement::Aggregate)
    {
        let mut evaluation = CypherReturnEvaluation {
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes: &mut nodes,
            edges: &mut edges,
        };
        let mut row = Vec::with_capacity(return_clause.projections.len());
        for projection in &return_clause.projections {
            let value = evaluate_return_aggregate(&mut evaluation, projection, row_count).await?;
            row.push(value);
        }
        let mut rows = vec![row];
        if return_clause.distinct {
            apply_return_distinct(&mut rows)?;
        }
        apply_return_control(
            &mut rows,
            &return_clause.order_by,
            return_clause.skip,
            return_clause.limit,
        );
        return Ok(CypherResultTable { columns, rows });
    }
    if return_clause
        .projections
        .iter()
        .any(|projection| projection.element == CypherReturnElement::Aggregate)
    {
        return evaluate_grouped_cypher_return_table(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            return_clause,
            columns,
            row_count,
            &mut nodes,
            &mut edges,
        )
        .await;
    }
    let mut rows = Vec::with_capacity(row_count);
    for row_index in 0..row_count {
        let mut row = Vec::with_capacity(return_clause.projections.len());
        for projection in &return_clause.projections {
            row.push(
                evaluate_scalar_return_projection(
                    store,
                    node_bindings,
                    edge_bindings,
                    row_node_values,
                    row_edge_values,
                    row_path_bindings,
                    &mut nodes,
                    &mut edges,
                    projection,
                    row_index,
                )
                .await?,
            );
        }
        rows.push(row);
    }
    if return_clause.distinct {
        apply_return_distinct(&mut rows)?;
    }
    apply_return_control(
        &mut rows,
        &return_clause.order_by,
        return_clause.skip,
        return_clause.limit,
    );
    Ok(CypherResultTable { columns, rows })
}

pub(crate) struct CypherReturnGroup {
    pub(crate) scalar_values: Vec<Value>,
    pub(crate) aggregate_states: Vec<Option<CypherGroupedAggregateState>>,
}

pub(crate) enum CypherGroupedAggregateState {
    Count {
        count: usize,
        distinct: Option<BTreeSet<String>>,
    },
    Values {
        aggregate: CypherReturnAggregate,
        values: Vec<Value>,
        distinct: bool,
    },
}

impl CypherGroupedAggregateState {
    pub(crate) fn new(projection: &CypherReturnProjection) -> Result<Self> {
        let aggregate = projection.aggregate.ok_or_else(|| {
            cypher_unsupported_cardinality("RETURN aggregate projection is missing aggregate kind")
        })?;
        Ok(match aggregate {
            CypherReturnAggregate::Count => CypherGroupedAggregateState::Count {
                count: 0,
                distinct: projection.distinct.then(BTreeSet::new),
            },
            CypherReturnAggregate::Sum
            | CypherReturnAggregate::Avg
            | CypherReturnAggregate::Min
            | CypherReturnAggregate::Max
            | CypherReturnAggregate::Collect => CypherGroupedAggregateState::Values {
                aggregate,
                values: Vec::new(),
                distinct: projection.distinct,
            },
        })
    }

    fn record(&mut self, values: Vec<Value>) -> Result<()> {
        match self {
            CypherGroupedAggregateState::Count { count, distinct } => {
                if let Some(seen) = distinct {
                    for value in values {
                        let key = serde_json::to_string(&value.to_json()).map_err(|err| {
                            GrustError::CypherExecution(format!(
                                "RETURN COUNT(DISTINCT) value serialization failed: {err}"
                            ))
                        })?;
                        if seen.insert(key) {
                            *count += 1;
                        }
                    }
                } else {
                    *count += values.len();
                }
            }
            CypherGroupedAggregateState::Values {
                values: aggregate_values,
                ..
            } => {
                aggregate_values.extend(values);
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<Value> {
        match self {
            CypherGroupedAggregateState::Count { count, .. } => count_value(count),
            CypherGroupedAggregateState::Values {
                aggregate,
                mut values,
                distinct,
            } => {
                if distinct {
                    values = distinct_return_values(values)?;
                }
                evaluate_non_count_aggregate(aggregate, values)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn evaluate_grouped_cypher_return_table<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    return_clause: &CypherReturnClause,
    columns: Vec<String>,
    row_count: usize,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
) -> Result<CypherResultTable>
where
    S: GraphStore + Sync,
{
    let scalar_indices = return_clause
        .projections
        .iter()
        .enumerate()
        .filter_map(|(index, projection)| {
            (projection.element != CypherReturnElement::Aggregate).then_some(index)
        })
        .collect::<Vec<_>>();
    if scalar_indices.is_empty() {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher grouped RETURN requires at least one scalar projection",
        ));
    }

    let mut group_indexes = BTreeMap::new();
    let mut groups = Vec::<CypherReturnGroup>::new();
    for row_index in 0..row_count {
        let mut scalar_values = Vec::with_capacity(scalar_indices.len());
        for index in &scalar_indices {
            scalar_values.push(
                evaluate_scalar_return_projection(
                    store,
                    node_bindings,
                    edge_bindings,
                    row_node_values,
                    row_edge_values,
                    row_path_bindings,
                    nodes,
                    edges,
                    &return_clause.projections[*index],
                    row_index,
                )
                .await?,
            );
        }
        let group_key = return_row_key(&scalar_values, "RETURN grouping")?;
        let group_index = if let Some(group_index) = group_indexes.get(&group_key).copied() {
            group_index
        } else {
            let group_index = groups.len();
            group_indexes.insert(group_key, group_index);
            let aggregate_states = return_clause
                .projections
                .iter()
                .map(|projection| {
                    if projection.element == CypherReturnElement::Aggregate {
                        CypherGroupedAggregateState::new(projection).map(Some)
                    } else {
                        Ok(None)
                    }
                })
                .collect::<Result<Vec<_>>>()?;
            groups.push(CypherReturnGroup {
                scalar_values,
                aggregate_states,
            });
            group_index
        };

        for (projection_index, projection) in return_clause.projections.iter().enumerate() {
            if projection.element != CypherReturnElement::Aggregate {
                continue;
            }
            let values = materialize_return_aggregate_row_values(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await?;
            let Some(state) = groups[group_index].aggregate_states[projection_index].as_mut()
            else {
                continue;
            };
            state.record(values)?;
        }
    }

    let mut rows = Vec::with_capacity(groups.len());
    for group in groups {
        let CypherReturnGroup {
            scalar_values,
            aggregate_states,
        } = group;
        let mut aggregate_states = aggregate_states.into_iter();
        let mut scalar_cursor = 0usize;
        let mut row = Vec::with_capacity(return_clause.projections.len());
        for projection in &return_clause.projections {
            if projection.element == CypherReturnElement::Aggregate {
                let state = aggregate_states.next().flatten().ok_or_else(|| {
                    cypher_unsupported_cardinality(
                        "RETURN grouped aggregate state is missing for projection",
                    )
                })?;
                row.push(state.finish()?);
            } else {
                let _ = aggregate_states.next();
                let value = scalar_values.get(scalar_cursor).cloned().ok_or_else(|| {
                    cypher_unsupported_cardinality("RETURN grouped scalar state is missing")
                })?;
                scalar_cursor += 1;
                row.push(value);
            }
        }
        rows.push(row);
    }
    if return_clause.distinct {
        apply_return_distinct(&mut rows)?;
    }
    apply_return_control(
        &mut rows,
        &return_clause.order_by,
        return_clause.skip,
        return_clause.limit,
    );
    Ok(CypherResultTable { columns, rows })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn evaluate_scalar_return_projection<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let _kind = classify_return_scalar_projection(&projection.target);
    let expression = scalar_return_ast(&projection.target);
    match classify_return_scalar_ast_family(&expression) {
        CypherReturnScalarAstFamily::Binding => {
            evaluate_scalar_binding_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
        CypherReturnScalarAstFamily::Wrapper => {
            evaluate_scalar_wrapper_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
        CypherReturnScalarAstFamily::Value => {
            evaluate_scalar_value_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
        CypherReturnScalarAstFamily::Control => {
            evaluate_scalar_control_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
        CypherReturnScalarAstFamily::Introspection => {
            evaluate_scalar_introspection_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
        CypherReturnScalarAstFamily::List => {
            evaluate_scalar_list_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
        CypherReturnScalarAstFamily::Numeric => {
            evaluate_scalar_numeric_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
        CypherReturnScalarAstFamily::Conversion => {
            evaluate_scalar_conversion_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
        CypherReturnScalarAstFamily::String => {
            evaluate_scalar_string_return_expression(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                expression,
                row_index,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn evaluate_scalar_binding_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::Star => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN * is only supported inside COUNT(*) or COLLECT(*)",
        )),
        CypherReturnScalarAst::Element => {
            materialize_return_element_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::DirectProperty(key) => {
            materialize_return_property_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await
        }
        _ => unreachable!("non-binding scalar expression routed to binding evaluator"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn evaluate_scalar_wrapper_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::ElementFunction => {
            materialize_return_element_function_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PathFunction => {
            materialize_return_path_function_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await
        }
        _ => unreachable!("non-wrapper scalar expression routed to wrapper evaluator"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn evaluate_scalar_value_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::Literal(value) => Ok(value.clone()),
        CypherReturnScalarAst::Map(map) => {
            materialize_return_map_projection_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                map,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::List(list) => {
            materialize_return_list_projection_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                list,
                row_index,
            )
            .await
        }
        _ => unreachable!("non-value scalar expression routed to value expression evaluator"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn evaluate_scalar_control_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::Conditional(case) => {
            materialize_return_case_projection_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                case,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::Coalesce(coalesce) => {
            materialize_return_coalesce_projection_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                coalesce,
                row_index,
            )
            .await
        }
        _ => unreachable!("non-control scalar expression routed to control expression evaluator"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn evaluate_scalar_introspection_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::PropertyExists(key) => {
            materialize_return_property_exists_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertySize(key) => {
            materialize_return_property_size_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await
        }
        _ => {
            unreachable!("non-introspection scalar expression routed to introspection evaluator")
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn evaluate_scalar_numeric_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::PropertyAbs(abs) => {
            materialize_return_property_abs_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                abs,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyNumericRound(round) => {
            materialize_return_property_numeric_round_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                round,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyNumericSign(sign) => {
            materialize_return_property_numeric_sign_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                sign,
                row_index,
            )
            .await
        }
        _ => unreachable!("non-numeric scalar expression routed to numeric expression evaluator"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn evaluate_scalar_conversion_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::PropertyNumericCast(cast) => {
            materialize_return_property_numeric_cast_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                cast,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyListCast(cast) => {
            materialize_return_property_list_cast_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                cast,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyToBoolean(to_boolean) => {
            materialize_return_property_to_boolean_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                to_boolean,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyToString(to_string) => {
            materialize_return_property_to_string_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                to_string,
                row_index,
            )
            .await
        }
        _ => {
            unreachable!(
                "non-conversion scalar expression routed to conversion expression evaluator"
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn evaluate_scalar_list_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::PropertyListIndex(index) => {
            materialize_return_property_list_index_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                index,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyListSlice(slice) => {
            materialize_return_property_list_slice_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                slice,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyListContains(contains) => {
            materialize_return_property_list_contains_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                contains,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyListPredicate(predicate) => {
            materialize_return_property_list_predicate_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                predicate,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyListElement(element) => {
            materialize_return_property_list_element_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                element,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyListTail(tail) => {
            materialize_return_property_list_tail_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                tail,
                row_index,
            )
            .await
        }
        _ => unreachable!("non-list scalar expression routed to list expression evaluator"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn evaluate_scalar_string_return_expression<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    expression: CypherReturnScalarAst<'_>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match expression {
        CypherReturnScalarAst::PropertyStringTransform(transform) => {
            materialize_return_string_transform_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                transform,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyStringTrim(trim) => {
            materialize_return_string_trim_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                trim,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyIsEmpty(is_empty) => {
            materialize_return_is_empty_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                is_empty,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyStringReverse(reverse) => {
            materialize_return_string_reverse_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                reverse,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyStringSplit(split) => {
            materialize_return_property_string_split_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                split,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertySubstring(substring) => {
            materialize_return_property_substring_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                substring,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyStringSlice(slice) => {
            materialize_return_property_string_slice_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                slice,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyReplace(replace) => {
            materialize_return_property_replace_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                replace,
                row_index,
            )
            .await
        }
        CypherReturnScalarAst::PropertyStringPredicate(predicate) => {
            materialize_return_property_string_predicate_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                predicate,
                row_index,
            )
            .await
        }
        _ => unreachable!("non-string scalar expression routed to string expression evaluator"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_aggregate_row_values<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_index: usize,
) -> Result<Vec<Value>>
where
    S: GraphStore + Sync,
{
    let aggregate = projection.aggregate.ok_or_else(|| {
        cypher_unsupported_cardinality("RETURN aggregate projection is missing aggregate kind")
    })?;
    match classify_return_target_materialization(&projection.target) {
        CypherReturnTargetMaterialization::Star
            if aggregate == CypherReturnAggregate::Count && !projection.distinct =>
        {
            Ok(vec![Value::Int(1)])
        }
        CypherReturnTargetMaterialization::Star if aggregate == CypherReturnAggregate::Collect => {
            materialize_return_star_row_value(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                row_index,
            )
            .await
            .map(|value| vec![value])
        }
        CypherReturnTargetMaterialization::Star => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN aggregates other than COUNT and COLLECT require variable.property",
        )),
        CypherReturnTargetMaterialization::Element
            if aggregate == CypherReturnAggregate::Count && !projection.distinct =>
        {
            Ok(vec![Value::Int(1)])
        }
        CypherReturnTargetMaterialization::Element
            if aggregate == CypherReturnAggregate::Count
                || aggregate == CypherReturnAggregate::Collect =>
        {
            materialize_return_element_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await
            .map(|value| vec![value])
        }
        CypherReturnTargetMaterialization::Element => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN aggregates other than COUNT and COLLECT require variable.property",
        )),
        CypherReturnTargetMaterialization::DirectProperty => {
            let CypherReturnTarget::Property(key) = &projection.target else {
                unreachable!("direct property classification requires property target");
            };
            let value = materialize_return_property_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                key,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTargetMaterialization::ElementFunction => {
            let value = materialize_return_element_function_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTargetMaterialization::PathFunction => {
            let value = materialize_return_path_function_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
        CypherReturnTargetMaterialization::ScalarProjection => {
            let value = evaluate_scalar_return_projection(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await?;
            Ok(non_null_return_value(value).into_iter().collect())
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_element_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    match projection.element {
        CypherReturnElement::Node => {
            let id = node_bindings.get(&projection.variable).ok_or_else(|| {
                cypher_unresolved_identity(format!(
                    "RETURN references variable '{}' that is not bound by the write plan",
                    projection.variable
                ))
            })?;
            let node = resolve_bound_node(store, nodes, &projection.variable, id).await?;
            graph_node_value(node)
        }
        CypherReturnElement::Edge => {
            let identity = edge_bindings.get(&projection.variable).ok_or_else(|| {
                cypher_unresolved_identity(format!(
                    "RETURN references relationship variable '{}' that is not bound by the write plan",
                    projection.variable
                ))
            })?;
            let edge =
                resolve_bound_edge_cached(store, edges, identity, &projection.variable).await?;
            graph_edge_value(edge)
        }
        CypherReturnElement::RowNode => {
            let node = row_node_values
                .get(&projection.variable)
                .and_then(|nodes| nodes.get(row_index))
                .ok_or_else(|| {
                    cypher_unsupported_cardinality(format!(
                        "writable Cypher RETURN cannot materialize matched node variable '{}'",
                        projection.variable
                    ))
                })?;
            graph_node_value(node)
        }
        CypherReturnElement::RowEdge => {
            let edge = row_edge_values
                .get(&projection.variable)
                .and_then(|edges| edges.get(row_index))
                .ok_or_else(|| {
                    cypher_unsupported_cardinality(format!(
                        "writable Cypher RETURN cannot materialize row-producing relationship variable '{}'",
                        projection.variable
                    ))
                })?;
            graph_edge_value(edge)
        }
        CypherReturnElement::RowPath => {
            materialize_return_path_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                &projection.variable,
                row_index,
            )
            .await
        }
        CypherReturnElement::Literal => match &projection.target {
            CypherReturnTarget::Literal(value) => Ok(value.clone()),
            _ => Err(cypher_unsupported_cardinality(
                "RETURN literal element received a non-literal projection",
            )),
        },
        CypherReturnElement::Aggregate => {
            match (
                node_bindings.contains_key(&projection.variable),
                edge_bindings.contains_key(&projection.variable),
                row_node_values.contains_key(&projection.variable),
                row_edge_values.contains_key(&projection.variable),
                row_path_bindings.contains_key(&projection.variable),
            ) {
                (true, false, false, false, false) => {
                    let id = node_bindings
                        .get(&projection.variable)
                        .expect("checked binding");
                    let node = resolve_bound_node(store, nodes, &projection.variable, id).await?;
                    graph_node_value(node)
                }
                (false, true, false, false, false) => {
                    let identity = edge_bindings
                        .get(&projection.variable)
                        .expect("checked binding");
                    let edge =
                        resolve_bound_edge_cached(store, edges, identity, &projection.variable)
                            .await?;
                    graph_edge_value(edge)
                }
                (false, false, true, false, false) => {
                    let node = row_node_values
                        .get(&projection.variable)
                        .and_then(|nodes| nodes.get(row_index))
                        .ok_or_else(|| {
                            cypher_unsupported_cardinality(format!(
                                "writable Cypher RETURN cannot materialize matched node variable '{}'",
                                projection.variable
                            ))
                        })?;
                    graph_node_value(node)
                }
                (false, false, false, true, false) => {
                    let edge = row_edge_values
                        .get(&projection.variable)
                        .and_then(|edges| edges.get(row_index))
                        .ok_or_else(|| {
                            cypher_unsupported_cardinality(format!(
                                "writable Cypher RETURN cannot materialize row-producing relationship variable '{}'",
                                projection.variable
                            ))
                        })?;
                    graph_edge_value(edge)
                }
                (false, false, false, false, true) => {
                    materialize_return_path_value_at(
                        store,
                        node_bindings,
                        edge_bindings,
                        row_node_values,
                        row_edge_values,
                        row_path_bindings,
                        nodes,
                        edges,
                        &projection.variable,
                        row_index,
                    )
                    .await
                }
                _ => Err(cypher_unresolved_identity(format!(
                    "RETURN references variable '{}' that is not bound by the write plan",
                    projection.variable
                ))),
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_path_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    variable: &str,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let path = row_path_bindings.get(variable).ok_or_else(|| {
        cypher_unsupported_cardinality(format!(
            "writable Cypher RETURN cannot materialize path variable '{variable}'"
        ))
    })?;
    let from = materialize_return_path_endpoint_node(
        store,
        node_bindings,
        row_node_values,
        nodes,
        &path.from_variable,
        row_index,
    )
    .await?;
    let from_value = graph_node_value(from)?.to_json();
    let edge = if let Some(edge) = row_edge_values
        .get(&path.edge_variable)
        .and_then(|edges| edges.get(row_index))
    {
        edge
    } else if let Some(identity) = edge_bindings.get(&path.edge_variable) {
        resolve_bound_edge_cached(store, edges, identity, &path.edge_variable).await?
    } else {
        return Err(cypher_unsupported_cardinality(format!(
            "writable Cypher RETURN cannot materialize path variable '{variable}'"
        )));
    };
    let to = materialize_return_path_endpoint_node(
        store,
        node_bindings,
        row_node_values,
        nodes,
        &path.to_variable,
        row_index,
    )
    .await?;
    let to_value = graph_node_value(to)?.to_json();
    let edge_value = graph_edge_value(edge)?.to_json();
    Ok(Value::Json(serde_json::json!({
        "nodes": [from_value, to_value],
        "relationships": [edge_value],
    })))
}

pub(crate) async fn materialize_return_path_endpoint_node<'a, S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    row_node_values: &'a HashMap<String, Vec<Node>>,
    nodes: &'a mut HashMap<String, Node>,
    variable: &str,
    row_index: usize,
) -> Result<&'a Node>
where
    S: GraphStore + Sync,
{
    if let Some(id) = node_bindings.get(variable) {
        return resolve_bound_node(store, nodes, variable, id).await;
    }
    row_node_values
        .get(variable)
        .and_then(|nodes| nodes.get(row_index))
        .ok_or_else(|| {
            cypher_unsupported_cardinality(format!(
                "writable Cypher RETURN cannot materialize path endpoint variable '{variable}'"
            ))
        })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_map_projection_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    map: &CypherReturnMapProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let mut output = serde_json::Map::new();
    for entry in &map.entries {
        let value = Box::pin(evaluate_scalar_return_projection(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            &CypherReturnProjection {
                target: entry.value.clone(),
                variable: map.variable.clone(),
                column: projection.column.clone(),
                expression: projection.expression.clone(),
                element: projection.element,
                aggregate: None,
                distinct: false,
            },
            row_index,
        ))
        .await?
        .to_json();
        output.insert(entry.output_key.clone(), value);
    }
    Ok(Value::Json(serde_json::Value::Object(output)))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_list_projection_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    list: &CypherReturnListProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let mut values = Vec::with_capacity(list.terms.len());
    for term in &list.terms {
        let value = Box::pin(evaluate_scalar_return_projection(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            &CypherReturnProjection {
                target: term.clone(),
                variable: list
                    .variable
                    .clone()
                    .unwrap_or_else(|| projection.variable.clone()),
                column: projection.column.clone(),
                expression: projection.expression.clone(),
                element: projection.element,
                aggregate: None,
                distinct: false,
            },
            row_index,
        ))
        .await?;
        values.push(value.to_json());
    }
    Ok(Value::Json(serde_json::Value::Array(values)))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_case_projection_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    case: &CypherReturnCase,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let actual = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        &case.key,
        row_index,
    )
    .await?;
    let target = if actual == case.equals {
        case.then_target.as_ref()
    } else {
        case.else_target.as_ref()
    };
    Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: target.clone(),
            variable: projection.variable.clone(),
            column: projection.column.clone(),
            expression: projection.expression.clone(),
            element: projection.element,
            aggregate: None,
            distinct: false,
        },
        row_index,
    ))
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_coalesce_projection_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    coalesce: &CypherReturnCoalesce,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    for term in &coalesce.terms {
        let value = Box::pin(evaluate_scalar_return_projection(
            store,
            node_bindings,
            edge_bindings,
            row_node_values,
            row_edge_values,
            row_path_bindings,
            nodes,
            edges,
            &CypherReturnProjection {
                target: term.clone(),
                variable: coalesce
                    .variable
                    .clone()
                    .unwrap_or_else(|| projection.variable.clone()),
                column: projection.column.clone(),
                expression: projection.expression.clone(),
                element: projection.element,
                aggregate: None,
                distinct: false,
            },
            row_index,
        ))
        .await?;
        if value != Value::Null {
            return Ok(value);
        }
    }
    Ok(Value::Null)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_exists_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    key: &str,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        key,
        row_index,
    )
    .await?;
    Ok(Value::Bool(value != Value::Null))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_size_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    key: &str,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let value = materialize_return_property_value_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        key,
        row_index,
    )
    .await?;
    restricted_size_value(value)
}
