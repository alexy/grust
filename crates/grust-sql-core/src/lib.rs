use grust_core::prelude::*;

#[derive(Clone, Debug)]
pub struct UniversalTableRefs {
    pub nodes: String,
    pub edges: String,
}

pub trait GraphSqlDialect {
    fn name(&self) -> &'static str;
    fn props_column_type(&self) -> &'static str;
    fn empty_props_default(&self) -> &'static str;
    fn node_props_select(&self, alias: &str) -> String;
    fn edge_props_select(&self, alias: &str) -> String;
    fn json_property_predicate(&self, alias: &str, key: &str, value: &Value) -> Result<String>;
    fn both_direction_join(
        &self,
        edges_table: &str,
        edge_alias: &str,
        prev_alias: &str,
        edge_label: &str,
    ) -> String;
    fn upsert_nodes_sql(&self, table: &str, nodes: &[Node]) -> Result<String>;
    fn upsert_edges_sql(&self, table: &str, edges: &[Edge]) -> Result<String>;
    fn patch_node_sql(&self, nodes_table: &str, id: &NodeId, props: &Props) -> Result<String>;

    fn begin_transaction(&self) -> &'static str {
        "BEGIN"
    }

    fn commit_transaction(&self) -> &'static str {
        "COMMIT"
    }

    fn create_view_prefix(&self) -> &'static str {
        "CREATE VIEW IF NOT EXISTS"
    }
}

pub fn universal_tables(prefix: &str, qualify: impl Fn(&str) -> String) -> UniversalTableRefs {
    UniversalTableRefs {
        nodes: qualify(&format!("{prefix}_nodes")),
        edges: qualify(&format!("{prefix}_edges")),
    }
}

pub fn universal_bootstrap_sql(
    dialect: &impl GraphSqlDialect,
    table_prefix: &str,
    tables: &UniversalTableRefs,
    prelude: Option<&str>,
    quote_ident: impl Fn(&str) -> String,
) -> String {
    let prelude = prelude
        .map(|prelude| format!("{};\n", prelude.trim().trim_end_matches(';')))
        .unwrap_or_default();
    format!(
        "{prelude}CREATE TABLE IF NOT EXISTS {nodes_table} (
            id text PRIMARY KEY,
            label text NOT NULL,
            props {props_type} NOT NULL DEFAULT {props_default}
         );
         CREATE TABLE IF NOT EXISTS {edges_table} (
            id text,
            from_id text NOT NULL REFERENCES {nodes_table}(id) ON DELETE CASCADE,
            to_id text NOT NULL REFERENCES {nodes_table}(id) ON DELETE CASCADE,
            label text NOT NULL,
            props {props_type} NOT NULL DEFAULT {props_default},
            PRIMARY KEY (from_id, label, to_id)
         );
         CREATE INDEX IF NOT EXISTS {edge_from_idx} ON {edges_table}(from_id);
         CREATE INDEX IF NOT EXISTS {edge_to_idx} ON {edges_table}(to_id);
         CREATE INDEX IF NOT EXISTS {node_label_idx} ON {nodes_table}(label);",
        nodes_table = tables.nodes,
        edges_table = tables.edges,
        props_type = dialect.props_column_type(),
        props_default = dialect.empty_props_default(),
        edge_from_idx = quote_ident(&format!("{table_prefix}_edges_from_idx")),
        edge_to_idx = quote_ident(&format!("{table_prefix}_edges_to_idx")),
        node_label_idx = quote_ident(&format!("{table_prefix}_nodes_label_idx")),
    )
}

pub fn select_node_sql(
    dialect: &impl GraphSqlDialect,
    nodes_table: &str,
    id: &NodeId,
    sql_str: impl Fn(&str) -> String,
) -> String {
    format!(
        "SELECT id, label, {} AS props FROM {nodes_table} WHERE id = {} LIMIT 1",
        dialect.node_props_select(""),
        sql_str(id.as_str())
    )
}

pub fn select_nodes_sql(
    dialect: &impl GraphSqlDialect,
    nodes_table: &str,
    ids: &[NodeId],
    sql_str: impl Fn(&str) -> String,
) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let ids = ids
        .iter()
        .map(|id| sql_str(id.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "SELECT id, label, {} AS props FROM {nodes_table} WHERE id IN ({ids})",
        dialect.node_props_select("")
    ))
}

