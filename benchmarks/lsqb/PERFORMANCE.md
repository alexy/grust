# Performance evidence

The matched comparison cohort uses two warm-ups and ten measured executions per
query, rotating query order within each round. The query deadline is 60 seconds.
Example-scale timings measure a tiny correctness fixture; larger LSQB scales are
required for meaningful workload-scaling conclusions.

After a matrix finishes and its publication receipt verifies, run:

```sh
python3 benchmarks/lsqb/summarize-performance.py /path/to/completed-matrix
```

The JSON summary is bound to the verified receipt digest. It contains measured
raw nanoseconds and minimum, median, and maximum for query execution, worker
setup, recovery, and their per-sample sum. Coordinator loading is recorded
separately as one duration, not a statistical distribution. The sum covers the
recorded boundaries, not all orchestration overhead.

Warm-ups are excluded from timing distributions. Any failed warm-up or measured
execution suppresses that query's timing statistics; failures and raw observations
remain visible. Unsupported and unavailable cases have no fabricated timings.

New canonical manifests opt into the required `grust-host-preflight-v1` startup
screen. Receipts bind `host-preflight.json` in both the artifact digests and full
inventory, and validate its three passing samples on creation and verification.
Historical manifests without the marker retain their exact original layout;
absence is unknown, never a presumed pass. The summary exposes this distinction
as `host_screen`. A startup screen can precede builds and does **not** establish
isolation during measurements. Per-query `performance_eligible` describes a
successful fixed-plan cohort, not clean-host qualification. Separate native/SDK
bundle contracts are not changed by this canonical matrix contract.

Each row retains execution class, backend identity, lifecycle, and resource limits.
Native aggregates, backend row production with Rust projection, and whole-graph
materialization followed by Rust evaluation must be disclosed distinctly. A
Grust adapter measurement is not necessarily the database's native query speed.

New matrix observations also declare their worker-selected [execution
plan](EXECUTION-PLANS.md). Summaries expose missing historical plans as `null`
(legacy, unknown), never as an inferred reference or native plan. A mixture of
plans across warm-ups and measurements suppresses pooled timing statistics.
Per-plan subsets retain the declared cohort size and disclose missing samples;
filtering cannot manufacture a complete, performance-eligible cohort.

To select one plan explicitly, without changing the source evidence:

```sh
python3 benchmarks/lsqb/summarize-performance.py /path/to/completed-matrix --plan clause-pipeline
python3 benchmarks/lsqb/summarize-performance.py /path/to/legacy-matrix --plan legacy
```

The other current selectors are `count-factorized`, `sql-row-source`,
`sql-count` and `backend-native`. Optimized plans also require the manifest's
hash-bound execution-plan registry; a filter cannot waive admission. Plan
identity supplements execution class; it does not make two differently timed,
configured, or implemented backend paths directly comparable. Preserve old
receipts and bundles exactly as recorded—do not add plan fields retroactively.

Native Neo4j uses its separately audited bundle and timing summaries described in
[NEO4J.md](NEO4J.md). Its query boundary includes scalar consumption and transaction
rollback. Matching repetition counts alone is insufficient: dataset, query order,
resource limits, semantics, and lifecycle differences must accompany comparisons.

These sequential latency measurements do not establish concurrent throughput.
Do not present reciprocal latency as measured queries per second under load.
These are not LDBC Benchmark Results.

For implementation profiling, the separate
[Memory diagnostic](examples/profile_memory/README.md) loads one immutable index
and checks pinned counts on every iteration. Its flushed records are explicitly
non-publication diagnostics: they do not use the matrix lifecycle or establish
a ranking against Neo4j or another backend.

The ignored [support-kernel diagnostic](../../crates/grust-cypher/src/read/count_support_profile.rs)
checks isolated, path, star and clique fixtures, with five orientation/traversal
measurements per fixture and analytical triangle counts. It isolates internal
support work, not parsing, Memory loading, q3 location products or q9 role
arithmetic. Its JSON records are explicitly non-publication and emitted after
each iteration. From the repository root, using a fresh output directory:

```sh
python3 benchmarks/lsqb/command-progress.py \
  --output /tmp/grust-support-profile-01 -- \
  cargo test --locked --release --jobs 2 -p grust-cypher --lib \
  read::count_support::profile::profile_sparse_and_dense_orientation \
  -- --exact --ignored --nocapture --test-threads=1
```

This is a finite test with one shared 120-second **cooperative** read deadline,
not a hard process/output deadline or a watchdog. Budgets remain active during
kernel measurements; setup and progress I/O are outside the reported intervals.
Do not mix these records with LSQB observations or use them as a backend ranking.
