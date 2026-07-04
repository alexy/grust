//! Structured errors, statement splitting/comment stripping, and node/relationship/DDL pattern parsing (extracted from lib.rs).

use crate::*;

pub fn cypher_syntax(message: impl Into<String>) -> GrustError {
    GrustError::CypherSyntax(message.into())
}

pub fn cypher_unresolved_identity(message: impl Into<String>) -> GrustError {
    GrustError::CypherUnresolvedIdentity(message.into())
}

pub fn cypher_unsupported_cardinality(message: impl Into<String>) -> GrustError {
    GrustError::CypherUnsupportedCardinality(message.into())
}

pub mod cypher_parser {
    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum CypherStatement<'a> {
        Match(&'a str),
        Create(&'a str),
        Merge(&'a str),
        Delete(&'a str),
    }

    pub fn classify_statement(cypher: &str) -> Result<CypherStatement<'_>> {
        if let Some(rest) = strip_leading_keyword(cypher, "MATCH") {
            return Ok(CypherStatement::Match(rest));
        }
        if find_unquoted_keyword(cypher, "SET").is_some() {
            return Err(cypher_syntax("writable Cypher SET is not supported in v1"));
        }
        if find_unquoted_keyword(cypher, "REMOVE").is_some() {
            return Err(cypher_syntax(
                "writable Cypher REMOVE is not supported in v1",
            ));
        }
        if let Some(rest) = strip_leading_keyword(cypher, "CREATE") {
            return Ok(CypherStatement::Create(rest));
        }
        if let Some(rest) = strip_leading_keyword(cypher, "MERGE") {
            return Ok(CypherStatement::Merge(rest));
        }
        if let Some(rest) = strip_leading_keyword(cypher, "DELETE") {
            return Ok(CypherStatement::Delete(rest));
        }
        Err(cypher_syntax(format!(
            "unsupported writable Cypher statement; expected CREATE, MERGE, or DELETE: {cypher}"
        )))
    }
}

pub fn cypher_execution_error(error: GrustError) -> GrustError {
    match error {
        GrustError::CypherSyntax(_)
        | GrustError::CypherUnresolvedIdentity(_)
        | GrustError::CypherUnsupportedCardinality(_)
        | GrustError::CypherExecution(_) => error,
        other => GrustError::CypherExecution(other.to_string()),
    }
}

pub(crate) fn parse_cypher_ddl_statement(statement: &str) -> Result<CypherDdlStatement> {
    if let Some(rest) = strip_leading_keyword(statement, "CREATE") {
        let rest = rest.trim_start();
        if let Some(rest) = strip_leading_keyword(rest, "CONSTRAINT") {
            return parse_create_constraint(rest.trim());
        }
        if let Some(rest) = strip_leading_keyword(rest, "INDEX") {
            return parse_create_index(rest.trim());
        }
        if let Some(rest) = strip_leading_keyword(rest, "GRAPH") {
            let rest = rest.trim_start();
            if let Some(rest) = strip_leading_keyword(rest, "TYPE") {
                return crate::graph_type_ddl::parse_create_graph_type(rest.trim());
            }
        }
        return Err(cypher_syntax(
            "only CREATE CONSTRAINT, CREATE INDEX, or CREATE GRAPH TYPE is supported as Cypher CREATE DDL",
        ));
    }
    if let Some(rest) = strip_leading_keyword(statement, "DROP") {
        let rest = rest.trim_start();
        if let Some(rest) = strip_leading_keyword(rest, "CONSTRAINT") {
            return parse_drop_constraint(rest.trim());
        }
        if let Some(rest) = strip_leading_keyword(rest, "INDEX") {
            return parse_drop_index(rest.trim());
        }
        if let Some(rest) = strip_leading_keyword(rest, "GRAPH") {
            let rest = rest.trim_start();
            if let Some(rest) = strip_leading_keyword(rest, "TYPE") {
                return crate::graph_type_ddl::parse_drop_graph_type(rest.trim());
            }
        }
        return Err(cypher_syntax(
            "only DROP CONSTRAINT, DROP INDEX, or DROP GRAPH TYPE is supported as Cypher DROP DDL",
        ));
    }
    Err(cypher_syntax(format!(
        "unsupported Cypher DDL statement; expected CREATE/DROP CONSTRAINT, INDEX, or GRAPH TYPE: {statement}"
    )))
}

pub(crate) fn parse_create_constraint(rest: &str) -> Result<CypherDdlStatement> {
    // Split the header (`[name] [IF NOT EXISTS]`) from the body, which starts
    // at `FOR` (or the legacy `ON`).
    let (for_index, body) = find_unquoted_keyword(rest, "FOR")
        .map(|index| (index, &rest[index + "FOR".len()..]))
        .or_else(|| {
            find_unquoted_keyword(rest, "ON").map(|index| (index, &rest[index + "ON".len()..]))
        })
        .ok_or_else(|| cypher_syntax("CREATE CONSTRAINT requires a FOR (or ON) pattern clause"))?;
    let header = rest[..for_index].trim();

    let (name, if_not_exists) = if let Some(if_index) = find_unquoted_keyword(header, "IF") {
        let tail = header[if_index + "IF".len()..].trim();
        if !tail.eq_ignore_ascii_case("NOT EXISTS")
            && tail.split_whitespace().collect::<Vec<_>>() != ["NOT", "EXISTS"]
        {
            return Err(cypher_syntax(
                "CREATE CONSTRAINT only supports the IF NOT EXISTS modifier",
            ));
        }
        (constraint_name(header[..if_index].trim())?, true)
    } else {
        (constraint_name(header)?, false)
    };

    // Body: `<pattern> REQUIRE <predicate>` (or legacy `ASSERT`).
    let (require_index, require_len) = find_unquoted_keyword(body, "REQUIRE")
        .map(|index| (index, "REQUIRE".len()))
        .or_else(|| find_unquoted_keyword(body, "ASSERT").map(|index| (index, "ASSERT".len())))
        .ok_or_else(|| {
            cypher_syntax("CREATE CONSTRAINT requires a REQUIRE (or ASSERT) predicate clause")
        })?;
    let pattern = body[..require_index].trim();
    let predicate = body[require_index + require_len..].trim();

    let (is_edge, pattern_variable, label) = parse_constraint_pattern(pattern)?;
    let (unique, key) = parse_constraint_predicate(predicate, &pattern_variable)?;

    let constraint = match (is_edge, unique) {
        (false, true) => GraphConstraint::NodePropertyUnique { label, key },
        (false, false) => GraphConstraint::NodePropertyRequired { label, key },
        (true, true) => GraphConstraint::EdgePropertyUnique { label, key },
        (true, false) => GraphConstraint::EdgePropertyRequired { label, key },
    };
    Ok(CypherDdlStatement::CreateConstraint {
        name,
        if_not_exists,
        constraint,
    })
}

pub(crate) fn parse_drop_constraint(rest: &str) -> Result<CypherDdlStatement> {
    let (name, if_exists) = if let Some(if_index) = find_unquoted_keyword(rest, "IF") {
        let tail = rest[if_index + "IF".len()..].trim();
        if !tail.eq_ignore_ascii_case("EXISTS") {
            return Err(cypher_syntax(
                "DROP CONSTRAINT only supports the IF EXISTS modifier",
            ));
        }
        (rest[..if_index].trim(), true)
    } else {
        (rest.trim(), false)
    };
    if !is_cypher_identifier(name) {
        return Err(cypher_syntax("DROP CONSTRAINT requires a constraint name"));
    }
    Ok(CypherDdlStatement::DropConstraint {
        name: name.to_string(),
        if_exists,
    })
}

pub(crate) fn parse_create_index(rest: &str) -> Result<CypherDdlStatement> {
    let (for_index, body) = find_unquoted_keyword(rest, "FOR")
        .map(|index| (index, &rest[index + "FOR".len()..]))
        .ok_or_else(|| cypher_syntax("CREATE INDEX requires a FOR pattern clause"))?;
    let header = rest[..for_index].trim();
    let (name, if_not_exists) = parse_required_ddl_name_and_if_not_exists(header, "index")?;

    let (on_index, on_len) = find_unquoted_keyword(body, "ON")
        .map(|index| (index, "ON".len()))
        .ok_or_else(|| cypher_syntax("CREATE INDEX requires an ON property clause"))?;
    let pattern = body[..on_index].trim();
    let property_clause = body[on_index + on_len..].trim();

    let (is_edge, pattern_variable, label) = parse_constraint_pattern(pattern)?;
    let key = parse_index_property_clause(property_clause, &pattern_variable)?;
    let index = GraphIndexDefinition {
        element: if is_edge {
            GraphIndexElement::Edge
        } else {
            GraphIndexElement::Node
        },
        label,
        key,
    };
    Ok(CypherDdlStatement::CreateIndex {
        name,
        if_not_exists,
        index,
    })
}

pub(crate) fn parse_drop_index(rest: &str) -> Result<CypherDdlStatement> {
    let (name, if_exists) = parse_required_ddl_name_and_if_exists(rest, "index")?;
    Ok(CypherDdlStatement::DropIndex { name, if_exists })
}

pub(crate) fn parse_required_ddl_name_and_if_not_exists(
    header: &str,
    object_kind: &str,
) -> Result<(String, bool)> {
    let (name, if_not_exists) = if let Some(if_index) = find_unquoted_keyword(header, "IF") {
        let tail = header[if_index + "IF".len()..].trim();
        if !tail.eq_ignore_ascii_case("NOT EXISTS")
            && tail.split_whitespace().collect::<Vec<_>>() != ["NOT", "EXISTS"]
        {
            return Err(cypher_syntax(format!(
                "CREATE {object_kind} only supports the IF NOT EXISTS modifier"
            )));
        }
        (header[..if_index].trim(), true)
    } else {
        (header.trim(), false)
    };
    if !is_cypher_identifier(name) {
        return Err(cypher_syntax(format!(
            "CREATE {object_kind} requires a {object_kind} name"
        )));
    }
    Ok((name.to_string(), if_not_exists))
}

pub(crate) fn parse_required_ddl_name_and_if_exists(
    rest: &str,
    object_kind: &str,
) -> Result<(String, bool)> {
    let (name, if_exists) = if let Some(if_index) = find_unquoted_keyword(rest, "IF") {
        let tail = rest[if_index + "IF".len()..].trim();
        if !tail.eq_ignore_ascii_case("EXISTS") {
            return Err(cypher_syntax(format!(
                "DROP {object_kind} only supports the IF EXISTS modifier"
            )));
        }
        (rest[..if_index].trim(), true)
    } else {
        (rest.trim(), false)
    };
    if !is_cypher_identifier(name) {
        return Err(cypher_syntax(format!(
            "DROP {object_kind} requires a {object_kind} name"
        )));
    }
    Ok((name.to_string(), if_exists))
}

pub(crate) fn parse_index_property_clause(
    property_clause: &str,
    pattern_variable: &str,
) -> Result<String> {
    let clause = property_clause.trim();
    let inner = clause
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
        .ok_or_else(|| cypher_syntax("CREATE INDEX ON clause must be a parenthesized property"))?
        .trim();
    if inner.contains(',') {
        return Err(cypher_syntax(
            "CREATE INDEX only supports a single indexed property",
        ));
    }
    let (variable, key) = parse_property_ref(inner, "index property")?;
    if variable != pattern_variable {
        return Err(cypher_syntax(format!(
            "index property variable '{variable}' does not match pattern variable '{pattern_variable}'"
        )));
    }
    Ok(key)
}

/// Parses the optional constraint name in a `CREATE CONSTRAINT` header.
pub(crate) fn constraint_name(header: &str) -> Result<Option<String>> {
    let header = header.trim();
    if header.is_empty() {
        return Ok(None);
    }
    if is_cypher_identifier(header) {
        Ok(Some(header.to_string()))
    } else {
        Err(cypher_syntax(format!(
            "unsupported CREATE CONSTRAINT name: {header}"
        )))
    }
}

/// Parses a constraint `FOR` pattern, returning whether it is a relationship
/// pattern, the bound variable, and the single label/type.
pub(crate) fn parse_constraint_pattern(pattern: &str) -> Result<(bool, String, Label)> {
    let pattern = pattern.trim();
    if let Some(open) = pattern.find('[') {
        let close = pattern[open + 1..]
            .find(']')
            .map(|offset| offset + open + 1)
            .ok_or_else(|| cypher_syntax("constraint relationship pattern is missing ']'"))?;
        let (variable, label) = parse_constraint_var_label(&pattern[open + 1..close])?;
        return Ok((true, variable, label));
    }
    let open = pattern.find('(').ok_or_else(|| {
        cypher_syntax("constraint pattern must be a node or relationship pattern")
    })?;
    let close = pattern[open + 1..]
        .find(')')
        .map(|offset| offset + open + 1)
        .ok_or_else(|| cypher_syntax("constraint node pattern is missing ')'"))?;
    let (variable, label) = parse_constraint_var_label(&pattern[open + 1..close])?;
    Ok((false, variable, label))
}

/// Parses the `variable:Label` body inside a constraint pattern.
pub(crate) fn parse_constraint_var_label(body: &str) -> Result<(String, Label)> {
    let (variable, label) = body
        .split_once(':')
        .ok_or_else(|| cypher_syntax("constraint pattern requires variable:Label"))?;
    let variable = parse_required_cypher_variable(variable.trim(), "constraint pattern variable")?;
    let label = label.trim();
    if label.is_empty() {
        return Err(cypher_syntax("constraint pattern requires a label or type"));
    }
    Ok((variable, Label::new(label.to_string())))
}

/// Parses a `variable.key IS [NOT NULL|UNIQUE]` constraint predicate, returning
/// `(is_unique, key)`. The predicate variable must match the pattern variable.
pub(crate) fn parse_constraint_predicate(
    predicate: &str,
    pattern_variable: &str,
) -> Result<(bool, String)> {
    let is_index = find_unquoted_keyword(predicate, "IS").ok_or_else(|| {
        cypher_syntax("constraint predicate requires 'IS UNIQUE' or 'IS NOT NULL'")
    })?;
    let (variable, key) = parse_property_ref(predicate[..is_index].trim(), "constraint predicate")?;
    if variable != pattern_variable {
        return Err(cypher_syntax(format!(
            "constraint predicate variable '{variable}' does not match pattern variable '{pattern_variable}'"
        )));
    }
    let kind = predicate[is_index + "IS".len()..].trim();
    if kind.eq_ignore_ascii_case("UNIQUE") {
        Ok((true, key))
    } else if kind.split_whitespace().collect::<Vec<_>>() == ["NOT", "NULL"] {
        Ok((false, key))
    } else {
        Err(cypher_syntax(format!(
            "unsupported constraint predicate; expected IS UNIQUE or IS NOT NULL, got: {kind}"
        )))
    }
}

/// Returns the id of a persisted node that conflicts with `candidate` under a
/// `NodePropertyUnique(label, key)` constraint, or `None` if there is no
/// conflict. A node with the same id as `candidate` is an update, not a
/// conflict, and nodes without the constrained property are ignored.
pub fn unique_node_conflict<'a>(
    existing: &'a [Node],
    candidate: &Node,
    label: &Label,
    key: &str,
) -> Option<&'a NodeId> {
    if &candidate.label != label {
        return None;
    }
    let value = candidate.props.get(key)?;
    existing
        .iter()
        .find(|node| {
            &node.label == label && node.id != candidate.id && node.props.get(key) == Some(value)
        })
        .map(|node| &node.id)
}

