# Changelog

All notable Grust changes are recorded here by date and release. This project
started before the changelog existed, so entries before 2026-06-12 were
reconstructed from Git history, release commits, and the shipped docs.

## Unreleased

- Added a strict-write **golden-snapshot** regression harness (`grust-cypher/tests/write_golden.rs` + `tests/gql/write_golden.json`, Unit 10a): pins the current planner output (plan or rejection) for a 20-statement write corpus so any future write-path change is caught byte-for-byte.

- Added graph-type validation (`grust_cypher::graph_type`, Unit 11): the open-vs-closed graph-type distinction (`GraphTypeMode`) and write-time type-violation checks `validate_node`/`validate_edge`/`validate_graph` over a `GraphSchema` — closed graph types reject undeclared labels/properties; both modes type-check declared properties and enforce required fields/constraints. Backend-neutral and additive (a `ValidateBeforeWrite` hook; changes no backend).

- Temporal values (`Value::DateTime`) now order chronologically (lexicographic over the RFC 3339 form) in both the read executor's comparison/`ORDER BY` and the RETURN projection ordering; previously any two datetimes compared equal. (Unit T, temporal.)

- Added a per-backend GQL/Cypher conformance model (`grust_cypher::gql`): `GqlBackend` + `GqlBackendDescriptor` + `GqlBackendRole`, with `backend_manifest()` and `cypher_conformance_backends()`. Honest capability flags (verified against the code): the executing Cypher-conformance set is Memory/Sail/Turso; only Sail has read pushdown; Postgres/pgGraph-PGQ are SQL/PGQ stores with no portable Cypher executor yet; helix/ladybug are internal (`publish=false`, out of facade); cocoindex is a sync target.

- Added backend-neutral read-query **pushdown** (`grust_cypher::pushdown`): a
  bounded `MATCH … RETURN` query's `MATCH`/`WHERE` filter is lowered into SQL via
  a `SqlDialect` (Spark and SQLite provided), while the `RETURN` projection runs
  through the shared Memory reference so pushdown results are identical to
  `grust_cypher::read::run_read_query` by construction. `SailGraphStore` gains a
  public `run_read_query` that pushes the filter into Spark SQL for the pushable
  subset (single node pattern with property comparisons) and falls back to the
  portable reference otherwise (additive public API). An embedded-SQLite
  differential oracle (`grust-turso`) verifies reference-vs-pushdown row equality
  without a server. The pushable subset now also covers a single **directed
  relationship segment** (`(a)-[:T]->(b)` / `<-[:T]-`, multiple rel types, inline
  endpoint/edge properties, and `WHERE` over `a`/`r`/`b`), lowered to a
  `grust_edges`/`grust_nodes` join; the backend returns the matched columns as
  text and `grust_cypher` reconstructs the bindings before projecting. A unified
  `plan_read` returns a `ReadPushdown` (single-query leaf or a `UNION`/`UNION ALL`
  of leaves, combined by `combine_union`). **`OPTIONAL MATCH`** (a mandatory node
  + one optional directed segment) lowers to a `LEFT JOIN` against a subquery for
  the optional segment, with null-padding (`r`/`b` → `null`) matching the
  reference. **Multi-pattern `MATCH`** (`(a)-[]->(b), (a)-[]->(c)` and bare cross
  products) lowers to a comma-join with shared variables reusing an alias. A
  **`WITH` horizon** (`MATCH … WITH … RETURN`) pushes the leading node scan/filter
  and runs the horizon (incl. aggregation) through the shared reference pipeline.
  This now
  covers **multi-segment paths** (`(a)-[]->(b)-[]->(c)`, chained joins) and
  **undirected** segments (`(a)-[]-(b)`, matched in either orientation), in any
  per-segment direction. **Variable-length** segments (`(a)-[:T*m..n]->(b)`, with
  an anonymous relationship) lower to a recursive CTE enumerating simple paths
  (no repeated nodes, like the reference); this is row-equality-verified against
  real SQLite and depends on recursive-CTE support in the target engine.
  `WHERE … IN [literals]` (and `NOT … IN`) is also pushed, on both the node and
  segment paths, for non-empty homogeneous int/float/string lists. `STARTS WITH`
  / `ENDS WITH` / `CONTAINS` with a non-empty string needle are pushed too
  (Spark `STARTSWITH`/`ENDSWITH`/`CONTAINS`, SQLite `instr`/`substr`), matching
  the reference for string-typed properties (a non-string value errors in the
  reference but filters under pushdown). Boolean equality (`prop = true|false`,
  `<>`) is pushed too (SQLite compares the `json_extract` integer `1`/`0`, Spark
  the `GET_JSON_OBJECT` text `'true'`/`'false'`). Arithmetic comparisons over
  typed numeric properties (`n.age + 1 > 40`) are pushed on the node and segment
  paths for the `+`/`-`/`*` subset (each property cast to its hinted type);
  `/` renders as floating-point division (reference `/` is f64); `%`/`^` and unknown-typed properties fall back (dialect-divergent). `ORDER BY` /
  `SKIP` / `LIMIT` are pushed into SQL on the single-node path for dialects whose
  JSON extraction is natively typed (SQLite/libSQL `json_extract`, not Spark
  `GET_JSON_OBJECT`), gated on no aggregate/`DISTINCT` and scan-var sort keys,
  with `NULLS LAST`/`FIRST` matching the reference; otherwise ordering stays in
  the reference projection. A `TypeHints` trait (built from the graph schema by
  the backend; `SailGraphStore` derives it from the applied `GraphSchema`) lets
  an untyped-JSON dialect like Spark push numeric `ORDER BY` too, by casting each
  sort key to its declared type. `ORDER BY`/`SKIP`/`LIMIT` pushdown also applies
  to the relationship-segment path (sort keys over `a`/`r`/`b`, including
  edge-property keys when the relationship has a single type the schema describes).
- Refactored `grust-cypher` from a single ~16k-line `lib.rs` and ~17k-line
  `tests.rs` into cohesive modules (`ddl`, `parse`, `primitives`, `planner`,
  `eval_rows`, `restricted_values`, `projection`, `where_clause`, `returning`,
  plus the new `gql`, `lexer`, `ast`, `parser`, `semantics`) and a per-area
  `tests/` directory. The public API is unchanged; crate internals are now
  `pub(crate)`.
- Tightened the `grust-sail` and `grust-graph` Cypher re-export surface
  (public-API change). `grust-sail` no longer re-exports all of `grust-cypher`
  via a glob — it now explicitly re-exports the portable Cypher API it executes.
  The `grust-graph` `sail` feature now enables `cypher`, and the facade
  re-exports the Cypher language surface once (from the `cypher` block) while the
  `sail` block re-exports only Sail-native items; this also fixes building the
  facade with `cypher` and `sail` enabled together. Removed dead
  `helix`/`ladybug` facade re-export blocks left over from those backends being
  dropped from the facade.
- Added `grust-postgres-pgq`, a PostgreSQL 19 SQL/PGQ backend that reuses the
  shared PostgreSQL universal-table store, creates a native `PROPERTY GRAPH`,
  executes bounded traversal through `GRAPH_TABLE`, and is exposed through the
  `grust-graph` facade feature `postgres-pgq`.
- Added Turso-backed matched-node patch execution for the Grust Cypher mutation
  executor. `TursoGraphStore` can now run the reusable Cypher
  `MATCH ... SET ... RETURN ...` path for bounded node patches while keeping
  unsupported matched edge/delete/update forms explicit.

## 0.10.0 - 2026-06-22

- Added `grust-sql-core`, a shared SQL generation crate for universal-table
  SQL backends, and refactored PostgreSQL/pgGraph and Turso lowering through
  it while keeping JSON operators, upsert syntax, view creation, transaction
  semantics, and bidirectional traversal join shapes dialect-specific.
- Added `grust-turso`, a Turso Rust SDK backend with local in-process Turso
  storage, optional Turso Cloud sync construction, universal node/edge tables,
  SQL-backed reads/traversal, schema views/indexes, and transactional mutation
  batches.

- Added a generic `grust-postgres` backend for extension-free PostgreSQL
  deployments such as Neon, with reusable `grust-postgres-core` storage,
  schema-view, traversal, and mutation SQL shared by `grust-pggraph`.
- Refactored `grust-pggraph` into a pgGraph extension/projection wrapper over
  the shared PostgreSQL backend implementation.
- Refreshed documentation status after the writable Cypher completion pass:
  updated book and Arrow examples for the `0.10.0` line, replaced the stale
  restart checkpoint, marked older backend proposal documents as historical
  design notes where implementation now exists, and added the next major
  Cypher work areas to `docs/CypherWrite.md`.
