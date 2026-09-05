//! Typed SDK3 nested-AST reads. Direct HTTP/v1 request builders stay separate.
use grust_core::prelude::*;
use helix_db::{QueryRequest, dsl::prelude as dsl};

pub(super) fn node(id: &NodeId) -> QueryRequest {
    QueryRequest::read(
        dsl::read_batch()
            .var_as(
                "nodes",
                dsl::g()
                    .n_where(dsl::SourcePredicate::eq("id", id.as_str()))
                    .value_map(None::<Vec<String>>),
            )
            .returning(["nodes"]),
    )
}

pub(super) fn edges(query: &EdgeQuery) -> QueryRequest {
    let mut predicates = Vec::new();
    if let Some(from) = &query.from {
        predicates.push(dsl::SourcePredicate::eq("from_id", from.as_str()));
    }
    if let Some(to) = &query.to {
        predicates.push(dsl::SourcePredicate::eq("to_id", to.as_str()));
    }
    if let Some(label) = &query.label {
        predicates.push(dsl::SourcePredicate::eq("relationship", label.as_str()));
    }
    let predicate = match predicates.len() {
        0 => dsl::SourcePredicate::has_key("relationship"),
        1 => predicates.remove(0),
        _ => dsl::SourcePredicate::and(predicates),
    };
    QueryRequest::read(
        dsl::read_batch()
            .var_as("edges", dsl::g().e_where(predicate).edge_properties())
            .returning(["edges"]),
    )
}

pub(super) fn traversal(traversal: &Traversal) -> Result<QueryRequest> {
    let mut query = match &traversal.start {
        Start::Node(id) => dsl::g().n_where(dsl::SourcePredicate::eq("id", id.as_str())),
        Start::NodesByLabel(label) => dsl::g().n_with_label(label.as_str()),
        Start::NodesByProperty { label, key, value } => {
            // Preserve the adapter's public predicate capability; a new SDK
            // version does not silently expand or reinterpret supported values.
            if !matches!(
                value,
                Value::String(_) | Value::Int(_) | Value::Float(_) | Value::Bool(_)
            ) {
                return Err(GrustError::Unsupported(
                    "Helix reads support scalar string, int, float, and bool predicates"
                        .to_string(),
                ));
            }
            dsl::g().n_with_label_where(
                label.as_str(),
                dsl::SourcePredicate::eq(key.as_str(), super::helix_property_input(value)?),
            )
        }
    };
    for step in &traversal.steps {
        let edge = step
            .edge
            .as_ref()
            .map(|label| super::relationship_type(label.as_str()));
        query = match step.direction {
            Direction::Out => query.out(edge),
            Direction::In => query.in_(edge),
            Direction::Both => query.both(edge),
        };
        if let Some(label) = &step.node {
            query = query.has_label(label.as_str());
        }
    }
    Ok(QueryRequest::read(
        dsl::read_batch()
            .var_as("nodes", query.value_map(None::<Vec<String>>))
            .returning(["nodes"]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(request: QueryRequest) -> serde_json::Value {
        assert_eq!(request.request_type(), helix_db::QueryRequestType::Read);
        let value = serde_json::to_value(&request).unwrap();
        let decoded: QueryRequest = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(decoded, request);
        assert!(value["query"]["read"]["entries"][0]["query"]["root"].is_object());
        assert!(
            value["query"]["queries"].is_null(),
            "SDK3 must not emit legacy step arrays"
        );
        value
    }

    #[test]
    fn node_read_preserves_identifier_as_data_in_nested_ast() {
        let value = wire(node(&NodeId::new("a' OR true // λ")));
        let root = &value["query"]["read"]["entries"][0]["query"]["root"];
        assert!(root["value_map"]["input"]["nodes_where"].is_object());
        assert!(
            serde_json::to_string(&value)
                .unwrap()
                .contains("a' OR true // λ")
        );
        assert_eq!(
            value["query"]["read"]["returns"],
            serde_json::json!(["nodes"])
        );
    }

    #[test]
    fn edge_reads_keep_all_filters_and_edge_property_projection() {
        let value = wire(edges(&EdgeQuery {
            from: Some(NodeId::new("from-marker")),
            to: Some(NodeId::new("to-marker")),
            label: Some(Label::new("label-marker")),
        }));
        let root = &value["query"]["read"]["entries"][0]["query"]["root"];
        assert!(root["edge_properties"]["input"]["edges_where"].is_object());
        let text = value.to_string();
        for expected in ["from-marker", "to-marker", "label-marker"] {
            assert!(text.contains(expected));
        }
        wire(edges(&EdgeQuery::default()));
    }

    #[test]
    fn traversal_covers_directions_and_retains_node_label_filter() {
        for (direction, operation) in [
            (Direction::Out, "out"),
            (Direction::In, "in"),
            (Direction::Both, "both"),
        ] {
            let request = traversal(&Traversal {
                start: Start::NodesByLabel(Label::new("Person")),
                steps: vec![Step {
                    direction,
                    edge: Some(Label::new("knows")),
                    node: Some(Label::new("Person")),
                }],
                limit: Some(2),
            })
            .unwrap();
            let value = wire(request);
            let root = &value["query"]["read"]["entries"][0]["query"]["root"];
            assert!(root["value_map"]["input"]["has_label"]["input"][operation].is_object());
        }
    }

    #[test]
    fn unsupported_predicate_remains_rejected_before_transport() {
        let result = traversal(&Traversal {
            start: Start::NodesByProperty {
                label: Label::new("Person"),
                key: "payload".into(),
                value: Value::Json(serde_json::json!({"injected": true})),
            },
            steps: vec![],
            limit: None,
        });
        assert!(matches!(result, Err(GrustError::Unsupported(_))));
    }
}
