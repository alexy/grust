//! Projection value evaluation, ordering/distinct/control, and mutation-execution helpers (extracted from lib.rs).

use crate::*;

pub(crate) fn props_value(props: &Props) -> Value {
    Value::Json(serde_json::Value::Object(
        props
            .iter()
            .map(|(key, value)| (key.clone(), value.to_json()))
            .collect(),
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_projection_values<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_count: usize,
) -> Result<Vec<Value>>
where
    S: GraphStore + Sync,
{
    let mut values = Vec::with_capacity(row_count);
    for row_index in 0..row_count {
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
        if let Some(value) = non_null_return_value(value) {
            values.push(value);
        }
    }
    Ok(values)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_property_value_at<S>(
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
    match projection.element {
        CypherReturnElement::Node => {
            let id = node_bindings.get(&projection.variable).ok_or_else(|| {
                cypher_unresolved_identity(format!(
                    "RETURN references variable '{}' that is not bound by the write plan",
                    projection.variable
                ))
            })?;
            if key == "id" {
                return Ok(Value::from(id.as_str()));
            }
            let node = resolve_bound_node(store, nodes, &projection.variable, id).await?;
            Ok(project_node_value(node, key))
        }
        CypherReturnElement::Edge => {
            let identity = edge_bindings.get(&projection.variable).ok_or_else(|| {
                cypher_unresolved_identity(format!(
                    "RETURN references relationship variable '{}' that is not bound by the write plan",
                    projection.variable
                ))
            })?;
            if key == "id" {
                return Ok(identity
                    .id
                    .as_ref()
                    .map(|id| Value::from(id.as_str()))
                    .unwrap_or(Value::Null));
            }
            if key == "label" {
                return Ok(Value::from(identity.label.as_str()));
            }
            let edge =
                resolve_bound_edge_cached(store, edges, identity, &projection.variable).await?;
            Ok(project_edge_value(edge, key))
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
            if key == "id" {
                Ok(Value::from(node.id.as_str()))
            } else {
                Ok(project_node_value(node, key))
            }
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
            if key == "id" {
                Ok(edge
                    .id
                    .as_ref()
                    .map(|id| Value::from(id.as_str()))
                    .unwrap_or(Value::Null))
            } else if key == "label" {
                Ok(Value::from(edge.label.as_str()))
            } else {
                Ok(project_edge_value(edge, key))
            }
        }
        CypherReturnElement::RowPath => {
            let _ = row_path_bindings.get(&projection.variable).ok_or_else(|| {
                cypher_unsupported_cardinality(format!(
                    "writable Cypher RETURN cannot materialize path variable '{}'",
                    projection.variable
                ))
            })?;
            Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN path properties are not supported",
            ))
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
                    if key == "id" {
                        return Ok(Value::from(id.as_str()));
                    }
                    let node = resolve_bound_node(store, nodes, &projection.variable, id).await?;
                    Ok(project_node_value(node, key))
                }
                (false, true, false, false, false) => {
                    let identity = edge_bindings
                        .get(&projection.variable)
                        .expect("checked binding");
                    if key == "id" {
                        return Ok(identity
                            .id
                            .as_ref()
                            .map(|id| Value::from(id.as_str()))
                            .unwrap_or(Value::Null));
                    }
                    if key == "label" {
                        return Ok(Value::from(identity.label.as_str()));
                    }
                    let edge =
                        resolve_bound_edge_cached(store, edges, identity, &projection.variable)
                            .await?;
                    Ok(project_edge_value(edge, key))
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
                    if key == "id" {
                        Ok(Value::from(node.id.as_str()))
                    } else {
                        Ok(project_node_value(node, key))
                    }
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
                    if key == "id" {
                        Ok(edge
                            .id
                            .as_ref()
                            .map(|id| Value::from(id.as_str()))
                            .unwrap_or(Value::Null))
                    } else if key == "label" {
                        Ok(Value::from(edge.label.as_str()))
                    } else {
                        Ok(project_edge_value(edge, key))
                    }
                }
                (false, false, false, false, true) => Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN path properties are not supported",
                )),
                _ => Err(cypher_unresolved_identity(format!(
                    "RETURN references variable '{}' that is not bound by the write plan",
                    projection.variable
                ))),
            }
        }
    }
}

pub(crate) fn return_row_key(values: &[Value], context: &str) -> Result<String> {
    serde_json::to_string(
        &values
            .iter()
            .map(Value::to_json)
            .collect::<Vec<serde_json::Value>>(),
    )
    .map_err(|err| {
        GrustError::CypherExecution(format!("{context} key serialization failed: {err}"))
    })
}

pub(crate) async fn evaluate_return_aggregate<S>(
    evaluation: &mut CypherReturnEvaluation<'_, S>,
    projection: &CypherReturnProjection,
    row_count: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let aggregate = projection.aggregate.ok_or_else(|| {
        cypher_unsupported_cardinality("RETURN aggregate projection is missing aggregate kind")
    })?;
    if aggregate == CypherReturnAggregate::Count {
        return count_return_projection(evaluation, projection, row_count)
            .await
            .and_then(count_value);
    }
    let values =
        materialize_return_aggregate_values(evaluation, projection, aggregate, row_count).await?;
    evaluate_non_count_aggregate(aggregate, values)
}

pub(crate) async fn count_return_projection<S>(
    evaluation: &mut CypherReturnEvaluation<'_, S>,
    projection: &CypherReturnProjection,
    row_count: usize,
) -> Result<usize>
where
    S: GraphStore + Sync,
{
    match classify_return_target_materialization(&projection.target) {
        CypherReturnTargetMaterialization::PathFunction => {
            let values = materialize_return_path_function_values(
                evaluation.store,
                evaluation.node_bindings,
                evaluation.edge_bindings,
                evaluation.row_node_values,
                evaluation.row_edge_values,
                evaluation.row_path_bindings,
                &mut *evaluation.nodes,
                &mut *evaluation.edges,
                projection,
                row_count,
            )
            .await?;
            return count_materialized_return_values(values, projection.distinct);
        }
        CypherReturnTargetMaterialization::ScalarProjection
        | CypherReturnTargetMaterialization::ElementFunction => {
            let values = materialize_return_projection_values(
                evaluation.store,
                evaluation.node_bindings,
                evaluation.edge_bindings,
                evaluation.row_node_values,
                evaluation.row_edge_values,
                evaluation.row_path_bindings,
                &mut *evaluation.nodes,
                &mut *evaluation.edges,
                projection,
                row_count,
            )
            .await?;
            return count_materialized_return_values(values, projection.distinct);
        }
        CypherReturnTargetMaterialization::Star
        | CypherReturnTargetMaterialization::Element
        | CypherReturnTargetMaterialization::DirectProperty => {}
    }
    let CypherReturnTarget::Property(key) = &projection.target else {
        if projection.distinct {
            if evaluation
                .row_path_bindings
                .contains_key(&projection.variable)
            {
                let values = materialize_return_path_values(
                    evaluation.store,
                    evaluation.node_bindings,
                    evaluation.edge_bindings,
                    evaluation.row_node_values,
                    evaluation.row_edge_values,
                    evaluation.row_path_bindings,
                    &mut *evaluation.nodes,
                    &mut *evaluation.edges,
                    &projection.variable,
                    row_count,
                )
                .await?;
                return Ok(distinct_return_values(values)?.len());
            }
            return count_distinct_elements(
                evaluation.node_bindings,
                evaluation.edge_bindings,
                evaluation.row_node_values,
                evaluation.row_edge_values,
                evaluation.row_path_bindings,
                projection,
                row_count,
            );
        }
        return Ok(row_count);
    };
    if evaluation
        .row_path_bindings
        .contains_key(&projection.variable)
    {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN path properties are not supported",
        ));
    }
    if let Some(id) = evaluation.node_bindings.get(&projection.variable) {
        if key == "id" {
            return Ok(1);
        }
        let node = resolve_bound_node(
            evaluation.store,
            &mut *evaluation.nodes,
            &projection.variable,
            id,
        )
        .await?;
        let value = project_node_value(node, key);
        return Ok(usize::from(value != Value::Null));
    }
    if let Some(identity) = evaluation.edge_bindings.get(&projection.variable) {
        if key == "id" {
            return Ok(usize::from(identity.id.is_some()));
        }
        if key == "label" {
            return Ok(1);
        }
        let edge = resolve_bound_edge_cached(
            evaluation.store,
            &mut *evaluation.edges,
            identity,
            &projection.variable,
        )
        .await?;
        let value = project_edge_value(edge, key);
        return Ok(usize::from(value != Value::Null));
    }
    if let Some(row_nodes) = evaluation.row_node_values.get(&projection.variable) {
        if projection.distinct {
            return count_distinct_values(row_nodes.iter().filter_map(|node| {
                if key == "id" {
                    Some(Value::from(node.id.as_str()))
                } else {
                    non_null_return_value(project_node_value(node, key))
                }
            }));
        }
        return Ok(row_nodes
            .iter()
            .filter(|node| {
                if key == "id" {
                    true
                } else {
                    project_node_value(node, key) != Value::Null
                }
            })
            .count());
    }
    if let Some(row_edges) = evaluation.row_edge_values.get(&projection.variable) {
        if projection.distinct {
            return count_distinct_values(row_edges.iter().filter_map(|edge| {
                if key == "id" {
                    edge.id.as_ref().map(|id| Value::from(id.as_str()))
                } else if key == "label" {
                    Some(Value::from(edge.label.as_str()))
                } else {
                    non_null_return_value(project_edge_value(edge, key))
                }
            }));
        }
        return Ok(row_edges
            .iter()
            .filter(|edge| {
                if key == "id" {
                    edge.id.is_some()
                } else if key == "label" {
                    true
                } else {
                    project_edge_value(edge, key) != Value::Null
                }
            })
            .count());
    }
    Err(cypher_unresolved_identity(format!(
        "RETURN references variable '{}' that is not bound by the write plan",
        projection.variable
    )))
}

