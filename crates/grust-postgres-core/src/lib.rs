mod sql_safety;

use async_trait::async_trait;
use grust_core::prelude::*;
use grust_cypher::pushdown::{NoTypeHints, SqlDialect, StrOp, combine_union, plan_read};
use grust_cypher::{CypherParameters, CypherResultTable};
use grust_sql_core::{GraphSqlDialect, UniversalTableRefs};
pub use sql_safety::POSTGRES_IDENTIFIER_MAX_BYTES;
use sql_safety::{
    validate_autocommit_sql, validate_identifier_length, validate_typed_column_alias,
};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::task::JoinHandle;
use tokio_postgres::{Client, NoTls};

#[derive(Clone, Debug)]
pub struct PostgresGraphConfig {
    pub connection_string: String,
    pub schema: String,
    pub table_prefix: String,
    pub batch_size: usize,
}

impl Default for PostgresGraphConfig {
    fn default() -> Self {
        Self {
            connection_string: "host=127.0.0.1 user=postgres dbname=graph".to_string(),
            schema: "public".to_string(),
            table_prefix: "grust".to_string(),
            batch_size: 500,
        }
    }
}

#[derive(Debug)]
pub struct PostgresGraphStore {
    config: PostgresGraphConfig,
    client: Client,
    /// A PostgreSQL `Client` multiplexes one connection. Keep explicit
    /// transactions isolated so concurrent store calls cannot accidentally
    /// become part of the same transaction.
    connection_gate: tokio::sync::Mutex<()>,
    /// Set before an explicit transaction begins and cleared only after its
    /// commit or rollback finishes. If a future is cancelled while the gate is
    /// held, the next caller recovers the abandoned transaction first.
    transaction_needs_rollback: AtomicBool,
    connection_task: JoinHandle<()>,
}

impl PostgresGraphStore {
    pub async fn connect(config: PostgresGraphConfig) -> Result<Self> {
        validate_postgres_config(&config)?;
        let (client, connection) = tokio_postgres::connect(&config.connection_string, NoTls)
            .await
            .map_err(|err| {
                GrustError::Backend(format!(
                    "failed to connect to PostgreSQL for PostgreSQL backend: {err}"
                ))
            })?;
        let connection_task = tokio::spawn(async move {
            if let Err(err) = connection.await {
                eprintln!("grust-postgres PostgreSQL connection task ended: {err}");
            }
        });
        Ok(Self {
            config,
            client,
            connection_gate: tokio::sync::Mutex::new(()),
            transaction_needs_rollback: AtomicBool::new(false),
            connection_task,
        })
    }

    pub fn config(&self) -> &PostgresGraphConfig {
        &self.config
    }

    /// Execute SQL without allowing callers to manage this shared connection's
    /// transaction state. Explicit transaction-control statements are rejected
    /// before the SQL is sent to PostgreSQL.
    pub async fn execute(&self, sql: &str) -> Result<()> {
        validate_autocommit_sql(sql)?;
        let _gate = self.lock_connection().await?;
        self.execute_unlocked(sql).await
    }

