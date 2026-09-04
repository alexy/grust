# Grust Full39075 Completion Goal

Status: **COMPLETE (2026-07-03).** All 11 tasks (F1–F11) and the FM5 claim flip
are done: every non-rejected Grust-manifest feature is `Supported` (69 of 74;
the other 5 are intentional strict-write rejections), and `Full39075` is the
**realized internal** profile per `docs/GQL_PROFILE_STATEMENT.md`, pinned by
`gql::tests::full_profile_claim_is_backed`. This file originally sequenced the
11 remaining non-supported features into an executable plan for turning the
`PortableGql` implementation into the `Full39075` profile; it is kept as the
record of that plan and its completion. The profile name is not a formal or
exhaustive claim of ISO/IEC 39075 certification.

Source of truth:

- Feature statuses live in `crates/grust-cypher/src/gql.rs`.
- The current profile claim lives in `docs/GQL_PROFILE_STATEMENT.md`.
- The completion test `gql::tests::full_profile_claim_is_backed` must keep this
  file, the profile statement, and the manifest honest.

## Historical guardrails for this completed goal

1. **No release or publish unless explicitly requested.** Do not run
   `cargo publish`, release `cargo package`, `cargo info` registry verification,
   tag a release, or date `CHANGELOG.md`.
2. **Keep the test floor rising.** `cargo test -p grust-cypher --lib` must stay
   green and the original strict-write floor must not shrink.
3. **Every feature status flip requires evidence.** Moving a feature to
   `Supported` requires implementation, focused tests, manifest update, profile
   statement update, and an updated scoped-out list in
   `full_profile_claim_is_backed`.
4. **Prefer portable reference semantics first.** Native/pushdown support may
   follow, but each feature lands first in the Memory/reference path or is
   explicitly marked backend-scoped.
5. **Do not hide backend gaps.** Update backend descriptors when a feature needs
   per-backend capability reporting.
6. **Book rebuilds are only for public behavior/API changes.** When a task flips
   feature support or adds public API, update user-facing docs and rebuild the
   book artifacts as part of the task.

## Verification Gate

Run the narrowest useful gate during development, then this gate before marking a
task complete:

```sh
cargo test -p grust-cypher
cargo check -p grust-graph --features cypher,memory
cargo check -p grust-sail
cargo test -p grust-turso
git diff --check
```

Add backend-specific checks when touching backend descriptors, pushdown, or native
passthrough.

## Sequenced Gaps

| Task | Feature | Depends on | Status | Notes |
|---|---|---|---|---|
| **F1** | `index-definition` | existing constraint DDL | Done | Portable single-property index DDL metadata, registry tracking, manifest/profile flip, and backend `index_ddl` capability flag landed. Physical native indexes remain backend-specific. |
| **F2** | `graph-type-definition` | F1 | Done | Portable `CREATE/DROP GRAPH TYPE` metadata landed for node/edge labels, field types, and open/closed mode. Catalog selection remains F3/F4. |
| **F3** | `catalog-metadata` | F2 | Done | Portable catalog snapshots now expose named graph, graph type, index, and constraint metadata, with read-only procedure-style tables and backend `catalog_metadata` capability flags. |
| **F4** | `named-graph-selection` | F3 | Done | `USE <graph>` now parses into the AST, validates against explicit single-graph execution names or catalog snapshots, and defaults existing read execution to `USE default`. |
| **F5** | `session-control` | F4 | Done | Standalone `USE`, `SET`, `RESET`, and `RESET ALL` session commands now parse and update portable `CypherSession` state without changing transaction control behavior. |
| **F6** | `path-values` | current path bindings | Done | Fixed-length path bindings now return first-class `Value::Path(PathValue)` values with stable node/relationship JSON serialization. |
| **F7** | `graph-values` | F2, F3, F6 | Done | First-class `Value::Graph(GraphValue)` landed: deduplicated node/relationship sets, `graph(nodes, relationships)` construction in the read reference, `nodes()`/`relationships()` accessors, stable JSON serialization. |
| **F8** | `subquery` | F4, F5 | Done | `CALL { … }` correlated subqueries landed: outer bindings visible (import-all), WITH-style RETURN join with binding preservation, UNION arms, per-row execution, structured collision/RETURN-required/star rejections. |
| **F9** | `table-valued-function` | F8 | Done | `CALL name(args) [YIELD …]` generalized into TVF-style row sources with per-row correlated argument evaluation; registry: `db.*` catalog procedures + `tvf.range`, `tvf.keys`. |
| **F10** | `shortest-path` | F6 | Done | `shortestPath(…)`/`allShortestPaths(…)` over a single relationship segment landed: minimal-length simple paths per endpoint pair via iterative lengthening, with path/relationship/endpoint variable binding and first-class path values. |
| **F11** | `native-cypher-passthrough` | backend descriptors, F5 | Done | `NativeQuery`/`NativeQueryLanguage` escape-hatch surface landed with per-backend capability flags (`GqlBackend::native_passthrough`), structured non-support, new Falkor/Surreal catalog entries, and executable hatches: `FalkorGraphStore::run_native_cypher`, Surreal `run_native_surrealql`, Sail `query_arrow_ipc` (SQL). |