pub(crate) fn count_materialized_return_values(
    values: Vec<Value>,
    distinct: bool,
) -> Result<usize> {
    if distinct {
        Ok(distinct_return_values(values)?.len())
    } else {
        Ok(values.len())
    }
}

pub(crate) async fn materialize_return_aggregate_values<S>(
    evaluation: &mut CypherReturnEvaluation<'_, S>,
    projection: &CypherReturnProjection,
    aggregate: CypherReturnAggregate,
    row_count: usize,
) -> Result<Vec<Value>>
where
    S: GraphStore + Sync,
{
    let mut values = match classify_return_target_materialization(&projection.target) {
        CypherReturnTargetMaterialization::Star if aggregate == CypherReturnAggregate::Collect => {
            let mut values = Vec::with_capacity(row_count);
            for row_index in 0..row_count {
                values.push(
                    materialize_return_star_row_value(
                        evaluation.store,
                        evaluation.node_bindings,
                        evaluation.edge_bindings,
                        evaluation.row_node_values,
                        evaluation.row_edge_values,
                        evaluation.row_path_bindings,
                        &mut *evaluation.nodes,
                        &mut *evaluation.edges,
                        row_index,
                    )
                    .await?,
                );
            }
            values
        }
        CypherReturnTargetMaterialization::Star => {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN aggregates other than COUNT and COLLECT require variable.property",
            ));
        }
        CypherReturnTargetMaterialization::Element
            if aggregate == CypherReturnAggregate::Collect =>
        {
            materialize_return_element_values(evaluation, projection).await?
        }
        CypherReturnTargetMaterialization::Element => {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN aggregates other than COUNT and COLLECT require variable.property",
            ));
        }
        CypherReturnTargetMaterialization::DirectProperty => {
            let CypherReturnTarget::Property(key) = &projection.target else {
                unreachable!("direct property classification requires property target");
            };
            materialize_return_property_values(evaluation, projection, key).await?
        }
        CypherReturnTargetMaterialization::PathFunction => {
            materialize_return_path_function_values(
                evaluation.store,
                evaluation.node_bindings,
                evaluation.edge_bindings,
                evaluation.row_node_values,
                evaluation.row_edge_values,
                evaluation.row_path_bindings,
                &mut *evaluation.nodes,
                &mut *evaluation.edges,
                projection,
                row_count,
            )
            .await?
        }
        CypherReturnTargetMaterialization::ScalarProjection
        | CypherReturnTargetMaterialization::ElementFunction => {
            materialize_return_projection_values(
                evaluation.store,
                evaluation.node_bindings,
                evaluation.edge_bindings,
                evaluation.row_node_values,
                evaluation.row_edge_values,
                evaluation.row_path_bindings,
                &mut *evaluation.nodes,
                &mut *evaluation.edges,
                projection,
                row_count,
            )
            .await?
        }
    };
    if projection.distinct {
        values = distinct_return_values(values)?;
    }
    Ok(values)
}

pub(crate) fn classify_return_target_materialization(
    target: &CypherReturnTarget,
) -> CypherReturnTargetMaterialization {
    match target {
        CypherReturnTarget::All => CypherReturnTargetMaterialization::Star,
        CypherReturnTarget::Element => CypherReturnTargetMaterialization::Element,
        CypherReturnTarget::Property(_) => CypherReturnTargetMaterialization::DirectProperty,
        CypherReturnTarget::PathLength
        | CypherReturnTarget::PathNodes
        | CypherReturnTarget::PathRelationships => CypherReturnTargetMaterialization::PathFunction,
        CypherReturnTarget::NodeLabels
        | CypherReturnTarget::RelationshipType
        | CypherReturnTarget::ElementProperties
        | CypherReturnTarget::ElementKeys
        | CypherReturnTarget::ElementId
        | CypherReturnTarget::RelationshipStartNode
        | CypherReturnTarget::RelationshipEndNode => {
            CypherReturnTargetMaterialization::ElementFunction
        }
        CypherReturnTarget::Literal(_)
        | CypherReturnTarget::MapProjection(_)
        | CypherReturnTarget::ListProjection(_)
        | CypherReturnTarget::Case(_)
        | CypherReturnTarget::Coalesce(_)
        | CypherReturnTarget::PropertyExists(_)
        | CypherReturnTarget::PropertySize(_)
        | CypherReturnTarget::PropertyListIndex(_)
        | CypherReturnTarget::PropertyListSlice(_)
        | CypherReturnTarget::PropertyListContains(_)
        | CypherReturnTarget::PropertyListPredicate(_)
        | CypherReturnTarget::PropertyListElement(_)
        | CypherReturnTarget::PropertyListTail(_)
        | CypherReturnTarget::PropertyAbs(_)
        | CypherReturnTarget::PropertyNumericRound(_)
        | CypherReturnTarget::PropertyNumericSign(_)
        | CypherReturnTarget::PropertyNumericCast(_)
        | CypherReturnTarget::PropertyListCast(_)
        | CypherReturnTarget::PropertyToBoolean(_)
        | CypherReturnTarget::PropertyToString(_)
        | CypherReturnTarget::PropertyStringTransform(_)
        | CypherReturnTarget::PropertyStringTrim(_)
        | CypherReturnTarget::PropertyIsEmpty(_)
        | CypherReturnTarget::PropertyStringReverse(_)
        | CypherReturnTarget::PropertyStringSplit(_)
        | CypherReturnTarget::PropertySubstring(_)
        | CypherReturnTarget::PropertyStringSlice(_)
        | CypherReturnTarget::PropertyReplace(_)
        | CypherReturnTarget::PropertyStringPredicate(_) => {
            CypherReturnTargetMaterialization::ScalarProjection
        }
    }
}

pub(crate) fn classify_return_scalar_projection(
    target: &CypherReturnTarget,
) -> CypherReturnScalarProjectionKind {
    match scalar_return_ast(target) {
        CypherReturnScalarAst::Star => CypherReturnScalarProjectionKind::Star,
        CypherReturnScalarAst::Element => CypherReturnScalarProjectionKind::Element,
        CypherReturnScalarAst::DirectProperty(_) => {
            CypherReturnScalarProjectionKind::DirectProperty
        }
        CypherReturnScalarAst::Literal(_) => CypherReturnScalarProjectionKind::Literal,
        CypherReturnScalarAst::Map(_) => CypherReturnScalarProjectionKind::Map,
        CypherReturnScalarAst::List(_) => CypherReturnScalarProjectionKind::List,
        CypherReturnScalarAst::Conditional(_) => CypherReturnScalarProjectionKind::Conditional,
        CypherReturnScalarAst::Coalesce(_) => CypherReturnScalarProjectionKind::Coalesce,
        CypherReturnScalarAst::PropertyExists(_)
        | CypherReturnScalarAst::PropertySize(_)
        | CypherReturnScalarAst::PropertyIsEmpty(_) => {
            CypherReturnScalarProjectionKind::Introspection
        }
        CypherReturnScalarAst::PropertyListIndex(_)
        | CypherReturnScalarAst::PropertyListSlice(_)
        | CypherReturnScalarAst::PropertyListContains(_)
        | CypherReturnScalarAst::PropertyListElement(_)
        | CypherReturnScalarAst::PropertyListTail(_) => {
            CypherReturnScalarProjectionKind::ListAccess
        }
        CypherReturnScalarAst::PropertyListPredicate(_) => {
            CypherReturnScalarProjectionKind::ListPredicate
        }
        CypherReturnScalarAst::PropertyAbs(_)
        | CypherReturnScalarAst::PropertyNumericRound(_)
        | CypherReturnScalarAst::PropertyNumericSign(_) => {
            CypherReturnScalarProjectionKind::Numeric
        }
        CypherReturnScalarAst::PropertyNumericCast(_)
        | CypherReturnScalarAst::PropertyListCast(_)
        | CypherReturnScalarAst::PropertyToBoolean(_)
        | CypherReturnScalarAst::PropertyToString(_) => {
            CypherReturnScalarProjectionKind::Conversion
        }
        CypherReturnScalarAst::PropertyStringTransform(_)
        | CypherReturnScalarAst::PropertyStringTrim(_)
        | CypherReturnScalarAst::PropertyStringReverse(_)
        | CypherReturnScalarAst::PropertyStringSplit(_)
        | CypherReturnScalarAst::PropertySubstring(_)
        | CypherReturnScalarAst::PropertyStringSlice(_)
        | CypherReturnScalarAst::PropertyReplace(_)
        | CypherReturnScalarAst::PropertyStringPredicate(_) => {
            CypherReturnScalarProjectionKind::String
        }
        CypherReturnScalarAst::ElementFunction => CypherReturnScalarProjectionKind::ElementFunction,
        CypherReturnScalarAst::PathFunction => CypherReturnScalarProjectionKind::PathFunction,
    }
}

