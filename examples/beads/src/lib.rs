//! grust-beads: model the [beads](https://github.com/gastownhall/beads) issue
//! tracker as a grust property graph.
//!
//! beads is a graph-shaped issue tracker: issues form a directed graph with
//! typed dependency edges (`blocks`, `parent-child`, `discovered-from`,
//! `related`). This example maps a `bd export` JSONL stream into a `grust::Graph`
//! so the issue graph can be traversed and queried with grust (including with
//! Cypher/GQL via `grust-cypher`).
//!
//! Mapping:
//! - each issue → an `Issue` node (id = issue id; properties = title, status,
//!   priority, issue_type, owner, assignee, timestamps, labels);
//! - each dependency → an edge `(issue_id)-[:<TYPE>]->(depends_on_id)`, where
//!   `<TYPE>` is the dependency type normalized to a Cypher-style label
//!   (`blocks` → `BLOCKS`, `parent-child` → `PARENT_CHILD`).

use std::collections::BTreeMap;
use std::io::BufRead;

use grust_core::{Edge, Graph, Node, Props, Value};
use serde::Deserialize;

/// A beads dependency edge (`{issue_id, depends_on_id, type}`), as emitted in an
/// issue record's `dependencies` array by `bd export`.
#[derive(Debug, Clone, Deserialize)]
pub struct BeadDependency {
    pub issue_id: String,
    pub depends_on_id: String,
    #[serde(rename = "type")]
    pub dep_type: String,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// A beads issue record from a `bd export` JSONL line. Only the fields mapped
/// into the graph are modeled; unknown fields are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct BeadIssue {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub priority: Option<i64>,
    #[serde(default)]
    pub issue_type: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<BeadDependency>,
}

/// Normalize a beads dependency type to a Cypher-style edge label, e.g.
/// `blocks` → `BLOCKS`, `parent-child` → `PARENT_CHILD`.
pub fn edge_label(dep_type: &str) -> String {
    dep_type.to_ascii_uppercase().replace('-', "_")
}

fn issue_node(issue: &BeadIssue) -> Node {
    let mut props: Props = BTreeMap::new();
    if !issue.title.is_empty() {
        props.insert("title".into(), Value::from(issue.title.as_str()));
    }
    if let Some(s) = &issue.status {
        props.insert("status".into(), Value::from(s.as_str()));
    }
    if let Some(p) = issue.priority {
        props.insert("priority".into(), Value::Int(p));
    }
    if let Some(t) = &issue.issue_type {
        props.insert("issue_type".into(), Value::from(t.as_str()));
    }
    if let Some(o) = &issue.owner {
        props.insert("owner".into(), Value::from(o.as_str()));
    }
    if let Some(a) = &issue.assignee {
        props.insert("assignee".into(), Value::from(a.as_str()));
    }
    if let Some(c) = &issue.created_at {
        props.insert("created_at".into(), Value::from(c.as_str()));
    }
    if let Some(u) = &issue.updated_at {
        props.insert("updated_at".into(), Value::from(u.as_str()));
    }
    if !issue.labels.is_empty() {
        props.insert("labels".into(), Value::StringArray(issue.labels.clone()));
    }
    Node::new("Issue", issue.id.as_str(), props)
}

/// Build a `grust::Graph` from beads issue records.
pub fn build_graph(issues: &[BeadIssue]) -> Graph {
    let mut nodes = Vec::with_capacity(issues.len());
    let mut edges = Vec::new();
    for issue in issues {
        nodes.push(issue_node(issue));
        for dep in &issue.dependencies {
            let mut props: Props = BTreeMap::new();
            if let Some(c) = &dep.created_at {
                props.insert("created_at".into(), Value::from(c.as_str()));
            }
            edges.push(Edge::new(
                edge_label(&dep.dep_type),
                dep.issue_id.as_str(),
                dep.depends_on_id.as_str(),
                props,
            ));
        }
    }
    Graph::new(nodes, edges)
}

/// Parse a beads `bd export` JSONL stream into issue records. Blank lines are
/// skipped, as are records whose `_type` is present and not `"issue"` (e.g.
/// comment records in a full export).
pub fn parse_jsonl<R: BufRead>(reader: R) -> Result<Vec<BeadIssue>, Box<dyn std::error::Error>> {
    let mut issues = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(trimmed)?;
        if let Some(t) = value.get("_type").and_then(|t| t.as_str()) {
            if t != "issue" {
                continue;
            }
        }
        issues.push(serde_json::from_value(value)?);
    }
    Ok(issues)
}

/// Convenience: load a beads JSONL export directly into a `grust::Graph`.
pub fn load_jsonl<R: BufRead>(reader: R) -> Result<Graph, Box<dyn std::error::Error>> {
    Ok(build_graph(&parse_jsonl(reader)?))
}
