//! RETURN/projection AST types and parsing, plus match/return split helpers (extracted from lib.rs).

use crate::*;

pub(crate) fn split_match_delete(statement: &str) -> Result<(&str, &str)> {
    if let Some(index) = find_unquoted_keyword(statement, "DELETE") {
        let pattern = statement[..index].trim();
        let target = statement[index + "DELETE".len()..].trim();
        if pattern.is_empty() || target.is_empty() {
            return Err(GrustError::Unsupported(
                "MATCH DELETE requires both a pattern and a delete target".to_string(),
            ));
        }
        return Ok((pattern, target));
    }
    Err(GrustError::Unsupported(
        "only ID-resolved MATCH ... DELETE is supported in writable Cypher".to_string(),
    ))
}

pub(crate) fn parse_match_delete_targets(targets: &str) -> Result<Vec<String>> {
    split_top_level_commas(targets)?
        .into_iter()
        .map(str::trim)
        .map(|target| {
            if target.is_empty() {
                Err(cypher_syntax("MATCH DELETE contains an empty target"))
            } else {
                parse_required_cypher_variable(target, "MATCH DELETE target")
            }
        })
        .collect()
}

pub(crate) fn split_match_edge_upsert<'a>(
    statement: &'a str,
    keyword: &str,
) -> Result<(&'a str, &'a str)> {
    if let Some(index) = find_unquoted_keyword(statement, keyword) {
        let match_clause = statement[..index].trim();
        let edge_pattern = statement[index + keyword.len()..].trim();
        if match_clause.is_empty() || edge_pattern.is_empty() {
            return Err(GrustError::Unsupported(format!(
                "MATCH {keyword} requires both matched node patterns and an edge pattern"
            )));
        }
        return Ok((match_clause, edge_pattern));
    }
    Err(GrustError::Unsupported(format!(
        "only ID-resolved MATCH ... {keyword} edge is supported in writable Cypher",
    )))
}

pub(crate) fn parse_path_binding<'a>(
    pattern: &'a str,
    context: &str,
) -> Result<(Option<String>, &'a str)> {
    let Some(index) = find_unquoted(pattern, '=') else {
        return Ok((None, pattern.trim()));
    };
    let variable = parse_required_cypher_variable(
        pattern[..index].trim(),
        &format!("{context} path variable"),
    )?;
    let relationship_pattern = pattern[index + 1..].trim();
    if !relationship_pattern.starts_with('(') {
        return Err(cypher_syntax(format!(
            "{context} path variable must bind a relationship pattern"
        )));
    }
    Ok((Some(variable), relationship_pattern))
}

pub(crate) fn parse_row_path_binding(pattern: &str) -> Result<(Option<String>, &str)> {
    parse_path_binding(pattern, "MATCH CREATE/MERGE")
}

pub(crate) fn split_match_set(statement: &str) -> Result<(&str, &str)> {
    if let Some(index) = find_unquoted_keyword(statement, "SET") {
        let pattern = statement[..index].trim();
        let assignment = statement[index + "SET".len()..].trim();
        if pattern.is_empty() || assignment.is_empty() {
            return Err(GrustError::Unsupported(
                "MATCH SET requires both a pattern and a patch assignment".to_string(),
            ));
        }
        return Ok((pattern, assignment));
    }
    Err(GrustError::Unsupported(
        "only ID-resolved MATCH ... SET += is supported in writable Cypher".to_string(),
    ))
}

pub(crate) fn split_match_remove(statement: &str) -> Result<(&str, &str)> {
    if let Some(index) = find_unquoted_keyword(statement, "REMOVE") {
        let pattern = statement[..index].trim();
        let target = statement[index + "REMOVE".len()..].trim();
        if pattern.is_empty() || target.is_empty() {
            return Err(cypher_syntax(
                "MATCH REMOVE requires both a pattern and a property target",
            ));
        }
        return Ok((pattern, target));
    }
    Err(cypher_syntax(
        "only ID-resolved MATCH ... REMOVE property is supported in writable Cypher",
    ))
}

pub(crate) fn split_final_return(statement: &str) -> Result<(&str, &str)> {
    let Some(index) = find_unquoted_keyword(statement, "RETURN") else {
        return Err(cypher_syntax(
            "writable Cypher returning execution requires a final RETURN clause",
        ));
    };
    let mutation = statement[..index].trim();
    let return_clause = statement[index + "RETURN".len()..].trim();
    if return_clause.is_empty() {
        return Err(cypher_syntax("RETURN requires at least one projection"));
    }
    Ok((mutation, return_clause))
}

pub(crate) fn find_return_control_clause(return_clause: &str) -> Option<usize> {
    let mut earliest: Option<usize> = None;
    for keyword in ["ORDER", "LIMIT", "SKIP", "OFFSET"] {
        let mut offset = 0usize;
        let mut rest = return_clause;
        while let Some(index) = find_unquoted_keyword(rest, keyword) {
            let absolute = offset + index;
            let previous = return_clause[..absolute]
                .chars()
                .rev()
                .find(|ch| !ch.is_whitespace());
            if previous != Some('.') && !is_return_alias_keyword_prefix(&return_clause[..absolute])
            {
                if keyword != "ORDER"
                    || rest[index + keyword.len()..]
                        .trim_start()
                        .get(..2)
                        .is_some_and(|value| value.eq_ignore_ascii_case("BY"))
                {
                    // Keep the earliest control keyword across all three so the
                    // projection/control split point is correct regardless of
                    // the order the keywords appear in the clause.
                    earliest = Some(earliest.map_or(absolute, |current| current.min(absolute)));
                    break;
                }
            }
            let next = index + keyword.len();
            offset += next;
            rest = &rest[next..];
        }
    }
    earliest
}

pub(crate) fn is_return_alias_keyword_prefix(prefix: &str) -> bool {
    let mut words = prefix.split_whitespace().rev();
    words
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case("AS"))
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnClause {
    pub projections: Vec<CypherReturnProjection>,
    pub order_by: Vec<CypherOrderItem>,
    pub skip: Option<usize>,
    pub limit: Option<usize>,
    pub distinct: bool,
}