pub(crate) fn classify_return_scalar_ast_family(
    expression: &CypherReturnScalarAst<'_>,
) -> CypherReturnScalarAstFamily {
    match expression {
        CypherReturnScalarAst::Star
        | CypherReturnScalarAst::Element
        | CypherReturnScalarAst::DirectProperty(_) => CypherReturnScalarAstFamily::Binding,
        CypherReturnScalarAst::ElementFunction | CypherReturnScalarAst::PathFunction => {
            CypherReturnScalarAstFamily::Wrapper
        }
        CypherReturnScalarAst::Literal(_)
        | CypherReturnScalarAst::Map(_)
        | CypherReturnScalarAst::List(_) => CypherReturnScalarAstFamily::Value,
        CypherReturnScalarAst::Conditional(_) | CypherReturnScalarAst::Coalesce(_) => {
            CypherReturnScalarAstFamily::Control
        }
        CypherReturnScalarAst::PropertyExists(_) | CypherReturnScalarAst::PropertySize(_) => {
            CypherReturnScalarAstFamily::Introspection
        }
        CypherReturnScalarAst::PropertyListIndex(_)
        | CypherReturnScalarAst::PropertyListSlice(_)
        | CypherReturnScalarAst::PropertyListContains(_)
        | CypherReturnScalarAst::PropertyListPredicate(_)
        | CypherReturnScalarAst::PropertyListElement(_)
        | CypherReturnScalarAst::PropertyListTail(_) => CypherReturnScalarAstFamily::List,
        CypherReturnScalarAst::PropertyAbs(_)
        | CypherReturnScalarAst::PropertyNumericRound(_)
        | CypherReturnScalarAst::PropertyNumericSign(_) => CypherReturnScalarAstFamily::Numeric,
        CypherReturnScalarAst::PropertyNumericCast(_)
        | CypherReturnScalarAst::PropertyListCast(_)
        | CypherReturnScalarAst::PropertyToBoolean(_)
        | CypherReturnScalarAst::PropertyToString(_) => CypherReturnScalarAstFamily::Conversion,
        CypherReturnScalarAst::PropertyStringTransform(_)
        | CypherReturnScalarAst::PropertyStringTrim(_)
        | CypherReturnScalarAst::PropertyIsEmpty(_)
        | CypherReturnScalarAst::PropertyStringReverse(_)
        | CypherReturnScalarAst::PropertyStringSplit(_)
        | CypherReturnScalarAst::PropertySubstring(_)
        | CypherReturnScalarAst::PropertyStringSlice(_)
        | CypherReturnScalarAst::PropertyReplace(_)
        | CypherReturnScalarAst::PropertyStringPredicate(_) => CypherReturnScalarAstFamily::String,
    }
}

pub(crate) fn scalar_return_ast(target: &CypherReturnTarget) -> CypherReturnScalarAst<'_> {
    match target {
        CypherReturnTarget::All => CypherReturnScalarAst::Star,
        CypherReturnTarget::Element => CypherReturnScalarAst::Element,
        CypherReturnTarget::Property(key) => CypherReturnScalarAst::DirectProperty(key),
        CypherReturnTarget::Literal(value) => CypherReturnScalarAst::Literal(value),
        CypherReturnTarget::MapProjection(map) => CypherReturnScalarAst::Map(map),
        CypherReturnTarget::ListProjection(list) => CypherReturnScalarAst::List(list),
        CypherReturnTarget::Case(case) => CypherReturnScalarAst::Conditional(case),
        CypherReturnTarget::Coalesce(coalesce) => CypherReturnScalarAst::Coalesce(coalesce),
        CypherReturnTarget::PropertyExists(key) => CypherReturnScalarAst::PropertyExists(key),
        CypherReturnTarget::PropertySize(key) => CypherReturnScalarAst::PropertySize(key),
        CypherReturnTarget::PropertyListIndex(index) => {
            CypherReturnScalarAst::PropertyListIndex(index)
        }
        CypherReturnTarget::PropertyListSlice(slice) => {
            CypherReturnScalarAst::PropertyListSlice(slice)
        }
        CypherReturnTarget::PropertyListContains(contains) => {
            CypherReturnScalarAst::PropertyListContains(contains)
        }
        CypherReturnTarget::PropertyListPredicate(predicate) => {
            CypherReturnScalarAst::PropertyListPredicate(predicate)
        }
        CypherReturnTarget::PropertyListElement(element) => {
            CypherReturnScalarAst::PropertyListElement(element)
        }
        CypherReturnTarget::PropertyListTail(tail) => CypherReturnScalarAst::PropertyListTail(tail),
        CypherReturnTarget::PropertyAbs(abs) => CypherReturnScalarAst::PropertyAbs(abs),
        CypherReturnTarget::PropertyNumericRound(round) => {
            CypherReturnScalarAst::PropertyNumericRound(round)
        }
        CypherReturnTarget::PropertyNumericSign(sign) => {
            CypherReturnScalarAst::PropertyNumericSign(sign)
        }
        CypherReturnTarget::PropertyNumericCast(cast) => {
            CypherReturnScalarAst::PropertyNumericCast(cast)
        }
        CypherReturnTarget::PropertyListCast(cast) => CypherReturnScalarAst::PropertyListCast(cast),
        CypherReturnTarget::PropertyToBoolean(to_boolean) => {
            CypherReturnScalarAst::PropertyToBoolean(to_boolean)
        }
        CypherReturnTarget::PropertyToString(to_string) => {
            CypherReturnScalarAst::PropertyToString(to_string)
        }
        CypherReturnTarget::PropertyStringTransform(transform) => {
            CypherReturnScalarAst::PropertyStringTransform(transform)
        }
        CypherReturnTarget::PropertyStringTrim(trim) => {
            CypherReturnScalarAst::PropertyStringTrim(trim)
        }
        CypherReturnTarget::PropertyIsEmpty(is_empty) => {
            CypherReturnScalarAst::PropertyIsEmpty(is_empty)
        }
        CypherReturnTarget::PropertyStringReverse(reverse) => {
            CypherReturnScalarAst::PropertyStringReverse(reverse)
        }
        CypherReturnTarget::PropertyStringSplit(split) => {
            CypherReturnScalarAst::PropertyStringSplit(split)
        }
        CypherReturnTarget::PropertySubstring(substring) => {
            CypherReturnScalarAst::PropertySubstring(substring)
        }
        CypherReturnTarget::PropertyStringSlice(slice) => {
            CypherReturnScalarAst::PropertyStringSlice(slice)
        }
        CypherReturnTarget::PropertyReplace(replace) => {
            CypherReturnScalarAst::PropertyReplace(replace)
        }
        CypherReturnTarget::PropertyStringPredicate(predicate) => {
            CypherReturnScalarAst::PropertyStringPredicate(predicate)
        }
        CypherReturnTarget::NodeLabels
        | CypherReturnTarget::RelationshipType
        | CypherReturnTarget::ElementProperties
        | CypherReturnTarget::ElementKeys
        | CypherReturnTarget::ElementId
        | CypherReturnTarget::RelationshipStartNode
        | CypherReturnTarget::RelationshipEndNode => CypherReturnScalarAst::ElementFunction,
        CypherReturnTarget::PathLength
        | CypherReturnTarget::PathNodes
        | CypherReturnTarget::PathRelationships => CypherReturnScalarAst::PathFunction,
    }
}