    async fn lock_connection(&self) -> Result<tokio::sync::MutexGuard<'_, ()>> {
        let gate = self.connection_gate.lock().await;
        if self.transaction_needs_rollback.load(Ordering::Acquire) {
            // Cancellation can happen while BEGIN, a statement, or COMMIT is
            // in flight. PostgreSQL processes this rollback after any earlier
            // request on the same connection; ROLLBACK outside a transaction
            // is harmless if COMMIT already won the race.
            self.rollback_transaction_unlocked().await?;
        }
        Ok(gate)
    }

    /// Resolve the explicit transaction and clear the recovery marker only
    /// after PostgreSQL acknowledges the rollback. A failed rollback leaves
    /// the marker set so later callers cannot silently join stale state.
    async fn rollback_transaction_unlocked(&self) -> Result<()> {
        self.execute_unlocked("ROLLBACK").await?;
        self.transaction_needs_rollback
            .store(false, Ordering::Release);
        Ok(())
    }

    async fn execute_unlocked(&self, sql: &str) -> Result<()> {
        self.client
            .batch_execute(sql)
            .await
            .map_err(|err| GrustError::Backend(format!("PostgreSQL command failed: {err}: {sql}")))
    }

    async fn query_nodes(&self, sql: &str) -> Result<Vec<Node>> {
        let _gate = self.lock_connection().await?;
        self.query_nodes_unlocked(sql).await
    }

    async fn query_nodes_unlocked(&self, sql: &str) -> Result<Vec<Node>> {
        let rows = self.client.query(sql, &[]).await.map_err(|err| {
            GrustError::Backend(format!("PostgreSQL node query failed: {err}: {sql}"))
        })?;
        rows.into_iter().map(row_to_node).collect()
    }

    async fn query_edges(&self, sql: &str) -> Result<Vec<Edge>> {
        let _gate = self.lock_connection().await?;
        self.query_edges_unlocked(sql).await
    }

    async fn query_edges_unlocked(&self, sql: &str) -> Result<Vec<Edge>> {
        let rows = self.client.query(sql, &[]).await.map_err(|err| {
            GrustError::Backend(format!("PostgreSQL edge query failed: {err}: {sql}"))
        })?;
        rows.into_iter().map(row_to_edge).collect()
    }

    async fn execute_transaction(&self, statements: &[String]) -> Result<()> {
        if statements.is_empty() {
            return Ok(());
        }
        let _gate = self.lock_connection().await?;
        self.transaction_needs_rollback
            .store(true, Ordering::Release);
        self.execute_unlocked("BEGIN").await?;
        for statement in statements {
            if let Err(err) = self.execute_unlocked(statement).await {
                if let Err(recovery_err) = self.rollback_transaction_unlocked().await {
                    return Err(GrustError::Backend(format!(
                        "{err}; PostgreSQL transaction recovery failed: {recovery_err}"
                    )));
                }
                return Err(err);
            }
        }
        if let Err(err) = self.execute_unlocked("COMMIT").await {
            if let Err(recovery_err) = self.rollback_transaction_unlocked().await {
                return Err(GrustError::Backend(format!(
                    "{err}; PostgreSQL transaction recovery failed: {recovery_err}"
                )));
            }
            return Err(err);
        }
        self.transaction_needs_rollback
            .store(false, Ordering::Release);
        Ok(())
    }

    fn tables(&self) -> UniversalTableRefs {
        UniversalTableRefs {
            nodes: self.nodes_table(),
            edges: self.edges_table(),
        }
    }

    pub fn nodes_table(&self) -> String {
        qualified_table(
            &self.config.schema,
            &format!("{}_nodes", self.config.table_prefix),
        )
    }

    pub fn edges_table(&self) -> String {
        qualified_table(
            &self.config.schema,
            &format!("{}_edges", self.config.table_prefix),
        )
    }
}

// ---------------------------------------------------------------------------
// Portable Cypher execution (GQL_POSTGRES_EXECUTOR_GOAL: Q1-Q3)
// ---------------------------------------------------------------------------

/// The read-pushdown [`SqlDialect`] for the PostgreSQL universal tables:
/// **tagged jsonb** props (each property is `{"type": t, "value": v}`, so
/// scalar extraction is `props #>> ARRAY['key','value']`, yielding text like
/// Spark's `GET_JSON_OBJECT`), `from_id`/`to_id`/`label` edge columns, and
/// byte-order (`COLLATE "C"`) text sorting so procedure rows match the
/// reference's Rust string order.
///
/// Honest gates: `ORDER BY` never pushes (`orders_json_typed` is false and
/// the store wires `NoTypeHints`, so ordering always runs in the reference —
/// PostgreSQL's default collation is not byte order); shortest-path walks are
/// off (no insertion-ordered `rowid` for the deterministic tie-break; an
/// ordinal-column migration is the noted follow-up); correlated `tvf.keys`
/// is off (`jsonb_object_keys` yields keys in jsonb storage order —
/// length-then-bytewise — not the reference's sorted order). Recursive CTEs,
/// `generate_series`, and `jsonb_object_keys`-with-outer-sort are on.
///
/// Numeric casts assume type-consistent property values per key (which
/// grust's own tagged writers produce); a mixed-type key would error the
/// query rather than filter, unlike the lenient SQLite/Spark casts.
#[derive(Clone, Debug)]
pub struct PostgresReadDialect {
    nodes: String,
    edges: String,
}

