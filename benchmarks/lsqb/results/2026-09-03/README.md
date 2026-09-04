# Graph benchmark evidence — 2026-09-03

This directory contains bounded, reviewable evidence for the graph benchmark
harness. The recorded upstream reference is complete. Grust compatibility and
adversari.al extension evidence is deliberately not promoted here until it is
rerun from a clean, committed source revision.

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
  and pending-Grust evidence state.

The run host was macOS 26.2 (25C56) on arm64. It used Docker Engine 29.4.3 on
`linux/arm64`, with 10 Docker-reported CPUs and 8,321,712,128 bytes of memory.
The host CPU model is intentionally omitted; Docker-reported architecture and
resource allocation describe the execution boundary used here.

The five original upstream CSV files remain ignored scratch output under
`benchmarks/lsqb/out/upstream/`. Their SHA-256 digests are retained in the JSON
summary. The checked-in TSV losslessly normalizes those six upstream fields by
splitting the `10 threads` value into a numeric `threads` column, then adds a
run number and header.

## Evidence boundary

No Grust result from a dirty worktree is included in this directory. Once the
0.13 sources are clean and committed, `run-grust.sh` must be rerun and its
embedded `grust_revision`, runner image ID, backend image identities, and
policy outcomes reviewed before any Grust JSON is added.
