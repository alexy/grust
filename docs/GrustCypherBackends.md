# Grust Cypher Backend Portability Plan

Status: planning document.

This document describes how to implement as much of Grust Cypher across Grust
backends as is reasonable, while preserving backend-specific truth. The goal is
not uniform marketing. The goal is a portable language core, backend-conformant
execution, and a clear report of what each backend can parse, plan, execute,
push down, enforce, or reject.

## Current Baseline

`grust-cypher` owns parser, planner, DDL registry, restricted returning
evaluation, and generic returning execution. `MemoryGraphStore` and
`SailGraphStore` implement `CypherMutationExecutor` for the implemented strict
Cypher surface.

Current backend reality:

- **Memory** is the deterministic reference executor.
- **Sail** is the strongest real backend execution target for the current
  writable-Cypher surface.
- **Other backends** can consume ordinary Grust graph mutations, reads, and
  traversal APIs, but they do not yet expose direct text-Cypher execution or
  complete Cypher conformance manifests.

## Portability Principles

1. **One language frontend.** All backends use `grust-cypher` for parsing,
   semantic analysis, logical planning, and structured errors.
2. **Reference first.** Memory executes every portable feature before the
   feature is declared generally supported.
3. **Backend capabilities are explicit.** A backend may support a feature by
   native pushdown, hybrid execution, portable fallback, or rejection.
4. **No silent weakening.** If a backend cannot enforce uniqueness,
   transactionality, ordering, or path semantics, the plan must expose that
   difference.
5. **Conformance is data.** Backend support should live in manifests and test
   output, not prose alone.

## Capability Model

Add a backend capability model in `grust-cypher` or `grust-core`:

```rust
pub enum CypherBackendSupport {
    Native,
    Hybrid,
    PortableFallback,
    PlannedOnly,
    Unsupported,
}

pub struct CypherBackendCapability {
    pub feature: GqlFeature,
    pub support: CypherBackendSupport,
    pub atomicity: CypherAtomicity,
    pub preserves_order: bool,
    pub exact_counters: bool,
    pub native_constraints: bool,
    pub notes: &'static str,
}
```

Feature families:

- parser and semantic analysis;
- resolved node/edge writes;
- broad matched node/edge writes;
- row-producing relationship writes;
- returning rows and aggregates;
- predicates and expression families;
- read-only matching;
- path matching;
- constraints and indexes;
- transactions;
- catalog/session state;
- native query passthrough where applicable.

## Conformance Harness

Create a reusable test crate or module:

```text
crates/grust-cypher-conformance/
  src/
    manifest.rs
    fixtures.rs
    runner.rs
    assertions.rs
  tests/
    memory.rs
    sail.rs
    backend_matrix.rs
```

The harness should accept a backend adapter:

```rust
pub trait CypherConformanceStore:
    GraphStore + GraphMutationStore + CypherMutationExecutor
{
    fn cypher_capabilities(&self) -> Vec<CypherBackendCapability>;
    async fn reset_fixture(&self, fixture: &Graph) -> Result<()>;
}
```

Test categories:

- parse-only;
- plan-only;
- execute mutation;
- execute returning mutation;
- execute read query;
- validate schema/constraint behavior;
- verify counters and generated identities;
- verify failure atomicity;
- verify backend-native pushdown where promised.

Each test case should declare:

```text
feature: gql.match.basic
requires: [nodes, edges, predicates.equal]
expected_support:
  memory: native-reference
  sail: hybrid
  lancedb: planned-only
```

## Backend Matrix

| Backend | Role | Near-term Cypher target |
| --- | --- | --- |
| Memory | Reference executor | Full portable Grust Cypher profile |
| Sail | Distributed tabular execution | Strong writable support, bounded reads, SQL pushdown |
| pgGraph | PostgreSQL graph index/runtime | Native traversal/read pushdown, SQL-backed writes |
| LadybugDB | Embedded native graph DB | Native read/path pushdown, schema-aware writes |
| LanceDB | Embedded Arrow/vector table store | Universal-table writes, predicate reads, vector extensions |
| SurrealDB | Service/native graph-ish DB | Native node/edge writes and simple reads/traversal |
| FalkorDB | RedisGraph/Cypher backend | Native Cypher passthrough plus Grust plan adapters |
| HelixDB | Service graph backend | Native write/read adapters where API supports them |
| CocoIndex | Sync/export adapter | Not a query backend; consume graph target-state plans |

## Memory Plan

Memory is the reference backend. It should be correct before it is fast.

Deliverables:

- Execute every portable Cypher logical plan.
- Maintain exact result rows, counters, generated identities, and error
  semantics.
- Enforce native required/unique constraints for node and edge properties.
- Add reference read-only `MATCH ... RETURN`.
- Add reference graph pattern matcher, including path variables and path modes.
- Provide deterministic ordering where the language requires it and documented
  stable ordering where Grust chooses it.

Reasonable limits:

- No query optimizer beyond simple deterministic planning.
- No parallelism requirement.
- No native index promises unless explicit in-memory indexes are added.

