//! DDL statements, constraint registry, schema manager, and DDL/constraint entrypoints (extracted from lib.rs).

use crate::*;

/// A parsed Cypher schema (DDL) statement.
///
/// DDL is deliberately kept separate from the data-mutation plan: constraint
/// statements describe schema intent that callers apply to a [`GraphSchema`]
/// (and then to a backend through [`GraphStore::apply_schema`]), rather than
/// flowing through [`GraphMutationStore`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CypherDdlStatement {
    /// `CREATE CONSTRAINT [name] [IF NOT EXISTS] FOR ... REQUIRE ... IS ...`.
    CreateConstraint {
        name: Option<String>,
        if_not_exists: bool,
        constraint: GraphConstraint,
    },
    /// `DROP CONSTRAINT name [IF EXISTS]`.
    DropConstraint { name: String, if_exists: bool },
}

/// A named Cypher constraint stored outside [`GraphSchema`].
///
/// `GraphSchema` remains the portable enforcement shape and stores unnamed
/// [`GraphConstraint`] values. This registry layer preserves Cypher constraint
/// names so callers can apply `DROP CONSTRAINT name` deterministically before
/// passing the resulting constraints into a schema.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NamedGraphConstraint {
    pub name: String,
    pub constraint: GraphConstraint,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CypherDdlApplicationReport {
    pub created: usize,
    pub skipped: usize,
    pub dropped: usize,
    pub missing: usize,
}

impl CypherDdlApplicationReport {
    fn merge(&mut self, other: Self) {
        self.created += other.created;
        self.skipped += other.skipped;
        self.dropped += other.dropped;
        self.missing += other.missing;
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CypherSchemaApplication {
    pub schema: GraphSchema,
    pub report: CypherDdlApplicationReport,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CypherSchemaManager {
    pub schema: GraphSchema,
    pub registry: CypherConstraintRegistry,
}

impl CypherSchemaManager {
    pub fn new(schema: GraphSchema) -> Self {
        let registry = CypherConstraintRegistry::from_schema(&schema);
        Self { schema, registry }
    }

    pub fn with_registry(schema: GraphSchema, registry: CypherConstraintRegistry) -> Self {
        Self { schema, registry }
    }

    pub fn from_registry_json(schema: GraphSchema, registry_json: &str) -> Result<Self> {
        Ok(Self::with_registry(
            schema,
            CypherConstraintRegistry::from_json(registry_json)?,
        ))
    }

    pub fn registry_json(&self) -> Result<String> {
        self.registry.to_json()
    }

    pub async fn apply_cypher_ddl<S>(
        &mut self,
        store: &S,
        cypher: &str,
    ) -> Result<CypherSchemaApplication>
    where
        S: GraphStore + Sync,
    {
        let applied =
            apply_cypher_ddl_to_schema(store, &self.schema, &mut self.registry, cypher).await?;
        self.schema = applied.schema.clone();
        Ok(applied)
    }
}

/// Named constraint metadata for applying parsed Cypher DDL.
///
/// The registry is intentionally separate from backend persistence. Callers can
/// parse DDL with [`sail_cypher_ddl`], apply it here, then build or update a
/// [`GraphSchema`] from [`CypherConstraintRegistry::constraints`] before calling
/// [`GraphStore::apply_schema`]. [`CypherConstraintRegistry::to_json`] and
/// [`CypherConstraintRegistry::from_json`] provide a caller-owned persistence
/// hook for storing that named metadata outside backend-native schema storage.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CypherConstraintRegistry {
    pub(crate) named: BTreeMap<String, GraphConstraint>,
    pub(crate) anonymous: Vec<GraphConstraint>,
}

impl CypherConstraintRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_schema(schema: &GraphSchema) -> Self {
        Self {
            named: BTreeMap::new(),
            anonymous: schema.constraints.clone(),
        }
    }

    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|err| {
            GrustError::Serialization(format!(
                "Cypher constraint registry JSON parse error: {err}"
            ))
        })
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|err| {
            GrustError::Serialization(format!(
                "Cypher constraint registry JSON serialization error: {err}"
            ))
        })
    }

    pub fn named_constraints(&self) -> Vec<NamedGraphConstraint> {
        self.named
            .iter()
            .map(|(name, constraint)| NamedGraphConstraint {
                name: name.clone(),
                constraint: constraint.clone(),
            })
            .collect()
    }

    pub fn anonymous_constraints(&self) -> &[GraphConstraint] {
        &self.anonymous
    }

    pub fn constraints(&self) -> Vec<GraphConstraint> {
        self.named
            .values()
            .cloned()
            .chain(self.anonymous.iter().cloned())
            .collect()
    }

    pub fn apply_to_schema(&self, schema: &GraphSchema) -> GraphSchema {
        let mut schema = schema.clone();
        schema.constraints = self.constraints();
        schema
    }

    pub fn apply_statement(
        &mut self,
        statement: CypherDdlStatement,
    ) -> Result<CypherDdlApplicationReport> {
        match statement {
            CypherDdlStatement::CreateConstraint {
                name,
                if_not_exists,
                constraint,
            } => {
                if let Some(name) = name {
                    if self.named.contains_key(&name) {
                        if if_not_exists {
                            return Ok(CypherDdlApplicationReport {
                                skipped: 1,
                                ..Default::default()
                            });
                        }
                        return Err(GrustError::CypherExecution(format!(
                            "constraint '{name}' already exists"
                        )));
                    }
                    self.named.insert(name, constraint);
                } else {
                    self.anonymous.push(constraint);
                }
                Ok(CypherDdlApplicationReport {
                    created: 1,
                    ..Default::default()
                })
            }
            CypherDdlStatement::DropConstraint { name, if_exists } => {
                if self.named.remove(&name).is_some() {
                    return Ok(CypherDdlApplicationReport {
                        dropped: 1,
                        ..Default::default()
                    });
                }
                if if_exists {
                    return Ok(CypherDdlApplicationReport {
                        missing: 1,
                        ..Default::default()
                    });
                }
                Err(GrustError::CypherExecution(format!(
                    "constraint '{name}' does not exist"
                )))
            }
        }
    }

    pub fn apply_statements(
        &mut self,
        statements: impl IntoIterator<Item = CypherDdlStatement>,
    ) -> Result<CypherDdlApplicationReport> {
        let mut next = self.clone();
        let mut report = CypherDdlApplicationReport::default();
        for statement in statements {
            report.merge(next.apply_statement(statement)?);
        }
        *self = next;
        Ok(report)
    }

    pub fn apply_cypher(&mut self, cypher: &str) -> Result<CypherDdlApplicationReport> {
        self.apply_statements(sail_cypher_ddl(cypher)?)
    }
}