- Added `docs/GrustCypherFull.md` and `docs/GrustCypherBackends.md` to plan the
  path from the current strict Grust Cypher subset toward full GQL coverage and
  backend-specific portable conformance profiles.
- Split the full GQL plan into execution-sized logical work units with
  dependencies, full-access Codex estimates, and done criteria.
- Extracted the writable Cypher parser, planner, DDL types, constraint
  registry, return evaluator, and generic returning executor into a new
  `grust-cypher` crate, so any `GraphStore` backend can use the Cypher
  planning and materialization layer without depending on `grust-sail`.
  `grust-sail` retains the Sail SQL lowering, Arrow IPC staging, and
  SparkConnect execution and depends on `grust-cypher` for all Cypher types.
  The `grust-graph` facade exposes a new `cypher` feature that pulls in
  `grust-cypher` without requiring the full `sail` feature.
- Moved backend-neutral writable Cypher parser, planner, DDL, restricted
  returning, and Memory-backed generic execution tests from `grust-sail` into
  `grust-cypher`; `grust-sail` now keeps Sail SQL, Arrow, SparkConnect, and
  live Sail persistence coverage.
- Added a restricted boolean AST for mutating Cypher `MATCH ... WHERE`
  lowering, so bounded `AND` / `OR` / one-term `NOT` groups lower through one
  conservative backend-neutral predicate path and factored unparenthesized
  `AND` / `OR` groups can be accepted when they canonicalize to the existing
  foldable predicate-vector shape.
- Consolidated restricted writable Cypher aggregate projection materialization
  so literal, map/list, introspection, string, numeric, conversion,
  `coalesce`, `CASE`, and list-helper aggregate bodies reuse the scalar
  projection materializer while aggregate-specific `*`, whole-element,
  property, and path-function paths remain explicit.
- Consolidated restricted writable Cypher `COUNT(...)` projection
  materialization onto the same scalar projection classifier while preserving
  explicit `count(*)`, whole-element, direct-property, path-function, non-null,
  and `DISTINCT` semantics.
- Consolidated grouped writable Cypher aggregate row materialization so
  classifier-covered restricted scalar targets reuse the scalar projection
  evaluator while aggregate-specific `*`, whole-element, direct-property, and
  path-function paths remain explicit.
- Added an internal writable Cypher `RETURN` target materialization classifier
  that separates star, whole-element, direct-property, scalar-projection,
  element-function, and path-function targets before aggregate, grouped
  aggregate, and `COUNT` routing.
- Added an internal writable Cypher scalar projection kind classifier so
  restricted scalar evaluation now explicitly routes star, whole-element,
  direct-property, literal, map/list, conditional, coalesce, introspection,
  list-helper, numeric, conversion, string, element-function, and path-function
  target shapes.
- Added an internal writable Cypher scalar expression view so restricted scalar
  classification and evaluation route through expression-shaped variants rather
  than matching the public return-target enum directly.
- Added a dedicated internal evaluator boundary for writable Cypher restricted
  list-helper scalar expressions while preserving the existing list projection
  materializers and supported syntax.
- Added a dedicated internal evaluator boundary for writable Cypher restricted
  string-helper scalar expressions while preserving the existing string
  projection materializers and supported syntax.
- Added dedicated internal evaluator boundaries for writable Cypher restricted
  numeric and conversion scalar expressions while preserving the existing
  numeric, scalar cast, and list cast materializers and supported syntax.
- Added dedicated internal evaluator boundaries for writable Cypher restricted
  literal/composite, `CASE`/`coalesce`, and introspection scalar expressions
  while preserving existing materializers and supported syntax.
- Added dedicated internal evaluator boundaries for writable Cypher scalar
  binding routes and element/path wrapper routes, completing expression-family
  dispatch for the currently supported restricted scalar target shapes.
- Added an internal writable Cypher scalar AST-family classifier so the
  top-level scalar dispatcher routes through binding, wrapper, value, control,
  introspection, list, numeric, conversion, and string evaluator families.
- Promoted the internal writable Cypher restricted scalar expression view to a
  `CypherReturnScalarAst` boundary used by scalar kind classification, family
  classification, and scalar projection evaluation.
- Extended restricted writable Cypher `coalesce(...)` so arguments can be
  direct properties, literals, or already-supported restricted scalar targets
  evaluated through the scalar AST while still requiring one variable.
- Extended restricted writable Cypher list projections so list items can be
  direct properties, literals, or already-supported restricted scalar targets
  evaluated through the scalar AST while still rejecting nested list/map
  composites and cross-variable lists.
- Extended restricted writable Cypher map projections so entry values can be
  same-variable properties, literals, or already-supported restricted scalar
  targets evaluated through the scalar AST while still rejecting nested
  list/map composites and cross-variable values.
- Consolidated nested restricted scalar parsing across `coalesce(...)`, list
  projection items, and map projection values, including shared rejection for
  nested list/map composites before a broader expression AST exists.
- Extended restricted writable Cypher `CASE` branch values so `THEN` and
  `ELSE` can wrap same-variable direct properties, literals, or
  already-supported restricted scalar targets while preserving equality-only
  CASE predicates.
- Extended restricted writable Cypher list predicate equality values so
  `any` / `all` / `none` / `single` comparisons can use same-variable direct
  properties, literals, or already-supported restricted scalar targets while
  preserving property-only haystacks and item-variable equality predicates.
- Extended restricted writable Cypher `toLower(...)` and `toUpper(...)`
  projections so they can wrap direct properties, literals, or
  already-supported restricted scalar targets while preserving the existing
  string-only value semantics.
- Extended restricted writable Cypher `trim(...)`, `lTrim(...)`, and
  `rTrim(...)` projections so they can wrap direct properties, literals, or
  already-supported restricted scalar targets while preserving the existing
  string-only trim semantics.
- Extended restricted writable Cypher `reverse(...)` projections so they can
  wrap direct properties, literals, or already-supported restricted scalar
  targets while preserving the existing string-or-array reverse semantics.
- Extended restricted writable Cypher `isEmpty(...)` projections so they can
  wrap direct properties, literals, or already-supported restricted scalar
  targets while preserving the existing string, array, and JSON collection
  emptiness semantics.
- Extended restricted writable Cypher `split(...)` projections so their first
  argument can wrap direct properties, literals, or already-supported
  restricted scalar targets while delimiters remain non-empty string literals
  or parameters.
- Extended restricted writable Cypher `substring(...)` projections so their
  first argument can wrap direct properties, literals, or already-supported
  restricted scalar targets while offsets remain non-negative integer literals
  or parameters.
- Extended restricted writable Cypher `left(...)` and `right(...)` projections
  so their first argument can wrap direct properties, literals, or
  already-supported restricted scalar targets while lengths remain
  non-negative integer literals or parameters.
- Extended restricted writable Cypher `startsWith(...)`, `endsWith(...)`, and
  `contains(...)` projections so their first argument can wrap direct
  properties, literals, or already-supported restricted scalar targets while
  needles remain string literals or parameters.
- Extended restricted writable Cypher `replace(...)` projections so their
  first argument can wrap direct properties, literals, or already-supported
  restricted scalar targets while search and replacement strings remain
  literals or parameters.
- Extended restricted writable Cypher `toString(...)` projections so their
  argument can wrap direct properties, literals, or already-supported
  restricted scalar targets while preserving scalar-only string conversion.
- Extended restricted writable Cypher `abs(...)` projections so their argument
  can wrap direct properties, literals, or already-supported restricted scalar
  targets while preserving numeric-only absolute-value semantics.
- Extended restricted writable Cypher `ceil(...)` and `floor(...)` projections
  so their argument can wrap direct properties, literals, or already-supported
  restricted scalar targets while preserving numeric-only rounding semantics.
- Extended restricted writable Cypher `sign(...)` projections so their
  argument can wrap direct properties, literals, or already-supported
  restricted scalar targets while preserving finite numeric sign semantics.
- Extended restricted writable Cypher `toInteger(...)` and `toFloat(...)`
  projections so their argument can wrap direct properties, literals, or
  already-supported restricted scalar targets while preserving numeric and
  numeric-string conversion semantics.
- Extended restricted writable Cypher `toBoolean(...)` projections so their
  argument can wrap direct properties, literals, or already-supported
  restricted scalar targets while preserving boolean and boolean-string
  conversion semantics.
- Extended restricted writable Cypher `head(...)`, `last(...)`, and
  `tail(...)` projections so their argument can wrap direct properties,
  literals, or already-supported restricted scalar targets while preserving
  array-only list access semantics.
- Extended restricted writable Cypher list indexes and slice bounds so their
  subscript expressions can wrap direct properties, literals, or
  already-supported restricted scalar targets while preserving
  non-negative-integer subscript semantics.
