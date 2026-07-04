//! Restricted scalar/list/string/numeric value evaluation (extracted from lib.rs).

use crate::*;

pub(crate) fn restricted_size_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::Int(value.chars().count() as i64)),
        Value::DateTime(value) => Ok(Value::Int(value.as_str().chars().count() as i64)),
        Value::StringArray(values) => Ok(Value::Int(values.len() as i64)),
        Value::IntArray(values) => Ok(Value::Int(values.len() as i64)),
        Value::FloatArray(values) => Ok(Value::Int(values.len() as i64)),
        Value::Json(serde_json::Value::String(value)) => {
            Ok(Value::Int(value.chars().count() as i64))
        }
        Value::Json(serde_json::Value::Array(values)) => Ok(Value::Int(values.len() as i64)),
        Value::Json(serde_json::Value::Object(values)) => Ok(Value::Int(values.len() as i64)),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN size only supports string, array, or JSON collection values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_list_index_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    index: &CypherReturnListIndexProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let index_value = evaluate_return_list_bound_at(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        projection,
        &index.index,
        row_index,
        "RETURN list index",
    )
    .await?;
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
        &index.key,
        row_index,
    )
    .await?;
    restricted_list_index_value(value, index_value)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn evaluate_return_list_bound_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    bound: &CypherReturnListBound,
    row_index: usize,
    context: &str,
) -> Result<usize>
where
    S: GraphStore + Sync,
{
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
            target: (*bound.target).clone(),
            variable: bound
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
    let Value::Int(value) = value else {
        return Err(cypher_unsupported_cardinality(format!(
            "{context} must evaluate to an integer"
        )));
    };
    usize::try_from(value).map_err(|_| {
        cypher_unsupported_cardinality(format!("{context} must evaluate to a non-negative integer"))
    })
}

pub(crate) fn restricted_list_index_value(value: Value, index: usize) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::StringArray(values) => Ok(values.get(index).map(Value::from).unwrap_or(Value::Null)),
        Value::IntArray(values) => Ok(values
            .get(index)
            .copied()
            .map(Value::Int)
            .unwrap_or(Value::Null)),
        Value::FloatArray(values) => Ok(values
            .get(index)
            .copied()
            .map(Value::Float)
            .unwrap_or(Value::Null)),
        Value::Json(serde_json::Value::Array(values)) => Ok(values
            .get(index)
            .cloned()
            .map(Value::from_json)
            .unwrap_or(Value::Null)),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list indexes only support array values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_list_slice_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    slice: &CypherReturnListSlice,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let start = if let Some(start) = &slice.start {
        Some(
            evaluate_return_list_bound_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                start,
                row_index,
                "RETURN list slice start",
            )
            .await?,
        )
    } else {
        None
    };
    let end = if let Some(end) = &slice.end {
        Some(
            evaluate_return_list_bound_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                projection,
                end,
                row_index,
                "RETURN list slice end",
            )
            .await?,
        )
    } else {
        None
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
        &slice.key,
        row_index,
    )
    .await?;
    restricted_list_slice_value(value, start, end)
}

pub(crate) fn bounded_list_slice_indexes(
    len: usize,
    start: Option<usize>,
    end: Option<usize>,
) -> (usize, usize) {
    let start = start.unwrap_or(0).min(len);
    let end = end.unwrap_or(len).min(len);
    if end < start {
        (start, start)
    } else {
        (start, end)
    }
}

pub(crate) fn restricted_list_slice_value(
    value: Value,
    start: Option<usize>,
    end: Option<usize>,
) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::StringArray(values) => {
            let (start, end) = bounded_list_slice_indexes(values.len(), start, end);
            Ok(Value::StringArray(values[start..end].to_vec()))
        }
        Value::IntArray(values) => {
            let (start, end) = bounded_list_slice_indexes(values.len(), start, end);
            Ok(Value::IntArray(values[start..end].to_vec()))
        }
        Value::FloatArray(values) => {
            let (start, end) = bounded_list_slice_indexes(values.len(), start, end);
            Ok(Value::FloatArray(values[start..end].to_vec()))
        }
        Value::Json(serde_json::Value::Array(values)) => {
            let (start, end) = bounded_list_slice_indexes(values.len(), start, end);
            Ok(Value::from_json(serde_json::Value::Array(
                values[start..end].to_vec(),
            )))
        }
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list slices only support array values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_list_contains_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    contains: &CypherReturnListContains,
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
        &contains.key,
        row_index,
    )
    .await?;
    restricted_list_contains_value(value, &contains.needle)
}