pub(crate) async fn materialize_return_property_values<S>(
    evaluation: &mut CypherReturnEvaluation<'_, S>,
    projection: &CypherReturnProjection,
    key: &str,
) -> Result<Vec<Value>>
where
    S: GraphStore + Sync,
{
    if evaluation
        .row_path_bindings
        .contains_key(&projection.variable)
    {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN path properties are not supported",
        ));
    }
    if let Some(id) = evaluation.node_bindings.get(&projection.variable) {
        Ok(if key == "id" {
            vec![Value::from(id.as_str())]
        } else {
            let node = resolve_bound_node(
                evaluation.store,
                &mut *evaluation.nodes,
                &projection.variable,
                id,
            )
            .await?;
            non_null_return_value(project_node_value(node, key))
                .into_iter()
                .collect()
        })
    } else if let Some(identity) = evaluation.edge_bindings.get(&projection.variable) {
        Ok(if key == "id" {
            identity
                .id
                .as_ref()
                .map(|id| Value::from(id.as_str()))
                .into_iter()
                .collect()
        } else if key == "label" {
            vec![Value::from(identity.label.as_str())]
        } else {
            let edge = resolve_bound_edge_cached(
                evaluation.store,
                &mut *evaluation.edges,
                identity,
                &projection.variable,
            )
            .await?;
            non_null_return_value(project_edge_value(edge, key))
                .into_iter()
                .collect()
        })
    } else if let Some(row_nodes) = evaluation.row_node_values.get(&projection.variable) {
        Ok(row_nodes
            .iter()
            .filter_map(|node| {
                if key == "id" {
                    Some(Value::from(node.id.as_str()))
                } else {
                    non_null_return_value(project_node_value(node, key))
                }
            })
            .collect())
    } else if let Some(row_edges) = evaluation.row_edge_values.get(&projection.variable) {
        Ok(row_edges
            .iter()
            .filter_map(|edge| {
                if key == "id" {
                    edge.id.as_ref().map(|id| Value::from(id.as_str()))
                } else if key == "label" {
                    Some(Value::from(edge.label.as_str()))
                } else {
                    non_null_return_value(project_edge_value(edge, key))
                }
            })
            .collect())
    } else {
        Err(cypher_unresolved_identity(format!(
            "RETURN references variable '{}' that is not bound by the write plan",
            projection.variable
        )))
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_star_row_value<'a, S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &'a mut HashMap<String, Node>,
    edges: &'a mut HashMap<String, Edge>,
    row_index: usize,
) -> Result<Value>
where
    S: GraphStore + Sync,
{
    let write_rows =
        CypherWriteResultRows::new(row_node_values, row_edge_values, row_path_bindings);
    let mut variables = BTreeSet::new();
    variables.extend(node_bindings.keys().cloned());
    variables.extend(edge_bindings.keys().cloned());
    variables.extend(write_rows.variable_names());

    let mut row = serde_json::Map::new();
    for variable in variables {
        let value = if let Some(id) = node_bindings.get(&variable) {
            let node = resolve_bound_node(store, nodes, &variable, id).await?;
            graph_node_value(node)?
        } else if let Some(identity) = edge_bindings.get(&variable) {
            let edge = resolve_bound_edge_cached(store, edges, identity, &variable).await?;
            graph_edge_value(edge)?
        } else {
            match write_rows.binding_kind(&variable) {
                Some(CypherWriteResultBindingKind::Node) => {
                    let row_nodes = write_rows.row_nodes(&variable).ok_or_else(|| {
                        cypher_unsupported_cardinality(format!(
                            "writable Cypher RETURN cannot materialize matched node variable '{variable}'"
                        ))
                    })?;
                    let node = row_nodes.get(row_index).ok_or_else(|| {
                        cypher_unsupported_cardinality(format!(
                            "writable Cypher RETURN cannot materialize matched node variable '{variable}'"
                        ))
                    })?;
                    graph_node_value(node)?
                }
                Some(CypherWriteResultBindingKind::Edge) => {
                    let row_edges = write_rows.row_edges(&variable).ok_or_else(|| {
                        cypher_unsupported_cardinality(format!(
                            "writable Cypher RETURN cannot materialize row-producing relationship variable '{variable}'"
                        ))
                    })?;
                    let edge = row_edges.get(row_index).ok_or_else(|| {
                        cypher_unsupported_cardinality(format!(
                            "writable Cypher RETURN cannot materialize row-producing relationship variable '{variable}'"
                        ))
                    })?;
                    graph_edge_value(edge)?
                }
                Some(CypherWriteResultBindingKind::Path) => {
                    materialize_return_path_value_at(
                        store,
                        node_bindings,
                        edge_bindings,
                        row_node_values,
                        row_edge_values,
                        row_path_bindings,
                        nodes,
                        edges,
                        &variable,
                        row_index,
                    )
                    .await?
                }
                None => {
                    return Err(cypher_unresolved_identity(format!(
                        "RETURN references variable '{variable}' that is not bound by the write plan"
                    )));
                }
            }
        };
        row.insert(variable, value.to_json());
    }
    Ok(Value::Json(serde_json::Value::Object(row)))
}

