# Changelog

All notable Grust changes are recorded here by date and release. This project
started before the changelog existed, so entries before 2026-06-12 were
reconstructed from Git history, release commits, and the shipped docs.

## Unreleased

## 0.13.0 "Prawn" - 2026-09-04

Prawn returns every publishable Grust crate to one lockstep release line. It
includes the complete surface that was only partially shipped in the scoped
0.12.1 Shrimp registry patch, plus the later safety, semantic-model,
performance, backend, documentation, and benchmark work recorded below.

### Release and portability

- Moved all 15 publishable Grust crates and their intra-workspace dependency
  requirements to 0.13.0; keep HelixDB, LadybugDB, QueryGraph Memory, and
  examples explicitly unpublished while testing them in the workspace.
- Removed the repository-wide absolute Apple SDK include path that mixed Xcode
  and Command Line Tools headers. Native C++ troubleshooting now derives any
  required fallback from the active `xcrun` SDK in the invoking shell.
- Reconciled the Shrimp patch history, current release marker, README and book
  examples, completed goal records, GQL claim scope, and centralized FirstPair
  book/TextPack handoff for the named Prawn release.
- Included Apache Ossie's `NOTICE` and Apache-2.0 text beside the exact upstream
  TPC-DS fixture in the published facade archive. The release gate
  `scripts/verify-package-attribution.sh` inspects the actual `.crate` archive
  so a future package cannot silently drop the fixture or its attribution.
- Qualified the backend dependency line rather than mechanically selecting the
  highest version: Redis advances to 1.6.0 with FalkorDB v4.20.4, SurrealDB and
  its Docker service advance to 3.2.4 with reqwest 0.13.4, pgGraph's live image
  advances to 1.2.0, tokio-postgres advances to 0.7.18, and Turso advances from
  a prerelease to stable 0.7.2. The applicable focused and live integration
  gates pass.
- Deliberately held LanceDB at 0.30.0 after the 0.38.0 default-feature,
  local-mode build failed inside upstream `lancedb`: `job.rs` references the
  remote-only `Error::Http` variant when `remote` is disabled. Held the
  unpublished Helix adapter at exact `helix-db` 2.0.0 after the 3.0.0 probe
  removed `DynamicQueryRequest`/`dynamic_query`, changed `Client::query`, and
  targeted `/v2/query` while the checked Helix v3.0.1 server still exposed
  `/v1/query`.

### Safety and semantic projection

- Added a backend-neutral semantic graph taxonomy for versioned models,
  datasets, fields, relationships, and metrics. Projection now validates
  SHA-256 identities, names, references, and uniqueness, uses collision-safe
  identity components, and preserves separately named parallel relationships
  in the constructed `Graph` with explicit edge IDs. Persisted multi-edge
  preservation remains a backend capability; structurally keyed stores can
  collapse same-endpoint, same-label edges.
- Replaced the synthetic Ossie proof with the exact Apache Ossie TPC-DS YAML
  from upstream commit `ddb19f1b135a61c65603f4823a3526e2fab00cf1`. The
  packaged fixture must match SHA-256
  `bafbdc9d0e304ab22a40592f2b6bdfd45cc399c566533cd71343d33380c0d6e1`
  before parsing and replay-stably projecting five datasets, 31 fields, five
  metrics, and four relationships.

- Added a parser-backed bounded read policy for applications that expose a
  deliberately small in-memory GQL/Cypher surface. It rejects updating clauses,
  graph selection, procedures, unbounded paths, and non-literal limits from the
  typed AST, then enforces parameter/graph/output byte ceilings, graph counts,
  cumulative candidate work, intermediate bytes, and path hops, result rows,
  per-range allocation, and a cooperative wall-clock timeout. The cumulative
  byte budget includes cloned bindings, expression and aggregate results, and
  DISTINCT/GROUP serialization, preventing pre-`LIMIT` projection amplification.
  All reference reads retain a global
  hard ceiling on scalar and table-valued `range()` allocation. Correlated
  `CALL { ... }` index construction and catalog-procedure graph scans are now
  charged on every invocation, closing a per-outer-row rescan bypass against
  both the candidate-work limit and cooperative deadline. Authorization, remote
  deadlines, and process isolation remain host responsibilities.

- Made non-returning PostgreSQL and Turso
  `execute_cypher_mutation_plan` calls atomic on their transactional stores.
  Each executor rejects unsupported lowering before writing, then preserves
  source order inside one isolated transaction. Their shared connections are
  serialized for the complete transaction, and the next caller rolls back an
  abandoned transaction if cancellation drops a future during `BEGIN`, a
  statement, or `COMMIT`. The generic
  write-with-`RETURN` helper remains sequential because later operations may
  consume intermediate bindings; it is not a whole-statement atomicity
  boundary. Explicit transaction scripts continue to batch supported
  mutations.
- Made PostgreSQL's public raw `execute` surface explicitly autocommit-only.
  Its lexical guard rejects transaction-control statements in multi-statement
  SQL while ignoring lookalike words in strings, quoted identifiers,
  dollar-quoted bodies, and comments; PostgreSQL PGQ inherits the same guard.
  Transaction helpers set their recovery marker before `BEGIN`, and every
  serialized connection user first rolls back an in-flight transaction left
  uncertain by cancellation.

### Backend correctness and graph performance