impl PostgresReadDialect {
    pub fn new(config: &PostgresGraphConfig) -> Self {
        Self {
            nodes: unquoted_qualified_table(
                &config.schema,
                &format!("{}_nodes", config.table_prefix),
            ),
            edges: unquoted_qualified_table(
                &config.schema,
                &format!("{}_edges", config.table_prefix),
            ),
        }
    }
}

impl SqlDialect for PostgresReadDialect {
    fn nodes_table(&self) -> &str {
        &self.nodes
    }
    fn edges_table(&self) -> &str {
        &self.edges
    }
    fn quote_ident(&self, ident: &str) -> String {
        // Table names arrive schema-qualified (`schema.table`); quote each part.
        ident
            .split('.')
            .map(quote_ident)
            .collect::<Vec<_>>()
            .join(".")
    }
    fn json_property(&self, props_column: &str, key: &str) -> String {
        format!("{props_column} #>> ARRAY[{}, 'value']", sql_str(key))
    }
    fn exact_string_property_eq(
        &self,
        props_column: &str,
        key: &str,
        value: &str,
    ) -> Option<String> {
        if value.contains('\0') {
            return None;
        }
        let path = format!("ARRAY[{}, 'value']", self.string_literal(key));
        let text = self.byte_order_expr(&format!("({props_column} #>> {path})"));
        let value = self.string_literal(value);
        Some(format!(
            "(jsonb_typeof({props_column} #> {path}) = 'string' AND {text} = {value})"
        ))
    }
    fn cast_int(&self, expr: &str) -> String {
        format!("({expr})::bigint")
    }
    fn cast_float(&self, expr: &str) -> String {
        format!("({expr})::double precision")
    }
    fn string_literal(&self, value: &str) -> String {
        sql_str(value)
    }
    fn string_predicate(&self, expr: &str, op: StrOp, needle: &str) -> String {
        // Literal (non-LIKE) matching; NULL operand propagates NULL.
        let n = self.string_literal(needle);
        match op {
            StrOp::StartsWith => format!("position({n} in {expr}) = 1"),
            StrOp::Contains => format!("position({n} in {expr}) > 0"),
            StrOp::EndsWith => format!("right({expr}, {}) = {n}", needle.chars().count()),
        }
    }
    fn bool_literal_sql(&self, value: bool) -> String {
        // `#>>` renders the tagged jsonb boolean as text.
        if value {
            "'true'".to_string()
        } else {
            "'false'".to_string()
        }
    }
    fn recursive_walk_id_token(&self, expr: &str) -> Option<String> {
        Some(format!("encode(convert_to({expr}, 'UTF8'), 'hex')"))
    }
    fn strpos_sql(&self, haystack: &str, needle: &str) -> String {
        format!("position({needle} in {haystack})")
    }
    fn byte_order_expr(&self, expr: &str) -> String {
        format!("{expr} COLLATE \"C\"")
    }
    fn edge_src_col(&self) -> &str {
        "from_id"
    }
    fn edge_dst_col(&self) -> &str {
        "to_id"
    }
    fn edge_type_col(&self) -> &str {
        "label"
    }
    fn json_props_keys_scan(&self, table: &str) -> Option<String> {
        let t = self.quote_ident(table);
        Some(format!(
            "SELECT j.k FROM {t}, LATERAL jsonb_object_keys({t}.props) AS j(k)"
        ))
    }
    fn integer_series_sql(&self, start: i64, end: i64, step: i64) -> Option<String> {
        // generate_series yields no rows when start is already past end.
        Some(format!(
            "SELECT g AS value FROM generate_series({start}, {end}, {step}) AS g"
        ))
    }
}

impl PostgresGraphStore {
    fn read_dialect(&self) -> PostgresReadDialect {
        PostgresReadDialect::new(&self.config)
    }

    /// Materialize the full graph — the reference-executor fallback input.
    pub async fn read_graph(&self) -> Result<Graph> {
        let nodes = self
            .query_nodes(&format!(
                "SELECT id, label, props::text AS props FROM {}",
                self.nodes_table()
            ))
            .await?;
        let edges = self
            .query_edges(&format!(
                "SELECT id, from_id, to_id, label, props::text AS props FROM {}",
                self.edges_table()
            ))
            .await?;
        Ok(Graph::new(nodes, edges))
    }

