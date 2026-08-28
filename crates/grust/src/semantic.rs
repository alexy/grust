use grust_core::{Graph, GrustError, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticField {
    pub name: String,
    pub data_type: Option<String>,
    pub nullable: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticDataset {
    pub name: String,
    pub physical_source: String,
    pub fields: Vec<SemanticField>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticMetric {
    pub name: String,
    pub expression_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticRelationship {
    pub name: String,
    pub from_dataset: String,
    pub to_dataset: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticModelProjection {
    pub model_id: String,
    pub version: u64,
    pub artifact_hash: String,
    pub datasets: Vec<SemanticDataset>,
    pub metrics: Vec<SemanticMetric>,
    pub relationships: Vec<SemanticRelationship>,
}

pub fn semantic_model_graph(model: &SemanticModelProjection) -> Result<Graph> {
    if model.model_id.trim().is_empty() || model.version == 0 {
        return Err(GrustError::Serialization(
            "semantic model identity and positive version are required".into(),
        ));
    }
    let model_node = format!(
        "semantic:model:{}:v{}:{}",
        model.model_id, model.version, model.artifact_hash
    );
    let mut builder = Graph::builder();
    let _ = builder
        .node("SemanticModel", model_node.clone())
        .prop("model_id", model.model_id.clone())
        .prop("version", model.version as i64)
        .prop("artifact_hash", model.artifact_hash.clone())
        .finish();
    for dataset in &model.datasets {
        let dataset_id = format!("{model_node}:dataset:{}", dataset.name);
        let _ = builder
            .node("SemanticDataset", dataset_id.clone())
            .prop("name", dataset.name.clone())
            .prop("physical_source", dataset.physical_source.clone())
            .finish();
        let _ = builder
            .edge("CONTAINS_DATASET", model_node.clone(), dataset_id.clone())
            .finish();
        for field in &dataset.fields {
            let field_id = format!("{dataset_id}:field:{}", field.name);
            let mut node = builder
                .node("SemanticField", field_id.clone())
                .prop("name", field.name.clone());
            if let Some(data_type) = &field.data_type {
                node = node.prop("data_type", data_type.clone());
            }
            if let Some(nullable) = field.nullable {
                node = node.prop("nullable", nullable);
            }
            let _ = node.finish();
            let _ = builder
                .edge("CONTAINS_FIELD", dataset_id.clone(), field_id)
                .finish();
        }
    }
    for metric in &model.metrics {
        let metric_id = format!("{model_node}:metric:{}", metric.name);
        let _ = builder
            .node("SemanticMetric", metric_id.clone())
            .prop("name", metric.name.clone())
            .prop("expression_hash", metric.expression_hash.clone())
            .finish();
        let _ = builder
            .edge("CONTAINS_METRIC", model_node.clone(), metric_id)
            .finish();
    }
    for relationship in &model.relationships {
        let from = format!("{model_node}:dataset:{}", relationship.from_dataset);
        let to = format!("{model_node}:dataset:{}", relationship.to_dataset);
        let known = model
            .datasets
            .iter()
            .map(|dataset| dataset.name.as_str())
            .collect::<Vec<_>>();
        if !known.contains(&relationship.from_dataset.as_str())
            || !known.contains(&relationship.to_dataset.as_str())
        {
            return Err(GrustError::Serialization(format!(
                "semantic relationship {} references an unknown dataset",
                relationship.name
            )));
        }
        let _ = builder
            .edge("RELATES_DATASET", from, to)
            .prop("name", relationship.name.clone())
            .finish();
    }
    Ok(builder.build())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model() -> SemanticModelProjection {
        SemanticModelProjection {
            model_id: "tpcds".into(),
            version: 1,
            artifact_hash: "sha256:abc".into(),
            datasets: vec![
                SemanticDataset {
                    name: "sales".into(),
                    physical_source: "lake.sales".into(),
                    fields: vec![SemanticField {
                        name: "amount".into(),
                        data_type: Some("decimal".into()),
                        nullable: Some(false),
                    }],
                },
                SemanticDataset {
                    name: "date".into(),
                    physical_source: "lake.date".into(),
                    fields: vec![],
                },
            ],
            metrics: vec![SemanticMetric {
                name: "revenue".into(),
                expression_hash: "sha256:def".into(),
            }],
            relationships: vec![SemanticRelationship {
                name: "sales_date".into(),
                from_dataset: "sales".into(),
                to_dataset: "date".into(),
            }],
        }
    }
    #[test]
    fn projection_is_complete_and_replay_stable() {
        let first = semantic_model_graph(&model()).unwrap();
        let replay = semantic_model_graph(&model()).unwrap();
        assert_eq!(first, replay);
        assert_eq!(first.nodes.len(), 5);
        assert_eq!(first.edges.len(), 5);
    }
    #[test]
    fn version_changes_identity_and_unknown_relationships_fail() {
        let first = semantic_model_graph(&model()).unwrap();
        let mut changed = model();
        changed.version = 2;
        assert_ne!(first, semantic_model_graph(&changed).unwrap());
        changed.relationships[0].to_dataset = "missing".into();
        assert!(semantic_model_graph(&changed).is_err());
    }

    #[test]
    fn pinned_ossie_tpcds_projection_is_complete_and_replay_stable() {
        let dataset_names = ["store_sales", "date_dim", "customer", "item", "store"];
        let field_counts = [9, 5, 5, 6, 6];
        let datasets = dataset_names
            .iter()
            .zip(field_counts)
            .map(|(name, count)| SemanticDataset {
                name: (*name).into(),
                physical_source: format!("local.tpcds.{name}"),
                fields: (0..count)
                    .map(|index| SemanticField {
                        name: format!("{name}_field_{index}"),
                        data_type: Some("pinned-ossie-logical-type".into()),
                        nullable: Some(true),
                    })
                    .collect(),
            })
            .collect();
        let projection = SemanticModelProjection {
            model_id: "tpcds_retail_model".into(),
            version: 1,
            artifact_hash:
                "sha256:438372de9b8ca0f074aed72806f92ac9b84047851a0385423f004748efe5a316".into(),
            datasets,
            metrics: [
                "total_sales",
                "total_profit",
                "customer_lifetime_value",
                "sales_by_brand",
                "store_productivity",
            ]
            .into_iter()
            .map(|name| SemanticMetric {
                name: name.into(),
                expression_hash: format!("sha256:pinned-{name}"),
            })
            .collect(),
            relationships: [
                ("sales_date", "store_sales", "date_dim"),
                ("sales_customer", "store_sales", "customer"),
                ("sales_item", "store_sales", "item"),
                ("sales_store", "store_sales", "store"),
            ]
            .into_iter()
            .map(|(name, from_dataset, to_dataset)| SemanticRelationship {
                name: name.into(),
                from_dataset: from_dataset.into(),
                to_dataset: to_dataset.into(),
            })
            .collect(),
        };
        let graph = semantic_model_graph(&projection).unwrap();
        assert_eq!(graph, semantic_model_graph(&projection).unwrap());
        assert_eq!(graph.nodes.len(), 42);
        assert_eq!(graph.edges.len(), 45);
    }
}