pub(crate) fn restricted_list_contains_value(value: Value, needle: &Value) -> Result<Value> {
    if matches!(needle, Value::Null) {
        return Ok(Value::Null);
    }
    match value {
        Value::Null => Ok(Value::Null),
        Value::StringArray(values) => {
            Ok(Value::Bool(needle.as_str().is_some_and(|needle| {
                values.iter().any(|value| value == needle)
            })))
        }
        Value::IntArray(values) => Ok(Value::Bool(matches!(
            needle,
            Value::Int(needle) if values.iter().any(|value| value == needle)
        ))),
        Value::FloatArray(values) => Ok(Value::Bool(matches!(
            needle,
            Value::Float(needle) if values.iter().any(|value| value == needle)
        ))),
        Value::Json(serde_json::Value::Array(values)) => Ok(Value::Bool(
            values
                .into_iter()
                .map(Value::from_json)
                .any(|value| &value == needle),
        )),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN IN only supports array values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_list_predicate_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    predicate: &CypherReturnListPredicateProjection,
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
        &predicate.key,
        row_index,
    )
    .await?;
    let needle = Box::pin(evaluate_scalar_return_projection(
        store,
        node_bindings,
        edge_bindings,
        row_node_values,
        row_edge_values,
        row_path_bindings,
        nodes,
        edges,
        &CypherReturnProjection {
            target: predicate.equals.as_ref().clone(),
            variable: predicate
                .equals_variable
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
    restricted_list_predicate_value(value, needle, predicate.predicate)
}

pub(crate) fn restricted_list_predicate_value(
    value: Value,
    needle: Value,
    predicate: CypherReturnListPredicate,
) -> Result<Value> {
    if matches!(needle, Value::Null) {
        return Ok(Value::Null);
    }
    match value {
        Value::Null => Ok(Value::Null),
        Value::StringArray(values) => Ok(Value::Bool(evaluate_list_predicate_matches(
            values
                .iter()
                .map(|value| matches!(needle.as_str(), Some(needle) if value == needle)),
            predicate,
        ))),
        Value::IntArray(values) => Ok(Value::Bool(evaluate_list_predicate_matches(
            values
                .iter()
                .map(|value| matches!(needle, Value::Int(needle) if *value == needle)),
            predicate,
        ))),
        Value::FloatArray(values) => Ok(Value::Bool(evaluate_list_predicate_matches(
            values
                .iter()
                .map(|value| matches!(needle, Value::Float(needle) if *value == needle)),
            predicate,
        ))),
        Value::Json(serde_json::Value::Array(values)) => {
            Ok(Value::Bool(evaluate_list_predicate_matches(
                values
                    .into_iter()
                    .map(Value::from_json)
                    .map(|value| value == needle),
                predicate,
            )))
        }
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list predicates only support array values",
        )),
    }
}

pub(crate) fn evaluate_list_predicate_matches(
    matches: impl IntoIterator<Item = bool>,
    predicate: CypherReturnListPredicate,
) -> bool {
    let mut total = 0usize;
    let mut matched = 0usize;
    for is_match in matches {
        total += 1;
        if is_match {
            matched += 1;
        }
    }
    match predicate {
        CypherReturnListPredicate::Any => matched > 0,
        CypherReturnListPredicate::All => matched == total,
        CypherReturnListPredicate::None => matched == 0,
        CypherReturnListPredicate::Single => matched == 1,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_list_element_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    element: &CypherReturnListElementProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*element.target).clone(),
            variable: element
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
    restricted_list_element_value(value, element.element)
}

pub(crate) fn restricted_list_element_value(
    value: Value,
    element: CypherReturnListElement,
) -> Result<Value> {
    let select = |len: usize| match element {
        CypherReturnListElement::Head => 0,
        CypherReturnListElement::Last => len.saturating_sub(1),
    };
    match value {
        Value::Null => Ok(Value::Null),
        Value::StringArray(values) => Ok(values
            .get(select(values.len()))
            .map(Value::from)
            .unwrap_or(Value::Null)),
        Value::IntArray(values) => Ok(values
            .get(select(values.len()))
            .copied()
            .map(Value::Int)
            .unwrap_or(Value::Null)),
        Value::FloatArray(values) => Ok(values
            .get(select(values.len()))
            .copied()
            .map(Value::Float)
            .unwrap_or(Value::Null)),
        Value::Json(serde_json::Value::Array(values)) => Ok(values
            .get(select(values.len()))
            .cloned()
            .map(Value::from_json)
            .unwrap_or(Value::Null)),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN head/last only supports array values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_list_tail_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    tail: &CypherReturnListTailProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*tail.target).clone(),
            variable: tail
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
    restricted_list_tail_value(value)
}

pub(crate) fn restricted_list_tail_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::StringArray(values) => Ok(Value::StringArray(
            values.into_iter().skip(1).collect::<Vec<_>>(),
        )),
        Value::IntArray(values) => Ok(Value::IntArray(
            values.into_iter().skip(1).collect::<Vec<_>>(),
        )),
        Value::FloatArray(values) => Ok(Value::FloatArray(
            values.into_iter().skip(1).collect::<Vec<_>>(),
        )),
        Value::Json(serde_json::Value::Array(values)) => Ok(Value::from_json(
            serde_json::Value::Array(values.into_iter().skip(1).collect()),
        )),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN tail only supports array values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_abs_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    abs: &CypherReturnAbsProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*abs.target).clone(),
            variable: abs
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
    restricted_abs_value(value)
}