    /// Execute pushdown SQL through the simple query protocol, which renders
    /// every column (text, bigint, jsonb, …) as text — exactly the
    /// text-rows contract the pushdown leaves reconstruct from.
    async fn run_text_rows(&self, sql: &str, columns: usize) -> Result<Vec<Vec<Option<String>>>> {
        let _gate = self.lock_connection().await?;
        self.run_text_rows_unlocked(sql, columns).await
    }

    async fn run_text_rows_unlocked(
        &self,
        sql: &str,
        columns: usize,
    ) -> Result<Vec<Vec<Option<String>>>> {
        let messages = self.client.simple_query(sql).await.map_err(|err| {
            GrustError::Backend(format!("PostgreSQL read pushdown failed: {err}: {sql}"))
        })?;
        let mut out = Vec::new();
        for message in messages {
            if let tokio_postgres::SimpleQueryMessage::Row(row) = message {
                let mut cells = Vec::with_capacity(columns);
                for i in 0..columns {
                    cells.push(row.get(i).map(str::to_string));
                }
                out.push(cells);
            }
        }
        Ok(out)
    }

    /// Portable read entrypoint: the query's `MATCH`/`WHERE`/row-source part
    /// pushes into PostgreSQL where the plan supports this dialect (node
    /// scans, fixed segments, `OPTIONAL MATCH`, multi-pattern, `UNION`,
    /// `WITH` pipelines, subqueries incl. correlated `WHERE`, variable-length
    /// paths via `WITH RECURSIVE`, catalog procedures, and `tvf.range` via
    /// `generate_series`); shortest paths, correlated `tvf.keys`, and
    /// everything unplannable fall back to the Memory reference over
    /// [`Self::read_graph`]. Eligible single `COUNT(*)` projections aggregate
    /// in SQL and transport only one scalar; final pagination remains in the
    /// shared Rust projection. Results are identical to
    /// [`grust_cypher::read::run_read_query`] by construction.
    pub async fn run_read_query(
        &self,
        cypher: &str,
        params: &CypherParameters,
    ) -> Result<CypherResultTable> {
        let dialect = self.read_dialect();
        if let Some(plan) = plan_read(cypher, params, &NoTypeHints)?
            && plan.supported_by(&dialect)
        {
            if let Some(count) = plan.scalar_count_read()
                && count.supported_by(&dialect)
            {
                let sql = count.to_sql(&dialect)?;
                let rows = self.run_text_rows(&sql, count.column_count()).await?;
                return count.project_text_rows(rows, params);
            }
            if let Some((arms, distinct)) = plan.union_arms() {
                let mut tables = Vec::with_capacity(arms.len());
                for arm in arms {
                    let rows = self
                        .run_text_rows(&arm.to_sql(&dialect), arm.column_count())
                        .await?;
                    tables.push(arm.project_text_rows(&dialect, rows, params)?);
                }
                return combine_union(tables, distinct);
            }
            let rows = self
                .run_text_rows(&plan.to_sql(&dialect), plan.column_count())
                .await?;
            return plan.project_text_rows(&dialect, rows, params);
        }
        let graph = self.read_graph().await?;
        grust_cypher::read::run_read_query(&graph, cypher, params)
    }

    /// Nodes matching a label + inline props (+ post-filtered predicates) —
    /// the bounded matched-write support, mirroring the Turso executor.
    async fn matching_nodes_unlocked(
        &self,
        label: Option<&Label>,
        props: &Props,
        predicates: &[GraphPropertyPredicate],
    ) -> Result<Vec<Node>> {
        let mut clauses = Vec::new();
        if let Some(label) = label {
            clauses.push(format!("n.label = {}", sql_str(label.as_str())));
        }
        for (key, value) in props {
            if key == "id" {
                let Some(id) = value.as_str() else {
                    return Ok(Vec::new());
                };
                clauses.push(format!("n.id = {}", sql_str(id)));
            } else {
                clauses.push(jsonb_predicate("n", key, value)?);
            }
        }
        let where_clause = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };
        let sql = format!(
            "SELECT n.id, n.label, n.props::text AS props FROM {} n{where_clause}",
            self.nodes_table()
        );
        let mut nodes = self.query_nodes_unlocked(&sql).await?;
        if !predicates.is_empty() {
            nodes.retain(|node| {
                predicates
                    .iter()
                    .all(|predicate| predicate.matches(node.props.get(&predicate.key)))
            });
        }
        Ok(nodes)
    }
}