/// One `ORDER BY` term, resolved to the index of a returned column. Ordering by
/// expressions that are not part of the projection is not supported.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CypherOrderItem {
    pub column: usize,
    pub descending: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnProjection {
    pub variable: String,
    pub target: CypherReturnTarget,
    pub column: String,
    pub expression: String,
    pub element: CypherReturnElement,
    pub aggregate: Option<CypherReturnAggregate>,
    pub distinct: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CypherReturnTarget {
    All,
    Element,
    Literal(Value),
    Property(String),
    MapProjection(CypherReturnMapProjection),
    ListProjection(CypherReturnListProjection),
    Case(CypherReturnCase),
    Coalesce(CypherReturnCoalesce),
    PropertyExists(String),
    PropertySize(String),
    PropertyListIndex(CypherReturnListIndexProjection),
    PropertyListSlice(CypherReturnListSlice),
    PropertyListContains(CypherReturnListContains),
    PropertyListPredicate(CypherReturnListPredicateProjection),
    PropertyListElement(CypherReturnListElementProjection),
    PropertyListTail(CypherReturnListTailProjection),
    PropertyAbs(CypherReturnAbsProjection),
    PropertyNumericRound(CypherReturnNumericRoundProjection),
    PropertyNumericSign(CypherReturnNumericSignProjection),
    PropertyNumericCast(CypherReturnNumericCastProjection),
    PropertyListCast(CypherReturnListCastProjection),
    PropertyToBoolean(CypherReturnToBooleanProjection),
    PropertyToString(CypherReturnToStringProjection),
    PropertyStringTransform(CypherReturnStringTransformProjection),
    PropertyStringTrim(CypherReturnStringTrimProjection),
    PropertyIsEmpty(CypherReturnIsEmptyProjection),
    PropertyStringReverse(CypherReturnStringReverseProjection),
    PropertyStringSplit(CypherReturnStringSplit),
    PropertySubstring(CypherReturnSubstring),
    PropertyStringSlice(CypherReturnStringSlice),
    PropertyReplace(CypherReturnReplace),
    PropertyStringPredicate(CypherReturnStringPredicateProjection),
    NodeLabels,
    RelationshipType,
    ElementProperties,
    ElementKeys,
    ElementId,
    RelationshipStartNode,
    RelationshipEndNode,
    PathLength,
    PathNodes,
    PathRelationships,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnCoalesce {
    pub(crate) variable: Option<String>,
    pub(crate) terms: Vec<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListProjection {
    pub(crate) variable: Option<String>,
    pub(crate) terms: Vec<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnMapProjection {
    pub(crate) variable: String,
    pub(crate) entries: Vec<CypherReturnMapProjectionEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CypherReturnMapProjectionEntry {
    pub(crate) output_key: String,
    pub(crate) value: CypherReturnTarget,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherReturnStringTransform {
    Lower,
    Upper,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnStringTransformProjection {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
    pub(crate) transform: CypherReturnStringTransform,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherReturnStringTrim {
    Both,
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnStringTrimProjection {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
    pub(crate) trim: CypherReturnStringTrim,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnToStringProjection {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnAbsProjection {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnNumericRoundProjection {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
    pub(crate) round: CypherReturnNumericRound,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnNumericSignProjection {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnNumericCastProjection {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
    pub(crate) cast: CypherReturnNumericCast,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnToBooleanProjection {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListCastProjection {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
    pub(crate) cast: CypherReturnListCast,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnStringReverseProjection {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnIsEmptyProjection {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListElementProjection {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
    pub(crate) element: CypherReturnListElement,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListTailProjection {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherReturnNumericRound {
    Ceil,
    Floor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherReturnNumericCast {
    Integer,
    Float,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherReturnListCast {
    String,
    Integer,
    Float,
    Boolean,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherReturnListElement {
    Head,
    Last,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnCase {
    pub(crate) key: String,
    pub(crate) equals: Value,
    pub(crate) then_target: Box<CypherReturnTarget>,
    pub(crate) else_target: Box<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnSubstring {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
    pub(crate) start: usize,
    pub(crate) length: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnStringSplit {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
    pub(crate) delimiter: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnStringSlice {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
    pub(crate) side: CypherReturnStringSliceSide,
    pub(crate) length: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListSlice {
    pub(crate) key: String,
    pub(crate) start: Option<CypherReturnListBound>,
    pub(crate) end: Option<CypherReturnListBound>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListIndexProjection {
    pub(crate) key: String,
    pub(crate) index: CypherReturnListBound,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListBound {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListContains {
    pub(crate) key: String,
    pub(crate) needle: Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CypherReturnListPredicate {
    Any,
    All,
    None,
    Single,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnListPredicateProjection {
    pub(crate) key: String,
    pub(crate) predicate: CypherReturnListPredicate,
    pub(crate) item_variable: String,
    pub(crate) equals_variable: Option<String>,
    pub(crate) equals: Box<CypherReturnTarget>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CypherReturnStringSliceSide {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnReplace {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
    pub(crate) search: String,
    pub(crate) replacement: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherReturnStringPredicateProjection {
    pub(crate) variable: Option<String>,
    pub(crate) target: Box<CypherReturnTarget>,
    pub(crate) predicate: CypherReturnStringPredicate,
    pub(crate) needle: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CypherReturnStringPredicate {
    StartsWith,
    EndsWith,
    Contains,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherReturnElement {
    Node,
    Edge,
    RowNode,
    RowEdge,
    RowPath,
    Literal,
    Aggregate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CypherReturnAggregate {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Collect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CypherReturnTargetMaterialization {
    Star,
    Element,
    DirectProperty,
    ScalarProjection,
    ElementFunction,
    PathFunction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CypherReturnScalarProjectionKind {
    Star,
    Element,
    DirectProperty,
    Literal,
    Map,
    List,
    Conditional,
    Coalesce,
    Introspection,
    ListAccess,
    ListPredicate,
    Numeric,
    Conversion,
    String,
    ElementFunction,
    PathFunction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CypherReturnScalarAstFamily {
    Binding,
    Wrapper,
    Value,
    Control,
    Introspection,
    List,
    Numeric,
    Conversion,
    String,
}

pub(crate) enum CypherReturnScalarAst<'a> {
    Star,
    Element,
    DirectProperty(&'a str),
    Literal(&'a Value),
    Map(&'a CypherReturnMapProjection),
    List(&'a CypherReturnListProjection),
    Conditional(&'a CypherReturnCase),
    Coalesce(&'a CypherReturnCoalesce),
    PropertyExists(&'a str),
    PropertySize(&'a str),
    PropertyListIndex(&'a CypherReturnListIndexProjection),
    PropertyListSlice(&'a CypherReturnListSlice),
    PropertyListContains(&'a CypherReturnListContains),
    PropertyListPredicate(&'a CypherReturnListPredicateProjection),
    PropertyListElement(&'a CypherReturnListElementProjection),
    PropertyListTail(&'a CypherReturnListTailProjection),
    PropertyAbs(&'a CypherReturnAbsProjection),
    PropertyNumericRound(&'a CypherReturnNumericRoundProjection),
    PropertyNumericSign(&'a CypherReturnNumericSignProjection),
    PropertyNumericCast(&'a CypherReturnNumericCastProjection),
    PropertyListCast(&'a CypherReturnListCastProjection),
    PropertyToBoolean(&'a CypherReturnToBooleanProjection),
    PropertyToString(&'a CypherReturnToStringProjection),
    PropertyStringTransform(&'a CypherReturnStringTransformProjection),
    PropertyStringTrim(&'a CypherReturnStringTrimProjection),
    PropertyIsEmpty(&'a CypherReturnIsEmptyProjection),
    PropertyStringReverse(&'a CypherReturnStringReverseProjection),
    PropertyStringSplit(&'a CypherReturnStringSplit),
    PropertySubstring(&'a CypherReturnSubstring),
    PropertyStringSlice(&'a CypherReturnStringSlice),
    PropertyReplace(&'a CypherReturnReplace),
    PropertyStringPredicate(&'a CypherReturnStringPredicateProjection),
    ElementFunction,
    PathFunction,
}

pub(crate) fn parse_cypher_return_clause(
    clause: &str,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_bindings: &HashMap<String, GraphNodeMatch>,
    row_edge_match_bindings: &HashMap<String, GraphRelationshipMatch>,
    row_edge_bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
    parameters: &CypherParameters,
) -> Result<CypherReturnClause> {
    let (projection_clause, control_clause) = split_return_control(clause);
    if projection_clause.eq_ignore_ascii_case("DISTINCT") {
        return Err(cypher_syntax("RETURN DISTINCT requires a projection"));
    }
    let (projection_clause, distinct) =
        if let Some(after_distinct) = strip_leading_keyword(projection_clause, "DISTINCT") {
            let projection_clause = after_distinct.trim();
            if projection_clause.is_empty() {
                return Err(cypher_syntax("RETURN DISTINCT requires a projection"));
            }
            (projection_clause, true)
        } else {
            (projection_clause, false)
        };
    let mut projections = Vec::new();
    for projection in split_top_level_commas(projection_clause)? {
        let projection = projection.trim();
        if projection.is_empty() {
            return Err(cypher_syntax("RETURN contains an empty projection"));
        }
        let (expression, alias) = split_return_alias(projection)?;
        if let Some((aggregate, variable, target, distinct)) =
            parse_aggregate_projection(expression, parameters)?
        {
            if let Some(variable) = variable.as_ref() {
                validate_return_variable_binding(
                    variable,
                    node_bindings,
                    edge_bindings,
                    row_node_bindings,
                    row_edge_match_bindings,
                    row_edge_bindings,
                    row_path_bindings,
                )?;
                if matches!(
                    target,
                    CypherReturnTarget::PathLength
                        | CypherReturnTarget::PathNodes
                        | CypherReturnTarget::PathRelationships
                ) && !row_path_bindings.contains_key(variable)
                {
                    return Err(cypher_unsupported_cardinality(
                        "writable Cypher RETURN path functions require a bound path variable",
                    ));
                }
                let element = cypher_return_element_for_variable(
                    variable,
                    node_bindings,
                    edge_bindings,
                    row_node_bindings,
                    row_edge_match_bindings,
                    row_edge_bindings,
                    row_path_bindings,
                )?;
                validate_return_function_target(&target, element)?;
            }
            projections.push(CypherReturnProjection {
                variable: variable.unwrap_or_default(),
                target,
                column: alias.unwrap_or_else(|| expression.trim().to_string()),
                expression: expression.trim().to_string(),
                element: CypherReturnElement::Aggregate,
                aggregate: Some(aggregate),
                distinct,
            });
            continue;
        }
        if projection == "*" {
            append_star_return_projections(
                &mut projections,
                node_bindings,
                edge_bindings,
                row_node_bindings,
                row_edge_match_bindings,
                row_edge_bindings,
                row_path_bindings,
            )?;
            continue;
        }
        let (variable, target) = if let Some((variable, target)) =
            parse_restricted_return_target_expression(expression, parameters)?
        {
            (variable.unwrap_or_default(), target)
        } else if expression.contains('(') || expression.contains(')') {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN only supports bound element, property, and restricted path projections",
            ));
        } else if let Ok((variable, key)) = parse_property_ref(expression, "RETURN projection") {
            (variable, CypherReturnTarget::Property(key))
        } else {
            (
                parse_required_cypher_variable(expression, "RETURN projection")?,
                CypherReturnTarget::Element,
            )
        };
        let element =
            if matches!(target, CypherReturnTarget::Literal(_))
                || matches!(
                    target,
                    CypherReturnTarget::Coalesce(CypherReturnCoalesce { variable: None, .. })
                        | CypherReturnTarget::ListProjection(CypherReturnListProjection {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyStringTransform(
                            CypherReturnStringTransformProjection { variable: None, .. },
                        )
                        | CypherReturnTarget::PropertyStringTrim(
                            CypherReturnStringTrimProjection { variable: None, .. }
                        )
                        | CypherReturnTarget::PropertyStringReverse(
                            CypherReturnStringReverseProjection { variable: None, .. },
                        )
                        | CypherReturnTarget::PropertyIsEmpty(CypherReturnIsEmptyProjection {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyStringSplit(CypherReturnStringSplit {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertySubstring(CypherReturnSubstring {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyStringSlice(CypherReturnStringSlice {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyStringPredicate(
                            CypherReturnStringPredicateProjection { variable: None, .. },
                        )
                        | CypherReturnTarget::PropertyReplace(CypherReturnReplace {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyToString(CypherReturnToStringProjection {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyAbs(CypherReturnAbsProjection {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyNumericRound(
                            CypherReturnNumericRoundProjection { variable: None, .. },
                        )
                        | CypherReturnTarget::PropertyNumericSign(
                            CypherReturnNumericSignProjection { variable: None, .. },
                        )
                        | CypherReturnTarget::PropertyNumericCast(
                            CypherReturnNumericCastProjection { variable: None, .. },
                        )
                        | CypherReturnTarget::PropertyToBoolean(CypherReturnToBooleanProjection {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyListElement(
                            CypherReturnListElementProjection { variable: None, .. },
                        )
                        | CypherReturnTarget::PropertyListTail(CypherReturnListTailProjection {
                            variable: None,
                            ..
                        })
                        | CypherReturnTarget::PropertyListCast(CypherReturnListCastProjection {
                            variable: None,
                            ..
                        })
                )
            {
                CypherReturnElement::Literal
            } else {
                cypher_return_element_for_variable(
                    &variable,
                    node_bindings,
                    edge_bindings,
                    row_node_bindings,
                    row_edge_match_bindings,
                    row_edge_bindings,
                    row_path_bindings,
                )?
            };
        if matches!(
            target,
            CypherReturnTarget::PathLength
                | CypherReturnTarget::PathNodes
                | CypherReturnTarget::PathRelationships
        ) && element != CypherReturnElement::RowPath
        {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN path functions require a bound path variable",
            ));
        }
        validate_return_function_target(&target, element)?;
        projections.push(CypherReturnProjection {
            variable,
            target,
            column: alias.unwrap_or_else(|| expression.trim().to_string()),
            expression: expression.trim().to_string(),
            element,
            aggregate: None,
            distinct: false,
        });
    }
    if projections.is_empty() {
        return Err(cypher_syntax("RETURN requires at least one projection"));
    }
    let order_keys = projections
        .iter()
        .map(|projection| vec![projection.column.clone(), projection.expression.clone()])
        .collect::<Vec<_>>();
    let (order_by, skip, limit) = parse_return_control(control_clause, &order_keys)?;
    Ok(CypherReturnClause {
        projections,
        order_by,
        skip,
        limit,
        distinct,
    })
}

/// Splits a RETURN clause into its projection list and the optional trailing
/// `ORDER BY` / `SKIP` / `LIMIT` control clause.
pub(crate) fn split_return_control(clause: &str) -> (&str, &str) {
    match find_return_control_clause(clause) {
        Some(index) => (clause[..index].trim(), clause[index..].trim()),
        None => (clause.trim(), ""),
    }
}

/// Parses a `ORDER BY ... [SKIP/OFFSET n] [LIMIT n]` control clause. Cypher's
/// canonical `ORDER BY`, then row offset, then `LIMIT` ordering is required,
/// and `ORDER BY` terms must reference returned column names or aliases.
pub(crate) fn parse_return_control(
    control: &str,
    order_keys: &[Vec<String>],
) -> Result<(Vec<CypherOrderItem>, Option<usize>, Option<usize>)> {
    let mut rest = control.trim();
    let mut order_by = Vec::new();
    if let Some(after_order) = strip_leading_keyword(rest, "ORDER") {
        let after_by = strip_leading_keyword(after_order.trim_start(), "BY")
            .ok_or_else(|| cypher_syntax("ORDER must be followed by BY"))?;
        let (items, tail) = split_before_keywords(after_by, &["SKIP", "OFFSET", "LIMIT"]);
        order_by = parse_order_items(items, order_keys)?;
        rest = tail.trim_start();
    }
    let mut skip = None;
    if let Some(after_skip) =
        strip_leading_keyword(rest, "SKIP").or_else(|| strip_leading_keyword(rest, "OFFSET"))
    {
        let (count, tail) = split_before_keywords(after_skip, &["LIMIT"]);
        skip = Some(parse_return_count(count, "SKIP/OFFSET")?);
        rest = tail.trim_start();
    }
    let mut limit = None;
    if let Some(after_limit) = strip_leading_keyword(rest, "LIMIT") {
        limit = parse_return_limit(after_limit)?;
        rest = "";
    }
    if !rest.trim().is_empty() {
        return Err(cypher_syntax(format!(
            "unsupported RETURN clause tail; expected ORDER BY, SKIP/OFFSET, then LIMIT: {}",
            rest.trim()
        )));
    }
    Ok((order_by, skip, limit))
}

/// Returns the slice of `value` before the first top-level occurrence of any of
/// `keywords`, plus the remainder starting at that keyword.
pub(crate) fn split_before_keywords<'a>(value: &'a str, keywords: &[&str]) -> (&'a str, &'a str) {
    let split = keywords
        .iter()
        .filter_map(|keyword| find_unquoted_keyword(value, keyword))
        .min();
    match split {
        Some(index) => (value[..index].trim(), &value[index..]),
        None => (value.trim(), ""),
    }
}

pub(crate) fn parse_order_items(
    items: &str,
    order_keys: &[Vec<String>],
) -> Result<Vec<CypherOrderItem>> {
    let mut order_by = Vec::new();
    for item in split_top_level_commas(items)? {
        let item = item.trim();
        if item.is_empty() {
            return Err(cypher_syntax("ORDER BY contains an empty term"));
        }
        let (expression, descending) = if let Some(prefix) = strip_trailing_keyword(item, "DESC") {
            (prefix, true)
        } else if let Some(prefix) = strip_trailing_keyword(item, "DESCENDING") {
            (prefix, true)
        } else if let Some(prefix) = strip_trailing_keyword(item, "ASC") {
            (prefix, false)
        } else if let Some(prefix) = strip_trailing_keyword(item, "ASCENDING") {
            (prefix, false)
        } else {
            (item, false)
        };
        let expression = expression.trim();
        let column = order_keys
            .iter()
            .position(|keys| keys.iter().any(|key| key == expression))
            .ok_or_else(|| {
                cypher_unsupported_cardinality(format!(
                    "ORDER BY '{expression}' must reference a returned column, alias, or projection expression"
                ))
            })?;
        order_by.push(CypherOrderItem { column, descending });
    }
    if order_by.is_empty() {
        return Err(cypher_syntax("ORDER BY requires at least one term"));
    }
    Ok(order_by)
}

pub(crate) fn parse_aggregate_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<
    Option<(
        CypherReturnAggregate,
        Option<String>,
        CypherReturnTarget,
        bool,
    )>,
> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let aggregate = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "count" => CypherReturnAggregate::Count,
        "sum" => CypherReturnAggregate::Sum,
        "avg" => CypherReturnAggregate::Avg,
        "min" => CypherReturnAggregate::Min,
        "max" => CypherReturnAggregate::Max,
        "collect" => CypherReturnAggregate::Collect,
        _ => {
            return Ok(None);
        }
    };
    let aggregate_name = match aggregate {
        CypherReturnAggregate::Count => "COUNT",
        CypherReturnAggregate::Sum => "SUM",
        CypherReturnAggregate::Avg => "AVG",
        CypherReturnAggregate::Min => "MIN",
        CypherReturnAggregate::Max => "MAX",
        CypherReturnAggregate::Collect => "COLLECT",
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(format!(
            "{aggregate_name} projection is missing ')'"
        )));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    let (body, distinct) = if let Some(after_distinct) = strip_leading_keyword(body, "DISTINCT") {
        let body = after_distinct.trim();
        if body.is_empty() {
            return Err(cypher_syntax(format!(
                "{aggregate_name} DISTINCT requires a target"
            )));
        }
        (body, true)
    } else {
        (body, false)
    };
    if let Some((variable, target)) = parse_return_path_function_projection(body)? {
        return Ok(Some((aggregate, Some(variable), target, distinct)));
    }
    if let Some((variable, target)) = parse_return_element_function_projection(body)? {
        return Ok(Some((aggregate, Some(variable), target, distinct)));
    }
    if !matches!(
        aggregate,
        CypherReturnAggregate::Count | CypherReturnAggregate::Collect
    ) && body == "*"
    {
        return Err(cypher_unsupported_cardinality(format!(
            "writable Cypher RETURN does not support {aggregate_name}(*)"
        )));
    }
    if body == "*" {
        if aggregate == CypherReturnAggregate::Count && distinct {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN does not support COUNT(DISTINCT *)",
            ));
        }
        return Ok(Some((aggregate, None, CypherReturnTarget::All, distinct)));
    }
    if let Some((variable, target)) = parse_restricted_return_target_expression(body, parameters)? {
        return Ok(Some((aggregate, variable, target, distinct)));
    }
    if !matches!(
        aggregate,
        CypherReturnAggregate::Count | CypherReturnAggregate::Collect
    ) && !body.contains('.')
    {
        return Err(cypher_unsupported_cardinality(format!(
            "writable Cypher RETURN only supports {aggregate_name}(variable.property) or restricted CASE"
        )));
    }
    if let Ok((variable, key)) = parse_property_ref(body, "RETURN aggregate projection") {
        return Ok(Some((
            aggregate,
            Some(variable),
            CypherReturnTarget::Property(key),
            distinct,
        )));
    }
    if !matches!(
        aggregate,
        CypherReturnAggregate::Count | CypherReturnAggregate::Collect
    ) {
        return Err(cypher_unsupported_cardinality(format!(
            "writable Cypher RETURN only supports {aggregate_name}(variable.property) or restricted CASE"
        )));
    }
    Ok(Some((
        aggregate,
        Some(parse_required_cypher_variable(
            body,
            "RETURN aggregate projection",
        )?),
        CypherReturnTarget::Element,
        distinct,
    )))
}

pub(crate) fn parse_restricted_return_target_expression(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnTarget)>> {
    if let Some((variable, path_target)) = parse_return_path_function_projection(expression)? {
        return Ok(Some((Some(variable), path_target)));
    }
    if let Some((variable, element_target)) = parse_return_element_function_projection(expression)?
    {
        return Ok(Some((Some(variable), element_target)));
    }
    if let Some((variable, coalesce)) = parse_return_coalesce_projection(expression, parameters)? {
        return Ok(Some((variable, CypherReturnTarget::Coalesce(coalesce))));
    }
    if let Some((variable, key)) = parse_return_exists_projection(expression)? {
        return Ok(Some((
            Some(variable),
            CypherReturnTarget::PropertyExists(key),
        )));
    }
    if let Some((variable, key)) = parse_return_size_projection(expression)? {
        return Ok(Some((
            Some(variable),
            CypherReturnTarget::PropertySize(key),
        )));
    }
    if let Some((variable, slice)) = parse_return_list_slice_projection(expression, parameters)? {
        return Ok(Some((
            Some(variable),
            CypherReturnTarget::PropertyListSlice(slice),
        )));
    }
    if let Some((variable, predicate)) =
        parse_return_list_predicate_projection(expression, parameters)?
    {
        return Ok(Some((
            Some(variable),
            CypherReturnTarget::PropertyListPredicate(predicate),
        )));
    }
    if let Some((variable, contains)) =
        parse_return_list_contains_projection(expression, parameters)?
    {
        return Ok(Some((
            Some(variable),
            CypherReturnTarget::PropertyListContains(contains),
        )));
    }
    if let Some((variable, index)) = parse_return_list_index_projection(expression, parameters)? {
        return Ok(Some((
            Some(variable),
            CypherReturnTarget::PropertyListIndex(index),
        )));
    }
    if let Some((variable, element)) = parse_return_list_element_projection(expression, parameters)?
    {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyListElement(element),
        )));
    }
    if let Some((variable, tail)) = parse_return_list_tail_projection(expression, parameters)? {
        return Ok(Some((variable, CypherReturnTarget::PropertyListTail(tail))));
    }
    if let Some((variable, abs)) = parse_return_abs_projection(expression, parameters)? {
        return Ok(Some((variable, CypherReturnTarget::PropertyAbs(abs))));
    }
    if let Some((variable, round)) = parse_return_numeric_round_projection(expression, parameters)?
    {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyNumericRound(round),
        )));
    }
    if let Some((variable, sign)) = parse_return_numeric_sign_projection(expression, parameters)? {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyNumericSign(sign),
        )));
    }
    if let Some((variable, cast)) = parse_return_numeric_cast_projection(expression, parameters)? {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyNumericCast(cast),
        )));
    }
    if let Some((variable, cast)) = parse_return_list_cast_projection(expression, parameters)? {
        return Ok(Some((variable, CypherReturnTarget::PropertyListCast(cast))));
    }
    if let Some((variable, to_boolean)) =
        parse_return_to_boolean_projection(expression, parameters)?
    {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyToBoolean(to_boolean),
        )));
    }
    if let Some((variable, to_string)) = parse_return_to_string_projection(expression, parameters)?
    {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyToString(to_string),
        )));
    }
    if let Some((variable, transform)) =
        parse_return_string_transform_projection(expression, parameters)?
    {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyStringTransform(transform),
        )));
    }
    if let Some((variable, trim)) = parse_return_string_trim_projection(expression, parameters)? {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyStringTrim(trim),
        )));
    }
    if let Some((variable, is_empty)) = parse_return_is_empty_projection(expression, parameters)? {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyIsEmpty(is_empty),
        )));
    }
    if let Some((variable, reverse)) =
        parse_return_string_reverse_projection(expression, parameters)?
    {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyStringReverse(reverse),
        )));
    }
    if let Some((variable, split)) = parse_return_string_split_projection(expression, parameters)? {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyStringSplit(split),
        )));
    }
    if let Some((variable, substring)) = parse_return_substring_projection(expression, parameters)?
    {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertySubstring(substring),
        )));
    }
    if let Some((variable, slice)) = parse_return_string_slice_projection(expression, parameters)? {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyStringSlice(slice),
        )));
    }
    if let Some((variable, replace)) = parse_return_replace_projection(expression, parameters)? {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyReplace(replace),
        )));
    }
    if let Some((variable, predicate)) =
        parse_return_string_predicate_projection(expression, parameters)?
    {
        return Ok(Some((
            variable,
            CypherReturnTarget::PropertyStringPredicate(predicate),
        )));
    }
    if let Some(range) = parse_return_range_projection(expression, parameters)? {
        return Ok(Some((None, CypherReturnTarget::Literal(range))));
    }
    if let Some((variable, case)) = parse_return_case_projection(expression, parameters)? {
        return Ok(Some((Some(variable), CypherReturnTarget::Case(case))));
    }
    if let Some(literal) = parse_return_literal_projection(expression, parameters)? {
        return Ok(Some((None, CypherReturnTarget::Literal(literal))));
    }
    if let Some(list) = parse_return_list_projection(expression, parameters)? {
        return Ok(Some((
            list.variable.clone(),
            CypherReturnTarget::ListProjection(list),
        )));
    }
    if let Some(map) = parse_return_map_projection(expression, parameters)? {
        return Ok(Some((
            Some(map.variable.clone()),
            CypherReturnTarget::MapProjection(map),
        )));
    }
    Ok(None)
}

pub(crate) fn parse_return_literal_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<Value>> {
    let expression = expression.trim();
    if !is_return_literal_candidate(expression) {
        return Ok(None);
    }
    parse_cypher_literal(expression, parameters).map(Some)
}

pub(crate) fn parse_return_range_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<Value>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("range") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN range projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax("RETURN range projection requires arguments"));
    }
    let arguments = split_top_level_commas(body)?;
    if !(2..=3).contains(&arguments.len()) {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN range requires start, end, and optional step",
        ));
    }
    let start = parse_integer_literal_argument(arguments[0], parameters, "RETURN range start")?;
    let end = parse_integer_literal_argument(arguments[1], parameters, "RETURN range end")?;
    let step = if let Some(step) = arguments.get(2) {
        parse_integer_literal_argument(step, parameters, "RETURN range step")?
    } else {
        1
    };
    restricted_range_value(start, end, step).map(Some)
}

pub(crate) fn parse_integer_literal_argument(
    expression: &str,
    parameters: &CypherParameters,
    context: &str,
) -> Result<i64> {
    let value = parse_cypher_literal(expression.trim(), parameters)?;
    let Value::Int(value) = value else {
        return Err(cypher_unsupported_cardinality(format!(
            "{context} must be an integer literal or parameter"
        )));
    };
    Ok(value)
}

pub(crate) fn restricted_range_value(start: i64, end: i64, step: i64) -> Result<Value> {
    const MAX_RANGE_VALUES: usize = 1_000_000;

    if step == 0 {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN range step must be non-zero",
        ));
    }

    let mut values = Vec::new();
    let mut current = start;
    while (step > 0 && current <= end) || (step < 0 && current >= end) {
        if values.len() == MAX_RANGE_VALUES {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN range would produce too many values",
            ));
        }
        values.push(current);
        let Some(next) = current.checked_add(step) else {
            break;
        };
        current = next;
    }
    Ok(Value::IntArray(values))
}

pub(crate) fn parse_return_list_slice_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(String, CypherReturnListSlice)>> {
    let expression = expression.trim();
    let Some(open) = find_unquoted(expression, '[') else {
        return Ok(None);
    };
    if open == 0 {
        return Ok(None);
    }
    if !expression.ends_with(']') {
        return Err(cypher_syntax("RETURN list slice projection is missing ']'"));
    }
    let target = expression[..open].trim();
    let bounds = expression[open + 1..expression.len() - 1].trim();
    let Some(dotdot) = find_unquoted_sequence(bounds, "..") else {
        return Ok(None);
    };
    if find_unquoted_sequence(&bounds[dotdot + 2..], "..").is_some() {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list slices only support one '..' range",
        ));
    }
    if target.is_empty() {
        return Err(cypher_syntax(
            "RETURN list slice projection requires variable.property[start..end]",
        ));
    }
    if bounds.contains('[') || bounds.contains(']') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list slices only support integer bounds",
        ));
    }
    let start = parse_optional_list_bound(
        bounds[..dotdot].trim(),
        parameters,
        "RETURN list slice start",
    )?;
    let end = parse_optional_list_bound(
        bounds[dotdot + 2..].trim(),
        parameters,
        "RETURN list slice end",
    )?;
    let (variable, key) =
        parse_property_ref(target, "RETURN list slice projection").map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN list slices require a variable.property target",
            )
        })?;
    let mut expected_variable = Some(variable.clone());
    if let Some(start) = &start {
        merge_single_return_variable(
            &mut expected_variable,
            start.variable.clone(),
            "writable Cypher RETURN list slice bounds must reference the list target variable",
        )?;
    }
    if let Some(end) = &end {
        merge_single_return_variable(
            &mut expected_variable,
            end.variable.clone(),
            "writable Cypher RETURN list slice bounds must reference the list target variable",
        )?;
    }
    Ok(Some((variable, CypherReturnListSlice { key, start, end })))
}

pub(crate) fn parse_optional_list_bound(
    expression: &str,
    parameters: &CypherParameters,
    context: &str,
) -> Result<Option<CypherReturnListBound>> {
    if expression.is_empty() {
        return Ok(None);
    }
    parse_return_list_bound(expression, parameters, context).map(Some)
}

pub(crate) fn parse_return_list_bound(
    expression: &str,
    parameters: &CypherParameters,
    context: &str,
) -> Result<CypherReturnListBound> {
    let (variable, target) = parse_nested_restricted_scalar_target(
        expression,
        parameters,
        context,
        "writable Cypher RETURN list indexes and slices only support restricted scalar integer bounds",
    )?;
    Ok(CypherReturnListBound {
        variable,
        target: Box::new(target),
    })
}

pub(crate) fn parse_return_list_contains_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(String, CypherReturnListContains)>> {
    let expression = expression.trim();
    let Some(in_index) = find_unquoted_keyword(expression, "IN") else {
        return Ok(None);
    };
    if expression.contains('(') || expression.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN IN only supports literal IN variable.property",
        ));
    }
    let needle = expression[..in_index].trim();
    let haystack = expression[in_index + "IN".len()..].trim();
    if needle.is_empty() || haystack.is_empty() {
        return Err(cypher_syntax(
            "RETURN list membership projection requires needle IN variable.property",
        ));
    }
    let needle = parse_cypher_literal(needle, parameters).map_err(|_| {
        cypher_unsupported_cardinality(
            "writable Cypher RETURN IN needle must be a literal or parameter",
        )
    })?;
    let (variable, key) = parse_property_ref(haystack, "RETURN list membership projection")
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN IN requires a variable.property haystack",
            )
        })?;
    Ok(Some((variable, CypherReturnListContains { key, needle })))
}

pub(crate) fn parse_return_list_predicate_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(String, CypherReturnListPredicateProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let predicate = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "any" => CypherReturnListPredicate::Any,
        "all" => CypherReturnListPredicate::All,
        "none" => CypherReturnListPredicate::None,
        "single" => CypherReturnListPredicate::Single,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN list predicate projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN list predicate projection requires item IN variable.property WHERE item = value",
        ));
    }
    let Some(in_index) = find_unquoted_keyword(body, "IN") else {
        return Err(cypher_syntax(
            "RETURN list predicate projection requires item IN variable.property WHERE item = value",
        ));
    };
    let item_variable = parse_required_cypher_variable(
        body[..in_index].trim(),
        "RETURN list predicate item variable",
    )?;
    let rest = body[in_index + "IN".len()..].trim();
    let Some(where_index) = find_unquoted_keyword(rest, "WHERE") else {
        return Err(cypher_syntax(
            "RETURN list predicate projection requires WHERE item = value",
        ));
    };
    let haystack = rest[..where_index].trim();
    let condition = rest[where_index + "WHERE".len()..].trim();
    let Some(equals_index) = find_unquoted(condition, '=') else {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list predicates only support equality predicates",
        ));
    };
    if find_unquoted(&condition[equals_index + 1..], '=').is_some() {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list predicates only support one equality predicate",
        ));
    }
    let left = condition[..equals_index].trim();
    let right = condition[equals_index + 1..].trim();
    if left != item_variable {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list predicates require the WHERE left side to be the list item variable",
        ));
    }
    if right.is_empty() {
        return Err(cypher_syntax(
            "RETURN list predicate projection requires an equality value",
        ));
    }
    let (variable, key) = parse_property_ref(haystack, "RETURN list predicate projection")
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN list predicates require a variable.property haystack",
            )
        })?;
    let (equals_variable, equals) = parse_nested_restricted_scalar_target(
        right,
        parameters,
        "RETURN list predicate equality value",
        "writable Cypher RETURN list predicate equality value must be a restricted scalar value",
    )?;
    if let Some(equals_variable) = equals_variable.as_ref()
        && equals_variable != &variable
    {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list predicate equality value must reference the haystack variable",
        ));
    }
    Ok(Some((
        variable,
        CypherReturnListPredicateProjection {
            key,
            predicate,
            item_variable,
            equals_variable,
            equals: Box::new(equals),
        },
    )))
}

pub(crate) fn parse_return_list_index_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(String, CypherReturnListIndexProjection)>> {
    let expression = expression.trim();
    let Some(open) = find_unquoted(expression, '[') else {
        return Ok(None);
    };
    if open == 0 {
        return Ok(None);
    }
    if !expression.ends_with(']') {
        return Err(cypher_syntax("RETURN list index projection is missing ']'"));
    }
    let target = expression[..open].trim();
    let index = expression[open + 1..expression.len() - 1].trim();
    if target.is_empty() || index.is_empty() {
        return Err(cypher_syntax(
            "RETURN list index projection requires variable.property[index]",
        ));
    }
    if index.contains('[') || index.contains(']') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN list indexes only support a single integer index",
        ));
    }
    let (variable, key) =
        parse_property_ref(target, "RETURN list index projection").map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN list indexes require a variable.property target",
            )
        })?;
    let index = parse_return_list_bound(index, parameters, "RETURN list index")?;
    let mut expected_variable = Some(variable.clone());
    merge_single_return_variable(
        &mut expected_variable,
        index.variable.clone(),
        "writable Cypher RETURN list index expressions must reference the list target variable",
    )?;
    Ok(Some((
        variable,
        CypherReturnListIndexProjection { key, index },
    )))
}

