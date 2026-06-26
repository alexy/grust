//! Low-level parsing primitives: numeric expressions, literals/property maps, and unquoted text scanning (extracted from lib.rs).

use crate::*;

pub(crate) struct NumericExpression {
    pub(crate) source_target: String,
    pub(crate) source_key: String,
    pub(crate) op: GraphNumericOp,
    pub(crate) operand: Value,
}

pub(crate) fn parse_numeric_expression(
    expression: &str,
    parameters: &CypherParameters,
) -> Result<Option<NumericExpression>> {
    for (index, op) in find_numeric_operator_candidates(expression) {
        let lhs = expression[..index].trim();
        let rhs = expression[index + 1..].trim();
        let Ok((source_target, source_key)) = parse_property_ref(lhs, "MATCH SET expression")
        else {
            continue;
        };
        let operand = parse_cypher_literal(rhs, parameters)?;
        if !matches!(operand, Value::Int(_) | Value::Float(_)) {
            return Err(cypher_syntax(
                "MATCH SET numeric expression operand must be an integer or float",
            ));
        }
        return Ok(Some(NumericExpression {
            source_target,
            source_key,
            op,
            operand,
        }));
    }
    Ok(None)
}

pub(crate) fn find_numeric_operator_candidates(expression: &str) -> Vec<(usize, GraphNumericOp)> {
    let mut candidates = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in expression.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '+' => candidates.push((index, GraphNumericOp::Add)),
            '-' if index > 0 => candidates.push((index, GraphNumericOp::Subtract)),
            '*' => candidates.push((index, GraphNumericOp::Multiply)),
            '/' => candidates.push((index, GraphNumericOp::Divide)),
            _ => {}
        }
    }
    candidates
}

pub(crate) fn parse_property_ref(value: &str, context: &str) -> Result<(String, String)> {
    let value = value.trim();
    let Some(index) = find_unquoted(value, '.') else {
        return Err(cypher_syntax(format!(
            "{context} requires property syntax target.key"
        )));
    };
    let target = parse_required_cypher_variable(&value[..index], context)?;
    let key = parse_cypher_prop_key(&value[index + 1..])?;
    Ok((target, key))
}

pub(crate) fn parse_cypher_props_map_literal(
    value: &str,
    parameters: &CypherParameters,
) -> Result<Props> {
    let value = value.trim();
    let Some(body) = value.strip_prefix('{') else {
        return Err(GrustError::Unsupported(
            "MATCH SET += requires a Cypher property map".to_string(),
        ));
    };
    let close = find_matching(body, '{', '}')?;
    if !body[close + 1..].trim().is_empty() {
        return Err(GrustError::Unsupported(
            "unsupported content after MATCH SET property map".to_string(),
        ));
    }
    parse_cypher_props(&body[..close], parameters)
}

pub(crate) fn split_cypher_body_props<'a>(
    body: &'a str,
    parameters: &CypherParameters,
) -> Result<(&'a str, Props)> {
    let body = body.trim();
    if let Some(open) = body.find('{') {
        let close = find_matching(&body[open + 1..], '{', '}')? + open + 1;
        if !body[close + 1..].trim().is_empty() {
            return Err(GrustError::Unsupported(
                "unsupported content after Cypher property map".to_string(),
            ));
        }
        Ok((
            &body[..open],
            parse_cypher_props(&body[open + 1..close], parameters)?,
        ))
    } else {
        Ok((body, Props::new()))
    }
}

pub(crate) fn parse_cypher_props(body: &str, parameters: &CypherParameters) -> Result<Props> {
    let mut props = Props::new();
    for entry in split_top_level_commas(body)? {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let colon = find_unquoted(entry, ':').ok_or_else(|| {
            GrustError::Unsupported(format!("Cypher property entry is missing ':': {entry}"))
        })?;
        let key = parse_cypher_prop_key(&entry[..colon])?;
        let value = parse_cypher_literal(&entry[colon + 1..], parameters)?;
        props.insert(key, value);
    }
    Ok(props)
}

pub(crate) fn parse_cypher_prop_key(key: &str) -> Result<String> {
    let key = key.trim();
    if key.is_empty() {
        return Err(GrustError::Unsupported(
            "Cypher property key cannot be empty".to_string(),
        ));
    }
    if is_quoted(key) {
        parse_cypher_string(key)
    } else if key
        .chars()
        .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        Ok(key.to_string())
    } else {
        Err(GrustError::Unsupported(format!(
            "unsupported Cypher property key: {key}"
        )))
    }
}

