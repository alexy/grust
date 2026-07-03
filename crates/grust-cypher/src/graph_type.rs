//! Graph-type validation (Unit 11): the open-vs-closed graph-type distinction
//! and write-time type-violation checks.
//!
//! GQL distinguishes an **open** graph type (any labels/properties are allowed)
//! from a **closed/typed** graph type (only the declared node/edge types and
//! their declared properties are allowed). `GraphSchema` is the portable
//! enforcement shape; this module validates a [`Node`]/[`Edge`] against it under
//! a [`GraphTypeMode`], returning a structured [`GqlError`] on the first
//! violation. It is additive and backend-neutral — a backend (or the write path)
//! calls it for `ValidateBeforeWrite` enforcement; it does not change any backend.

use crate::*;

/// Whether a graph type constrains which labels/properties may appear.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GraphTypeMode {
    /// Any labels and properties are allowed; only declared properties are
    /// type-checked and `*PropertyRequired` constraints are enforced.
    Open,
    /// Only declared node/edge labels and their declared properties are allowed
    /// (plus the synthetic `id`); undeclared labels/properties are rejected.
    Closed,
}

/// True if `value` conforms to the declared `ty`. `Int` widens to `Float`;
/// `Json` accepts anything; `Null` is handled by the caller (absent/optional).
pub fn value_matches_field_type(value: &Value, ty: &FieldType) -> bool {
    matches!(
        (ty, value),
        (FieldType::String, Value::String(_))
            | (FieldType::Int, Value::Int(_))
            | (FieldType::Float, Value::Float(_))
            | (FieldType::Float, Value::Int(_))
            | (FieldType::Bool, Value::Bool(_))
            | (FieldType::DateTime, Value::DateTime(_))
            | (FieldType::StringArray, Value::StringArray(_))
            | (FieldType::IntArray, Value::IntArray(_))
            | (FieldType::FloatArray, Value::FloatArray(_))
            | (FieldType::Json, _)
    )
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::String(_) => "string",
        Value::DateTime(_) => "datetime",
        Value::Decimal(_) => "decimal",
        Value::Duration(_) => "duration",
        Value::StringArray(_) => "string-array",
        Value::IntArray(_) => "int-array",
        Value::FloatArray(_) => "float-array",
        Value::Path(_) => "path",
        Value::Json(_) => "json",
    }
}

/// Validate a node against `schema` under `mode`.
///
/// - Closed: the node's label must be a declared `NodeType`, and every property
///   (other than `id`) must be a declared field.
/// - Both modes: declared properties are type-checked, and
///   `NodePropertyRequired` constraints for the label must be satisfied.
pub fn validate_node(schema: &GraphSchema, mode: GraphTypeMode, node: &Node) -> Result<()> {
    let node_type = schema.nodes.iter().find(|n| n.label == node.label);
    match node_type {
        None => {
            if mode == GraphTypeMode::Closed {
                return Err(graph_type_error(format!(
                    "node '{}' has label '{}' which is not declared in the closed graph type",
                    node.id.as_str(),
                    node.label.as_str()
                )));
            }
        }
        Some(node_type) => {
            for field in &node_type.fields {
                match node.props.get(&field.name) {
                    None => {
                        if field.required {
                            return Err(gql_cardinality(format!(
                                "node '{}' is missing required property '{}'",
                                node.id.as_str(),
                                field.name
                            )));
                        }
                    }
                    Some(value) => {
                        if !matches!(value, Value::Null)
                            && !value_matches_field_type(value, &field.ty)
                        {
                            return Err(gql_type(format!(
                                "node '{}' property '{}' is {} but the graph type declares {:?}",
                                node.id.as_str(),
                                field.name,
                                value_kind(value),
                                field.ty
                            )));
                        }
                    }
                }
            }
            if mode == GraphTypeMode::Closed {
                for key in node.props.keys() {
                    if key != "id" && !node_type.fields.iter().any(|f| &f.name == key) {
                        return Err(graph_type_error(format!(
                            "node '{}' has undeclared property '{}' under the closed graph type",
                            node.id.as_str(),
                            key
                        )));
                    }
                }
            }
        }
    }
    for constraint in &schema.constraints {
        if let GraphConstraint::NodePropertyRequired { label, key } = constraint {
            if &node.label == label && !node.props.contains_key(key) {
                return Err(gql_cardinality(format!(
                    "node '{}' is missing required property '{}'",
                    node.id.as_str(),
                    key
                )));
            }
        }
    }
    Ok(())
}