- Extended restricted writable Cypher `toStringList(...)`,
  `toIntegerList(...)`, `toFloatList(...)`, and `toBooleanList(...)`
  projections so their argument can wrap direct properties, literals, or
  already-supported restricted scalar targets while preserving array-only list
  conversion semantics.
- Extended restricted mutating Cypher `MATCH ... WHERE` boolean lowering to
  collapse double negation over an otherwise bounded predicate back to the
  positive backend-neutral predicate.
- Extended restricted mutating Cypher `MATCH ... WHERE` `OR` folding to
  flatten nested parenthesized foldable `OR` terms before applying the
  existing same-property grouped predicate or grouped exclusion lowering.
- Extended restricted mutating Cypher `MATCH ... WHERE` boolean lowering so
  negated foldable `AND` groups, such as
  `NOT (n.status <> 'active' AND n.status <> 'pending')`, can lower through
  the existing same-property grouped predicate path, including matching string
  predicate groups such as
  `NOT (NOT n.name STARTS WITH 'Ad' AND NOT n.name STARTS WITH 'Gr')`, while
  mixed-property and general De Morgan cases remain rejected.
- Extended restricted mutating Cypher `MATCH ... WHERE` boolean lowering so
  duplicate negated `AND` terms such as
  `NOT (n.status = 'blocked' AND n.status = 'blocked')` collapse to the same
  bounded predicate as `NOT n.status = 'blocked'`.
- Extended restricted mutating Cypher `MATCH ... WHERE` string folding so
  nested negated `AND` groups can merge an already-grouped string predicate
  with another matching same-property string predicate while general boolean
  evaluation remains rejected.
- Extended restricted mutating Cypher `MATCH ... WHERE` factored-branch
  pruning so exact string predicates can be recognized as covered by sibling
  grouped string predicates over the same variable, property, and string
  operation family.
- Extended restricted mutating Cypher `MATCH ... WHERE` factored-branch
  pruning so negated string predicates can be recognized as covered by sibling
  grouped negated string predicates over the same variable, property, and
  string operation family.
- Extended restricted mutating Cypher `MATCH ... WHERE` factored-branch
  pruning so bounded predicates that imply `IS NOT NULL`, plus exact-null
  predicates that imply `IS NULL`, can be recognized as covered by sibling
  null-check predicates without reversing missing-property semantics.
- Extended restricted mutating Cypher `MATCH ... WHERE` factored-branch
  pruning so exact inequality predicates can be recognized as covered by
  equivalent singleton leading-`NOT` membership exclusions.
- Extended restricted mutating Cypher `MATCH ... WHERE` factored-branch
  pruning so singleton membership predicates can be recognized as covered by
  equivalent exact equality predicates.
- Canonicalized restricted mutating Cypher `MATCH ... WHERE` factored-branch
  pruning so equivalent singleton membership and exact equality branches keep
  the equality predicate form.
- Extended restricted mutating Cypher `MATCH ... WHERE` factored-branch
  pruning so ordered-bound predicates can be recognized as covered by sibling
  scalar inequality predicates when the excluded value cannot satisfy the
  bound.
- Extended restricted mutating Cypher `MATCH ... WHERE` factored-branch
  pruning so ordered-bound predicates can be recognized as covered by sibling
  grouped exclusion predicates when every excluded value cannot satisfy the
  bound.
- Extended restricted mutating Cypher `MATCH ... WHERE` simple `OR` lowering
  so non-folded bounded terms can reuse conservative branch subsumption, such
  as pruning a narrower string predicate when a sibling `IS NOT NULL` predicate
  already covers it.
- Extended restricted mutating Cypher `MATCH ... WHERE` negated simple `OR`
  lowering so a disjunction that first collapses to one bounded predicate can
  be inverted, preserving rejection for general De Morgan expansion.
- Extended restricted mutating Cypher `MATCH ... WHERE` negated factored `OR`
  lowering so a factored disjunction that first collapses to one non-empty
  bounded predicate can be inverted while broader De Morgan cases stay
  rejected.
- Extended restricted mutating Cypher `MATCH ... WHERE` negated `AND` lowering
  so the disjunction of negated bounded terms can reuse conservative branch
  subsumption when it collapses to one non-empty predicate.
- Extended restricted mutating Cypher `MATCH ... WHERE` negated simple `OR`
  lowering for same-property null disjunctions, producing bounded `IS NOT
  NULL` plus negated equality or membership predicates.
- Added `GraphNativeConstraintCapability` and
  `GraphStore::apply_native_constraint` to `grust-core` so backends can
  declare whether they support native index or native-enforcing constraint DDL
  for a given `GraphConstraint` and then handle explicit native DDL requests
  independently of `apply_schema`. The default implementation returns
  `Unsupported`, keeping Sail's read-before-write uniqueness honest until a
  backend-native unique constraint implementation exists.
- Implemented native graph constraint application for `MemoryGraphStore`:
  required and unique node or edge property constraints can now be explicitly
  applied, validated against the current graph, skipped with `if_not_exists`,
  and enforced on later writes without requiring typed `GraphSchema` metadata.
- Added `apply_cypher_native_constraints` in `grust-cypher` so parsed
  `CREATE CONSTRAINT` DDL can be applied directly through
  `GraphStore::apply_native_constraint`; the helper preserves
  `IF NOT EXISTS` semantics and rejects `DROP CONSTRAINT` until native drop
  semantics exist.
- Added a reusable LakeCat catalog-event graph projection helper in the
  `grust-graph` facade, covering event, warehouse, namespace, and table nodes
  with stable catalog containment edges.
- Added a LakeCat catalog graph adapter in the `grust-graph` facade that
  converts LakeCat `nodes`/`edges` envelopes into validated Grust graphs.
- Added `CypherConstraintRegistry`, `NamedGraphConstraint`, and
  `CypherDdlApplicationReport` for applying parsed Cypher constraint DDL to
  named schema metadata before projecting the resulting `GraphConstraint`
  values into `GraphSchema`, including `IF NOT EXISTS` and `IF EXISTS`
  reporting and atomic multi-statement registry application while keeping
  backend-native DDL and migrations deferred.
- Added `CypherConstraintRegistry::from_schema` and `apply_to_schema` so parsed
  Cypher constraint DDL can update a schema's constraint set while preserving
  existing node and edge type metadata.
- Fixed writable Cypher `RETURN` mutation report aggregation so precise
  insert/update counters are preserved when returning execution runs and merges
  per-operation mutation reports.
- Added `apply_cypher_ddl_to_schema` and `CypherSchemaApplication` as a small
  schema-management helper that parses Cypher constraint DDL, updates a
  `CypherConstraintRegistry`, projects the resulting constraints onto an
  existing `GraphSchema`, and calls `GraphStore::apply_schema`.
- Fixed `apply_cypher_ddl_to_schema` to stage registry changes until
  `GraphStore::apply_schema` succeeds, so backend schema-validation failures do
  not leave the caller's named constraint registry ahead of the applied schema.
- Added narrow writable Cypher `RETURN count(*)` support over the already
  materialized restricted write-result table, while still rejecting mixed
  aggregate and non-aggregate projections.
- Extended narrow writable Cypher count support to `COUNT(variable)` for
  variables bound by the write plan, including concrete and row-producing
  variables, while rejecting unbound count targets.
- Extended narrow writable Cypher count support to `COUNT(variable.property)`,
  counting only non-null projected values over the restricted materialized
  write-result table.
- Extended narrow writable Cypher count support to `COUNT(DISTINCT variable)`
  and `COUNT(DISTINCT variable.property)` over the restricted materialized
  write-result table, while keeping grouping and `COUNT(DISTINCT *)` deferred.
- Added restricted writable Cypher `RETURN` support for `SUM`, `AVG`, `MIN`,
  and `MAX` over `variable.property` projections already present in the
  materialized write-result table, including `DISTINCT` value deduplication and
  null/missing-value exclusion.
- Added restricted writable Cypher `RETURN collect(...)` support over
  variables and `variable.property` values already present in the materialized
  write-result table, returning a `Value::Json` array with optional
  `DISTINCT` value deduplication.
- Added restricted writable Cypher `RETURN collect(*)` support over the same
  materialized write-result table, returning JSON row objects keyed by bound
  variable name and supporting grouped collection.
- Added restricted writable Cypher `RETURN *` support over variables already
  bound by the write plan, expanding to deterministic element columns without
  adding arbitrary read-query projection semantics.
- Added endpoint-aligned row values for row-producing writable Cypher
  relationship writes, so matched source and destination variables can be
  returned alongside the produced relationship without independent node scans.
- Added restricted writable Cypher map projections such as
  `RETURN n { .id, .label }` over variables already bound by the write plan,
  now extended to allow literal, parameter, and same-variable property entries
  while keeping arbitrary map expressions deferred.
