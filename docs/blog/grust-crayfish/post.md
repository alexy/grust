# Grust 0.13.1 "Crayfish": Safer Sail, Faithful Surreal, Verifiable Comparisons

Grust is a backend-neutral property-graph library for Rust. Applications use
one model for labeled nodes, relationships, typed properties, schema,
traversal, and mutation while Memory, Turso, PostgreSQL, PostgreSQL SQL/PGQ,
pgGraph, Sail/Spark, SurrealDB, FalkorDB, LanceDB, or CocoIndex supplies the
storage, execution, or export mechanics. Crayfish is a deliberately small
registry patch to that broad story: it makes SurrealDB round trips faithful,
prevents Sail connection failures from disclosing endpoint secrets, and gives
the multi-backend graph benchmark a much stronger evidence boundary.

The project and detailed documentation are here:

- Repository and API guide: [github.com/querygraph/grust](https://github.com/querygraph/grust)
- Public facade crate: [grust-graph](https://crates.io/crates/grust-graph)
- Backend qualification record: [`benchmarks/lsqb/BACKENDS.md`](https://github.com/querygraph/grust/blob/main/benchmarks/lsqb/BACKENDS.md)
- Dataset and workload ladder: [`benchmarks/lsqb/DATASETS.md`](https://github.com/querygraph/grust/blob/main/benchmarks/lsqb/DATASETS.md)
- Durable graph benchmark hub: [adversari.al/graph](https://adversari.al/graph)
- Read the Grust book: [FirstPair hosted reader](https://firstpair.org/read/grust/), [PDF](https://firstpair.org/grust/pdf/), or [EPUB](https://firstpair.org/grust/epub/)

## A scoped 0.13 patch

Crayfish publishes `grust-sail`, `grust-surreal`, and the `grust-graph` facade
at 0.13.1. Every other publishable Grust crate remains on the compatible 0.13.0
Prawn line. The facade raises both optional dependencies so selecting `sail` or
`surreal` cannot silently miss these corrections; it does not pretend that
unchanged backends received new releases.

```toml
[dependencies]
grust = { package = "grust-graph", version = "0.13.1", features = ["sail", "surreal"] }
```

## Sail failures do not disclose endpoint secrets

A Spark Connect endpoint can contain user information, tokens, or signed query
parameters. Sail transport libraries may also echo the target in their error
chain. Crayfish replaces that connection failure with a stable, endpoint-free
message and covers a credential-bearing endpoint with a regression test.

## Logical identity survives SurrealDB storage

The Grust SurrealDB adapter maps logical labels to normalized physical table
names. Before Crayfish, reading a node reconstructed its Grust label from that
table, so a logical `City` could return as `city`. Record strings created
another edge case: splitting `` city:`City:4` `` at the last colon produced
the corrupted ID `` 4` `` instead of `City:4`.

The adapter now persists the original logical label in a reserved
`__grust_label` field and separates a record's table at its first colon. Its
schema-full lowering declares the internal label field, and validation prevents
applications from colliding with it. Existing rows remain readable through a
physical-label fallback. Focused unit tests and a live SurrealDB 3.2.4 gate
cover case-preserved labels, colon-bearing IDs, ordinary reads, and traversal.

## Twelve cells, without twelve kinds of wishful thinking

The Docker LSQB-derived harness now emits a rectangular twelve-backend matrix:
Memory, Turso, PostgreSQL, Ladybug, FalkorDB, SurrealDB, LanceDB, Sail,
pgGraph, PostgreSQL PGQ, Helix, and CocoIndex. Every baseline and adversarial
cell records an explicit state—pass, mismatch, timeout, unsupported,
unavailable, error, or not applicable—and an execution class. Backend-native
aggregates, backend row sources with disclosed Rust projection, whole-store
materialization with Rust reference execution, and the in-process reference
are never collapsed into a single winner table.

The adversari.al extension now has 13 deterministic count attacks and 14
backend-neutral policy-rejection attacks. New cases cover zero-hop paths,
Unicode and null semantics, comment trivia, full edge scans, query-byte and
path-hop pressure, invalid Unicode scalars, graph selection, and unterminated
comments. The policy track stays separate from storage timing.

Publication evidence is deliberately harder to fake accidentally. Clean runs
bind the source commit, immutable runner and service images, authenticated
dataset, exact query bytes, resource caps, warmups, repetitions, component
reports, merged matrices, logs, and canonical manifest into a standalone
receipt. Every executed cell contributes a normalized watchdog record that
binds its configured hard limit and elapsed time to the observed immutable
container identity and child exit status. Dirty discovery runs carry a
rejected marker. Sail, PostgreSQL PGQ, and Helix remain honestly unavailable
by default; an operator can qualify a local Docker service only by supplying a
complete pinned identity and matching CPU/memory limits. The orchestrator
never discovers or mutates an unrelated container.

Large-scale cells now fail closed before Rust can materialize an explosive
intermediate. The manifest records the maximum logical rows separately for the
Memory reference plan and a qualifying backend row source, and admits only an
exact cardinality or certified upper bound at or below one million. A monotonic
post-return check turns late non-yielding work into a timeout. Summary tables
expose execution class and per-component resources rather than presenting
unlike systems as one flat race.

## Bigger graphs answer different questions

The tiny 28-node LSQB example remains the conformance gate. Authenticated LSQB
SF0.1 and SF0.3 inputs provide the immediate performance and strain rungs,
with results grouped by execution class. Beyond LSQB, the documented ladder
uses SNB Business Intelligence for analytical reads and updates, SNB
Interactive and FinBench for transactional and anti-fraud workloads, and
Graphalytics for whole-graph algorithms. Text2GraphQuery belongs in a separate
language-generation accuracy track. There is no honest shortcut that turns
those distinct workloads into one ISO GQL leaderboard; translated GQL is
versioned and hashed as adversari.al work rather than described as unchanged
GDC input.

The concrete next database rungs are SNB BI SF10 validation and SF30+
performance, SNB Interactive v1 SF10 validation and SF30+ performance, and
FinBench SF10 with its complete parameter and ACID boundary. Graphalytics can
reach 17 billion edges, but remains an algorithm lane; Text2GraphQuery's latest
paper reports 267,276 language/query pairs, while its currently served artifact
still advertises 178,184, so each download is hashed and kept out of engine
throughput rankings.

LSQB itself is a Graph Data Council-maintained microbenchmark, not an official
LDBC benchmark. The Grust and adversari.al runs are independent and unaudited:
**These are not LDBC Benchmark Results.** Exact comparison evidence belongs at
[adversari.al/graph](https://adversari.al/graph), alongside the method and the
limits needed to interpret it.
