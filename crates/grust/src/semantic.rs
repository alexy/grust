use std::collections::BTreeSet;

use grust_core::{EdgePolicy, Graph, GrustError, Result};

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
    validate_projection(model)?;
    let model_node = format!(
        "semantic:model:{}:v{}:{}",
        id_component(&model.model_id),
        model.version,
        id_component(&model.artifact_hash)
    );
    // Relationship names are part of semantic identity. The default graph
    // builder intentionally collapses parallel (from, label, to) edges, so use
    // explicit edge ids and retain parallel semantic relationships here.
    let mut builder = Graph::builder().edge_policy(EdgePolicy::AllowDuplicates);
    let _ = builder
        .node("SemanticModel", model_node.clone())
        .prop("model_id", model.model_id.clone())
        .prop("version", model.version as i64)
        .prop("artifact_hash", model.artifact_hash.clone())
        .finish();
    for dataset in &model.datasets {
        let dataset_id = format!("{model_node}:dataset:{}", id_component(&dataset.name));
        let _ = builder
            .node("SemanticDataset", dataset_id.clone())
            .prop("name", dataset.name.clone())
            .prop("physical_source", dataset.physical_source.clone())
            .finish();
        let _ = builder
            .edge("CONTAINS_DATASET", model_node.clone(), dataset_id.clone())
            .id(format!(
                "{model_node}:contains-dataset:{}",
                id_component(&dataset.name)
            ))
            .finish();
        for field in &dataset.fields {
            let field_id = format!("{dataset_id}:field:{}", id_component(&field.name));
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
                .id(format!(
                    "{dataset_id}:contains-field:{}",
                    id_component(&field.name)
                ))
                .finish();
        }
    }
    for metric in &model.metrics {
        let metric_id = format!("{model_node}:metric:{}", id_component(&metric.name));
        let _ = builder
            .node("SemanticMetric", metric_id.clone())
            .prop("name", metric.name.clone())
            .prop("expression_hash", metric.expression_hash.clone())
            .finish();
        let _ = builder
            .edge("CONTAINS_METRIC", model_node.clone(), metric_id)
            .id(format!(
                "{model_node}:contains-metric:{}",
                id_component(&metric.name)
            ))
            .finish();
    }
    for relationship in &model.relationships {
        let from = format!(
            "{model_node}:dataset:{}",
            id_component(&relationship.from_dataset)
        );
        let to = format!(
            "{model_node}:dataset:{}",
            id_component(&relationship.to_dataset)
        );
        let _ = builder
            .edge("RELATES_DATASET", from, to)
            .id(format!(
                "{model_node}:relationship:{}",
                id_component(&relationship.name)
            ))
            .prop("name", relationship.name.clone())
            .finish();
    }
    Ok(builder.build())
}

fn id_component(value: &str) -> String {
    // A byte length prefix makes the concatenation injective even when names
    // contain colons or strings that resemble the structural separators.
    format!("{}:{value}", value.len())
}

fn serialization_error(message: impl Into<String>) -> GrustError {
    GrustError::Serialization(message.into())
}

fn validate_text(kind: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(serialization_error(format!(
            "semantic {kind} must be non-empty, trimmed, and contain no control characters"
        )));
    }
    Ok(())
}

fn validate_sha256(kind: &str, value: &str) -> Result<()> {
    let Some(digest) = value.strip_prefix("sha256:") else {
        return Err(serialization_error(format!(
            "semantic {kind} must use sha256:<64 hex digits>"
        )));
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(serialization_error(format!(
            "semantic {kind} must use sha256:<64 hex digits>"
        )));
    }
    Ok(())
}

