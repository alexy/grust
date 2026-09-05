# Native Neo4j comparison lane

The example and SF0.1 rotating W2/R10 runs are now published at
[adversari.al/graph](https://adversari.al/graph/), with independently verified
counts, observations, runtime metadata and timing summaries. This is a native
engine comparator, not a Grust Neo4j adapter. Existing Grust receipts remain
unchanged; earlier diagnostics below retain their original qualification limits.

## Published comparison evidence

Both runs use client source `4995115ad95e7e12215e86bcc13e60a78ddcea00`, a
60-second query deadline, rotating query order and fresh isolated workers.
Each passes all264 samples (44 warm-ups and220 measurements), with zero
mismatches, timeouts or errors. Each Docker component has8CPU/6GiB without swap;
the host is shared. These are not LDBC Benchmark Results.

| Dataset | Frozen bundle SHA-256 |
| --- | --- |
| Example | `88d82a42e516f8e7746dccb2cb7d93dd7c11eb8ad94116ac1ec92850d95353b8` |
| SF0.1 | `7191d265ca6f1a6e602cd98509447ba9e6fd23fa8baf44dd35c29a5c687c6289` |

SF0.1 contains432,235 nodes and2,080,404 edges. Import took70.403seconds.
Measured q4 median is1.975seconds (range1.843–3.162); the reversed-chain attack
median is19.015seconds (range18.020–19.964). Setup and recovery are separate;
query time includes scalar consumption and rollback completion. Raw samples
and every query summary are published alongside the bundle, not pooled with
legacy query-major runs or translated into concurrent throughput.

SF0.3 rotating repeated qualification remains pending. The earlier SF0.3
single-run diagnostic below is not substituted for that comparison.

## Startup readiness

The Docker wrapper now waits for `cypher-shell` to return the exact scalar42
before creating the benchmark client. Readiness has a120second total deadline,
individual attempts are bounded to10seconds, and every attempt emits a status
record. This prevents Docker's running state from admitting a server whose Bolt
listener is still starting. Startup failures stop the selected disposable
server before import; readiness time is not a query performance sample.

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
client metadata is captured before the watchdog removes the container. The live
targeted-recovery probe passed through this runner, and retained runtime records
passed the independent runtime audit. Earlier diagnostic directories used the
temporary runner and must not be represented as having these additional records.

SF0.3 also passed all22 cases, with no timeouts, mismatches or OOM:
1,179,535 nodes and6,183,839 edges, import241.602seconds, whole run367.018seconds.
The independent diagnostic audit passed. A subsequent process-deadline probe
exposed a possible SHOW/TERMINATE race after worker exit. The runner failed closed
and stopped the owned server. Neo4j confirms `Transaction not found.` for an
absent transaction; the fix distinguishes disappearance from acknowledged
termination and still requires independent absence and next-query proof.
Regression tests cover both responses and reject wrong transaction identities.
The race-handling build `bb4f7c161fdae90cc9fc2b35aaf0870e9da91164` passed three
consecutive Docker process-deadline probes and a targeted-termination probe.
All four retained-runtime audits passed. The disappearance branch was not
observed in those three deadline runs; its exact response classification is
covered by the regression test and the live absent-transaction response check.
The new client platform manifest is
`sha256:cb694a3f3b15f0008cec656bdda295cd1d7ff82b56d2929b304b8b06b53630db`.
Use the diagnostic validator's `--runtime` option to also check supported
source/image pairs, stable container identities and resource limits, process
states, OOM flags and watchdog ownership. This still does not issue a
publication receipt or establish an isolated performance ranking.

The fresh example run `out/neo4j-runtime-example-bb4f7c1` passed all22 cases
and the combined diagnostic/runtime audit. Its watchdog completed in21.053seconds,
and all client/server before/after records were retained before client cleanup.
This is the first full native example diagnostic with the new runtime evidence;
larger-scale runs above still belong to their original source/provenance cohort.

## Repeated measurements

The native binary accepts optional `WARMUPS RUNS` after its four qualification
arguments. The Docker runner exposes these as paired `--warmups` and `--runs`
options. Bounds are0–5 warm-ups and1–10 measurements per query. Without them,
the legacy W0/R1 protocol is unchanged. With them, schema v2 records an explicit
schedule. The initial4c385e2/242b6b8 builds group samples by query; these are
within-backend diagnostic cohorts, not matched Grust comparisons. Subsequent
builds match Grust's suite/phase-major schedule: run every query in each round,
rotate the starting query by the round index, and reset rotation when moving
from warm-ups to measurements. Both runners use the same rotation helper.
Each sample executes in a fresh worker
with its own server-recovery proof. Phase and zero-based sample index appear
in every start/observation record. No warm-up sample enters measurement totals;
warm-up failures remain visible separately and do not get silently dropped.

The full comparison cohort will use that rotating schedule, W2/R10 and the explicitly selected
60second query deadline. Grust matrix defaults are W2/R10 but30seconds, so its
deadline must be overridden to match; existing W0/R1 results are a separate
qualification cohort. For repeated runs, the runner computes an emergency outer
ceiling from the import allowance and all per-sample bounds. This ceiling is not
an expected duration;30second heartbeats and flushed per-sample records continue
throughout. The W2/R10 example Docker run of source
`4c385e26135547f1771577f20a90234f830488b6` passed all44 warm-ups and220 measured
samples in60.812seconds. Its query-major sampling, oracle/journal and runtime
audits passed; it must not be pooled into the rotating comparison cohort.
Output: `out/neo4j-sampled-example-4c385e2`. The client platform manifest is
`sha256:2f68b1f5e47a3627124ec02390de3088660059c860c2ab2461cf133ba9af3aca`.
Matched repeated larger-scale runs and publication receipts remain pending.
The rotating example run of source `4995115ad95e7e12215e86bcc13e60a78ddcea00`
passed all 44 warm-ups and 220 measured executions. Its independent diagnostic,
runtime, and matched-sampling audits passed. Evidence is retained in
`out/neo4j-rotating-example-4995115`; this is not yet a published comparison.
Use `--runtime --matched-sampling` with the diagnostic validator to require the
rotating W2/R10 cohort and 60-second query deadline. The gate rejects historical
query-major cohorts even when their repetition counts agree.
The validator supports both historical and rotating schedules and binds each
declared order to source-image capability. Identical repetition counts alone
do not establish a matched comparison protocol.

The diagnostic validator's `--summaries` option emits measured-only raw timing
series and minimum/median/maximum. It withholds a query's timing summary if any
warm-up or measured sample failed, avoiding a success-only timing summary that
would conceal failures. These summaries are descriptive, not a ranking against
unmatched backend cohorts.

Subsequent sampling builds also bound the asynchronous import and database-total
checks to600seconds, independently of the larger overall sampling ceiling.
A load failure or deadline aborts the run; the Docker runner stops the owned
server rather than proceeding to queries. This is separate from the strict
per-query process deadline. The earlier4c385e2 example run predates this added
load guard and must not be relabeled as exercising it.

## Required completion gates

`bundle-native-neo4j.py INPUT OUTPUT` freezes the eight allowlisted structured
records, audits those exact bytes including runtime and matched sampling, and
exports a checksummed manifest plus measured-only summaries. It refuses to
overwrite an existing output and writes the manifest last. Raw Docker logs are
not exported. This transport bundle remains `publication_qualified: false`;
independent site admission and a publication receipt are still required.

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
