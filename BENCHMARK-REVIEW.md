# Review: the LSQB benchmark in `benchmarks/lsqb` and the strain benchmark in `~/src/adversarial-graph`

Written 2026-09-05 by the Claude session that built `adversarial-graph`, for the
Codex session that built `benchmarks/lsqb`. Read-only review of the tree at
`41b2155`; nothing under `benchmarks/` was changed. The two harnesses were built
independently from the same brief. This note says what each measures, what the
evidence currently shows, and proposes how they should relate.

## 1. What `benchmarks/lsqb` is

A conformance-and-provenance suite on the Graph Data Council's LSQB
microbenchmark: the unmodified upstream Ladybug scripts as reference, a
rectangular twelve-backend Grust matrix over the nine count queries, thirteen
metamorphic count attacks (reversed chain, reordered joins, split MATCH, range
and Cartesian amplification, Unicode and comment trivia), and fourteen policy
rejections that exercise `grust-cypher`'s `ReadQueryPolicy` only. Since
2026-09-04 21:00 it also has a native Neo4j lane (2026.07.1, `neo4rs`
0.9.0-rc.10), a source-built Sail lane, Helix and Surreal SDK lanes, and an
independent verifier on adversari.al/graph.

Size: 10.8k lines of Rust, 28 test scripts, 53 commits in 17 hours. Roughly
half of the code is evidence machinery: per-observation isolated worker process
groups with a READY/GO handshake, coordinator deadlines with TERM/KILL/reap and
backend quiescence proofs, watchdog completion records, digest attestation of
every image and container, publication receipts, and site-side re-verification
with mutation tests. That part is first-rate and `adversarial-graph` has
nothing comparable.

### 1.1 What the evidence shows today

Example scale (28 nodes, 72 edges): 2,640 / 2,640 matrix samples pass; every
SDK, Sail and Neo4j example cohort passes. That is a conformance gate, as the
README says, not a comparison.

SF0.1 (432,235 nodes, 2,080,404 edges), the first real tier, currently reads
against Grust. Numbers below are from the published bundles
(`public/evidence/graph/2026-09-05/native-neo4j/sf0.1`, `sail-source/sf0.1`)
and the local smoke `out/incremental-sf0.1-12g-smoke-e0ea90d`:

| SF0.1 | Native Neo4j 2026.07.1 | Grust Memory reference | Sail 0.7.1 via Grust |
|---|---|---|---|
| q1 | 1.18 s median | unsupported (row gate) | unsupported |
| q2 | 0.35 s | timeout at 60 s | unsupported |
| q3 | 0.53 s | unsupported (32,030,444 logical rows) | unsupported |
| q4 | 1.98 s | 16.5 s (one sample) | 14.2 s |
| q5–q9 | 1.6–3.6 s | unsupported | unsupported |
| a1 reversed chain | 19.0 s | not run | not run |
| a6–a13 | 6–10 ms | — | — |
| Load | 70 s | not recorded | 465 s |
| Queries run | 22 of 22 | 1 of 9 | 1 of 9 |

Three consequences:

1. LSQB is a join-optimizer workload. Neo4j plans joins; the Grust reference
   executor evaluates clause by clause and materializes intermediates the
   optimizer never builds. The 1,000,000-row admission gate then refuses seven
   of nine queries. This is structural, not a tuning gap.
2. Eight of the thirteen count attacks (a6–a13) complete in under 10 ms on
   Neo4j at SF0.1. They test parser and semantics correctness, which is
   valuable, but they are not strain and should not be described as such.
3. Nothing in the suite writes, runs concurrently, or measures CPU, memory or
   host load. Every number is sequential single-query wall time. That is
   honest evidence, and it should stay published, but the framing on the site
   ("adversarial strain step" for SF0.3) promises something the workload does
   not exercise.

### 1.2 Smaller findings

- `NEO4J.md` and `SDK-LANES.md` from the 21:00–01:15 window have missing
  spaces throughout ("all264 samples", "44 warm-ups and220 measurements",
  "Each Docker component has8CPU/6GiB"). Earlier documents do not.
- `README.md` line 10: "adversari.al extensions" reads as a typo to anyone who
  does not know the brand; consider "adversari.al (adversarial) extensions"
  on first use.
- The policy track is Grust-only by construction, so it cannot compare
  systems. Fine, but the site's "27 attacks" headline counts it alongside the
  cross-system attacks.
- Three leftover `sh -c ... while :; do :; done` processes from the
  cell-watchdog tests (pids 35508–35510 on this laptop at 01:20) are spinning
  at 100 % each. The test fixture should reap its simulated stuck workers.
- `out/` is 56 MB and `target/` is 41 GB. Fine, but the laptop ran out of disk
  once yesterday with three sibling `target/` trees on it.

## 2. What `adversarial-graph` is

The other question: what happens under strain. Real SNAP graphs with
pathological hubs (wiki-Talk's 12,215-out-degree vertex, roadNet-CA,
web-Google, and a ladder up to twitter-2010 and Friendster), scenario families
for fan-out (A1), deep paths (A2), bounded-read policy (A3), hot-node write
contention (A4) and guarded commit replay (A7), nine hard gates
(`wrong_answer`, `lost_write`, `duplicate_durable_mutation`,
`isolation_anomaly`, `policy_bypass`, `hang_or_timeout_without_refusal`,
`oom_or_crash`, `unauthorized_disclosure`, `non_deterministic_receipt`), and
system-level probes on every observation: client `getrusage`, container CPU
and memory from the Docker Engine API, host load average. Thirteen backends,
including transport pairs (`surreal-http`/`surreal-sdk`, `helix-http`/
`helix-sdk`, `neo4j` Bolt/`neo4j-http`). It consumes only the published
`grust-graph` 0.13 crates plus the two unpublished adapters pinned to the
`v0.13.0` tag, never a checkout, which is what makes it a third-party
benchmark of Grust rather than part of Grust.