- Hardened the dynamic graph backends before query construction or transport.
  FalkorDB validates configurable identity-property names, schema and
  complete-graph label/relationship claims after lossy normalization, and
  losslessly quoted property names, prevents a property map from overwriting
  the configured structural ID, and no longer includes Redis URLs or
  credentials in pool/query errors. Helix rejects unsafe
  schema names, normalized relationship collisions, and attempts to overwrite
  node/edge structural metadata, preflights complete graph batches, and omits
  configured URLs from transport errors. SurrealDB uses lossless SurrealQL
  identifier quoting, rejects ambiguous normalized record-table mappings and
  reserved structural-field writes, validates schema/configuration and every
  batch before I/O, preserves optional edge IDs in `edge_id`, and redacts URLs
  from HTTP/WebSocket failures.
- Added shared physical-schema claim validation for backends whose logical
  names are lowered into native object namespaces. FalkorDB, Helix, LadybugDB,
  LanceDB, and Sail now reject both lossy-name collisions and exact duplicate
  declarations before emitting schema or write operations.
- Added `validate_edge_key_components` and `checked_edge_key` for every
  persistence/export path that materializes Grust's compatibility edge key.
  CocoIndex, Cypher capture/refetch, LadybugDB, LanceDB, and Sail now reject
  U+001F in source IDs, labels, target IDs, or explicit edge IDs before two
  distinct edges can alias. Mixed explicit/idless equality additionally checks
  the structural owner. LadybugDB also rejects U+001F in node IDs before its
  managed metadata index can confuse a user record with a table marker.
  Separately, `GraphValue` relationship deduplication now
  uses length-framed identity components, so its payloads require no reserved
  delimiter and crafted explicit/structural identities remain distinct.
- Made recursive SQL walk pushdown safe for arbitrary `NodeId` text. PostgreSQL,
  Spark SQL, and the generic SQLite dialect hex-encode IDs into delimiter-free
  visited-set tokens before framing them; a dialect without both recursive-CTE
  support and such an encoder declines variable-length and shortest-walk
  pushdown instead of relying on a delimiter that may occur in an ID.

- Added a Docker-reproducible LSQB compatibility and adversarial-query
  harness under `benchmarks/lsqb`. The unchanged upstream baseline pins Graph
  Data Council LSQB commit `242cb2fd31340ca688954cb94794d74c0d5b6f92`,
  LadybugDB 0.19.0, and a digest-pinned Python 3.12.11 container; five
  repetitions over `sfexample` match all nine expected result counts in 45/45
  executions. A separately labeled Grust adapter is configured to run the same
  28-node/72-edge fixture plus eight adversari.al count attacks across Memory,
  Turso, and PostgreSQL 18.6 with a clean load per repetition. Nine additional,
  backend-neutral attacks require stable policy rejections for unbounded paths,
  range allocation,
  candidate work, updating-clause smuggling, forbidden procedures, and excess
  UNION arms, plus cumulative intermediate projection, correlated subquery
  replanning, and correlated catalog rescans. Those eight count attacks plus
  nine policy attacks are 17 separate
  adversari.al attacks; each storage-backend cell has 17 count oracles (nine
  LSQB-derived plus eight adversarial), while policy is one backend-neutral
  rejection track. This is a reproducibility and compatibility microbenchmark,
  not a performance ranking: LSQB is GDC-maintained but not an official LDBC
  benchmark. The tracked evidence currently covers the unchanged upstream
  45/45 run; the Grust matrix remains a release gate until its result artifacts
  are captured.
  These are not LDBC Benchmark Results.
- Removed the Rust 1.96 strict-Clippy debt from the Cypher and Sail execution
  paths without adding lint suppressions. Writable `RETURN` parsing and
  evaluation now use named scope, aggregate, and cache contexts; Sail mutation
  execution uses typed capture/output objects and operation views instead of
  parallel optional tuples and long argument lists. Shared schema SQL now takes
  an explicit table layout, and the remaining first-party workspace findings
  in PostgreSQL control flow and test/benchmark code are resolved.
- Fixed typed Sail schemas that explicitly declare the node `id` property.
  The property now reuses the one structural identity column throughout table
  descriptors, Delta DDL, and staged merges; incompatible non-string identity
  declarations fail before SQL reaches Sail.
- Added the defaulted `GraphSqlDialect::max_identifier_bytes` hook. PostgreSQL
  reports its 63-byte ceiling, so typed node/edge views and property indexes at
  that limit remain valid while longer generated names fail with
  `GrustError::Schema` before PostgreSQL can silently truncate or collide them.
  Dialects without a declared limit retain their previous behavior.
- Updated the internal, unpublished LadybugDB adapter from `lbug` 0.17.1 to
  0.20.2 while retaining its Arrow 55 IPC boundary. The unchanged upstream
  LSQB reference run deliberately keeps Ladybug 0.19.0 because that is the
  version selected by the pinned upstream scripts.
- Verified the QueryGraph stack against the exact optimized graph-enabled Sail
  `c5309365` artifact: 26 adapter tests, the dedicated non-null and temp-view
  gates, and both governed cognition parity/secrecy tests pass. The same
  artifact also accepts Typesec's 5-node, 4-edge typed company schema that
  exposed the duplicate-identity-column defect.
- Added statistically sampled Criterion coverage for graph index construction,
  edge-key materialization, unique-property validation, and in-memory graph
  point writes, filtered reads, traversal, and bulk upserts at realistic graph
  sizes. Coverage now also isolates constrained edge writes, deep and
  high-fanout traversal, and adjacency-sensitive node and edge deletion.
- Replaced quadratic graph property-uniqueness scans with hash-bucketed,
  collision-checked value tracking that preserves floating-point and JSON
  equality semantics.
- Removed full-graph cloning and validation staging from schemaless in-memory
  point writes and bulk upserts; constrained stores retain the same pre-write
  validation path.
- Added maintained incoming and outgoing adjacency indexes to the in-memory
  backend. Endpoint-filtered reads and traversals now inspect only incident
  edges, while all mutation paths update the indexes through shared helpers.