pub(crate) fn parse_cypher_literal(value: &str, parameters: &CypherParameters) -> Result<Value> {
    let value = value.trim();
    if value.is_empty() {
        return Err(GrustError::Unsupported(
            "Cypher property value cannot be empty".to_string(),
        ));
    }
    if is_quoted(value) {
        return Ok(Value::String(parse_cypher_string(value)?));
    }
    if let Some(parameter) = value.strip_prefix('$') {
        if !is_cypher_identifier(parameter) {
            return Err(cypher_syntax(format!(
                "unsupported Cypher parameter reference: {value}"
            )));
        }
        return parameters.get(parameter).cloned().ok_or_else(|| {
            cypher_unresolved_identity(format!("Cypher parameter '{value}' was not provided"))
        });
    }
    match value {
        "true" | "TRUE" => return Ok(Value::Bool(true)),
        "false" | "FALSE" => return Ok(Value::Bool(false)),
        "null" | "NULL" => return Ok(Value::Null),
        _ => {}
    }
    if value.contains('.') {
        return value.parse::<f64>().map(Value::Float).map_err(|_| {
            GrustError::Unsupported(format!("unsupported Cypher literal value: {value}"))
        });
    }
    value
        .parse::<i64>()
        .map(Value::Int)
        .map_err(|_| GrustError::Unsupported(format!("unsupported Cypher literal value: {value}")))
}

pub(crate) fn parse_cypher_in_values(value: &str, parameters: &CypherParameters) -> Result<Value> {
    let value = value.trim();
    if let Some(parameter) = value.strip_prefix('$') {
        let parsed = parse_cypher_literal(value, parameters)?;
        validate_cypher_in_values(&parsed)?;
        if !is_cypher_identifier(parameter) {
            return Err(cypher_syntax(format!(
                "unsupported Cypher parameter reference: {value}"
            )));
        }
        return Ok(parsed);
    }
    if !(value.starts_with('[') && value.ends_with(']')) {
        return Err(cypher_syntax(
            "MATCH WHERE IN predicates require a list literal or list parameter",
        ));
    }
    let inner = &value[1..value.len() - 1];
    let mut values = Vec::new();
    if !inner.trim().is_empty() {
        for item in split_top_level_commas(inner)? {
            let item = parse_cypher_literal(item, parameters)?;
            validate_cypher_in_item(&item)?;
            values.push(item.to_json());
        }
    }
    Ok(Value::Json(serde_json::Value::Array(values)))
}

pub(crate) fn validate_cypher_in_values(value: &Value) -> Result<()> {
    match value {
        Value::StringArray(_) | Value::IntArray(_) | Value::FloatArray(_) => Ok(()),
        Value::Json(serde_json::Value::Array(values)) => {
            for value in values {
                match value {
                    serde_json::Value::Bool(_)
                    | serde_json::Value::Number(_)
                    | serde_json::Value::String(_) => {}
                    serde_json::Value::Null
                    | serde_json::Value::Array(_)
                    | serde_json::Value::Object(_) => {
                        return Err(cypher_syntax(
                            "MATCH WHERE IN predicates only support scalar string, integer, float, or boolean list items",
                        ));
                    }
                }
            }
            Ok(())
        }
        _ => Err(cypher_syntax(
            "MATCH WHERE IN predicates require a list literal or list parameter",
        )),
    }
}

pub(crate) fn validate_cypher_in_item(value: &Value) -> Result<()> {
    match value {
        Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::String(_) => Ok(()),
        Value::Null
        | Value::DateTime(_)
        | Value::Decimal(_)
        | Value::Duration(_)
        | Value::StringArray(_)
        | Value::IntArray(_)
        | Value::FloatArray(_)
        | Value::Json(_) => Err(cypher_syntax(
            "MATCH WHERE IN predicates only support scalar string, integer, float, or boolean list items",
        )),
    }
}

pub(crate) fn parse_cypher_string(value: &str) -> Result<String> {
    let value = value.trim();
    if !is_quoted(value) {
        return Err(GrustError::Unsupported(format!(
            "expected quoted Cypher string literal: {value}"
        )));
    }
    let quote = value.as_bytes()[0] as char;
    let inner = &value[1..value.len() - 1];
    let mut output = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let escaped = chars.next().ok_or_else(|| {
                GrustError::Unsupported("unterminated Cypher string escape".to_string())
            })?;
            output.push(match escaped {
                '\\' => '\\',
                '\'' if quote == '\'' => '\'',
                '"' if quote == '"' => '"',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
        } else {
            output.push(ch);
        }
    }
    Ok(output)
}

pub(crate) fn optional_string_prop(props: &Props, key: &str) -> Option<String> {
    props.get(key).and_then(Value::as_str).map(str::to_string)
}

pub(crate) fn has_relationship_predicates_beyond_id(props: &Props) -> bool {
    props.keys().any(|key| key.as_str() != "id")
}