pub(crate) fn restricted_abs_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Int(value) => value
            .checked_abs()
            .map(Value::Int)
            .ok_or_else(|| GrustError::CypherExecution("RETURN abs integer overflow".to_string())),
        Value::Float(value) => {
            let value = value.abs();
            if value.is_finite() {
                Ok(Value::Float(value))
            } else {
                Err(GrustError::CypherExecution(
                    "RETURN abs produced non-finite float".to_string(),
                ))
            }
        }
        Value::Json(serde_json::Value::Number(value)) => {
            if let Some(value) = value.as_i64() {
                restricted_abs_value(Value::Int(value))
            } else if let Some(value) = value.as_f64() {
                restricted_abs_value(Value::Float(value))
            } else {
                Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN abs only supports finite numeric values",
                ))
            }
        }
        Value::Bool(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN abs only supports numeric values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_numeric_round_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    round: &CypherReturnNumericRoundProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*round.target).clone(),
            variable: round
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
    restricted_numeric_round_value(value, round.round)
}

pub(crate) fn restricted_numeric_round_value(
    value: Value,
    round: CypherReturnNumericRound,
) -> Result<Value> {
    let round_float = |value: f64| match round {
        CypherReturnNumericRound::Ceil => value.ceil(),
        CypherReturnNumericRound::Floor => value.floor(),
    };
    match value {
        Value::Null => Ok(Value::Null),
        Value::Int(value) => Ok(Value::Int(value)),
        Value::Float(value) => {
            let value = round_float(value);
            if value.is_finite() {
                Ok(Value::Float(value))
            } else {
                Err(GrustError::CypherExecution(
                    "RETURN ceil/floor produced non-finite float".to_string(),
                ))
            }
        }
        Value::Json(serde_json::Value::Number(value)) => {
            if let Some(value) = value.as_i64() {
                Ok(Value::Int(value))
            } else if let Some(value) = value.as_f64() {
                restricted_numeric_round_value(Value::Float(value), round)
            } else {
                Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN ceil/floor only supports finite numeric values",
                ))
            }
        }
        Value::Bool(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN ceil/floor only supports numeric values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_numeric_sign_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    sign: &CypherReturnNumericSignProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*sign.target).clone(),
            variable: sign
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
    restricted_numeric_sign_value(value)
}

pub(crate) fn restricted_numeric_sign_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Int(value) => Ok(Value::Int(value.signum())),
        Value::Float(value) => {
            if !value.is_finite() {
                return Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN sign only supports finite numeric values",
                ));
            }
            let sign = if value > 0.0 {
                1.0
            } else if value < 0.0 {
                -1.0
            } else {
                0.0
            };
            Ok(Value::Float(sign))
        }
        Value::Json(serde_json::Value::Number(value)) => {
            if let Some(value) = value.as_i64() {
                restricted_numeric_sign_value(Value::Int(value))
            } else if let Some(value) = value.as_f64() {
                restricted_numeric_sign_value(Value::Float(value))
            } else {
                Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN sign only supports finite numeric values",
                ))
            }
        }
        Value::Bool(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN sign only supports numeric values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_numeric_cast_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    cast: &CypherReturnNumericCastProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*cast.target).clone(),
            variable: cast
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
    restricted_numeric_cast_value(value, cast.cast)
}

