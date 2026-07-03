# Grust GQL/Cypher — Profile Statement (Unit 16)

This is the precise, backed statement of what Grust's GQL/Cypher layer conforms to
today. It is the terminal deliverable of the GQL completion goal (U16): the
`Full39075` claim is **candidate** and is backed feature-by-feature against the
machine-readable manifest (`grust_cypher::gql::feature_manifest`), not by prose.

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
   transaction surface, and the U10b write widenings.
3. **Full39075** — the candidate full ISO/IEC 39075 profile. Equals PortableGql
   **plus** the items still marked `Future`/`Planned` below once they land. The
   realized profile today is PortableGql; `Full39075` is a candidate with an
   explicit, enumerated remainder.

## Realized profile (today)

**67 of 74 catalogued features are `Supported`** (implemented + tested). By family
(run `support_summary()` for the live list): parser/semantics, resolved writes,
broad matched writes, row-producing relationship writes, returning & aggregates,
predicates & expressions, read-only matching, path matching, query composition,
type system (temporal/duration/decimal/path/graph incl. arithmetic, ordering, and
stable serialization where applicable), constraints and index metadata, graph
type metadata, catalog metadata, named graph selection, transactions (control
surface + capability reporting), session state commands, catalog procedures.

Counts: **supported 67 · rejected 5 · planned 1 · future 1 · total 74.**

## Scoped OUT of the realized profile (with rationale)

These are the only gaps between the realized profile and a full-39075 claim. Each
is deliberate and enumerated so the claim is never silently unbacked.

### Future (1) — deferred to a later full-39075 pass

| Feature | Rationale |
|---|---|
| `shortest-path` (U9) | Shortest-path families need a dedicated traversal/cost model; the bounded var-length read path is supported, shortest-path is not. |

### Planned (1) — near-term, partially scaffolded

| Feature | Rationale |
|---|---|
| `native-cypher-passthrough` (U14) | Backend-native Cypher/SurrealQL/Falkor passthrough, intentionally separate from portable conformance. |

## Intentional rejections (5) — conformance guards, not gaps

These are *required* rejections in the strict surface (pinned by tests); they are
correctness guards, not missing features:

- `reject-create-node-without-explicit-identity` (unless `GenerateForCreate`)
- `reject-merge-without-explicit-identity`
- `reject-unresolved-edge-endpoint-write`
- `reject-non-literal-assignment-value`
- `reject-trailing-node-creation-after-row-producing-edge`

## How the claim is backed

- **Manifest** — every feature has a status + min-profile + summary; `is_supported_in`
  drives profile membership.
- **`full_profile_claim_is_backed`** — pins the scoped-out set to this document.
- **Conformance corpora** — `tests/gql/portable_read.json`, `tests/golden/write_golden.json`
  (byte-identical write plans), `tests/golden/write_corpus.json`, plus the
  `grust-turso` differential read-pushdown oracle.
- **Test floor** — 524 lib + integration in `grust-cypher`, never shrinking; the
  327 strict-write tests remain green.

## Per-backend conformance

The executing-Cypher set is **Memory** (reference), **Sail** (writes + read
pushdown), **Turso** (writes + differential oracle). Postgres/Postgres-PGQ are
SQL/PGQ backends without a portable Cypher executor; Helix/Ladybug are internal
(`publish=false`); CocoIndex is a sync target. See `GqlBackend::descriptor()` and
`backend_manifest()` for the honest per-backend capability flags (including the
W3 cross-variable correlated update, which only the Memory reference executes).
