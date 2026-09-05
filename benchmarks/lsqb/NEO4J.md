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
`db.transaction.timeout = 1m`; the later targeted cancellation probe is
described below.

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

The Docker-client/Docker-server example diagnostic also passed all nine
baseline queries and thirteen attacks on 2026-09-05 UTC. Its22 incrementally
flushed observations exactly match the final diagnostic. The outer watchdog
completed successfully in16.279seconds. Client source was
`3628d2d3bffc9f4324e418818d1c76e6f9f30e9f`; both containers had8CPU/6GiB limits
on a shared host. Evidence is retained under
`out/neo4j-native-docker-example-3628d2d`. This remains W0/R1 qualification,
not a published performance ranking or a complete hard-deadline receipt.

`recovery-probe` now tests a uniquely tagged running transaction, targeted
termination acknowledgement, worker error, discarded worker connection pool,
absence observed through a separate connection, and a successful subsequent
scalar query. Live qualification passed this sequence in about1.42seconds.
The first attempt failed the absence check while retaining the worker pool;
the failed pool must never be reused. Neo4j2026.07.1 exposes `tx.setMetaData`,
not the older `dbms.setTXMetaData` spelling. This server-cancellation probe does
not yet establish the full isolated-process benchmark deadline contract.

The native qualifier now uses the shared READY/GO isolated-process protocol:
transaction creation and unique metadata tagging precede READY; query execution,
scalar consumption and rollback follow GO. The coordinator enforces a60second
query deadline, reaps the worker, and independently verifies server absence and
a scalar probe within15seconds before continuing. Any remaining tagged
transaction receives targeted termination; protocol or recovery failures reject
the run. Setup, query, process-reaping and server-recovery times are separate.

The `deadline-probe` command forces a2second process deadline and then executes
a fresh isolated scalar query. A host-client/Docker-server live probe passed:
deadline observed at2.009seconds, SIGTERM/reap5.6ms, server absence and scalar
verification260ms, next isolated result42. No transaction remained after
process death in this probe, so no termination acknowledgement was needed.
The separate `recovery-probe` above exercises the targeted-termination branch.
These probes are diagnostics; independent publication receipts are still
required.

The host-client/Docker-server example rerun with isolated workers passed all
22 cases. Each observation records a normal worker exit, zero remaining owned
server transactions and a successful independent scalar probe; the flushed
journal exactly matches the final diagnostic. Output:
`out/neo4j-native-process-example`. This rerun is not the earlier Docker-client
run and the two timing boundaries must not be pooled.

Docker qualification of source `aaf0999706fa8cfdb7eeb10e8349b9a471229857`
subsequently passed both recovery probes and all22 example cases. The2second
deadline was observed at2.0085seconds with SIGTERM/reap11.6ms, server recovery
792.5ms and next isolated scalar42. The separate targeted-termination probe
observed the stress query running and completed recovery in384ms. The example
watchdog completed in16.261seconds; all22 transaction tags were distinct, all
server-recovery checks passed and journal/report observations matched exactly.
The Docker client platform manifest is
`sha256:c32f508bd0f86f63fb97fdbcffbd0e6f101552b6db0ace4dc3d757dd40468b9d`.
Artifacts are the three `out/neo4j-native-docker-example-*-aaf0999` directories.
No published ranking or larger-scale result is implied by example qualification.

SF0.1 Docker qualification of the same source/image passed all nine baseline
queries and thirteen attacks:432,235 nodes and2,080,404 edges. Import took
89.923seconds and the whole diagnostic completed in145.877seconds; no query
timed out. The independent Python diagnostic audit checked pinned upstream
counts, attack counts, ordered journal equality, distinct transaction tags,
normal process exits and server recovery. This remains a W0/R1 diagnostic on a
shared host, not a published performance ranking.

Run `python3 benchmarks/lsqb/validate-neo4j-diagnostic.py OUTPUT_DIRECTORY`
to audit these new Docker diagnostics. Its mutation tests are in
`test-neo4j-diagnostic.py`. The audit deliberately returns
`publication_qualified: false`: it does not authenticate the complete
container/source/resource provenance or issue a publication receipt.

`run-native-neo4j.py` provides a reusable Docker entry point for subsequent
qualification. It requires an explicitly selected disposable service, checks
the pinned server image and8CPU/6GiB/no-swap limits, addresses that exact
server's private IP, and creates the client from its immutable inspected image
identity. Client and server before/after metadata are retained without exporting
arbitrary environment variables. Failed cells stop the selected server; exited
client containers remain available for inspection. This runner's live integration
qualification is pending; earlier diagnostic directories used the temporary
runner and must not be represented as having these additional records.

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
