//! WHERE predicate parsing, canonicalization, and boolean lowering (extracted from lib.rs).

use crate::*;

pub(crate) fn split_match_where<'a>(
    pattern: &'a str,
    parameters: &CypherParameters,
) -> Result<(&'a str, Vec<ParsedWherePredicate>)> {
    let Some(index) = find_unquoted_keyword(pattern, "WHERE") else {
        return Ok((pattern.trim(), Vec::new()));
    };
    let match_pattern = pattern[..index].trim();
    let where_clause = pattern[index + "WHERE".len()..].trim();
    if match_pattern.is_empty() || where_clause.is_empty() {
        return Err(cypher_syntax(
            "MATCH WHERE requires both a pattern and predicate".to_string(),
        ));
    }
    let ast = parse_where_boolean_ast(where_clause)?;
    let mut predicates = lower_where_boolean_ast(&ast, parameters)?;
    canonicalize_where_predicates(&mut predicates)?;
    Ok((match_pattern, predicates))
}

pub(crate) fn canonicalize_where_predicates(predicates: &mut Vec<ParsedWherePredicate>) -> Result<()> {
    dedupe_where_predicates(predicates);
    merge_where_membership_predicates(predicates)?;
    merge_where_equality_membership_predicates(predicates)?;
    merge_where_order_predicates(predicates);
    merge_where_equality_order_predicates(predicates);
    merge_where_membership_order_predicates(predicates)?;
    Ok(())
}

pub(crate) fn dedupe_where_predicates(predicates: &mut Vec<ParsedWherePredicate>) {
    let mut deduped = Vec::with_capacity(predicates.len());
    for predicate in predicates.drain(..) {
        if !deduped.contains(&predicate) {
            deduped.push(predicate);
        }
    }
    *predicates = deduped;
}

pub(crate) fn merge_where_membership_predicates(predicates: &mut Vec<ParsedWherePredicate>) -> Result<()> {
    let mut merged = Vec::with_capacity(predicates.len());
    for predicate in predicates.drain(..) {
        if !matches!(
            predicate.predicate.op,
            GraphPredicateOp::In | GraphPredicateOp::NotIn
        ) {
            merged.push(predicate);
            continue;
        }

        let Some(existing) = merged
            .iter_mut()
            .find(|existing: &&mut ParsedWherePredicate| {
                existing.target == predicate.target
                    && existing.predicate.key == predicate.predicate.key
                    && existing.predicate.op == predicate.predicate.op
            })
        else {
            merged.push(predicate);
            continue;
        };

        match predicate.predicate.op {
            GraphPredicateOp::In => {
                let intersection = intersect_membership_values(
                    &existing.predicate.value,
                    &predicate.predicate.value,
                )?;
                existing.predicate.value = Value::Json(serde_json::Value::Array(intersection));
            }
            GraphPredicateOp::NotIn => {
                let union =
                    union_membership_values(&existing.predicate.value, &predicate.predicate.value)?;
                existing.predicate.value = Value::Json(serde_json::Value::Array(union));
            }
            _ => unreachable!(),
        }
    }
    *predicates = merged;
    Ok(())
}

pub(crate) fn intersect_membership_values(left: &Value, right: &Value) -> Result<Vec<serde_json::Value>> {
    let right_values = cypher_in_predicate_values(right)?
        .into_iter()
        .map(|value| value.to_json())
        .collect::<Vec<_>>();
    let mut intersection = Vec::new();
    for value in cypher_in_predicate_values(left)? {
        let value = value.to_json();
        if right_values.contains(&value) {
            push_unique(&mut intersection, value);
        }
    }
    Ok(intersection)
}

pub(crate) fn union_membership_values(left: &Value, right: &Value) -> Result<Vec<serde_json::Value>> {
    let mut union = Vec::new();
    for value in cypher_in_predicate_values(left)?
        .into_iter()
        .chain(cypher_in_predicate_values(right)?)
    {
        push_unique(&mut union, value.to_json());
    }
    Ok(union)
}

pub(crate) fn merge_where_equality_membership_predicates(
    predicates: &mut Vec<ParsedWherePredicate>,
) -> Result<()> {
    let mut merged = Vec::with_capacity(predicates.len());
    for predicate in predicates.drain(..) {
        if !matches!(
            predicate.predicate.op,
            GraphPredicateOp::Equal
                | GraphPredicateOp::NotEqual
                | GraphPredicateOp::In
                | GraphPredicateOp::NotIn
        ) {
            merged.push(predicate);
            continue;
        }

        let Some(existing) = merged
            .iter_mut()
            .find(|existing: &&mut ParsedWherePredicate| {
                existing.target == predicate.target
                    && existing.predicate.key == predicate.predicate.key
                    && matches!(
                        existing.predicate.op,
                        GraphPredicateOp::Equal
                            | GraphPredicateOp::NotEqual
                            | GraphPredicateOp::In
                            | GraphPredicateOp::NotIn
                    )
            })
        else {
            merged.push(predicate);
            continue;
        };

        if !merge_where_equality_membership_pair(existing, &predicate)? {
            merged.push(predicate);
        }
    }
    *predicates = merged;
    Ok(())
}

pub(crate) fn merge_where_equality_membership_pair(
    existing: &mut ParsedWherePredicate,
    incoming: &ParsedWherePredicate,
) -> Result<bool> {
    match (existing.predicate.op, incoming.predicate.op) {
        (GraphPredicateOp::Equal, GraphPredicateOp::Equal) => {
            if existing.predicate.value != incoming.predicate.value {
                set_where_no_match(existing);
            }
        }
        (GraphPredicateOp::Equal, GraphPredicateOp::In) => {
            if !membership_contains_value(&incoming.predicate.value, &existing.predicate.value)? {
                set_where_no_match(existing);
            }
        }
        (GraphPredicateOp::In, GraphPredicateOp::Equal) => {
            if membership_contains_value(&existing.predicate.value, &incoming.predicate.value)? {
                existing.predicate.op = GraphPredicateOp::Equal;
                existing.predicate.value = incoming.predicate.value.clone();
            } else {
                set_where_no_match(existing);
            }
        }
        (GraphPredicateOp::Equal, GraphPredicateOp::NotIn) => {
            if membership_contains_value(&incoming.predicate.value, &existing.predicate.value)? {
                set_where_no_match(existing);
            }
        }
        (GraphPredicateOp::NotIn, GraphPredicateOp::Equal) => {
            if membership_contains_value(&existing.predicate.value, &incoming.predicate.value)? {
                set_where_no_match(existing);
            } else {
                existing.predicate.op = GraphPredicateOp::Equal;
                existing.predicate.value = incoming.predicate.value.clone();
            }
        }
        (GraphPredicateOp::In, GraphPredicateOp::NotIn) => {
            let values =
                difference_membership_values(&existing.predicate.value, &incoming.predicate.value)?;
            existing.predicate.value = Value::Json(serde_json::Value::Array(values));
        }
        (GraphPredicateOp::NotIn, GraphPredicateOp::In) => {
            let values =
                difference_membership_values(&incoming.predicate.value, &existing.predicate.value)?;
            existing.predicate.op = GraphPredicateOp::In;
            existing.predicate.value = Value::Json(serde_json::Value::Array(values));
        }
        (GraphPredicateOp::Equal, GraphPredicateOp::NotEqual) => {
            if existing.predicate.value == incoming.predicate.value {
                set_where_no_match(existing);
            }
        }
        (GraphPredicateOp::NotEqual, GraphPredicateOp::Equal) => {
            if existing.predicate.value == incoming.predicate.value {
                set_where_no_match(existing);
            } else {
                existing.predicate.op = GraphPredicateOp::Equal;
                existing.predicate.value = incoming.predicate.value.clone();
            }
        }
        (GraphPredicateOp::NotEqual, GraphPredicateOp::NotEqual) => {
            let Some(incoming_value) = membership_item_json(&incoming.predicate.value) else {
                return Ok(false);
            };
            let Some(existing_value) = membership_item_json(&existing.predicate.value) else {
                return Ok(false);
            };
            existing.predicate.op = GraphPredicateOp::NotIn;
            existing.predicate.value = Value::Json(serde_json::Value::Array(vec![
                existing_value,
                incoming_value,
            ]));
        }
        (GraphPredicateOp::In, GraphPredicateOp::NotEqual) => {
            let Some(excluded) = membership_item_json(&incoming.predicate.value) else {
                return Ok(false);
            };
            let values = difference_membership_json_values(&existing.predicate.value, &[excluded])?;
            existing.predicate.value = Value::Json(serde_json::Value::Array(values));
        }
        (GraphPredicateOp::NotEqual, GraphPredicateOp::In) => {
            let Some(excluded) = membership_item_json(&existing.predicate.value) else {
                return Ok(false);
            };
            let values = difference_membership_json_values(&incoming.predicate.value, &[excluded])?;
            existing.predicate.op = GraphPredicateOp::In;
            existing.predicate.value = Value::Json(serde_json::Value::Array(values));
        }
        (GraphPredicateOp::NotIn, GraphPredicateOp::NotEqual) => {
            let Some(value) = membership_item_json(&incoming.predicate.value) else {
                return Ok(false);
            };
            let union = union_membership_json_values(&existing.predicate.value, value)?;
            existing.predicate.value = Value::Json(serde_json::Value::Array(union));
        }
        (GraphPredicateOp::NotEqual, GraphPredicateOp::NotIn) => {
            let Some(value) = membership_item_json(&existing.predicate.value) else {
                return Ok(false);
            };
            let union = union_membership_json_values(&incoming.predicate.value, value)?;
            existing.predicate.op = GraphPredicateOp::NotIn;
            existing.predicate.value = Value::Json(serde_json::Value::Array(union));
        }
        (GraphPredicateOp::In, GraphPredicateOp::In)
        | (GraphPredicateOp::NotIn, GraphPredicateOp::NotIn) => {
            unreachable!("same-op membership predicates are merged before equality folding")
        }
        _ => unreachable!("only equality and membership predicates reach this merge path"),
    }
    Ok(true)
}