pub(crate) fn is_return_literal_candidate(expression: &str) -> bool {
    if expression.starts_with('\'')
        || expression.starts_with('"')
        || expression.starts_with('$')
        || expression.eq_ignore_ascii_case("true")
        || expression.eq_ignore_ascii_case("false")
        || expression.eq_ignore_ascii_case("null")
    {
        return true;
    }
    expression
        .chars()
        .next()
        .is_some_and(|ch| ch == '-' || ch == '+' || ch.is_ascii_digit())
}

pub(crate) fn parse_return_coalesce_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnCoalesce)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("coalesce") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN coalesce projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN coalesce projection requires at least one argument",
        ));
    }

    let mut variable = None;
    let mut terms = Vec::new();
    for argument in split_top_level_commas(body)? {
        let argument = argument.trim();
        if argument.is_empty() {
            return Err(cypher_syntax(
                "RETURN coalesce projection contains an empty argument",
            ));
        }
        let (argument_variable, target) = parse_return_coalesce_argument(argument, parameters)?;
        merge_return_coalesce_variable(&mut variable, argument_variable)?;
        terms.push(target);
    }

    Ok(Some((
        variable.clone(),
        CypherReturnCoalesce { variable, terms },
    )))
}

pub(crate) fn parse_return_coalesce_argument(
    argument: &str,
    parameters: &CypherParameters,
) -> Result<(Option<String>, CypherReturnTarget)> {
    parse_nested_restricted_scalar_target(
        argument,
        parameters,
        "RETURN coalesce projection",
        "writable Cypher RETURN coalesce only supports restricted scalar arguments",
    )
}

