# Grust 0.11.0 "Crab": A Portable GQL/Cypher Layer for Property Graphs

Grust is a backend-neutral property-graph library for Rust: one graph model —
labeled nodes and edges, typed properties, stable IDs — over many storage and
execution engines (in-memory, Sail/Spark, PostgreSQL and SQL/PGQ, Turso,
SurrealDB, FalkorDB, LanceDB, CocoIndex, …).

**Crab** is the first *named* Grust release, and it is a big one: it adds a
real, standards-conformant **GQL/Cypher language layer** on top of the property
graph, plus first-class temporal/decimal values, catalog procedures, a
transaction surface, write-path widening, and MVCC concurrent writes for Turso.

- Repository: [github.com/querygraph/grust](https://github.com/querygraph/grust)
- Facade crate: [grust-graph](https://crates.io/crates/grust-graph) · core: [grust-core](https://crates.io/crates/grust-core) · language: [grust-cypher](https://crates.io/crates/grust-cypher)
- The Grust book: [`docs/book`](https://github.com/querygraph/grust/tree/main/docs/book)

## The headline: a standards-conformant Cypher → GQL pipeline

`grust-cypher` is a clean, additive language layer built as a real pipeline —
span-bearing **lexer → recursive-descent parser → AST → semantic analysis** —
with a structured conformance spine (`GqlFeature` taxonomy, `GqlError`) rather
than ad-hoc string parsing. Highlights:

- **Portable read core** — a Memory *reference executor* runs `MATCH` /
  `OPTIONAL MATCH` / variable-length paths, a three-valued `WHERE` engine,
  `RETURN` with aliases / `DISTINCT` / `ORDER BY` / `SKIP` / `LIMIT`, aggregates
  with implicit `GROUP BY`, `WITH`, `UNWIND`, and `UNION`.
- **Backend read pushdown** — the bounded read subset lowers into backend SQL
  (Spark and SQLite dialects); the `RETURN` projection runs through the shared
  reference, so pushed results are **byte-identical by construction**. An
  embedded-SQLite **differential oracle** verifies reference-vs-pushdown row
  equality.
- **Write-path cutover** — the writable-Cypher entry points now route acceptance
  through the standards-conformant parser, narrowing the public surface to
  standard GQL/Cypher while keeping mutation plans byte-identical (golden-guarded).

## First-class values: decimal, duration, temporal

`grust_core::Value` gains lossless **`Decimal`** (SQL `DECIMAL(38, s)`-style) and
ISO 8601 **`Duration`** types, with parsing, canonical display, ordering, and
checked arithmetic — wired through every backend. Temporal values now order
chronologically. In a query: `decimal('0.1') + decimal('0.2')` is exactly `0.3`,
and `MATCH (a)-[:R]->(b) SET a.x = b.y + 1` correlates across variables.

## More language surface

- **Catalog procedures**: `CALL db.labels()`, `db.relationshipTypes()`,
  `db.propertyKeys()` with `YIELD`.
- **Transactions**: a `START TRANSACTION` / `BEGIN` / `COMMIT` / `ROLLBACK`
  command surface plus honest per-backend atomicity capability reporting.
- **Write widening**: multiple relationship patterns per statement, incoming
  `<-[:T]-` edge writes, and cross-variable correlated `SET`.

## Turso MVCC and concurrent writes

`grust-turso` gains a `journal_mode` option. Selecting `Mvcc` enables Turso's
multi-version concurrency control (`PRAGMA journal_mode = mvcc`), and data writes
run inside `BEGIN CONCURRENT` transactions with bounded conflict retry — so
concurrent writers make progress. WAL behavior is unchanged.

## An honest profile

Crab ships a **backed conformance profile statement**
([`docs/GQL_PROFILE_STATEMENT.md`](https://github.com/querygraph/grust/blob/main/docs/GQL_PROFILE_STATEMENT.md)):
58 of 74 catalogued features are `Supported`, and every not-yet-supported feature
is enumerated with a rationale — the candidate `Full39075` claim is never silently
unbacked. A test pins the scoped-out set to the manifest so docs and code can't drift.

## More

The plan and progress live in the repo:
[`GQL_GOAL.md`](https://github.com/querygraph/grust/blob/main/docs/GQL_GOAL.md),
[`GQL_M1_CHECKPOINT.md`](https://github.com/querygraph/grust/blob/main/docs/GQL_M1_CHECKPOINT.md),
and the profile statement above. For the full architecture — the property graph
model, traversal IR, store contract, and every backend — see the Grust book.

Build your graph once, keep the domain model in Rust, and let the backend (and
now the query language) translate.