pub(crate) fn membership_contains_value(membership: &Value, value: &Value) -> Result<bool> {
    let value = value.to_json();
    Ok(cypher_in_predicate_values(membership)?
        .into_iter()
        .any(|candidate| candidate.to_json() == value))
}

pub(crate) fn difference_membership_values(
    positive: &Value,
    excluded: &Value,
) -> Result<Vec<serde_json::Value>> {
    let excluded = cypher_in_predicate_values(excluded)?
        .into_iter()
        .map(|value| value.to_json())
        .collect::<Vec<_>>();
    let mut difference = Vec::new();
    for value in cypher_in_predicate_values(positive)? {
        let value = value.to_json();
        if !excluded.contains(&value) {
            push_unique(&mut difference, value);
        }
    }
    Ok(difference)
}

pub(crate) fn difference_membership_json_values(
    positive: &Value,
    excluded: &[serde_json::Value],
) -> Result<Vec<serde_json::Value>> {
    let mut difference = Vec::new();
    for value in cypher_in_predicate_values(positive)? {
        let value = value.to_json();
        if !excluded.contains(&value) {
            push_unique(&mut difference, value);
        }
    }
    Ok(difference)
}

pub(crate) fn union_membership_json_values(
    membership: &Value,
    value: serde_json::Value,
) -> Result<Vec<serde_json::Value>> {
    let mut union = cypher_in_predicate_values(membership)?
        .into_iter()
        .map(|value| value.to_json())
        .collect::<Vec<_>>();
    push_unique(&mut union, value);
    Ok(union)
}

pub(crate) fn membership_item_json(value: &Value) -> Option<serde_json::Value> {
    validate_cypher_in_item(value).ok()?;
    Some(value.to_json())
}

pub(crate) fn set_where_no_match(predicate: &mut ParsedWherePredicate) {
    predicate.predicate.op = GraphPredicateOp::In;
    predicate.predicate.value = Value::Json(serde_json::Value::Array(Vec::new()));
}

pub(crate) fn is_where_no_match(predicate: &ParsedWherePredicate) -> bool {
    predicate.predicate.op == GraphPredicateOp::In
        && matches!(
            &predicate.predicate.value,
            Value::Json(serde_json::Value::Array(values)) if values.is_empty()
        )
}

pub(crate) fn merge_where_order_predicates(predicates: &mut Vec<ParsedWherePredicate>) {
    let mut merged = Vec::with_capacity(predicates.len());
    for predicate in predicates.drain(..) {
        if !is_order_predicate_op(predicate.predicate.op) {
            merged.push(predicate);
            continue;
        }

        let Some(existing) = merged
            .iter_mut()
            .find(|existing: &&mut ParsedWherePredicate| {
                existing.target == predicate.target
                    && existing.predicate.key == predicate.predicate.key
                    && is_order_predicate_op(existing.predicate.op)
            })
        else {
            merged.push(predicate);
            continue;
        };

        if !merge_where_order_pair(existing, &predicate) {
            merged.push(predicate);
        }
    }
    *predicates = merged;
}

pub(crate) fn merge_where_equality_order_predicates(predicates: &mut Vec<ParsedWherePredicate>) {
    let mut merged = Vec::with_capacity(predicates.len());
    for predicate in predicates.drain(..) {
        if !matches!(predicate.predicate.op, GraphPredicateOp::Equal)
            && !is_order_predicate_op(predicate.predicate.op)
        {
            merged.push(predicate);
            continue;
        }

        let Some(existing) = merged
            .iter_mut()
            .find(|existing: &&mut ParsedWherePredicate| {
                existing.target == predicate.target
                    && existing.predicate.key == predicate.predicate.key
                    && (matches!(existing.predicate.op, GraphPredicateOp::Equal)
                        || is_order_predicate_op(existing.predicate.op))
            })
        else {
            merged.push(predicate);
            continue;
        };

        if !merge_where_equality_order_pair(existing, &predicate) {
            merged.push(predicate);
        }
    }
    *predicates = merged;
}

pub(crate) fn merge_where_equality_order_pair(
    existing: &mut ParsedWherePredicate,
    incoming: &ParsedWherePredicate,
) -> bool {
    match (existing.predicate.op, incoming.predicate.op) {
        (GraphPredicateOp::Equal, op) if is_order_predicate_op(op) => {
            if !equality_satisfies_order_bound(
                &existing.predicate.value,
                op,
                &incoming.predicate.value,
            ) {
                set_where_no_match(existing);
            }
            true
        }
        (op, GraphPredicateOp::Equal) if is_order_predicate_op(op) => {
            if equality_satisfies_order_bound(
                &incoming.predicate.value,
                op,
                &existing.predicate.value,
            ) {
                existing.predicate.op = GraphPredicateOp::Equal;
                existing.predicate.value = incoming.predicate.value.clone();
            } else {
                set_where_no_match(existing);
            }
            true
        }
        _ => false,
    }
}

pub(crate) fn equality_satisfies_order_bound(
    equality_value: &Value,
    bound_op: GraphPredicateOp,
    bound_value: &Value,
) -> bool {
    compare_where_order_values(equality_value, bound_value).is_some_and(|ordering| match bound_op {
        GraphPredicateOp::GreaterThan => ordering.is_gt(),
        GraphPredicateOp::GreaterThanOrEqual => ordering.is_gt() || ordering.is_eq(),
        GraphPredicateOp::LessThan => ordering.is_lt(),
        GraphPredicateOp::LessThanOrEqual => ordering.is_lt() || ordering.is_eq(),
        _ => false,
    })
}

pub(crate) fn order_bound_excludes_value(
    bound_op: GraphPredicateOp,
    bound_value: &Value,
    value: &Value,
) -> bool {
    compare_where_order_values(value, bound_value)
        .map(|_| !equality_satisfies_order_bound(value, bound_op, bound_value))
        .unwrap_or(false)
}

pub(crate) fn merge_where_membership_order_predicates(
    predicates: &mut Vec<ParsedWherePredicate>,
) -> Result<()> {
    loop {
        let before_len = predicates.len();
        merge_where_membership_order_predicates_once(predicates)?;
        if predicates.len() == before_len {
            break;
        }
    }
    Ok(())
}

pub(crate) fn merge_where_membership_order_predicates_once(
    predicates: &mut Vec<ParsedWherePredicate>,
) -> Result<()> {
    let mut merged = Vec::with_capacity(predicates.len());
    for predicate in predicates.drain(..) {
        if predicate.predicate.op != GraphPredicateOp::In
            && !is_order_predicate_op(predicate.predicate.op)
        {
            merged.push(predicate);
            continue;
        }

        let Some(existing) = merged
            .iter_mut()
            .find(|existing: &&mut ParsedWherePredicate| {
                existing.target == predicate.target
                    && existing.predicate.key == predicate.predicate.key
                    && (existing.predicate.op == GraphPredicateOp::In
                        || is_order_predicate_op(existing.predicate.op))
            })
        else {
            merged.push(predicate);
            continue;
        };

        if !merge_where_membership_order_pair(existing, &predicate)? {
            merged.push(predicate);
        }
    }
    *predicates = merged;
    Ok(())
}

