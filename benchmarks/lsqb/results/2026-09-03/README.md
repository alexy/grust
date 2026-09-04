# Graph benchmark evidence — 2026-09-03

This directory contains bounded, reviewable evidence for the graph benchmark
harness. Its name is the methodology version; the recorded upstream and Grust
runs completed on 2026-09-04 UTC. The Grust matrix was captured from clean,
pushed source revision `2680c451db450cefe2411a944cce9960507bc0d3`.

> **These are not LDBC Benchmark Results.** LSQB is a GDC-maintained
> labelled-subgraph-query microbenchmark, not an official LDBC benchmark. This
> run is unaudited. The tiny `sfexample` graph is conformance and
> reproducibility evidence, not a general performance ranking.

## Recorded upstream reference

Command:

```sh
RUNS=5 SF=example benchmarks/lsqb/run-upstream.sh
```

The command ran the pinned LSQB checkout's own Ladybug scripts unchanged in
five fresh container work directories. All 45 observations matched the nine
upstream count oracles. See:

- [`upstream-summary.json`](upstream-summary.json) for provenance, environment,
  artifact hashes, counts, medians, and full ranges;
- [`upstream-ladybug-sfexample.tsv`](upstream-ladybug-sfexample.tsv) for every
  raw observation in a single small table;
- [`manifest.json`](manifest.json) for source, package, container, PostgreSQL,
  and Grust evidence identity.

The run host was macOS 26.2 (25C56) on arm64. It used Docker Engine 29.4.3 on
`linux/arm64`, with 10 Docker-reported CPUs and 8,321,712,128 bytes of memory.
The host CPU model is intentionally omitted; Docker-reported architecture and
resource allocation describe the execution boundary used here.

The five original upstream CSV files remain ignored scratch output under
`benchmarks/lsqb/out/upstream/`. Their SHA-256 digests are retained in the JSON
summary. The checked-in TSV losslessly normalizes those six upstream fields by
splitting the `10 threads` value into a numeric `threads` column, then adds a
run number and header.

## Recorded Grust matrix

Command:

```sh
RUNS=5 SF=example benchmarks/lsqb/run-grust.sh
```

The command built image
`sha256:45ca904ba0e68772e89db4410e8165b77111322ff3b5df03c8025fe421f6ba7c`,
started the digest-pinned PostgreSQL 18.6 service, and clean-loaded the same
28-node, 72-edge graph for every repetition. All 135 LSQB-derived compatibility
observations and all 120 adversarial count observations matched their oracles.
The separate portable-policy track matched all nine expected rejection
categories.

| Track | Backend | Result | Median load | Median query-set wall time (range) |
|---|---|---:|---:|---:|
| LSQB-derived compatibility | Memory | 45 / 45 | 0.063 ms | 3.197 ms (3.029–3.746) |
| LSQB-derived compatibility | Turso | 45 / 45 | 2.279 ms | 24.927 ms (24.524–25.838) |
| LSQB-derived compatibility | PostgreSQL | 45 / 45 | 5.970 ms | 21.791 ms (20.956–30.050) |
| adversari.al counts | Memory | 40 / 40 | 0.061 ms | 6.038 ms (5.204–9.286) |
| adversari.al counts | Turso | 40 / 40 | 2.312 ms | 8.291 ms (7.918–12.436) |
| adversari.al counts | PostgreSQL | 40 / 40 | 6.199 ms | 16.213 ms (15.836–21.563) |
| adversari.al policy | portable policy | 9 / 9 | n/a | 101.170 ms total |

The query-set figure sums the query wall times within one fresh-load
repetition, then reports the median and full five-run range. It excludes image
build, container startup, data parsing, and report serialization. The policy
track runs once because it is a backend-neutral safety check rather than a
storage-backend score.

The seven JSON reports retain every observation, source and adapter digest,
execution mode, exact source revision, image identity, and Docker-reported
environment:

- [`baseline-memory-sfexample.json`](baseline-memory-sfexample.json),
  [`baseline-turso-sfexample.json`](baseline-turso-sfexample.json), and
  [`baseline-postgres-sfexample.json`](baseline-postgres-sfexample.json);
- [`adversarial-memory-sfexample.json`](adversarial-memory-sfexample.json),
  [`adversarial-turso-sfexample.json`](adversarial-turso-sfexample.json), and
  [`adversarial-postgres-sfexample.json`](adversarial-postgres-sfexample.json);
- [`policy-portable-sfexample.json`](policy-portable-sfexample.json).

## Evidence boundary

No Grust result from a dirty worktree is included. Memory uses the in-process
reference executor. Turso and PostgreSQL reports label each query as SQL
row-source pushdown plus Rust projection or as an in-memory reference fallback;
a matching count is not silently presented as fully native execution. The
example graph is intentionally too small for backend-winner claims.