- In-memory node, endpoint-edge, and Cypher relationship deletion now resolve
  incident keys through those adjacency indexes instead of scanning the full
  edge map. At 10,000 edges, deleting a node with two incident edges improves
  from about 42.1 microseconds to 100 nanoseconds, and deleting one matched
  edge from 52.1 microseconds to 63 nanoseconds.
- Replaced constrained point-write graph snapshots with focused node, edge,
  uniqueness, native-constraint, and incident-endpoint validation. Bulk writes
  still validate one complete staged graph so cross-item constraints remain
  atomic and fail before mutation.

### Integration reliability

- Bounded each Sail integration command with a configurable timeout. A stalled
  live phase now fails the harness instead of leaving the release gate running
  indefinitely.

- Replaced `querygraph-memory`'s TypeSec sibling paths with the exact reviewed
  public Git revision, so the governed cognition crate can build and test from
  a clean Grust checkout.

- Recorded a passing focused live Sail integration gate for the governed
  cognition substrate: 26 Sail adapter tests, two live backend checks, and two
  cognition reference-parity and evidence-secrecy cases. The documentation
  keeps this explicitly local-source verification separate from Marciana's
  remote-reachable Sail compatibility baseline.

### Governed memory, FirstPair, and query execution

- Changed `querygraph-memory` relationship facts from one structurally
  deduplicated direct edge to a hash-stable, per-record `MemoryRelation`
  assertion node between the two entities. Distinct records can now retain
  independent lineage for the same endpoints and relationship name. Assertion
  IDs hash fixed-width length-prefixed components and remain identical across
  32- and 64-bit hosts. Neighborhood reads can cross any mixture of legacy
  direct `RELATES` edges and current assertion-node relationships at
  successive logical hops, while tombstone preflight discovers the record's
  assertion nodes and legacy fact edges before deleting that discovered set
  with the record as one atomic batch on transactional stores. Discovery is
  outside that transaction, so callers must synchronize concurrent link and
  tombstone operations.

- Updated `grust-sail` for current Sail Spark Connect sessions. Each session
  can leave its warehouse server-managed or configure and verify an explicit
  absolute override. Client-local session-scoped warehouses are opt-in and
  caller-cleaned, keeping the default safe for remote endpoints and persistent
  server catalogs. Generated Delta tables retain named columns and structural
  non-null constraints, registry values are staged through Arrow, and session
  temp views can be dropped safely and idempotently.

- Added bounded Arrow IPC collection to `grust-sail`. Accepted batches move
  into the collection without a second copy; inclusive chunk and cumulative
  byte limits fail before retention. Spark Connect accepts at most a 17 MiB
  decoded message, reserving 16 MiB for Arrow payload and 1 MiB for protobuf
  envelope metadata.

- Added canonical cognition operation parsing and hardened planning around one
  TypeSec-authorized input and binding. LakeCat evidence, projection, field
  mapping, source revisions, and label joins are checked before Sail runs;
  proposals are born bound. Live Sail uses collision-safe temp views and always
  attempts cleanup under its own bounded deadline, including after caller
  cancellation; operation and abort time are bounded independently. It
  requires the complete exact result set under
  fixed source-count, authorized-input byte, Arrow, result, projection,
  identity, mutation, and local-work budgets shared with TypeSec and the
  reference engine. Cognition rejects excessive encoded Arrow collection and
  declared row, buffer, or decompressed sizes before `StreamReader` allocates
  result arrays, then rechecks the complete result locally. Native provenance
  is accepted only for exact bounded operation identities. Deduplicate and
  reconcile each use an explicit version-2 semantic contract shared by their
  reference and Sail profiles; package/build versions are implementation
  metadata and never mutation authority. Legacy version-1 or package-bound
  intents fail closed and require fresh authorization. Fixed host-selectable
  profiles let a trusted composition registry bind an engine independently and
  check signed intent before authorized input is loaded; public engine
  implementations cannot self-report identity, and Sail executors cannot
  choose a version after execution. TypeSec canonical proposal validation also
  rejects an executor's malformed or over-budget output before it leaves the
  engine wrapper. The adapter exposes only fixed failure categories rather than
  caller, source, or adapter text.
  Reference and Sail proposals now share permutation-stable canonical dedup and
  reconciliation planning, including ID tie-breaking at Sail's staged timestamp
  precision. Optional TypeSec-governed source scopes are preserved through
  bound proposals, durable audit evidence, retry, and reopen; explicit local
  cognition remains scope-free. Bound proposal schema version 4 now binds the
  immutable snapshot digest separately from the LakeCat grant digest and names
  an explicit `mutated` or `no_change` effect. Reference and Sail engines derive
  that effect from the complete canonical plan. Durable outcome schema version
  3 validates TypeSec audit schema version 2, including the same effect,
  separate snapshot identity, and authority-revalidation time. Audit and
  commit-envelope digest domains are version 3, so this evidence epoch cannot
  collide with either earlier layout.

