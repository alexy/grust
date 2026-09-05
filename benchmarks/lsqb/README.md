# Graph benchmark harness

This directory keeps two deliberately separate tracks:

1. **Upstream reference run.** The pinned Graph Data Council (GDC) repository's
   LSQB Ladybug scripts and example dataset run unchanged in a container.
2. **Grust compatibility and adversarial runs.** Grust imports the same pinned
   projected-foreign-key data, checks the original LSQB query bytes, applies a
   documented compatibility adapter, and emits an explicit twelve-backend
   matrix for the nine LSQB query shapes plus 27 clearly separate adversari.al
   extensions: 13 per-backend count attacks and 14 backend-neutral
   bounded-policy rejection attacks.

LSQB is a GDC-maintained labelled-subgraph-query microbenchmark. It is **not an
official LDBC benchmark**, these runs are not audited, and the checked-in
`sfexample` graph is intentionally tiny. The results here are reproducibility
and conformance evidence, not a general database performance ranking.

> **These are not LDBC Benchmark Results.**

The qualification above follows the GDC/LDBC
[fair-use policy](https://ldbcouncil.org/benchmarks/fair-use-policies/).

## Pinned upstream inputs

| Input | Identity |
|---|---|
| Repository | [`ldbc/lsqb`](https://github.com/ldbc/lsqb) |
| Commit | `242cb2fd31340ca688954cb94794d74c0d5b6f92` (2026-08-04, “Kuzu -> Ladybug”) |
| Full tree | `d99fab28d47791dbc0e7173abc4c66d8aadc64ca` |
| [Codeload source archive](https://codeload.github.com/ldbc/lsqb/tar.gz/242cb2fd31340ca688954cb94794d74c0d5b6f92) | 2,861,380 bytes; SHA-256 `db17ee8b0a8559d6cb7c06e1388e6d89cee2ac924779473ac847965c0c0d37bb` |
| `cypher/` tree | `50937f3d075245e2abd4c00a36c4b3c236766265` |
| Example projected-FK data tree | `45181e6b274d014f8626038e1d398fa1b9e4c19d` |
| Expected-output blob | `4d9dedb2f8c7a42af6defa327303b1aded39e3ad` |
| Expected-output SHA-256 | `f2467b14cd6a060e8513d5357471ae6cff486c2f5e38074febe08a4cf4db0d3a` |
| LSQB source license | Apache-2.0; see upstream `LICENSE` and `NOTICE` |
| Upstream system | Ladybug 0.19.0, as pinned by `ladybug/vars.sh` |

The upstream image downloads the exact commit's codeload archive with Python's
standard library, verifies its byte length and SHA-256 before safely extracting
regular files and directories, and installs no mutable `apt` packages. That
archive fixes the recorded tree, query, example-data, and expected-output
identities. The compatibility runner also verifies a SHA-256 digest for every
q1–q9 source file before executing its separate track.

LSQB publishes projected-FK and merged-FK datasets at `example`, `0.1`, `0.3`,
`1`, `3`, `10`, `30`, `100`, `300`, and `1000`. This harness checks in no
downloaded dataset. The pinned example projected-FK dataset is already in the
upstream repository. The immediately supported downloaded tiers are the
official projected-FK SF0.1 and SF0.3 archives; fetch and verify them with
`fetch-dataset.sh`. See [`DATASETS.md`](DATASETS.md) for hashes, provenance,
and the larger dataset ladder.

The selected post-LSQB ladder uses SNB BI SF1/SF3 for development before SF10
validation and SF30+ analytical performance; SNB Interactive v1 SF1 for
development before SF10 validation and SF30+ OLTP performance; and FinBench
SF0.1 for loader/correctness before SF1 validation and the complete SF10 run. A
separate Graphalytics algorithms track adds a weighted Datagen graph for all
six kernels before multi-billion-edge strain, while Text2GraphQuery remains a
language/model accuracy track.
Dataset sizes, source links, pinning requirements, and fair-use qualifications
are recorded in [`DATASETS.md`](DATASETS.md); none of these development runs
are labelled LDBC Benchmark Results. GDC's [current published benchmark
catalog](https://ldbcouncil.org/benchmarks/) does not list a drop-in ISO GQL
engine-performance suite: SNB BI/Interactive and FinBench are the stronger
database workloads, while Text2GraphQuery tests language-generation accuracy.
Any GQL translation is therefore a separately hashed adversari.al workload
pinned to ISO/IEC 39075:2024 plus Cor 1:2026, never presented as unchanged
upstream input.

## Exact Docker commands

Run the pristine upstream reference first:

```sh
CELL_TIMEOUT_MS=3600000 RUNS=5 SF=example benchmarks/lsqb/run-upstream.sh
```

For the unchanged native Ladybug reference at SF0.1, fetch the pinned archive
first. `run-upstream.sh` validates the extracted directory before building and
requires the exact receipt written by `fetch-dataset.sh`, then bind-mounts only
that dataset into the pinned checkout, read-only:

```sh
benchmarks/lsqb/fetch-dataset.sh --scale 0.1
CELL_TIMEOUT_MS=14400000 RUNS=5 SF=0.1 benchmarks/lsqb/run-upstream.sh
```

Both wrappers enforce the same default 8-CPU, 6-GiB per-container cap with
swap disabled. The upstream wrapper passes that CPU count through Ladybug's
supported thread argument, so its worker count and CPU quota are identical.
It requires a clean committed worktree before the image build and confirms the
same revision after execution. Set a fresh `OUTPUT_DIR` for every run.

Before running queries, the upstream directory receives an atomically written
`environment.tsv` identity containing the OCI-labelled runner image ID,
authenticated source/dataset identities, protocol, container-scoped CPU model,
and resource limits. The image validates exactly q1–q9 for every repetition
against the byte-pinned upstream oracle and atomically emits
`raw-validation.tsv` with every raw CSV hash. Only then does the host create
`complete.tsv`, which hashes both records plus the normalized `watchdog.json`
completion attestation and is the required terminal
publication receipt. Before reporting success, the wrapper invokes
`validate-upstream-bundle.sh`; that standalone gate rejects every unlisted
entry or symlink, enforces the exact TSV schemas and run identity, and
read-only revalidates every raw CSV and receipt hash against the bundled,
byte-pinned `expected-output.csv`. `complete.tsv` hashes that oracle alongside
the environment and raw-validation receipts, making the output independently
reverifiable without a checkout. A failed terminal check removes
`complete.tsv`, so the directory remains explicitly incomplete.
Existing identity, result, validation, or completion files are never
overwritten. The bundle validator deliberately requires the independently
expected revision, image ID, timestamps, resources, and environment identity
as arguments; it consumes the bundled oracle directly. Use
`validate-upstream-bundle.sh --help` for its offline
interface. The non-Docker validator, bundle mutation, and safe-extractor
self-tests are:

```sh
benchmarks/lsqb/test-validate-upstream.sh
benchmarks/lsqb/test-validate-upstream-bundle.sh
benchmarks/lsqb/test-run-upstream.sh
benchmarks/lsqb/test-launcher-portability.sh
benchmarks/lsqb/test-dataset-integrity.sh
benchmarks/lsqb/test-external-service.sh
python3 benchmarks/lsqb/test-cell-watchdog.py
python3 benchmarks/lsqb/test-cell-watchdog-interruptions.py
python3 benchmarks/lsqb/test-cell-watchdog-nested-cancellation.py
python3 benchmarks/lsqb/test-command-progress.py
python3 benchmarks/lsqb/test-fetch-upstream-source.py
```

`Dockerfile.upstream` authenticates the exact codeload archive and installs the
exact Ladybug version requested upstream. For each repetition, the wrapper
copies the pristine source and invokes the upstream scripts; the only supplied
argument is Ladybug's supported explicit worker-thread count:

```sh
cd ladybug/..
SF=example ./ladybug/init-and-load.sh
SF=example ./ladybug/run.sh 8
SF=example ./ladybug/stop.sh
```

The fresh copy matters because this upstream revision's loader cannot safely
reuse the same Ladybug database path for a second initialization.

After the upstream run, execute the separate Grust matrix:

```sh
CELL_TIMEOUT_MS=3600000 RUNS=5 SF=example benchmarks/lsqb/run-grust.sh
```

That builds a core runner and one Cargo-feature image for each optional
adapter, then runs baseline and adversarial cells in canonical order. Every
backend-suite cell gets a fresh container and, where applicable, a fresh
service and volume. It produces 24 one-backend reports, two merged schema-v3
matrices, the one backend-neutral 14-attack policy report, logs, and an image
manifest under `out/matrix-sfexample/`. A feature-build failure, configured
service failure, runner crash, or load failure can never be replaced by a
neutral fallback cell: the run either retains an explicit error result or
stops. Only services explicitly unconfigured below may be `unavailable`;
CocoIndex is explicitly `not_applicable`.

A structurally complete clean publication run atomically adds
`publication-receipt.json`, even when a truthful query or policy outcome
failed and the wrapper consequently exits nonzero. The receipt records each
suite's validity, policy validity, and `all_required_outcomes_valid`; neutral
unsupported, unavailable, and not-applicable outcomes remain explicitly
neutral rather than being called passes. Completion is an evidence-integrity
claim, not a passing-result claim. It binds the exact
40-hex source revision to the bundled canonical evidence manifest, every
component, matrix, policy, and `images.tsv` hash, and the exact output-file
inventory. Each cell also has one normalized record under `watchdogs/` binding
the configured wall-clock limit, measured elapsed wall time, child exit status,
and the immutable container ID, name, project, and service observed by the
supervisor. A missing, timed-out, malformed, or cross-cell record is not
publishable. Recheck a copied or staged result directory without depending on a
mutable catalog file outside the bundle:

```sh
python3 benchmarks/lsqb/validate-matrix-publication.py verify \
  --output-dir benchmarks/lsqb/out/matrix-sfexample
```

Missing receipts, discovery/manual output, symlinks, extra files, mutations,
and report/image identity drift are rejected.

PostgreSQL, FalkorDB, SurrealDB, and pgGraph have pinned, orchestrator-owned
service images. Sail, PostgreSQL PGQ, and Helix remain unavailable by default,
so a run cannot silently discover or reuse a local service. PostgreSQL 19 Beta
3 and the Helix local-development runtime now provide explicit opt-in image
contracts; Sail can use the Grust-built image from `Dockerfile.sail` and the
manual `sail-benchmark-image.yml` publication workflow. A vendor-hosted image
is not required; the resulting registry manifest still needs qualification.
The public [Sail Docker build guide](SAIL-DOCKER.md) reproduces the runtime from
pinned upstream inputs without GHCR access and explains the source-built lane.
The exact qualified candidates and their platform/config digests are recorded
in [`BACKENDS.md`](BACKENDS.md). Ladybug and LanceDB are embedded. Set
`SMOKE=1` for a one-run Memory-only infrastructure check:

```sh
CELL_TIMEOUT_MS=600000 SMOKE=1 \
  OUTPUT_DIR=/tmp/grust-lsqb-smoke benchmarks/lsqb/run-grust.sh
```

Those three external-service cells are executable when an operator explicitly
qualifies a local Docker container. Set the backend's
`*_SERVICE_MODE=external`, existing endpoint variable, `*_VERSION`,
platform-manifest-pinned `*_IMAGE`, matching registry config-digest
`*_IMAGE_ID`, and
`*_CONTAINER`; an optional positive `*_WORKER_THREADS` is recorded too.
Prefixes are `SAIL`, `POSTGRES_PGQ`, and `HELIX`; endpoint variables are
respectively `SAIL_ENDPOINT`, `POSTGRES_PGQ_URL`, and `HELIX_QUERY_URL`. The
endpoint must use `host.docker.internal` with an explicit port, and the declared
container must publish that port on `0.0.0.0` or `::`. Only the current
backend's endpoint is injected into its runner container; every other backend
endpoint is absent. This keeps credentials out of unrelated backend cells.

The orchestrator resolves the declared platform manifest through the registry,
proves that it names the configured registry config digest, and inspects the
running container and its local image before and after each cell. Each
attestation retains `image_id` as that config digest, records the digest suffix
of `*_IMAGE` as `platform_manifest_digest`, and records Docker's local
container/image identity as `runtime_image_id`. Legacy graphdriver image stores
usually expose the config digest as the runtime ID, while containerd-backed
Docker stores can expose the platform-manifest digest; either representation is
accepted only when it is one of those two registry-authenticated identities and
the container and inspected local image agree. The attestation also requires
Linux/runner architecture, CPU and memory limits, disabled swap, no cpuset
pinning, published-port binding, immutable container ID/start time, and restart
count to remain exact. Canonical sanitized inspections go to receipt-bound
service logs, and the publication verifier parses and binds them to the report
and image manifest. The endpoint itself is never recorded because it can
contain credentials. Partial tuples, mutable images, mismatched resources,
loopback-only port publishing, restarts, and changed containers are rejected.

External qualification is opt-in because preparation clears and reloads the
target graph. It never discovers, starts, stops, or reuses an unrelated
container automatically. A typical shape is:

```sh
HELIX_SERVICE_MODE=external \
HELIX_QUERY_URL=http://host.docker.internal:8080/v1/query \
HELIX_VERSION=<exact-version> \
HELIX_IMAGE=<repository>@sha256:<platform-manifest> \
HELIX_IMAGE_ID=sha256:<image-config> \
HELIX_CONTAINER=<running-container-name-or-id> \
CELL_TIMEOUT_MS=3600000 \
RUNS=5 SF=example benchmarks/lsqb/run-grust.sh
```

For a complete twelve-cell development pass from a dirty worktree, use a fresh
directory and opt into discovery mode:

```sh
CELL_TIMEOUT_MS=3600000 DISCOVERY=1 \
  OUTPUT_DIR=/tmp/grust-lsqb-discovery benchmarks/lsqb/run-grust.sh
```

`DISCOVERY=1` remains subject to the rectangular matrix contract but appends an
independently rejected `-discovery` marker to the revision (in addition to any
`-dirty` marker) and skips publication validators. Its output is diagnostic
and must not be checked in, deployed, or described as result evidence. It
cannot be combined with `SMOKE=1`.

After the example conformance gate passes, run the authenticated downloaded
tiers in fresh directories:

```sh
benchmarks/lsqb/fetch-dataset.sh --scale 0.1
CELL_TIMEOUT_MS=3600000 WARMUPS=0 RUNS=1 QUERY_TIMEOUT_MS=600000 \
  WORKER_READY_TIMEOUT_MS=30000 QUERY_REAP_GRACE_MS=1000 \
  QUERY_KILL_REAP_TIMEOUT_MS=5000 QUERY_RECOVERY_TIMEOUT_MS=10000 SF=0.1 \
  benchmarks/lsqb/run-grust.sh

benchmarks/lsqb/fetch-dataset.sh --scale 0.3
CELL_TIMEOUT_MS=3600000 WARMUPS=0 RUNS=1 QUERY_TIMEOUT_MS=1200000 \
  WORKER_READY_TIMEOUT_MS=1200000 QUERY_REAP_GRACE_MS=1000 \
  QUERY_KILL_REAP_TIMEOUT_MS=5000 QUERY_RECOVERY_TIMEOUT_MS=10000 SF=0.3 \
  benchmarks/lsqb/run-grust.sh
```

These are deliberately fail-fast, one-sample qualification runs with a
one-hour budget per named backend/suite cell; they are not worst-case
completion guarantees. If enough queries consume their ceilings, the cell
watchdog stops the incomplete qualification and its output is not publishable.
Choose any larger full-run cap explicitly only after reviewing the arithmetic:
at SF0.1, the adversarial W0/R1 query-ceiling total is 13 × 10 minutes = 2
hours 10 minutes, while W2/R5 is 13 × 7 × 10 minutes = 15 hours 10 minutes,
before worker setup and recovery. Steady start/ready/finish events make actual
progress visible throughout a successful run.

These ten- and twenty-minute values are per-query ceilings, not expected
latencies. They leave headroom for the admitted in-process reference work under
the fixed 8-CPU, 6-GiB container envelope: a one-sample qualification measured
SF0.1 q2 at about 54 seconds and SF0.3 q2 at about 311 seconds, while the
adversarial reordered-join case has a larger certified intermediate. Keep the
ceiling identical across cells in a published scale run; compare the recorded
elapsed samples, not the timeout budget.

At downloaded scales the matrix executes Memory as the in-process reference,
Turso/PostgreSQL (and a qualified Sail service) as backend-row-source plus Rust
projection, and FalkorDB as backend-native aggregate. Whole-store
materialization bridges remain explicit `unsupported` cells, and no summary
ranks unlike execution classes against each other.

To inspect or run one cell manually, build its feature-specific image and use
the same read-only mounts:

```sh
export GRUST_SOURCE_REVISION="$(git rev-parse HEAD)"
export BENCHMARK_FEATURE=lancedb
export BENCHMARK_IMAGE_TAG=grust-lsqb-matrix-lancedb:0.13
export BENCHMARK_EXECUTION_IMAGE="$BENCHMARK_IMAGE_TAG"
docker compose -f benchmarks/lsqb/compose.yaml build benchmark
export BENCHMARK_IMAGE_ID="$(docker image inspect --format '{{.Id}}' "$BENCHMARK_IMAGE_TAG")"
export BENCHMARK_EXECUTION_IMAGE="$BENCHMARK_IMAGE_ID"
docker compose -f benchmarks/lsqb/compose.yaml run --rm --no-deps benchmark \
  --backend lancedb --suite baseline --scale example \
  --warmups 2 --runs 5 --query-timeout-ms 30000 \
  --worker-ready-timeout-ms 1200000 --query-reap-grace-ms 1000 \
  --query-kill-reap-timeout-ms 5000 --query-recovery-timeout-ms 10000 \
  --cell-timeout-ms 3600000 \
  --output /out/baseline-lancedb-sfexample.json
```

This direct one-cell command is diagnostic only: the declared cell timeout is
not a watchdog unless the command is supervised by `cell-watchdog.py`, and it
does not produce the clean-worktree publication receipt emitted and validated
by `run-grust.sh`.

The benchmark container root is read-only, `/tmp` is a fresh tmpfs, and the
output mount is writable. For downloaded scales, the orchestrator first copies
only authenticated CSVs and their receipt into a private, read-only snapshot;
that snapshot is mounted at `/datasets` read-only and re-authenticated before
and after every benchmark container. Downloaded scales use a pinned in-image
query/oracle tree plus that snapshot. Before starting containers, the
orchestrator recomputes
the extracted CSV manifest and requires the known fingerprint derived from the
verified official SF0.1 or SF0.3 archive. It also checks that same fingerprint
in every emitted component before merge, so selecting a scale cannot by itself
assert archive provenance. Every benchmark or service container has an enforced
default limit of 8 CPUs and 6,442,450,944 bytes (6 GiB), and the runner refuses
to start if the Docker VM exposes less. Override both runs consistently with
`BENCHMARK_CPU_LIMIT` and `BENCHMARK_MEMORY_LIMIT_BYTES`; the exact enforced
values are recorded in every report with `resource_limit_scope=per-container`.
Backend identity records the components actually started for that cell, so a
two-container execution is not presented as sharing a single cap and an
unconfigured-service `unavailable` cell records only its runner rather than an
imaginary service process.
Scratch data and raw output live under
ignored `data/`, `upstream/`, and `out/` directories. The orchestrator refuses
to overwrite an image manifest or component/matrix report; choose a new
`OUTPUT_DIR` for another run. Only bounded result records selected for review
belong in `results/`.

## Images

The Dockerfiles and Compose file pin multi-platform image indexes by digest:

| Purpose | Pinned image |
|---|---|
| Unchanged upstream run | `python:3.12.11-slim-bookworm@sha256:519591d6871b7bc437060736b9f7456b8731f1499a57e22e6c285135ae657bf7` |
| Grust builder | `rust:1.97.1-trixie@sha256:b1b3c9c0d921d7fa0a6d1f9ec7e4eab87f8c8ec97644c3d791450f131dec813f` |
| Grust runtime | `debian:trixie-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132` |
| PostgreSQL | `postgres:18.6-bookworm@sha256:1c59e2c3c818eaa0f0628f695b36e7c9e362d6b219b36a54a32df645cbd7e1af` |
| FalkorDB | `falkordb/falkordb:v4.20.4@sha256:adbddd418916c25618564ff8597a919b08bc76452ebeb74eb985c38d7281df62` |
| SurrealDB | `surrealdb/surrealdb:v3.2.4@sha256:51baed8709f57f67dcf04b30e3177db846803fa9342dae2be58c6fa5f8d59843` |
| pgGraph | `ghcr.io/evokoa/pggraph:1.2.0@sha256:5a69355fbac9f62222c072f3882ba0de7690d45d710e273a0515937e349b5873` |

Those are registry index digests. The orchestrator chooses a pinned amd64 or
arm64 platform manifest and records its separate image config digest in each
backend identity and `images.tsv`; it never treats a config digest as a
pullable manifest. Before starting a configured service it resolves that
platform manifest through the registry and requires the expected config digest.
For external services, the receipt-bound pre/post attestations additionally
record the platform-manifest digest and Docker's observed runtime image ID so a
legacy config-ID image store and a containerd manifest-ID image store remain
distinguishable without changing the declared config identity.
The manifest also records every locally built runner image ID. The policy
report records its control-runner tag and immutable `sha256:` image ID, the
`per-container` resource-limit scope, and the concrete host CPU model supplied
to the container. Reports include the exact Git revision. `run-grust.sh`
automatically suffixes a dirty worktree revision with `-dirty`; strict evidence
validation accepts only a clean committed revision.

LanceDB's generated bindings require build-only protobuf tooling. The builder
pins `libprotobuf32t64`, `libprotobuf-dev`, `libprotoc32t64`, and
`protobuf-compiler` to Debian version `3.21.12-11+deb13u1` and verifies
`libprotoc 3.21.12`; these packages are not copied into the runtime image.
Reqwest 0.13 initializes its platform verifier even for the matrix's plain-HTTP
service endpoints. The slim runtime therefore receives the `ca-certificates`
20250419 bundle from the digest-pinned builder image, verifies bundle SHA-256
`714d457d580922dbf1d0be8bd35ba236a842b50b0072ae791582a19adef772a5`,
and sets `SSL_CERT_FILE`; it does not install mutable runtime packages.

## What is measured

The upstream CSV records system/version, thread count, scale, query number,
wall-clock query seconds, and result count. The upstream convention is five
repetitions. Its expected example counts are:

```text
q1 q2 q3 q4 q5 q6 q7 q8 q9
 8  3  6  8  3  8 11  2  4
```

Schema-v3 JSON records initial graph load nanoseconds separately, then keeps
every warm-up and measured observation with its rotating query position,
expected/actual count, outcome, source/adapter digest, execution class,
unmeasured worker `setup_ns`, process `termination`, and post-result/deadline
`recovery_ns`. New observations also include the worker-selected `plan`; old
records without it remain unknown rather than being backfilled. See
[observation plan identity](EXECUTION-PLANS.md) for the labels and comparison
rules. Each backend cell also declares whether workers attach to state
loaded once or reload before every observation, plus the recovery proof that a
forced timeout requires. The default protocol is two warm-ups and five
measured iterations. The measured boundary is the coordinator's monotonic GO
write through consumption of the scalar result; worker startup, READY setup,
reaping/recovery, image and service startup, graph load, and report
serialization are outside `elapsed_ns` and separately disclosed where
applicable. For a forced deadline, `query_timeout_ms` is the configured cutoff
and `elapsed_ns` is the coordinator's actual monotonic observation of that
deadline, including any scheduler wake-up overshoot. TERM/KILL/reap and
backend quiescence begin only afterwards and are recorded in `recovery_ns`;
the report never substitutes the configured cutoff for an observed duration.
Unlike schema v2's direct in-process API interval, schema v3 necessarily
includes coordinator-to-worker GO delivery plus result serialization and pipe
delivery. V2 and v3 latency samples therefore must never be pooled or treated
as the same timing boundary. The wire contract makes that distinction explicit:
v2 uses `submit-to-scalar-consumed`, while v3 uses
`coordinator-go-to-result-consumed`.

`query_timeout_ms` is a hard coordinator deadline. Each observation runs in a
fresh OS process group. The worker completes setup, emits one bounded,
token-bound READY record on a private pipe, and cannot submit the query until
the coordinator sends GO. At the deadline the coordinator sends SIGTERM to the
whole group, waits `query_reap_grace_ms`, escalates to SIGKILL, and requires the
group to disappear within `query_kill_reap_timeout_ms`. It then performs the
cell's backend-specific quiescence proof within
`query_recovery_timeout_ms`; a backend without cancellation or introspection
fails the cell after forced termination rather than starting a possibly
overlapping sample. An unacknowledged transport/query error follows the same
rule: process-owned work is gone, PostgreSQL sessions are probed, and other
remote services fail closed. READY itself is bounded by
`worker_ready_timeout_ms`, so a stalled load/connect is a prompt
infrastructure failure rather than a query timeout. Malformed, duplicate,
late, wrong-token, or oversized worker records are fatal protocol errors. Only
the coordinator writes the final report.

The coordinator's local containment proof covers the dedicated worker process
group and closure of its inherited control pipes. Benchmark workers are trusted
runner code and must not create a new session or process group; this is not a
claim that the coordinator can discover an arbitrary hostile descendant that
re-sessions and closes every inherited pipe. The exact container watchdog is
the full-tree outer safety boundary. A descendant that escapes while retaining
a pipe makes the cell fail because pipe closure cannot be proven.

Process exit proves recovery for Memory, in-memory Turso, Ladybug, and local
LanceDB, which therefore reload inside each observation worker. Persistent
PostgreSQL-family services load once; workers attach with a unique
`application_name` and server `statement_timeout`, and forced recovery polls
`pg_stat_activity` until those sessions disappear. FalkorDB's native TIMEOUT
reserves ten percent of the coordinator cutoff, capped at five seconds, for its
timeout acknowledgement and fresh connection; successful queries still use
the common coordinator deadline. An unacknowledged Falkor forced kill fails
closed. Sail, SurrealDB, and Helix likewise fail closed after forced
termination until their adapters expose an acknowledged server-side interrupt
or quiescence probe.

`run-grust.sh` still requires a positive `CELL_TIMEOUT_MS`, records it as
`timing.cell_timeout_ms`, gives every Compose run container a unique name, and
supervises that exact name under a last-resort wall-clock watchdog. A single
query no longer waits for this cell deadline. The watchdog
verifies the container's Compose project/service labels before killing by its
immutable ID. Successful run containers are retained long enough for that
identity to be observed, then removed by the same exact-ID check instead of an
auto-remove race. The watchdog writes the observed identity to an exclusively
created completion record on controlled success, timeout and error paths. That receipt-bound record
also fixes the configured timeout, elapsed wall time, child exit status, and
terminal state. If the watchdog fires—or cannot prove the container
identity—the run stops without a publication receipt. The bound covers the
entire Compose run, including container creation/start, dataset load, all
warm-up and measurement work, and report serialization. The downloaded
examples above intentionally use a one-hour fail-fast qualification budget;
they do not silently turn that budget into a per-query wait or claim it can
contain every configured worst-case timeout. Full-run caps remain explicit
and configurable, and the cost arithmetic must be reviewed before launch.

SIGINT/SIGTERM are latched through child creation and cleanup. Cancellation
first stops/reaps the owned CLI group, then kills/removes only the observed
immutable container ID after revalidating its name and Compose labels. Cleanup
failures are errors, never successful timeouts. If no identity was observed,
one late lookup follows CLI termination; this cannot exclude daemon creation
after that lookup. New diagnostic flows should create/attest a stopped container
before starting work. Unexpected exceptions attempt cleanup but may lack a
completion record; missing records fail publication validation. When nesting
this watchdog under `command-progress.py`, use `--termination-grace-seconds 60`
to allow bounded Docker cleanup before escalation. This is cancellation grace,
not a guessed runtime limit. SIGKILL, an unavailable daemon or blocked record
I/O cannot guarantee cleanup or durable completion.

The supervisor emits a secret-safe stderr heartbeat every 30 seconds with only
the constrained expected container name and elapsed/remaining wall-clock
milliseconds. The matrix runner also emits `grust-lsqb-progress` JSONL start,
ready, and finish events for each executed query, including its authenticated
backend/suite/scale/query IDs, protocol position, setup completion, terminal
outcome, and measured nanoseconds. It never logs
query text, counts, errors, paths, endpoints, or environment values. Both
line types are assembled to at most 512 bytes and issued with one write at
their producer boundary. That is atomic on a normal blocking POSIX pipe;
Docker/Compose may reframe output downstream, so no end-to-end atomicity is
claimed. Heartbeat delivery allows only one queued-or-writing line and drops
new lines under backpressure so output cannot delay hard-timeout enforcement.
It never retries a short write from an exotic sink. Query events are explicitly
flushed outside the timed query boundary. Neither line type alters the
receipt-bound watchdog completion schema.

Each completed observation additionally emits an `observation-recorded` JSON
line to stdout, captured incrementally in the host-side cell log. Unlike the
bounded progress telemetry, this includes the full scalar count, setup/query/
recovery timings, outcome, termination mechanism, and the selected `plan`.
The record is flushed
outside query timing before starting another observation; a write failure fails
the cell. These partial records are diagnostic, not a completion receipt, and
must not be pooled or published as a completed comparison. A machine power loss
can still lose buffered filesystem writes; a Docker process failure does not
erase records already captured by the host logger.

`load_ns` is diagnostic and is not compared across adapter classes.
Sail's benchmark configuration uses 10,000-row write batches (the adapter
default is 1,000), reducing small Delta transactions during bulk loading.
Projected-chunk coordinator loads emit `load_chunk_complete` telemetry after
each successful chunk, with cumulative nodes, edges, chunks, and elapsed time.
Worker stderr remains private; these counters describe coordinator loading,
not worker setup or query completion. They do not constitute a receipt.

At downloaded scales Memory decodes bounded node-first/edge-next chunks directly
into its single owned in-process graph. Turso, PostgreSQL, a qualified Sail
service, and FalkorDB decode and insert the same bounded chunks inside their
load intervals; none of these query phases retains a duplicate Rust source
graph. Example-scale and example-only materializing adapters receive an already
decoded graph. Dataset inspection and manifest hashing occur before either
boundary. Query latencies exclude all of those load and verification steps in
both cases. If a downloaded-scale portable query
cannot use its backend row-source plan, that query is explicitly unsupported;
it cannot fall back to timed backend materialization plus the Rust reference.

The bundled evidence manifest also fixes a 1,000,000-row admission ceiling for
downloaded-scale execution that would materialize logical rows in Rust. It
records plan-specific cardinality evidence for the in-process reference and
backend row source because their intermediate row counts can differ sharply:
SF0.1 q3 reaches 32,030,444 logical rows in the clause-by-clause Memory plan
but sends 30,456 final matches through a qualifying SQL row source. Only a
canonical exact cardinality or upper bound at or below the ceiling is admitted.
Larger bounds use `performance.rust-row-limit`; an insufficient lower bound
uses `performance.rust-row-bound-unavailable`; both are `unsupported` without
observations. FalkorDB's backend-native scalar aggregate remains admitted.
This prevents an explosive count such as the 4.913-billion-row SF0.1 Cartesian
attack from exhausting the runner before its cooperative timeout can quiesce.
It is a capability and safety boundary, not a zero-time performance result.

Proven non-materializing counts use a separate, hash-bound
[execution-plan registry](EXECUTION-PLANS.md). All 22 pinned Memory cases now
select `count-factorized`, with `rust_rows.kind = "not-materialized"`; an empty
materialized result does not qualify. Index and query work still consume
resources. Turso and PostgreSQL admit four scalar SQL cases each. These are
implementation classifications, not qualified larger-scale performance results.
The separate [Memory profiler](examples/profile_memory/README.md) is a load-once,
oracle-checked developer diagnostic, never a publication comparison.

`backend-native-aggregate` means the backend computed the scalar itself.
`backend-row-source-rust-projection` means SQL or Spark supplied rows and Grust
completed the disclosed projection. `backend-materialize-rust-reference`
includes backend reads for every source label and edge shape, source node/edge
multiset validation, and the shared Rust query execution in each timed sample.
It detects missing, changed, duplicated, or additional records within those
source shapes; it does not claim to enumerate unrelated backend-only labels.
`in-process-reference` is the Memory control. These classes are not
performance-equivalent and summaries must not collapse them. Unsupported,
unavailable, and not-applicable cells have no timing samples. The 28-node,
72-edge example graph is useful for conformance and orchestration checks, not
backend-winner claims.

## Compatibility adapter boundary

The original LSQB files are never edited. The runner retains and hashes their
exact bytes, then makes these explicit executable-model adaptations:

- LSQB's `Post` and `Comment` node types become Grust's single `Message` label
  plus a `kind` property. This represents LSQB's `Message` supertype in Grust's
  one-primary-label model.
- q8 and q9 abbreviated openCypher `NOT (a)-[:TYPE]->(b)` pattern predicates
  become equivalent `OPTIONAL MATCH` plus `IS NULL` anti-joins, which the Grust
  portable reader supports.
- Source IDs are prefixed by LSQB type while preserving the numeric source ID
  as a property, preventing otherwise ambiguous IDs across CSV domains.

For those reasons, the Grust baseline is a **compatibility run derived from
LSQB**, not the unchanged upstream run. Both the source and adapted query
digests appear in every JSON record.

## adversari.al extension

The extension is not part of LSQB. It uses the same graph only after the
upstream reference and compatibility checks succeed. Its 27 attacks have two
non-overlapping expectation models: 13 exact counts and 14 required policy
rejections. Each storage-backend cell therefore has 22 count oracles (nine
LSQB-derived plus 13 adversarial); the 14 policy attacks are one separate,
backend-neutral rejection track.

| Attack | Boundary exercised | `sfexample` expected count |
|---|---|---:|
| `a1-reversed-chain` | Entire q1 chain written in reverse | 8 |
| `a2-reordered-join` | q2 atoms reordered around shared variables | 3 |
| `a3-split-match` | q4 decomposed across three `MATCH` clauses | 8 |
| `a4-optional-fanout` | Optional fanout across a `WITH` boundary | 11 |
| `a5-negated-pattern` | Anti-join predicates reordered | 2 |
| `a6-range-expansion` | Bounded `range`/`UNWIND` amplification | 10,000 |
| `a7-cartesian-count` | Three-way Cartesian cardinality | 125 |
| `a8-union-dedup` | Deduplication of identical aggregate rows | 5 |
| `a9-path-zero-hop` | Zero-hop bounded-path identity over `Person` | 5 |
| `a10-unicode-literal` | Unicode literal/escape equivalence and result identifier | 5 |
| `a11-schema-null-probe` | Quoted missing-property GQL null semantics | 5 |
| `a12-parser-comment-trivia` | Comment-delimited tokens and nested projection parentheses | 28 |
| `a13-resource-edge-scan` | Full directed-edge aggregate cardinality | 72 |

Every attack has one deterministic count oracle. A query error, missing result,
wrong type, or wrong count fails that backend cell.

The bounded-policy track runs once through Grust's portable parser and
cooperative read executor; it is not repeated as if it were a storage-backend
performance score. Each case records the expected and actual stable rejection
category, error text, source hash, elapsed time, and pass/fail status.
It is deliberately fixed to the small, pinned `sfexample` graph: it validates
backend-neutral parser and resource-policy rejection semantics, not storage
performance. `run-grust.sh` therefore omits this track for SF0.1 and SF0.3
instead of materializing those downloaded datasets in the legacy policy runner
or publishing a misleading scale-qualified policy result.

| Attack | Required rejection category |
|---|---|
| `p1-unbounded-path` | `syntax.unbounded-path` |
| `p2-range-bomb` (`range(1, 10001)`) | `execution.range-limit` |
| `p3-cartesian-work` | `execution.candidate-work` |
| `p4-updating-smuggle` | `syntax.updating-clause` |
| `p5-forbidden-procedure` | `syntax.forbidden-procedure` |
| `p6-union-arms` (five arms) | `syntax.union-arms` |
| `p7-intermediate-projection` | `execution.intermediate-bytes` |
| `p8-correlated-replan` | `execution.candidate-work` |
| `p9-catalog-rescan` | `execution.candidate-work` |
| `p10-resource-query-bytes` | `syntax.query-bytes` |
| `p11-path-hop-limit` | `syntax.path-limit` |
| `p12-unicode-invalid-scalar` | `syntax.invalid-unicode-scalar` |
| `p13-schema-graph-selection` | `syntax.graph-selection` |
| `p14-parser-unterminated-comment` | `syntax.unterminated-comment` |

Policy report schema 2 serializes the complete effective base
`ReadQueryPolicy`, not a hand-picked limit subset. The pinned policy allows at
most 2,000 query bytes, 64 KiB of parameters, 100,000 graph nodes, 500,000
graph edges, 64 MiB of encoded graph data, 10,000 candidate-work units, 256 MiB
of cumulative intermediate materialization, 50 result rows, 1 MiB of output,
10,000 range items, four UNION arms, four cumulative path hops, and 2,000 ms of
cooperative execution. Graph selection and catalog procedures are disabled,
and a `MATCH` is required. Each attack also records an exact override object:
`p7-intermediate-projection` uses a disclosed 48 KiB parameter and raises only
its candidate-work ceiling to 50,000 so the byte-budget boundary is reached
first; `p9-catalog-rescan` enables catalog procedures so its candidate-work
guard can be tested. Every other override object is empty. These negative
cases never contribute to the LSQB count table.

## Backend scope

The schema-v3 matrix is rectangular across Memory, Turso, PostgreSQL, Ladybug,
FalkorDB, SurrealDB, LanceDB, Sail, pgGraph, PostgreSQL PGQ, Helix, and
CocoIndex. It does not pretend all twelve have the same capability: native,
row-source-plus-Rust, materialize-plus-reference, unavailable, unsupported,
and export-only outcomes stay distinct. The unchanged upstream Ladybug run is
separate from Grust's Ladybug adapter cell. The policy track remains a
backend-neutral `portable-policy` check and is never counted as twelve storage
backend measurements. See [`BACKENDS.md`](BACKENDS.md) for exact qualification
and service gaps.

A separate [native Neo4j lane](NEO4J.md) is under implementation using the
Neo4j Labs Rust driver. It is not yet a completed benchmark or an additional
Grust adapter cell; existing twelve-backend receipts remain unchanged.

See [`results/2026-09-03`](results/2026-09-03) for the historical schema-v1,
three-backend bounded evidence. The receipt-backed schema-v3 evidence and its
canonical presentation belong at
[adversari.al/graph](https://adversari.al/graph).