pub async fn apply_cypher_ddl_to_schema<S>(
    store: &S,
    schema: &GraphSchema,
    registry: &mut CypherConstraintRegistry,
    cypher: &str,
) -> Result<CypherSchemaApplication>
where
    S: GraphStore + Sync,
{
    let mut next = registry.clone();
    let report = next.apply_cypher(cypher)?;
    let schema = next.apply_to_schema(schema);
    store.apply_schema(&schema).await?;
    *registry = next;
    Ok(CypherSchemaApplication { schema, report })
}

/// Applies parsed Cypher `CREATE CONSTRAINT` DDL through a backend's native
/// constraint path.
///
/// This helper preserves `IF NOT EXISTS` reporting from
/// [`GraphStore::apply_native_constraint`]. `DROP CONSTRAINT` is rejected until
/// Grust has backend-neutral native drop semantics.
pub async fn apply_cypher_native_constraints<S>(
    store: &S,
    cypher: &str,
) -> Result<GraphNativeConstraintReport>
where
    S: GraphStore + Sync,
{
    let mut report = GraphNativeConstraintReport::default();
    for statement in cypher_ddl(cypher)? {
        match statement {
            CypherDdlStatement::CreateConstraint {
                if_not_exists,
                constraint,
                ..
            } => {
                let applied = store
                    .apply_native_constraint(GraphNativeConstraintRequest {
                        constraint,
                        if_not_exists,
                    })
                    .await?;
                report.applied += applied.applied;
                report.skipped += applied.skipped;
            }
            CypherDdlStatement::DropConstraint { .. } => {
                return Err(cypher_syntax(
                    "native Cypher constraint application does not support DROP CONSTRAINT",
                ));
            }
        }
    }
    Ok(report)
}

/// Parses one or more Cypher DDL statements (currently `CREATE CONSTRAINT` and
/// `DROP CONSTRAINT`) into backend-neutral [`CypherDdlStatement`] values.
///
/// Supported constraint forms:
///
/// ```cypher
/// CREATE CONSTRAINT person_id IF NOT EXISTS
/// FOR (n:Person) REQUIRE n.id IS UNIQUE;
/// CREATE CONSTRAINT FOR (n:Person) REQUIRE n.name IS NOT NULL;
/// CREATE CONSTRAINT FOR ()-[r:KNOWS]-() REQUIRE r.since IS NOT NULL;
/// DROP CONSTRAINT person_id IF EXISTS;
/// ```
///
/// The legacy `ON ... ASSERT ...` spelling is accepted as a synonym for
/// `FOR ... REQUIRE ...`. Composite/node-key constraints, index DDL, and
/// property existence on multiple keys are rejected with a clear error.
pub fn cypher_ddl(cypher: &str) -> Result<Vec<CypherDdlStatement>> {
    let cypher = strip_cypher_comments(cypher)?;
    let statements = split_cypher_statements(&cypher)?;
    if statements.is_empty() {
        return Err(cypher_syntax("Cypher DDL statement is empty"));
    }
    statements
        .into_iter()
        .map(|statement| parse_cypher_ddl_statement(statement.trim()))
        .collect()
}

pub fn sail_cypher_ddl(cypher: &str) -> Result<Vec<CypherDdlStatement>> {
    cypher_ddl(cypher)
}

/// Parses Cypher constraint DDL and returns only the resulting
/// [`GraphConstraint`] values, discarding names and `IF [NOT] EXISTS` flags.
///
/// `DROP CONSTRAINT` statements are rejected because they carry no constraint
/// body; use [`sail_cypher_ddl`] when those are needed.
pub fn cypher_constraints(cypher: &str) -> Result<Vec<GraphConstraint>> {
    sail_cypher_ddl(cypher)?
        .into_iter()
        .map(|statement| match statement {
            CypherDdlStatement::CreateConstraint { constraint, .. } => Ok(constraint),
            CypherDdlStatement::DropConstraint { .. } => Err(cypher_syntax(
                "sail_cypher_constraints does not accept DROP CONSTRAINT statements",
            )),
        })
        .collect()
}

pub fn sail_cypher_constraints(cypher: &str) -> Result<Vec<GraphConstraint>> {
    cypher_constraints(cypher)
}