Conformance target:

- Full portable profile.
- Full parser/semantic/plan coverage.
- Full generic execution coverage.

## Sail Plan

Sail is the main real execution target for large tabular graphs.

Deliverables:

- Keep current writable Cypher helpers and compatibility wrappers.
- Route all text parsing and planning through `grust-cypher`.
- Push down bounded node/edge predicates, relationship matching, ordering,
  skip/limit, and aggregations into Spark SQL when semantics align.
- Use hybrid execution for result shapes that are portable but awkward in SQL.
- Keep Arrow IPC staging for row-producing writes.
- Add bounded read-only `MATCH ... RETURN` over generic `grust_nodes` and
  `grust_edges`, then typed table pushdown when `GraphSchema` makes it safe.
- Persist Cypher constraint registry metadata, and continue read-before-write
  validation until native Delta/Spark constraints are explicit.

Reasonable limits:

- Do not claim backend-native uniqueness until Sail can enforce it
  transactionally or with documented Delta constraints.
- Do not claim atomic multi-statement execution until Spark/Sail transaction
  semantics are implemented and tested.
- Avoid full path semantics until the shared row model and bounded path planner
  can express the results cleanly.

Conformance target:

- Strong writable profile.
- Strong bounded read profile.
- Hybrid returning profile.
- Backend-native constraints only when actually enforced.

## pgGraph Plan

pgGraph is the natural home for graph traversal pushdown over PostgreSQL.

Deliverables:

- Add a `CypherMutationExecutor` implementation if pgGraph can apply all
  planned mutations through its table model or through PostgreSQL writes.
- Implement read-only `MATCH ... RETURN` by lowering suitable graph patterns to
  pgGraph functions or SQL over registered graph tables.
- Push down bounded traversal, reachability, and shortest-path families where
  pgGraph supports them.
- Use PostgreSQL constraints and indexes for native enforcement where the
  universal-table layout can express them.
- Hydrate result rows back into Grust node/edge/path values.

Reasonable limits:

- If pgGraph remains alpha or read-oriented, keep writes through ordinary
  PostgreSQL table mutations and treat graph runtime rebuild/sync explicitly.
- Do not push down expressions that depend on Grust `Value` semantics unless
  the SQL translation is exact.

Conformance target:

- Strong read/path profile.
- Moderate write profile through table mutations.
- Strong native constraint/index profile where PostgreSQL enforces it.

## LadybugDB Plan

LadybugDB is an embedded native graph database and should do more native graph
work than the table stores.

Deliverables:

- Lower read-only graph patterns to Ladybug Cypher where the native semantics
  match Grust.
- Use Grust planning for writes, then emit Ladybug node/relationship table
  operations.
- Add schema-aware table creation from `GraphSchema` and GQL graph types.
- Support Arrow IPC read/write boundaries already exposed by the backend.
- Hydrate native query rows into Grust result rows.

Reasonable limits:

- Dynamic/schema-free writes should remain conservative because Ladybug's table
  model is schema-oriented.
- Native Cypher passthrough should be clearly marked backend-specific, not
  portable Grust Cypher.

Conformance target:

- Strong native graph read profile.
- Good schema-backed write profile.
- Portable fallback only where Ladybug table metadata is sufficient.

## LanceDB Plan

LanceDB is an Arrow/vector table store, not a graph database.

Deliverables:

- Execute resolved writes and broad table-scanned writes over universal node
  and edge tables.
- Push down equality, membership, range, string predicates, order, limit, and
  vector-adjacent filters where LanceDB supports exact semantics.
- Add bounded read-only node/edge `MATCH ... RETURN` through table scans and
  joins.
- Keep vector search as an extension function or procedure family, not core
  GQL unless mapped explicitly.

Reasonable limits:

- Avoid complex path matching except bounded small-depth joins or portable
  materialized traversal.
- Do not emulate graph-native path algorithms if performance would be
  surprising; require explicit opt-in for portable fallback.

Conformance target:

- Good node/edge table query profile.
- Good resolved write profile.
- Limited path profile.
- Strong extension story for vector search.

## SurrealDB Plan

SurrealDB has native graph-like records and traversal syntax, but its semantics
are not identical to GQL.

Deliverables:

- Add direct text helper only through `grust-cypher`, not SurrealQL passthrough.
- Lower resolved node/edge writes to existing Surreal write paths.
- Lower simple read patterns and traversals where record-link semantics align.
- Use backend-native constraints only where Surreal can enforce the exact
  Grust/GQL property constraint.
- Hydrate Surreal records into Grust values with explicit type conversion.

Reasonable limits:

- Avoid claiming full GQL path semantics through Surreal traversal syntax until
  equivalence tests prove it.
- Keep backend-native SurrealQL escape hatches separate from portable Cypher.

Conformance target:

- Good resolved write profile.
- Moderate read/traversal profile.
- Conservative constraint profile.

## FalkorDB Plan

FalkorDB already speaks a Cypher-like language through Redis Graph commands.
That makes passthrough tempting and dangerous.

Deliverables:

- Continue using `grust-cypher` for portable Grust Cypher.
- Add an optional native passthrough API explicitly named as Falkor-native.
- Lower portable read patterns to Falkor Cypher only where result semantics,
  property typing, null behavior, and path behavior are proven equivalent.
- Lower writes either through Grust mutation plans or carefully constrained
  native Cypher.
- Add result hydration from Falkor values to Grust `Value` and row bindings.

Reasonable limits:

- Do not treat Falkor's accepted Cypher surface as Grust's GQL conformance.
- Be strict around nulls, missing properties, labels, path values, and
  duplicate edge identities.

Conformance target:

- Potentially strong native read profile.
- Moderate write profile.
- Native passthrough profile separate from portable profile.

## HelixDB Plan

HelixDB support should follow the backend's real API surface.

Deliverables:

- Lower resolved Grust Cypher writes through existing Helix write methods.
- Add simple read/traversal pattern support where Helix APIs expose exact
  primitives.
- Preserve all Grust property values if the backend supports them; otherwise
  reject or document conversions clearly.
- Add conformance tests for serialization, relationship identity, and traversal
  direction.

Reasonable limits:

- Do not widen Cypher support by silently dropping non-string or unsupported
  property values.
- Keep path and expression support conservative until backend APIs can support
  exact behavior.

Conformance target:

- Resolved write profile.
- Simple traversal profile.
- Conservative value/profile reporting.

## CocoIndex Plan

CocoIndex is not a `GraphStore` query backend today. It is a sync/export
integration.

Deliverables:

- Allow Cypher/GQL plans that produce target graph state to export as
  CocoIndex node/relationship target state.
- Treat CocoIndex flows as producers or consumers of graph deltas, not as a
  query execution engine.
- Keep custom target integration separate from Cypher conformance.

Reasonable limits:

- No direct `MATCH ... RETURN` execution target unless CocoIndex exposes a
  stable embedded Rust query API.
- No claims of GQL backend conformance.

Conformance target:

- Export/profile conformance, not query conformance.

## Implementation Phases

### Phase 1: Capability Manifest

- Add static backend capability reports.
- Add generated Markdown support matrix.
- Wire `grust-graph` facade docs to expose backend support honestly.

### Phase 2: Conformance Runner

- Extract Memory-backed Cypher tests into reusable cases.
- Add backend expected-support manifests.
- Keep ignored/live backend tests as opt-in but visible in reports.

### Phase 3: Direct Text Helpers

- Add backend-neutral helpers for any store implementing
  `CypherMutationExecutor`.
- Keep Sail compatibility helpers.
- Add optional helpers for backends that can execute plans but should not parse
  text themselves.

### Phase 4: Portable Read Core

- Implement bounded read-only `MATCH ... RETURN` in Memory.
- Add Sail lowering.
- Add table-store lowering for LanceDB and pgGraph where straightforward.

### Phase 5: Constraint and Index Pushdown

- Expand `GraphNativeConstraintCapability`.
- Add backend-native unique/required/index support where exact.
- Keep read-before-write validation visible as hybrid, not native.

### Phase 6: Path Profiles

- Memory reference path matcher.
- Sail bounded SQL join path profile.
- pgGraph and Ladybug native path pushdown.
- Conservative table-store fallbacks.

### Phase 7: Backend Release Profiles

Publish a generated support matrix for each release:

```text
Feature family                  Memory  Sail  pgGraph  Ladybug  LanceDB  Surreal  Falkor  Helix
resolved writes                 full    full  partial  partial  partial  partial  partial partial
broad matched writes            full    full  planned  planned  planned  planned  planned planned
returning rows                  full    full  planned  planned  planned  planned  planned planned
bounded read MATCH              next    next  next     next     next     next     next    next
native unique constraints       full    no    next     next     next     maybe    maybe   maybe
path matching                   next    next  native   native   limited  limited  native  limited
```

The real matrix should be generated from code, not maintained by hand.

## Definition of Done

A backend feature is done only when:

- the capability manifest declares it;
- conformance tests cover success and failure behavior;
- docs explain whether it is native, hybrid, portable fallback, planned-only,
  or unsupported;
- exact counter, identity, ordering, null/missing, and atomicity behavior is
  tested;
- `git diff --check`, focused crate tests, and facade feature checks pass.

## Release Workflow

For any backend Cypher expansion:

1. Update the backend capability manifest.
2. Add or update conformance cases.
3. Add backend-specific tests only for lowering, hydration, live integration,
   or native capability behavior.
4. Update `docs/CypherWrite.md` or the successor GQL plan if public semantics
   change.
5. Update README/book/blog prose if the public surface changes.
6. Run focused tests plus the relevant backend feature check.
7. Rebuild the book if release-facing docs changed.

## Non-Goals

- Do not make every backend expose native text Cypher.
- Do not claim full GQL conformance for a backend that only accepts a related
  native Cypher dialect.
- Do not hide differences in transactionality, native constraints, path
  semantics, or value typing.
- Do not require vector/table/sync backends to become graph-native databases.