fn validate_projection(model: &SemanticModelProjection) -> Result<()> {
    validate_text("model id", &model.model_id)?;
    if model.version == 0 || model.version > i64::MAX as u64 {
        return Err(serialization_error(
            "semantic model version must be in 1..=i64::MAX",
        ));
    }
    validate_sha256("artifact hash", &model.artifact_hash)?;

    let mut dataset_names = BTreeSet::new();
    for dataset in &model.datasets {
        validate_text("dataset name", &dataset.name)?;
        validate_text("dataset physical source", &dataset.physical_source)?;
        if !dataset_names.insert(dataset.name.as_str()) {
            return Err(serialization_error(format!(
                "duplicate semantic dataset name `{}`",
                dataset.name
            )));
        }
        let mut field_names = BTreeSet::new();
        for field in &dataset.fields {
            validate_text("field name", &field.name)?;
            if !field_names.insert(field.name.as_str()) {
                return Err(serialization_error(format!(
                    "duplicate semantic field name `{}` in dataset `{}`",
                    field.name, dataset.name
                )));
            }
            if let Some(data_type) = &field.data_type {
                validate_text("field data type", data_type)?;
            }
        }
    }

    let mut metric_names = BTreeSet::new();
    for metric in &model.metrics {
        validate_text("metric name", &metric.name)?;
        if !metric_names.insert(metric.name.as_str()) {
            return Err(serialization_error(format!(
                "duplicate semantic metric name `{}`",
                metric.name
            )));
        }
        validate_sha256(
            &format!("metric `{}` expression hash", metric.name),
            &metric.expression_hash,
        )?;
    }

    let mut relationship_names = BTreeSet::new();
    for relationship in &model.relationships {
        validate_text("relationship name", &relationship.name)?;
        if !relationship_names.insert(relationship.name.as_str()) {
            return Err(serialization_error(format!(
                "duplicate semantic relationship name `{}`",
                relationship.name
            )));
        }
        if !dataset_names.contains(relationship.from_dataset.as_str())
            || !dataset_names.contains(relationship.to_dataset.as_str())
        {
            return Err(serialization_error(format!(
                "semantic relationship {} references an unknown dataset",
                relationship.name
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha256(value: &str) -> String {
        format!("sha256:{}", value.repeat(64))
    }

    fn model() -> SemanticModelProjection {
        SemanticModelProjection {
            model_id: "tpcds".into(),
            version: 1,
            artifact_hash: sha256("a"),
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
                expression_hash: sha256("d"),
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
    fn parallel_relationships_keep_distinct_stable_identities() {
        let mut projection = model();
        projection.relationships.push(SemanticRelationship {
            name: "sales_date_fallback".into(),
            from_dataset: "sales".into(),
            to_dataset: "date".into(),
        });
        let graph = semantic_model_graph(&projection).unwrap();
        let relationships = graph
            .edges
            .iter()
            .filter(|edge| edge.label.as_str() == "RELATES_DATASET")
            .collect::<Vec<_>>();
        assert_eq!(relationships.len(), 2);
        assert_ne!(relationships[0].id, relationships[1].id);
    }

    #[test]
    fn duplicate_names_invalid_hashes_and_ambiguous_text_are_rejected() {
        let mut projection = model();
        projection.datasets.push(projection.datasets[0].clone());
        assert!(
            semantic_model_graph(&projection)
                .unwrap_err()
                .to_string()
                .contains("duplicate semantic dataset")
        );

        let mut projection = model();
        let duplicate_field = projection.datasets[0].fields[0].clone();
        projection.datasets[0].fields.push(duplicate_field);
        assert!(
            semantic_model_graph(&projection)
                .unwrap_err()
                .to_string()
                .contains("duplicate semantic field")
        );

        let mut projection = model();
        projection.artifact_hash = "sha256:not-a-digest".into();
        assert!(semantic_model_graph(&projection).is_err());

        let mut projection = model();
        let duplicate_metric = projection.metrics[0].clone();
        projection.metrics.push(duplicate_metric);
        assert!(semantic_model_graph(&projection).is_err());

        let mut projection = model();
        let duplicate_relationship = projection.relationships[0].clone();
        projection.relationships.push(duplicate_relationship);
        assert!(semantic_model_graph(&projection).is_err());

        let mut projection = model();
        projection.metrics[0].name = " revenue".into();
        assert!(semantic_model_graph(&projection).is_err());
    }

    #[test]
    fn length_prefixed_ids_do_not_collide_on_structural_delimiters() {
        let mut projection = model();
        projection.datasets.push(SemanticDataset {
            name: "sales:field:amount".into(),
            physical_source: "lake.encoded".into(),
            fields: vec![],
        });
        let graph = semantic_model_graph(&projection).unwrap();
        let ids = graph
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), graph.nodes.len());
    }

    #[derive(serde::Deserialize)]
    struct OssieDocument {
        version: String,
        semantic_model: Vec<OssieModel>,
    }

    #[derive(serde::Deserialize)]
    struct OssieModel {
        name: String,
        datasets: Vec<OssieDataset>,
        relationships: Vec<OssieRelationship>,
        metrics: Vec<OssieMetric>,
    }

    #[derive(serde::Deserialize)]
    struct OssieDataset {
        name: String,
        source: String,
        fields: Vec<OssieField>,
    }

    #[derive(serde::Deserialize)]
    struct OssieField {
        name: String,
        datatype: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct OssieRelationship {
        name: String,
        from: String,
        to: String,
    }

    #[derive(serde::Deserialize)]
    struct OssieMetric {
        name: String,
        expression: OssieExpression,
    }

    #[derive(serde::Deserialize)]
    struct OssieExpression {
        dialects: Vec<OssieDialectExpression>,
    }

    #[derive(serde::Deserialize)]
    struct OssieDialectExpression {
        expression: String,
    }

    fn digest(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};

        format!("sha256:{:x}", Sha256::digest(bytes))
    }

    #[test]
    fn pinned_apache_ossie_tpcds_fixture_parses_hashes_and_replays() {
        // Exact upstream bytes from apache/ossie commit
        // ddb19f1b135a61c65603f4823a3526e2fab00cf1,
        // examples/tpcds_semantic_model.yaml. The digest makes an accidental
        // fixture edit visible before projection behavior is considered.
        let bytes = include_bytes!("../tests/fixtures/apache-ossie-tpcds-ddb19f1b.yaml");
        let artifact_hash = digest(bytes);
        assert_eq!(
            artifact_hash,
            "sha256:bafbdc9d0e304ab22a40592f2b6bdfd45cc399c566533cd71343d33380c0d6e1"
        );
        let document: OssieDocument = serde_yaml::from_slice(bytes).unwrap();
        assert_eq!(document.version, "0.2.0.dev0");
        let [source] = document.semantic_model.as_slice() else {
            panic!("pinned Ossie fixture must contain exactly one semantic model");
        };

        let datasets = source
            .datasets
            .iter()
            .map(|dataset| SemanticDataset {
                name: dataset.name.clone(),
                physical_source: dataset.source.clone(),
                fields: dataset
                    .fields
                    .iter()
                    .map(|field| SemanticField {
                        name: field.name.clone(),
                        data_type: field.datatype.clone(),
                        nullable: None,
                    })
                    .collect(),
            })
            .collect();
        let metrics = source
            .metrics
            .iter()
            .map(|metric| SemanticMetric {
                name: metric.name.clone(),
                expression_hash: digest(
                    metric
                        .expression
                        .dialects
                        .first()
                        .expect("metric has an expression dialect")
                        .expression
                        .as_bytes(),
                ),
            })
            .collect();
        let relationships = source
            .relationships
            .iter()
            .map(|relationship| SemanticRelationship {
                name: relationship.name.clone(),
                from_dataset: relationship.from.clone(),
                to_dataset: relationship.to.clone(),
            })
            .collect();
        let projection = SemanticModelProjection {
            model_id: source.name.clone(),
            version: 1,
            artifact_hash,
            datasets,
            metrics,
            relationships,
        };
        let graph = semantic_model_graph(&projection).unwrap();
        assert_eq!(graph, semantic_model_graph(&projection).unwrap());
        assert_eq!(graph.nodes.len(), 42);
        assert_eq!(graph.edges.len(), 45);
    }
}