pub(crate) fn parse_nested_restricted_scalar_target(
    expression: &str,
    parameters: &CypherParameters,
    property_context: &str,
    unsupported_message: &'static str,
) -> Result<(Option<String>, CypherReturnTarget)> {
    if let Some((variable, target)) =
        parse_restricted_return_target_expression(expression, parameters)?
    {
        if matches!(
            target,
            CypherReturnTarget::ListProjection(_) | CypherReturnTarget::MapProjection(_)
        ) {
            return Err(cypher_unsupported_cardinality(unsupported_message));
        }
        return Ok((variable, target));
    }
    let (variable, key) = parse_property_ref(expression, property_context)
        .map_err(|_| cypher_unsupported_cardinality(unsupported_message))?;
    Ok((Some(variable), CypherReturnTarget::Property(key)))
}

pub(crate) fn merge_return_coalesce_variable(
    variable: &mut Option<String>,
    argument_variable: Option<String>,
) -> Result<()> {
    merge_single_return_variable(
        variable,
        argument_variable,
        "writable Cypher RETURN coalesce arguments must reference one variable",
    )
}

pub(crate) fn merge_single_return_variable(
    variable: &mut Option<String>,
    argument_variable: Option<String>,
    message: &'static str,
) -> Result<()> {
    let Some(argument_variable) = argument_variable else {
        return Ok(());
    };
    if let Some(variable) = variable {
        if variable != &argument_variable {
            return Err(cypher_unsupported_cardinality(message));
        }
    } else {
        *variable = Some(argument_variable);
    }
    Ok(())
}