pub(crate) fn restricted_numeric_cast_value(
    value: Value,
    cast: CypherReturnNumericCast,
) -> Result<Value> {
    match cast {
        CypherReturnNumericCast::Integer => restricted_to_integer_value(value),
        CypherReturnNumericCast::Float => restricted_to_float_value(value),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_list_cast_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    cast: &CypherReturnListCastProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*cast.target).clone(),
            variable: cast
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
    restricted_list_cast_value(value, cast.cast)
}

pub(crate) fn restricted_list_cast_value(
    value: Value,
    cast: CypherReturnListCast,
) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::StringArray(values) => restricted_string_array_cast_value(values, cast),
        Value::IntArray(values) => restricted_int_array_cast_value(values, cast),
        Value::FloatArray(values) => restricted_float_array_cast_value(values, cast),
        Value::Json(serde_json::Value::Array(values)) => {
            let values = values.into_iter().map(Value::from_json).collect::<Vec<_>>();
            restricted_json_array_cast_value(values, cast)
        }
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::DateTime(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list casts only support array values",
        )),
    }
}

pub(crate) fn restricted_string_array_cast_value(
    values: Vec<String>,
    cast: CypherReturnListCast,
) -> Result<Value> {
    match cast {
        CypherReturnListCast::String => Ok(Value::StringArray(values)),
        CypherReturnListCast::Integer => values
            .into_iter()
            .map(|value| parse_integer_string_value(&value).and_then(expect_int_value))
            .collect::<Result<Vec<_>>>()
            .map(Value::IntArray),
        CypherReturnListCast::Float => values
            .into_iter()
            .map(|value| parse_float_string_value(&value).and_then(expect_float_value))
            .collect::<Result<Vec<_>>>()
            .map(Value::FloatArray),
        CypherReturnListCast::Boolean => values
            .into_iter()
            .map(|value| parse_boolean_string_value(&value).and_then(expect_bool_value))
            .collect::<Result<Vec<_>>>()
            .map(|values| Value::Json(serde_json::Value::Array(values))),
    }
}

pub(crate) fn restricted_int_array_cast_value(
    values: Vec<i64>,
    cast: CypherReturnListCast,
) -> Result<Value> {
    match cast {
        CypherReturnListCast::String => Ok(Value::StringArray(
            values.into_iter().map(|value| value.to_string()).collect(),
        )),
        CypherReturnListCast::Integer => Ok(Value::IntArray(values)),
        CypherReturnListCast::Float => Ok(Value::FloatArray(
            values.into_iter().map(|value| value as f64).collect(),
        )),
        CypherReturnListCast::Boolean => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toBooleanList only supports boolean or boolean-string array values",
        )),
    }
}

pub(crate) fn restricted_float_array_cast_value(
    values: Vec<f64>,
    cast: CypherReturnListCast,
) -> Result<Value> {
    match cast {
        CypherReturnListCast::String => Ok(Value::StringArray(
            values.into_iter().map(|value| value.to_string()).collect(),
        )),
        CypherReturnListCast::Integer => values
            .into_iter()
            .map(|value| float_to_integer_value(value).and_then(expect_int_value))
            .collect::<Result<Vec<_>>>()
            .map(Value::IntArray),
        CypherReturnListCast::Float => {
            if values.iter().any(|value| !value.is_finite()) {
                return Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN toFloatList only supports finite numeric values",
                ));
            }
            Ok(Value::FloatArray(values))
        }
        CypherReturnListCast::Boolean => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toBooleanList only supports boolean or boolean-string array values",
        )),
    }
}

pub(crate) fn restricted_json_array_cast_value(
    values: Vec<Value>,
    cast: CypherReturnListCast,
) -> Result<Value> {
    match cast {
        CypherReturnListCast::String => values
            .into_iter()
            .map(|value| restricted_to_string_value(value).and_then(expect_string_value))
            .collect::<Result<Vec<_>>>()
            .map(Value::StringArray),
        CypherReturnListCast::Integer => values
            .into_iter()
            .map(|value| restricted_to_integer_value(value).and_then(expect_int_value))
            .collect::<Result<Vec<_>>>()
            .map(Value::IntArray),
        CypherReturnListCast::Float => values
            .into_iter()
            .map(|value| restricted_to_float_value(value).and_then(expect_float_value))
            .collect::<Result<Vec<_>>>()
            .map(Value::FloatArray),
        CypherReturnListCast::Boolean => values
            .into_iter()
            .map(|value| restricted_to_boolean_value(value).and_then(expect_bool_value))
            .collect::<Result<Vec<_>>>()
            .map(|values| Value::Json(serde_json::Value::Array(values))),
    }
}

