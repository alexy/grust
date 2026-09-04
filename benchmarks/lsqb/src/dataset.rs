use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use grust_core::{Edge, EdgeId, Graph, Node, Props, Value};

pub fn load_projected_dataset(directory: &Path) -> Result<Graph, String> {
    let mut paths = fs::read_dir(directory)
        .map_err(|err| format!("cannot read {}: {err}", directory.display()))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|err| err.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.retain(|path| path.extension().is_some_and(|ext| ext == "csv"));
    paths.sort();

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for path in &paths {
        let text = fs::read_to_string(path)
            .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
        let mut lines = text.lines();
        let header = lines
            .next()
            .ok_or_else(|| format!("{} is empty", path.display()))?;
        if header.starts_with("id:ID(") {
            load_nodes(path, header, lines, &mut nodes)?;
        } else if header.starts_with(":START_ID(") {
            load_edges(path, header, lines, &mut edges)?;
        } else {
            return Err(format!(
                "unsupported LSQB CSV header in {}: {header}",
                path.display()
            ));
        }
    }
    Ok(Graph::new(nodes, edges))
}

fn load_nodes<'a>(
    path: &Path,
    header: &str,
    lines: impl Iterator<Item = &'a str>,
    nodes: &mut Vec<Node>,
) -> Result<(), String> {
    let source_type = between(header, "id:ID(", ")")?;
    let label = logical_label(source_type);
    for (offset, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let raw_id = line
            .split('|')
            .next()
            .ok_or_else(|| format!("{}:{} has no id", path.display(), offset + 2))?;
        let mut props = Props::new();
        props.insert("source_type".to_string(), Value::from(source_type));
        props.insert("source_id".to_string(), source_value(raw_id));
        if matches!(source_type, "Post" | "Comment") {
            props.insert("kind".to_string(), Value::from(source_type));
        }
        nodes.push(Node::new(label, namespaced_id(source_type, raw_id), props));
    }
    Ok(())
}

fn load_edges<'a>(
    path: &Path,
    header: &str,
    lines: impl Iterator<Item = &'a str>,
    edges: &mut Vec<Edge>,
) -> Result<(), String> {
    let mut columns = header.split('|');
    let source_type = between(
        columns
            .next()
            .ok_or_else(|| "missing start column".to_string())?,
        ":START_ID(",
        ")",
    )?;
    let target_type = between(
        columns
            .next()
            .ok_or_else(|| "missing end column".to_string())?,
        ":END_ID(",
        ")",
    )?;
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("invalid LSQB filename {}", path.display()))?;
    let label = relationship_label(stem, source_type, target_type)?;

    for (offset, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let values = line.split('|').collect::<Vec<_>>();
        if values.len() < 2 {
            return Err(format!(
                "{}:{} needs start and end ids",
                path.display(),
                offset + 2
            ));
        }
        let edge_id = format!("{stem}:{}", offset + 1);
        edges.push(
            Edge::new(
                &label,
                namespaced_id(source_type, values[0]),
                namespaced_id(target_type, values[1]),
                BTreeMap::new(),
            )
            .with_id(EdgeId::new(edge_id)),
        );
    }
    Ok(())
}

fn between<'a>(value: &'a str, prefix: &str, suffix: &str) -> Result<&'a str, String> {
    value
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(suffix))
        .ok_or_else(|| format!("invalid LSQB id header: {value}"))
}

fn namespaced_id(source_type: &str, raw_id: &str) -> String {
    format!("{source_type}:{raw_id}")
}

fn logical_label(source_type: &str) -> &str {
    match source_type {
        "Post" | "Comment" => "Message",
        other => other,
    }
}

fn source_value(raw: &str) -> Value {
    raw.parse::<i64>()
        .map(Value::Int)
        .unwrap_or_else(|_| Value::from(raw))
}

fn relationship_label(stem: &str, source: &str, target: &str) -> Result<String, String> {
    let prefix = format!("{source}_");
    let suffix = format!("_{target}");
    let relation = stem
        .strip_prefix(&prefix)
        .and_then(|value| value.strip_suffix(&suffix))
        .ok_or_else(|| format!("filename {stem}.csv does not match its typed header"))?;
    Ok(camel_to_upper_snake(relation))
}

fn camel_to_upper_snake(value: &str) -> String {
    let mut output = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch.is_ascii_uppercase() && index != 0 {
            output.push('_');
        }
        output.push(ch.to_ascii_uppercase());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_lsqb_relationship_names() {
        assert_eq!(
            relationship_label("Person_isLocatedIn_City", "Person", "City").unwrap(),
            "IS_LOCATED_IN"
        );
        assert_eq!(
            relationship_label("Person_knows_Person", "Person", "Person").unwrap(),
            "KNOWS"
        );
    }
}
