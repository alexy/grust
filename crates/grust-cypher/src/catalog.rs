//! Portable catalog metadata derived from Cypher DDL registry state.

use crate::*;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NamedGraphCatalog {
    pub name: String,
    pub graph_type: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CypherCatalogSnapshot {
    pub graphs: Vec<NamedGraphCatalog>,
    pub graph_types: Vec<NamedGraphType>,
    pub indexes: Vec<NamedGraphIndex>,
    pub constraints: Vec<NamedGraphConstraint>,
    pub anonymous_constraint_count: usize,
}

impl CypherCatalogSnapshot {
    pub fn single_graph(name: impl Into<String>, registry: &CypherConstraintRegistry) -> Self {
        Self {
            graphs: vec![NamedGraphCatalog {
                name: name.into(),
                graph_type: registry
                    .named_graph_types()
                    .into_iter()
                    .next()
                    .map(|graph_type| graph_type.name),
            }],
            graph_types: registry.named_graph_types(),
            indexes: registry.named_indexes(),
            constraints: registry.named_constraints(),
            anonymous_constraint_count: registry.anonymous_constraints().len(),
        }
    }
}

pub fn cypher_catalog_procedure(
    catalog: &CypherCatalogSnapshot,
    procedure: &str,
) -> Result<CypherResultTable> {
    match procedure.to_ascii_lowercase().as_str() {
        "db.graphs" => Ok(graph_rows(catalog)),
        "db.graphtypes" => Ok(graph_type_rows(catalog)),
        "db.indexes" => Ok(index_rows(catalog)),
        "db.constraints" => Ok(constraint_rows(catalog)),
        other => Err(unsupported_gql_feature(
            GqlFeature::CatalogMetadata,
            GqlConformanceProfile::Full39075,
            format!(
                "catalog procedure `{other}` is not supported (known: db.graphs, db.graphTypes, db.indexes, db.constraints)"
            ),
        )),
    }
}

fn graph_rows(catalog: &CypherCatalogSnapshot) -> CypherResultTable {
    CypherResultTable {
        columns: vec!["graph".to_string(), "graphType".to_string()],
        rows: catalog
            .graphs
            .iter()
            .map(|graph| {
                vec![
                    Value::from(graph.name.clone()),
                    graph
                        .graph_type
                        .clone()
                        .map(Value::from)
                        .unwrap_or(Value::Null),
                ]
            })
            .collect(),
    }
}

fn graph_type_rows(catalog: &CypherCatalogSnapshot) -> CypherResultTable {
    let mut rows = Vec::new();
    for graph_type in &catalog.graph_types {
        for node in &graph_type.graph_type.schema.nodes {
            rows.push(vec![
                Value::from(graph_type.name.clone()),
                Value::from("node"),
                Value::from(node.label.as_str()),
                Value::Null,
                Value::Null,
                Value::from(node.fields.len()),
                Value::from(graph_type_mode_name(graph_type.graph_type.mode)),
            ]);
        }
        for edge in &graph_type.graph_type.schema.edges {
            rows.push(vec![
                Value::from(graph_type.name.clone()),
                Value::from("edge"),
                Value::from(edge.label.as_str()),
                Value::from(label_list(&edge.from)),
                Value::from(label_list(&edge.to)),
                Value::from(edge.fields.len()),
                Value::from(graph_type_mode_name(graph_type.graph_type.mode)),
            ]);
        }
    }
    CypherResultTable {
        columns: vec![
            "graphType".to_string(),
            "elementKind".to_string(),
            "label".to_string(),
            "fromLabels".to_string(),
            "toLabels".to_string(),
            "fieldCount".to_string(),
            "mode".to_string(),
        ],
        rows,
    }
}

fn index_rows(catalog: &CypherCatalogSnapshot) -> CypherResultTable {
    CypherResultTable {
        columns: vec![
            "index".to_string(),
            "elementKind".to_string(),
            "label".to_string(),
            "propertyKey".to_string(),
        ],
        rows: catalog
            .indexes
            .iter()
            .map(|index| {
                vec![
                    Value::from(index.name.clone()),
                    Value::from(match index.index.element {
                        GraphIndexElement::Node => "node",
                        GraphIndexElement::Edge => "edge",
                    }),
                    Value::from(index.index.label.as_str()),
                    Value::from(index.index.key.clone()),
                ]
            })
            .collect(),
    }
}

fn constraint_rows(catalog: &CypherCatalogSnapshot) -> CypherResultTable {
    CypherResultTable {
        columns: vec![
            "constraint".to_string(),
            "constraintKind".to_string(),
            "label".to_string(),
            "propertyKey".to_string(),
        ],
        rows: catalog
            .constraints
            .iter()
            .map(|constraint| {
                let (kind, label, key) = constraint_parts(&constraint.constraint);
                vec![
                    Value::from(constraint.name.clone()),
                    Value::from(kind),
                    Value::from(label.as_str()),
                    Value::from(key),
                ]
            })
            .collect(),
    }
}

fn constraint_parts(constraint: &GraphConstraint) -> (&'static str, &Label, &str) {
    match constraint {
        GraphConstraint::NodePropertyRequired { label, key } => {
            ("node-property-required", label, key)
        }
        GraphConstraint::EdgePropertyRequired { label, key } => {
            ("edge-property-required", label, key)
        }
        GraphConstraint::NodePropertyUnique { label, key } => ("node-property-unique", label, key),
        GraphConstraint::EdgePropertyUnique { label, key } => ("edge-property-unique", label, key),
    }
}

fn graph_type_mode_name(mode: GraphTypeMode) -> &'static str {
    match mode {
        GraphTypeMode::Open => "open",
        GraphTypeMode::Closed => "closed",
    }
}

fn label_list(labels: &[Label]) -> Vec<String> {
    labels
        .iter()
        .map(|label| label.as_str().to_string())
        .collect()
}