#[async_trait]
impl CypherMutationExecutor for PostgresGraphStore {
    async fn execute_cypher_mutation_plan(
        &self,
        plan: &GraphMutationPlan,
    ) -> Result<GraphMutationReport> {
        // Lower every fixed operation before opening the transaction so an
        // unsupported trailing operation cannot commit a valid prefix.
        let prepared = plan
            .operations
            .iter()
            .map(|operation| match operation {
                GraphMutationPlanOp::PatchMatchingNodes { .. } => Ok(None),
                other => mutation_sql(
                    &self.nodes_table(),
                    &self.edges_table(),
                    &GraphMutation::from(other.clone()),
                )
                .map(Some),
            })
            .collect::<Result<Vec<_>>>()?;

        let mut report = plan.report();
        let _gate = self.lock_connection().await?;
        self.transaction_needs_rollback
            .store(true, Ordering::Release);
        self.execute_unlocked("BEGIN").await?;
        let execution = async {
            for (operation, prepared_sql) in plan.operations.iter().zip(prepared) {
                match operation {
                    GraphMutationPlanOp::PatchMatchingNodes {
                        label,
                        props,
                        predicates,
                        patch,
                        ..
                    } => {
                        let nodes = self
                            .matching_nodes_unlocked(label.as_ref(), props, predicates)
                            .await?;
                        report.matched_rows += nodes.len();
                        report.node_patches += nodes.len();
                        report.changed_nodes += nodes.len();
                        for node in nodes {
                            let sql = patch_node_sql(&self.nodes_table(), &node.id, patch)?;
                            self.execute_unlocked(&sql).await?;
                        }
                    }
                    _ => {
                        self.execute_unlocked(
                            prepared_sql
                                .as_deref()
                                .expect("fixed mutation SQL was precomputed"),
                        )
                        .await?;
                    }
                }
            }
            Ok::<_, GrustError>(report)
        }
        .await;

        match execution {
            Ok(report) => {
                if let Err(err) = self.execute_unlocked("COMMIT").await {
                    if let Err(recovery_err) = self.rollback_transaction_unlocked().await {
                        return Err(GrustError::Backend(format!(
                            "{err}; PostgreSQL transaction recovery failed: {recovery_err}"
                        )));
                    }
                    return Err(err);
                }
                self.transaction_needs_rollback
                    .store(false, Ordering::Release);
                Ok(report)
            }
            Err(err) => {
                if let Err(recovery_err) = self.rollback_transaction_unlocked().await {
                    return Err(GrustError::Backend(format!(
                        "{err}; PostgreSQL transaction recovery failed: {recovery_err}"
                    )));
                }
                Err(err)
            }
        }
    }
}

impl Drop for PostgresGraphStore {
    fn drop(&mut self) {
        self.connection_task.abort();
    }
}

#[async_trait]
impl GraphStore for PostgresGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        let ddl = postgres_schema_sql(
            &self.config,
            &self.nodes_table(),
            &self.edges_table(),
            schema,
        )?;
        self.bootstrap().await?;
        self.execute(&ddl).await
    }

    async fn put_node(&self, node: &Node) -> Result<PutOutcome> {
        self.execute(&upsert_nodes_sql(
            &self.nodes_table(),
            std::slice::from_ref(node),
        )?)
        .await?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        self.execute(&upsert_edges_sql(
            &self.edges_table(),
            std::slice::from_ref(edge),
        )?)
        .await?;
        Ok(PutOutcome::Upserted)
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        let batch_size = self.config.batch_size.max(1);
        let mut report = LoadReport::default();
        for chunk in graph.nodes.chunks(batch_size) {
            self.execute(&upsert_nodes_sql(&self.nodes_table(), chunk)?)
                .await?;
            report.nodes += chunk.len();
        }
        for chunk in graph.edges.chunks(batch_size) {
            self.execute(&upsert_edges_sql(&self.edges_table(), chunk)?)
                .await?;
            report.edges += chunk.len();
        }
        Ok(report)
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        let sql =
            grust_sql_core::select_node_sql(&PostgresDialect, &self.nodes_table(), id, sql_str);
        Ok(self.query_nodes(&sql).await?.into_iter().next())
    }

    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>> {
        match grust_sql_core::select_nodes_sql(&PostgresDialect, &self.nodes_table(), ids, sql_str)
        {
            Some(sql) => self.query_nodes(&sql).await,
            None => Ok(Vec::new()),
        }
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        let sql =
            grust_sql_core::select_edges_sql(&PostgresDialect, &self.edges_table(), query, sql_str);
        self.query_edges(&sql).await
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        let sql = traversal_sql(&self.nodes_table(), &self.edges_table(), &traversal)?;
        self.query_nodes(&sql).await
    }
}

