//! Portable graph-type DDL parsing helpers.
//!
//! This module intentionally owns the graph-type DDL grammar so `parse.rs`
//! does not keep growing. The first supported surface is conservative:
//!
//! ```text
//! CREATE GRAPH TYPE name [IF NOT EXISTS] [OPEN|CLOSED] AS
//!   NODE Person (name STRING REQUIRED, age INT),
//!   EDGE KNOWS FROM Person TO Person (since INT)
//! DROP GRAPH TYPE name [IF EXISTS]
//! ```

use crate::*;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphTypeDefinition {
    pub mode: GraphTypeMode,
    pub schema: GraphSchema,
}

impl Eq for GraphTypeDefinition {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NamedGraphType {
    pub name: String,
    pub graph_type: GraphTypeDefinition,
}

pub(crate) fn parse_create_graph_type(rest: &str) -> Result<CypherDdlStatement> {
    let (as_index, body) = find_unquoted_keyword(rest, "AS")
        .map(|index| (index, &rest[index + "AS".len()..]))
        .ok_or_else(|| cypher_syntax("CREATE GRAPH TYPE requires an AS body"))?;
    let header = rest[..as_index].trim();
    let (header, mode) = parse_graph_type_mode(header);
    let (name, if_not_exists) = parse_required_ddl_name_and_if_not_exists(header, "graph type")?;
    let graph_type = parse_graph_type_body(body.trim(), mode)?;
    Ok(CypherDdlStatement::CreateGraphType {
        name,
        if_not_exists,
        graph_type,
    })
}

pub(crate) fn parse_drop_graph_type(rest: &str) -> Result<CypherDdlStatement> {
    let (name, if_exists) = parse_required_ddl_name_and_if_exists(rest, "graph type")?;
    Ok(CypherDdlStatement::DropGraphType { name, if_exists })
}

fn parse_graph_type_mode(header: &str) -> (&str, GraphTypeMode) {
    let header = header.trim();
    if let Some(prefix) = header.strip_suffix(" CLOSED") {
        (prefix.trim_end(), GraphTypeMode::Closed)
    } else if header.eq_ignore_ascii_case("CLOSED") {
        ("", GraphTypeMode::Closed)
    } else if let Some(prefix) = header.strip_suffix(" OPEN") {
        (prefix.trim_end(), GraphTypeMode::Open)
    } else if header.eq_ignore_ascii_case("OPEN") {
        ("", GraphTypeMode::Open)
    } else {
        (header, GraphTypeMode::Open)
    }
}

fn parse_graph_type_body(body: &str, mode: GraphTypeMode) -> Result<GraphTypeDefinition> {
    let mut schema = GraphSchema::builder();
    for item in split_top_level_commas(body)? {
        let item = item.trim();
        if let Some(rest) = strip_leading_keyword(item, "NODE") {
            let (label, fields) = parse_typed_fields(rest.trim(), "node type")?;
            schema = schema.node(label, fields);
        } else if let Some(rest) = strip_leading_keyword(item, "EDGE") {
            let edge = parse_edge_type(rest.trim())?;
            schema = schema.edge_type(edge);
        } else {
            return Err(cypher_syntax(format!(
                "graph type body item must start with NODE or EDGE: {item}"
            )));
        }
    }
    Ok(GraphTypeDefinition {
        mode,
        schema: schema.build(),
    })
}

fn parse_edge_type(rest: &str) -> Result<EdgeType> {
    let (from_index, _) = find_unquoted_keyword(rest, "FROM")
        .map(|index| (index, "FROM".len()))
        .ok_or_else(|| cypher_syntax("EDGE graph type item requires FROM"))?;
    let label = parse_label(rest[..from_index].trim(), "edge type label")?;
    let after_from = rest[from_index + "FROM".len()..].trim();
    let to_index = find_unquoted_keyword(after_from, "TO")
        .ok_or_else(|| cypher_syntax("EDGE graph type item requires TO"))?;
    let from_label = parse_label(after_from[..to_index].trim(), "edge FROM label")?;
    let after_to = after_from[to_index + "TO".len()..].trim();
    let (to_label, fields) = parse_typed_fields(after_to, "edge type")?;
    Ok(EdgeType {
        label,
        from: vec![from_label],
        to: vec![to_label],
        fields,
        directed: true,
        uniqueness: EdgeUniqueness::FromLabelTo,
    })
}

fn parse_typed_fields(rest: &str, context: &str) -> Result<(Label, Vec<Field>)> {
    let open = rest
        .find('(')
        .ok_or_else(|| cypher_syntax(format!("{context} requires a field list")))?;
    let close = rest
        .rfind(')')
        .ok_or_else(|| cypher_syntax(format!("{context} field list is missing ')'")))?;
    if !rest[close + 1..].trim().is_empty() {
        return Err(cypher_syntax(format!(
            "{context} has trailing input after field list"
        )));
    }
    let label = parse_label(rest[..open].trim(), context)?;
    let fields = parse_fields(&rest[open + 1..close])?;
    Ok((label, fields))
}

fn parse_fields(body: &str) -> Result<Vec<Field>> {
    let body = body.trim();
    if body.is_empty() {
        return Ok(Vec::new());
    }
    split_top_level_commas(body)?
        .into_iter()
        .map(parse_field)
        .collect()
}

fn parse_field(field: &str) -> Result<Field> {
    let parts = field.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 2 {
        return Err(cypher_syntax(format!(
            "graph type field requires name and type: {field}"
        )));
    }
    if !is_cypher_identifier(parts[0]) {
        return Err(cypher_syntax(format!(
            "graph type field has invalid name '{}'",
            parts[0]
        )));
    }
    let required = match &parts[2..] {
        [] => false,
        ["REQUIRED"] => true,
        ["NOT", "NULL"] => true,
        _ => {
            return Err(cypher_syntax(format!(
                "unsupported graph type field modifier: {}",
                parts[2..].join(" ")
            )));
        }
    };
    Ok(Field {
        name: parts[0].to_string(),
        ty: parse_field_type(parts[1])?,
        required,
    })
}

fn parse_field_type(value: &str) -> Result<FieldType> {
    match value.to_ascii_uppercase().as_str() {
        "STRING" => Ok(FieldType::String),
        "INT" | "INTEGER" => Ok(FieldType::Int),
        "FLOAT" | "DOUBLE" => Ok(FieldType::Float),
        "BOOL" | "BOOLEAN" => Ok(FieldType::Bool),
        "DATETIME" | "DATE_TIME" => Ok(FieldType::DateTime),
        "STRING[]" | "LIST<STRING>" => Ok(FieldType::StringArray),
        "INT[]" | "INTEGER[]" | "LIST<INT>" | "LIST<INTEGER>" => Ok(FieldType::IntArray),
        "FLOAT[]" | "DOUBLE[]" | "LIST<FLOAT>" | "LIST<DOUBLE>" => Ok(FieldType::FloatArray),
        "JSON" | "ANY" => Ok(FieldType::Json),
        other => Err(cypher_syntax(format!(
            "unsupported graph type field type: {other}"
        ))),
    }
}

fn parse_label(value: &str, context: &str) -> Result<Label> {
    if !is_cypher_identifier(value) {
        return Err(cypher_syntax(format!("{context} must be an identifier")));
    }
    Ok(Label::new(value.to_string()))
}