pub(crate) fn expect_string_value(value: Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(value),
        Value::Null => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toStringList does not support null array elements",
        )),
        _ => Err(GrustError::CypherExecution(
            "RETURN toStringList produced a non-string value".to_string(),
        )),
    }
}

pub(crate) fn expect_int_value(value: Value) -> Result<i64> {
    match value {
        Value::Int(value) => Ok(value),
        Value::Null => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toIntegerList does not support null array elements",
        )),
        _ => Err(GrustError::CypherExecution(
            "RETURN toIntegerList produced a non-integer value".to_string(),
        )),
    }
}

pub(crate) fn expect_float_value(value: Value) -> Result<f64> {
    match value {
        Value::Float(value) => Ok(value),
        Value::Null => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toFloatList does not support null array elements",
        )),
        _ => Err(GrustError::CypherExecution(
            "RETURN toFloatList produced a non-float value".to_string(),
        )),
    }
}

pub(crate) fn expect_bool_value(value: Value) -> Result<serde_json::Value> {
    match value {
        Value::Bool(value) => Ok(serde_json::Value::Bool(value)),
        Value::Null => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toBooleanList does not support null array elements",
        )),
        _ => Err(GrustError::CypherExecution(
            "RETURN toBooleanList produced a non-boolean value".to_string(),
        )),
    }
}

pub(crate) fn restricted_to_integer_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Int(value) => Ok(Value::Int(value)),
        Value::Float(value) => float_to_integer_value(value),
        Value::String(value) => parse_integer_string_value(&value),
        Value::Json(serde_json::Value::Null) => Ok(Value::Null),
        Value::Json(serde_json::Value::Number(value)) => {
            if let Some(value) = value.as_i64() {
                Ok(Value::Int(value))
            } else if let Some(value) = value.as_f64() {
                float_to_integer_value(value)
            } else {
                Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN toInteger only supports finite numeric values",
                ))
            }
        }
        Value::Json(serde_json::Value::String(value)) => parse_integer_string_value(&value),
        Value::Bool(_)
        | Value::DateTime(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toInteger only supports numeric or integer-string values",
        )),
    }
}

pub(crate) fn restricted_to_float_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Int(value) => Ok(Value::Float(value as f64)),
        Value::Float(value) if value.is_finite() => Ok(Value::Float(value)),
        Value::Float(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toFloat only supports finite numeric values",
        )),
        Value::String(value) => parse_float_string_value(&value),
        Value::Json(serde_json::Value::Null) => Ok(Value::Null),
        Value::Json(serde_json::Value::Number(value)) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(Value::Float)
            .ok_or_else(|| {
                cypher_unsupported_cardinality(
                    "writable Cypher RETURN toFloat only supports finite numeric values",
                )
            }),
        Value::Json(serde_json::Value::String(value)) => parse_float_string_value(&value),
        Value::Bool(_)
        | Value::DateTime(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toFloat only supports numeric or numeric-string values",
        )),
    }
}

pub(crate) fn float_to_integer_value(value: f64) -> Result<Value> {
    if !value.is_finite() || value.trunc() < i64::MIN as f64 || value.trunc() > i64::MAX as f64 {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toInteger only supports finite in-range numeric values",
        ));
    }
    Ok(Value::Int(value.trunc() as i64))
}

pub(crate) fn parse_integer_string_value(value: &str) -> Result<Value> {
    value.trim().parse::<i64>().map(Value::Int).map_err(|_| {
        cypher_unsupported_cardinality(
            "writable Cypher RETURN toInteger only supports integer-string values",
        )
    })
}

pub(crate) fn parse_float_string_value(value: &str) -> Result<Value> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(Value::Float)
        .ok_or_else(|| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN toFloat only supports finite numeric-string values",
            )
        })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_to_boolean_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    to_boolean: &CypherReturnToBooleanProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*to_boolean.target).clone(),
            variable: to_boolean
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
    restricted_to_boolean_value(value)
}

pub(crate) fn restricted_to_boolean_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Bool(value) => Ok(Value::Bool(value)),
        Value::String(value) => parse_boolean_string_value(&value),
        Value::Json(serde_json::Value::Null) => Ok(Value::Null),
        Value::Json(serde_json::Value::Bool(value)) => Ok(Value::Bool(value)),
        Value::Json(serde_json::Value::String(value)) => parse_boolean_string_value(&value),
        Value::Int(_)
        | Value::Float(_)
        | Value::DateTime(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toBoolean only supports boolean or boolean-string values",
        )),
    }
}