#[async_trait]
impl GraphAdminStore for PostgresGraphStore {
    async fn bootstrap(&self) -> Result<()> {
        let tables = self.tables();
        self.execute(&bootstrap_sql(&self.config, &tables.nodes, &tables.edges)?)
            .await
    }

    async fn clear(&self) -> Result<()> {
        self.execute(&format!(
            "TRUNCATE TABLE {}, {}",
            self.edges_table(),
            self.nodes_table()
        ))
        .await
    }
}

#[async_trait]
impl GraphMutationStore for PostgresGraphStore {
    fn mutation_atomicity(&self) -> GraphMutationAtomicity {
        GraphMutationAtomicity::Transactional
    }

    async fn delete_node(&self, id: &NodeId) -> Result<()> {
        self.execute(&delete_node_sql(&self.nodes_table(), id))
            .await?;
        Ok(())
    }

    async fn delete_edge(&self, from: &NodeId, label: &Label, to: &NodeId) -> Result<()> {
        self.execute(&delete_edge_sql(&self.edges_table(), from, label, to))
            .await?;
        Ok(())
    }

    async fn apply_mutations(&self, mutations: &[GraphMutation]) -> Result<()> {
        if mutations.is_empty() {
            return Ok(());
        }
        let statements = mutations
            .iter()
            .map(|mutation| mutation_sql(&self.nodes_table(), &self.edges_table(), mutation))
            .collect::<Result<Vec<_>>>()?;
        self.execute_transaction(&statements).await
    }
}

pub fn bootstrap_sql(
    config: &PostgresGraphConfig,
    nodes_table: &str,
    edges_table: &str,
) -> Result<String> {
    validate_postgres_config(config)?;
    let schema = quote_ident(&config.schema);
    Ok(grust_sql_core::universal_bootstrap_sql(
        &PostgresDialect,
        &config.table_prefix,
        &UniversalTableRefs {
            nodes: nodes_table.to_string(),
            edges: edges_table.to_string(),
        },
        Some(&format!("CREATE SCHEMA IF NOT EXISTS {schema}")),
        quote_ident,
    ))
}

pub fn postgres_schema_sql(
    config: &PostgresGraphConfig,
    nodes_table: &str,
    edges_table: &str,
    schema: &GraphSchema,
) -> Result<String> {
    validate_postgres_config(config)?;
    grust_sql_core::schema_sql(
        &PostgresDialect,
        grust_sql_core::GraphSqlSchemaLayout {
            table_prefix: &config.table_prefix,
            nodes_table,
            edges_table,
        },
        schema,
        |view| qualified_table(&config.schema, view),
        quote_ident,
        sql_str,
        postgres_prop_expr,
    )
}

pub fn postgres_typed_column(field: &Field) -> Result<String> {
    validate_typed_column_alias(&field.name)?;
    Ok(format!(
        "{} AS {}",
        postgres_prop_expr(field),
        quote_ident(&field.name)
    ))
}

pub fn postgres_prop_expr(field: &Field) -> String {
    let value = format!("props #>> ARRAY[{}, 'value']", sql_str(&field.name));
    match field.ty {
        FieldType::String
        | FieldType::DateTime
        | FieldType::StringArray
        | FieldType::IntArray
        | FieldType::FloatArray
        | FieldType::Json => value,
        FieldType::Int => format!("({value})::bigint"),
        FieldType::Float => format!("({value})::double precision"),
        FieldType::Bool => format!("({value})::boolean"),
    }
}