pub(crate) fn parse_return_exists_projection(expression: &str) -> Result<Option<(String, String)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("exists") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN exists projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN exists projection requires a property reference",
        ));
    }
    if body.contains('(') || body.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN exists only supports variable.property arguments",
        ));
    }
    parse_property_ref(body, "RETURN exists projection")
        .map(Some)
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN exists only supports variable.property arguments",
            )
        })
}

pub(crate) fn parse_return_size_projection(expression: &str) -> Result<Option<(String, String)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("size") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN size projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN size projection requires a property reference",
        ));
    }
    if body.contains('(') || body.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN size only supports variable.property arguments",
        ));
    }
    parse_property_ref(body, "RETURN size projection")
        .map(Some)
        .map_err(|_| {
            cypher_unsupported_cardinality(
                "writable Cypher RETURN size only supports variable.property arguments",
            )
        })
}

pub(crate) fn parse_return_list_element_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnListElementProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let element = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "head" => CypherReturnListElement::Head,
        "last" => CypherReturnListElement::Last,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN list element projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN list element projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN list element projection",
        "writable Cypher RETURN head/last only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnListElementProjection {
            variable,
            target: Box::new(target),
            element,
        },
    )))
}

pub(crate) fn parse_return_list_tail_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnListTailProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("tail") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN tail projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax("RETURN tail projection requires an argument"));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN tail projection",
        "writable Cypher RETURN tail only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnListTailProjection {
            variable,
            target: Box::new(target),
        },
    )))
}

pub(crate) fn parse_return_abs_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnAbsProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("abs") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN abs projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax("RETURN abs projection requires an argument"));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN abs projection",
        "writable Cypher RETURN abs only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnAbsProjection {
            variable,
            target: Box::new(target),
        },
    )))
}

pub(crate) fn parse_return_numeric_round_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnNumericRoundProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let round = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "ceil" => CypherReturnNumericRound::Ceil,
        "floor" => CypherReturnNumericRound::Floor,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN numeric rounding projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN numeric rounding projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN numeric rounding projection",
        "writable Cypher RETURN ceil/floor only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnNumericRoundProjection {
            variable,
            target: Box::new(target),
            round,
        },
    )))
}

pub(crate) fn parse_return_numeric_sign_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnNumericSignProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("sign") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN sign projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax("RETURN sign projection requires an argument"));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN sign projection",
        "writable Cypher RETURN sign only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnNumericSignProjection {
            variable,
            target: Box::new(target),
        },
    )))
}

pub(crate) fn parse_return_numeric_cast_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnNumericCastProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let cast = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "tointeger" => CypherReturnNumericCast::Integer,
        "tofloat" => CypherReturnNumericCast::Float,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN numeric cast projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN numeric cast projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN numeric cast projection",
        "writable Cypher RETURN toInteger/toFloat only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnNumericCastProjection {
            variable,
            target: Box::new(target),
            cast,
        },
    )))
}