pub(crate) fn parse_boolean_string_value(value: &str) -> Result<Value> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Ok(Value::Bool(true)),
        "false" => Ok(Value::Bool(false)),
        _ => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toBoolean only supports true/false string values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_to_string_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    to_string: &CypherReturnToStringProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*to_string.target).clone(),
            variable: to_string
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
    restricted_to_string_value(value)
}

pub(crate) fn restricted_to_string_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Bool(value) => Ok(Value::from(value.to_string())),
        Value::Int(value) => Ok(Value::from(value.to_string())),
        Value::Float(value) => Ok(Value::from(value.to_string())),
        Value::String(value) => Ok(Value::from(value)),
        Value::DateTime(value) => Ok(Value::from(value.as_str().to_string())),
        Value::Decimal(value) => Ok(Value::from(value.to_canonical_string())),
        Value::Duration(value) => Ok(Value::from(value.to_iso_string())),
        Value::Json(serde_json::Value::Null) => Ok(Value::Null),
        Value::Json(serde_json::Value::Bool(value)) => Ok(Value::from(value.to_string())),
        Value::Json(serde_json::Value::Number(value)) => Ok(Value::from(value.to_string())),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::from(value)),
        Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(serde_json::Value::Array(_))
        | Value::Json(serde_json::Value::Object(_))
        | Value::Path(_)
        | Value::Graph(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN toString only supports scalar values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_is_empty_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    is_empty: &CypherReturnIsEmptyProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*is_empty.target).clone(),
            variable: is_empty
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
    restricted_is_empty_value(value)
}

pub(crate) fn restricted_is_empty_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::Bool(value.is_empty())),
        Value::DateTime(value) => Ok(Value::Bool(value.as_str().is_empty())),
        Value::StringArray(values) => Ok(Value::Bool(values.is_empty())),
        Value::IntArray(values) => Ok(Value::Bool(values.is_empty())),
        Value::FloatArray(values) => Ok(Value::Bool(values.is_empty())),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::Bool(value.is_empty())),
        Value::Json(serde_json::Value::Array(values)) => Ok(Value::Bool(values.is_empty())),
        Value::Json(serde_json::Value::Object(values)) => Ok(Value::Bool(values.is_empty())),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN isEmpty only supports string, array, or JSON collection values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_string_transform_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    transform: &CypherReturnStringTransformProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*transform.target).clone(),
            variable: transform
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
    restricted_string_transform_value(value, transform.transform)
}

pub(crate) fn restricted_string_transform_value(
    value: Value,
    transform: CypherReturnStringTransform,
) -> Result<Value> {
    let transform_value = |value: String| match transform {
        CypherReturnStringTransform::Lower => value.to_lowercase(),
        CypherReturnStringTransform::Upper => value.to_uppercase(),
    };
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::from(transform_value(value))),
        Value::DateTime(value) => Ok(Value::from(transform_value(value.as_str().to_string()))),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::from(transform_value(value))),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN string transforms only support string values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_string_trim_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    trim: &CypherReturnStringTrimProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*trim.target).clone(),
            variable: trim
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
    restricted_string_trim_value(value, trim.trim)
}

pub(crate) fn restricted_string_trim_value(
    value: Value,
    trim: CypherReturnStringTrim,
) -> Result<Value> {
    let trim_value = |value: String| match trim {
        CypherReturnStringTrim::Both => value.trim().to_string(),
        CypherReturnStringTrim::Left => value.trim_start().to_string(),
        CypherReturnStringTrim::Right => value.trim_end().to_string(),
    };
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::from(trim_value(value))),
        Value::DateTime(value) => Ok(Value::from(trim_value(value.as_str().to_string()))),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::from(trim_value(value))),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN string trims only support string values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_string_reverse_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    reverse: &CypherReturnStringReverseProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*reverse.target).clone(),
            variable: reverse
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
    restricted_string_reverse_value(value)
}

pub(crate) fn restricted_string_reverse_value(value: Value) -> Result<Value> {
    let reverse_value = |value: String| value.chars().rev().collect::<String>();
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::from(reverse_value(value))),
        Value::DateTime(value) => Ok(Value::from(reverse_value(value.as_str().to_string()))),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::from(reverse_value(value))),
        Value::StringArray(values) => Ok(Value::StringArray(values.into_iter().rev().collect())),
        Value::IntArray(values) => Ok(Value::IntArray(values.into_iter().rev().collect())),
        Value::FloatArray(values) => Ok(Value::FloatArray(values.into_iter().rev().collect())),
        Value::Json(serde_json::Value::Array(values)) => Ok(Value::from_json(
            serde_json::Value::Array(values.into_iter().rev().collect()),
        )),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN reverse only supports string or array values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_string_split_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    split: &CypherReturnStringSplit,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*split.target).clone(),
            variable: split
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
    restricted_string_split_value(value, split)
}