- Added restricted writable Cypher list projections such as
  `RETURN [n.id, n.label]` over one variable already bound by the write plan,
  now extended to allow literal and parameter items in the same restricted
  single-variable list while keeping arbitrary list expressions deferred.
- Updated the writable Cypher planning docs to reflect the post-review
  implementation status and the next continuation batches for backend-native
  constraints, shared write-result rows, and future expression slices.
- Added an explicit backend-native graph constraint DDL surface in `grust-core`
  through `GraphNativeConstraintCapability`,
  `GraphNativeConstraintRequest`, `GraphNativeConstraintReport`, and
  `GraphStore::apply_native_constraint`, keeping native constraint/index DDL
  separate from portable `GraphStore::apply_schema`.
- Added an explicit internal writable-Cypher write-result row model in
  `grust-sail` for row-node and row-edge values, centralizing restricted
  `RETURN` row-count validation and deterministic row-variable ordering for
  `RETURN *` and `collect(*)`.
- Added restricted writable Cypher `RETURN CASE WHEN variable.property =
  literal THEN literal ELSE literal END` scalar projections over the existing
  write-result row model while keeping general expression evaluation deferred.
- Extended restricted writable Cypher `RETURN CASE` projections to accept
  `CypherMutationOptions::parameters` in the equality value and literal branch
  positions.
- Added restricted writable Cypher aggregates over the existing restricted
  `CASE WHEN variable.property = literal THEN literal ELSE literal END`
  projection form, supporting `COUNT`, `SUM`, `AVG`, `MIN`, `MAX`, and
  `COLLECT` while preserving the literal-only CASE grammar.
- Added backend-neutral relationship numeric property updates for writable
  Cypher, lowering `MATCH ... SET e.key = e.key + literal_or_parameter` and
  the corresponding `-`, `*`, and `/` forms into explicit matched-edge
  read-modify-write mutation operations for Memory and Sail.
- Added strict multi-target `MATCH ... DELETE` support for relationship
  patterns such as `DELETE e, a`, lowering relationship deletes and
  ID-resolved endpoint node deletes into ordered Grust mutation operations.
- Added backend-neutral relationship-row deletes for writable Cypher, lowering
  broad endpoint targets and mixed forms such as `DELETE e, a` into captured
  `DeleteRelationshipRows` operations implemented by Memory and Sail.
- Aligned the Sail backend proposal and Cypher implementation plan with the
  current writable Cypher public contract, including the `grust-cypher`
  parser/planner split, restricted returning surface, native constraint helper,
  and relationship-row delete semantics.
- Added restricted writable Cypher path-shaped `RETURN` support for
  row-producing `MATCH ... CREATE/MERGE` relationship writes that bind a path
  variable such as `CREATE p = (n)-[r:TYPE]->(t)`, returning aligned source
  node, relationship, and target node JSON while keeping path properties and
  resolved-edge paths deferred.
- Added restricted writable Cypher path-shaped `RETURN` support for existing
  matched relationship rows updated by `MATCH p = (a)-[e:TYPE]->(b) SET ...`
  or `REMOVE ...`, reusing the same path JSON shape and row alignment as other
  write-result path returns.
- Added restricted writable Cypher path-shaped `RETURN` support for
  relationship-only matched deletes such as
  `MATCH p = (a)-[e:TYPE]->(b) DELETE e RETURN p`, returning the pre-delete
  path rows.
- Extended deleted relationship path returns to mixed relationship-row endpoint
  deletes such as `MATCH p = (a)-[e:TYPE]->(b) DELETE e, a RETURN p`,
  snapshotting endpoint nodes before the delete so returned paths can describe
  graph elements removed by the same operation.
- Extended path-bound mixed relationship deletes to explicit-ID endpoint
  targets by routing them through row snapshots, so
  `MATCH p = (a {id: ...})-[e:TYPE]->(b) DELETE e, a RETURN p` returns the
  pre-delete path while still deleting the resolved endpoint node.
- Extended restricted writable Cypher path-shaped `RETURN` support to
  `count(p)`, `count(DISTINCT p)`, and `collect(p)` over row-producing path
  variables, reusing the same aligned path materialization used by `RETURN p`.
- Extended restricted writable Cypher path-shaped `RETURN` support to resolved
  single-edge `MATCH ... CREATE/MERGE p = (a)-[r:TYPE]->(b)` writes, including
  `RETURN p`, `count(p)`, and `collect(p)` over the concrete path binding.
- Added restricted writable Cypher path introspection projections
  `length(p)`, `nodes(p)`, and `relationships(p)` for writable path variables,
  reusing the same path materialization as `RETURN p`.
- Extended restricted writable Cypher aggregates to accept path introspection
  projections such as `sum(length(p))`, `avg(length(p))`,
  `collect(nodes(p))`, and `collect(relationships(p))` over writable path
  variables.
- Extended restricted writable Cypher `COUNT` and `COLLECT` aggregates to
  accept the existing restricted map and list projection forms.
- Added restricted writable Cypher literal `RETURN` projections and aggregate
  bodies, including parameters in literal positions and `count(1)`,
  `count(null)`, `sum(1)`, `avg(1)`, and `collect('value')` over the existing
  materialized write-result table.
- Added restricted writable Cypher `coalesce(...)` projections and aggregate
  bodies over one bound variable's properties plus literal or parameter
  fallbacks, while keeping nested functions and cross-variable expression
  evaluation deferred.
- Added restricted writable Cypher `labels(node)` and `type(relationship)`
  projections and aggregate bodies over variables already bound by the
  materialized write-result table.
- Added restricted writable Cypher `properties(element)` and `keys(element)`
  projections and aggregate bodies over bound node and relationship variables,
  returning deterministic JSON values from Grust's stored property maps.
- Added restricted writable Cypher `startNode(relationship)` and
  `endNode(relationship)` projections and aggregate bodies over bound
  relationship variables, materializing endpoint nodes through the existing
  writable result table.
- Added restricted writable Cypher `id(element)` and `elementId(element)`
  projections and aggregate bodies over bound node and relationship variables,
  reusing the existing physical identity projection semantics.
- Added restricted writable Cypher `exists(variable.property)` projections and
  aggregate bodies over bound node and relationship variables, returning
  booleans from the existing property materialization path.
- Added restricted writable Cypher `size(variable.property)` projections and
  aggregate bodies over bound node and relationship variables, returning
  lengths for string and array-like property values.
- Added restricted writable Cypher `variable.property[index]` projections and
  aggregate bodies over array-like property values with literal or parameter
  non-negative integer indexes.
- Added restricted writable Cypher `variable.property[start..end]` projections
  and aggregate bodies over array-like property values with literal or
  parameter non-negative integer bounds.
- Added restricted writable Cypher `needle IN variable.property` projections
  and aggregate bodies over array-like property values with literal or
  parameter scalar needles.
- Added restricted writable Cypher `any` / `all` / `none` / `single` list
  predicate projections and aggregate bodies over array-like property values
  with equality predicates against literal or parameter values.
- Added restricted writable Cypher `toStringList`, `toIntegerList`,
  `toFloatList`, and `toBooleanList` projections and aggregate bodies over
  array-like property values.
- Added restricted writable Cypher `head(variable.property)` and
  `last(variable.property)` projections and aggregate bodies over array-like
  property values.
- Added restricted writable Cypher `tail(variable.property)` projections and
  aggregate bodies over array-like property values.
- Added restricted writable Cypher `range(start, end[, step])` literal list
  projections and aggregate bodies with integer literal or parameter bounds.
- Added restricted writable Cypher `toLower(variable.property)` and
  `toUpper(variable.property)` projections and aggregate bodies over bound
  node and relationship variables, keeping string normalization explicit and
  type-aware.
- Added restricted writable Cypher `trim(variable.property)`,
  `lTrim(variable.property)`, and `rTrim(variable.property)` projections and
  aggregate bodies over bound node and relationship variables.
- Added restricted writable Cypher `substring(variable.property, start[, length])`
  projections and aggregate bodies with literal or parameter integer offsets.
- Added restricted writable Cypher
  `replace(variable.property, search, replacement)` projections and aggregate
  bodies with literal or parameter string search and replacement values.
- Added restricted writable Cypher `startsWith(variable.property, needle)`,
  `endsWith(variable.property, needle)`, and
  `contains(variable.property, needle)` projections and aggregate bodies with
  literal or parameter string needles.
- Added restricted mutating `MATCH ... WHERE variable.property IN [...]`
  predicate support, including list-valued parameters and one leading `NOT`,
  lowering membership checks through backend-neutral `GraphPropertyPredicate`
  operators.
- Added restricted mutating `MATCH ... WHERE` support for same-property
  equality `OR` groups by folding them into backend-neutral membership
  predicates while keeping general boolean expression trees deferred.