- Added durable cognition jobs and atomic application to `querygraph-memory`.
  Renewable bounded leases, cancellation, retry, digest-only proposal state,
  subject-and-purpose-scoped job identities, and an ID-only leased outbox
  survive reopen. Every derived job, outcome, audit, outbox, and ledger address
  includes TypeSec's opaque authority-scope digest. Only TypeSec's opaque
  prepared token can atomically exact-guard sources, apply the exact memory
  operations and ID-only outbox, write audit evidence, persist the outcome, and
  complete the job. A typed no-change decision has no memory or outbox
  mutations, retains the prior memory version, and still commits its job,
  audit, outcome, and guarded ledger atomically.
  Scheduler submission, proposal staging, commit, and recovery bind the same
  verified TypeDID request digest.
  Idempotent recovery cross-validates the durable job, authority scope, audit,
  outcome, and backend receipt; raw bearer, owner, failure, proposal, and
  plaintext values are never persisted in scheduler metadata. A job's logical
  transition time is caller-supplied; completed jobs bind it explicitly to
  TypeSec preparation, and their completion digest is the exact canonical
  TypeSec prepared digest for either effect, never the resulting memory
  version. Authoritative backend commit time exists only in the outcome and
  receipt. Backend commit time must
  be canonical RFC 3339 and
  must not predate preparation; malformed or regressive evidence fails closed
  instead of being relabeled as another phase. Commit-then-response-loss tests
  prove that retry and reopen retain exactly one mutation, job, audit, outcome,
  and outbox manifest. Concurrent identical applications recover the original
  byte-stable evidence even when completion wins between the initial recovery
  lookup and either the exact-source or staged-job read.
  Recovery checks durable schema versions before deserialization, rejects
  checked-in legacy outcome and audit shapes precisely, and requires affected
  memory IDs to retain TypeSec's strict canonical order.
  Scheduler and outbox methods are explicitly storage primitives for an
  authenticated trusted worker pool: submitters and transferable workers may
  differ, scoped keys and owner strings are not credentials, and active lease
  or claim tokens are bearer credentials for worker transitions.

- Clarified that these Grust capabilities are the generic governed-cognition
  substrate; standalone Marciana still owns authenticated orchestration,
  product receipts, and the pending qg-rust cognition cutover.

- Added a domain-neutral guarded graph commit capability and a durable Turso
  implementation. Exact-node and absence expectations, graph mutations, and a
  backend-issued commit identity now share one transaction; identical retries
  return the original receipt, while idempotency-key digest collisions fail
  closed. A read-only guarded-receipt lookup recovers an exact prior commit
  without issuing a probe mutation. Turso mints its receipt time at nanosecond
  precision immediately before the ledger insert, persists and replays those
  exact backend-issued transaction-boundary bytes, and rejects a malformed
  durable timestamp without disclosing it.

- Added ignored live-Sail conformance tests for Marciana cognition that
  separately compare distributed deduplication and reconciliation with the
  reference engine, prove repeated jobs are deterministic, and reject governed
  inputs or source plaintext in audit evidence. The integration launcher now
  runs them alongside `grust-sail`, accepts an explicit `SAIL_TEST_BIN`, and no
  longer silently substitutes an unrelated `sail` executable from `PATH` for the
  configured source checkout.

- Added an optional live Spark Connect cognition executor to the private
  `querygraph-memory` integration crate. Governed memories are staged in a
  session-local Arrow view under a random collision-safe name. Sail computes
  bounded deduplication and reconciliation candidates, and only inert TypeSec
  plans return; authorization secrets are never staged or logged.
- Made the cognition engine and Sail executor contracts asynchronous so live
  Spark Connect execution does not block a runtime worker.

- **QueryGraph-native Marciana cognition boundary:** `querygraph-memory` now
  binds cognition jobs to a hash-verified LakeCat/Iceberg snapshot, governed
  Sail scan projection, verified TypeDID subject, purpose, and authorization
  receipt. Reference analytics and injectable Sail executors emit inert
  TypeSec cognition proposals without receiving a store handle.

- **Grust book covers durable TypeSec Memory:** expanded the architecture tour
  with the implemented `querygraph-memory` boundary, including opaque record
  storage, the `MemoryRecord`/`MemoryEntity` graph, space-only pushdown,
  transactional Turso consolidation, the sanctioned synchronous/asynchronous
  bridge, privacy-aware reference vector ranking, inert cognition plans, and
  the explicit post-v1 security, scale, and hosting limits.

- **First Pair Press image cover:** adapted the live Grust announcement
  headboard into a portrait cover, set Alexy Khrabrov as the sole author,
  added the reusable First Pair Press publisher seal, and wired the same image
  into PDF, EPUB, MOBI, and hosted HTML output.

- **Unified FirstPair book build:** `book.build.json` now delegates the Grust
  book to FirstPair's pinned Pandoc/Typst and Mermaid toolchain while retaining
  `build.mjs`, EPUB repair, and page-label hooks. PDF, EPUB, MOBI, single-file
  HTML, chapter HTML, manifest links, and rendered layout checks share one
  publish-complete contract.

- **PostgreSQL joins the executing Cypher conformance set**
  (`docs/GQL_POSTGRES_EXECUTOR_GOAL.md`, complete): `PostgresGraphStore` now
  implements `CypherMutationExecutor` (resolved writes, bounded matched-node
  patches via `jsonb` predicates, atomic `CypherTransaction` batches through
  its transactional `apply_mutations`) and `run_read_query` pushdown over the
  universal tables via a new `PostgresReadDialect` — tagged-jsonb extraction
  (`#>> ARRAY['key','value']`), `position()`/`right()` string predicates,
  `generate_series` for `tvf.range`, `jsonb_object_keys` for
  `db.propertyKeys`, byte-order (`COLLATE "C"`) procedure sorts, and
  `WITH RECURSIVE` variable-length paths (the first non-SQLite engine to push
  them). Shortest paths and correlated `tvf.keys` are honestly gated off (no
  insertion-ordered rowid; jsonb key order) and fall back to the reference;
  `ORDER BY` stays in the reference (collation). Proven by a gated live
  differential suite (`GRUST_PG_URL`) run green against PostgreSQL 17:
  pushed reads, fallback reads, writes, and transaction scripts all match the
  Memory reference. The pushdown walk CTEs' remaining SQLite-isms (`instr`,
  edge columns in the recursive step, `ORDER BY` collation under `DISTINCT`)
  became dialect hooks; the executing set is now Memory/Sail/Turso/Postgres.

