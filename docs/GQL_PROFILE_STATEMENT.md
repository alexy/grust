# Grust GQL/Cypher — Profile Statement

This is the precise, backed statement of what Grust's GQL/Cypher layer conforms to
today. It began as the terminal deliverable of the GQL completion goal (U16) with
`Full39075` as a **candidate** claim; the Full39075 completion goal
(`docs/GQL_FULL39075_GOAL.md`, F1–F11) has since implemented every remaining
feature, so **`Full39075` is now the realized profile**, backed feature-by-feature
against the machine-readable manifest (`grust_cypher::gql::feature_manifest`),
not by prose.

**Source of truth:** the `GqlFeature` manifest in `crates/grust-cypher/src/gql.rs`.
This document must agree with it; the test `gql::tests::full_profile_claim_is_backed`
fails if the scoped-out set below drifts from the manifest.

## Profiles (nested)

`GqlConformanceProfile`, narrowest → widest:

1. **StrictWrite** — the original strict writable-Cypher surface (the 327-test
   floor): explicit-id `CREATE`/`MERGE`/`DELETE`/`SET`/`REMOVE`, resolved-endpoint
   edge writes, literal-only assignments, structured rejections.
2. **PortableGql** — StrictWrite **plus** the portable read core, expression
   engine, aggregates/`RETURN`, read pushdown, the type-system values, procedures,
   `CALL { … }` subqueries, transaction surface, and the U10b write widenings.
3. **Full39075** — the full ISO/IEC 39075 profile: PortableGql **plus** index and
   graph-type DDL metadata, catalog metadata, named graph selection, session
   control, first-class path and graph values, table-valued functions,
   shortest-path matching, and backend-native passthrough. **This is the
   realized profile today.**

## Realized profile (today): Full39075

**69 of 74 catalogued features are `Supported`** (implemented + tested); the
remaining 5 are intentional rejections. By family (run `support_summary()` for
the live list): parser/semantics, resolved writes, broad matched writes,
row-producing relationship writes, returning & aggregates, predicates &
expressions, read-only matching, path matching (incl. `shortestPath` /
`allShortestPaths`), query composition (incl. `CALL { … }` subqueries), type
system (temporal/duration/decimal/path/graph incl. arithmetic, ordering, and
stable serialization where applicable), constraints and index metadata, graph
type metadata, catalog metadata, named graph selection, transactions (control
surface + capability reporting), session state commands, catalog procedures and
table-valued functions, backend-native passthrough.

Counts: **supported 69 · rejected 5 · planned 0 · future 0 · total 74.**

## Scoped OUT of the profile: intentional rejections (5)

These are the *only* non-`Supported` manifest entries. They are *required*
rejections in the strict surface (pinned by tests) — correctness guards, not
missing features:

- `reject-create-node-without-explicit-identity` (unless `GenerateForCreate`)
- `reject-merge-without-explicit-identity`
- `reject-unresolved-edge-endpoint-write`
- `reject-non-literal-assignment-value`
- `reject-trailing-node-creation-after-row-producing-edge`

## How the claim is backed

- **Manifest** — every feature has a status + min-profile + summary; `is_supported_in`
  drives profile membership.
- **`full_profile_claim_is_backed`** — pins the scoped-out set (exactly the five
  rejections) to this document.
- **Conformance corpora** — `tests/gql/portable_read.json`, `tests/golden/write_golden.json`
  (byte-identical write plans), `tests/golden/write_corpus.json`, plus the
  `grust-turso` differential read-pushdown oracle.
- **Test floor** — 574 lib + integration in `grust-cypher`, never shrinking; the
  327 strict-write tests remain green.

## Honest scope notes (within `Supported`)

The realized profile claims exactly what the summaries in the manifest say, no
more. Notable deliberate scopings:

- `CALL { … }` subqueries use correlated **import-all** scoping (the subquery
  sees the outer row's bindings); `RETURN *` inside a subquery is a structured
  rejection.
- `shortestPath` / `allShortestPaths` cover a single relationship segment over
  minimal-length **simple** paths.
- Index/graph-type DDL and catalog metadata are portable, caller-owned
  **metadata**; physical/native index creation remains backend-specific and is
  reported through backend capability flags.
- Native passthrough (`NativeQuery`) is an explicit escape hatch **outside**
  portable conformance: no portable semantics are claimed for passthrough text.

## Per-backend conformance

The executing-Cypher set is **Memory** (reference), **Sail** (writes + read
pushdown), **Turso** (writes + differential oracle). Postgres/Postgres-PGQ are
SQL/PGQ backends without a portable Cypher executor; **Falkor** and **Surreal**
are native-graph backends (backend-native Cypher / SurrealQL passthrough via
`run_native_cypher` / `run_native_surrealql`); Helix/Ladybug are internal
(`publish=false`); CocoIndex is a sync target. See `GqlBackend::descriptor()`
and `backend_manifest()` for the honest per-backend capability flags (including
`native_passthrough` languages and the W3 cross-variable correlated update,
which only the Memory reference executes).