- Added restricted mutating `MATCH ... WHERE NOT (...)` support for those same
  same-property equality `OR` groups by folding them into backend-neutral
  membership exclusion predicates.
- Extended the restricted mutating `MATCH ... WHERE` `OR` fold to combine
  same-property equality and membership predicates into one backend-neutral
  membership predicate, including the matching negated exclusion form.
- Added restricted mutating `MATCH ... WHERE` support for same-property string
  predicate `OR` groups such as repeated `STARTS WITH`, `ENDS WITH`, or
  `CONTAINS`, lowering them to backend-neutral grouped string predicates.
- Extended the restricted mutating `MATCH ... WHERE` boolean grammar to factor
  positive `OR` branches whose `AND` groups share identical bounded predicates
  and differ by one foldable same-property predicate, while still rejecting
  unfactorable general boolean expressions.
- Extended that factored `OR`-of-`AND` lowering to allow common branch terms
  that are themselves foldable parenthesized `OR` groups, preserving the flat
  backend-neutral predicate vector.
- Canonicalized restricted mutating `MATCH ... WHERE` lowering by removing
  exact duplicate bounded predicates after parsing and `OR` folding while
  preserving deterministic predicate order.
- Canonicalized each candidate factored `OR` branch before branch comparison so
  duplicate bounded predicates inside a branch do not block otherwise valid
  `OR`-of-`AND` lowering.
- Canonicalized folded mutating `MATCH ... WHERE` `OR` groups by removing exact
  duplicate membership values or grouped string needles while preserving
  first-seen order.
- Canonicalized repeat same-property membership filters in mutating
  `MATCH ... WHERE` by intersecting representable positive `IN` predicates and
  unioning repeat `NOT IN` exclusions.
- Represented empty same-property positive membership intersections as one
  empty `IN` predicate, giving mutating `MATCH ... WHERE` a backend-neutral
  no-match filter without adding a new predicate operator.
- Canonicalized same-property equality and membership combinations in mutating
  `MATCH ... WHERE`, including equality selected by `IN`, equality excluded by
  `NOT IN`, conflicting equality, and `IN` minus `NOT IN`.
- Canonicalized same-property scalar inequality combinations in mutating
  `MATCH ... WHERE`, including equality conflicts, repeated `<>` exclusions,
  `IN` minus `<>`, and `NOT IN` plus `<>`.
- Canonicalized same-property ordered comparison ranges in mutating
  `MATCH ... WHERE`, keeping stricter lower or upper bounds and collapsing
  impossible ranges to empty `IN`.
- Canonicalized same-property equality plus ordered range predicates in
  mutating `MATCH ... WHERE`, keeping equality when it satisfies the range and
  lowering out-of-range equality to empty `IN`.
- Canonicalized same-property positive membership plus ordered range
  predicates in mutating `MATCH ... WHERE`, filtering `IN` lists to values that
  satisfy the range and lowering fully excluded lists to empty `IN`.
- Canonicalized each factored mutating `MATCH ... WHERE` `OR` branch with the
  same bounded-predicate pipeline used by top-level `AND`, allowing
  branch-local equality, membership, inequality, and range simplifications to
  expose a backend-neutral common-predicate plus same-property fold shape.
- Pruned impossible factored mutating `MATCH ... WHERE` `OR` branches after
  branch-local canonicalization, lowering single-survivor groups directly and
  all-impossible groups to the existing empty `IN` no-match predicate.
- Pruned subsumed factored mutating `MATCH ... WHERE` `OR` branches after
  canonicalization, so narrower conjunctions such as `(A AND B) OR A` lower to
  the broader backend-neutral predicate set.
- Extended factored mutating `MATCH ... WHERE` `OR` branch subsumption with
  conservative same-property predicate implication for equality, membership,
  negated membership, scalar inequality, and ordered-bound predicates.
- Extended factored mutating `MATCH ... WHERE` branch subsumption to prune
  stricter same-direction ordered bounds when a sibling branch already accepts
  the broader range predicate.
- Consolidated restricted writable Cypher `RETURN` parsing so scalar
  projections and aggregate bodies share one return-target recognizer for the
  existing literal, map/list, path-helper, introspection, string, numeric,
  conversion, `coalesce`, and `CASE` forms.
- Added restricted writable Cypher `left(variable.property, length)` and
  `right(variable.property, length)` projections and aggregate bodies with
  literal or parameter integer lengths.
- Added restricted writable Cypher `reverse(variable.property)` projections
  and aggregate bodies over string and array property values.
- Added restricted writable Cypher `split(variable.property, delimiter)`
  projections and aggregate bodies with non-empty literal or parameter string
  delimiters, returning JSON string arrays.
- Added restricted writable Cypher `isEmpty(variable.property)` projections
  and aggregate bodies over string, array, and JSON collection property
  values.
- Added restricted writable Cypher `toString(variable.property)` projections
  and aggregate bodies over scalar property values.
- Added restricted writable Cypher `abs(variable.property)` projections and
  aggregate bodies over numeric property values.
- Added restricted writable Cypher `ceil(variable.property)` and
  `floor(variable.property)` projections and aggregate bodies over numeric
  property values.
- Added restricted writable Cypher `sign(variable.property)` projections and
  aggregate bodies over numeric property values.
- Added restricted writable Cypher `toInteger(variable.property)` and
  `toFloat(variable.property)` projections and aggregate bodies over numeric
  and numeric-string property values.
- Added restricted writable Cypher `toBoolean(variable.property)` projections
  and aggregate bodies over boolean and boolean-string property values.
- Added restricted writable Cypher grouping for mixed scalar and aggregate
  `RETURN` projections, grouping only by scalar projections over the
  materialized write-result table and then applying the existing
  `ORDER BY`/offset/limit controls.
- Added restricted writable Cypher `RETURN` rows for broad
  `MATCH ... DELETE` node and relationship writes by capturing the matched
  rows before deletion and projecting those pre-delete values after execution.
- Added ignored live Sail regression coverage for broad
  `MATCH ... DELETE ... RETURN` node and relationship writes, covering the
  native returning path in addition to the Memory/Sail helper path.
- Added opt-in generated relationship IDs for row-producing
  `MATCH ... CREATE` edge writes through
  `CypherRelationshipIdPolicy::GenerateForRowCreate`, and for row-producing
  `MATCH ... CREATE/MERGE` edge writes through
  `GenerateForRowCreateAndMerge`, backed by
  backend-neutral `GraphRowEdgeIdPolicy` metadata and deterministic
  `generated_row_edge_id` generation shared by Sail and Memory.
- Added row-level `RETURN DISTINCT` support for writable Cypher's restricted
  materialized result tables, with deduplication applied before existing
  `ORDER BY`, `SKIP`, and `LIMIT` controls.
- Extended writable Cypher `RETURN ORDER BY` to accept returned projection
  expressions, such as `ORDER BY n.name` when `n.name AS name` is projected,
  while still rejecting non-returned expressions.
- Added `OFFSET` as a writable Cypher `RETURN` control synonym for `SKIP` over
  the restricted materialized result table.
- Added explicit relationship `id` support for row-producing
  `MATCH ... CREATE/MERGE` edge writes when the matched endpoint row set
  produces exactly one edge, while rejecting multi-row fan-out with one literal
  relationship id.
- Fixed generic writable Cypher returning execution so
  `collect_written_edge_identities` can report row-producing
  `MATCH ... CREATE/MERGE` edge identities instead of rejecting that plan shape.
- Added `LIMIT ALL` support to writable Cypher `RETURN` control clauses,
  matching the existing read-query spelling while preserving numeric `LIMIT`
  behavior.
- Added serde serialization support for Cypher constraint DDL helper types,
  including `CypherConstraintRegistry`, so callers can persist named
  constraint metadata outside backend-native schema storage.
- Added `CypherConstraintRegistry::to_json` and `from_json` convenience helpers
  for caller-owned named constraint metadata persistence with Grust error
  mapping.
- Added Sail-owned `save_cypher_constraint_registry` and
  `load_cypher_constraint_registry` helpers that persist named registry JSON in
  a `grust_cypher_constraint_registry` table while keeping native backend
  constraint/index DDL and migrations deferred.
- Added `CypherSchemaManager` to keep a `GraphSchema` and named Cypher
  constraint registry together while applying Cypher DDL through
  `GraphStore::apply_schema` with success-only state updates.
- Added precise insert-versus-update classification to `GraphMutationReport`
  through `node_inserts`, `node_updates`, `edge_inserts`, and `edge_updates`,
  populated during plan execution by backends that can distinguish create from
  replace (the in-memory executor, Sail resolved node/edge upserts, and Sail
  and Memory row-producing MERGE/CREATE edges); unresolved upsert-only paths
  continue to report through the existing `*_upserts` totals when the backend
  cannot classify the write outcome.