pub fn select_edges_sql(
    dialect: &impl GraphSqlDialect,
    edges_table: &str,
    query: EdgeQuery,
    sql_str: impl Fn(&str) -> String,
) -> String {
    let mut conditions = Vec::new();
    if let Some(from) = query.from {
        conditions.push(format!("from_id = {}", sql_str(from.as_str())));
    }
    if let Some(to) = query.to {
        conditions.push(format!("to_id = {}", sql_str(to.as_str())));
    }
    if let Some(label) = query.label {
        conditions.push(format!("label = {}", sql_str(label.as_str())));
    }
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };
    format!(
        "SELECT id, from_id, to_id, label, {} AS props FROM {edges_table}{where_clause}",
        dialect.edge_props_select("")
    )
}

pub fn delete_node_sql(nodes_table: &str, id: &NodeId, sql_str: impl Fn(&str) -> String) -> String {
    format!(
        "DELETE FROM {nodes_table} WHERE id = {}",
        sql_str(id.as_str())
    )
}

pub fn delete_edge_sql(
    edges_table: &str,
    from: &NodeId,
    label: &Label,
    to: &NodeId,
    sql_str: impl Fn(&str) -> String,
) -> String {
    format!(
        "DELETE FROM {edges_table} WHERE from_id = {} AND label = {} AND to_id = {}",
        sql_str(from.as_str()),
        sql_str(label.as_str()),
        sql_str(to.as_str())
    )
}

pub fn mutation_sql(
    dialect: &impl GraphSqlDialect,
    nodes_table: &str,
    edges_table: &str,
    mutation: &GraphMutation,
    sql_str: impl Fn(&str) -> String + Copy,
) -> Result<String> {
    Ok(match mutation {
        GraphMutation::UpsertNode(node) => {
            dialect.upsert_nodes_sql(nodes_table, std::slice::from_ref(node))?
        }
        GraphMutation::PatchNode { id, props } => dialect.patch_node_sql(nodes_table, id, props)?,
        GraphMutation::DeleteNode(id) => delete_node_sql(nodes_table, id, sql_str),
        GraphMutation::UpsertEdge(edge) => {
            dialect.upsert_edges_sql(edges_table, std::slice::from_ref(edge))?
        }
        GraphMutation::DeleteEdge { from, label, to } => {
            delete_edge_sql(edges_table, from, label, to, sql_str)
        }
        GraphMutation::PatchMatchingNodes { .. } => {
            return unsupported_mutation(dialect.name(), "matched node patches");
        }
        GraphMutation::UpdateMatchingNodeProperty { .. } => {
            return unsupported_mutation(dialect.name(), "matched node expression updates");
        }
        GraphMutation::SetMatchingNodeFromNode { .. } => {
            return unsupported_mutation(dialect.name(), "cross-variable correlated updates");
        }
        GraphMutation::PatchEdge { .. } => {
            return unsupported_mutation(dialect.name(), "edge patches");
        }
        GraphMutation::PatchMatchingEdges { .. } => {
            return unsupported_mutation(dialect.name(), "matched edge patches");
        }
        GraphMutation::RemoveNodeProps { .. } => {
            return unsupported_mutation(dialect.name(), "node property removals");
        }
        GraphMutation::RemoveMatchingNodeProps { .. } => {
            return unsupported_mutation(dialect.name(), "matched node property removals");
        }
        GraphMutation::RemoveEdgeProps { .. } => {
            return unsupported_mutation(dialect.name(), "edge property removals");
        }
        GraphMutation::UpdateMatchingEdgeProperty { .. } => {
            return unsupported_mutation(dialect.name(), "matched edge property updates");
        }
        GraphMutation::RemoveMatchingEdgeProps { .. } => {
            return unsupported_mutation(dialect.name(), "matched edge property removals");
        }
        GraphMutation::DeleteMatchingNodes { .. } => {
            return unsupported_mutation(dialect.name(), "matched node deletes");
        }
        GraphMutation::UpsertEdgesFromNodeMatches { .. } => {
            return unsupported_mutation(dialect.name(), "row-producing edge upserts");
        }
        GraphMutation::DeleteMatchingEdges { .. } => {
            return unsupported_mutation(dialect.name(), "matched edge deletes");
        }
        GraphMutation::DeleteRelationshipRows { .. } => {
            return unsupported_mutation(dialect.name(), "row-producing relationship deletes");
        }
    })
}