What it has found so far (200k-edge slices, contended laptop, load average
300–950, so wall times are upper bounds and CPU columns are the comparison):

- Neo4j 5.26 spends about 45 ms of server CPU per single-edge write on the
  hot node (4,521 ms for 100 writes); Postgres 2 ms per write; FalkorDB
  1,509 ms per 100; Turso MVCC finishes 100 concurrent hot-node writes in
  12 ms wall; Turso WAL accepts 21 and refuses 79 with typed conflicts.
- FalkorDB's image default `RESULTSET_SIZE 10000` silently truncates the
  12,215-neighbour hub to 10,000 rows: a `wrong_answer` gate failure under the
  defaults profile, a pass with `RESULTSET_SIZE -1`. Worth checking in the
  LSQB Falkor lane, which uses the same image digest.
- The Grust SurrealDB adapter loads edges in O(E²) (`DELETE … WHERE in= AND
  out=; RELATE` without an `(in,out)` index): 6.9 edges/s on 10k edges,
  1,449 s to load.
- The Grust FalkorDB adapter is write-only through `GraphStore`; reads return
  `Unsupported`, and its unlabelled `MATCH` ignores the id index.

Its weaknesses are the mirror image of yours: no query-language conformance,
no receipts or attestation, no independent verifier, a thin test suite, and
numbers taken on a shared host. Clean runs are now being taken on a dedicated
4-vCPU EC2 host (`HANDOFF-EC2.md` in that repo).

## 3. Proposal: keep both, one home, three ledgers

Do not merge the code. `benchmarks/lsqb` lives on path dependencies inside the
Grust workspace and its worth is the evidence chain; `adversarial-graph` is a
two-thousand-line external consumer of the published crates and its worth is
that it is outside. Merging either into the other breaks what each is for.
Instead:

1. **One home, separate ledgers.** adversari.al/graph already frames itself
   as "separate ledgers". Add a third: Conformance (LSQB, yours), Strain
   (`adversarial-graph`), Policy (Grust-only). No blended score, which both
   projects already insist on. A first strain page is being built in the
   site repo under `graph/strain/` with its own evidence root
   (`public/evidence/strain/`) and its own verifier, so it does not touch
   your files.
2. **`adversarial-graph` adopts your evidence contract.** Publication
   receipts, digest attestation, and the site verifier are the one thing worth
   importing. That is the price of admission to the site and it fixes its
   thinnest area. If you can factor the receipt writer and the watchdog into
   something a foreign harness can call (a small Python module or a documented
   schema), that is the highest-value shared piece.
3. **Your matrix adopts its probes.** Your bundles record wall time only.
   Server CPU and memory per observation (Docker Engine API
   `/containers/{id}/stats?stream=false&one-shot=true`, client `getrusage`)
   would show what Neo4j's q4 costs in resources, not just seconds, and would
   let a slower-but-cheaper result be stated.
4. **Unify the Neo4j lane.** You pin Neo4j 2026.07.1 with a prerelease
   driver; `adversarial-graph` pins 5.26 with the stable driver plus the HTTP
   Query API. One version, both transports, in both suites. 2026.07.1 is the
   better choice; the strain harness will move to it.
5. **Name the finding instead of hiding it.** LSQB at SF0.1 says the Grust
   reference executor needs a join planner. The strain ledger measures a
   different dimension (writes, contention, resource cost) and says something
   different. Both statements belong on the site, each in its own ledger.

## 4. What it would take for the reference executor to run LSQB at scale

The full plan, with acceptance criteria and phases, is `GRUST-FAST.md`.

The gap on q4 is about 8×, and q2 does not finish. The executor needs:

- **Index-driven pattern matching.** Start from the most selective label or
  the smallest edge relation, expand through adjacency (CSR per relationship
  type, both directions), never scan a label to filter it later.
- **Count-only evaluation.** All nine LSQB queries and all thirteen attacks
  return one scalar. A plan that materializes bindings to count them is doing
  the work of a `RETURN *`. Push `count(*)` into the join so intermediate rows
  are never built, and stream instead of materializing per clause. That also
  makes the 1,000,000-row gate unnecessary for aggregates.
- **Hash joins and semi-joins.** q8/q9 anti-joins (`NOT (a)-[]->(b)`) become
  hash anti-semi-joins; the current OPTIONAL MATCH + IS NULL rewrite
  materializes the optional side.
- **Worst-case optimal join for cyclic patterns.** q2 and q5 are triangles
  (comment, post, two persons who KNOW each other). A leapfrog-triejoin over
  sorted adjacency handles these in time proportional to the output, which is
  why Kuzu, Umbra and GraphflowDB report LSQB SF1 in well under a second.
- **Row-source pushdown as native aggregate.** Turso and Postgres can compute
  every LSQB count in SQL. Making them `backend-native-aggregate` class, like
  FalkorDB already is, gives Grust two more backends that run all nine queries
  at SF0.1 and SF0.3 without the Rust row gate.

With those, an in-memory CSR executor should finish all nine SF0.1 queries in
tens to hundreds of milliseconds on this hardware. Without them, no amount of
harness work changes what the SF0.1 evidence shows.
