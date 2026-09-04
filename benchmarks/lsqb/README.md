# Graph benchmark harness

This directory keeps two deliberately separate tracks:

1. **Upstream reference run.** The pinned Graph Data Council (GDC) repository's
   LSQB Ladybug scripts and example dataset run unchanged in a container.
2. **Grust compatibility and adversarial runs.** Grust imports the same pinned
   projected-foreign-key data, checks the original LSQB query bytes, applies a
   documented compatibility adapter, and then runs the nine LSQB query shapes
   plus 17 clearly separate adversari.al extensions: eight per-backend count
   attacks and nine backend-neutral bounded-policy rejection attacks.

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
| `cypher/` tree | `50937f3d075245e2abd4c00a36c4b3c236766265` |
| Example projected-FK data tree | `45181e6b274d014f8626038e1d398fa1b9e4c19d` |
| Expected-output blob | `4d9dedb2f8c7a42af6defa327303b1aded39e3ad` |
| LSQB source license | Apache-2.0; see upstream `LICENSE` and `NOTICE` |
| Upstream system | Ladybug 0.19.0, as pinned by `ladybug/vars.sh` |

The upstream image build verifies the exact commit and full repository tree,
which transitively fixes the recorded query and example-data subtrees. The
compatibility runner also verifies a SHA-256 digest for every q1–q9 source file
before executing its separate track.

LSQB publishes projected-FK and merged-FK datasets at `example`, `0.1`, `0.3`,
`1`, `3`, `10`, `30`, `100`, `300`, and `1000`. This harness checks in no
downloaded dataset. The pinned example projected-FK dataset is already in the
upstream repository and is the only dataset used by the recorded evidence.
Follow LSQB's own download instructions for larger scales.

## Exact Docker commands

Run the pristine upstream reference first:

```sh
RUNS=5 SF=example benchmarks/lsqb/run-upstream.sh
```

`Dockerfile.upstream` clones the exact commit and installs the exact Ladybug
version requested upstream. For each repetition, the wrapper copies the
pristine checkout and invokes the upstream scripts without modifying them:

```sh
cd ladybug/..
SF=example ./ladybug/init-and-load.sh
SF=example ./ladybug/run.sh
SF=example ./ladybug/stop.sh
```

The fresh copy matters because this upstream revision's loader cannot safely
reuse the same Ladybug database path for a second initialization.

After the upstream run, execute the separate Grust matrix:

```sh
RUNS=5 SF=example benchmarks/lsqb/run-grust.sh
```

That builds one runner image, starts the digest-pinned PostgreSQL service, and
runs both tracks over Grust Memory, Turso, and PostgreSQL. To run one cell:

```sh
export GRUST_SOURCE_REVISION="$(git rev-parse HEAD)"
docker compose -f benchmarks/lsqb/compose.yaml build benchmark
docker compose -f benchmarks/lsqb/compose.yaml up -d --wait postgres
docker compose -f benchmarks/lsqb/compose.yaml run --rm benchmark \
  --backend memory --suite baseline --scale example --runs 5 \
  --output /out/grust/baseline-memory-sfexample.json
docker compose -f benchmarks/lsqb/compose.yaml down --volumes
```

Scratch data and raw output live under ignored `data/`, `upstream/`, and `out/`
directories. Only bounded result records selected for review belong in
`results/`.

## Images

The Dockerfiles and Compose file pin multi-platform image indexes by digest:

| Purpose | Pinned image |
|---|---|
| Unchanged upstream run | `python:3.12.11-slim-bookworm@sha256:519591d6871b7bc437060736b9f7456b8731f1499a57e22e6c285135ae657bf7` |
| Grust builder | `rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97` |
| Grust runtime | `debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171` |
| PostgreSQL | `postgres:18.6-bookworm@sha256:1c59e2c3c818eaa0f0628f695b36e7c9e362d6b219b36a54a32df645cbd7e1af` |