- **`querygraph-memory` durable v1:** typesec-memory's `MemoryStore` runs over
  compatible Grust `GraphMutationStore` backends, with Turso/libSQL as the
  default and proven persistent path. `TursoMemoryStore::open(path)` creates
  and bootstraps the database on the store-owned bridge runtime; the advanced
  `open_with_config` constructor supports table-prefix and journal tuning.
  The bridge now owns Tokio I/O/time drivers and shuts down safely when the
  store is dropped inside an async service. File-backed integration tests run
  the full TypeSec conformance corpus, close/reopen the database, preserve a
  capability-gated transactional consolidation and its SecLib label join,
  and exercise construction and use from inside an existing Tokio runtime.
  Records and their entity graph remain incremental nodes/edges, space scope
  is pushed into the backend, and all remaining query dimensions use the
  shared conformance-pinned matcher. The reference `VectorIndex` enforces
  embedding privacy, analytics emit plans through the vault front door, and
  the shared-store test proves vault-level tenant authorization. TypeSec
  Memory is published in `0.13.0` Lido; this adapter remains
  `publish = false` pending a deliberate Grust release/distribution decision;
  its TypeSec contract is pinned to the reviewed public Git revision so clean
  checkout and integration CI builds do not require a sibling repository. LanceDB ANN,
  Sail-distributed cognition, fuller GQL pushdown, and a hosted multi-tenant
  service are explicitly post-v1 work rather than claims of this release. The
  adapter is consumed by qg-rust's signed-only memory API and its qg-python
  Pydantic AI v2 restart demonstration. The workspace lock is aligned with
  TypeSec `0.13.0` Lido.

- **Turso joins the read-pushdown consumers**: `TursoGraphStore::run_read_query`
  now pushes the portable read subset into SQL over the universal tables via a
  new `TursoReadDialect` — tagged-JSON property extraction (`$.key.value`),
  `from_id`/`to_id`/`label` edge columns (the pushdown SQL builders'
  edge-column names are now dialect-owned), typed-JSON ordering — with the
  embedded engine's gaps honestly gated (`WITH RECURSIVE` and `json_each`
  leaves report unsupported and fall back to the reference over
  `read_graph()`). The Turso backend descriptor flips `portable_reads` and
  `read_pushdown` to true. A store-level differential test runs pushed and
  gated shapes end-to-end against the reference.

- **Reference executor expression gaps closed**: map literals evaluate,
  list/map indexing works (negative indices, out-of-range → NULL),
  `RETURN DISTINCT … ORDER BY` sorts deduplicated rows by projected items,
  `RETURN *`/`WITH *` combine with aggregates (star variables become grouping
  keys), and multi-label patterns are conjunctive over the single-label model
  instead of erroring.

- **PUSHDOWN2 PM3**: correlated subqueries and shortest paths now push into
  backend SQL. A correlated inner `WHERE` (e.g. `b.age > a.age` under type
  hints) lowers through the segment predicate machinery into the subquery
  join's `ON` clause — the inner pipeline, aggregates included, still runs in
  the reference — and the correlation rule relaxed to reject only *rebinds*
  of the outer variable (pure references are honored by the reference seeds).
  Endpoint-only `shortestPath`/`allShortestPaths` lower to a recursive walk
  CTE with per-pair minimal-depth selection (SQLite-gated); `allShortestPaths`
  keeps tie multiplicity and `shortestPath` picks the reference's DFS-first
  path via a zero-padded edge-`rowid` sequence key. The oracle exposed and
  fixed an F10 reference bug: a no-`*` relationship inside `shortestPath`
  searched unbounded instead of exactly one hop. Oracle 20 differential
  tests.

- **PUSHDOWN2 PM2**: uncorrelated `CALL { … }` subqueries now push into
  backend SQL (`SubqueryReadPushdown`): a leading subquery lowers to its inner
  node scan, and `MATCH … CALL { … } …` lowers to a `LEFT JOIN ON 1=1` of the
  two scans — LEFT so an inner aggregate over an empty inner scan still
  produces its per-outer-row empty-group row, exactly like the reference. The
  inner pipeline, subquery-`RETURN` join, and outer tail run through the
  shared reference; correlated shapes (including same-name shadowing) fall
  back conservatively. Correlated `tvf.keys(n)` also pushes via a lateral
  `json_each` join (SQLite-gated). The oracle work exposed and fixed a latent
  reference bug: `DISTINCT` over computed `WITH`/subquery-`RETURN` items
  errored ("variable not bound") because dedup re-evaluated pre-projection
  expressions against post-projection rows; dedup now keys on the produced
  rows' values. Oracle +2 differential tests.

- Started the **PUSHDOWN2 goal** (`docs/GQL_PUSHDOWN2_GOAL.md`): PM1 done.
  Catalog procedures now push into backend SQL as a `ProcedureReadPushdown`
  leaf — `db.labels`/`db.relationshipTypes` as `SELECT DISTINCT` scans on both
  dialects, `db.propertyKeys` via SQLite `json_each`, and `tvf.range` with
  constant/parameter arguments via a guarded SQLite recursive CTE — with
  `YIELD`/`WHERE`/pipeline tails running through the shared reference so
  results stay byte-identical by construction. Dialect-gated row sources are
  reported through the new `ReadPushdown::supported_by`, and Sail falls back
  to the reference for them. The P0 fallback-pinning test found and fixed a
  bug where F10's `shortestPath(…)` wrapper was not rejected by the pushdown
  lowerers (a bare wrapped var-length pattern lowered as a plain var-length
  scan, returning wrong rows on Sail). Differential-oracle coverage grows by
  two tests over the embedded `turso` and real-SQLite engines.

