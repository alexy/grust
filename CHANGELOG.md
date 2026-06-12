# Changelog

All notable Grust changes are recorded here by date and release. This project
started before the changelog existed, so entries before 2026-06-12 were
reconstructed from Git history, release commits, and the shipped docs.

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