pub(crate) fn parse_return_list_cast_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnListCastProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let cast = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "tostringlist" => CypherReturnListCast::String,
        "tointegerlist" => CypherReturnListCast::Integer,
        "tofloatlist" => CypherReturnListCast::Float,
        "tobooleanlist" => CypherReturnListCast::Boolean,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN list cast projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN list cast projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN list cast projection",
        "writable Cypher RETURN list casts only support restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnListCastProjection {
            variable,
            target: Box::new(target),
            cast,
        },
    )))
}

pub(crate) fn parse_return_to_boolean_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnToBooleanProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("toBoolean") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN toBoolean projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN toBoolean projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN toBoolean projection",
        "writable Cypher RETURN toBoolean only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnToBooleanProjection {
            variable,
            target: Box::new(target),
        },
    )))
}

pub(crate) fn parse_return_to_string_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnToStringProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("toString") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN toString projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN toString projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN toString projection",
        "writable Cypher RETURN toString only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnToStringProjection {
            variable,
            target: Box::new(target),
        },
    )))
}

pub(crate) fn parse_return_is_empty_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnIsEmptyProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("isEmpty") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN isEmpty projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN isEmpty projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN isEmpty projection",
        "writable Cypher RETURN isEmpty only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnIsEmptyProjection {
            variable,
            target: Box::new(target),
        },
    )))
}

pub(crate) fn parse_return_string_transform_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnStringTransformProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let transform = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "tolower" => CypherReturnStringTransform::Lower,
        "toupper" => CypherReturnStringTransform::Upper,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN string transform projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN string transform projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN string transform projection",
        "writable Cypher RETURN string transforms only support restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnStringTransformProjection {
            variable,
            target: Box::new(target),
            transform,
        },
    )))
}

pub(crate) fn parse_return_string_trim_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnStringTrimProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let trim = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "trim" => CypherReturnStringTrim::Both,
        "ltrim" => CypherReturnStringTrim::Left,
        "rtrim" => CypherReturnStringTrim::Right,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN string trim projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN string trim projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN string trim projection",
        "writable Cypher RETURN string trims only support restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnStringTrimProjection {
            variable,
            target: Box::new(target),
            trim,
        },
    )))
}

pub(crate) fn parse_return_string_reverse_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnStringReverseProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("reverse") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN string reverse projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN string reverse projection requires an argument",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        body,
        parameters,
        "RETURN string reverse projection",
        "writable Cypher RETURN reverse only supports restricted scalar arguments",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnStringReverseProjection {
            variable,
            target: Box::new(target),
        },
    )))
}

pub(crate) fn parse_return_string_split_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnStringSplit)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("split") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN string split projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN string split projection requires arguments",
        ));
    }
    let arguments = split_top_level_commas(body)?;
    if arguments.len() != 2 {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN split requires a restricted scalar argument and delimiter",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        arguments[0].trim(),
        parameters,
        "RETURN string split projection",
        "writable Cypher RETURN split requires a restricted scalar first argument",
    )?;
    let delimiter = parse_string_literal_argument(
        arguments[1].trim(),
        parameters,
        "RETURN string split delimiter",
    )?;
    if delimiter.is_empty() {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN split delimiter must be non-empty",
        ));
    }
    Ok(Some((
        variable.clone(),
        CypherReturnStringSplit {
            variable,
            target: Box::new(target),
            delimiter,
        },
    )))
}

pub(crate) fn parse_return_substring_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnSubstring)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("substring") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN substring projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN substring projection requires arguments",
        ));
    }
    let arguments = split_top_level_commas(body)?;
    if !(2..=3).contains(&arguments.len()) {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN substring requires a restricted scalar argument, start, and optional length",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        arguments[0].trim(),
        parameters,
        "RETURN substring projection",
        "writable Cypher RETURN substring requires a restricted scalar first argument",
    )?;
    let start = parse_non_negative_usize_literal(
        arguments[1].trim(),
        parameters,
        "RETURN substring start",
    )?;
    let length = if let Some(length) = arguments.get(2) {
        Some(parse_non_negative_usize_literal(
            length.trim(),
            parameters,
            "RETURN substring length",
        )?)
    } else {
        None
    };
    Ok(Some((
        variable.clone(),
        CypherReturnSubstring {
            variable,
            target: Box::new(target),
            start,
            length,
        },
    )))
}

pub(crate) fn parse_non_negative_usize_literal(
    expression: &str,
    parameters: &CypherParameters,
    context: &str,
) -> Result<usize> {
    let value = parse_cypher_literal(expression, parameters)?;
    let Value::Int(value) = value else {
        return Err(cypher_unsupported_cardinality(format!(
            "{context} must be an integer literal or parameter"
        )));
    };
    usize::try_from(value).map_err(|_| {
        cypher_unsupported_cardinality(format!("{context} must be a non-negative integer"))
    })
}

pub(crate) fn parse_return_string_slice_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnStringSlice)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let side = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "left" => CypherReturnStringSliceSide::Left,
        "right" => CypherReturnStringSliceSide::Right,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN string slice projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN string slice projection requires arguments",
        ));
    }
    let arguments = split_top_level_commas(body)?;
    if arguments.len() != 2 {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN left/right requires a restricted scalar argument and length",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        arguments[0].trim(),
        parameters,
        "RETURN string slice projection",
        "writable Cypher RETURN left/right requires a restricted scalar first argument",
    )?;
    let length = parse_non_negative_usize_literal(
        arguments[1].trim(),
        parameters,
        "RETURN left/right length",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnStringSlice {
            variable,
            target: Box::new(target),
            side,
            length,
        },
    )))
}

pub(crate) fn parse_return_replace_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnReplace)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    if !expression[..open].trim().eq_ignore_ascii_case("replace") {
        return Ok(None);
    }
    if !expression.ends_with(')') {
        return Err(cypher_syntax("RETURN replace projection is missing ')'"));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN replace projection requires arguments",
        ));
    }
    let arguments = split_top_level_commas(body)?;
    if arguments.len() != 3 {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN replace requires a restricted scalar argument, search, and replacement",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        arguments[0].trim(),
        parameters,
        "RETURN replace projection",
        "writable Cypher RETURN replace requires a restricted scalar first argument",
    )?;
    let search =
        parse_string_literal_argument(arguments[1].trim(), parameters, "RETURN replace search")?;
    let replacement = parse_string_literal_argument(
        arguments[2].trim(),
        parameters,
        "RETURN replace replacement",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnReplace {
            variable,
            target: Box::new(target),
            search,
            replacement,
        },
    )))
}

pub(crate) fn parse_return_string_predicate_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(Option<String>, CypherReturnStringPredicateProjection)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let predicate = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "startswith" => CypherReturnStringPredicate::StartsWith,
        "endswith" => CypherReturnStringPredicate::EndsWith,
        "contains" => CypherReturnStringPredicate::Contains,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN string predicate projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN string predicate projection requires arguments",
        ));
    }
    let arguments = split_top_level_commas(body)?;
    if arguments.len() != 2 {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN string predicates require a restricted scalar argument and needle",
        ));
    }
    let (variable, target) = parse_nested_restricted_scalar_target(
        arguments[0].trim(),
        parameters,
        "RETURN string predicate projection",
        "writable Cypher RETURN string predicates require a restricted scalar first argument",
    )?;
    let needle = parse_string_literal_argument(
        arguments[1].trim(),
        parameters,
        "RETURN string predicate needle",
    )?;
    Ok(Some((
        variable.clone(),
        CypherReturnStringPredicateProjection {
            variable,
            target: Box::new(target),
            predicate,
            needle,
        },
    )))
}

pub(crate) fn parse_string_literal_argument(
    expression: &str,
    parameters: &CypherParameters,
    context: &str,
) -> Result<String> {
    let value = parse_cypher_literal(expression, parameters)?;
    let Value::String(value) = value else {
        return Err(cypher_unsupported_cardinality(format!(
            "{context} must be a string literal or parameter"
        )));
    };
    Ok(value)
}

pub(crate) fn parse_return_element_function_projection(
    expression: &str,
) -> Result<Option<(String, CypherReturnTarget)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let target = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "labels" => CypherReturnTarget::NodeLabels,
        "type" => CypherReturnTarget::RelationshipType,
        "properties" => CypherReturnTarget::ElementProperties,
        "keys" => CypherReturnTarget::ElementKeys,
        "id" | "elementid" => CypherReturnTarget::ElementId,
        "startnode" => CypherReturnTarget::RelationshipStartNode,
        "endnode" => CypherReturnTarget::RelationshipEndNode,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN element function projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.contains('(') || body.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN element functions do not support nested expressions",
        ));
    }
    let variable = parse_required_cypher_variable(body, "RETURN element function variable")?;
    Ok(Some((variable, target)))
}