## 0.12.1 "Shrimp" - 2026-08-06

Shrimp was a scoped crates.io patch, not a lockstep workspace release. It
published `grust-core`, `grust-cypher`, `grust-memory`, `grust-sail`,
`grust-sql-core`, `grust-turso`, and the `grust-graph` facade at 0.12.1 for the
graph-commit and Sail/Turso APIs consumed by Marciana. The other publishable
backend crates remained at 0.12.0, and no `v0.12.1` repository tag, Shrimp post,
TextPack, or 0.12.1 book was produced. Prawn is the first subsequent lockstep
workspace release and supplies those missing cross-workspace documentation and
verification boundaries without rewriting the historical registry state.

## 0.12.0 "Lobster" - 2026-07-03

Note: `0.12.0` widens public enums and structs (`Value::Graph`,
`GqlBackend::Falkor`/`Surreal`, new `GqlBackendDescriptor` and `CallClause`
fields), which is technically breaking for exhaustive matchers even within
`0.x`.

- Added **atomic Cypher transaction batches**: `CypherTransaction` accumulates
  eagerly-planned write statements between `START TRANSACTION`/`BEGIN` and
  `COMMIT` (with `READ ONLY` enforcement), and
  `execute_cypher_transaction_on_store` submits the whole batch in a single
  `apply_mutations` call — atomic on stores reporting
  `GraphMutationAtomicity::Transactional` (proven end-to-end on Turso, whose
  store wraps the slice in one `BEGIN…COMMIT` SQL transaction), and refused
  with a structured feature-tagged error on `OrderedNonAtomic` stores rather
  than silently committing non-atomically.
  `run_cypher_transaction_script_on_store` executes a full
  `BEGIN; …; COMMIT|ROLLBACK` script (lexer-aware statement splitting;
  `ROLLBACK` never touches the store). This closes the Unit 13 deferred tail;
  the `transaction-control` manifest summary now reflects executable
  atomicity.

- The **portable read corpus is now executable**: every case in
  `tests/gql/portable_read.json` (grown to 24 cases covering path/graph
  values, subqueries, TVFs, and shortest paths, including structured
  rejections) runs against the fixture graph and must match its expected
  outcome and error kind.

- **Full39075 is now the realized GQL profile** (Full39075 FM5): with F1–F11
  complete, every non-rejected feature in the manifest is `Supported` (69 of
  74; the other 5 are intentional strict-write rejections — conformance
  guards, not gaps). `docs/GQL_PROFILE_STATEMENT.md` now claims `Full39075` as
  the realized profile, `gql::tests::full_profile_claim_is_backed` pins the
  scoped-out set to exactly the five rejections, and the book chapter on the
  GQL layer is updated accordingly.

