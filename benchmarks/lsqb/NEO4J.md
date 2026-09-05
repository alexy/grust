# Native Neo4j comparison lane

This lane is under implementation, not published benchmark evidence. It adds
Neo4j as a native engine comparator; it does not invent a Grust Neo4j adapter.
The existing twelve-backend Grust receipts remain unchanged.

## Pinned qualification target

- Neo4j Community 2026.07.1, the current server release identified in
  [Neo4j's release notes](https://neo4j.com/release-notes/) on 2026-09-05 UTC.
- Linux ARM64 platform image:
  `neo4j:2026.07.1-community@sha256:31697c776d8c255152be39430d4b306a414c1409c91dccd093ac5e6baf2cae9d`.
- Rust driver: `neo4rs = 0.9.0-rc.10`, pinned exactly. This is a prerelease
  from [Neo4j Labs](https://github.com/neo4j-labs/neo4rs); runtime/protocol
  compatibility must be demonstrated, not inferred from the version number.

The initial `grust-lsqb-neo4j probe` command is read-only and reports server
version, edition, driver version, and scalar decoding. It uses explicit
transactions and rollback, not automatically retried convenience queries.
Configuration uses `NEO4J_URI`, `NEO4J_USER`, and `NEO4J_PASSWORD`; errors do not
echo endpoint credentials. A probe pass is not a benchmark result.

The initial live probe passed on 2026-09-05 UTC against the pinned ARM64 image:
server2026.07.1 Community, scalar42, explicit rollback acknowledged. Both
scalar-shape unit tests passed. Server inspection confirms
`db.transaction.timeout = 1m`; forced cancellation recovery is not yet qualified.

The `qualify LSQB_ROOT ATTACKS_DIR SCALE NEW_OUTPUT_DIR` subcommand now imports
bounded chunks using the shared Rust dataset and oracle validators. Import
requires `NEO4J_BENCHMARK_DISPOSABLE=1` and an empty database; it never clears a
database. It preserves Post/Comment labels alongside Message, stable node IDs,
and edge multiplicity. An ID uniqueness constraint supports indexed endpoints.
Every batch verifies its inserted count, and final database totals are checked.

The first example diagnostic passed all nine original native baseline queries
and thirteen attacks with a host Rust client and Docker server. All22 flushed
journal observations exactly match the final diagnostic report. Reimport into
the nonempty database was refused. These are W0/R1 diagnostics, not published
or resource-isolated performance measurements. A transaction failure stops the
run; it is not silently reclassified as a successfully recovered timeout.

The shared Dockerfile accepts `BENCHMARK_FEATURE=neo4j-native` and includes the
native executable. Use `--entrypoint grust-lsqb-neo4j` for this lane; it is not
part of the twelve-backend matrix's default entry point.

`recovery-probe` now tests a uniquely tagged running transaction, targeted
termination acknowledgement, worker error, discarded worker connection pool,
absence observed through a separate connection, and a successful subsequent
scalar query. Live qualification passed this sequence in about1.42seconds.
The first attempt failed the absence check while retaining the worker pool;
the failed pool must never be reused. Neo4j2026.07.1 exposes `tx.setMetaData`,
not the older `dbms.setTXMetaData` spelling. This server-cancellation probe does
not yet establish the full isolated-process benchmark deadline contract.

## Required completion gates

1. Reuse the Rust dataset fingerprint/oracle loaders and bounded graph chunks.
   Preserve Message/Post/Comment inheritance and relationship multiplicity.
2. Run native baseline Cypher and the thirteen adversarial cases. Disclose
   native-engine semantics and any query adaptation separately from Grust's
   portable/reference execution; never pool their timing samples.
3. Use explicit non-retrying transactions, coordinator process deadlines,
   transaction identity, and verified server quiescence after cancellation.
   Merely dropping a Rust future does not establish remote cancellation.
4. Emit flushed incremental observations, load progress, version/image/resource
   identity, and distinct setup/query/recovery timing. Require an independent
   complete receipt before the site claims published Neo4j results.
5. Qualify example and downloaded scales in an isolated owned container. Never
   clear or import into an existing user/service database.

The private harness feature is `neo4j-native`, separate from `full-backends`.
No publishable Grust crate or public API changes are required for this lane.