pub(crate) fn parse_return_map_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<CypherReturnMapProjection>> {
    let expression = expression.trim();
    let Some(open) = find_unquoted(expression, '{') else {
        return Ok(None);
    };
    if !expression.ends_with('}') {
        return Err(cypher_syntax("RETURN map projection is missing '}'"));
    }
    let variable = parse_required_cypher_variable(
        expression[..open].trim(),
        "RETURN map projection variable",
    )?;
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN map projection requires at least one entry",
        ));
    }
    let mut entries = Vec::new();
    for selector in split_top_level_commas(body)? {
        let selector = selector.trim();
        if selector.is_empty() {
            return Err(cypher_syntax(
                "RETURN map projection contains an empty entry",
            ));
        }
        let entry = if let Some(key) = selector.strip_prefix('.') {
            let key = key.trim();
            validate_json_key(key)?;
            CypherReturnMapProjectionEntry {
                output_key: key.to_string(),
                value: CypherReturnTarget::Property(key.to_string()),
            }
        } else {
            let Some(colon) = find_unquoted(selector, ':') else {
                return Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN map projections only support .property selectors and key: literal/property entries",
                ));
            };
            if find_unquoted(&selector[colon + 1..], ':').is_some() {
                return Err(cypher_unsupported_cardinality(
                    "writable Cypher RETURN map projection entries only support one ':' separator",
                ));
            }
            let output_key = selector[..colon].trim();
            validate_json_key(output_key)?;
            let value = selector[colon + 1..].trim();
            if value.is_empty() {
                return Err(cypher_syntax(
                    "RETURN map projection entry requires a value",
                ));
            }
            let value = parse_return_map_projection_value(value, &variable, parameters)?;
            CypherReturnMapProjectionEntry {
                output_key: output_key.to_string(),
                value,
            }
        };
        if entries
            .iter()
            .any(|existing: &CypherReturnMapProjectionEntry| {
                existing.output_key == entry.output_key
            })
        {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN map projection entries must have unique output keys",
            ));
        }
        entries.push(entry);
    }
    Ok(Some(CypherReturnMapProjection { variable, entries }))
}

pub(crate) fn parse_return_map_projection_value(
    value: &str,
    map_variable: &str,
    parameters: &CypherParameters,
) -> Result<CypherReturnTarget> {
    let (value_variable, target) = parse_nested_restricted_scalar_target(
        value,
        parameters,
        "RETURN map projection entry",
        "writable Cypher RETURN map projection entries only support restricted scalar values",
    )?;
    if let Some(value_variable) = value_variable
        && value_variable != map_variable
    {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN map projection values must reference the projection variable",
        ));
    }
    Ok(target)
}

pub(crate) fn parse_return_list_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<CypherReturnListProjection>> {
    let expression = expression.trim();
    if !expression.starts_with('[') {
        return Ok(None);
    }
    if !expression.ends_with(']') {
        return Err(cypher_syntax("RETURN list projection is missing ']'"));
    }
    let body = expression[1..expression.len() - 1].trim();
    if body.is_empty() {
        return Err(cypher_syntax(
            "RETURN list projection requires at least one item",
        ));
    }
    let mut variable = None;
    let mut terms = Vec::new();
    for item in split_top_level_commas(body)? {
        let item = item.trim();
        if item.is_empty() {
            return Err(cypher_syntax(
                "RETURN list projection contains an empty item",
            ));
        }
        let (item_variable, target) = parse_return_list_projection_term(item, parameters)?;
        merge_return_list_projection_variable(&mut variable, item_variable)?;
        terms.push(target);
    }
    Ok(Some(CypherReturnListProjection { variable, terms }))
}

pub(crate) fn parse_return_list_projection_term(
    item: &str,
    parameters: &CypherParameters,
) -> Result<(Option<String>, CypherReturnTarget)> {
    parse_nested_restricted_scalar_target(
        item,
        parameters,
        "RETURN list projection",
        "writable Cypher RETURN list projections only support restricted scalar items",
    )
}

pub(crate) fn merge_return_list_projection_variable(
    variable: &mut Option<String>,
    item_variable: Option<String>,
) -> Result<()> {
    let Some(item_variable) = item_variable else {
        return Ok(());
    };
    if let Some(variable) = variable {
        if variable != &item_variable {
            return Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN list projections must reference one variable",
            ));
        }
    } else {
        *variable = Some(item_variable);
    }
    Ok(())
}

pub(crate) fn parse_return_case_projection(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<(String, CypherReturnCase)>> {
    let expression = expression.trim();
    let Some(after_case) = strip_leading_keyword(expression, "CASE") else {
        return Ok(None);
    };
    let after_when = strip_leading_keyword(after_case.trim_start(), "WHEN")
        .ok_or_else(|| cypher_syntax("RETURN CASE requires WHEN"))?;
    let Some(then_index) = find_unquoted_keyword(after_when, "THEN") else {
        return Err(cypher_syntax("RETURN CASE requires THEN"));
    };
    let condition = after_when[..then_index].trim();
    let after_then = after_when[then_index + "THEN".len()..].trim_start();
    let Some(else_index) = find_unquoted_keyword(after_then, "ELSE") else {
        return Err(cypher_syntax("RETURN CASE requires ELSE"));
    };
    let then_value = after_then[..else_index].trim();
    let after_else = after_then[else_index + "ELSE".len()..].trim_start();
    let Some(end_index) = find_unquoted_keyword(after_else, "END") else {
        return Err(cypher_syntax("RETURN CASE requires END"));
    };
    let else_value = after_else[..end_index].trim();
    if !after_else[end_index + "END".len()..].trim().is_empty() {
        return Err(cypher_syntax(
            "RETURN CASE does not support trailing content after END",
        ));
    }
    let Some(equals_index) = find_unquoted(condition, '=') else {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN CASE only supports property equality predicates",
        ));
    };
    let (variable, key) = parse_property_ref(
        condition[..equals_index].trim(),
        "RETURN CASE predicate property",
    )
    .map_err(|_| {
        cypher_unsupported_cardinality(
            "writable Cypher RETURN CASE only supports property equality predicates",
        )
    })?;
    let equals = parse_cypher_literal(&condition[equals_index + 1..], parameters)?;
    let (then_variable, then_target) = parse_nested_restricted_scalar_target(
        then_value,
        parameters,
        "RETURN CASE THEN value",
        "writable Cypher RETURN CASE branches only support restricted scalar values",
    )?;
    let (else_variable, else_target) = parse_nested_restricted_scalar_target(
        else_value,
        parameters,
        "RETURN CASE ELSE value",
        "writable Cypher RETURN CASE branches only support restricted scalar values",
    )?;
    let mut branch_variable = Some(variable.clone());
    merge_single_return_variable(
        &mut branch_variable,
        then_variable,
        "writable Cypher RETURN CASE branches must reference the predicate variable",
    )?;
    merge_single_return_variable(
        &mut branch_variable,
        else_variable,
        "writable Cypher RETURN CASE branches must reference the predicate variable",
    )?;
    Ok(Some((
        variable,
        CypherReturnCase {
            key,
            equals,
            then_target: Box::new(then_target),
            else_target: Box::new(else_target),
        },
    )))
}

pub(crate) fn parse_return_path_function_projection(
    expression: &str,
) -> Result<Option<(String, CypherReturnTarget)>> {
    let expression = expression.trim();
    let Some(open) = expression.find('(') else {
        return Ok(None);
    };
    let target = match expression[..open].trim().to_ascii_lowercase().as_str() {
        "length" => CypherReturnTarget::PathLength,
        "nodes" => CypherReturnTarget::PathNodes,
        "relationships" => CypherReturnTarget::PathRelationships,
        _ => return Ok(None),
    };
    if !expression.ends_with(')') {
        return Err(cypher_syntax(
            "RETURN path function projection is missing ')'",
        ));
    }
    let body = expression[open + 1..expression.len() - 1].trim();
    if body.contains('(') || body.contains(')') {
        return Err(cypher_unsupported_cardinality(
            "writable Cypher RETURN path functions do not support nested expressions",
        ));
    }
    let variable = parse_required_cypher_variable(body, "RETURN path function variable")?;
    Ok(Some((variable, target)))
}

pub(crate) fn append_star_return_projections(
    projections: &mut Vec<CypherReturnProjection>,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_bindings: &HashMap<String, GraphNodeMatch>,
    row_edge_match_bindings: &HashMap<String, GraphRelationshipMatch>,
    row_edge_bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
) -> Result<()> {
    let mut variables = BTreeSet::new();
    variables.extend(node_bindings.keys().cloned());
    variables.extend(edge_bindings.keys().cloned());
    variables.extend(row_node_bindings.keys().cloned());
    variables.extend(row_edge_match_bindings.keys().cloned());
    variables.extend(row_edge_bindings.keys().cloned());
    variables.extend(row_path_bindings.keys().cloned());
    for binding in row_edge_bindings.values() {
        variables.insert(binding.from_variable.clone());
        variables.insert(binding.to_variable.clone());
    }
    if variables.is_empty() {
        return Err(cypher_unresolved_identity(
            "RETURN * has no variables bound by the write plan",
        ));
    }
    for variable in variables {
        let element = cypher_return_element_for_variable(
            &variable,
            node_bindings,
            edge_bindings,
            row_node_bindings,
            row_edge_match_bindings,
            row_edge_bindings,
            row_path_bindings,
        )?;
        projections.push(CypherReturnProjection {
            variable: variable.clone(),
            target: CypherReturnTarget::Element,
            column: variable.clone(),
            expression: variable,
            element,
            aggregate: None,
            distinct: false,
        });
    }
    Ok(())
}