## Milestones

- **FM1 Schema & Catalog Base:** F1-F3. Index DDL, graph type DDL, and named
  catalog metadata exist with honest backend capabilities.
- **FM2 Session Model:** F4-F5. Named graph selection and session state are
  implemented and tested.
- **FM3 Value Model:** F6-F7. Path and graph values are first-class and wired
  through result serialization.
- **FM4 Query Generalization:** F8-F10. Subqueries, TVFs, and shortest paths are
  supported by the portable reference path.
- **FM5 Native Escapes & Claim Flip:** F11, then final profile hardening. At this
  point every non-rejected manifest feature should be `Supported`, the scoped-out
  list should contain only intentional rejections, and `Full39075` can become the
  realized profile if the profile statement supports that claim.
  **Done (2026-07-03):** the manifest reads supported 69 · rejected 5 ·
  planned 0 · future 0; the profile statement claims `Full39075` as realized;
  `full_profile_claim_is_backed` pins the scoped-out set to exactly the five
  intentional rejections.

## Completed Work Item: F1 Index Definition

Goal: implement portable index definition metadata without changing existing
constraint behavior.

Deliverables:

- Parser/AST support for a conservative index DDL surface. **Done.**
- Public metadata types matching the existing schema DDL style. **Done.**
- Backend capability flags for index DDL support/enforcement. **Done.**
- Manifest flip for `index-definition` from `Planned` to `Supported`. **Done.**
- Focused tests for parse success, structured rejection, manifest/profile
  consistency, and backend capability reporting. **Done for parse/registry and
  manifest/profile consistency; backend descriptor consistency remains covered by
  the existing backend manifest test.**

Open design points to resolve in code before flipping support:

- The first supported form covers node and relationship single-property indexes.
- Index names are registry-scoped, matching named constraints.
- Execution is metadata-only in the portable registry; callers must check native
  backend capabilities before relying on physical index creation.

## Completed Work Item: F3 Catalog Metadata

Goal: expose caller-owned DDL metadata through a portable catalog surface.

Deliverables:

- `CypherCatalogSnapshot` models named graph, graph type, index, and named
  constraint metadata. **Done.**
- `CypherConstraintRegistry::catalog_snapshot` materializes a single-graph
  catalog view while preserving registry JSON ownership. **Done.**
- `cypher_catalog_procedure` returns deterministic read-only metadata rows for
  `db.graphs`, `db.graphTypes`, `db.indexes`, and `db.constraints`. **Done.**
- `GqlBackendDescriptor::catalog_metadata` reports backend capability honestly.
  **Done.**
- Manifest/profile docs flipped `catalog-metadata` to `Supported`. **Done.**

## Completed Work Item: F4 Named Graph Selection

Goal: support `USE <graph>` without weakening single-graph execution honesty.

Deliverables:

- Lexer/parser/AST support for `USE <graph>` clauses. **Done.**
- Semantic feature reporting for named graph selection. **Done.**
- Single-graph read fallback semantics: the default path accepts no `USE` or
  `USE default`; `run_read_query_on_named_graph` binds a snapshot to any
  explicit graph name. **Done.**
- Catalog validation through `ensure_catalog_graph_selection`. **Done.**
- Backend descriptor capability flag and manifest/profile flip. **Done.**

## Completed Work Item: F5 Session Control

Goal: add portable session state commands while keeping transaction control
unchanged.

Deliverables:

- `CypherSession` tracks current graph and session settings. **Done.**
- `SessionCommand::parse` recognizes standalone `USE`, `SET name = literal`,
  `RESET name`, and `RESET ALL`. **Done.**
- `SessionCommand::apply` updates session state and validates `USE` against a
  catalog snapshot when one is provided. **Done.**
- `GqlBackendDescriptor::session_control` reports capability. **Done.**
- Manifest/profile docs flipped `session-control` to `Supported`. **Done.**

## Completed Work Item: F11 Native Passthrough

Goal: explicit backend-native escape hatches outside portable conformance.

Deliverables:

- `NativeQueryLanguage` (cypher / sql / surrealql) + `NativeQuery` types in
  the conformance spine; `ensure_native_passthrough` produces the structured,
  feature-tagged non-support error; `native_passthrough_backends` is the
  reverse lookup. **Done.**