- Added **backend-native query passthrough** (Full39075 F11): the conformance
  spine gains `NativeQuery` / `NativeQueryLanguage` (cypher · sql · surrealql),
  a per-backend capability table (`GqlBackend::native_passthrough` + descriptor
  field), `ensure_native_passthrough` with structured feature-tagged
  non-support, and `native_passthrough_backends` reverse lookup. The backend
  catalog now includes **Falkor** and **Surreal** as `native-graph-backend`
  entries (Surreal honestly reports transactional atomicity). Executable
  escape hatches: `FalkorGraphStore::run_native_cypher` and
  `SurrealHttpGraphStore` / `SurrealSdkGraphStore::run_native_surrealql`
  (Sail's `query_arrow_ipc` already covered native SQL). All are deliberately
  outside portable conformance. `NativeCypherPassthrough` is now `Supported`:
  **every non-rejected manifest feature is implemented** (69 supported + 5
  intentional rejections).

- Added **shortest-path matching** (Full39075 F10): `MATCH p =
  shortestPath((a)-[:T*]->(b))` and `allShortestPaths(…)` over a single
  relationship segment now execute on the read reference. Per (start, end)
  endpoint pair the executor finds minimal-length simple paths by iterative
  lengthening over the bounded var-length enumerator; `allShortestPaths` keeps
  same-length ties, `shortestPath` returns the first in deterministic edge
  order. Endpoint, relationship-list, and path variables bind like the
  var-length machinery, and path variables return first-class `Value::Path`
  values. `ShortestPath` is now `Supported`; the candidate `Full39075`
  remainder drops to 1 planned feature + 5 intentional rejections.

- Added **table-valued functions** (Full39075 F9): `CALL name(args) [YIELD …]`
  now accepts argument expressions, evaluated against each incoming row
  (correlated TVFs), with the procedure's rows cross-joined onto the row
  stream. The registry keeps the nullary `db.*` catalog procedures (which now
  reject arguments with a structured error) and adds
  `tvf.range(start, end[, step]) YIELD value` and
  `tvf.keys(element_or_map) YIELD key`. `TableValuedFunction` is now
  `Supported`; the candidate `Full39075` remainder drops to 1 future + 1
  planned feature + 5 intentional rejections.

- Added **`CALL { … }` subqueries** (Full39075 F8) to the read reference
  executor: correlated import-all scoping (the subquery sees the outer row's
  bindings), WITH-style `RETURN` that preserves node/edge bindings for later
  `MATCH` extension, per-row execution (rows with empty subquery results are
  dropped, join semantics otherwise), `UNION`/`UNION ALL` arms, and structured
  rejections for column collisions, missing `RETURN`, and `RETURN *`.
  `Subquery` is now `Supported`; the candidate `Full39075` remainder drops to
  2 future + 1 planned features + 5 intentional rejections.

- Added **first-class graph values** (Full39075 F7): `grust_core::GraphValue`
  and `Value::Graph` model set-shaped graph values — construction deduplicates
  nodes by id and relationships by id-or-endpoint identity, preserving
  first-seen order for deterministic serialization. `GraphValue::from_graph` /
  `from_graph_parts` build graph values from snapshots, `Value::to_json` keeps
  the `{nodes, relationships}` shape, and the read reference executor gains a
  `graph(nodes, relationships)` constructor (e.g. over `collect(...)` lists)
  plus `nodes(g)` / `relationships(g)` accessors. `GraphValues` is now
  `Supported`; the candidate `Full39075` remainder drops to 3 future + 1
  planned features + 5 intentional rejections.

- Added the **Full39075 follow-on goal** (`docs/GQL_FULL39075_GOAL.md`) and
  landed F1 index-definition support: `cypher_ddl` / `sail_cypher_ddl` now parse
  portable single-property `CREATE INDEX name [IF NOT EXISTS] FOR ... ON (...)`
  and `DROP INDEX name [IF EXISTS]` DDL for node and relationship properties.
  `CypherConstraintRegistry` tracks named index metadata through
  `named_indexes()`, new public metadata types (`GraphIndexDefinition`,
  `GraphIndexElement`, `NamedGraphIndex`) are exported through `grust-cypher`,
  `grust-sail`, and the `grust-graph` facade, and `GqlBackendDescriptor` gains an
  `index_ddl` capability flag. `IndexDefinition` is now `Supported` in the GQL
  manifest; the candidate `Full39075` remainder drops to 8 future + 2 planned
  features + 5 intentional rejections.

- Added **graph-type definition DDL** (Full39075 F2): `cypher_ddl` /
  `sail_cypher_ddl` now parse portable `CREATE GRAPH TYPE name [IF NOT EXISTS]
  [OPEN|CLOSED] AS ...` and `DROP GRAPH TYPE name [IF EXISTS]` metadata. The
  first supported body surface covers `NODE Label (...)` and directed
  `EDGE Type FROM Source TO Target (...)` declarations with scalar/array field
  types and `REQUIRED` / `NOT NULL` markers, lowering to `GraphSchema` inside
  `GraphTypeDefinition`. `CypherConstraintRegistry` now tracks named graph types
  through `named_graph_types()`, and `GraphTypeDefinition` / `NamedGraphType` are
  exported through the language crate and facade. `GraphTypeDefinition` is now
  `Supported` in the GQL manifest; the candidate `Full39075` remainder drops to
  7 future + 2 planned features + 5 intentional rejections.

- Added **portable catalog metadata** (Full39075 F3): `CypherConstraintRegistry`
  can now materialize a `CypherCatalogSnapshot` for a named graph, carrying graph
  type, index, and named constraint metadata. `cypher_catalog_procedure` exposes
  deterministic read-only metadata tables for `db.graphs`, `db.graphTypes`,
  `db.indexes`, and `db.constraints`, and `GqlBackendDescriptor` gains a
  `catalog_metadata` capability flag. `CatalogMetadata` is now `Supported`; the
  candidate `Full39075` remainder drops to 6 future + 2 planned features + 5
  intentional rejections.

- Added **named graph selection** (Full39075 F4): the parser and semantic
  analyzer now recognize `USE <graph>` clauses, the Memory read reference path
  validates the selected graph before execution, and
  `run_read_query_on_named_graph` lets callers bind a single-graph snapshot to a
  non-default graph name. Catalog-backed callers can validate graph names through
  `ensure_catalog_graph_selection`, and `GqlBackendDescriptor` gains a
  `named_graph_selection` capability flag. `NamedGraphSelection` is now
  `Supported`; the candidate `Full39075` remainder drops to 5 future + 2 planned
  features + 5 intentional rejections.

- Added **session control** (Full39075 F5): `CypherSession` tracks the current
  graph and portable session settings, while `SessionCommand::parse` /
  `SessionCommand::apply` handle standalone `USE`, `SET name = literal`,
  `RESET name`, and `RESET ALL` commands. `USE` can validate against a
  `CypherCatalogSnapshot` before changing session state, and transaction-control
  behavior remains unchanged. `GqlBackendDescriptor` gains a `session_control`
  capability flag. `SessionControl` is now `Supported`; the candidate
  `Full39075` remainder drops to 5 future + 1 planned feature + 5 intentional
  rejections.

- Added **first-class path values** (Full39075 F6): `grust_core::PathValue` and
  `Value::Path` now represent fixed-length path bindings directly while
  `Value::to_json` preserves the existing `{nodes, relationships}` serialization
  shape. Returning a path variable now yields `Value::Path`, and existing
  `nodes(p)`, `relationships(p)`, and `length(p)` behavior remains compatible.
  `PathValues` is now `Supported`; the candidate `Full39075` remainder drops to
  4 future + 1 planned feature + 5 intentional rejections.

## 0.11.0 "Crab" - 2026-06-26

- **Turso MVCC + concurrent writes:** `TursoConfig` gains a `journal_mode: TursoJournalMode` field (`Wal` default, or `Mvcc`). `Mvcc` enables Turso's multi-version concurrency control via `PRAGMA journal_mode = mvcc` on connect — a database-*header* mode, so it only applies to a fresh database (an existing WAL database can't be converted; `connect` verifies the mode and errors otherwise). In MVCC mode, data writes (`put_node`/`put_edge`/`put_graph`/`delete_*`/`apply_mutations`) run inside a `BEGIN CONCURRENT … COMMIT` transaction with bounded retry on write-write/busy conflicts, so concurrent writers make progress; WAL-mode behavior is unchanged. Verified end-to-end: mode reported as `mvcc`, `BEGIN CONCURRENT` accepted, batch round-trips, and two concurrent writers writing overlapping keys both succeed via retry.

