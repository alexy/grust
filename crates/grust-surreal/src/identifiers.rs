use std::collections::BTreeMap;

use grust_core::prelude::*;

use super::SurrealConfig;

const BASE_TABLE: &str = "record";
const NODE_STORAGE_FIELDS: &[&str] = &["id", "labels", "__grust_label", "__grust_physical_label"];
const EDGE_STORAGE_FIELDS: &[&str] = &[
    "id",
    "in",
    "out",
    "relationship",
    "edge_id",
    "__grust_label",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TableKind {
    Base,
    Node,
    Relationship,
}

impl TableKind {
    fn description(self) -> &'static str {
        match self {
            Self::Base => "base table",
            Self::Node => "node label",
            Self::Relationship => "relationship label",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TableClaim {
    kind: TableKind,
    logical: String,
}

#[derive(Debug)]
struct TableClaims {
    claims: BTreeMap<String, TableClaim>,
}

impl TableClaims {
    fn new() -> Self {
        Self {
            claims: BTreeMap::from([(
                BASE_TABLE.to_string(),
                TableClaim {
                    kind: TableKind::Base,
                    logical: BASE_TABLE.to_string(),
                },
            )]),
        }
    }

    fn claim(&mut self, kind: TableKind, logical: &str, allow_exact_repeat: bool) -> Result<()> {
        validate_table_source(kind, logical)?;
        let physical = physical_table_name(kind, logical);
        let claim = TableClaim {
            kind,
            logical: logical.to_string(),
        };
        if let Some(existing) = self.claims.get(&physical) {
            if allow_exact_repeat && existing == &claim {
                return Ok(());
            }
            return Err(GrustError::Schema(format!(
                "SurrealDB {} '{}' and {} '{}' both map to table '{}'",
                existing.kind.description(),
                existing.logical,
                kind.description(),
                logical,
                physical
            )));
        }
        self.claims.insert(physical, claim);
        Ok(())
    }

    fn merge_compatible(&mut self, other: Self) -> Result<()> {
        for (physical, claim) in other.claims {
            if claim.kind == TableKind::Base {
                continue;
            }
            if let Some(existing) = self.claims.get(&physical) {
                if existing == &claim {
                    continue;
                }
                return Err(GrustError::Schema(format!(
                    "SurrealDB {} '{}' and {} '{}' both map to table '{}'",
                    existing.kind.description(),
                    existing.logical,
                    claim.kind.description(),
                    claim.logical,
                    physical
                )));
            }
            self.claims.insert(physical, claim);
        }
        Ok(())
    }

    fn claim_resolved_node_table(&mut self, physical: &str) -> Result<()> {
        if let Some(existing) = self.claims.get(physical) {
            if existing.kind == TableKind::Node {
                return Ok(());
            }
            return Err(GrustError::Schema(format!(
                "SurrealDB {} '{}' and inferred node endpoint both map to table '{}'",
                existing.kind.description(),
                existing.logical,
                physical
            )));
        }
        self.claims.insert(
            physical.to_string(),
            TableClaim {
                kind: TableKind::Node,
                logical: physical.to_string(),
            },
        );
        Ok(())
    }
}

pub(crate) fn validate_surreal_config(config: &SurrealConfig) -> Result<()> {
    validate_scope_name("namespace", &config.namespace)?;
    validate_scope_name("database", &config.database)?;
    validated_surreal_url(&config.url)?;
    config_table_claims(config).map(|_| ())
}

pub(crate) fn validate_schema_for_config(
    config: &SurrealConfig,
    schema: &GraphSchema,
) -> Result<()> {
    validate_surreal_config(config)?;
    let mut claims = config_table_claims(config)?;
    claims.merge_compatible(schema_table_claims(schema)?)?;
    validate_schema_fields(schema)
}

pub(crate) fn validate_schema(schema: &GraphSchema) -> Result<()> {
    schema_table_claims(schema)?;
    validate_schema_fields(schema)
}

pub(crate) fn validate_node_write(config: &SurrealConfig, node: &Node) -> Result<()> {
    let mut claims = config_table_claims(config)?;
    claims.claim(TableKind::Node, node.label.as_str(), true)?;
    validate_node_props(node)
}

pub(crate) fn validate_edge_write(config: &SurrealConfig, edge: &Edge) -> Result<()> {
    let mut claims = config_table_claims(config)?;
    claims.claim(TableKind::Relationship, edge.label.as_str(), true)?;
    claim_node_id_prefix(&mut claims, &edge.from)?;
    claim_node_id_prefix(&mut claims, &edge.to)?;
    validate_props(
        "edge",
        edge.label.as_str(),
        &edge.props,
        EDGE_STORAGE_FIELDS,
    )
}

pub(crate) fn validate_node_ids(config: &SurrealConfig, ids: &[NodeId]) -> Result<()> {
    validate_surreal_config(config)?;
    let mut claims = config_table_claims(config)?;
    for id in ids {
        claim_node_id_prefix(&mut claims, id)?;
    }
    Ok(())
}

pub(crate) fn validate_node_start(config: &SurrealConfig, start: &Start) -> Result<()> {
    validate_surreal_config(config)?;
    let mut claims = config_table_claims(config)?;
    match start {
        Start::Node(id) => claim_node_id_prefix(&mut claims, id),
        Start::NodesByLabel(label) => claims.claim(TableKind::Node, label.as_str(), true),
        Start::NodesByProperty { label, key, .. } => {
            claims.claim(TableKind::Node, label.as_str(), true)?;
            validate_field_name(key)
        }
    }
}

pub(crate) fn validate_edge_read(config: &SurrealConfig, query: &EdgeQuery) -> Result<()> {
    validate_surreal_config(config)?;
    let mut claims = config_table_claims(config)?;
    if let Some(label) = &query.label {
        claims.claim(TableKind::Relationship, label.as_str(), true)?;
    }
    Ok(())
}

pub(crate) fn validate_edge_delete(
    config: &SurrealConfig,
    from: &NodeId,
    label: &Label,
    to: &NodeId,
) -> Result<()> {
    validate_surreal_config(config)?;
    let mut claims = config_table_claims(config)?;
    claims.claim(TableKind::Relationship, label.as_str(), true)?;
    claim_node_id_prefix(&mut claims, from)?;
    claim_node_id_prefix(&mut claims, to)
}

pub(crate) fn validate_graph_write(config: &SurrealConfig, graph: &Graph) -> Result<()> {
    let mut claims = config_table_claims(config)?;
    let mut id_tables = BTreeMap::new();
    for node in &graph.nodes {
        claims.claim(TableKind::Node, node.label.as_str(), true)?;
        validate_node_props(node)?;
        let table = physical_table_name(TableKind::Node, node.label.as_str());
        if let Some(existing) = id_tables.insert(node.id.as_str().to_string(), table.clone())
            && existing != table
        {
            return Err(GrustError::Schema(format!(
                "SurrealDB node id '{}' is claimed by tables '{}' and '{}'",
                node.id.as_str(),
                existing,
                table
            )));
        }
    }
    for edge in &graph.edges {
        claims.claim(TableKind::Relationship, edge.label.as_str(), true)?;
        claim_resolved_endpoint(&mut claims, &edge.from, id_tables.get(edge.from.as_str()))?;
        claim_resolved_endpoint(&mut claims, &edge.to, id_tables.get(edge.to.as_str()))?;
        validate_props(
            "edge",
            edge.label.as_str(),
            &edge.props,
            EDGE_STORAGE_FIELDS,
        )?;
    }
    Ok(())
}

pub(crate) fn validate_node_batch(nodes: &[Node]) -> Result<()> {
    let mut claims = TableClaims::new();
    for node in nodes {
        claims.claim(TableKind::Node, node.label.as_str(), true)?;
        validate_node_props(node)?;
    }
    Ok(())
}

pub(crate) fn validate_edge_batch(edges: &[Edge]) -> Result<()> {
    let mut claims = TableClaims::new();
    for edge in edges {
        claims.claim(TableKind::Relationship, edge.label.as_str(), true)?;
        validate_props(
            "edge",
            edge.label.as_str(),
            &edge.props,
            EDGE_STORAGE_FIELDS,
        )?;
    }
    Ok(())
}

pub(crate) fn validate_resolved_edge_batch(
    config: &SurrealConfig,
    edges: &[Edge],
    id_tables: &BTreeMap<String, String>,
) -> Result<()> {
    let mut claims = config_table_claims(config)?;
    for edge in edges {
        claims.claim(TableKind::Relationship, edge.label.as_str(), true)?;
        claim_resolved_endpoint(&mut claims, &edge.from, id_tables.get(edge.from.as_str()))?;
        claim_resolved_endpoint(&mut claims, &edge.to, id_tables.get(edge.to.as_str()))?;
        validate_props(
            "edge",
            edge.label.as_str(),
            &edge.props,
            EDGE_STORAGE_FIELDS,
        )?;
    }
    Ok(())
}

pub(crate) fn validate_node_patch(props: &Props) -> Result<()> {
    validate_props("node", "patch", props, NODE_STORAGE_FIELDS)
}

pub(crate) fn validate_mutations_write(
    config: &SurrealConfig,
    mutations: &[GraphMutation],
) -> Result<()> {
    let mut claims = config_table_claims(config)?;
    for mutation in mutations {
        match mutation {
            GraphMutation::UpsertNode(node) => {
                claims.claim(TableKind::Node, node.label.as_str(), true)?;
                validate_node_props(node)?;
            }
            GraphMutation::PatchNode { id, props } => {
                claim_node_id_prefix(&mut claims, id)?;
                validate_props("node", "patch", props, NODE_STORAGE_FIELDS)?;
            }
            GraphMutation::UpsertEdge(edge) => {
                claims.claim(TableKind::Relationship, edge.label.as_str(), true)?;
                claim_node_id_prefix(&mut claims, &edge.from)?;
                claim_node_id_prefix(&mut claims, &edge.to)?;
                validate_props(
                    "edge",
                    edge.label.as_str(),
                    &edge.props,
                    EDGE_STORAGE_FIELDS,
                )?;
            }
            GraphMutation::DeleteEdge { from, label, to } => {
                claims.claim(TableKind::Relationship, label.as_str(), true)?;
                claim_node_id_prefix(&mut claims, from)?;
                claim_node_id_prefix(&mut claims, to)?;
            }
            GraphMutation::DeleteNode(id) => {
                claim_node_id_prefix(&mut claims, id)?;
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn validated_surreal_url(value: &str) -> Result<url::Url> {
    let parsed = url::Url::parse(value)
        .map_err(|error| GrustError::Schema(format!("invalid SurrealDB URL: {error}")))?;
    if parsed.host_str().is_none() {
        return Err(GrustError::Schema(
            "invalid SurrealDB URL: a host is required".to_string(),
        ));
    }
    Ok(parsed)
}

fn config_table_claims(config: &SurrealConfig) -> Result<TableClaims> {
    let mut claims = TableClaims::new();
    for label in &config.labels {
        claims.claim(TableKind::Node, label, true)?;
    }
    for label in &config.relationships {
        claims.claim(TableKind::Relationship, label, true)?;
    }
    Ok(claims)
}

fn schema_table_claims(schema: &GraphSchema) -> Result<TableClaims> {
    let mut claims = TableClaims::new();
    for node in &schema.nodes {
        claims.claim(TableKind::Node, node.label.as_str(), false)?;
    }
    for edge in &schema.edges {
        claims.claim(TableKind::Relationship, edge.label.as_str(), false)?;
    }
    Ok(claims)
}

fn claim_node_id_prefix(claims: &mut TableClaims, id: &NodeId) -> Result<()> {
    if let Some((prefix, _)) = id.as_str().split_once(':') {
        validate_table_source(TableKind::Node, prefix)?;
        claims.claim_resolved_node_table(&physical_table_name(TableKind::Node, prefix))?;
    }
    Ok(())
}

fn claim_resolved_endpoint(
    claims: &mut TableClaims,
    id: &NodeId,
    resolved_table: Option<&String>,
) -> Result<()> {
    if let Some(resolved) = resolved_table {
        return claims.claim_resolved_node_table(resolved);
    }
    claim_node_id_prefix(claims, id)
}

fn validate_schema_fields(schema: &GraphSchema) -> Result<()> {
    for node in &schema.nodes {
        validate_fields(
            "node",
            node.label.as_str(),
            &node.fields,
            NODE_STORAGE_FIELDS,
        )?;
    }
    for edge in &schema.edges {
        validate_fields(
            "edge",
            edge.label.as_str(),
            &edge.fields,
            EDGE_STORAGE_FIELDS,
        )?;
    }
    Ok(())
}

fn validate_fields(kind: &str, label: &str, fields: &[Field], reserved: &[&str]) -> Result<()> {
    let mut names = BTreeMap::new();
    for field in fields {
        validate_field_name(&field.name)?;
        reject_reserved(kind, label, &field.name, reserved)?;
        if let Some(existing) = names.insert(field.name.clone(), field.name.clone()) {
            return Err(GrustError::Schema(format!(
                "SurrealDB {kind} '{label}' defines duplicate field '{existing}'"
            )));
        }
    }
    Ok(())
}

fn validate_props(kind: &str, label: &str, props: &Props, reserved: &[&str]) -> Result<()> {
    for key in props.keys() {
        validate_field_name(key)?;
        reject_reserved(kind, label, key, reserved)?;
    }
    Ok(())
}

fn validate_node_props(node: &Node) -> Result<()> {
    for (key, value) in &node.props {
        if key == "id" {
            if value == &Value::from(node.id.as_str()) {
                continue;
            }
            return Err(GrustError::Schema(format!(
                "SurrealDB node '{}' property 'id' conflicts with reserved storage field 'id' (record id '{}')",
                node.label.as_str(),
                node.id.as_str()
            )));
        }
        validate_field_name(key)?;
        reject_reserved("node", node.label.as_str(), key, NODE_STORAGE_FIELDS)?;
    }
    Ok(())
}

fn physical_table_name(kind: TableKind, logical: &str) -> String {
    match kind {
        TableKind::Base | TableKind::Node => surreal_table_name(logical),
        TableKind::Relationship => surreal_table_name(&relationship_type(logical)),
    }
}

fn reject_reserved(kind: &str, label: &str, key: &str, reserved: &[&str]) -> Result<()> {
    if let Some(storage_field) = reserved
        .iter()
        .find(|storage_field| key.eq_ignore_ascii_case(storage_field))
    {
        return Err(GrustError::Schema(format!(
            "SurrealDB {kind} '{label}' property '{key}' conflicts with reserved storage field '{storage_field}'"
        )));
    }
    Ok(())
}

fn validate_scope_name(kind: &str, value: &str) -> Result<()> {
    validate_name(kind, value)
}

fn validate_table_source(kind: TableKind, value: &str) -> Result<()> {
    validate_name(kind.description(), value)
}

fn validate_field_name(value: &str) -> Result<()> {
    validate_name("field", value)
}

fn validate_name(kind: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(GrustError::Schema(format!(
            "invalid SurrealDB {kind} name {value:?}"
        )));
    }
    Ok(())
}

/// Quote a SurrealQL identifier exactly. SurrealDB 3.2 accepts backticks for
/// literal field names; escaping mirrors its `EscapeSqonIdent` formatter.
pub(crate) fn surreal_identifier(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('`');
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '`' => escaped.push_str("\\`"),
            _ => escaped.push(character),
        }
    }
    escaped.push('`');
    escaped
}

/// Preserve Grust's existing lowercase table mapping while validation rejects
/// any two logical labels that would collapse to the same physical table.
pub(crate) fn surreal_table_name(value: &str) -> String {
    let table = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    if table.is_empty() {
        "related_to".to_string()
    } else {
        table
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_quoting_is_lossless_and_escapes_delimiters() {
        assert_eq!(surreal_identifier("display-name"), "`display-name`");
        assert_eq!(surreal_identifier("1st value"), "`1st value`");
        assert_eq!(surreal_identifier("select"), "`select`");
        assert_eq!(
            surreal_identifier("x`; DELETE person"),
            "`x\\`; DELETE person`"
        );
        assert_eq!(surreal_identifier(r"path\key"), r"`path\\key`");
    }

    #[test]
    fn config_rejects_normalized_table_collisions() {
        let collision = SurrealConfig {
            labels: vec!["Person-Role".to_string(), "Person_Role".to_string()],
            ..SurrealConfig::default()
        };
        let error = validate_surreal_config(&collision)
            .expect_err("lossy table normalization must not alias labels");
        assert!(
            error
                .to_string()
                .contains("both map to table 'person_role'")
        );

        let cross_kind = SurrealConfig {
            labels: vec!["Membership".to_string()],
            relationships: vec!["membership".to_string()],
            ..SurrealConfig::default()
        };
        assert!(validate_surreal_config(&cross_kind).is_err());

        let relationship_collision = SurrealConfig {
            relationships: vec!["member-of".to_string(), "member_of".to_string()],
            ..SurrealConfig::default()
        };
        let error = validate_surreal_config(&relationship_collision)
            .expect_err("relationship claims must use the runtime physical mapping");
        assert!(error.to_string().contains("table 'member_of'"));
        assert_eq!(
            physical_table_name(TableKind::Relationship, "memberOf"),
            surreal_table_name(&relationship_type("memberOf"))
        );
    }

    #[test]
    fn config_rejects_the_fixed_base_table_and_invalid_scopes() {
        let fixed = SurrealConfig {
            labels: vec!["RECORD".to_string()],
            ..SurrealConfig::default()
        };
        assert!(validate_surreal_config(&fixed).is_err());

        for (namespace, database) in [("", "graph"), ("test", "bad\nname")] {
            let config = SurrealConfig {
                namespace: namespace.to_string(),
                database: database.to_string(),
                ..SurrealConfig::default()
            };
            assert!(validate_surreal_config(&config).is_err());
        }
    }
}