pub(crate) fn cypher_return_element_for_variable(
    variable: &str,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_bindings: &HashMap<String, GraphNodeMatch>,
    row_edge_match_bindings: &HashMap<String, GraphRelationshipMatch>,
    row_edge_bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
) -> Result<CypherReturnElement> {
    match (
        node_bindings.contains_key(variable),
        edge_bindings.contains_key(variable),
        row_node_bindings.contains_key(variable),
        row_edge_match_bindings.contains_key(variable),
        row_edge_bindings.contains_key(variable),
        row_edge_endpoint_variable(variable, row_edge_bindings),
        row_path_bindings.contains_key(variable),
    ) {
        (true, false, false, false, false, false, false)
        | (true, false, false, false, false, true, false) => Ok(CypherReturnElement::Node),
        (false, true, false, false, false, false, false) => Ok(CypherReturnElement::Edge),
        (false, false, true, false, false, false, false)
        | (false, false, true, false, false, true, false)
        | (false, false, false, false, false, true, false) => Ok(CypherReturnElement::RowNode),
        (false, false, false, true, false, false, false)
        | (false, false, false, false, true, false, false) => Ok(CypherReturnElement::RowEdge),
        (false, false, false, false, false, false, true) => Ok(CypherReturnElement::RowPath),
        (true, _, _, _, _, _, _) | (_, true, _, _, _, _, _) | (_, _, true, _, _, _, _) => {
            Err(cypher_unresolved_identity(format!(
                "RETURN variable '{variable}' is ambiguously bound",
            )))
        }
        (false, false, false, true, true, _, _) => Err(cypher_unresolved_identity(format!(
            "RETURN relationship variable '{variable}' is ambiguously bound",
        ))),
        (false, false, false, _, _, true, _) => Err(cypher_unresolved_identity(format!(
            "RETURN variable '{variable}' is ambiguously bound",
        ))),
        (false, false, false, _, _, _, true) => Err(cypher_unresolved_identity(format!(
            "RETURN path variable '{variable}' is ambiguously bound",
        ))),
        (false, false, false, false, false, false, false) => Err(cypher_unresolved_identity(
            format!("RETURN references variable '{variable}' that is not bound by the write plan"),
        )),
    }
}

pub(crate) fn row_edge_endpoint_variable(
    variable: &str,
    row_edge_bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
) -> bool {
    row_edge_bindings
        .values()
        .any(|binding| binding.from_variable == variable || binding.to_variable == variable)
}

pub(crate) fn validate_return_variable_binding(
    variable: &str,
    node_bindings: &HashMap<String, NodeId>,
    edge_bindings: &HashMap<String, CypherBoundEdgeIdentity>,
    row_node_bindings: &HashMap<String, GraphNodeMatch>,
    row_edge_match_bindings: &HashMap<String, GraphRelationshipMatch>,
    row_edge_bindings: &HashMap<String, CypherRowProducedEdgeBinding>,
    row_path_bindings: &HashMap<String, CypherRowProducedPathBinding>,
) -> Result<()> {
    match (
        node_bindings.contains_key(variable),
        edge_bindings.contains_key(variable),
        row_node_bindings.contains_key(variable),
        row_edge_match_bindings.contains_key(variable),
        row_edge_bindings.contains_key(variable),
        row_edge_endpoint_variable(variable, row_edge_bindings),
        row_path_bindings.contains_key(variable),
    ) {
        (true, false, false, false, false, false, false)
        | (true, false, false, false, false, true, false)
        | (false, true, false, false, false, false, false)
        | (false, false, true, false, false, false, false)
        | (false, false, true, false, false, true, false)
        | (false, false, false, true, false, false, false)
        | (false, false, false, false, true, false, false)
        | (false, false, false, false, false, true, false)
        | (false, false, false, false, false, false, true) => Ok(()),
        (true, _, _, _, _, _, _) | (_, true, _, _, _, _, _) | (_, _, true, _, _, _, _) => {
            Err(cypher_unresolved_identity(format!(
                "RETURN variable '{variable}' is ambiguously bound",
            )))
        }
        (false, false, false, true, true, _, _) => Err(cypher_unresolved_identity(format!(
            "RETURN relationship variable '{variable}' is ambiguously bound",
        ))),
        (false, false, false, _, _, true, _) => Err(cypher_unresolved_identity(format!(
            "RETURN variable '{variable}' is ambiguously bound",
        ))),
        (false, false, false, _, _, _, true) => Err(cypher_unresolved_identity(format!(
            "RETURN path variable '{variable}' is ambiguously bound",
        ))),
        (false, false, false, false, false, false, false) => Err(cypher_unresolved_identity(
            format!("RETURN references variable '{variable}' that is not bound by the write plan"),
        )),
    }
}

pub(crate) fn validate_return_function_target(
    target: &CypherReturnTarget,
    element: CypherReturnElement,
) -> Result<()> {
    match target {
        CypherReturnTarget::NodeLabels
            if !matches!(
                element,
                CypherReturnElement::Node | CypherReturnElement::RowNode
            ) =>
        {
            Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN labels(...) requires a bound node variable",
            ))
        }
        CypherReturnTarget::RelationshipType
            if !matches!(
                element,
                CypherReturnElement::Edge | CypherReturnElement::RowEdge
            ) =>
        {
            Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN type(...) requires a bound relationship variable",
            ))
        }
        CypherReturnTarget::ElementProperties
        | CypherReturnTarget::ElementKeys
        | CypherReturnTarget::ElementId
            if !matches!(
                element,
                CypherReturnElement::Node
                    | CypherReturnElement::Edge
                    | CypherReturnElement::RowNode
                    | CypherReturnElement::RowEdge
            ) =>
        {
            Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN properties(...), keys(...), id(...), and elementId(...) require a bound node or relationship variable",
            ))
        }
        CypherReturnTarget::RelationshipStartNode | CypherReturnTarget::RelationshipEndNode
            if !matches!(
                element,
                CypherReturnElement::Edge | CypherReturnElement::RowEdge
            ) =>
        {
            Err(cypher_unsupported_cardinality(
                "writable Cypher RETURN startNode(...) and endNode(...) require a bound relationship variable",
            ))
        }
        _ => Ok(()),
    }
}

pub(crate) fn strip_trailing_keyword<'a>(value: &'a str, keyword: &str) -> Option<&'a str> {
    let value = value.trim_end();
    let candidate = value.get(value.len().checked_sub(keyword.len())?..)?;
    if !candidate.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let prefix = &value[..value.len() - keyword.len()];
    // Require a word boundary so we do not strip the tail of an identifier.
    if prefix
        .chars()
        .next_back()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(prefix.trim_end())
}

pub(crate) fn parse_return_count(value: &str, context: &str) -> Result<usize> {
    let value = value.trim();
    value.parse::<usize>().map_err(|_| {
        cypher_syntax(format!(
            "{context} requires a non-negative integer, got '{value}'"
        ))
    })
}

pub(crate) fn parse_return_limit(value: &str) -> Result<Option<usize>> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("ALL") {
        return Ok(None);
    }
    Ok(Some(parse_return_count(value, "LIMIT")?))
}

pub(crate) fn split_return_alias(projection: &str) -> Result<(&str, Option<String>)> {
    let Some(index) = find_unquoted_keyword(projection, "AS") else {
        return Ok((projection, None));
    };
    let expression = projection[..index].trim();
    let alias = projection[index + "AS".len()..].trim();
    if expression.is_empty() || alias.is_empty() {
        return Err(cypher_syntax(
            "RETURN aliases require both an expression and an alias",
        ));
    }
    let alias = parse_required_cypher_variable(alias, "RETURN alias")?;
    Ok((expression, Some(alias)))
}

pub(crate) struct PatchAssignment {
    pub(crate) target: String,
    pub(crate) kind: PatchAssignmentKind,
}

pub(crate) enum PatchAssignmentKind {
    Props(Props),
    RemoveProperty {
        key: String,
    },
    NumericExpression {
        key: String,
        source_target: String,
        source_key: String,
        op: GraphNumericOp,
        operand: Value,
    },
}

pub(crate) fn parse_patch_assignment(
    assignment: &str,
    parameters: &CypherParameters,
    null_assignment: CypherNullAssignment,
) -> Result<PatchAssignment> {
    if let Some(index) = find_unquoted_sequence(assignment, "+=") {
        let target = parse_required_cypher_variable(&assignment[..index], "MATCH SET target")?;
        let props = parse_cypher_props_map_literal(&assignment[index + 2..], parameters)?;
        return Ok(PatchAssignment {
            target,
            kind: PatchAssignmentKind::Props(props),
        });
    }
    let Some(index) = find_unquoted(assignment, '=') else {
        return Err(cypher_syntax(
            "MATCH SET only supports map patch or literal property assignment",
        ));
    };
    let (target, key) = parse_property_ref(&assignment[..index], "MATCH SET target")?;
    let rhs = &assignment[index + 1..];
    if let Some(expression) = parse_numeric_expression(rhs, parameters)? {
        return Ok(PatchAssignment {
            target,
            kind: PatchAssignmentKind::NumericExpression {
                key,
                source_target: expression.source_target,
                source_key: expression.source_key,
                op: expression.op,
                operand: expression.operand,
            },
        });
    }
    let value = parse_cypher_literal(rhs, parameters)?;
    if value == Value::Null && null_assignment == CypherNullAssignment::RemoveProperty {
        return Ok(PatchAssignment {
            target,
            kind: PatchAssignmentKind::RemoveProperty { key },
        });
    }
    Ok(PatchAssignment {
        target,
        kind: PatchAssignmentKind::Props(Props::from([(key, value)])),
    })
}

pub(crate) fn parse_patch_assignments(
    assignments: &str,
    parameters: &CypherParameters,
    null_assignment: CypherNullAssignment,
) -> Result<Vec<PatchAssignment>> {
    split_top_level_commas(assignments)?
        .into_iter()
        .map(str::trim)
        .map(|assignment| {
            if assignment.is_empty() {
                Err(cypher_syntax("MATCH SET contains an empty assignment"))
            } else {
                parse_patch_assignment(assignment, parameters, null_assignment)
            }
        })
        .collect()
}

pub fn cypher_written_edge_identity(
    kind: GraphMutationPlanKind,
    edge: &Edge,
) -> CypherWrittenEdgeIdentity {
    CypherWrittenEdgeIdentity {
        kind,
        from: edge.from.clone(),
        label: edge.label.clone(),
        to: edge.to.clone(),
        id: edge.id.clone(),
    }
}

pub fn cypher_written_node_identity(
    kind: GraphMutationPlanKind,
    node: &Node,
) -> CypherWrittenNodeIdentity {
    CypherWrittenNodeIdentity {
        kind,
        label: node.label.clone(),
        id: node.id.clone(),
    }
}