- `GqlBackend::native_passthrough` capability table + descriptor field,
  verified against the public escape hatches in the working tree. **Done.**
- Catalog now includes **Falkor** and **Surreal** as `NativeGraphBackend`
  entries (honest flags: no portable executor; Surreal reports transactional
  atomicity). **Done.**
- Executable hatches: `FalkorGraphStore::run_native_cypher` (openCypher via
  GRAPH.QUERY), `SurrealHttpGraphStore`/`SurrealSdkGraphStore::
  run_native_surrealql`, existing Sail `query_arrow_ipc` (Spark SQL). **Done.**
- Manifest/profile docs flipped `native-cypher-passthrough` to `Supported`.
  **Done.**

## Completed Work Item: F10 Shortest Path

Goal: add shortest-path families on top of first-class path values.

Deliverables:

- `PathPattern::shortest` + `ShortestKind` AST and parser support for
  `shortestPath((a)-[…*…]->(b))` / `allShortestPaths(…)` wrappers (exactly one
  relationship segment; multi-segment forms are a structured syntax error).
  **Done.**
- Execution: per (start, end) endpoint pair, minimal-length **simple** paths
  found by iterative lengthening over the bounded var-length enumerator —
  first hit per endpoint is shortest by construction; `All` keeps same-length
  ties, `Single` keeps the first in deterministic edge order. Endpoint, edge
  list, and path variables bind exactly like the var-length path machinery;
  path variables return `Value::Path`. **Done.**
- Semantics report the `ShortestPath` feature; manifest/profile docs flipped
  `shortest-path` to `Supported`. **Done.**

## Completed Work Item: F9 Table-Valued Functions

Goal: generalize the read-only catalog procedures into TVF-style row sources.

Deliverables:

- `CallClause` carries argument expressions; the parser accepts
  `CALL name(arg, …)`. **Done.**
- Arguments are evaluated against each incoming row (correlated TVFs); the
  procedure's rows cross-join onto the row stream exactly like the nullary
  catalog procedures. **Done.**
- Registry: `db.labels`, `db.relationshipTypes`, `db.propertyKeys` (nullary,
  reject arguments) plus `tvf.range(start, end[, step]) YIELD value` and
  `tvf.keys(element_or_map) YIELD key`. **Done.**
- `YIELD` projection/aliasing/`WHERE` semantics unchanged; standalone `CALL`
  still shapes the result table. **Done.**
- Manifest/profile docs flipped `table-valued-function` to `Supported`. **Done.**

## Completed Work Item: F8 Subquery

Goal: implement `CALL { ... }` scoping and execution on the read reference.

Deliverables:

- `Clause::Subquery`/`SubqueryClause` AST + parser support (`CALL {` branches
  off the existing procedure `CALL`). **Done.**
- Correlated import-all scoping: the subquery sees the outer row's bindings;
  its `RETURN` columns join onto the outer scope, with structured errors for
  column collisions, missing `RETURN`, and `RETURN *`. **Done.**
- Execution: per-incoming-row evaluation with binding-preserving WITH-style
  RETURN (bare node/edge variables stay MATCH-extensible downstream), rows
  with empty subquery results dropped, `UNION`/`UNION ALL` arms combined with
  distinct dedup. **Done.**
- Manifest/profile docs flipped `subquery` to `Supported`. **Done.**

## Completed Work Item: F7 Graph Values

Goal: add first-class graph values with carefully scoped construction and
serialization.

Deliverables:

- `grust_core::GraphValue` and `Value::Graph` model set-shaped graph values:
  construction deduplicates nodes by id and relationships by
  id-or-endpoint-triple identity, preserving first-seen order. **Done.**
- `GraphValue::from_graph_parts` / `from_graph` build graph values from graph
  snapshots; `Value::to_json` serializes the `{nodes, relationships}` shape and
  the tagged serde form round-trips. **Done.**
- Read reference: `graph(nodes, relationships)` constructor over element lists
  (e.g. `collect(n)`), `nodes(g)`/`relationships(g)` accessors. **Done.**
- Manifest/profile docs flipped `graph-values` to `Supported`. **Done.**

## Completed Work Item: F6 Path Values

Goal: promote path bindings from executor-only JSON objects to first-class
values.

Deliverables:

- `grust_core::PathValue` and `Value::Path` represent node/relationship path
  values. **Done.**
- `Value::to_json` preserves the historical `{nodes, relationships}` path
  serialization shape. **Done.**
- Fixed-length path variable projections return `Value::Path`; `nodes(p)`,
  `relationships(p)`, and `length(p)` behavior remains compatible. **Done.**
- Manifest/profile docs flipped `path-values` to `Supported`. **Done.**
