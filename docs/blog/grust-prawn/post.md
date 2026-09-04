# Grust 0.13.0 "Prawn": Bounded Reads, Semantic Graphs, and One Backend Line

Grust is a backend-neutral property-graph library for Rust. Applications build labeled nodes and relationships with typed properties, then use one graph, traversal, schema, and mutation model while Memory, Turso, PostgreSQL, PostgreSQL SQL/PGQ, pgGraph, Sail/Spark, SurrealDB, FalkorDB, LanceDB, or CocoIndex supplies the storage and execution mechanics. Prawn keeps that center small while making its operational edges more honest: resource limits apply during reference-query execution, semantic-model identities survive real-world names and the constructed graph distinguishes parallel relationships, transactional claims line up with actual write boundaries, and every publishable backend returns to one 0.13.0 release line.

The project and detailed documentation are here:

- Repository and API guide: [github.com/querygraph/grust](https://github.com/querygraph/grust)
- Public facade crate: [grust-graph](https://crates.io/crates/grust-graph)
- GQL/Cypher scope: [`docs/GQL_PROFILE_STATEMENT.md`](https://github.com/querygraph/grust/blob/main/docs/GQL_PROFILE_STATEMENT.md)
- Backend integration guide: [`docs/INTEGRATION.md`](https://github.com/querygraph/grust/blob/main/docs/INTEGRATION.md)
- Durable graph benchmark hub: [adversari.al/graph](https://adversari.al/graph)
- Read the Grust book: [FirstPair hosted reader](https://firstpair.org/read/grust/), [PDF](https://firstpair.org/grust/pdf/), or [EPUB](https://firstpair.org/grust/epub/)

## One coherent 0.13 line

The previous Shrimp release was a real but deliberately scoped 0.12.1 registry patch: core, Cypher, Memory, Sail, SQL-core, Turso, and the facade moved, while the other publishable backends remained on 0.12.0. Prawn does not rewrite that history. It moves all 15 publishable Grust packages to 0.13.0 in dependency order, keeps the internal HelixDB and LadybugDB adapters explicitly unpublished, and rebuilds the README, book, release records, and publishing handoff around the same surface.

That lockstep matters to applications using more than the facade. Direct dependencies such as `grust-core`, `grust-cypher`, and a backend crate should all select 0.13.0; mixed 0.12.0/0.12.1 instructions are gone.

## Bounded work, not just a final LIMIT

`ReadQueryPolicy`, `validate_read_query`, and `run_bounded_read_query` are now exposed by the `cypher` facade feature. The parser-backed gate still rejects updating clauses and disallowed query shapes without keyword scanning, but Prawn also accounts for the work and memory that happen before a final row limit: serialized parameter, graph, and output sizes; graph node and edge counts; cumulative candidate scans and expansions; cumulative intermediate bytes; cumulative path hops; result rows; and per-call range allocation. The intermediate budget covers cloned bindings, expression and aggregate results, and DISTINCT/GROUP keys before a final `LIMIT`. Every reference-executor entry point also has a global `MAX_RANGE_ITEMS` ceiling, and checked range arithmetic cannot wrap into runaway execution. Correlated `CALL { ... }` subqueries charge every repeated node/adjacency index build, while catalog procedures charge every graph scan, so per-outer-row rescanning cannot reset the candidate-work or cooperative deadline budget.

The wall-clock limit is cooperative: the reference engine checks it at pipeline, expression, path, and encoding boundaries. It is not an operating-system hard kill. Applications still own authorization, tenant-safe graph projection, outer request deadlines, remote-backend cancellation, and process-level isolation.

## Semantic models become ordinary graphs safely

`SemanticModelProjection` and `semantic_model_graph` turn versioned datasets, fields, metrics, physical sources, and named dataset relationships into the same `Graph` shape used everywhere else. Before construction, Prawn validates trimmed and control-free names, positive representable versions, canonical lowercase SHA-256 artifact and metric-expression identities, relationship endpoints, and uniqueness in every relevant scope.

Stable IDs use length-prefixed components, so punctuation that resembles an internal separator cannot alias another identity. Containment and relationship edges carry explicit stable IDs, and the projection opts into parallel edges, so two differently named relationships between the same dataset pair no longer collapse in the constructed `Graph`. Persisting both remains backend-specific: stores with explicit edge-ID support preserve them, while structurally keyed stores collapse edges sharing a source, label, and destination.

The proof now uses source data rather than a fabricated shape. The packaged fixture is the exact Apache Ossie TPC-DS YAML from upstream commit `ddb19f1b135a61c65603f4823a3526e2fab00cf1`; the test verifies SHA-256 `bafbdc9d0e304ab22a40592f2b6bdfd45cc399c566533cd71343d33380c0d6e1` before parsing and replay-comparing five datasets, 31 fields, five metrics, and four relationships. The published `grust-graph` crate carries Apache Ossie's `NOTICE` and Apache-2.0 text beside the fixture, and the release package gate inspects the archive for all three files.

## Transaction boundaries say what they mean

`GraphMutationAtomicity::Transactional` describes one backend `apply_mutations` call, not every higher-level helper automatically. PostgreSQL and Turso now reject unsupported lowering for a complete non-returning Cypher mutation plan before any write, then execute its operations in source order inside one isolated transaction. A later failure cannot strand earlier mutations.

Both adapters also serialize use of their shared connection across the complete transaction. If cancellation drops a future while `BEGIN`, a statement, or `COMMIT` is in flight, a recovery marker makes the next caller resolve the uncertain state with a rollback before doing new work.

PostgreSQL also makes its public raw `execute` boundary explicit: it is autocommit-only. A lexical guard rejects transaction-control statements in a batch without mistaking words inside strings, quoted identifiers, dollar-quoted bodies, or comments for commands. PostgreSQL PGQ forwards through the same guarded connection path, so native callers cannot bypass cancellation recovery by opening an unmanaged transaction.

The generic write-with-`RETURN` helper deliberately remains sequential because later operations may consume intermediate bindings; it is not a whole-statement atomicity boundary. When a supported multi-statement unit must commit together, the explicit transaction-script surface remains the portable choice.

## Faster local graphs and governed memory

The Memory backend and reference paths gained maintained incoming and outgoing adjacency indexes, shared graph identifiers, indexed conflict and uniqueness checks, and focused constrained-write validation. Endpoint reads, traversal, relationship deletion, and sparse node deletion no longer scan or clone an entire graph merely to touch a small neighborhood. The repository's Criterion suites cover point writes, bulk upserts, fan-out and deep traversal, deletion, constraint checks, index construction, and reference-query scaling.

The private `querygraph-memory` integration continues to demonstrate a domain-neutral governed-cognition substrate over Grust. Relationship facts now use hash-stable, per-record `MemoryRelation` assertion nodes, so two records can preserve independent lineage for the same named relation and endpoints. The hash uses fixed-width length prefixes for architecture-stable IDs, and neighborhood reads can cross a mixture of legacy direct edges and current assertion nodes at successive logical hops. Tombstone preflight discovers a record's assertion nodes and legacy fact edges before a transactional store atomically deletes that discovered set with the record. Discovery is outside the deletion transaction, so callers must synchronize concurrent link and tombstone operations. Turso supplies durable, guarded, idempotent graph commits; TypeSec remains responsible for authority and protected-content release; LakeCat binds source evidence; and an optional Sail executor produces inert bounded proposals. This is infrastructure, not a claim that Grust itself is the Marciana product boundary: authenticated orchestration, service quotas, and the remaining application cutover stay with Marciana and QueryGraph.

## A reproducible benchmark boundary

Prawn adds a Docker-reproducible [LSQB compatibility harness](https://github.com/querygraph/grust/tree/main/benchmarks/lsqb) and keeps its evidence boundary explicit. The unchanged upstream run pins Graph Data Council LSQB commit `242cb2fd31340ca688954cb94794d74c0d5b6f92`, LadybugDB 0.19.0, and a digest-pinned Python 3.12.11 container. Five repetitions over the 28-node/72-edge `sfexample` graph match the nine upstream query-count oracles: 8, 3, 6, 8, 3, 8, 11, 2, and 4—45/45 count checks.

The Grust compatibility adapter is separate from that upstream baseline. It clean-loaded the same fixture for each of five repetitions on Memory, Turso, and PostgreSQL 18.6. The separately labeled adversari.al extension contains 17 attacks: eight exact-count queries probe rewrite, optional-match, range-expansion, Cartesian-product, and union boundaries on every backend, while nine backend-neutral negative cases require stable policy rejections for unbounded paths, excessive range allocation or candidate work, updating-clause smuggling, forbidden procedures, excess UNION arms, intermediate projection amplification, correlated subquery replanning, and correlated catalog rescans. Each storage cell therefore has 17 count oracles; policy is one separate rejection track, not a duplicated backend score. The clean `2680c451` matrix passed all 135 LSQB-derived compatibility observations, all 120 adversarial count observations, and all nine expected policy rejections. This example-scale run is a conformance and reproducibility microbenchmark, not a performance ranking. LSQB is maintained by the Graph Data Council but is not an official LDBC benchmark. These are not LDBC Benchmark Results. Detailed result artifacts and future workloads belong at the durable [adversari.al graph benchmark hub](https://adversari.al/graph).

## Backend truth remains backend-specific

Memory is the deterministic reference. Turso and PostgreSQL execute portable writes and SQL read pushdown; Sail combines portable writes with Spark SQL/Arrow pushdown; PostgreSQL SQL/PGQ and pgGraph layer native graph surfaces over PostgreSQL storage; SurrealDB and FalkorDB expose explicit native-language paths; LanceDB provides Arrow-native graph storage and traversal; and CocoIndex converts between Grust graphs and incremental target state. Those descriptions are capability statements, not a promise that every backend executes every Grust-language feature natively.

The shared SQL dialect contract now carries backend identifier limits as well as syntax. PostgreSQL declares its 63-byte ceiling, so an overlong generated typed-view or property-index name fails as a Grust schema error before the server can silently truncate two names into a collision; dialects without a declared limit are unchanged.

Prawn applies the same fail-before-I/O rule to the dynamic graph adapters without pretending their query languages are identical. FalkorDB validates configurable identity-property names and schema/full-graph label and relationship claims, losslessly quotes supported property names, prevents property maps from replacing the configured structural identity, and omits Redis URLs and credentials from pool/query errors. Helix validates schema names and normalized relationship claims, rejects attempts to overwrite structural node and edge metadata, preflights the complete graph before transporting its chunks, and omits configured URLs from transport failures. SurrealDB losslessly quotes SurrealQL identifiers, rejects normalized table collisions and reserved storage-field overrides, validates configuration/schema/full graph batches before network writes, stores optional Grust edge IDs as `edge_id`, and omits URL userinfo and query secrets from HTTP/WebSocket failures.

Identity framing is hardened below those adapters too. Persistence and export paths that materialize Grust's delimiter-based structural edge key now use `checked_edge_key`, rejecting U+001F in either endpoint, the relationship label, or an explicit edge ID before distinct edges can alias. Mixed explicit/idless comparisons also check the structural owner. Ladybug additionally rejects U+001F in node IDs because its managed metadata index reserves the delimiter. `GraphValue` relationship deduplication instead uses length-framed components, so its payloads need no reserved delimiter. Schema-lowering backends share a namespace-aware claim checker that rejects both normalized collisions and exact duplicate declarations before emitting native objects. Recursive SQL walks hex-encode arbitrary node IDs before placing them in visited sets; a dialect without a delimiter-free encoding hook declines that pushdown and falls back.

The dependency pass is qualified rather than merely “latest.” Redis moves to 1.6.0 with FalkorDB v4.20.4, SurrealDB and its service move to 3.2.4 with reqwest 0.13.4, pgGraph's live service moves to 1.2.0, tokio-postgres moves to 0.7.18, and Turso moves from a prerelease to stable 0.7.2. LanceDB stays at 0.30.0 because the attempted 0.38.0 local-mode build references a remote-feature-only `Error::Http` variant with `remote` disabled. The internal Helix adapter stays at exact `helix-db` 2.0.0 because 3.0.0 removed its dynamic-query APIs and targets `/v2/query`, while the checked Helix v3.0.1 server still exposes `/v1/query`. The [backend qualification record](https://github.com/querygraph/grust/blob/main/benchmarks/lsqb/BACKENDS.md) carries the full matrix and live-gate evidence.

The internal HelixDB and LadybugDB adapters remain `publish = false` and are not `grust-graph` facade features. LadybugDB nevertheless moves to `lbug` 0.20.2 for workspace integration; the unchanged upstream LSQB reference intentionally retains its own pinned Ladybug 0.19.0. The `Full39075` name likewise refers to Grust's widest machine-checked feature catalog, not formal ISO/IEC 39075 certification or uniform backend conformance. The profile statement and integration guide carry the precise boundary.

## Moving to Prawn

Facade users can select the backend features they actually need:

```toml
[dependencies]
grust = { package = "grust-graph", version = "0.13.0", features = ["cypher", "memory", "turso"] }
```

Applications with direct Grust crate dependencies should move the complete set to 0.13.0 together. The full release record is in [`CHANGELOG.md`](https://github.com/querygraph/grust/blob/main/CHANGELOG.md), and the rebuilt book develops the architecture, backend differences, semantic projection, bounded-reference policy, and governed-memory boundary in more depth.
