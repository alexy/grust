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

The other current selectors are `sql-row-source` and `backend-native`. Plan
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