- Added `ORDER BY`, `SKIP`, and `LIMIT` support to the writable Cypher `RETURN`
  slice, applied as a stable post-materialization step shared by Sail and the
  backend-neutral Memory returning helper, while still rejecting grouping and
  path returns.
- Changed `SailGraphStore` to validate unique-property constraints before writes
  through a read-before-write existence check in `put_node`, `put_edge`, and
  `put_graph`, and to report `ValidateBeforeWrite` instead of metadata-only for
  node and edge uniqueness.
- Added Cypher schema (DDL) parsing through `sail_cypher_ddl` and
  `sail_cypher_constraints`, turning `CREATE CONSTRAINT` and `DROP CONSTRAINT`
  statements into backend-neutral `CypherDdlStatement` / `GraphConstraint`
  values for node and edge uniqueness and `IS NOT NULL`, kept separate from the
  data-mutation plan and rejecting composite/node-key and index DDL.
- Added a batched `GraphStore::get_nodes` override for `SailGraphStore` that
  reads all requested ids in one `IN (...)` query instead of one round trip per
  id, matching the input-order, duplicate, and skip-missing default contract.
- Changed the Sail writable-Cypher scanners to share a single quote-aware
  `scan_unquoted` helper and changed the four fully-static degree SQL builders
  to return `&'static str` instead of allocating a `String` per call.
- Added a strict first writable Cypher `RETURN` slice for Sail through
  `CypherMutationTableResult` and `CypherResultTable`, allowing final
  property projections over node variables and concrete relationship variables
  already resolved by the write plan, including concrete edge upserts and edge
  patches, while keeping mutation reports count-oriented and rejecting
  aggregation, paths, ordering, limiting, broad matched-row result tables, and
  arbitrary read-query features.
- Fixed `MemoryGraphStore` to preserve parallel edges when they carry distinct
  explicit edge IDs, so the deterministic test backend matches Grust's
  identity model for id-bearing multi-edges.
- Fixed Sail matched relationship deletes to delete by the persisted
  `edge_key` selected by the relationship match, preserving sibling parallel
  edges when an explicit edge ID narrows the match.
- Added strict `CREATE` conflict checks to the generic writable Cypher
  `RETURN` helper for concrete node and edge writes, keeping row-producing
  edge strict checks backend-specific.
- Fixed strict writable Cypher `CREATE` preflight to reject duplicate concrete
  node or edge identities inside the same planned batch before any writes run.
- Added `n.label` and `e.label` projections to the strict writable Cypher
  `RETURN` slice for concrete bound node and relationship variables.
- Added concrete bound node and relationship element projections such as
  `RETURN n AS node, e AS relationship`, returned as `Value::Json` using the
  existing Grust `Node` / `Edge` serde shape.
- Added Sail writable Cypher `RETURN` rows for row-producing
  `MATCH ... CREATE/MERGE` relationship variables such as
  `RETURN e.label, e.source`.
- Added the same row-producing relationship `RETURN` support to the
  backend-neutral Memory/Sail returning helper for upsert-compatible execution.
- Added portable writable Cypher `RETURN` rows for restricted broad node
  `MATCH ... SET/REMOVE` writes, so the Memory/Sail returning helper can return
  post-write projections such as `RETURN n.id, n.seen` for matched node rows.
- Added portable writable Cypher `RETURN` rows for restricted broad
  relationship `MATCH ... SET/REMOVE` writes, returning post-write projections
  such as `RETURN e.id, e.seen` for matched edge rows.
- Fixed writable Cypher `RETURN` parsing so aliases such as `AS limit` and
  `AS skip` no longer trip the `LIMIT` / `SKIP` clause rejection.
- Added backend-neutral graph constraint metadata for required and unique node
  or edge properties, plus constraint capability reporting so backends can
  distinguish metadata-only constraints from validate-before-write behavior.
- Added portable unique-property validation to `GraphSchema::validate_graph`
  and wired the memory backend to reject duplicate unique node or edge
  properties before writes when a schema is applied.
- Added opt-in Sail writable Cypher node and edge identity payloads through
  `CypherMutationOptions::collect_written_node_identities`,
  `CypherMutationOptions::collect_written_edge_identities`,
  `CypherMutationResult::written_node_identities`, and
  `CypherMutationResult::written_edge_identities`, covering explicit and
  generated node writes plus resolved and row-producing edge writes without
  changing the count-oriented mutation report.
- Added Sail writable Cypher support for comma-separated `MATCH ... SET`
  assignments, preserving source order across literal patches, map patches,
  remove-on-null compatibility, and numeric node property updates.
- Added row-producing Sail writable Cypher `MATCH ... MERGE` for edges whose
  endpoints come from matched node variables, reusing the row materialization
  and backend-neutral execution path introduced for row-producing
  `MATCH ... CREATE`.
- Fixed Sail writable Cypher edge/node pattern classification so `->` inside a
  string literal no longer misclassifies a node pattern as an edge pattern.
- Added row-producing Sail writable Cypher `MATCH ... CREATE` for edges whose
  endpoints come from matched node variables, with backend-neutral planning,
  Sail and Memory execution, strict-create conflict checks, and ignored live
  Sail coverage for zero-, one-, and many-row creates.
- Added a bounded writable Cypher `MATCH ... WHERE` predicate grammar for Sail,
  lowering `AND`-joined property comparisons into backend-neutral
  `GraphPropertyPredicate` values that Memory can evaluate and Sail can lower
  to SQL, now including one leading `NOT` before a supported comparison and
  explicit `IS NULL` / `IS NOT NULL` property checks, with parentheses around
  supported predicate terms and `AND` groups, and restricted string predicates
  using `STARTS WITH`, `ENDS WITH`, and `CONTAINS`.
- Added opt-in strict `CREATE` execution for Sail writable Cypher through
  `CypherMutationOptions` and `CypherCreateMode::ErrorIfExists`, preserving the
  default upsert-compatible path.
- Added backend-neutral node patch mutations and Sail writable Cypher lowering
  for strict `MATCH ... SET n += { ... }` node map patches.
- Added cardinality-aware Sail writable Cypher planning and execution for broad
  node `MATCH ... DELETE`, including matched-row and changed-element mutation
  report fields plus ignored live Sail cascade coverage.
- Polished Sail writable Cypher parsing with case-insensitive top-level
  mutation keywords and comment stripping outside string literals.
- Added structured Cypher error variants for syntax, unresolved identity,
  unsupported cardinality, and execution failures while keeping execution
  Sail-specific over backend-neutral mutation plans.
- Added backend-neutral matching-node patch planning and Sail execution for
  broad node `MATCH ... SET n += { ... }`, including matched-row reporting and
  typed-node mirror updates through the existing node load path.
- Added backend-neutral edge patch mutations and Sail lowering for ID-resolved
  `MATCH ... SET e += { ... }`, with typed-edge mirror updates through the
  existing edge load path.
- Added Sail writable Cypher lowering for literal property assignment and
  explicit `REMOVE` on resolved node and edge identities, backed by
  backend-neutral property remove mutations and existing patch/load paths.
- Added backend-neutral matching-node property removal plus Sail and Memory
  execution for broad node `MATCH ... SET n.key = value` and
  `MATCH ... REMOVE n.key`, preserving literal-only assignment and matched-row
  reporting.
- Added Sail writable Cypher planning for resolved edge
  `MATCH ... CREATE`, reusing explicit-ID endpoint bindings and preserving
  strict `CREATE` intent for execution options.
- Added backend-neutral relationship match descriptors plus Sail and Memory
  execution for broad relationship `MATCH ... DELETE`, `SET`, and `REMOVE`
  mutations over endpoint label/property predicates and optional edge `id`.
- Extended relationship match descriptors to carry relationship property
  predicates beyond `id`, with Sail SQL lowering and Memory execution for
  broad relationship delete, patch, assignment, and removal.
- Added Sail writable Cypher parameters through
  `CypherMutationOptions::parameters`, limited to literal positions such as
  IDs, property maps, and literal property assignments.
- Added minimal Sail writable Cypher numeric node property updates such as
  `MATCH (n:Counter {id: 'c1'}) SET n.count = n.count + 1`, lowering through
  backend-neutral read-modify-write mutation plans shared by Sail and Memory.
- Added `CypherNullAssignment` and
  `CypherMutationOptions::null_assignment` so callers can opt into
  Cypher-compatible `SET x.key = null` property removal while preserving
  `Value::Null` storage by default.
- Added opt-in generated node IDs for Sail writable Cypher node `CREATE`
  through `CypherNodeIdPolicy::GenerateForCreate` and
  `CypherMutationResult::generated_node_ids`, while keeping explicit IDs as the
  default and preserving resolved edge endpoint requirements.