pub(crate) async fn materialize_return_element_values<S>(
    evaluation: &mut CypherReturnEvaluation<'_, S>,
    projection: &CypherReturnProjection,
) -> Result<Vec<Value>>
where
    S: GraphStore + Sync,
{
    if let Some(id) = evaluation.node_bindings.get(&projection.variable) {
        let node = resolve_bound_node(
            evaluation.store,
            &mut *evaluation.nodes,
            &projection.variable,
            id,
        )
        .await?;
        Ok(vec![graph_node_value(node)?])
    } else if let Some(identity) = evaluation.edge_bindings.get(&projection.variable) {
        let edge = resolve_bound_edge_cached(
            evaluation.store,
            &mut *evaluation.edges,
            identity,
            &projection.variable,
        )
        .await?;
        Ok(vec![graph_edge_value(edge)?])
    } else if let Some(row_nodes) = evaluation.row_node_values.get(&projection.variable) {
        row_nodes.iter().map(graph_node_value).collect()
    } else if let Some(row_edges) = evaluation.row_edge_values.get(&projection.variable) {
        row_edges.iter().map(graph_edge_value).collect()
    } else if evaluation
        .row_path_bindings
        .contains_key(&projection.variable)
    {
        let write_rows = CypherWriteResultRows::new(
            evaluation.row_node_values,
            evaluation.row_edge_values,
            evaluation.row_path_bindings,
        );
        let row_count = write_rows.path_row_count(&projection.variable)?;
        materialize_return_path_values(
            evaluation.store,
            evaluation.node_bindings,
            evaluation.edge_bindings,
            evaluation.row_node_values,
            evaluation.row_edge_values,
            evaluation.row_path_bindings,
            &mut *evaluation.nodes,
            &mut *evaluation.edges,
            &projection.variable,
            row_count,
        )
        .await
    } else {
        Err(cypher_unresolved_identity(format!(
            "RETURN references variable '{}' that is not bound by the write plan",
            projection.variable
        )))
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_path_values<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    variable: &str,
    row_count: usize,
) -> Result<Vec<Value>>
where
    S: GraphStore + Sync,
{
    let mut values = Vec::with_capacity(row_count);
    for row_index in 0..row_count {
        values.push(
            materialize_return_path_value_at(
                store,
                node_bindings,
                edge_bindings,
                row_node_values,
                row_edge_values,
                row_path_bindings,
                nodes,
                edges,
                variable,
                row_index,
            )
            .await?,
        );
    }
    Ok(values)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_path_function_value_at<S>(
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
    let path = materialize_return_path_value_at(
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
    .await?;
    let Value::Json(path) = path else {
        return Err(GrustError::CypherExecution(
            "RETURN path materialization did not produce JSON".to_string(),
        ));
    };
    match projection.target {
        CypherReturnTarget::PathLength => {
            let relationships = path
                .get("relationships")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    GrustError::CypherExecution(
                        "RETURN path materialization is missing relationships".to_string(),
                    )
                })?;
            count_value(relationships.len())
        }
        CypherReturnTarget::PathNodes => Ok(Value::Json(path.get("nodes").cloned().ok_or_else(
            || {
                GrustError::CypherExecution(
                    "RETURN path materialization is missing nodes".to_string(),
                )
            },
        )?)),
        CypherReturnTarget::PathRelationships => Ok(Value::Json(
            path.get("relationships").cloned().ok_or_else(|| {
                GrustError::CypherExecution(
                    "RETURN path materialization is missing relationships".to_string(),
                )
            })?,
        )),
        _ => Err(cypher_unsupported_cardinality(
            "RETURN path function materializer received a non-path projection",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_return_path_function_values<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    nodes: &mut HashMap<String, Node>,
    edges: &mut HashMap<String, Edge>,
    projection: &CypherReturnProjection,
    row_count: usize,
) -> Result<Vec<Value>>
where
    S: GraphStore + Sync,
{
    let mut values = Vec::with_capacity(row_count);
    for row_index in 0..row_count {
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
        if let Some(value) = non_null_return_value(value) {
            values.push(value);
        }
    }
    Ok(values)
}

pub(crate) fn evaluate_non_count_aggregate(
    aggregate: CypherReturnAggregate,
    values: Vec<Value>,
) -> Result<Value> {
    match aggregate {
        CypherReturnAggregate::Count => unreachable!("COUNT handled before non-count aggregate"),
        CypherReturnAggregate::Sum => sum_return_values(&values),
        CypherReturnAggregate::Avg => avg_return_values(&values),
        CypherReturnAggregate::Min => Ok(values
            .into_iter()
            .min_by(compare_return_values)
            .unwrap_or(Value::Null)),
        CypherReturnAggregate::Max => Ok(values
            .into_iter()
            .max_by(compare_return_values)
            .unwrap_or(Value::Null)),
        CypherReturnAggregate::Collect => Ok(Value::Json(serde_json::Value::Array(
            values.into_iter().map(|value| value.to_json()).collect(),
        ))),
    }
}

pub(crate) fn distinct_return_values(values: Vec<Value>) -> Result<Vec<Value>> {
    let mut seen = BTreeSet::new();
    let mut distinct = Vec::with_capacity(values.len());
    for value in values {
        let key = serde_json::to_string(&value.to_json()).map_err(|err| {
            GrustError::CypherExecution(format!(
                "RETURN DISTINCT aggregate value serialization failed: {err}"
            ))
        })?;
        if seen.insert(key) {
            distinct.push(value);
        }
    }
    Ok(distinct)
}

pub(crate) fn sum_return_values(values: &[Value]) -> Result<Value> {
    let mut int_sum = 0i64;
    let mut float_sum = 0.0f64;
    let mut saw_float = false;
    for value in values {
        match value {
            Value::Int(value) if !saw_float => {
                int_sum = int_sum.checked_add(*value).ok_or_else(|| {
                    GrustError::CypherExecution("RETURN SUM integer overflow".to_string())
                })?;
            }
            Value::Int(value) => {
                float_sum += *value as f64;
            }
            Value::Float(value) => {
                if !saw_float {
                    float_sum = int_sum as f64;
                    saw_float = true;
                }
                float_sum += *value;
            }
            other => {
                return Err(cypher_unsupported_cardinality(format!(
                    "RETURN SUM only supports numeric values, got {:?}",
                    other
                )));
            }
        }
    }
    if values.is_empty() {
        Ok(Value::Null)
    } else if saw_float {
        Ok(Value::Float(float_sum))
    } else {
        Ok(Value::Int(int_sum))
    }
}

pub(crate) fn avg_return_values(values: &[Value]) -> Result<Value> {
    if values.is_empty() {
        return Ok(Value::Null);
    }
    let mut sum = 0.0f64;
    for value in values {
        match value {
            Value::Int(value) => sum += *value as f64,
            Value::Float(value) => sum += *value,
            other => {
                return Err(cypher_unsupported_cardinality(format!(
                    "RETURN AVG only supports numeric values, got {:?}",
                    other
                )));
            }
        }
    }
    Ok(Value::Float(sum / values.len() as f64))
}

pub(crate) fn count_distinct_elements(
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_values: &HashMap<String, Vec<Edge>>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    projection: &CypherReturnProjection,
    row_count: usize,
) -> Result<usize> {
    if row_path_bindings.contains_key(&projection.variable) {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN aggregates over path variables are not supported",
        ));
    }
    if let Some(id) = node_bindings.get(&projection.variable) {
        return Ok(usize::from(row_count > 0 && !id.as_str().is_empty()));
    }
    if let Some(identity) = edge_bindings.get(&projection.variable) {
        return Ok(usize::from(
            row_count > 0 && !edge_identity_count_key(identity).is_empty(),
        ));
    }
    if let Some(row_nodes) = row_node_values.get(&projection.variable) {
        return Ok(row_nodes
            .iter()
            .map(|node| node.id.as_str().to_string())
            .collect::<BTreeSet<_>>()
            .len());
    }
    if let Some(row_edges) = row_edge_values.get(&projection.variable) {
        return Ok(row_edges
            .iter()
            .map(edge_count_key)
            .collect::<BTreeSet<_>>()
            .len());
    }
    Err(cypher_unresolved_identity(format!(
        "RETURN references variable '{}' that is not bound by the write plan",
        projection.variable
    )))
}

pub(crate) fn count_distinct_values(values: impl IntoIterator<Item = Value>) -> Result<usize> {
    let mut distinct = BTreeSet::new();
    for value in values {
        let key = serde_json::to_string(&value.to_json()).map_err(|err| {
            GrustError::CypherExecution(format!(
                "COUNT(DISTINCT) value serialization failed: {err}"
            ))
        })?;
        distinct.insert(key);
    }
    Ok(distinct.len())
}

pub(crate) fn non_null_return_value(value: Value) -> Option<Value> {
    (value != Value::Null).then_some(value)
}

pub(crate) fn edge_identity_count_key(identity: &CypherBoundEdgeIdentity) -> String {
    identity
        .id
        .as_ref()
        .map(|id| format!("id:{}", id.as_str()))
        .unwrap_or_else(|| {
            format!(
                "struct:{}:{}:{}",
                identity.from.as_str(),
                identity.label.as_str(),
                identity.to.as_str()
            )
        })
}

pub(crate) fn edge_count_key(edge: &Edge) -> String {
    edge.id
        .as_ref()
        .map(|id| format!("id:{}", id.as_str()))
        .unwrap_or_else(|| {
            format!(
                "struct:{}:{}:{}",
                edge.from.as_str(),
                edge.label.as_str(),
                edge.to.as_str()
            )
        })
}

pub(crate) fn count_value(count: usize) -> Result<Value> {
    i64::try_from(count).map(Value::Int).map_err(|_| {
        GrustError::CypherExecution(format!("RETURN count {count} cannot fit in int64"))
    })
}

pub(crate) fn apply_return_distinct(rows: &mut Vec<Vec<Value>>) -> Result<()> {
    let mut seen = BTreeSet::new();
    let mut distinct = Vec::with_capacity(rows.len());
    for row in rows.drain(..) {
        let key = serde_json::to_string(
            &row.iter()
                .map(Value::to_json)
                .collect::<Vec<serde_json::Value>>(),
        )
        .map_err(|err| {
            GrustError::CypherExecution(format!("RETURN DISTINCT row serialization failed: {err}"))
        })?;
        if seen.insert(key) {
            distinct.push(row);
        }
    }
    *rows = distinct;
    Ok(())
}

/// Applies `ORDER BY` (stable), then `SKIP`, then `LIMIT` to materialized rows.
pub(crate) fn apply_return_control(
    rows: &mut Vec<Vec<Value>>,
    order_by: &[CypherOrderItem],
    skip: Option<usize>,
    limit: Option<usize>,
) {
    if !order_by.is_empty() {
        rows.sort_by(|a, b| {
            for item in order_by {
                let ordering = compare_return_values(&a[item.column], &b[item.column]);
                let ordering = if item.descending {
                    ordering.reverse()
                } else {
                    ordering
                };
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            }
            std::cmp::Ordering::Equal
        });
    }
    if let Some(skip) = skip {
        rows.drain(0..skip.min(rows.len()));
    }
    if let Some(limit) = limit {
        rows.truncate(limit);
    }
}

/// Total ordering over result values for `ORDER BY`. Nulls sort last (ascending),
/// numbers compare numerically, strings and bools compare naturally, and values
/// of different kinds fall back to a stable type rank so sorting is deterministic.
pub(crate) fn compare_return_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Greater,
        (_, Value::Null) => Ordering::Less,
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Int(x), Value::Float(y)) => (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Float(x), Value::Int(y)) => x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        // Temporal ordering: lexicographic over the RFC 3339 form, which is
        // chronological for a consistent offset (the normalized `Z` form the
        // reference produces). Without this, two datetimes compared equal.
        (Value::DateTime(x), Value::DateTime(y)) => x.as_str().cmp(y.as_str()),
        (Value::Decimal(x), Value::Decimal(y)) => x.cmp(y),
        (Value::Duration(x), Value::Duration(y)) => x.cmp(y),
        _ => value_kind_rank(a).cmp(&value_kind_rank(b)),
    }
}

pub(crate) fn value_kind_rank(value: &Value) -> u8 {
    match value {
        Value::Bool(_) => 0,
        Value::Int(_) | Value::Float(_) => 1,
        Value::Decimal(_) => 2,
        Value::String(_) => 3,
        Value::DateTime(_) => 4,
        Value::Duration(_) => 5,
        Value::StringArray(_) | Value::IntArray(_) | Value::FloatArray(_) => 6,
        Value::Json(_) | Value::Path(_) | Value::Graph(_) => 7,
        Value::Null => 8,
    }
}

pub(crate) async fn resolve_bound_edge<S>(
    store: &S,
    identity: &CypherBoundEdgeIdentity,
    variable: &str,
) -> Result<Edge>
where
    S: GraphStore + Sync,
{
    let edges = store
        .get_edges(EdgeQuery {
            from: Some(identity.from.clone()),
            to: Some(identity.to.clone()),
            label: Some(identity.label.clone()),
        })
        .await?;
    edges
        .into_iter()
        .find(|edge| identity.id.as_ref().is_none_or(|id| edge.id.as_ref() == Some(id)))
        .ok_or_else(|| {
            GrustError::CypherExecution(format!(
                "RETURN relationship variable '{variable}' resolved to an edge that does not exist after the write"
            ))
        })
}