/// Validate an edge against `schema` under `mode`. Endpoint-label checks
/// (`EdgeType::from`/`to`) need graph context and are out of scope here; this
/// validates the edge's declared label, property types, and required properties.
pub fn validate_edge(schema: &GraphSchema, mode: GraphTypeMode, edge: &Edge) -> Result<()> {
    let edge_type = schema.edges.iter().find(|e| e.label == edge.label);
    match edge_type {
        None => {
            if mode == GraphTypeMode::Closed {
                return Err(graph_type_error(format!(
                    "relationship type '{}' is not declared in the closed graph type",
                    edge.label.as_str()
                )));
            }
        }
        Some(edge_type) => {
            for field in &edge_type.fields {
                match edge.props.get(&field.name) {
                    None => {
                        if field.required {
                            return Err(gql_cardinality(format!(
                                "relationship '{}' is missing required property '{}'",
                                edge.label.as_str(),
                                field.name
                            )));
                        }
                    }
                    Some(value) => {
                        if !matches!(value, Value::Null)
                            && !value_matches_field_type(value, &field.ty)
                        {
                            return Err(gql_type(format!(
                                "relationship '{}' property '{}' is {} but the graph type declares {:?}",
                                edge.label.as_str(),
                                field.name,
                                value_kind(value),
                                field.ty
                            )));
                        }
                    }
                }
            }
            if mode == GraphTypeMode::Closed {
                for key in edge.props.keys() {
                    if !edge_type.fields.iter().any(|f| &f.name == key) {
                        return Err(graph_type_error(format!(
                            "relationship '{}' has undeclared property '{}' under the closed graph type",
                            edge.label.as_str(),
                            key
                        )));
                    }
                }
            }
        }
    }
    for constraint in &schema.constraints {
        if let GraphConstraint::EdgePropertyRequired { label, key } = constraint {
            if &edge.label == label && !edge.props.contains_key(key) {
                return Err(gql_cardinality(format!(
                    "relationship '{}' is missing required property '{}'",
                    edge.label.as_str(),
                    key
                )));
            }
        }
    }
    Ok(())
}

/// Validate a whole graph (every node and edge) against `schema` under `mode`.
pub fn validate_graph(schema: &GraphSchema, mode: GraphTypeMode, graph: &Graph) -> Result<()> {
    for node in &graph.nodes {
        validate_node(schema, mode, node)?;
    }
    for edge in &graph.edges {
        validate_edge(schema, mode, edge)?;
    }
    Ok(())
}

/// Graph-type violations (undeclared label/property in a closed graph type) are
/// surfaced as GQL type errors.
fn graph_type_error(message: impl Into<String>) -> GrustError {
    gql_type(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> GraphSchema {
        GraphSchema::builder()
            .node(
                "Person",
                vec![
                    Field {
                        name: "name".into(),
                        ty: FieldType::String,
                        required: true,
                    },
                    Field {
                        name: "age".into(),
                        ty: FieldType::Int,
                        required: false,
                    },
                ],
            )
            .required_node_property("Person", "name")
            .build()
    }

    fn person(id: &str, props: &[(&str, Value)]) -> Node {
        let mut p = Props::new();
        for (k, v) in props {
            p.insert((*k).to_string(), v.clone());
        }
        Node::new("Person", id, p)
    }

    #[test]
    fn open_allows_undeclared_labels_and_props() {
        let s = schema();
        // Undeclared label is fine when open.
        let city = Node::new("City", "c1", Props::new());
        assert!(validate_node(&s, GraphTypeMode::Open, &city).is_ok());
        // Undeclared property is fine when open (declared ones still type-checked).
        let n = person(
            "p1",
            &[("name", Value::from("Ada")), ("extra", Value::from("x"))],
        );
        assert!(validate_node(&s, GraphTypeMode::Open, &n).is_ok());
    }

    #[test]
    fn closed_rejects_undeclared_label_and_property() {
        let s = schema();
        let city = Node::new("City", "c1", Props::new());
        assert!(validate_node(&s, GraphTypeMode::Closed, &city).is_err());
        let extra = person(
            "p1",
            &[("name", Value::from("Ada")), ("extra", Value::from("x"))],
        );
        assert!(validate_node(&s, GraphTypeMode::Closed, &extra).is_err());
    }

    #[test]
    fn type_mismatch_and_required_are_enforced_in_both_modes() {
        let s = schema();
        // age declared Int but given a String → violation (open and closed).
        let bad = person(
            "p1",
            &[("name", Value::from("Ada")), ("age", Value::from("old"))],
        );
        assert!(validate_node(&s, GraphTypeMode::Open, &bad).is_err());
        assert!(validate_node(&s, GraphTypeMode::Closed, &bad).is_err());
        // missing required `name`.
        let missing = person("p1", &[("age", Value::Int(36))]);
        assert!(validate_node(&s, GraphTypeMode::Open, &missing).is_err());
        // a conforming node passes both modes.
        let ok = person(
            "p1",
            &[("name", Value::from("Ada")), ("age", Value::Int(36))],
        );
        assert!(validate_node(&s, GraphTypeMode::Open, &ok).is_ok());
        assert!(validate_node(&s, GraphTypeMode::Closed, &ok).is_ok());
    }
}