pub fn upsert_nodes_sql(table: &str, nodes: &[Node]) -> Result<String> {
    PostgresDialect.upsert_nodes_sql(table, nodes)
}

pub fn upsert_edges_sql(table: &str, edges: &[Edge]) -> Result<String> {
    PostgresDialect.upsert_edges_sql(table, edges)
}

pub fn delete_node_sql(nodes_table: &str, id: &NodeId) -> String {
    grust_sql_core::delete_node_sql(nodes_table, id, sql_str)
}

pub fn patch_node_sql(nodes_table: &str, id: &NodeId, props: &Props) -> Result<String> {
    PostgresDialect.patch_node_sql(nodes_table, id, props)
}

pub fn delete_edge_sql(edges_table: &str, from: &NodeId, label: &Label, to: &NodeId) -> String {
    grust_sql_core::delete_edge_sql(edges_table, from, label, to, sql_str)
}

pub fn mutation_sql(
    nodes_table: &str,
    edges_table: &str,
    mutation: &GraphMutation,
) -> Result<String> {
    grust_sql_core::mutation_sql(
        &PostgresDialect,
        nodes_table,
        edges_table,
        mutation,
        sql_str,
    )
}

pub fn apply_mutations_sql(
    nodes_table: &str,
    edges_table: &str,
    mutations: &[GraphMutation],
) -> Result<String> {
    grust_sql_core::apply_mutations_sql(
        &PostgresDialect,
        nodes_table,
        edges_table,
        mutations,
        sql_str,
    )
}

pub fn traversal_sql(
    nodes_table: &str,
    edges_table: &str,
    traversal: &Traversal,
) -> Result<String> {
    grust_sql_core::traversal_sql(
        &PostgresDialect,
        nodes_table,
        edges_table,
        traversal,
        sql_str,
    )
}

fn jsonb_predicate(alias: &str, key: &str, value: &Value) -> Result<String> {
    validate_json_key(key)?;
    let prop = format!("{alias}.props #>> ARRAY[{}, 'value']", sql_str(key));
    Ok(match value {
        Value::Null => format!("{alias}.props -> {} ->> 'type' = 'null'", sql_str(key)),
        Value::Bool(value) => format!("({prop})::boolean = {value}"),
        Value::Int(value) => format!("({prop})::bigint = {value}"),
        Value::Float(value) => format!("({prop})::double precision = {value}"),
        Value::String(value) => format!("{prop} = {}", sql_str(value)),
        other => {
            let json = serde_json::to_string(other)
                .map_err(|err| GrustError::Serialization(err.to_string()))?;
            format!(
                "{alias}.props -> {} = {}::jsonb",
                sql_str(key),
                sql_str(&json)
            )
        }
    })
}

fn row_to_node(row: tokio_postgres::Row) -> Result<Node> {
    let id: String = row.get("id");
    let label: String = row.get("label");
    let props_json: String = row.get("props");
    let props: Props = serde_json::from_str(&props_json)
        .map_err(|err| GrustError::Serialization(format!("node props JSON parse failed: {err}")))?;
    Ok(Node {
        id: NodeId::new(id),
        label: Label::new(label),
        props,
    })
}

fn row_to_edge(row: tokio_postgres::Row) -> Result<Edge> {
    let id: Option<String> = row.get("id");
    let from_id: String = row.get("from_id");
    let to_id: String = row.get("to_id");
    let label: String = row.get("label");
    let props_json: String = row.get("props");
    let props: Props = serde_json::from_str(&props_json)
        .map_err(|err| GrustError::Serialization(format!("edge props JSON parse failed: {err}")))?;
    let mut edge = Edge::new(label, from_id, to_id, props);
    edge.id = id.map(EdgeId::new);
    Ok(edge)
}

pub fn qualified_table(schema: &str, table: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(table))
}

pub fn unquoted_qualified_table(schema: &str, table: &str) -> String {
    format!("{schema}.{table}")
}

pub fn quote_ident(value: &str) -> String {
    grust_sql_core::quote_ident(value)
}

pub fn sql_str(value: &str) -> String {
    grust_sql_core::sql_str(value)
}

pub fn validate_identifier(value: &str) -> Result<()> {
    grust_sql_core::validate_identifier("PostgreSQL", value)?;
    validate_identifier_length(value)
}