pub(crate) async fn row_edge_return_values_on_store<S>(
    store: &S,
    bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
) -> Result<HashMap<String, Vec<Edge>>>
where
    S: GraphStore + Sync,
{
    let mut values = HashMap::new();
    for (variable, binding) in bindings {
        let edges = store
            .get_edges(EdgeQuery {
                from: None,
                to: None,
                label: Some(binding.label.clone()),
            })
            .await?;
        let mut rows = Vec::new();
        for edge in edges {
            if !props_match(&edge.props, &binding.props) {
                continue;
            }
            let Some(from) = store.get_node(&edge.from).await? else {
                continue;
            };
            if !node_match(&from, &binding.from) {
                continue;
            }
            let Some(to) = store.get_node(&edge.to).await? else {
                continue;
            };
            if !node_match(&to, &binding.to) {
                continue;
            }
            rows.push(edge);
        }
        match binding.kind {
            GraphMutationPlanKind::Create | GraphMutationPlanKind::Merge => {}
        }
        values.insert(variable.clone(), rows);
    }
    Ok(values)
}

pub async fn row_edge_endpoint_node_values_on_store<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
    edge_values: &HashMap<String, Vec<Edge>>,
) -> Result<HashMap<String, Vec<Node>>>
where
    S: GraphStore + Sync,
{
    let mut values = HashMap::new();
    for (edge_variable, binding) in edge_bindings {
        let Some(edges) = edge_values.get(edge_variable) else {
            continue;
        };
        for (variable, endpoint) in [
            (&binding.from_variable, RowEdgeEndpoint::From),
            (&binding.to_variable, RowEdgeEndpoint::To),
        ] {
            if node_bindings.contains_key(variable) || values.contains_key(variable) {
                continue;
            }
            let mut nodes = Vec::with_capacity(edges.len());
            for edge in edges {
                let id = match endpoint {
                    RowEdgeEndpoint::From => &edge.from,
                    RowEdgeEndpoint::To => &edge.to,
                };
                let node = store.get_node(id).await?.ok_or_else(|| {
                    GrustError::CypherExecution(format!(
                        "RETURN endpoint variable '{variable}' resolved to missing node '{}'",
                        id.as_str()
                    ))
                })?;
                nodes.push(node);
            }
            values.insert(variable.clone(), nodes);
        }
    }
    Ok(values)
}

pub async fn row_edge_match_path_endpoint_node_values_on_store<S>(
    store: &S,
    node_bindings: &HashMap<String, NodeId>,
    row_node_values: &HashMap<String, Vec<Node>>,
    row_edge_match_bindings: &HashMap<String, GraphRelationshipMatch>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    edge_values: &HashMap<String, Vec<Edge>>,
) -> Result<HashMap<String, Vec<Node>>>
where
    S: GraphStore + Sync,
{
    let mut values = HashMap::new();
    for path in row_path_bindings.values() {
        if !row_edge_match_bindings.contains_key(&path.edge_variable) {
            continue;
        }
        let Some(edges) = edge_values.get(&path.edge_variable) else {
            continue;
        };
        for (variable, endpoint) in [
            (&path.from_variable, RowEdgeEndpoint::From),
            (&path.to_variable, RowEdgeEndpoint::To),
        ] {
            if node_bindings.contains_key(variable)
                || row_node_values.contains_key(variable)
                || values.contains_key(variable)
            {
                continue;
            }
            let mut nodes = Vec::with_capacity(edges.len());
            for edge in edges {
                let id = match endpoint {
                    RowEdgeEndpoint::From => &edge.from,
                    RowEdgeEndpoint::To => &edge.to,
                };
                let node = store.get_node(id).await?.ok_or_else(|| {
                    GrustError::CypherExecution(format!(
                        "RETURN matched path endpoint variable '{variable}' resolved to missing node '{}'",
                        id.as_str()
                    ))
                })?;
                nodes.push(node);
            }
            values.insert(variable.clone(), nodes);
        }
    }
    Ok(values)
}

#[derive(Clone, Copy)]
pub(crate) enum RowEdgeEndpoint {
    From,
    To,
}

pub async fn collect_row_node_ids_for_operation<S>(
    store: &S,
    operation: &GraphMutationPlanOp,
    bindings: &HashMap<String, GraphNodeMatch>,
    values: &mut HashMap<String, Vec<NodeId>>,
) -> Result<()>
where
    S: GraphStore + Sync,
{
    let Some(operation_match) = operation_node_match(operation) else {
        return Ok(());
    };
    for (variable, binding) in bindings {
        if values.contains_key(variable) || binding != &operation_match {
            continue;
        }
        let nodes = matching_nodes_on_store(store, binding, variable).await?;
        values.insert(
            variable.clone(),
            nodes.into_iter().map(|node| node.id).collect(),
        );
    }
    Ok(())
}

pub async fn collect_row_edge_keys_for_operation<S>(
    store: &S,
    operation: &GraphMutationPlanOp,
    bindings: &HashMap<String, GraphRelationshipMatch>,
    values: &mut HashMap<String, Vec<String>>,
) -> Result<()>
where
    S: GraphStore + Sync,
{
    let Some(operation_match) = operation_relationship_match(operation) else {
        return Ok(());
    };
    for (variable, binding) in bindings {
        if values.contains_key(variable) || binding != &operation_match {
            continue;
        }
        let edges = matching_edges_on_store(store, binding).await?;
        values.insert(
            variable.clone(),
            edges.into_iter().map(|edge| edge_key(&edge)).collect(),
        );
    }
    Ok(())
}

pub async fn collect_deleted_row_node_values_for_operation<S>(
    store: &S,
    operation: &GraphMutationPlanOp,
    bindings: &HashMap<String, GraphNodeMatch>,
    values: &mut HashMap<String, Vec<Node>>,
) -> Result<()>
where
    S: GraphStore + Sync,
{
    let GraphMutationPlanOp::DeleteMatchingNodes {
        label,
        props,
        predicates,
        ..
    } = operation
    else {
        return Ok(());
    };
    let operation_match = GraphNodeMatch {
        label: label.clone(),
        props: props.clone(),
        predicates: predicates.clone(),
    };
    for (variable, binding) in bindings {
        if values.contains_key(variable) || binding != &operation_match {
            continue;
        }
        values.insert(
            variable.clone(),
            matching_nodes_on_store(store, binding, variable).await?,
        );
    }
    Ok(())
}

pub async fn collect_deleted_row_edge_values_for_operation<S>(
    store: &S,
    operation: &GraphMutationPlanOp,
    bindings: &HashMap<String, GraphRelationshipMatch>,
    values: &mut HashMap<String, Vec<Edge>>,
) -> Result<()>
where
    S: GraphStore + Sync,
{
    let relationship = match operation {
        GraphMutationPlanOp::DeleteMatchingEdges { relationship, .. }
        | GraphMutationPlanOp::DeleteRelationshipRows { relationship, .. } => relationship,
        _ => return Ok(()),
    };
    for (variable, binding) in bindings {
        if values.contains_key(variable) || binding != relationship {
            continue;
        }
        values.insert(
            variable.clone(),
            matching_edges_on_store(store, binding).await?,
        );
    }
    Ok(())
}

pub async fn collect_deleted_path_endpoint_node_values_for_operation<S>(
    store: &S,
    operation: &GraphMutationPlanOp,
    row_edge_match_bindings: &HashMap<String, GraphRelationshipMatch>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    values: &mut HashMap<String, Vec<Node>>,
) -> Result<()>
where
    S: GraphStore + Sync,
{
    let relationship = match operation {
        GraphMutationPlanOp::DeleteMatchingEdges { relationship, .. }
        | GraphMutationPlanOp::DeleteRelationshipRows { relationship, .. } => relationship,
        _ => return Ok(()),
    };
    for path in row_path_bindings.values() {
        let Some(binding) = row_edge_match_bindings.get(&path.edge_variable) else {
            continue;
        };
        if binding != relationship {
            continue;
        }
        let edges = matching_edges_on_store(store, binding).await?;
        for (variable, endpoint) in [
            (&path.from_variable, RowEdgeEndpoint::From),
            (&path.to_variable, RowEdgeEndpoint::To),
        ] {
            if values.contains_key(variable) {
                continue;
            }
            let mut nodes = Vec::with_capacity(edges.len());
            for edge in &edges {
                let id = match endpoint {
                    RowEdgeEndpoint::From => &edge.from,
                    RowEdgeEndpoint::To => &edge.to,
                };
                let node = store.get_node(id).await?.ok_or_else(|| {
                    GrustError::CypherExecution(format!(
                        "RETURN deleted path endpoint variable '{variable}' resolved to missing node '{}'",
                        id.as_str()
                    ))
                })?;
                nodes.push(node);
            }
            values.insert(variable.clone(), nodes);
        }
    }
    Ok(())
}