pub(crate) fn split_top_level_commas(value: &str) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;

    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => paren_depth += 1,
            ')' => {
                paren_depth = paren_depth.checked_sub(1).ok_or_else(|| {
                    cypher_syntax("unmatched ')' in Cypher expression".to_string())
                })?;
            }
            '[' => bracket_depth += 1,
            ']' => {
                bracket_depth = bracket_depth.checked_sub(1).ok_or_else(|| {
                    cypher_syntax("unmatched ']' in Cypher expression".to_string())
                })?;
            }
            '{' => brace_depth += 1,
            '}' => {
                brace_depth = brace_depth.checked_sub(1).ok_or_else(|| {
                    cypher_syntax("unmatched '}' in Cypher expression".to_string())
                })?;
            }
            ',' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => {
                parts.push(&value[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if quote.is_some() {
        return Err(GrustError::Unsupported(
            "unterminated Cypher string literal".to_string(),
        ));
    }
    if paren_depth != 0 || bracket_depth != 0 || brace_depth != 0 {
        return Err(cypher_syntax("unclosed grouping in Cypher expression"));
    }
    parts.push(&value[start..]);
    Ok(parts)
}

pub(crate) fn find_top_level_keyword(value: &str, keyword: &str) -> Result<Option<usize>> {
    find_top_level_keyword_sequence(value, keyword)
}

pub(crate) fn find_top_level_keyword_sequence(value: &str, keyword: &str) -> Result<Option<usize>> {
    let mut quote = None;
    let mut escaped = false;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;

    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => paren_depth += 1,
            ')' => {
                paren_depth = paren_depth.checked_sub(1).ok_or_else(|| {
                    cypher_syntax("unmatched ')' in Cypher expression".to_string())
                })?;
            }
            '[' => bracket_depth += 1,
            ']' => {
                bracket_depth = bracket_depth.checked_sub(1).ok_or_else(|| {
                    cypher_syntax("unmatched ']' in Cypher expression".to_string())
                })?;
            }
            '{' => brace_depth += 1,
            '}' => {
                brace_depth = brace_depth.checked_sub(1).ok_or_else(|| {
                    cypher_syntax("unmatched '}' in Cypher expression".to_string())
                })?;
            }
            _ if paren_depth == 0
                && bracket_depth == 0
                && brace_depth == 0
                && value[index..]
                    .get(..keyword.len())
                    .is_some_and(|candidate| candidate.eq_ignore_ascii_case(keyword))
                && keyword_boundary(value[..index].chars().next_back())
                && keyword_boundary(value[index + keyword.len()..].chars().next()) =>
            {
                return Ok(Some(index));
            }
            _ => {}
        }
    }
    if quote.is_some() {
        return Err(GrustError::Unsupported(
            "unterminated Cypher string literal".to_string(),
        ));
    }
    if paren_depth != 0 || bracket_depth != 0 || brace_depth != 0 {
        return Err(cypher_syntax("unclosed grouping in Cypher expression"));
    }
    Ok(None)
}

pub(crate) fn strip_enclosing_parentheses(value: &str) -> Result<&str> {
    let mut value = value.trim();
    loop {
        let Some(after_open) = value.strip_prefix('(') else {
            return Ok(value);
        };
        if !value.ends_with(')') {
            return Ok(value);
        }
        let mut quote = None;
        let mut escaped = false;
        let mut paren_depth = 0usize;
        let mut closes_at_end = false;
        for (index, ch) in value.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' && quote.is_some() {
                escaped = true;
                continue;
            }
            if let Some(active_quote) = quote {
                if ch == active_quote {
                    quote = None;
                }
                continue;
            }
            match ch {
                '\'' | '"' => quote = Some(ch),
                '(' => paren_depth += 1,
                ')' => {
                    paren_depth = paren_depth.checked_sub(1).ok_or_else(|| {
                        cypher_syntax("unmatched ')' in Cypher expression".to_string())
                    })?;
                    if paren_depth == 0 {
                        closes_at_end = index + ch.len_utf8() == value.len();
                        if !closes_at_end {
                            return Ok(value);
                        }
                    }
                }
                _ => {}
            }
        }
        if quote.is_some() {
            return Err(GrustError::Unsupported(
                "unterminated Cypher string literal".to_string(),
            ));
        }
        if paren_depth != 0 {
            return Err(cypher_syntax("unclosed grouping in Cypher expression"));
        }
        if !closes_at_end {
            return Ok(value);
        }
        value = after_open[..after_open.len() - 1].trim();
    }
}