pub fn apply_mutations_sql(
    dialect: &impl GraphSqlDialect,
    nodes_table: &str,
    edges_table: &str,
    mutations: &[GraphMutation],
    sql_str: impl Fn(&str) -> String + Copy,
) -> Result<String> {
    let mut statements = vec![dialect.begin_transaction().to_string()];
    for mutation in mutations {
        statements.push(mutation_sql(
            dialect,
            nodes_table,
            edges_table,
            mutation,
            sql_str,
        )?);
    }
    statements.push(dialect.commit_transaction().to_string());
    Ok(join_statements(statements))
}

pub fn traversal_sql(
    dialect: &impl GraphSqlDialect,
    nodes_table: &str,
    edges_table: &str,
    traversal: &Traversal,
    sql_str: impl Fn(&str) -> String + Copy,
) -> Result<String> {
    if traversal.steps.is_empty() {
        let where_clause = start_where_clause(dialect, &traversal.start, "n0", sql_str)?;
        return Ok(format!(
            "SELECT n0.id, n0.label, {} AS props FROM {nodes_table} n0{where_clause}{}",
            dialect.node_props_select("n0"),
            limit_clause(traversal.limit)
        ));
    }

    let mut joins = Vec::new();
    for (idx, step) in traversal.steps.iter().enumerate() {
        let prev = format!("n{idx}");
        let edge = format!("e{idx}");
        let next = format!("n{}", idx + 1);
        let edge_label = step
            .edge
            .as_ref()
            .map(|label| format!(" AND {edge}.label = {}", sql_str(label.as_str())))
            .unwrap_or_default();
        let node_label = step
            .node
            .as_ref()
            .map(|label| format!(" AND {next}.label = {}", sql_str(label.as_str())))
            .unwrap_or_default();

        match step.direction {
            Direction::Out => {
                joins.push(format!(
                    "JOIN {edges_table} {edge} ON {edge}.from_id = {prev}.id{edge_label}"
                ));
                joins.push(format!(
                    "JOIN {nodes_table} {next} ON {next}.id = {edge}.to_id{node_label}"
                ));
            }
            Direction::In => {
                joins.push(format!(
                    "JOIN {edges_table} {edge} ON {edge}.to_id = {prev}.id{edge_label}"
                ));
                joins.push(format!(
                    "JOIN {nodes_table} {next} ON {next}.id = {edge}.from_id{node_label}"
                ));
            }
            Direction::Both => {
                joins.push(dialect.both_direction_join(edges_table, &edge, &prev, &edge_label));
                joins.push(format!(
                    "JOIN {nodes_table} {next} ON {next}.id = {edge}.next_id{node_label}"
                ));
            }
        }
    }

    let last = format!("n{}", traversal.steps.len());
    Ok(format!(
        "SELECT {last}.id, {last}.label, {} AS props
         FROM {nodes_table} n0
         {}
         {}{}",
        dialect.node_props_select(&last),
        joins.join(" "),
        start_where_clause(dialect, &traversal.start, "n0", sql_str)?,
        limit_clause(traversal.limit)
    ))
}

/// Physical names used while rendering graph-schema views and indexes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GraphSqlSchemaLayout<'a> {
    /// Prefix for generated views and indexes.
    pub table_prefix: &'a str,
    /// SQL reference to the universal node table.
    pub nodes_table: &'a str,
    /// SQL reference to the universal edge table.
    pub edges_table: &'a str,
}