pub(crate) fn operation_node_match(operation: &GraphMutationPlanOp) -> Option<GraphNodeMatch> {
    match operation {
        GraphMutationPlanOp::PatchMatchingNodes {
            label,
            props,
            predicates,
            ..
        }
        | GraphMutationPlanOp::UpdateMatchingNodeProperty {
            label,
            props,
            predicates,
            ..
        }
        | GraphMutationPlanOp::RemoveMatchingNodeProps {
            label,
            props,
            predicates,
            ..
        } => Some(GraphNodeMatch {
            label: label.clone(),
            props: props.clone(),
            predicates: predicates.clone(),
        }),
        _ => None,
    }
}

pub(crate) fn operation_relationship_match(
    operation: &GraphMutationPlanOp,
) -> Option<GraphRelationshipMatch> {
    match operation {
        GraphMutationPlanOp::PatchMatchingEdges { relationship, .. }
        | GraphMutationPlanOp::UpdateMatchingEdgeProperty { relationship, .. }
        | GraphMutationPlanOp::RemoveMatchingEdgeProps { relationship, .. } => {
            Some(relationship.clone())
        }
        _ => None,
    }
}

pub async fn row_node_return_values_on_store<S>(
    store: &S,
    ids: HashMap<String, Vec<NodeId>>,
) -> Result<HashMap<String, Vec<Node>>>
where
    S: GraphStore + Sync,
{
    let mut values = HashMap::new();
    for (variable, ids) in ids {
        let nodes = store.get_nodes(&ids).await?;
        if nodes.len() != ids.len() {
            return Err(GrustError::CypherExecution(format!(
                "RETURN matched node variable '{variable}' includes a node that does not exist after the write"
            )));
        }
        values.insert(variable, nodes);
    }
    Ok(values)
}

pub async fn row_edge_match_return_values_on_store<S>(
    store: &S,
    ids: HashMap<String, Vec<String>>,
) -> Result<HashMap<String, Vec<Edge>>>
where
    S: GraphStore + Sync,
{
    let mut values = HashMap::new();
    for (variable, keys) in ids {
        let mut rows = Vec::with_capacity(keys.len());
        for key in keys {
            let edge = edge_by_key_on_store(store, &key).await?.ok_or_else(|| {
                GrustError::CypherExecution(format!(
                    "RETURN matched relationship variable '{variable}' includes an edge that does not exist after the write"
                ))
            })?;
            rows.push(edge);
        }
        values.insert(variable, rows);
    }
    Ok(values)
}

pub(crate) async fn edge_by_key_on_store<S>(store: &S, key: &str) -> Result<Option<Edge>>
where
    S: GraphStore + Sync,
{
    for edge in store
        .get_edges(EdgeQuery {
            from: None,
            to: None,
            label: None,
        })
        .await?
    {
        if edge_key_matches(&edge, key) {
            return Ok(Some(edge));
        }
    }
    Ok(None)
}

pub(crate) fn merge_cypher_reports(report: &mut CypherMutationReport, next: CypherMutationReport) {
    report.creates += next.creates;
    report.merges += next.merges;
    report.deletes += next.deletes;
    report.patches += next.patches;
    report.property_removes += next.property_removes;
    report.matched_rows += next.matched_rows;
    report.changed_nodes += next.changed_nodes;
    report.changed_edges += next.changed_edges;
    report.node_upserts += next.node_upserts;
    report.edge_upserts += next.edge_upserts;
    report.node_deletes += next.node_deletes;
    report.edge_deletes += next.edge_deletes;
    report.node_patches += next.node_patches;
    report.edge_patches += next.edge_patches;
    report.node_property_removes += next.node_property_removes;
    report.edge_property_removes += next.edge_property_removes;
    report.node_inserts += next.node_inserts;
    report.node_updates += next.node_updates;
    report.edge_inserts += next.edge_inserts;
    report.edge_updates += next.edge_updates;
}

pub fn row_edge_id_policy_generates(
    kind: GraphMutationPlanKind,
    policy: GraphRowEdgeIdPolicy,
) -> bool {
    matches!(
        (kind, policy),
        (
            GraphMutationPlanKind::Create,
            GraphRowEdgeIdPolicy::GenerateForCreate
                | GraphRowEdgeIdPolicy::GenerateForCreateAndMerge
        ) | (
            GraphMutationPlanKind::Merge,
            GraphRowEdgeIdPolicy::GenerateForCreateAndMerge
        )
    )
}

pub enum CypherResolvedUpsertClassification {
    Node { existed: bool },
    Edge { existed: bool },
}

impl CypherResolvedUpsertClassification {
    pub fn record(self, report: &mut CypherMutationReport) {
        match self {
            CypherResolvedUpsertClassification::Node { existed: true } => {
                report.node_updates += 1;
            }
            CypherResolvedUpsertClassification::Node { existed: false } => {
                report.node_inserts += 1;
            }
            CypherResolvedUpsertClassification::Edge { existed: true } => {
                report.edge_updates += 1;
            }
            CypherResolvedUpsertClassification::Edge { existed: false } => {
                report.edge_inserts += 1;
            }
        }
    }
}

pub(crate) async fn matching_edges_on_store<S>(
    store: &S,
    relationship: &GraphRelationshipMatch,
) -> Result<Vec<Edge>>
where
    S: GraphStore + Sync,
{
    let edges = store
        .get_edges(EdgeQuery {
            from: None,
            to: None,
            label: Some(relationship.label.clone()),
        })
        .await?;
    let mut rows = Vec::new();
    for edge in edges {
        if relationship
            .id
            .as_ref()
            .is_some_and(|id| edge.id.as_ref() != Some(id))
        {
            continue;
        }
        if !props_match(&edge.props, &relationship.props)
            || !relationship
                .predicates
                .iter()
                .all(|predicate| predicate.matches(edge.props.get(&predicate.key)))
        {
            continue;
        }
        let Some(from) = store.get_node(&edge.from).await? else {
            continue;
        };
        if !node_match(&from, &relationship.from) {
            continue;
        }
        let Some(to) = store.get_node(&edge.to).await? else {
            continue;
        };
        if !node_match(&to, &relationship.to) {
            continue;
        }
        rows.push(edge);
    }
    Ok(rows)
}

pub(crate) async fn matching_nodes_on_store<S>(
    store: &S,
    binding: &GraphNodeMatch,
    variable: &str,
) -> Result<Vec<Node>>
where
    S: GraphStore + Sync,
{
    let candidates = if let Some(id) = binding
        .props
        .get("id")
        .and_then(Value::as_str)
        .map(NodeId::new)
    {
        store.get_node(&id).await?.into_iter().collect()
    } else if let Some(label) = &binding.label {
        if binding.props.is_empty() {
            store
                .traverse(Traversal {
                    start: Start::NodesByLabel(label.clone()),
                    steps: Vec::new(),
                    limit: None,
                })
                .await?
        } else if binding.props.len() == 1 {
            let (key, value) = binding
                .props
                .iter()
                .next()
                .expect("checked single property");
            store
                .traverse(Traversal {
                    start: Start::NodesByProperty {
                        label: label.clone(),
                        key: key.clone(),
                        value: value.clone(),
                    },
                    steps: Vec::new(),
                    limit: None,
                })
                .await?
        } else {
            return Err(cypher_unsupported_cardinality(format!(
                "writable Cypher RETURN cannot portably materialize matched node variable '{variable}' with multiple property predicates"
            )));
        }
    } else {
        return Err(cypher_unsupported_cardinality(format!(
            "writable Cypher RETURN cannot portably materialize matched node variable '{variable}' without a label or id"
        )));
    };
    Ok(candidates
        .into_iter()
        .filter(|node| node_match(node, binding))
        .collect())
}

pub(crate) fn props_match(actual: &Props, expected: &Props) -> bool {
    expected
        .iter()
        .all(|(key, value)| actual.get(key) == Some(value))
}

pub(crate) fn node_match(node: &Node, expected: &GraphNodeMatch) -> bool {
    expected
        .label
        .as_ref()
        .is_none_or(|label| &node.label == label)
        && expected.props.iter().all(|(key, value)| {
            if key == "id" {
                value.as_str().is_some_and(|id| node.id.as_str() == id)
            } else {
                node.props.get(key) == Some(value)
            }
        })
        && expected
            .predicates
            .iter()
            .all(|predicate| predicate.matches(node.props.get(&predicate.key)))
}

