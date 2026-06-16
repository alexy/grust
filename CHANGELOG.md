# Changelog

All notable Grust changes are recorded here by date and release. This project
started before the changelog existed, so entries before 2026-06-12 were
reconstructed from Git history, release commits, and the shipped docs.

## Unreleased

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
- Added opt-in generated node IDs for Sail writable Cypher node `CREATE`
  through `CypherNodeIdPolicy::GenerateForCreate` and
  `CypherMutationResult::generated_node_ids`, while keeping explicit IDs as the
  default and preserving resolved edge endpoint requirements.

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