- Added the backend-neutral `CypherMutationExecutor` plan-execution facade and
  implemented it for Sail and Memory, allowing Sail-planned writable Cypher to
  execute deterministically on the in-memory backend.
- Added `GraphMutationAtomicity` as an optional mutation-batch capability marker
  and tests documenting default ordered/non-atomic partial-failure behavior.
- Added an internal Sail writable-Cypher parser front-door that classifies
  top-level mutation statements before lowering while preserving the existing
  Sail-owned parser.

## 2026-06-15 - 0.8.4

- Extended strict writable Cypher planning in `grust-sail` with ID-resolved
  `MATCH ... DELETE` for single node or edge patterns.
- Added ID-resolved `MATCH ... MERGE` edge planning, allowing explicit-ID node
  matches to bind variables used by one relationship `MERGE`.
- Documented the remaining writable Cypher completion batches in
  `docs/CypherWrite.md`.

## 2026-06-14 - 0.8.3

- Extended strict writable Cypher planning in `grust-sail` to accept ordered
  multi-statement mutation batches and aggregate mutation reports across the
  whole batch.
- Added local node variable binding for writable Cypher batches, allowing
  explicit-ID node patterns to bind variables and later edge or delete patterns
  to reuse those variables while rejecting unbound references and conflicting
  rebinding.

## 2026-06-14 - 0.8.2

- Added backend-neutral `GraphMutationPlan`, `GraphMutationPlanOp`, and
  `GraphMutationReport` types in `grust-core` for resolved graph mutation
  planning.
- Added strict v1 writable Cypher support in `grust-sail`, including
  `sail_cypher_mutation_plan` and `SailGraphStore::execute_cypher_mutation`.
  The v1 subset supports explicit-ID node `CREATE`/`MERGE`, resolved endpoint
  edge `CREATE`/`MERGE`, and resolved node/edge `DELETE` through existing
  `GraphMutationStore` semantics.
- Added unit tests and an ignored live Sail integration test for writable
  Cypher planning and execution.

## 2026-06-14 - 0.8.1

- Added Sail Delta table properties for typed graph tables, marking generated
  node and edge tables with `grust.graph.kind` and `grust.graph.label`
  metadata for downstream planners.
- Added public Sail constants for graph table property names and values.
- Added an ignored live Sail test covering Cypher `MATCH` over Grust backend
  tables, including outgoing, incoming, undirected, and `LIMIT ALL` query
  forms.

## 2026-06-14 - 0.8.0

- Added `GraphIndex` to `grust-core` as a shared dense adjacency layer for
  local analytics, backend planning, and adapters that need validated edge
  endpoint indexes.
- Added a dependency-free `benchmarks` example in `grust-graph` with ring,
  grid, layered DAG, clustered, Graph500-style R-MAT, and GAP-style R-MAT graph
  families for core graph/index operations.
- Added Sail graph analytics helpers for reading the persisted generic graph
  tables and computing in-degree, out-degree, total degree, and directed degree
  pairs through Spark SQL.
- Added public Sail table/column contract helpers for generic and typed graph
  planning, including field projection helpers shared with GrustFrames-style
  lowerings.
- Added Sail typed-table descriptors and directional triplet SQL helpers for
  GrustFrames-style triplet filters, motifs, and aggregate-message lowerings.
- Changed Sail generic edge persistence to keep staged `edge_key` and optional
  explicit edge `id` columns in `grust_edges`, so read-back and external
  planners can preserve stable edge identity.
- Changed structural `edge_key` construction to preallocate and append instead
  of using `format!`, reducing allocation overhead in graph-index and benchmark
  paths.

## 2026-06-13 - 0.7.2

- Extended `grust-ladybug` to expose Ladybug typed and untyped graph modes
  explicitly through `LadybugGraphMode`, `LadybugConfig::typed`, and
  `LadybugConfig::untyped`.
- Changed `grust-ladybug` to preserve an applied `GraphSchema` and validate
  later node, edge, and graph writes against it, while keeping untyped dynamic
  graph writes as the default mode.
- Changed Ladybug `clear` to recreate applied schema tables so typed-mode
  stores remain ready for validated writes after reset.
- Updated README, the Ladybug backend proposal, the Grust book, and the
  overview blog to describe Ladybug as supporting both typed and untyped graph
  usage rather than only schema-first usage.

## 2026-06-13 - 0.7.1

- Added Arrow IPC data-source support for `grust-ladybug`, including embedded
  Ladybug node-table, relationship-table, CSR relationship-table, Arrow query,
  and Arrow table drop helpers behind the `arrow` feature.
- Added `grust-graph`'s `ladybug-arrow` facade feature so applications can
  enable embedded LadybugDB Arrow support through the main package.
- Added Sail Arrow IPC APIs for staging arbitrary Arrow streams as session temp
  views, collecting Spark SQL results as Arrow IPC chunks, and loading
  Grust-shaped node/edge IPC streams through the normal graph write path.
- Documented the Arrow IPC boundary in `docs/Arrow.md`, including why Grust
  avoids requiring one exact Rust `arrow` crate version across Ladybug and Sail.

## 2026-06-13 - 0.7.0

- Added `grust-ladybug`, an embedded LadybugDB backend using the Rust `lbug`
  crate directly for schema-backed graph writes, reads, and traversal.
- Proposed `grust-ladybug` as a schema-first embedded LadybugDB backend, with
  notes on storage layout, `lbug` integration, traversal lowering, and testing.

- Added `#[must_use]` diagnostics to graph builder completion methods so
  accidentally discarded builder results warn at compile time.
- Added `cocoindex_export_to_graph` so CocoIndex target-state JSON can be
  loaded back into Grust graphs.
- Changed the `grust-graph` memory facade and prelude exports to re-export the
  full `grust-memory` crate surface, matching other backend feature exports.
- Expanded CocoIndex adapter coverage for zero-edge exports, missing source
  nodes, explicit edge IDs, and non-finite float export errors.
- Documented the portable `PutOutcome` and `GraphSchema::apply_schema`
  contracts so backend-specific upsert and schema-enforcement behavior is
  explicit.
- Changed `Value::DateTime` to store an opaque validated `RfcDate`, including
  validating serde deserialization for tagged date-time values.
- Removed the unused `id` field from `GraphMutation::DeleteEdge`; edge deletes
  are represented by `(from, label, to)`.
- Replaced per-operation FalkorDB Redis connection creation with a reusable
  connection pool.
- Changed Sail read filters to pass Spark Connect named arguments instead of
  inlining literals into SQL text, and changed Sail deletes to stage values in
  Arrow temp views before running argument-free SQL commands.
- Changed FalkorDB schema and write paths to share the canonical lower_snake
  schema identifier normalizer for node labels.
- Expanded SurrealDB response-parser unit coverage across string, object,
  typed-object, and backtick-quoted record ID shapes.

## 2026-06-13 - 0.6.8

- Added typed readback helpers: `TypedNode::from_node`,
  `TypedNode::from_node_with`, `TypedEdge::from_edge`, and
  `TypedEdge::from_edge_with`.
- Preserved existing typed `id` properties during `TypedGraphBuilder` lowering
  so domain IDs can round-trip through stored Grust nodes.
- Added typed round-trip tests through `MemoryGraphStore`.

## 2026-06-13 - 0.6.7

- Documented that the default `GraphMutationStore::apply_mutations`
  implementation is ordered but non-atomic.
- Added transactional `apply_mutations` overrides for pgGraph and SurrealDB so
  mutation batches are wrapped in backend transactions.
- Added pgGraph mutation support and SurrealDB HTTP/SDK mutation support for
  node deletes, edge deletes, and ordered mutation batches.

## 2026-06-13 - 0.6.6

- Replaced LanceDB `Start::NodesByProperty` JSON substring matching with exact
  property comparison after reading label-filtered rows, avoiding false
  positives from nested JSON or serialized property fragments.

## 2026-06-13 - 0.6.5

- Changed SurrealDB generic edge reads to return a clear configuration error
  when `SurrealConfig.relationships` is empty, instead of silently returning no
  edges from an empty table scan.
- Preserved explicit SurrealDB edge-label reads without requiring
  `SurrealConfig.relationships`, so callers can still query a known relation
  table directly.

## 2026-06-12 - 0.6.4

- Added `GraphStore::get_nodes` as an additive batch-read API with a default
  repeated-`get_node` implementation.
- Added native `get_nodes` overrides for memory, LanceDB, pgGraph, and
  SurrealDB stores.
- Updated LanceDB and SurrealDB traversal paths to batch target-node reads per
  traversal step instead of issuing one node read per traversed edge.

## 2026-06-12 - 0.6.3