pub(crate) fn split_top_level_patterns(value: &str) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;

    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ',' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => {
                let part = value[start..index].trim();
                if !part.is_empty() {
                    parts.push(part);
                }
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if quote.is_some() {
        return Err(GrustError::Unsupported(
            "unterminated Cypher string literal".to_string(),
        ));
    }
    let part = value[start..].trim();
    if !part.is_empty() {
        parts.push(part);
    }
    Ok(parts)
}

pub(crate) fn find_matching(value: &str, _open: char, close: char) -> Result<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            ch if ch == close => return Ok(index),
            _ => {}
        }
    }
    Err(GrustError::Unsupported(format!(
        "Cypher pattern is missing '{close}'"
    )))
}

/// Scans `value` left to right, skipping single- and double-quoted spans (with
/// backslash escapes inside them), and returns the first unquoted byte offset
/// where `at_unquoted(index, rest)` returns true. `rest` is `&value[index..]`.
pub(crate) fn scan_unquoted(
    value: &str,
    mut at_unquoted: impl FnMut(usize, &str) -> bool,
) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if at_unquoted(index, &value[index..]) {
            return Some(index);
        }
    }
    None
}

pub(crate) fn find_unquoted(value: &str, target: char) -> Option<usize> {
    scan_unquoted(value, |_, rest| rest.starts_with(target))
}

pub(crate) fn find_unquoted_sequence(value: &str, target: &str) -> Option<usize> {
    scan_unquoted(value, |_, rest| rest.starts_with(target))
}

/// True if `pattern` contains a (directed) relationship arrow outside of string
/// literals — either outgoing `->` or incoming `<-` (Unit 10b/W2). Used to route
/// a writable pattern to edge handling rather than node handling.
pub(crate) fn is_cypher_edge_pattern(pattern: &str) -> bool {
    find_unquoted_sequence(pattern, "->").is_some()
        || find_unquoted_sequence(pattern, "<-").is_some()
}

pub(crate) fn find_unquoted_keyword(value: &str, keyword: &str) -> Option<usize> {
    scan_unquoted(value, |index, rest| {
        rest.get(..keyword.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(keyword))
            && keyword_boundary(value[..index].chars().next_back())
            && keyword_boundary(rest[keyword.len()..].chars().next())
    })
}

pub(crate) fn keyword_boundary(ch: Option<char>) -> bool {
    ch.is_none_or(char::is_whitespace)
}

pub(crate) fn strip_leading_keyword<'a>(value: &'a str, keyword: &str) -> Option<&'a str> {
    let candidate = value.get(..keyword.len())?;
    if !candidate.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &value[keyword.len()..];
    let first = rest.chars().next()?;
    if !first.is_whitespace() {
        return None;
    }
    Some(rest.trim_start())
}

pub(crate) fn is_quoted(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2
        && ((bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"'))
}

pub fn validate_json_key(value: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    if valid {
        Ok(())
    } else {
        Err(GrustError::Schema(format!(
            "invalid JSON property key '{value}'"
        )))
    }
}

pub fn cypher_in_predicate_values(value: &Value) -> Result<Vec<Value>> {
    match value {
        Value::StringArray(values) => Ok(values.iter().map(Value::from).collect()),
        Value::IntArray(values) => Ok(values.iter().copied().map(Value::Int).collect()),
        Value::FloatArray(values) => Ok(values.iter().copied().map(Value::Float).collect()),
        Value::Json(serde_json::Value::Array(values)) => values
            .iter()
            .map(|value: &serde_json::Value| match value {
                serde_json::Value::Bool(value) => Ok(Value::Bool(*value)),
                serde_json::Value::Number(value) => value
                    .as_i64()
                    .map(Value::Int)
                    .or_else(|| value.as_f64().map(Value::Float))
                    .ok_or_else(|| cypher_syntax("unsupported numeric value in MATCH WHERE IN")),
                serde_json::Value::String(value) => Ok(Value::from(value)),
                serde_json::Value::Null
                | serde_json::Value::Array(_)
                | serde_json::Value::Object(_) => Err(cypher_syntax(
                    "MATCH WHERE IN predicates only support scalar string, integer, float, or boolean list items",
                )),
            })
            .collect(),
        _ => Err(cypher_syntax(
            "MATCH WHERE IN predicates require a list literal or list parameter",
        )),
    }
}

pub fn strict_create_edge_conflicts(edge: &Edge, existing: &[Edge]) -> bool {
    existing.iter().any(|existing| {
        let same_explicit_id = edge
            .id
            .as_ref()
            .is_some_and(|id| existing.id.as_ref() == Some(id));
        let same_structural_identity =
            existing.from == edge.from && existing.to == edge.to && existing.label == edge.label;
        same_explicit_id || same_structural_identity
    })
}