pub fn schema_sql(
    dialect: &impl GraphSqlDialect,
    layout: GraphSqlSchemaLayout<'_>,
    schema: &GraphSchema,
    view_ref: impl Fn(&str) -> String,
    quote_ident: impl Fn(&str) -> String + Copy,
    sql_str: impl Fn(&str) -> String + Copy,
    typed_expr: impl Fn(&Field) -> String + Copy,
) -> Result<String> {
    let GraphSqlSchemaLayout {
        table_prefix,
        nodes_table,
        edges_table,
    } = layout;
    let mut statements = Vec::new();

    for node_type in &schema.nodes {
        let view_name = format!(
            "{table_prefix}_node_{}",
            schema_identifier(node_type.label.as_str())?
        );
        let view = view_ref(&view_name);
        let columns = typed_columns(&node_type.fields, quote_ident, typed_expr);
        statements.push(format!(
            "{} {view} AS
             SELECT id{columns}
             FROM {nodes_table}
             WHERE label = {};",
            dialect.create_view_prefix(),
            sql_str(node_type.label.as_str())
        ));

        for field in &node_type.fields {
            statements.push(format!(
                "CREATE INDEX IF NOT EXISTS {} ON {nodes_table} (({})) WHERE label = {};",
                quote_ident(&format!(
                    "{table_prefix}_node_{}_{}_idx",
                    schema_identifier(node_type.label.as_str())?,
                    schema_identifier(&field.name)?
                )),
                typed_expr(field),
                sql_str(node_type.label.as_str())
            ));
        }
    }

    for edge_type in &schema.edges {
        let view_name = format!(
            "{table_prefix}_edge_{}",
            schema_identifier(edge_type.label.as_str())?
        );
        let view = view_ref(&view_name);
        let columns = typed_columns(&edge_type.fields, quote_ident, typed_expr);
        statements.push(format!(
            "{} {view} AS
             SELECT id, from_id, to_id{columns}
             FROM {edges_table}
             WHERE label = {};",
            dialect.create_view_prefix(),
            sql_str(edge_type.label.as_str())
        ));

        for field in &edge_type.fields {
            statements.push(format!(
                "CREATE INDEX IF NOT EXISTS {} ON {edges_table} (({})) WHERE label = {};",
                quote_ident(&format!(
                    "{table_prefix}_edge_{}_{}_idx",
                    schema_identifier(edge_type.label.as_str())?,
                    schema_identifier(&field.name)?
                )),
                typed_expr(field),
                sql_str(edge_type.label.as_str())
            ));
        }
    }

    Ok(statements.join("\n"))
}

pub fn props_to_json(props: &Props) -> Result<String> {
    serde_json::to_string(props).map_err(|err| GrustError::Serialization(err.to_string()))
}

pub fn quote_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

pub fn sql_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub fn validate_identifier(dialect: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        || value.chars().next().is_some_and(|ch| ch.is_ascii_digit())
    {
        return Err(GrustError::Schema(format!(
            "invalid {dialect} identifier '{value}'"
        )));
    }
    Ok(())
}

pub fn json_path_key(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn join_statements(statements: Vec<String>) -> String {
    statements
        .into_iter()
        .map(|statement| statement.trim().trim_end_matches(';').to_string())
        .filter(|statement| !statement.is_empty())
        .collect::<Vec<_>>()
        .join(";\n")
}

fn start_where_clause(
    dialect: &impl GraphSqlDialect,
    start: &Start,
    alias: &str,
    sql_str: impl Fn(&str) -> String,
) -> Result<String> {
    match start {
        Start::Node(id) => Ok(format!(" WHERE {alias}.id = {}", sql_str(id.as_str()))),
        Start::NodesByLabel(label) => Ok(format!(
            " WHERE {alias}.label = {}",
            sql_str(label.as_str())
        )),
        Start::NodesByProperty { label, key, value } => Ok(format!(
            " WHERE {alias}.label = {} AND {}",
            sql_str(label.as_str()),
            dialect.json_property_predicate(alias, key, value)?
        )),
    }
}

fn typed_columns(
    fields: &[Field],
    quote_ident: impl Fn(&str) -> String,
    typed_expr: impl Fn(&Field) -> String,
) -> String {
    let columns = fields
        .iter()
        .map(|field| format!("{} AS {}", typed_expr(field), quote_ident(&field.name)))
        .collect::<Vec<_>>()
        .join(",\n            ");
    if columns.is_empty() {
        String::new()
    } else {
        format!(",\n            {columns}")
    }
}

fn unsupported_mutation<T>(dialect: &str, name: &str) -> Result<T> {
    Err(GrustError::Unsupported(format!(
        "{dialect} {name} are not implemented yet"
    )))
}

fn limit_clause(limit: Option<u32>) -> String {
    limit
        .map(|limit| format!(" LIMIT {limit}"))
        .unwrap_or_default()
}