pub fn validate_postgres_config(config: &PostgresGraphConfig) -> Result<()> {
    validate_identifier(&config.schema)?;
    validate_identifier(&config.table_prefix)?;
    grust_sql_core::validate_universal_identifier_lengths(&PostgresDialect, &config.table_prefix)
}

pub fn validate_json_key(value: &str) -> Result<()> {
    grust_sql_core::validate_identifier("PostgreSQL JSON property key", value)
        .map_err(|_| GrustError::Schema(format!("invalid JSON property key '{value}'")))
}

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug)]
struct PostgresDialect;

impl GraphSqlDialect for PostgresDialect {
    fn name(&self) -> &'static str {
        "PostgreSQL"
    }

    fn max_identifier_bytes(&self) -> Option<usize> {
        Some(POSTGRES_IDENTIFIER_MAX_BYTES)
    }

    fn props_column_type(&self) -> &'static str {
        "jsonb"
    }

    fn empty_props_default(&self) -> &'static str {
        "'{}'::jsonb"
    }

    fn create_view_prefix(&self) -> &'static str {
        "CREATE OR REPLACE VIEW"
    }

    fn node_props_select(&self, alias: &str) -> String {
        if alias.is_empty() {
            "props::text".to_string()
        } else {
            format!("{alias}.props::text")
        }
    }

    fn edge_props_select(&self, alias: &str) -> String {
        self.node_props_select(alias)
    }

    fn json_property_predicate(&self, alias: &str, key: &str, value: &Value) -> Result<String> {
        jsonb_predicate(alias, key, value)
    }

    fn both_direction_join(
        &self,
        edges_table: &str,
        edge_alias: &str,
        prev_alias: &str,
        edge_label: &str,
    ) -> String {
        format!(
            "JOIN LATERAL (
                SELECT to_id AS next_id
                FROM {edges_table}
                WHERE from_id = {prev_alias}.id{edge_label}
                UNION ALL
                SELECT from_id AS next_id
                FROM {edges_table}
                WHERE to_id = {prev_alias}.id{edge_label}
            ) {edge_alias} ON TRUE"
        )
    }

    fn upsert_nodes_sql(&self, table: &str, nodes: &[Node]) -> Result<String> {
        if nodes.is_empty() {
            return Ok(String::new());
        }
        let rows = nodes
            .iter()
            .map(|node| {
                let props = grust_sql_core::props_to_json(&node.props)?;
                Ok(format!(
                    "({}, {}, {}::jsonb)",
                    sql_str(node.id.as_str()),
                    sql_str(node.label.as_str()),
                    sql_str(&props)
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        Ok(format!(
            "INSERT INTO {table} (id, label, props) VALUES {rows}
             ON CONFLICT (id) DO UPDATE SET
                label = EXCLUDED.label,
                props = EXCLUDED.props"
        ))
    }

    fn upsert_edges_sql(&self, table: &str, edges: &[Edge]) -> Result<String> {
        if edges.is_empty() {
            return Ok(String::new());
        }
        let rows = edges
            .iter()
            .map(|edge| {
                let props = grust_sql_core::props_to_json(&edge.props)?;
                Ok(format!(
                    "({}, {}, {}, {}, {}::jsonb)",
                    edge.id
                        .as_ref()
                        .map(|id| sql_str(id.as_str()))
                        .unwrap_or_else(|| "NULL".to_string()),
                    sql_str(edge.from.as_str()),
                    sql_str(edge.to.as_str()),
                    sql_str(edge.label.as_str()),
                    sql_str(&props)
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        Ok(format!(
            "INSERT INTO {table} (id, from_id, to_id, label, props) VALUES {rows}
             ON CONFLICT (from_id, label, to_id) DO UPDATE SET
                id = EXCLUDED.id,
                props = EXCLUDED.props"
        ))
    }

    fn patch_node_sql(&self, nodes_table: &str, id: &NodeId, props: &Props) -> Result<String> {
        let props = grust_sql_core::props_to_json(props)?;
        Ok(format!(
            "UPDATE {nodes_table} SET props = props || {}::jsonb WHERE id = {}",
            sql_str(&props),
            sql_str(id.as_str())
        ))
    }
}