- Preserved supported non-string properties in Helix node and edge writes
  instead of silently dropping them; unsupported JSON object properties now
  return an explicit error.
- Moved shared relationship-type and structural edge-key helpers into
  `grust-core`, reducing duplicated backend lowering logic.
- Tightened pgGraph JSON property-key validation so generated JSONB
  expressions only accept safe identifier-shaped keys.
- Simplified SurrealDB HTTP authentication through reqwest's Basic auth helper
  and selected the SurrealDB SDK namespace/database once at connection time.
- Added `docs/INTEGRATION.md` as the contributor-facing guide for backend
  integration tests, including Docker, source-checkout, quick, full, and CI
  workflows.
- Added integration-test launcher profiles:
  - `quick` for local LanceDB and CocoIndex checks;
  - `docker` for Docker-backed contributor runs;
  - `all` for the full maintainer matrix.
- Added launcher modes:
  - `auto` to prefer already-running services, then source checkouts, then
    Docker where available;
  - `docker` to avoid source checkouts and use Compose-backed services;
  - `source` to avoid Docker and use local backend checkouts.
- Added `scripts/integration-test.sh doctor` to report selected backends,
  startup mode, Docker availability, source checkout state, ports, and Docker
  image choices before a long integration run.
- Pinned contributor Docker images for reproducible integration runs while
  keeping `GRUST_INTEGRATION_IMAGE_CHANNEL=latest` as an explicit compatibility
  lane.
- Hardened pgGraph startup so an occupied PostgreSQL-compatible port is only
  reused if the `graph` extension is available; otherwise Docker-capable modes
  automatically start Grust's pgGraph container on a free fallback port.

## 2026-06-12 - 0.6.2

- Expanded the backend integration launcher to run the full backend family by
  default: Sail, SurrealDB, FalkorDB, HelixDB, LanceDB, CocoIndex, and pgGraph.
- Added pgGraph Docker coverage with the official
  `ghcr.io/evokoa/pggraph:0.1.7` image on host port `55432`, so the pgGraph
  integration test no longer depends on a manually installed local PostgreSQL
  extension.
- Added HelixDB live integration coverage through a disposable local Helix
  project started from the configured `~/src/HelixDB` checkout.
- Added explicit LanceDB and CocoIndex integration checks to the shared
  launcher, covering local LanceDB persistence/traversal and CocoIndex public
  export shape.
- Fixed HelixDB live read hydration for current Helix responses by reading
  nested `properties` payloads, `$id` node identifiers, and `$from`/`$to` edge
  endpoints.
- Fixed pgGraph table registration against the current extension API by passing
  node and edge tables as `regclass` values instead of plain text names.
- Updated README, the Grust book, and the overview blog so backend integration
  instructions describe the full real-test matrix instead of the earlier
  three-backend subset.

## 2026-06-12 - 0.6.1

- Added an explicit backend integration-test launcher:
  - `scripts/integration-test.sh`
  - `integration/backends.conf`
  - `docker-compose.integration.yml`
- Made live backend tests visible and intentional instead of silently passing
  when a service is absent. Live tests are now ignored in ordinary unit-test
  runs and exercised through the launcher.
- Configured the launcher to prefer local source checkouts for Sail,
  SurrealDB, FalkorDB, and HelixDB, with Docker Compose fallback for
  Docker-friendly backends.
- Added live FalkorDB and SurrealDB integration tests to complement the
  existing Sail live tests.
- Fixed Sail live-test reset behavior by dropping and recreating Delta tables,
  including typed schema tables, instead of relying on fragile deletes.
- Hardened Sail SQL execution for the current Spark Connect/Sail behavior by
  inlining validated literal arguments when server-side SQL parameters are not
  accepted.
- Kept Sail traversal joins keyed on globally unique node IDs so single-edge
  writes with unknown endpoint labels still traverse correctly.
- Fixed SurrealDB live traversal by:
  - running the live HTTP test inside a Tokio runtime;
  - ensuring bootstrap creates the generic `record` fallback table;
  - creating missing relation tables before idempotent relation upserts;
  - normalizing Surreal record keys such as ``person:`person-1` `` back to
    Grust node IDs.
- Updated README, Sail backend notes, the Grust book, book metadata notes, and
  the overview blog for the `0.6.1` release and current `GraphStore` return
  types.
- Rebuilt the Grust PDF, EPUB, MOBI, and version marker artifacts for `0.6.1`.

## 2026-06-12 - 0.6.0

- Released Grust `0.6.0`.
- Added the `GraphMutationStore` path for incremental upserts and deletes
  where a backend can support mutation semantics beyond replacement.
- Expanded `PutOutcome` and updated write paths so single-element writes can
  report inserted, updated, deduped, or backend-opaque upserted outcomes.
- Extended `Value` and `FieldType` with timestamp and numeric-array support,
  including validation for RFC 3339 datetime strings.
- Wired schema edge uniqueness and undirected endpoint validation through the
  core schema path.
- Improved schema validation performance by indexing node labels for edge
  validation.
- Tightened Sail correctness and safety:
  - traversal joins use node IDs instead of empty endpoint-label columns;
  - property keys and non-finite floats are rejected before SQL generation;
  - single-edge writes validate and mirror into typed edge tables;
  - Arrow IPC staging is used for bulk node and edge batches.
- Improved memory-store edge validation so `put_edge` no longer clones the
  whole graph for every edge.
- Updated book and blog artifacts for the release.

## 2026-06-11 - 0.5.0

- Released Grust `0.5.0`.
- Added schema-backed typed storage across the backend family:
  - memory validates schema-backed writes;
  - LanceDB mirrors labeled rows into typed Arrow tables;
  - pgGraph exposes typed SQL views and expression indexes;
  - Sail mirrors schema-labeled rows into typed Delta tables;
  - SurrealDB lowers schemas into `DEFINE TABLE` and `DEFINE FIELD`;
  - FalkorDB creates useful label/property indexes.
- Updated the Grust book and overview blog to describe typed ingestion,
  schema-backed writes, and backend-specific typed storage surfaces.
- Polished book artifacts, metadata, page numbering, and Kindle-facing EPUB
  packaging.

## 2026-06-10 - 0.4.0

- Published the Elmarit `0.4.0` line.
- Added the optional `typed-garde` feature and `TypedGraphBuilder`.
- Added typed graph examples that validate Rust structs with `garde` and lower
  them into normal Grust nodes and edges.
- Added typed ingestion tests for coexistence with raw graph values and
  validation failures before graph construction.
- Documented the typed graph-builder design and release workflow.
- Hardened and documented the Grust book publishing pipeline:
  - separate generated cover;
  - stable `grust.epub` output;
  - versioned Send to Kindle symlink;
  - metadata validation;
  - visible table of contents;
  - PDF page numbering that starts after the cover.

## 2026-06-10 - 0.3.0

- Prepared and released the `0.3.0` workspace under the `querygraph/grust`
  repository identity.
- Updated repository and crate metadata to use `https://github.com/querygraph/grust`.
- Added release workflow documentation, including dependency-order publishing
  and registry verification.
- Continued book publishing work in preparation for the Elmarit line.

## 2026-06-07 - 0.2.0

- Released Grust `0.2.0`.
- Added JSON, YAML, and XML graph document loading and saving.
- Updated the Grust book for graph document formats and the import/export
  story.
- Renamed the public facade package to `grust-graph` while keeping the Rust
  library name `grust`, so downstream imports can continue to use
  `use grust::prelude::*`.
- Added a separate book cover build.

## 2026-06-06 - 0.1.x Publication Preparation

- Prepared the workspace crates for publication.
- Added Apache-2.0 and MIT license files.
- Added repository, homepage, keyword, category, and description metadata to
  the publishable crates.
- Started aligning README examples and crate manifests for crates.io.

## 2026-06-05 - Book

- Added the first Grust architecture book under `docs/book`.
- Documented the shape of the core model, traversal IR, store contract,
  backend architecture, and future design direction.

## 2026-06-02 - CocoIndex Adapter

- Added `grust-cocoindex`.
- Exported Grust graphs into CocoIndex-style node and relationship target
  state.
- Preserved stable node keys, endpoint labels, and plain JSON properties in the
  export adapter.

## 2026-06-01 - Backend Expansion

- Added and documented the Sail Spark Connect backend.
- Added pgGraph backend work and design notes.
- Added the LanceDB backend.
- Moved unit tests into crate-local test files.
- Updated README and backend proposals to describe the new backend family.

## 2026-05-31 - 0.1.0

- Created the initial Grust workspace.
- Added the core property graph model, graph builder, traversal IR, store
  traits, public facade crate, and deterministic in-memory store.
- Added the first backend graph stores.
- Switched graph stores to async HTTP/client patterns where appropriate.