Runner output also includes the locally built image ID and exact Git revision.
`run-grust.sh` automatically suffixes a dirty worktree revision with `-dirty`;
only a clean committed revision is eligible for checked-in Grust evidence.

## What is measured

The upstream CSV records system/version, thread count, scale, query number,
wall-clock query seconds, and result count. The upstream convention is five
repetitions. Its expected example counts are:

```text
q1 q2 q3 q4 q5 q6 q7 q8 q9
 8  3  6  8  3  8 11  2  4
```

The Grust JSON records graph load nanoseconds separately from query wall-clock
nanoseconds, plus expected/actual counts, status, original source digest,
executed-adapter digest, and the execution mode for every query and repetition.
`sql-row-source-pushdown+rust-projection` means Turso or PostgreSQL executed the
supported row-source plan in SQL and Grust completed projection using the
shared Rust semantics. `in-memory-reference-fallback` means the backend first
materialized its stored graph and the reference executor handled that query.
Memory is always recorded as `in-memory-reference`. Thus a matching count is an
end-to-end compatibility result; it is not, by itself, a claim of fully native
database execution. Report summaries use the median and full min–max range
across five clean loads. No warm-up samples are removed.

Elapsed times include in-process query dispatch but exclude image build,
container startup, CSV parsing, and report serialization. They are useful for
reproducibility and regression detection on the same environment. The 28-node,
72-edge example graph is too small for backend-winner claims.

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
upstream reference and compatibility checks succeed. Its 17 attacks have two
non-overlapping expectation models: eight exact counts and nine required policy
rejections. Each storage-backend cell therefore has 17 count oracles (nine
LSQB-derived plus eight adversarial); the nine policy attacks are one separate,
backend-neutral rejection track.

| Attack | Boundary exercised | Expected count |
|---|---|---:|
| `a1-reversed-chain` | Entire q1 chain written in reverse | 8 |
| `a2-reordered-join` | q2 atoms reordered around shared variables | 3 |
| `a3-split-match` | q4 decomposed across three `MATCH` clauses | 8 |
| `a4-optional-fanout` | Optional fanout across a `WITH` boundary | 11 |
| `a5-negated-pattern` | Anti-join predicates reordered | 2 |
| `a6-range-expansion` | Bounded `range`/`UNWIND` amplification | 10,000 |
| `a7-cartesian-count` | Three-way Cartesian cardinality | 125 |
| `a8-union-dedup` | Deduplication of identical aggregate rows | 5 |

Every attack has one deterministic count oracle. A query error, missing result,
wrong type, or wrong count fails that backend cell.

The bounded-policy track runs once through Grust's portable parser and
cooperative read executor; it is not repeated as if it were a storage-backend
performance score. Each case records the expected and actual stable rejection
category, error text, source hash, elapsed time, and pass/fail status.

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

The recorded policy inherits the Grust defaults of at most 256 MiB of
cumulative intermediate materialization, 10,000 range items, four UNION arms,
and four cumulative path hops. It deliberately tightens the candidate-work
budget to 10,000 units so work-amplification rejections are fast and
deterministic even under a loaded CI host. The intermediate-projection case
uses a disclosed 48 KiB parameter and raises only that case's candidate-work
ceiling to 50,000 so its expected byte-budget boundary is reached first. All
accepted queries must end in a
positive literal `LIMIT` within the result-row ceiling. These negative cases
never contribute to the LSQB count table.

## Backend scope

The Grust Docker matrix covers the three backends that currently share the
portable read-query executor needed for all seventeen count oracles: Memory,
Turso, and PostgreSQL. The unchanged upstream cell covers Ladybug through
LSQB's own implementation. The policy track uses the backend-neutral
`portable-policy` label. Other Grust storage adapters remain outside this query
matrix until they expose the same aggregate/openCypher surface; omission is not
reported as a pass or failure.

See [`results/2026-09-03`](results/2026-09-03) for the bounded evidence and the
canonical public presentation at [adversari.al/graph](https://adversari.al/graph).