pub(crate) fn merge_where_membership_order_pair(
    existing: &mut ParsedWherePredicate,
    incoming: &ParsedWherePredicate,
) -> Result<bool> {
    match (existing.predicate.op, incoming.predicate.op) {
        (GraphPredicateOp::In, op) if is_order_predicate_op(op) => {
            let values = filter_membership_values_by_order_bound(
                &existing.predicate.value,
                op,
                &incoming.predicate.value,
            )?;
            existing.predicate.value = Value::Json(serde_json::Value::Array(values));
            Ok(true)
        }
        (op, GraphPredicateOp::In) if is_order_predicate_op(op) => {
            let values = filter_membership_values_by_order_bound(
                &incoming.predicate.value,
                op,
                &existing.predicate.value,
            )?;
            existing.predicate.op = GraphPredicateOp::In;
            existing.predicate.value = Value::Json(serde_json::Value::Array(values));
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub(crate) fn filter_membership_values_by_order_bound(
    membership: &Value,
    bound_op: GraphPredicateOp,
    bound_value: &Value,
) -> Result<Vec<serde_json::Value>> {
    let mut filtered = Vec::new();
    for value in cypher_in_predicate_values(membership)? {
        if equality_satisfies_order_bound(&value, bound_op, bound_value) {
            push_unique(&mut filtered, value.to_json());
        }
    }
    Ok(filtered)
}

pub(crate) fn is_order_predicate_op(op: GraphPredicateOp) -> bool {
    matches!(
        op,
        GraphPredicateOp::GreaterThan
            | GraphPredicateOp::GreaterThanOrEqual
            | GraphPredicateOp::LessThan
            | GraphPredicateOp::LessThanOrEqual
    )
}

pub(crate) fn is_positive_string_predicate_op(op: GraphPredicateOp) -> bool {
    matches!(
        op,
        GraphPredicateOp::StartsWith | GraphPredicateOp::EndsWith | GraphPredicateOp::Contains
    )
}

pub(crate) fn where_predicate_uses_only_string_values(predicate: &ParsedWherePredicate) -> Result<bool> {
    match predicate.predicate.op {
        GraphPredicateOp::Equal => Ok(matches!(predicate.predicate.value, Value::String(_))),
        GraphPredicateOp::In => {
            membership_values_all_satisfy(&predicate.predicate.value, |value| {
                Ok(matches!(value, Value::String(_)))
            })
        }
        _ => Ok(false),
    }
}

pub(crate) fn merge_where_order_pair(
    existing: &mut ParsedWherePredicate,
    incoming: &ParsedWherePredicate,
) -> bool {
    match (
        order_bound_kind(existing.predicate.op),
        order_bound_kind(incoming.predicate.op),
    ) {
        (Some(OrderBoundKind::Lower), Some(OrderBoundKind::Lower)) => {
            if order_lower_is_stricter(
                incoming.predicate.op,
                &incoming.predicate.value,
                existing.predicate.op,
                &existing.predicate.value,
            ) {
                existing.predicate.op = incoming.predicate.op;
                existing.predicate.value = incoming.predicate.value.clone();
            }
            true
        }
        (Some(OrderBoundKind::Upper), Some(OrderBoundKind::Upper)) => {
            if order_upper_is_stricter(
                incoming.predicate.op,
                &incoming.predicate.value,
                existing.predicate.op,
                &existing.predicate.value,
            ) {
                existing.predicate.op = incoming.predicate.op;
                existing.predicate.value = incoming.predicate.value.clone();
            }
            true
        }
        (Some(OrderBoundKind::Lower), Some(OrderBoundKind::Upper)) => {
            if order_bounds_are_contradictory(
                existing.predicate.op,
                &existing.predicate.value,
                incoming.predicate.op,
                &incoming.predicate.value,
            ) {
                set_where_no_match(existing);
                true
            } else {
                false
            }
        }
        (Some(OrderBoundKind::Upper), Some(OrderBoundKind::Lower)) => {
            if order_bounds_are_contradictory(
                incoming.predicate.op,
                &incoming.predicate.value,
                existing.predicate.op,
                &existing.predicate.value,
            ) {
                set_where_no_match(existing);
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OrderBoundKind {
    Lower,
    Upper,
}

pub(crate) fn order_bound_kind(op: GraphPredicateOp) -> Option<OrderBoundKind> {
    match op {
        GraphPredicateOp::GreaterThan | GraphPredicateOp::GreaterThanOrEqual => {
            Some(OrderBoundKind::Lower)
        }
        GraphPredicateOp::LessThan | GraphPredicateOp::LessThanOrEqual => {
            Some(OrderBoundKind::Upper)
        }
        _ => None,
    }
}

pub(crate) fn order_lower_is_stricter(
    candidate_op: GraphPredicateOp,
    candidate_value: &Value,
    current_op: GraphPredicateOp,
    current_value: &Value,
) -> bool {
    match compare_where_order_values(candidate_value, current_value) {
        Some(std::cmp::Ordering::Greater) => true,
        Some(std::cmp::Ordering::Equal) => {
            candidate_op == GraphPredicateOp::GreaterThan
                && current_op == GraphPredicateOp::GreaterThanOrEqual
        }
        _ => false,
    }
}

pub(crate) fn order_upper_is_stricter(
    candidate_op: GraphPredicateOp,
    candidate_value: &Value,
    current_op: GraphPredicateOp,
    current_value: &Value,
) -> bool {
    match compare_where_order_values(candidate_value, current_value) {
        Some(std::cmp::Ordering::Less) => true,
        Some(std::cmp::Ordering::Equal) => {
            candidate_op == GraphPredicateOp::LessThan
                && current_op == GraphPredicateOp::LessThanOrEqual
        }
        _ => false,
    }
}

pub(crate) fn order_bounds_are_contradictory(
    lower_op: GraphPredicateOp,
    lower_value: &Value,
    upper_op: GraphPredicateOp,
    upper_value: &Value,
) -> bool {
    match compare_where_order_values(lower_value, upper_value) {
        Some(std::cmp::Ordering::Greater) => true,
        Some(std::cmp::Ordering::Equal) => {
            lower_op == GraphPredicateOp::GreaterThan || upper_op == GraphPredicateOp::LessThan
        }
        _ => false,
    }
}

pub(crate) fn compare_where_order_values(left: &Value, right: &Value) -> Option<std::cmp::Ordering> {
    match (left, right) {
        (Value::Int(left), Value::Int(right)) => Some(left.cmp(right)),
        (Value::Float(left), Value::Float(right)) => left.partial_cmp(right),
        (Value::Int(left), Value::Float(right)) => (*left as f64).partial_cmp(right),
        (Value::Float(left), Value::Int(right)) => left.partial_cmp(&(*right as f64)),
        (Value::String(left), Value::String(right)) => Some(left.cmp(right)),
        _ => None,
    }
}

pub(crate) fn split_top_level_and(value: &str) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    let mut rest = value.trim();
    while let Some(index) = find_top_level_keyword(rest, "AND")? {
        let part = rest[..index].trim();
        if part.is_empty() {
            return Err(cypher_syntax("empty predicate before AND".to_string()));
        }
        parts.push(part);
        rest = rest[index + "AND".len()..].trim();
    }
    if rest.is_empty() {
        return Err(cypher_syntax("empty predicate after AND".to_string()));
    }
    parts.push(rest);
    let mut flattened = Vec::new();
    for part in parts {
        let stripped = strip_enclosing_parentheses(part)?;
        if stripped != part.trim() {
            if find_top_level_keyword(stripped, "AND")?.is_some() {
                flattened.extend(split_top_level_and(stripped)?);
            } else {
                flattened.push(part);
            }
        } else {
            flattened.push(part);
        }
    }
    Ok(flattened)
}

pub(crate) fn split_top_level_or(value: &str) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    let mut rest = value.trim();
    while let Some(index) = find_top_level_keyword(rest, "OR")? {
        let part = rest[..index].trim();
        if part.is_empty() {
            return Err(cypher_syntax("empty predicate before OR".to_string()));
        }
        parts.push(part);
        rest = rest[index + "OR".len()..].trim();
    }
    if rest.is_empty() {
        return Err(cypher_syntax("empty predicate after OR".to_string()));
    }
    parts.push(rest);
    let mut flattened = Vec::new();
    for part in parts {
        let stripped = strip_enclosing_parentheses(part)?;
        if stripped != part.trim() {
            flattened.extend(split_top_level_or(stripped)?);
        } else {
            flattened.push(part);
        }
    }
    Ok(flattened)
}

pub(crate) fn parse_where_boolean_ast(predicate: &str) -> Result<CypherWhereBoolean<'_>> {
    let predicate = strip_enclosing_parentheses(predicate.trim())?;
    if predicate.is_empty() {
        return Err(cypher_syntax("MATCH WHERE requires a predicate"));
    }
    let or_terms = split_top_level_or(predicate)?;
    if or_terms.len() > 1 {
        return Ok(CypherWhereBoolean::Or(
            or_terms
                .into_iter()
                .map(parse_where_boolean_ast)
                .collect::<Result<Vec<_>>>()?,
        ));
    }
    let and_terms = split_top_level_and(predicate)?;
    if and_terms.len() > 1 {
        return Ok(CypherWhereBoolean::And(
            and_terms
                .into_iter()
                .map(parse_where_boolean_ast)
                .collect::<Result<Vec<_>>>()?,
        ));
    }
    if let Some(after_not) = strip_leading_keyword(predicate, "NOT") {
        let inner = after_not.trim();
        if inner.is_empty() {
            return Err(cypher_syntax(
                "MATCH WHERE NOT requires a predicate".to_string(),
            ));
        }
        return Ok(CypherWhereBoolean::Not(Box::new(parse_where_boolean_ast(
            inner,
        )?)));
    }
    Ok(CypherWhereBoolean::Predicate(predicate))
}

pub(crate) fn lower_where_boolean_ast(
    predicate: &CypherWhereBoolean<'_>,
    parameters: &CypherParameters,
) -> Result<Vec<ParsedWherePredicate>> {
    match predicate {
        CypherWhereBoolean::Predicate(predicate) => {
            Ok(vec![parse_where_predicate(predicate, parameters)?])
        }
        CypherWhereBoolean::Not(inner) => lower_negated_where_boolean_ast(inner, parameters),
        CypherWhereBoolean::And(terms) => {
            let mut predicates = Vec::new();
            for term in terms {
                predicates.extend(lower_where_boolean_ast(term, parameters)?);
            }
            Ok(predicates)
        }
        CypherWhereBoolean::Or(terms) => {
            if terms.iter().any(where_boolean_contains_and) {
                return lower_where_boolean_or_branches(terms, parameters);
            }
            match parse_where_or_fold_ast_terms(terms, parameters, GraphPredicateOp::In) {
                Ok(predicate) => Ok(vec![predicate]),
                Err(_) => lower_where_boolean_or_branches(terms, parameters),
            }
        }
    }
}

pub(crate) fn lower_where_boolean_or_branches(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Vec<ParsedWherePredicate>> {
    let branches = terms
        .iter()
        .map(|term| {
            let mut branch = lower_where_boolean_ast(term, parameters)?;
            canonicalize_where_predicates(&mut branch)?;
            Ok(branch)
        })
        .collect::<Result<Vec<_>>>()?;
    lower_where_or_branches(branches)
}

pub(crate) fn lower_negated_where_boolean_ast(
    predicate: &CypherWhereBoolean<'_>,
    parameters: &CypherParameters,
) -> Result<Vec<ParsedWherePredicate>> {
    match predicate {
        CypherWhereBoolean::Predicate(predicate) => {
            let negated = format!("NOT {predicate}");
            Ok(vec![parse_where_predicate(&negated, parameters)?])
        }
        CypherWhereBoolean::Or(terms) => {
            if !terms.iter().any(where_boolean_contains_and)
                && let Ok(predicate) =
                    parse_where_or_fold_ast_terms(terms, parameters, GraphPredicateOp::NotIn)
            {
                return Ok(vec![predicate]);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) = lower_negated_null_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) = lower_negated_null_string_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) =
                    lower_negated_null_mixed_string_order_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) =
                    lower_negated_null_string_order_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) =
                    lower_negated_null_mixed_string_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) = lower_negated_null_order_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) =
                    lower_negated_null_mixed_order_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) = lower_negated_order_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) = lower_negated_string_order_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) = lower_negated_mixed_string_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) =
                    lower_negated_mixed_string_order_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            if !terms.iter().any(where_boolean_contains_and)
                && let Some(predicates) = lower_negated_mixed_order_or_terms(terms, parameters)?
            {
                return Ok(predicates);
            }
            let mut predicates = lower_where_boolean_or_branches(terms, parameters)?;
            if predicates.len() != 1 || is_where_no_match(&predicates[0]) {
                return Err(cypher_syntax(
                    "MATCH WHERE NOT over OR only supports collapsed bounded predicate terms",
                ));
            }
            predicates[0].predicate.op = inverted_graph_predicate_op(predicates[0].predicate.op);
            Ok(predicates)
        }
        CypherWhereBoolean::Not(inner) => lower_where_boolean_ast(inner, parameters),
        CypherWhereBoolean::And(terms) => {
            let mut parsed = Vec::new();
            for term in terms {
                let negated = lower_negated_where_boolean_ast(term, parameters)?;
                if negated.len() != 1 {
                    return Err(cypher_syntax(
                        "MATCH WHERE NOT over AND only supports foldable bounded predicate terms",
                    ));
                }
                parsed.extend(negated);
            }
            if let Some(first) = parsed.first()
                && parsed.iter().all(|predicate| predicate == first)
            {
                return Ok(vec![first.clone()]);
            }
            match parse_where_or_fold_parsed_terms(parsed.clone(), GraphPredicateOp::In) {
                Ok(predicate) => Ok(vec![predicate]),
                Err(_) => {
                    let branches = parsed
                        .into_iter()
                        .map(|predicate| vec![predicate])
                        .collect::<Vec<_>>();
                    let predicates = lower_where_or_branches(branches)?;
                    if predicates.len() != 1 || is_where_no_match(&predicates[0]) {
                        return Err(cypher_syntax(
                            "MATCH WHERE NOT over AND only supports collapsed bounded predicate terms",
                        ));
                    }
                    Ok(predicates)
                }
            }
        }
    }
}