/// Returns the [`edge_key`] of a persisted edge that conflicts with `candidate`
/// under an `EdgePropertyUnique(label, key)` constraint, or `None`. An edge with
/// the same structural key as `candidate` is an update, not a conflict.
pub fn unique_edge_conflict(
    existing: &[Edge],
    candidate: &Edge,
    label: &Label,
    key: &str,
) -> Option<String> {
    if &candidate.label != label {
        return None;
    }
    let value = candidate.props.get(key)?;
    let candidate_key = edge_key(candidate);
    existing
        .iter()
        .find(|edge| {
            &edge.label == label
                && edge_key(edge) != candidate_key
                && edge.props.get(key) == Some(value)
        })
        .map(edge_key)
}

pub(crate) fn split_cypher_statements(cypher: &str) -> Result<Vec<&str>> {
    let mut statements = Vec::new();
    let mut start = 0usize;
    let mut quote = None;
    let mut escaped = false;

    for (index, ch) in cypher.char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active {
                quote = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' => quote = Some(ch),
            ';' => {
                let statement = cypher[start..index].trim();
                if !statement.is_empty() {
                    statements.push(statement);
                }
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }

    if quote.is_some() {
        return Err(cypher_syntax(
            "Cypher statement has an unterminated string literal".to_string(),
        ));
    }

    let statement = cypher[start..].trim();
    if !statement.is_empty() {
        statements.push(statement);
    }
    Ok(statements)
}

pub(crate) fn strip_cypher_comments(cypher: &str) -> Result<String> {
    let mut output = String::with_capacity(cypher.len());
    let mut chars = cypher.char_indices().peekable();
    let mut quote = None;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;

    while let Some((_, ch)) = chars.next() {
        if line_comment {
            if ch == '\n' {
                line_comment = false;
                output.push(ch);
            }
            continue;
        }
        if block_comment {
            if ch == '*' && chars.peek().is_some_and(|(_, next)| *next == '/') {
                chars.next();
                block_comment = false;
            } else if ch == '\n' {
                output.push(ch);
            }
            continue;
        }
        if let Some(active) = quote {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active {
                quote = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                output.push(ch);
            }
            '/' if chars.peek().is_some_and(|(_, next)| *next == '/') => {
                chars.next();
                line_comment = true;
            }
            '/' if chars.peek().is_some_and(|(_, next)| *next == '*') => {
                chars.next();
                block_comment = true;
            }
            _ => output.push(ch),
        }
    }

    if block_comment {
        return Err(cypher_syntax(
            "Cypher statement has an unterminated block comment".to_string(),
        ));
    }
    Ok(output)
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedCypherNode {
    pub(crate) variable: Option<String>,
    pub(crate) label: Option<Label>,
    pub(crate) props: Props,
    pub(crate) predicates: Vec<GraphPropertyPredicate>,
}

#[derive(Debug)]
pub(crate) struct ParsedCypherEdge {
    pub(crate) from_id: NodeId,
    pub(crate) to_id: NodeId,
    pub(crate) edge: Edge,
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedCypherEdgeMatch {
    pub(crate) from: ParsedCypherNode,
    pub(crate) relationship: ParsedCypherRelationship,
    pub(crate) to: ParsedCypherNode,
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedCypherRelationship {
    pub(crate) variable: Option<String>,
    pub(crate) label: Label,
    pub(crate) props: Props,
    pub(crate) predicates: Vec<GraphPropertyPredicate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CypherBoundEdgeIdentity {
    pub from: NodeId,
    pub label: Label,
    pub to: NodeId,
    pub id: Option<EdgeId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CypherRowProducedEdgeBinding {
    pub kind: GraphMutationPlanKind,
    pub from_variable: String,
    pub from: GraphNodeMatch,
    pub to_variable: String,
    pub to: GraphNodeMatch,
    pub label: Label,
    pub props: Props,
    pub edge_id_policy: GraphRowEdgeIdPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CypherRowProducedPathBinding {
    pub from_variable: String,
    pub edge_variable: String,
    pub to_variable: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ParsedWherePredicate {
    pub(crate) target: String,
    pub(crate) predicate: GraphPropertyPredicate,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CypherWhereBoolean<'a> {
    Predicate(&'a str),
    Not(Box<CypherWhereBoolean<'a>>),
    And(Vec<CypherWhereBoolean<'a>>),
    Or(Vec<CypherWhereBoolean<'a>>),
}

pub(crate) fn parse_cypher_node_pattern<'a>(
    input: &'a str,
    parameters: &CypherParameters,
) -> Result<(ParsedCypherNode, &'a str)> {
    let input = input.trim_start();
    let input = input.strip_prefix('(').ok_or_else(|| {
        GrustError::Unsupported("writable Cypher node pattern must start with '('".to_string())
    })?;
    let close = find_matching(input, '(', ')')?;
    let body = input[..close].trim();
    let rest = &input[close + 1..];
    let (variable, label, props) = parse_cypher_node_body(body, parameters)?;
    Ok((
        ParsedCypherNode {
            variable,
            label,
            props,
            predicates: Vec::new(),
        },
        rest,
    ))
}

pub(crate) fn parse_cypher_node_body(
    body: &str,
    parameters: &CypherParameters,
) -> Result<(Option<String>, Option<Label>, Props)> {
    let (head, props) = split_cypher_body_props(body, parameters)?;
    let head = head.trim();
    let (variable, label) = if let Some((variable, label)) = head.split_once(':') {
        let label = label.trim();
        (
            parse_optional_cypher_variable(variable.trim())?,
            if label.is_empty() {
                None
            } else {
                Some(Label::new(label.to_string()))
            },
        )
    } else {
        (parse_optional_cypher_variable(head)?, None)
    };
    Ok((variable, label, props))
}

pub(crate) fn parse_optional_cypher_variable(value: &str) -> Result<Option<String>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if is_cypher_identifier(value) {
        return Ok(Some(value.to_string()));
    }
    Err(GrustError::Unsupported(format!(
        "unsupported Cypher variable name: {value}"
    )))
}

pub(crate) fn parse_required_cypher_variable(value: &str, context: &str) -> Result<String> {
    parse_optional_cypher_variable(value)?
        .ok_or_else(|| GrustError::Unsupported(format!("{context} requires a variable name")))
}

pub(crate) fn is_cypher_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

pub(crate) fn parse_cypher_relationship(
    body: &str,
    parameters: &CypherParameters,
) -> Result<ParsedCypherRelationship> {
    let (head, props) = split_cypher_body_props(body.trim(), parameters)?;
    let Some((variable, label)) = head.trim().split_once(':') else {
        return Err(GrustError::Unsupported(
            "edge CREATE/MERGE/DELETE requires a relationship type".into(),
        ));
    };
    let label = label.trim();
    if label.is_empty() {
        return Err(GrustError::Unsupported(
            "edge CREATE/MERGE/DELETE requires a relationship type".into(),
        ));
    }
    Ok(ParsedCypherRelationship {
        variable: parse_optional_cypher_variable(variable.trim())?,
        label: Label::new(label.to_string()),
        props,
        predicates: Vec::new(),
    })
}

pub(crate) fn validate_optional_edge_id_property(props: &Props) -> Result<()> {
    edge_id_from_props(props).map(|_| ())
}

pub fn edge_id_from_props(props: &Props) -> Result<Option<String>> {
    match props.get("id") {
        Some(Value::String(id)) => Ok(Some(id.clone())),
        Some(_) => Err(cypher_syntax(
            "relationship id property must be a string literal",
        )),
        None => Ok(None),
    }
}

pub(crate) fn match_node_cardinality(node: &ParsedCypherNode) -> GraphMutationCardinality {
    if node.label.is_some() || !node.props.is_empty() || !node.predicates.is_empty() {
        GraphMutationCardinality::BoundedMany
    } else {
        GraphMutationCardinality::UnboundedMany
    }
}