pub(crate) fn restricted_string_split_value(
    value: Value,
    split: &CypherReturnStringSplit,
) -> Result<Value> {
    let split_value = |value: String| {
        Value::Json(serde_json::Value::Array(
            value
                .split(&split.delimiter)
                .map(|part| serde_json::Value::String(part.to_string()))
                .collect(),
        ))
    };
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(split_value(value)),
        Value::DateTime(value) => Ok(split_value(value.as_str().to_string())),
        Value::Json(serde_json::Value::String(value)) => Ok(split_value(value)),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN split only supports string values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_substring_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    substring: &CypherReturnSubstring,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*substring.target).clone(),
            variable: substring
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
    restricted_substring_value(value, substring)
}

pub(crate) fn restricted_substring_value(
    value: Value,
    substring: &CypherReturnSubstring,
) -> Result<Value> {
    let slice_value = |value: String| -> String {
        let chars = value.chars().skip(substring.start);
        match substring.length {
            Some(length) => chars.take(length).collect(),
            None => chars.collect(),
        }
    };
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::from(slice_value(value))),
        Value::DateTime(value) => Ok(Value::from(slice_value(value.as_str().to_string()))),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::from(slice_value(value))),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN substring only supports string values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_string_slice_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    slice: &CypherReturnStringSlice,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*slice.target).clone(),
            variable: slice
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
    restricted_string_slice_value(value, slice)
}

pub(crate) fn restricted_string_slice_value(
    value: Value,
    slice: &CypherReturnStringSlice,
) -> Result<Value> {
    let slice_value = |value: String| -> String {
        match slice.side {
            CypherReturnStringSliceSide::Left => value.chars().take(slice.length).collect(),
            CypherReturnStringSliceSide::Right => {
                let chars: Vec<_> = value.chars().collect();
                let start = chars.len().saturating_sub(slice.length);
                chars.into_iter().skip(start).collect()
            }
        }
    };
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::from(slice_value(value))),
        Value::DateTime(value) => Ok(Value::from(slice_value(value.as_str().to_string()))),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::from(slice_value(value))),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN left/right only supports string values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_replace_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    replace: &CypherReturnReplace,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*replace.target).clone(),
            variable: replace
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
    restricted_replace_value(value, replace)
}

pub(crate) fn restricted_replace_value(
    value: Value,
    replace: &CypherReturnReplace,
) -> Result<Value> {
    let replace_value = |value: String| value.replace(&replace.search, &replace.replacement);
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::from(replace_value(value))),
        Value::DateTime(value) => Ok(Value::from(replace_value(value.as_str().to_string()))),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::from(replace_value(value))),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN replace only supports string values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_string_predicate_value_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    predicate: &CypherReturnStringPredicateProjection,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
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
            target: (*predicate.target).clone(),
            variable: predicate
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
    restricted_string_predicate_value(value, predicate)
}