pub(crate) async fn resolve_bound_node<'a, S>(
    store: &S,
    nodes: &'a mut HashMap<String, Node>,
    variable: &str,
    id: &NodeId,
) -> Result<&'a Node>
where
    S: GraphStore + Sync,
{
    if !nodes.contains_key(variable) {
        let node = store.get_node(id).await?.ok_or_else(|| {
            GrustError::CypherExecution(format!(
                "RETURN variable '{variable}' resolved to node '{}' but the node does not exist after the write",
                id.as_str()
            ))
        })?;
        nodes.insert(variable.to_string(), node);
    }
    Ok(nodes
        .get(variable)
        .expect("node inserted before projection evaluation"))
}

pub(crate) async fn resolve_bound_edge_cached<'a, S>(
    store: &S,
    edges: &'a mut HashMap<String, Edge>,
    identity: &CypherBoundEdgeIdentity,
    variable: &str,
) -> Result<&'a Edge>
where
    S: GraphStore + Sync,
{
    if !edges.contains_key(variable) {
        let edge = resolve_bound_edge(store, identity, variable).await?;
        edges.insert(variable.to_string(), edge);
    }
    Ok(edges
        .get(variable)
        .expect("edge inserted before projection evaluation"))
}

pub(crate) fn project_node_value(node: &Node, key: &str) -> Value {
    if key == "label" {
        Value::from(node.label.as_str())
    } else {
        node.props.get(key).cloned().unwrap_or(Value::Null)
    }
}

pub(crate) fn project_edge_value(edge: &Edge, key: &str) -> Value {
    edge.props.get(key).cloned().unwrap_or(Value::Null)
}

pub(crate) fn graph_node_value(node: &Node) -> Result<Value> {
    serde_json::to_value(node).map(Value::from).map_err(|err| {
        GrustError::CypherExecution(format!("RETURN node serialization failed: {err}"))
    })
}

pub(crate) fn graph_edge_value(edge: &Edge) -> Result<Value> {
    serde_json::to_value(edge).map(Value::from).map_err(|err| {
        GrustError::CypherExecution(format!("RETURN relationship serialization failed: {err}"))
    })
}

pub async fn execute_cypher_mutation_returning_with_options_on_store<S>(
    store: &S,
    cypher: &str,
    options: CypherMutationOptions,
) -> Result<CypherMutationTableResult>
where
    S: CypherMutationExecutor + Sync,
{
    let create_mode = options.create_mode;
    let planned = sail_cypher_mutation_plan_with_return_options(cypher, options.clone())?;
    if create_mode == CypherCreateMode::ErrorIfExists {
        check_strict_create_conflicts_on_store(store, &planned.plan)
            .await
            .map_err(cypher_execution_error)?;
    }
    let mut row_node_ids = HashMap::new();
    let mut row_edge_keys = HashMap::new();
    let mut row_node_pre_delete_values = HashMap::new();
    let mut row_edge_pre_delete_values = HashMap::new();
    let mut report = CypherMutationReport::default();
    for operation in &planned.plan.operations {
        collect_deleted_row_node_values_for_operation(
            store,
            operation,
            &planned.row_node_bindings,
            &mut row_node_pre_delete_values,
        )
        .await
        .map_err(cypher_execution_error)?;
        collect_deleted_row_edge_values_for_operation(
            store,
            operation,
            &planned.row_edge_match_bindings,
            &mut row_edge_pre_delete_values,
        )
        .await
        .map_err(cypher_execution_error)?;
        collect_deleted_path_endpoint_node_values_for_operation(
            store,
            operation,
            &planned.row_edge_match_bindings,
            &planned.row_path_bindings,
            &mut row_node_pre_delete_values,
        )
        .await
        .map_err(cypher_execution_error)?;
        collect_row_node_ids_for_operation(
            store,
            operation,
            &planned.row_node_bindings,
            &mut row_node_ids,
        )
        .await
        .map_err(cypher_execution_error)?;
        collect_row_edge_keys_for_operation(
            store,
            operation,
            &planned.row_edge_match_bindings,
            &mut row_edge_keys,
        )
        .await
        .map_err(cypher_execution_error)?;
        let operation_plan = GraphMutationPlan::new(vec![operation.clone()]);
        let operation_report = store
            .execute_cypher_mutation_plan(&operation_plan)
            .await
            .map_err(cypher_execution_error)?;
        merge_cypher_reports(&mut report, operation_report);
    }
    let mut row_node_values = row_node_return_values_on_store(store, row_node_ids)
        .await
        .map_err(cypher_execution_error)?;
    row_node_values.extend(row_node_pre_delete_values);
    let mut row_edge_values = row_edge_match_return_values_on_store(store, row_edge_keys)
        .await
        .map_err(cypher_execution_error)?;
    row_edge_values.extend(row_edge_pre_delete_values);
    row_edge_values.extend(
        row_edge_return_values_on_store(store, &planned.row_edge_bindings)
            .await
            .map_err(cypher_execution_error)?,
    );
    for (variable, values) in row_edge_endpoint_node_values_on_store(
        store,
        &planned.node_bindings,
        &planned.row_edge_bindings,
        &row_edge_values,
    )
    .await
    .map_err(cypher_execution_error)?
    {
        row_node_values.entry(variable).or_insert(values);
    }
    for (variable, values) in row_edge_match_path_endpoint_node_values_on_store(
        store,
        &planned.node_bindings,
        &row_node_values,
        &planned.row_edge_match_bindings,
        &planned.row_path_bindings,
        &row_edge_values,
    )
    .await
    .map_err(cypher_execution_error)?
    {
        row_node_values.entry(variable).or_insert(values);
    }
    let table = evaluate_cypher_return_table(
        store,
        &planned.node_bindings,
        &planned.edge_bindings,
        &row_node_values,
        &row_edge_values,
        &planned.row_path_bindings,
        &planned.return_clause,
    )
    .await
    .map_err(cypher_execution_error)?;
    let mutation = cypher_mutation_result_from_plan(
        report,
        planned.generated_node_ids,
        &planned.plan,
        &options,
        &planned.row_edge_bindings,
        &row_edge_values,
    )?;
    Ok(CypherMutationTableResult { mutation, table })
}

pub(crate) async fn check_strict_create_conflicts_on_store<S>(
    store: &S,
    plan: &GraphMutationPlan,
) -> Result<()>
where
    S: GraphStore + Sync,
{
    check_strict_create_plan_conflicts(plan)?;
    for operation in &plan.operations {
        match operation {
            GraphMutationPlanOp::UpsertNode {
                kind: GraphMutationPlanKind::Create,
                node,
            } => {
                if store.get_node(&node.id).await?.is_some() {
                    return Err(GrustError::Unsupported(format!(
                        "Cypher CREATE would overwrite existing node '{}'",
                        node.id.as_str()
                    )));
                }
            }
            GraphMutationPlanOp::UpsertEdge {
                kind: GraphMutationPlanKind::Create,
                edge,
            } => {
                let mut existing = store
                    .get_edges(EdgeQuery {
                        from: Some(edge.from.clone()),
                        to: Some(edge.to.clone()),
                        label: Some(edge.label.clone()),
                    })
                    .await?;
                if edge.id.is_some() {
                    existing.extend(store.get_edges(EdgeQuery::default()).await?);
                }
                if strict_create_edge_conflicts(edge, &existing) {
                    return Err(GrustError::Unsupported(format!(
                        "Cypher CREATE would overwrite existing edge '{}'",
                        edge_key(edge)
                    )));
                }
            }
            GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
                kind: GraphMutationPlanKind::Create,
                ..
            } => {
                return Err(cypher_unsupported_cardinality(
                    "generic writable Cypher RETURN execution does not support strict CREATE checks for row-producing edge writes",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn check_strict_create_plan_conflicts(plan: &GraphMutationPlan) -> Result<()> {
    let mut node_ids = HashSet::with_capacity(plan.operations.len());
    let mut edge_ids = HashSet::with_capacity(plan.operations.len());
    let mut structural_edges = HashSet::with_capacity(plan.operations.len());
    for operation in &plan.operations {
        match operation {
            GraphMutationPlanOp::UpsertNode {
                kind: GraphMutationPlanKind::Create,
                node,
            } => {
                if !node_ids.insert(node.id.clone()) {
                    return Err(GrustError::Unsupported(format!(
                        "Cypher CREATE batch contains duplicate node '{}'",
                        node.id.as_str()
                    )));
                }
            }
            GraphMutationPlanOp::UpsertEdge {
                kind: GraphMutationPlanKind::Create,
                edge,
            } => {
                let duplicate_id = edge
                    .id
                    .as_ref()
                    .is_some_and(|id| !edge_ids.insert(id.clone()));
                let duplicate_structure = !structural_edges.insert((
                    edge.from.clone(),
                    edge.label.clone(),
                    edge.to.clone(),
                ));
                if duplicate_id || duplicate_structure {
                    return Err(GrustError::Unsupported(format!(
                        "Cypher CREATE batch contains duplicate edge '{}'",
                        edge_key(edge)
                    )));
                }
            }
            _ => {}
        }
    }
    Ok(())
}