- **Profile statement (Unit 16):** added `docs/GQL_PROFILE_STATEMENT.md` — the precise, backed statement of the realized GQL/Cypher profile (58 of 74 catalogued features `Supported`) with every not-yet-supported feature explicitly enumerated and given a rationale, so the candidate `Full39075` claim is never silently unbacked. A new `full_profile_claim_is_backed` test pins the scoped-out set (8 future + 3 planned + 5 intentional rejections) to the manifest, so flipping any feature status forces the doc to be updated in lockstep.

- **Write widening (Unit 10b, W1/W2/W3):** a `MATCH … CREATE/MERGE` clause may now carry **multiple comma-separated relationship patterns** in one statement (`… CREATE (a)-[:R]->(b), (b)-[:S]->(c)`), each planned in order (W1); **incoming `<-[:T]-` edge writes** are accepted, normalized to the arrow's source→destination (W2); and **cross-variable correlated `SET`** (`MATCH (a)-[:R]->(b) SET a.x = b.y + 1`, and the cartesian `MATCH (a),(b) …` form) is supported via a new `GraphMutationPlanOp::SetMatchingNodeFromNode` executed by the Memory reference backend (other backends reject explicitly) (W3). Single-pattern / outgoing / single-target writes stay byte-identical (golden-guarded). Generated-id-by-default (W4) was intentionally **not** changed — generated ids remain opt-in via `CypherNodeIdPolicy`. See `docs/GQL_U10b_WRITE_WIDENING_AUDIT.md`.

- **Write-path cutover (Unit 10a, decision B):** the writable-Cypher entrypoints now route *acceptance of the mutation grammar* through the new standards-conformant parser as a gate, narrowing the public accept-set to standard GQL/Cypher. The non-standard **DELETE-by-pattern** forms the legacy string planner accepted (`DELETE (:Person {id})`, `DELETE (:a)-[:R]->(:b)`) are now rejected — use `MATCH … DELETE <var>` instead. Plan *building* still runs through the legacy planner, so plan shapes stay byte-identical (guarded by `tests/golden/write_golden.json`); only the `RETURN` projection and cross-statement local-variable bindings are intentionally left to the legacy path (the gate is parse-only over each mutation statement, RETURN split off). The new parser also now accepts reserved keywords as property/map keys (e.g. `{order: 1, limit: 3}`), preventing an unintended accept-set regression. Strict-write tests using the non-standard forms were migrated to standard Cypher.

- Added the **transaction-control language surface + capability reporting** (Unit 13). `grust_cypher::transaction` recognizes standalone `START TRANSACTION [READ ONLY|READ WRITE]` / `BEGIN` / `COMMIT` / `ROLLBACK` commands (`TransactionCommand::parse`, returning `Ok(None)` for non-transaction input so query parsing still applies — the keywords are *not* reserved in the lexer, so `start`/`commit`/… remain usable as identifiers). Per-backend atomicity is reported honestly via `GqlBackend::transactional()` / the new `GqlBackendDescriptor::transactional` flag (Turso/Postgres/Postgres-PGQ report `Transactional`; Memory/Sail do not) and `transactional_backends()`. `TransactionControl` is now `Supported`; atomic *execution* (wrapping a batch through the backend store) is delegated and wired after the write-path cutover. `SessionControl` is `Planned`.

- Added first-class **decimal** and **duration** value types (Unit T). `grust_core` gains dependency-free `Decimal` (fixed-point `mantissa(i128) × 10^−scale`, mirroring SQL DECIMAL(38,s); lossless within 38 digits, value-normalized) and `Duration` (ISO 8601 month/day/second/nanos model), each with parse/canonical-display, serde (as canonical string), ordering, and checked arithmetic. `Value` gains `Decimal`/`Duration` variants with `Value::decimal`/`Value::duration` constructors and `as_decimal`/`as_duration`; every backend's value serialization handles them (canonical/ISO strings). The Cypher read executor adds `decimal(...)`/`duration(...)` constructor functions, lossless `+`/`-`/`*` decimal arithmetic (ints coerce exactly; floats route to the f64 path), duration `+`/`-`, and exact decimal/duration comparison & ordering in `WHERE`/`ORDER BY`. `TemporalValues`/`DurationValues`/`DecimalValues` are now `Supported`.

- Added read-only **catalog procedures** via `CALL [YIELD]` (Unit 14): `db.labels()`, `db.relationshipTypes()`, and `db.propertyKeys()` parse in the new pipeline and execute in the Memory read reference over a `Graph` snapshot, returning deterministically sorted, distinct values. Supports standalone `CALL db.labels()` (the YIELD shape becomes the result table) and `CALL … YIELD col [AS alias] [WHERE …]` feeding downstream `WHERE`/`RETURN`/aggregation. `ProcedureCall` is now `Supported` in the feature manifest; procedure *arguments* remain feature-tagged unsupported.

- Expanded the read-path scalar function registry (Unit 14) with unary math functions `sqrt`, `exp`, `ln`/`log`, `log10`, `sin`, `cos`, `tan` (numeric → Float, null-propagating), usable in `WHERE` and `RETURN`.

- Added a strict-write **golden-snapshot** regression harness (`grust-cypher/tests/write_golden.rs` + `tests/golden/write_golden.json`, Unit 10a): pins the current planner output (plan or rejection) for a 20-statement write corpus so any future write-path change is caught byte-for-byte.

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