pub(crate) fn restricted_string_predicate_value(
    value: Value,
    predicate: &CypherReturnStringPredicateProjection,
) -> Result<Value> {
    let predicate_value = |value: String| match predicate.predicate {
        CypherReturnStringPredicate::StartsWith => value.starts_with(&predicate.needle),
        CypherReturnStringPredicate::EndsWith => value.ends_with(&predicate.needle),
        CypherReturnStringPredicate::Contains => value.contains(&predicate.needle),
    };
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) => Ok(Value::Bool(predicate_value(value))),
        Value::DateTime(value) => Ok(Value::Bool(predicate_value(value.as_str().to_string()))),
        Value::Json(serde_json::Value::String(value)) => Ok(Value::Bool(predicate_value(value))),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_)
        | Value::Path(_)
        | Value::Graph(_)
        | Value::Decimal(_)
        | Value::Duration(_) => Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN string predicates only support string values",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_element_function_value_at<S>(
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
    match projection.target {
        CypherReturnTarget::NodeLabels => match projection.element {
            CypherReturnElement::Node
            | CypherReturnElement::RowNode
            | CypherReturnElement::Aggregate => {
                let label = materialize_return_property_value_at(
                    store,
                    node_bindings,
                    edge_bindings,
                    row_node_values,
                    row_edge_values,
                    row_path_bindings,
                    nodes,
                    edges,
                    projection,
                    "label",
                    row_index,
                )
                .await?;
                let Value::String(label) = label else {
                    return Err(GrustError::CypherExecution(
                        "RETURN labels(...) could not materialize node label".to_string(),
                    ));
                };
                Ok(Value::Json(serde_json::json!([label])))
            }
            _ => Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN labels(...) requires a bound node variable",
            )),
        },
        CypherReturnTarget::RelationshipType => match projection.element {
            CypherReturnElement::Edge
            | CypherReturnElement::RowEdge
            | CypherReturnElement::Aggregate => {
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
                    "label",
                    row_index,
                )
                .await
            }
            _ => Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN type(...) requires a bound relationship variable",
            )),
        },
        CypherReturnTarget::ElementProperties => {
            let props = materialize_return_element_props_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await?;
            Ok(props_value(&props))
        }
        CypherReturnTarget::ElementKeys => {
            let props = materialize_return_element_props_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                nodes,
                edges,
                projection,
                row_index,
            )
            .await?;
            Ok(Value::Json(serde_json::Value::Array(
                props
                    .keys()
                    .map(|key| serde_json::Value::String(key.clone()))
                    .collect(),
            )))
        }
        CypherReturnTarget::ElementId => {
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
                "id",
                row_index,
            )
            .await
        }
        CypherReturnTarget::RelationshipStartNode | CypherReturnTarget::RelationshipEndNode => {
            let edge = materialize_return_relationship_edge_at(
                store,
                edge_bindings,
                row_edge_values,
                edges,
                projection,
                row_index,
            )
            .await?;
            let id = match projection.target {
                CypherReturnTarget::RelationshipStartNode => &edge.from,
                CypherReturnTarget::RelationshipEndNode => &edge.to,
                _ => unreachable!("checked relationship endpoint target"),
            };
            let node = store.get_node(id).await?.ok_or_else(|| {
                GrustError::CypherExecution(format!(
                    "RETURN relationship endpoint '{}' does not exist after the write",
                    id.as_str()
                ))
            })?;
            graph_node_value(&node)
        }
        _ => Err(cypher_unsupported_cardinality(
            "RETURN element function materializer received a non-element-function projection",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_element_props_at<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_index: usize,
) -> Result<Props>
where
    S: GraphStore + Sync,
{
    if let Some(id) = node_bindings.get(&projection.variable) {
        let node = resolve_bound_node(store, nodes, &projection.variable, id).await?;
        return Ok(node.props.clone());
    }
    if let Some(identity) = edge_bindings.get(&projection.variable) {
        let edge = resolve_bound_edge_cached(store, edges, identity, &projection.variable).await?;
        return Ok(edge.props.clone());
    }
    if let Some(row_nodes) = row_node_values.get(&projection.variable) {
        let node = row_nodes.get(row_index).ok_or_else(|| {
            cypher_unsupported_cardinality(format!(
                "writable Cypher RETURN cannot materialize matched node variable '{}'",
                projection.variable
            ))
        })?;
        return Ok(node.props.clone());
    }
    if let Some(row_edges) = row_edge_values.get(&projection.variable) {
        let edge = row_edges.get(row_index).ok_or_else(|| {
            cypher_unsupported_cardinality(format!(
                "writable Cypher RETURN cannot materialize row-producing relationship variable '{}'",
                projection.variable
            ))
        })?;
        return Ok(edge.props.clone());
    }
    Err(cypher_unresolved_identity(format!(
        "RETURN references variable '{}' that is not bound by the write plan",
        projection.variable
    )))
}

pub(crate) async fn materialize_return_relationship_edge_at<S>(
    store: &S,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_index: usize,
) -> Result<Edge>
where
    S: GraphStore + Sync,
{
    if let Some(identity) = edge_bindings.get(&projection.variable) {
        return resolve_bound_edge_cached(store, edges, identity, &projection.variable)
            .await
            .cloned();
    }
    if let Some(row_edges) = row_edge_values.get(&projection.variable) {
        return row_edges.get(row_index).cloned().ok_or_else(|| {
            cypher_unsupported_cardinality(format!(
                "writable Cypher RETURN cannot materialize row-producing relationship variable '{}'",
                projection.variable
            ))
        });
    }
    Err(cypher_unsupported_cardinality(format!(
        "writable Cypher RETURN cannot materialize relationship variable '{}'",
        projection.variable
    )))
}
