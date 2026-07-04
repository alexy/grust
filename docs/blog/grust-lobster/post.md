# Grust 0.12 "Lobster": the Full GQL Profile, Realized

Grust is a backend-neutral property-graph library for Rust: labeled nodes and edges with typed properties, a graph builder, a traversal IR, schema metadata, and mutation contracts — one graph model that many storage and execution engines persist, query, index, or synchronize. Applications write graph-shaped Rust once; Memory, Sail/Spark, Turso, PostgreSQL, SurrealDB, FalkorDB, LanceDB, and CocoIndex decide what happens underneath.

On top of that model sits a GQL/Cypher language layer, introduced in 0.11 "Crab" as a standards-conformant pipeline with a precise, machine-checked claim about what it supports. Lobster is the release where that claim reaches its ceiling: **the full ISO/IEC 39075 profile Grust set out to catalog is now realized** — 69 of the 74 features in the machine-readable manifest are implemented and tested, and the remaining 5 are intentional strict-write rejections that guard correctness rather than gaps. A test pins the scoped-out set to the profile statement, so the documentation and the code cannot drift apart.

The project is here:

- Repository: [github.com/querygraph/grust](https://github.com/querygraph/grust)
- Public facade crate: [grust-graph](https://crates.io/crates/grust-graph)
- Language layer: [grust-cypher](https://crates.io/crates/grust-cypher)
- The profile statement: [`docs/GQL_PROFILE_STATEMENT.md`](https://github.com/querygraph/grust/blob/main/docs/GQL_PROFILE_STATEMENT.md)
- The Grust book (in-repo, rebuilt for this release): [`docs/book`](https://github.com/querygraph/grust/tree/main/docs/book)

## What landed

**Query composition grew up.** `CALL { … }` subqueries execute once per incoming row with the outer bindings visible, and their `RETURN` joins back onto the row — returned nodes stay first-class, so a later `MATCH` can extend them. Procedures generalized into table-valued functions: `CALL name(args) [YIELD …]` evaluates its arguments against each incoming row, with `tvf.range` and `tvf.keys` joining the `db.*` catalog procedures.

**Paths and graphs are values.** `shortestPath(…)` and `allShortestPaths(…)` find minimal-length simple paths per endpoint pair over a relationship segment, binding first-class `Value::Path` results. And `Value::Graph` adds set-shaped graph values — deduplicated node/relationship sets built with `graph(nodes, relationships)` — completing the type lattice alongside temporal, duration, decimal, and path values.

**Schema and session surfaces are portable.** `CREATE INDEX` and `CREATE GRAPH TYPE` DDL land as caller-owned metadata with honest per-backend capability flags; catalog snapshots expose graphs, graph types, indexes, and constraints through deterministic read-only procedure tables; `USE <graph>` selection and standalone `USE`/`SET`/`RESET` session commands round out the model.

**Transactions execute atomically.** `CypherTransaction` accumulates eagerly-planned statements between `BEGIN` and `COMMIT` and submits the whole batch in a single store call — atomic on backends whose mutation store is transactional (proven end-to-end on Turso, where the batch runs as one SQL transaction), and *refused with a structured error* on backends that would have to fake it. `ROLLBACK` never touches the store at all.

**The escape hatch is explicit.** For work that deliberately steps outside portable conformance, `NativeQuery` carries backend-native text with per-backend language capability flags: FalkorDB accepts native Cypher, SurrealDB accepts SurrealQL, and the SQL backends accept their own dialects — with structured non-support everywhere else. FalkorDB and SurrealDB join the backend conformance catalog as native-graph backends with honest flags.

**The conformance corpus runs.** Every case in the portable read corpus now executes against a reference graph and must match its expected outcome — including the structured error kind for rejected cases. The corpus is living documentation that fails the build when it lies.

## The honesty story

Lobster does not claim to be a fully conformant GQL database. It claims something more useful: a *precise* profile — every supported feature enumerated with its scope, every deviation deliberate and documented, and the whole claim backed by a test against the feature manifest rather than by prose. Where a supported feature is narrower than the standard (import-all subquery scoping, single-segment shortest paths, metadata-only DDL), the profile statement says so in as many words.

## Breaking changes

Within `0.x`, `0.12.0` widens public enums and structs: `Value` gains the `Graph` variant, `GqlBackend` gains `Falkor` and `Surreal`, and `GqlBackendDescriptor` and `CallClause` gain fields. Exhaustive matchers downstream will need new arms; everything else is additive.

## What's next

The new read features execute on the Memory reference today, with backends falling back to it safely. The planned follow-on goal lowers them into backend SQL pushdown — differential-oracle-backed, SQLite-first — when a workload shows the reference path is a bottleneck. The full changelog lives in [`CHANGELOG.md`](https://github.com/querygraph/grust/blob/main/CHANGELOG.md), and the book's GQL chapter has the narrative version of everything above.