pub(crate) fn lower_negated_null_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }
    let mut has_null_branch = false;
    for predicate in &parsed {
        if where_predicate_implies_is_null(predicate)? {
            has_null_branch = true;
            break;
        }
    }
    if !has_null_branch {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    let mut has_not_null_guard = false;
    for predicate in parsed {
        if where_predicate_implies_is_null(&predicate)? {
            if !has_not_null_guard {
                predicates.push(ParsedWherePredicate {
                    target: predicate.target,
                    predicate: GraphPropertyPredicate {
                        key: predicate.predicate.key,
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                });
                has_not_null_guard = true;
            }
            continue;
        }
        let op = match predicate.predicate.op {
            GraphPredicateOp::Equal => GraphPredicateOp::NotEqual,
            GraphPredicateOp::In => GraphPredicateOp::NotIn,
            _ => return Ok(None),
        };
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op,
                value: predicate.predicate.value,
            },
        });
    }
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_null_string_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut string_terms = Vec::new();
    let mut has_null_branch = false;
    for predicate in &parsed {
        if where_predicate_implies_is_null(predicate)? {
            has_null_branch = true;
        } else if is_positive_string_predicate_op(predicate.predicate.op) {
            string_terms.push(predicate.clone());
        } else {
            return Ok(None);
        }
    }
    if !has_null_branch || string_terms.is_empty() {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    if string_terms.len() == 1 {
        let predicate = string_terms
            .into_iter()
            .next()
            .expect("checked non-empty string terms");
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    } else {
        predicates.push(parse_where_or_fold_parsed_terms(
            string_terms,
            GraphPredicateOp::NotIn,
        )?);
    }
    let mut has_not_null_guard = false;
    for predicate in parsed {
        if where_predicate_implies_is_null(&predicate)? {
            if !has_not_null_guard {
                predicates.push(ParsedWherePredicate {
                    target: predicate.target,
                    predicate: GraphPropertyPredicate {
                        key: predicate.predicate.key,
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                });
                has_not_null_guard = true;
            }
            continue;
        }
    }
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_null_string_order_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut string_terms = Vec::new();
    let mut order_terms = Vec::new();
    let mut has_null_branch = false;
    for predicate in &parsed {
        if where_predicate_implies_is_null(predicate)? {
            has_null_branch = true;
        } else if is_positive_string_predicate_op(predicate.predicate.op) {
            string_terms.push(predicate.clone());
        } else if is_order_predicate_op(predicate.predicate.op) {
            if !matches!(predicate.predicate.value, Value::String(_)) {
                return Ok(None);
            }
            order_terms.push(predicate.clone());
        } else {
            return Ok(None);
        }
    }
    if !has_null_branch || string_terms.is_empty() || order_terms.is_empty() {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    if string_terms.len() == 1 {
        let predicate = string_terms
            .into_iter()
            .next()
            .expect("checked non-empty string terms");
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    } else {
        predicates.push(parse_where_or_fold_parsed_terms(
            string_terms,
            GraphPredicateOp::NotIn,
        )?);
    }
    for predicate in order_terms {
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    }

    let mut has_not_null_guard = false;
    for predicate in parsed {
        if where_predicate_implies_is_null(&predicate)? && !has_not_null_guard {
            predicates.push(ParsedWherePredicate {
                target: predicate.target,
                predicate: GraphPropertyPredicate {
                    key: predicate.predicate.key,
                    op: GraphPredicateOp::IsNotNull,
                    value: Value::Null,
                },
            });
            has_not_null_guard = true;
        }
    }
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_null_mixed_string_order_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut string_terms = Vec::new();
    let mut order_terms = Vec::new();
    let mut scalar_terms = Vec::new();
    let mut has_null_branch = false;
    for predicate in &parsed {
        if where_predicate_implies_is_null(predicate)? {
            has_null_branch = true;
        } else if is_positive_string_predicate_op(predicate.predicate.op) {
            string_terms.push(predicate.clone());
        } else if is_order_predicate_op(predicate.predicate.op) {
            if !matches!(predicate.predicate.value, Value::String(_)) {
                return Ok(None);
            }
            order_terms.push(predicate.clone());
        } else if matches!(
            predicate.predicate.op,
            GraphPredicateOp::Equal | GraphPredicateOp::In
        ) {
            if !where_predicate_uses_only_string_values(predicate)? {
                return Ok(None);
            }
            scalar_terms.push(predicate.clone());
        } else {
            return Ok(None);
        }
    }
    if !has_null_branch
        || string_terms.is_empty()
        || order_terms.is_empty()
        || scalar_terms.is_empty()
    {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    if string_terms.len() == 1 {
        let predicate = string_terms
            .into_iter()
            .next()
            .expect("checked non-empty string terms");
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    } else {
        predicates.push(parse_where_or_fold_parsed_terms(
            string_terms,
            GraphPredicateOp::NotIn,
        )?);
    }
    for predicate in order_terms.into_iter().chain(scalar_terms) {
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    }

    let mut has_not_null_guard = false;
    for predicate in parsed {
        if where_predicate_implies_is_null(&predicate)? && !has_not_null_guard {
            predicates.push(ParsedWherePredicate {
                target: predicate.target,
                predicate: GraphPropertyPredicate {
                    key: predicate.predicate.key,
                    op: GraphPredicateOp::IsNotNull,
                    value: Value::Null,
                },
            });
            has_not_null_guard = true;
        }
    }
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_null_mixed_string_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut string_terms = Vec::new();
    let mut scalar_terms = Vec::new();
    let mut has_null_branch = false;
    for predicate in &parsed {
        if where_predicate_implies_is_null(predicate)? {
            has_null_branch = true;
        } else if is_positive_string_predicate_op(predicate.predicate.op) {
            string_terms.push(predicate.clone());
        } else if matches!(
            predicate.predicate.op,
            GraphPredicateOp::Equal | GraphPredicateOp::In
        ) {
            scalar_terms.push(predicate.clone());
        } else {
            return Ok(None);
        }
    }
    if !has_null_branch || string_terms.is_empty() || scalar_terms.is_empty() {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    if string_terms.len() == 1 {
        let predicate = string_terms
            .into_iter()
            .next()
            .expect("checked non-empty string terms");
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    } else {
        predicates.push(parse_where_or_fold_parsed_terms(
            string_terms,
            GraphPredicateOp::NotIn,
        )?);
    }
    for predicate in scalar_terms {
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    }

    let mut has_not_null_guard = false;
    for predicate in parsed {
        if where_predicate_implies_is_null(&predicate)? {
            if !has_not_null_guard {
                predicates.push(ParsedWherePredicate {
                    target: predicate.target,
                    predicate: GraphPropertyPredicate {
                        key: predicate.predicate.key,
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                });
                has_not_null_guard = true;
            }
        }
    }
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_null_order_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut order_bounds = Vec::new();
    let mut has_null_branch = false;
    for predicate in &parsed {
        if where_predicate_implies_is_null(predicate)? {
            has_null_branch = true;
        } else if is_order_predicate_op(predicate.predicate.op) {
            order_bounds.push(&predicate.predicate.value);
        } else {
            return Ok(None);
        }
    }
    if !has_null_branch || order_bounds.is_empty() {
        return Ok(None);
    }
    let Some(first_order_bound) = order_bounds.first() else {
        return Ok(None);
    };
    if order_bounds
        .iter()
        .any(|bound| compare_where_order_values(bound, first_order_bound).is_none())
    {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    let mut has_not_null_guard = false;
    for predicate in parsed {
        if where_predicate_implies_is_null(&predicate)? {
            if !has_not_null_guard {
                predicates.push(ParsedWherePredicate {
                    target: predicate.target,
                    predicate: GraphPropertyPredicate {
                        key: predicate.predicate.key,
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                });
                has_not_null_guard = true;
            }
            continue;
        }
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    }
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_null_mixed_order_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut order_bounds = Vec::new();
    let mut has_null_branch = false;
    let mut has_equality_or_membership = false;
    for predicate in &parsed {
        if where_predicate_implies_is_null(predicate)? {
            has_null_branch = true;
            continue;
        }
        if is_order_predicate_op(predicate.predicate.op) {
            order_bounds.push(&predicate.predicate.value);
            continue;
        }
        if matches!(
            predicate.predicate.op,
            GraphPredicateOp::Equal | GraphPredicateOp::In
        ) {
            has_equality_or_membership = true;
            continue;
        }
        return Ok(None);
    }
    if !has_null_branch || order_bounds.is_empty() || !has_equality_or_membership {
        return Ok(None);
    }
    let Some(first_order_bound) = order_bounds.first() else {
        return Ok(None);
    };
    if order_bounds
        .iter()
        .any(|bound| compare_where_order_values(bound, first_order_bound).is_none())
    {
        return Ok(None);
    }

    for predicate in &parsed {
        if where_predicate_implies_is_null(predicate)? {
            continue;
        }
        match predicate.predicate.op {
            GraphPredicateOp::Equal => {
                if predicate.predicate.value == Value::Null
                    || order_bounds.iter().any(|bound| {
                        compare_where_order_values(&predicate.predicate.value, bound).is_none()
                    })
                {
                    return Ok(None);
                }
            }
            GraphPredicateOp::In => {
                for value in cypher_in_predicate_values(&predicate.predicate.value)? {
                    if value == Value::Null
                        || order_bounds
                            .iter()
                            .any(|bound| compare_where_order_values(&value, bound).is_none())
                    {
                        return Ok(None);
                    }
                }
            }
            _ => {}
        }
    }

    let mut predicates = Vec::new();
    let mut has_not_null_guard = false;
    for predicate in parsed {
        if where_predicate_implies_is_null(&predicate)? {
            if !has_not_null_guard {
                predicates.push(ParsedWherePredicate {
                    target: predicate.target,
                    predicate: GraphPropertyPredicate {
                        key: predicate.predicate.key,
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                });
                has_not_null_guard = true;
            }
            continue;
        }
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    }
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_order_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target
            || predicate.predicate.key != first.predicate.key
            || !is_order_predicate_op(predicate.predicate.op)
            || compare_where_order_values(&predicate.predicate.value, &first.predicate.value)
                .is_none()
    }) {
        return Ok(None);
    }

    let mut predicates = parsed
        .into_iter()
        .map(|predicate| ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        })
        .collect::<Vec<_>>();
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_string_order_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut string_terms = Vec::new();
    let mut order_terms = Vec::new();
    for predicate in &parsed {
        if is_positive_string_predicate_op(predicate.predicate.op) {
            string_terms.push(predicate.clone());
        } else if is_order_predicate_op(predicate.predicate.op) {
            if !matches!(predicate.predicate.value, Value::String(_)) {
                return Ok(None);
            }
            order_terms.push(predicate.clone());
        } else {
            return Ok(None);
        }
    }
    if string_terms.is_empty() || order_terms.is_empty() {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    if string_terms.len() == 1 {
        let predicate = string_terms
            .into_iter()
            .next()
            .expect("checked non-empty string terms");
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    } else {
        predicates.push(parse_where_or_fold_parsed_terms(
            string_terms,
            GraphPredicateOp::NotIn,
        )?);
    }
    for predicate in order_terms {
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    }
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_mixed_string_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut string_terms = Vec::new();
    let mut scalar_terms = Vec::new();
    for predicate in &parsed {
        if is_positive_string_predicate_op(predicate.predicate.op) {
            string_terms.push(predicate.clone());
        } else if matches!(
            predicate.predicate.op,
            GraphPredicateOp::Equal | GraphPredicateOp::In
        ) {
            if !where_predicate_uses_only_string_values(predicate)? {
                return Ok(None);
            }
            scalar_terms.push(predicate.clone());
        } else {
            return Ok(None);
        }
    }
    if string_terms.is_empty() || scalar_terms.is_empty() {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    if string_terms.len() == 1 {
        let predicate = string_terms
            .into_iter()
            .next()
            .expect("checked non-empty string terms");
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    } else {
        predicates.push(parse_where_or_fold_parsed_terms(
            string_terms,
            GraphPredicateOp::NotIn,
        )?);
    }
    for predicate in scalar_terms {
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    }
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_mixed_string_order_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let mut string_terms = Vec::new();
    let mut order_terms = Vec::new();
    let mut scalar_terms = Vec::new();
    for predicate in &parsed {
        if is_positive_string_predicate_op(predicate.predicate.op) {
            string_terms.push(predicate.clone());
        } else if is_order_predicate_op(predicate.predicate.op) {
            if !matches!(predicate.predicate.value, Value::String(_)) {
                return Ok(None);
            }
            order_terms.push(predicate.clone());
        } else if matches!(
            predicate.predicate.op,
            GraphPredicateOp::Equal | GraphPredicateOp::In
        ) {
            if !where_predicate_uses_only_string_values(predicate)? {
                return Ok(None);
            }
            scalar_terms.push(predicate.clone());
        } else {
            return Ok(None);
        }
    }
    if string_terms.is_empty() || order_terms.is_empty() || scalar_terms.is_empty() {
        return Ok(None);
    }

    let mut predicates = Vec::new();
    if string_terms.len() == 1 {
        let predicate = string_terms
            .into_iter()
            .next()
            .expect("checked non-empty string terms");
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    } else {
        predicates.push(parse_where_or_fold_parsed_terms(
            string_terms,
            GraphPredicateOp::NotIn,
        )?);
    }
    for predicate in order_terms.into_iter().chain(scalar_terms) {
        predicates.push(ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        });
    }
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn lower_negated_mixed_order_or_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
) -> Result<Option<Vec<ParsedWherePredicate>>> {
    let mut parsed = Vec::new();
    for term in terms {
        match collect_where_or_fold_ast_terms(term, parameters, &mut parsed) {
            Ok(()) => {}
            Err(_) => return Ok(None),
        };
    }
    let Some(first) = parsed.first() else {
        return Ok(None);
    };
    if parsed.iter().any(|predicate| {
        predicate.target != first.target || predicate.predicate.key != first.predicate.key
    }) {
        return Ok(None);
    }

    let has_order = parsed
        .iter()
        .any(|predicate| is_order_predicate_op(predicate.predicate.op));
    let has_equality_or_membership = parsed.iter().any(|predicate| {
        matches!(
            predicate.predicate.op,
            GraphPredicateOp::Equal | GraphPredicateOp::In
        )
    });
    if !has_order || !has_equality_or_membership {
        return Ok(None);
    }

    let order_bounds = parsed
        .iter()
        .filter(|predicate| is_order_predicate_op(predicate.predicate.op))
        .map(|predicate| &predicate.predicate.value)
        .collect::<Vec<_>>();
    for predicate in &parsed {
        match predicate.predicate.op {
            GraphPredicateOp::Equal => {
                if predicate.predicate.value == Value::Null
                    || order_bounds.iter().any(|bound| {
                        compare_where_order_values(&predicate.predicate.value, bound).is_none()
                    })
                {
                    return Ok(None);
                }
            }
            GraphPredicateOp::In => {
                for value in cypher_in_predicate_values(&predicate.predicate.value)? {
                    if value == Value::Null
                        || order_bounds
                            .iter()
                            .any(|bound| compare_where_order_values(&value, bound).is_none())
                    {
                        return Ok(None);
                    }
                }
            }
            op if is_order_predicate_op(op) => {
                if order_bounds.iter().any(|bound| {
                    compare_where_order_values(&predicate.predicate.value, bound).is_none()
                }) {
                    return Ok(None);
                }
            }
            _ => return Ok(None),
        }
    }

    let mut predicates = parsed
        .into_iter()
        .map(|predicate| ParsedWherePredicate {
            target: predicate.target,
            predicate: GraphPropertyPredicate {
                key: predicate.predicate.key,
                op: inverted_graph_predicate_op(predicate.predicate.op),
                value: predicate.predicate.value,
            },
        })
        .collect::<Vec<_>>();
    canonicalize_where_predicates(&mut predicates)?;
    Ok(Some(predicates))
}

pub(crate) fn where_boolean_contains_and(predicate: &CypherWhereBoolean<'_>) -> bool {
    match predicate {
        CypherWhereBoolean::And(_) => true,
        CypherWhereBoolean::Not(inner) => where_boolean_contains_and(inner),
        CypherWhereBoolean::Or(terms) => terms.iter().any(where_boolean_contains_and),
        CypherWhereBoolean::Predicate(_) => false,
    }
}

pub(crate) fn parse_where_or_fold_ast_terms(
    terms: &[CypherWhereBoolean<'_>],
    parameters: &CypherParameters,
    op: GraphPredicateOp,
) -> Result<ParsedWherePredicate> {
    let mut parsed = Vec::new();
    for term in terms {
        collect_where_or_fold_ast_terms(term, parameters, &mut parsed)?;
    }
    parse_where_or_fold_parsed_terms(parsed, op)
}

pub(crate) fn collect_where_or_fold_ast_terms(
    predicate: &CypherWhereBoolean<'_>,
    parameters: &CypherParameters,
    parsed: &mut Vec<ParsedWherePredicate>,
) -> Result<()> {
    match predicate {
        CypherWhereBoolean::Or(terms) => {
            for term in terms {
                collect_where_or_fold_ast_terms(term, parameters, parsed)?;
            }
            Ok(())
        }
        _ => {
            parsed.push(parse_where_single_predicate_ast(predicate, parameters)?);
            Ok(())
        }
    }
}

pub(crate) fn parse_where_single_predicate_ast(
    predicate: &CypherWhereBoolean<'_>,
    parameters: &CypherParameters,
) -> Result<ParsedWherePredicate> {
    match predicate {
        CypherWhereBoolean::Predicate(predicate) => parse_where_predicate(predicate, parameters),
        CypherWhereBoolean::Not(_) | CypherWhereBoolean::And(_) | CypherWhereBoolean::Or(_) => Err(
            cypher_syntax("MATCH WHERE OR only supports bounded predicate terms"),
        ),
    }
}

pub(crate) fn parse_where_or_fold_parsed_terms(
    parsed: Vec<ParsedWherePredicate>,
    op: GraphPredicateOp,
) -> Result<ParsedWherePredicate> {
    let first = parsed
        .first()
        .ok_or_else(|| cypher_syntax("MATCH WHERE OR requires at least one predicate"))?;
    if matches!(
        first.predicate.op,
        GraphPredicateOp::Equal | GraphPredicateOp::In
    ) {
        return parse_where_or_membership_fold_terms(parsed, op);
    }
    if matches!(
        first.predicate.op,
        GraphPredicateOp::StartsWith
            | GraphPredicateOp::StartsWithAny
            | GraphPredicateOp::EndsWith
            | GraphPredicateOp::EndsWithAny
            | GraphPredicateOp::Contains
            | GraphPredicateOp::ContainsAny
    ) {
        return parse_where_or_string_fold_terms(parsed, op);
    }
    Err(cypher_syntax(
        "MATCH WHERE OR only supports same-property equality, membership, or matching string predicate disjunctions",
    ))
}

pub(crate) fn lower_where_or_branches(
    branches: Vec<Vec<ParsedWherePredicate>>,
) -> Result<Vec<ParsedWherePredicate>> {
    let mut no_match_predicate = None;
    let mut branches = branches
        .into_iter()
        .filter(|branch| {
            if let Some(predicate) = branch.iter().find(|predicate| is_where_no_match(predicate)) {
                no_match_predicate.get_or_insert_with(|| predicate.clone());
                false
            } else {
                true
            }
        })
        .collect::<Vec<_>>();
    prune_subsumed_where_or_branches(&mut branches)?;

    if branches.len() == 1 {
        return Ok(branches.into_iter().next().expect("single branch"));
    }
    if branches.is_empty() {
        return Ok(vec![no_match_predicate.ok_or_else(|| {
            cypher_syntax("MATCH WHERE OR requires at least one predicate")
        })?]);
    }

    let first = branches
        .first()
        .ok_or_else(|| cypher_syntax("MATCH WHERE OR requires at least one predicate"))?;
    for varying_index in 0..first.len() {
        let mut common = first.clone();
        let varying = common.remove(varying_index);
        let mut fold_terms = vec![varying];
        let mut matched = true;
        for branch in branches.iter().skip(1) {
            let mut remaining = branch.clone();
            for common_predicate in &common {
                if let Some(index) = remaining
                    .iter()
                    .position(|predicate| predicate == common_predicate)
                {
                    remaining.remove(index);
                } else {
                    matched = false;
                    break;
                }
            }
            if !matched || remaining.len() != 1 {
                matched = false;
                break;
            }
            fold_terms.push(remaining.remove(0));
        }
        if matched {
            let folded = parse_where_or_fold_parsed_terms(fold_terms, GraphPredicateOp::In)?;
            common.push(folded);
            return Ok(common);
        }
    }

    Err(cypher_syntax(
        "MATCH WHERE OR of AND groups only supports identical bounded predicates plus one same-property foldable predicate",
    ))
}

pub(crate) fn prune_subsumed_where_or_branches(branches: &mut Vec<Vec<ParsedWherePredicate>>) -> Result<()> {
    let mut keep = Vec::with_capacity(branches.len());
    for index in 0..branches.len() {
        let mut redundant = false;
        for (candidate_index, candidate) in branches.iter().enumerate() {
            if candidate_index != index && where_branch_implies_all(&branches[index], candidate)? {
                let candidate_implies_branch =
                    where_branch_implies_all(candidate, &branches[index])?;
                let candidate_preferred =
                    where_branch_preferred_for_subsumption(candidate, &branches[index])?;
                let branch_preferred =
                    where_branch_preferred_for_subsumption(&branches[index], candidate)?;
                if candidate.len() < branches[index].len()
                    || !candidate_implies_branch
                    || candidate_preferred
                    || (!branch_preferred && candidate_index < index)
                {
                    redundant = true;
                    break;
                }
            }
        }
        if !redundant {
            keep.push(branches[index].clone());
        }
    }
    *branches = keep;
    Ok(())
}

pub(crate) fn where_branch_implies_all(
    branch: &[ParsedWherePredicate],
    required: &[ParsedWherePredicate],
) -> Result<bool> {
    for required in required {
        let mut implied = false;
        for predicate in branch {
            if where_predicate_implies(predicate, required)? {
                implied = true;
                break;
            }
        }
        if !implied {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn where_branch_preferred_for_subsumption(
    candidate: &[ParsedWherePredicate],
    branch: &[ParsedWherePredicate],
) -> Result<bool> {
    for branch_predicate in branch {
        for candidate_predicate in candidate {
            if candidate_predicate.target != branch_predicate.target
                || candidate_predicate.predicate.key != branch_predicate.predicate.key
            {
                continue;
            }
            match (
                candidate_predicate.predicate.op,
                branch_predicate.predicate.op,
            ) {
                (GraphPredicateOp::Equal, GraphPredicateOp::In)
                    if membership_values_all_satisfy(
                        &branch_predicate.predicate.value,
                        |value| Ok(value == &candidate_predicate.predicate.value),
                    )? =>
                {
                    return Ok(true);
                }
                (GraphPredicateOp::NotIn, GraphPredicateOp::NotEqual)
                    if membership_values_all_satisfy(
                        &candidate_predicate.predicate.value,
                        |value| Ok(value == &branch_predicate.predicate.value),
                    )? =>
                {
                    return Ok(true);
                }
                _ => {}
            }
        }
    }
    Ok(false)
}

pub(crate) fn where_predicate_implies(
    predicate: &ParsedWherePredicate,
    required: &ParsedWherePredicate,
) -> Result<bool> {
    if predicate == required {
        return Ok(true);
    }
    if predicate.target != required.target || predicate.predicate.key != required.predicate.key {
        return Ok(false);
    }
    match (predicate.predicate.op, required.predicate.op) {
        (_, GraphPredicateOp::IsNull) => where_predicate_implies_is_null(predicate),
        (_, GraphPredicateOp::IsNotNull) => where_predicate_implies_is_not_null(predicate),
        (GraphPredicateOp::Equal, GraphPredicateOp::In) => {
            membership_contains_value(&required.predicate.value, &predicate.predicate.value)
        }
        (GraphPredicateOp::Equal, GraphPredicateOp::NotIn) => Ok(!membership_contains_value(
            &required.predicate.value,
            &predicate.predicate.value,
        )?),
        (GraphPredicateOp::Equal, GraphPredicateOp::NotEqual) => {
            Ok(predicate.predicate.value != required.predicate.value)
        }
        (GraphPredicateOp::Equal, op) if is_order_predicate_op(op) => {
            Ok(equality_satisfies_order_bound(
                &predicate.predicate.value,
                op,
                &required.predicate.value,
            ))
        }
        (GraphPredicateOp::In, GraphPredicateOp::Equal) => {
            membership_values_all_satisfy(&predicate.predicate.value, |value| {
                Ok(value == &required.predicate.value)
            })
        }
        (GraphPredicateOp::In, GraphPredicateOp::In) => {
            membership_values_all_satisfy(&predicate.predicate.value, |value| {
                membership_contains_value(&required.predicate.value, value)
            })
        }
        (GraphPredicateOp::In, GraphPredicateOp::NotIn) => {
            membership_values_all_satisfy(&predicate.predicate.value, |value| {
                Ok(!membership_contains_value(
                    &required.predicate.value,
                    value,
                )?)
            })
        }
        (GraphPredicateOp::In, GraphPredicateOp::NotEqual) => {
            membership_values_all_satisfy(&predicate.predicate.value, |value| {
                Ok(value != &required.predicate.value)
            })
        }
        (GraphPredicateOp::In, op) if is_order_predicate_op(op) => {
            membership_values_all_satisfy(&predicate.predicate.value, |value| {
                Ok(equality_satisfies_order_bound(
                    value,
                    op,
                    &required.predicate.value,
                ))
            })
        }
        (GraphPredicateOp::NotIn, GraphPredicateOp::NotIn) => {
            membership_values_all_satisfy(&required.predicate.value, |value| {
                membership_contains_value(&predicate.predicate.value, value)
            })
        }
        (GraphPredicateOp::NotIn, GraphPredicateOp::NotEqual) => {
            membership_contains_value(&predicate.predicate.value, &required.predicate.value)
        }
        (GraphPredicateOp::NotEqual, GraphPredicateOp::NotIn) => {
            membership_values_all_satisfy(&required.predicate.value, |value| {
                Ok(value == &predicate.predicate.value)
            })
        }
        (op, GraphPredicateOp::NotEqual) if is_order_predicate_op(op) => Ok(
            order_bound_excludes_value(op, &predicate.predicate.value, &required.predicate.value),
        ),
        (op, GraphPredicateOp::NotIn) if is_order_predicate_op(op) => {
            membership_values_all_satisfy(&required.predicate.value, |value| {
                Ok(order_bound_excludes_value(
                    op,
                    &predicate.predicate.value,
                    value,
                ))
            })
        }
        (GraphPredicateOp::StartsWith, GraphPredicateOp::StartsWithAny)
        | (GraphPredicateOp::EndsWith, GraphPredicateOp::EndsWithAny)
        | (GraphPredicateOp::Contains, GraphPredicateOp::ContainsAny) => {
            string_group_contains_value(&required.predicate.value, &predicate.predicate.value)
        }
        (GraphPredicateOp::StartsWithAny, GraphPredicateOp::StartsWithAny)
        | (GraphPredicateOp::EndsWithAny, GraphPredicateOp::EndsWithAny)
        | (GraphPredicateOp::ContainsAny, GraphPredicateOp::ContainsAny) => {
            string_group_values_all_satisfy(&predicate.predicate.value, |value| {
                string_group_contains_value(&required.predicate.value, value)
            })
        }
        (GraphPredicateOp::NotStartsWithAny, GraphPredicateOp::NotStartsWith)
        | (GraphPredicateOp::NotEndsWithAny, GraphPredicateOp::NotEndsWith)
        | (GraphPredicateOp::NotContainsAny, GraphPredicateOp::NotContains) => {
            string_group_contains_value(&predicate.predicate.value, &required.predicate.value)
        }
        (GraphPredicateOp::NotStartsWith, GraphPredicateOp::NotStartsWithAny)
        | (GraphPredicateOp::NotEndsWith, GraphPredicateOp::NotEndsWithAny)
        | (GraphPredicateOp::NotContains, GraphPredicateOp::NotContainsAny) => {
            string_group_values_all_satisfy(&required.predicate.value, |value| {
                Ok(value == &predicate.predicate.value)
            })
        }
        (GraphPredicateOp::NotStartsWithAny, GraphPredicateOp::NotStartsWithAny)
        | (GraphPredicateOp::NotEndsWithAny, GraphPredicateOp::NotEndsWithAny)
        | (GraphPredicateOp::NotContainsAny, GraphPredicateOp::NotContainsAny) => {
            string_group_values_all_satisfy(&required.predicate.value, |value| {
                string_group_contains_value(&predicate.predicate.value, value)
            })
        }
        (op, required_op) if is_order_predicate_op(op) && is_order_predicate_op(required_op) => {
            Ok(order_predicate_implies_order_bound(
                op,
                &predicate.predicate.value,
                required_op,
                &required.predicate.value,
            ))
        }
        _ => Ok(false),
    }
}

pub(crate) fn where_predicate_implies_is_null(predicate: &ParsedWherePredicate) -> Result<bool> {
    match predicate.predicate.op {
        GraphPredicateOp::Equal => Ok(predicate.predicate.value == Value::Null),
        GraphPredicateOp::In => {
            membership_values_all_satisfy(&predicate.predicate.value, |value| {
                Ok(value == &Value::Null)
            })
        }
        _ => Ok(false),
    }
}

pub(crate) fn where_predicate_implies_is_not_null(predicate: &ParsedWherePredicate) -> Result<bool> {
    match predicate.predicate.op {
        GraphPredicateOp::Equal => Ok(predicate.predicate.value != Value::Null),
        GraphPredicateOp::NotEqual => Ok(predicate.predicate.value == Value::Null),
        GraphPredicateOp::In => {
            membership_values_all_satisfy(&predicate.predicate.value, |value| {
                Ok(value != &Value::Null)
            })
        }
        GraphPredicateOp::NotIn => {
            membership_contains_value(&predicate.predicate.value, &Value::Null)
        }
        GraphPredicateOp::StartsWith
        | GraphPredicateOp::NotStartsWith
        | GraphPredicateOp::StartsWithAny
        | GraphPredicateOp::NotStartsWithAny
        | GraphPredicateOp::EndsWith
        | GraphPredicateOp::NotEndsWith
        | GraphPredicateOp::EndsWithAny
        | GraphPredicateOp::NotEndsWithAny
        | GraphPredicateOp::Contains
        | GraphPredicateOp::NotContains
        | GraphPredicateOp::ContainsAny
        | GraphPredicateOp::NotContainsAny
        | GraphPredicateOp::GreaterThan
        | GraphPredicateOp::GreaterThanOrEqual
        | GraphPredicateOp::LessThan
        | GraphPredicateOp::LessThanOrEqual => Ok(true),
        _ => Ok(false),
    }
}

pub(crate) fn string_group_contains_value(group: &Value, value: &Value) -> Result<bool> {
    let Value::String(needle) = value else {
        return Ok(false);
    };
    Ok(match group {
        Value::StringArray(values) => values.iter().any(|value| value == needle),
        Value::Json(json) => json
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(needle))),
        _ => false,
    })
}

pub(crate) fn string_group_values_all_satisfy(
    group: &Value,
    mut predicate: impl FnMut(&Value) -> Result<bool>,
) -> Result<bool> {
    match group {
        Value::StringArray(values) => {
            for value in values {
                if !predicate(&Value::from(value.as_str()))? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Value::Json(json) => {
            let Some(values) = json.as_array() else {
                return Ok(false);
            };
            for value in values {
                let Some(value) = value.as_str() else {
                    return Ok(false);
                };
                if !predicate(&Value::from(value))? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub(crate) fn order_predicate_implies_order_bound(
    op: GraphPredicateOp,
    value: &Value,
    required_op: GraphPredicateOp,
    required_value: &Value,
) -> bool {
    match (order_bound_kind(op), order_bound_kind(required_op)) {
        (Some(OrderBoundKind::Lower), Some(OrderBoundKind::Lower)) => {
            order_lower_is_stricter(op, value, required_op, required_value)
        }
        (Some(OrderBoundKind::Upper), Some(OrderBoundKind::Upper)) => {
            order_upper_is_stricter(op, value, required_op, required_value)
        }
        _ => false,
    }
}

pub(crate) fn membership_values_all_satisfy(
    membership: &Value,
    mut predicate: impl FnMut(&Value) -> Result<bool>,
) -> Result<bool> {
    for value in cypher_in_predicate_values(membership)? {
        if !predicate(&value)? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn parse_where_or_membership_fold_terms(
    mut parsed: Vec<ParsedWherePredicate>,
    op: GraphPredicateOp,
) -> Result<ParsedWherePredicate> {
    let first = parsed
        .first()
        .ok_or_else(|| cypher_syntax("MATCH WHERE OR requires at least one predicate"))?;
    if !matches!(
        first.predicate.op,
        GraphPredicateOp::Equal | GraphPredicateOp::In
    ) {
        return Err(cypher_syntax(
            "MATCH WHERE OR only supports same-property equality or membership disjunctions",
        ));
    }
    let target = first.target.clone();
    let key = first.predicate.key.clone();
    let mut values = Vec::with_capacity(parsed.len());
    for predicate in parsed.drain(..) {
        if predicate.target != target || predicate.predicate.key != key {
            return Err(cypher_syntax(
                "MATCH WHERE OR only supports same-property equality or membership disjunctions",
            ));
        }
        match predicate.predicate.op {
            GraphPredicateOp::Equal => {
                validate_cypher_in_item(&predicate.predicate.value)?;
                push_unique(&mut values, predicate.predicate.value.to_json());
            }
            GraphPredicateOp::In => {
                for value in cypher_in_predicate_values(&predicate.predicate.value)? {
                    push_unique(&mut values, value.to_json());
                }
            }
            _ => {
                return Err(cypher_syntax(
                    "MATCH WHERE OR only supports same-property equality or membership disjunctions",
                ));
            }
        }
    }
    Ok(ParsedWherePredicate {
        target,
        predicate: GraphPropertyPredicate {
            key,
            op,
            value: Value::Json(serde_json::Value::Array(values)),
        },
    })
}

pub(crate) fn parse_where_or_string_fold_terms(
    mut parsed: Vec<ParsedWherePredicate>,
    op: GraphPredicateOp,
) -> Result<ParsedWherePredicate> {
    let first = parsed
        .first()
        .ok_or_else(|| cypher_syntax("MATCH WHERE OR requires at least one predicate"))?;
    let target = first.target.clone();
    let key = first.predicate.key.clone();
    let Some(string_op) = string_fold_base_op(first.predicate.op) else {
        return Err(cypher_syntax(
            "MATCH WHERE OR only supports matching string predicate disjunctions",
        ));
    };
    let folded_op = match (op, string_op) {
        (GraphPredicateOp::In, GraphPredicateOp::StartsWith) => GraphPredicateOp::StartsWithAny,
        (GraphPredicateOp::NotIn, GraphPredicateOp::StartsWith) => {
            GraphPredicateOp::NotStartsWithAny
        }
        (GraphPredicateOp::In, GraphPredicateOp::EndsWith) => GraphPredicateOp::EndsWithAny,
        (GraphPredicateOp::NotIn, GraphPredicateOp::EndsWith) => GraphPredicateOp::NotEndsWithAny,
        (GraphPredicateOp::In, GraphPredicateOp::Contains) => GraphPredicateOp::ContainsAny,
        (GraphPredicateOp::NotIn, GraphPredicateOp::Contains) => GraphPredicateOp::NotContainsAny,
        _ => {
            return Err(cypher_syntax(
                "MATCH WHERE OR only supports matching string predicate disjunctions",
            ));
        }
    };
    let mut needles = Vec::with_capacity(parsed.len());
    for predicate in parsed.drain(..) {
        if predicate.target != target
            || predicate.predicate.key != key
            || string_fold_base_op(predicate.predicate.op) != Some(string_op)
        {
            return Err(cypher_syntax(
                "MATCH WHERE OR only supports matching same-property string predicate disjunctions",
            ));
        }
        match predicate.predicate.op {
            GraphPredicateOp::StartsWith
            | GraphPredicateOp::NotStartsWith
            | GraphPredicateOp::EndsWith
            | GraphPredicateOp::NotEndsWith
            | GraphPredicateOp::Contains => {
                let Some(needle) = predicate.predicate.value.as_str() else {
                    return Err(cypher_syntax(
                        "MATCH WHERE string predicates require string literals or parameters",
                    ));
                };
                push_unique(&mut needles, needle.to_string());
            }
            GraphPredicateOp::NotContains => {
                let Some(needle) = predicate.predicate.value.as_str() else {
                    return Err(cypher_syntax(
                        "MATCH WHERE string predicates require string literals or parameters",
                    ));
                };
                push_unique(&mut needles, needle.to_string());
            }
            GraphPredicateOp::StartsWithAny
            | GraphPredicateOp::NotStartsWithAny
            | GraphPredicateOp::EndsWithAny
            | GraphPredicateOp::NotEndsWithAny
            | GraphPredicateOp::ContainsAny
            | GraphPredicateOp::NotContainsAny => {
                let Value::StringArray(values) = &predicate.predicate.value else {
                    return Err(cypher_syntax(
                        "MATCH WHERE grouped string predicates require string list values",
                    ));
                };
                for needle in values {
                    push_unique(&mut needles, needle.clone());
                }
            }
            _ => {
                return Err(cypher_syntax(
                    "MATCH WHERE OR only supports matching string predicate disjunctions",
                ));
            }
        }
    }
    Ok(ParsedWherePredicate {
        target,
        predicate: GraphPropertyPredicate {
            key,
            op: folded_op,
            value: Value::from(needles),
        },
    })
}

pub(crate) fn string_fold_base_op(op: GraphPredicateOp) -> Option<GraphPredicateOp> {
    match op {
        GraphPredicateOp::StartsWith
        | GraphPredicateOp::StartsWithAny
        | GraphPredicateOp::NotStartsWith
        | GraphPredicateOp::NotStartsWithAny => Some(GraphPredicateOp::StartsWith),
        GraphPredicateOp::EndsWith
        | GraphPredicateOp::EndsWithAny
        | GraphPredicateOp::NotEndsWith
        | GraphPredicateOp::NotEndsWithAny => Some(GraphPredicateOp::EndsWith),
        GraphPredicateOp::Contains
        | GraphPredicateOp::ContainsAny
        | GraphPredicateOp::NotContains
        | GraphPredicateOp::NotContainsAny => Some(GraphPredicateOp::Contains),
        _ => None,
    }
}

pub(crate) fn push_unique<T: PartialEq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

pub(crate) fn parse_where_predicate(
    predicate: &str,
    parameters: &CypherParameters,
) -> Result<ParsedWherePredicate> {
    let predicate = strip_enclosing_parentheses(predicate.trim())?;
    let (predicate, negated) = if let Some(after_not) = strip_leading_keyword(predicate, "NOT") {
        let predicate = strip_enclosing_parentheses(after_not.trim())?;
        if predicate.is_empty() || strip_leading_keyword(predicate, "NOT").is_some() {
            return Err(cypher_syntax(
                "MATCH WHERE NOT only supports a single property comparison",
            ));
        }
        (predicate, true)
    } else {
        (predicate, false)
    };
    if let Some(index) = find_unquoted_keyword(predicate, "IS") {
        let rest = predicate[index + "IS".len()..].trim();
        let words = rest.split_whitespace().collect::<Vec<_>>();
        let op = if words.len() == 2
            && words[0].eq_ignore_ascii_case("NOT")
            && words[1].eq_ignore_ascii_case("NULL")
        {
            GraphPredicateOp::IsNotNull
        } else if words.len() == 1 && words[0].eq_ignore_ascii_case("NULL") {
            GraphPredicateOp::IsNull
        } else {
            return Err(cypher_syntax(
                "MATCH WHERE IS predicates only support IS NULL or IS NOT NULL",
            ));
        };
        let op = if negated {
            inverted_graph_predicate_op(op)
        } else {
            op
        };
        let (target, key) = parse_property_ref(&predicate[..index], "MATCH WHERE predicate")?;
        return Ok(ParsedWherePredicate {
            target,
            predicate: GraphPropertyPredicate {
                key,
                op,
                value: Value::Null,
            },
        });
    }
    for (keyword, op) in [
        ("STARTS WITH", GraphPredicateOp::StartsWith),
        ("ENDS WITH", GraphPredicateOp::EndsWith),
        ("CONTAINS", GraphPredicateOp::Contains),
    ] {
        if let Some(index) = find_top_level_keyword_sequence(predicate, keyword)? {
            let (target, key) =
                parse_property_ref(&predicate[..index], "MATCH WHERE string predicate")?;
            let value = parse_cypher_literal(&predicate[index + keyword.len()..], parameters)?;
            if !matches!(value, Value::String(_)) {
                return Err(cypher_syntax(
                    "MATCH WHERE string predicates require string literals or parameters",
                ));
            }
            let op = if negated {
                inverted_graph_predicate_op(op)
            } else {
                op
            };
            return Ok(ParsedWherePredicate {
                target,
                predicate: GraphPropertyPredicate { key, op, value },
            });
        }
    }
    if let Some(index) = find_top_level_keyword_sequence(predicate, "IN")? {
        let (target, key) = parse_property_ref(&predicate[..index], "MATCH WHERE IN predicate")?;
        let value = parse_cypher_in_values(&predicate[index + "IN".len()..], parameters)?;
        let op = if negated {
            GraphPredicateOp::NotIn
        } else {
            GraphPredicateOp::In
        };
        return Ok(ParsedWherePredicate {
            target,
            predicate: GraphPropertyPredicate { key, op, value },
        });
    }
    for (token, op) in [
        (">=", GraphPredicateOp::GreaterThanOrEqual),
        ("<=", GraphPredicateOp::LessThanOrEqual),
        ("<>", GraphPredicateOp::NotEqual),
        ("!=", GraphPredicateOp::NotEqual),
        ("=", GraphPredicateOp::Equal),
        (">", GraphPredicateOp::GreaterThan),
        ("<", GraphPredicateOp::LessThan),
    ] {
        let index = if token.len() == 1 {
            find_unquoted(predicate, token.chars().next().expect("operator"))
        } else {
            find_unquoted_sequence(predicate, token)
        };
        if let Some(index) = index {
            let (target, key) = parse_property_ref(&predicate[..index], "MATCH WHERE predicate")?;
            let value = parse_cypher_literal(&predicate[index + token.len()..], parameters)?;
            let op = if negated {
                inverted_graph_predicate_op(op)
            } else {
                op
            };
            if matches!(
                op,
                GraphPredicateOp::GreaterThan
                    | GraphPredicateOp::GreaterThanOrEqual
                    | GraphPredicateOp::LessThan
                    | GraphPredicateOp::LessThanOrEqual
            ) && !matches!(value, Value::Int(_) | Value::Float(_) | Value::String(_))
            {
                return Err(cypher_syntax(
                    "MATCH WHERE ordered comparisons require integer, float, or string literals",
                ));
            }
            return Ok(ParsedWherePredicate {
                target,
                predicate: GraphPropertyPredicate { key, op, value },
            });
        }
    }
    Err(cypher_syntax(
        "MATCH WHERE only supports property comparisons against literals or parameters",
    ))
}

pub(crate) fn inverted_graph_predicate_op(op: GraphPredicateOp) -> GraphPredicateOp {
    match op {
        GraphPredicateOp::Equal => GraphPredicateOp::NotEqual,
        GraphPredicateOp::NotEqual => GraphPredicateOp::Equal,
        GraphPredicateOp::IsNull => GraphPredicateOp::IsNotNull,
        GraphPredicateOp::IsNotNull => GraphPredicateOp::IsNull,
        GraphPredicateOp::StartsWith => GraphPredicateOp::NotStartsWith,
        GraphPredicateOp::NotStartsWith => GraphPredicateOp::StartsWith,
        GraphPredicateOp::StartsWithAny => GraphPredicateOp::NotStartsWithAny,
        GraphPredicateOp::NotStartsWithAny => GraphPredicateOp::StartsWithAny,
        GraphPredicateOp::EndsWith => GraphPredicateOp::NotEndsWith,
        GraphPredicateOp::NotEndsWith => GraphPredicateOp::EndsWith,
        GraphPredicateOp::EndsWithAny => GraphPredicateOp::NotEndsWithAny,
        GraphPredicateOp::NotEndsWithAny => GraphPredicateOp::EndsWithAny,
        GraphPredicateOp::Contains => GraphPredicateOp::NotContains,
        GraphPredicateOp::NotContains => GraphPredicateOp::Contains,
        GraphPredicateOp::ContainsAny => GraphPredicateOp::NotContainsAny,
        GraphPredicateOp::NotContainsAny => GraphPredicateOp::ContainsAny,
        GraphPredicateOp::In => GraphPredicateOp::NotIn,
        GraphPredicateOp::NotIn => GraphPredicateOp::In,
        GraphPredicateOp::GreaterThan => GraphPredicateOp::LessThanOrEqual,
        GraphPredicateOp::GreaterThanOrEqual => GraphPredicateOp::LessThan,
        GraphPredicateOp::LessThan => GraphPredicateOp::GreaterThanOrEqual,
        GraphPredicateOp::LessThanOrEqual => GraphPredicateOp::GreaterThan,
    }
}

pub(crate) fn apply_node_where_predicates(
    node: &mut ParsedCypherNode,
    predicates: Vec<ParsedWherePredicate>,
    context: &str,
) -> Result<()> {
    for predicate in predicates {
        if node.variable.as_deref() == Some(predicate.target.as_str()) {
            node.predicates.push(predicate.predicate);
        } else {
            return Err(cypher_unresolved_identity(format!(
                "{context} WHERE references unknown variable '{}'",
                predicate.target
            )));
        }
    }
    Ok(())
}

pub(crate) fn apply_match_where_predicates(
    nodes: &mut BTreeMap<String, ParsedCypherNode>,
    predicates: Vec<ParsedWherePredicate>,
    context: &str,
) -> Result<()> {
    for predicate in predicates {
        let Some(node) = nodes.get_mut(&predicate.target) else {
            return Err(cypher_unresolved_identity(format!(
                "{context} WHERE references unknown variable '{}'",
                predicate.target
            )));
        };
        node.predicates.push(predicate.predicate);
    }
    Ok(())
}

pub(crate) fn apply_edge_where_predicates(
    edge: &mut ParsedCypherEdgeMatch,
    predicates: Vec<ParsedWherePredicate>,
    context: &str,
) -> Result<()> {
    for predicate in predicates {
        if edge.from.variable.as_deref() == Some(predicate.target.as_str()) {
            edge.from.predicates.push(predicate.predicate);
        } else if edge.to.variable.as_deref() == Some(predicate.target.as_str()) {
            edge.to.predicates.push(predicate.predicate);
        } else if edge.relationship.variable.as_deref() == Some(predicate.target.as_str()) {
            edge.relationship.predicates.push(predicate.predicate);
        } else {
            return Err(cypher_unresolved_identity(format!(
                "{context} WHERE references unknown variable '{}'",
                predicate.target
            )));
        }
    }
    Ok(())
}

