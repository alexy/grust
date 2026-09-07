# Read-executor speed work

Updated 2026-09-05. This is an implementation ledger, not benchmark evidence or
a release-completion statement. [GRUST-FAST](../GRUST-FAST.md) is the original
proposal; [indexed reads](INDEXED_READS.md) describes the implemented API.

Test totals were recounted from retained `test result:` lines on 2026-09-05.
Several older engine/adapter aggregates were overstated by 23; the historical
counts here and in the linked candidate/single-pass/adjacency ledgers are now
corrected. Raw logs are unchanged, and doctests are counted separately.

## Completed implementation slices

- 2026-09-06 (EC2 session): `TursoGraphStore::indexed_snapshot`, the
  resident typed index for a durable store, built from a full read of the
  store under the connection gate and dropped by every statement that could
  change it. The LSQB harness routes a Turso query with no scalar SQL count
  but a proven `count-factorized` plan over that index under the distinct
  class `backend-resident-index-rust-count`; the snapshot is built in the
  worker's `load_ns`, never inside the query boundary. Registry, validator
  (`validate-matrix-publication.py`) and site verifier (`allowedClasses`)
  admit the class, each with tests. Differential validation at example scale
  is the count-plan integration test. `PostgresGraphStore::indexed_snapshot`
  followed on the same day: the LSQB PostgreSQL worker attaches to the
  once-loaded service, reads it back over the wire and builds its index
  before READY, and declares the same class for the same 18 example cases
  (registry, validator and site verifier extended, with tests). SF0.1
  diagnostics are recorded under "Resident index at SF0.1" below; SF0.3 is
  not measured on this host. Resume mode for `run-grust.sh` is not
  implemented yet.

- Observation `plan` metadata landed separately in local commit `c514e52`,
  before the indexed speed changes. Missing historical plans stay unknown;
  Neo4j and upstream bundles are not backfilled.
- Immutable typed incoming/outgoing adjacency, cached exact serialized graph
  size, and write-invalidated shared Memory snapshots. Dense offsets are used
  only for sufficiently populated types; sparse types store active source
  slots. Structural auxiliary storage is O(V + E), not O(V × types).
- Exact factorized counts for proven pattern forests, including chains,
  shared-variable stars and disconnected products. General shapes fall back.
- Exact factorization of an undirected two-hop wedge with unequal outer nodes
  and a differently typed outgoing leaf. The subtraction is exact; self-loops
  and parallel/reciprocal multiplicities are preserved.
- Independent optional-leaf factors on mandatory forests, including a
  bag-preserving variable-only `WITH`. Missing matches and failed optional
  anchor predicates pad once; dropped bindings keep their multiplicity.
- Weighted tag intersections and tag/wedge anti-joins. Exclusion is by witness
  existence, while matching parallel edges retain their multiplicity.
- Wedge anti-joins now use degree-oriented weighted support triangles instead
  of repeated neighbor probes. The topology helper is shared with location
  triangles; six asymmetric role placements, loop invariance and exact u128
  cancellation have independent regressions. Work/byte sweeps cover later
  support, orientation, sorting and intersection stages.
- Shared support targets now use explicit rank order and safe strict-suffix
  intersections, with original-ordinal callbacks. Compact weighted storage and
  wide arithmetic remain; rank construction and sparse-graph overhead are
  accounted separately from traversal savings.
- Wedge leaf/non-anti center scans and mandatory forest branch combination
  reuse already-borrowed label candidates, retaining all predicates and full
  array initialization charges. See the [candidate pass](GRUST_SPEED_CANDIDATES.md)
  for baseline logs and comparison-readiness gaps.
- Non-anti wedges now accumulate checked degree, weighted-leaf and overlap
  totals in one grouped traversal per center, without extra allocation. New
  canonical receipts explicitly bind startup host evidence, retaining legacy
  layouts. See the [single-pass slice](GRUST_SPEED_SINGLE_PASS.md) for the
  latest implementation, verification and qualification limitations.
- Non-anti physical-slot scans now prepay bounded 256-slot chunks, retaining
  exact successful totals, group charges and final deadline checks. Group
  multiplicity/degree and weighted totals now use proven checked u32/u64 bounds,
  retaining u128 cancellation and the shared leaf array. Narrowing gates pass
  894 engine/adapter and 118 runner tests plus strict Clippy; both
  scales pass all 22 pinned counts and 2,400 q6 repetitions pass. Details and
  diagnostic-only caveats are in the same single-pass ledger. Historical SDK
  auditors now select their authenticated legacy contract; four frozen audit
  outputs remain byte-identical and 35 contract/audit/export tests pass.
- Property-bearing forest roles with at least two mandatory incident atoms
  now prefilter impossible candidates through charged typed-adjacency existence
  checks, retaining all predicates for survivors and excluding optional edges.
  Prepared borrowed typed views now resolve each type once per enabled role,
  with separately charged storage and row probes. Enabled roles now borrow a
  strictly shorter sparse directed source seed when available, keeping every
  predicate, row probe and full-domain charge. Current gates pass 917
  engine/adapter, 111 core/Memory and 118 runner tests plus a lifetime doctest
  and strict Clippy; all 22 counts pass at both scales, and q1/reversed-chain
  diagnostics each complete 120 oracle-checked repetitions. The
  [adjacency ledger](GRUST_SPEED_ADJACENCY.md) records diagnostic-only profiles,
  the remaining accounting/adjacency hotspots and outstanding timing/release qualification.
- Directed four-cycle counts with adaptive merge/probe intersections, without
  assuming functional creator edges. Symmetric location triangles use sparse
  path weights and oriented intersections; nonfunctional location paths,
  self-loops and repeated-person cases are counted explicitly.
- Scalar node/edge scans, zero-hop identity paths, bounded range cardinality,
  constant/null predicates and scalar unions. Separate proofs preserve binding
  nullability and union semantics; matching rows are never buffered.
- Borrowed scalar inline-property comparisons avoid copying incompatible JSON
  payloads. The reference executor shares this shortcut and precharges complex
  nonliteral equality conversions, including decimal formatting.
- Reference `MATCH` relationship uniqueness now uses physical edge slots,
  including comma paths and mixed fixed/variable paths. Separate `MATCH`
  clauses reset uniqueness. Named fixed edges retain identity through `WITH`
  aliases, bare-variable grouping and `WITH DISTINCT`;
  unsupported repeated relationship-list bindings fail explicitly.
- Indexed bounded entrypoint with the reference policy gates and cumulative
  work/memory/deadline charging. Load/index construction remains outside the
  query budget and inside the benchmark load interval.
- LSQB Memory routing, worker declarations and plan-bound row admission.
  Only an admitted non-materializing algorithm receives the row-limit exemption;
  a materialized result containing zero rows does not qualify.
- Surreal HTTP and SDK endpoint predicates pushed to the server, retaining
  full logical IDs and Rust postfiltering. No live seek/performance claim.
- Scalar SQL predicates narrowed to genuine property/string equalities with
  exact JSON payload-type and byte-comparison guards. Dialect capability checks
  are shared by execution, declarations and SQL hashes. Numeric, inline-label
  and other unproven filters retain their older routes. SQL joins with
  overlapping relationship types now conservatively decline: nullable or
  duplicate public IDs do not prove physical edge uniqueness. Sail still has
  no scalar-count opt-in, but its unsafe row-source joins also decline.

These changes are still in the worktree unless a source commit is named above.

## Verification and remaining work

- 2026-09-06 (EC2, strain harness A8): the clause-pipeline reference
  executor's grouped aggregation is quadratic in the number of groups.
  `MATCH (c:Message {kind: 'Comment'})-[:REPLY_OF]->(m:Message) RETURN
  m.id AS root, count(c) AS replies ORDER BY replies DESC, root LIMIT 20`
  over a 200,000-edge SNB slice (151k comments, about 100k distinct roots)
  took 1,230 s, while the same shape over 16k groups (tag popularity, 290k
  rows) took 400 ms. On a proportional 200k slice `MATCH (m:Message {kind:
  'Post'})-[:HAS_CREATOR]->(p:Person) RETURN p.id AS person, count(m) AS
  posts ORDER BY posts DESC, person LIMIT 50` took 107 s over about 39k
  rows and 1.5k groups, and the reply fan-in 114 s, where every other row
  query answers in 30–300 ms. Turso's row-source pushdown answered each in
  about a second. The harness runs the oracle under the bounded read
  policy's 120 s cooperative deadline and records the shape as not
  comparable; the executor's grouped aggregation is the thing to fix.

| Area | Verified locally | Still required |
|---|---|---|
| Memory counts | All 22 pinned example answers match independent/reference oracles; all 22 example workers execute their declared fast plans; all 22 counts also pass one-pass SF0.1/SF0.3 diagnostics | Qualified container cohorts and performance comparison |
| Index | Dense/sparse direct-scan parity; generated multigraphs; actual allocation-capacity bounds | Representative load/resident-memory measurements |
| Policy | Fast-path and fallback work/memory/deadline refusals; cached exact byte limit; length-dependent scalar/forest label and null-property lookup charges | Maintain gates as plans change |
| SQL scalar counts | Mixed-value exact string filters; scalar decoding; legacy row-source oracles; real workers; 4 optimized cases per SQL backend | Live PostgreSQL check; qualified measurements |
| Evidence tooling | Hash-bound plan registry; legacy compatibility; mixed/fallback-plan refusals; automatic classifier/SQL-digest parity test | Independent site admission before new optimized cohorts are published |
| Release | API and book source updates underway | Final source/package checks, rebuilt book, release documentation and publication |

All nine baseline shapes and thirteen attacks now have structural fast-plan
proofs. The plan registry is generated from those proofs, not query-name
shortcuts. Its 22 Memory entries are separate from Turso's and PostgreSQL's four
scalar-count entries each; the eight registered SQL digests remain unchanged.
This is workload coverage, not a claim that arbitrary Cypher counts optimize.
Unproven queries retain the bounded reference route.

The scalar SQL path is opt-in for Turso/PostgreSQL, not Sail. Review identified
existing row-source property-coercion and inline-property/structural-label
ambiguities. Scalar admission now rejects those forms and uses exact checks for
the supported string-equality subset. The older row-source limitations are not
claimed fixed. Mixed-value tests supplement the example oracle; an example-only
pass cannot settle heterogeneous property semantics.

The candidate-pass Cypher, Turso and PostgreSQL-core gate passed 880 tests, with seven
existing live-service/regeneration tests and one manual kernel diagnostic
ignored. Retained output is at
`/tmp/grust-candidate-full-tests-20260905-01/command.log`.
Core/Memory storage checks pass 98 tests and strict Clippy. Offline plan/evidence
validation passes 77 Python tests and the shell evidence suite against the
22-entry Memory registry. Frozen v3 matrices still rebuild from their original
components, and four frozen publication receipts verify unchanged. The v2
bundle remains valid evidence but is intentionally ineligible for the v3
separated-timing summarizer. Retained validation output is at
`/tmp/grust-plan-offline-validation-20260905.QUJ1a5`.
Surreal passes 41 tests, with one existing live-service test ignored. The LSQB
gate passes 118 tests: 79 library, 11 matrix, one CLI policy test, 24 count
integrations, one registry parity test and two real-worker protocol tests (the
Memory test executes all 22 cases). Retained output is at
`/tmp/grust-candidate-lsqb-tests-20260905-01/command.log`.
Strict Clippy passes for Cypher, Turso, PostgreSQL-core and the LSQB runner after
the candidate-reuse changes. Retained checks are
`/tmp/grust-candidate-clippy-20260905-01` and
`/tmp/grust-candidate-runner-clippy-20260905-01`. An earlier triangle lint pass
caught one range-indexing loop in a new test; it was rewritten with iterators,
and its two arithmetic tests pass again. The earlier Cypher/Memory-enabled facade gate
also passes strict Clippy and ten tests. The new
[load-once diagnostic profiler](../benchmarks/lsqb/examples/profile_memory/README.md)
passes eight focused tests and strict Clippy, including bounded subprocess
regressions for a full, unread stdout pipe. It retains one production Memory
index, checks pinned counts on every iteration, emits flushed progress and owns
one finite deadline thread that is joined on return. Hard timeout uses immediate
process exit without waiting for output locks; the external logger records exit
124, and a partial final diagnostic line is possible. It is not a publication
worker or a change to the measured lifecycle. The optimized executable passes
the all-22 example smoke test and a deliberately one-second repeat exits 124;
both process IDs were verified gone afterward.
These are offline verification tests, not warm-up/measurement observations.
Live-service tests remain separately qualified.

## Qualified SF0.1 cohort (2026-09-06, laptop, revision `68d1b09`)

The first complete, receipt-bound SF0.1 matrix with the indexed count plans:
`benchmarks/lsqb/out/matrix-sf0.1-w2r10-68d1b09-f1`, receipt SHA-256
`4c29af22be0b4437f4b4e221bc2f8d4e10fa8623a4ff4c1ea509584bf6ace1f3`, W2/R10
rotating, 60 s deadline, 8 CPU / 6 GiB per container, host preflight at the
recorded 400 percent aggregate limit with no busy process. The receipt was
issued by hand with the launcher's own arguments after the launcher's receipt
step hit a bash 3.2 empty-array error (fixed in `12ae290`); `verify` passes.
`all_required_outcomes_valid` is false: the FalkorDB cells end as declared
quiescence terminations (q9 and a7 in warm-up 1), and the SQL-routed Turso
and PostgreSQL queries time out as recorded below.

Medians in ms of ten measured samples, all counts exact; the native column is
the site's published SF0.1 native comparator bundle, listed for scale only
(different execution class and query boundary):

| Query | Memory (in-process-reference) | Turso | PostgreSQL | Native comparator |
|---|---|---|---|---|
| q1 | 52 | timeout 60 s (sql-count) | timeout 60 s (sql-count) | 1,182 |
| q2 | 150 | 180 (resident index) | 174 (resident index) | 352 |
| q3 | 10 | 11 | 9 | 526 |
| q4 | 123 | 10,187 (sql-count) | 13,843 (sql-count) | 1,975 |
| q5 | 108 | 128 | 119 | 1,621 |
| q6 | 6 | 7 | 5 | 3,312 |
| q7 | 123 | 132 | 121 | 2,546 |
| q8 | 106 | 126 | 119 | 2,507 |
| q9 | 14 | 13 | 12 | 3,606 |
| a1 reversed chain | 51 | timeout 60 s (sql-count) | timeout 60 s (sql-count) | 19,015 |
| a2 reordered join | 156 | 185 | 187 | 808 |
| a3 split match | 124 | 140 | 142 | 2,178 |
| a4 optional fan-out | 107 | 113 | 109 | 2,564 |
| a5 negated pattern | 108 | 128 | 126 | 4,010 |
| a6 to a13 | 1.5 to 4.5 | 1.7 to 3.0 | 1.7 to 3.0 (a7 54,208 sql-count) | 6 to 10 |

Cell wall times: Memory 6 + 9 min; Turso 121 + 174 min (59 s per-observation
reload and index build); PostgreSQL 27 + 35 min (4 s per-observation attach
and index build); Ladybug, SurrealDB, LanceDB, pgGraph, PGQ and Helix are
`unsupported` at this scale (materialization refused); Sail `unavailable`;
CocoIndex `not_applicable`.

Follow-ups recorded for the EC2 session in
`querygraph/adversarial-graph/docs/notes/task-turso-resident-index.md`: when
both a `sql-count` entry and a proven `count-factorized` plan exist the worker
takes the SQL count (q1, q4, a1, a7), and Turso's per-observation reload
makes the two Turso cells five hours; a prebuilt database copy would keep the
fresh-process proof at a fraction of that.

## Qualified SF0.1 cohort after the route and reload changes (2026-09-06, laptop, revision `7429fc7`)

`benchmarks/lsqb/out/matrix-sf0.1-w2r10-7429fc7-g1`, receipt SHA-256
`eb05aa3199443cb58248b4fbe7d48f9cb781a41858b222eb0567309788900669`, the same
W2/R10, 60 s, 8 CPU / 6 GiB protocol and host screen as the `68d1b09` cohort
above; every cell records zero host CPU steal. 83 minutes end to end
(the `68d1b09` run took 6 h 40 min). `all_required_outcomes_valid` is false
only because FalkorDB's two cells are declared quiescence terminations (q9
and a7 in warm-up 1), unchanged.

Memory, Turso and PostgreSQL each pass all 22 cases with exact counts; Turso
and PostgreSQL route every case through the resident index. Medians in ms of
ten measured samples; the native column is the site's published SF0.1 native
comparator bundle, listed for scale only:

| Query | Memory | Turso (resident) | PostgreSQL (resident) | Native comparator |
|---|---|---|---|---|
| q1 | 52 | 50 | 50 | 1,182 |
| q2 | 149 | 172 | 174 | 352 |
| q3 | 9 | 9 | 9 | 526 |
| q4 | 121 | 128 | 130 | 1,975 |
| q5 | 106 | 118 | 118 | 1,621 |
| q6 | 6 | 4 | 5 | 3,312 |
| q7 | 122 | 118 | 120 | 2,546 |
| q8 | 105 | 118 | 118 | 2,507 |
| q9 | 15 | 12 | 12 | 3,606 |
| a1 reversed chain | 50 | 51 | 51 | 19,015 |
| a2 to a5 | 104 to 153 | 107 to 181 | 108 to 182 | 808 to 4,010 |
| a6 to a13 | 1.8 to 5.2 | 1.5 to 4.5 | 1.7 to 4.6 | 6 to 10 |

Cell wall times: Memory 5.6 + 8.3 min; Turso 8.4 + 12.4 min (prebuilt store
copy, 4.7 s per observation); PostgreSQL 10.4 + 14.9 min; FalkorDB 3.9 + 3.2
min to termination; the rest seconds. Published on adversari.al/graph as the
second bundle of 2026-09-06 (`grust/sf0.1-7429fc7`); the `68d1b09` bundle
stays as the pre-change baseline.

## Measurement discipline

No new qualified performance cohort has been run for these speed changes. Historical
performance exclusions remain in force. In particular, the previously completed
SF0.3 Neo4j diagnostic overlapped orphaned CPU loops and is not a valid timing
baseline. Correctness receipts and frozen evidence remain immutable.

Docker exposes 10 CPUs and approximately 40 GiB RAM. A retained startup screen
at `/tmp/grust-count-host-preflight-20260905-01.json` failed all three samples
(943.1%, 968.9%, 959.6% aggregate host CPU). Four unrelated `et` processes were
using roughly eight cores; they were not stopped. A fresh passing screen and
ongoing isolation checks are required before a qualified timing run.
A subsequent screen at `/tmp/grust-count-host-preflight-20260905-02.json`
also failed all three samples (936.2%, 965.3%, 965.8%); the same four unrelated
processes remained active. No isolation gate has been bypassed.
The next screen at `/tmp/grust-count-host-preflight-20260905-03.json` also fails
(956.6%, 967.2%, 959.4%); the unrelated `et` processes remain untouched.
After the rank builds ended, `/tmp/grust-count-host-preflight-20260905-04.json`
again fails (929.2%, 967.4%, 963.8%). No qualified run was started.

### Initial large-scale diagnostics

The optimized, native macOS load-once diagnostic passes all 22 pinned counts
at both downloaded scales. These runs are explicitly non-publication, used an
unrestricted host process rather than the comparison's Docker envelope, and
ran under the known host contention. They are not Neo4j comparisons.

| Scale | Nodes | Edges | Counts | Total diagnostic wall time | Peak resident memory |
|---|---:|---:|---|---:|---:|
| SF0.1 | 432,235 | 2,080,404 | 22/22 | 6.40 s | 1.14 GiB |
| SF0.3 | 1,179,535 | 6,183,839 | 22/22 | 19.12 s | 3.24 GiB |

Totals include loading/index construction; they are single passes, not medians.
Memory is macOS `time -l` maximum resident set size. Retained logs are
`/tmp/grust-memory-profile-sf01-20260905-01` and
`/tmp/grust-memory-profile-sf03-20260905-01`. The executable SHA-256 is
`0a5a8060c4f0037ef8f3e6529dee29b4365ee57c3689f394fe317f11adfa6a28`.

### Resident index at SF0.1 (EC2 host, 2026-09-06)

Diagnostic only: `grust-lsqb-matrix` run in-process on the dedicated 4-vCPU
EC2 host (no Docker envelope, `-discovery` revision marker, one warm-up and
three measured iterations, official SF0.1 projected-FK dataset: 432,235
nodes, 2,080,404 edges). The Turso cell is the baseline suite through
`TursoGraphStore` in memory with the resident typed index; the Memory cell
is the reference. Every count matched the oracle. The first measured
iteration overlapped a `cargo build` on the same host and is excluded here
(it raised worker setup from 71 s to 180 s and pushed q1 past a 600 s
timeout); the clean rerun is recorded below it.

| Query | Turso route | Turso, iterations 2–3 | Memory (`count-factorized`) |
|---|---|---:|---:|
| q1 | `sql-count` (Turso SQL) | 260.2 s, 264.8 s | 70.5 ms |
| q4 | `sql-count` (Turso SQL) | 14.71 s, 14.70 s | 165.9 ms |
| q2 | resident index | 202.0 ms, 201.4 ms | 181.6 ms |
| q3 | resident index | 12.1 ms, 12.1 ms | 12.0 ms |
| q5 | resident index | 144.4 ms, 143.1 ms | 149.9 ms |
| q6 | resident index | 6.4 ms, 5.8 ms | 6.3 ms |
| q7 | resident index | 166.9 ms, 163.4 ms | 159.4 ms |
| q8 | resident index | 145.0 ms, 144.9 ms | 151.7 ms |
| q9 | resident index | 18.8 ms, 18.6 ms | 18.4 ms |

Worker setup per observation: Turso 71–73 s, of which the chunked
single-transaction load is about 67 s and the read-back plus index build
5.0 s (`resident_index_built`: 432,235 nodes, 2,080,404 edges,
359,300,479 serialized bytes, 4,957 ms); Memory 2.7–2.8 s. A one-iteration
A/B on a quiet host with the binary before and after the telemetry change
agreed on every query (q1 260.0 s / 261.4 s, q2 194 / 197 ms, setup 71–72 s).

The host is a burstable t2.xlarge. A full repeat of the Turso cell started
after 2.5 h of continuous compute ran uniformly 2× slower (setup 133 s, q1
490 s, q2 390 ms) with no other load on the machine, and `/proc/stat`
carried 8.2 h of accumulated CPU steal; after five idle hours the A/B above
ran at the original speed with zero steal. CPU-credit throttling does not
show in the load average the harness records, so timing runs on this host
need either a non-burstable instance or steal-time accounting per run.
Two readings of the table:

- Over the resident index, Turso answers within a few percent of Memory:
  the store's engine is out of the query boundary and the class says so.
- The two queries the dialect still routes to Turso's own scalar SQL count
  (q1, q4) are three to four orders of magnitude slower than the same plan
  over the resident index would be, and q1 sits at the edge of a ten-minute
  timeout. The route order "scalar SQL first, resident plan second" was a
  design assumption; at SF0.1 on Turso it is a measured cost. The order was
  reversed on the strength of this table (the resident plan whenever it is
  proven, `sql-count` under its own class otherwise), for Turso and
  PostgreSQL; all 22 pinned cases now register as resident entries.

Both follow-ups from the first SF0.1 cohort landed the same day and were
rerun on the same cell (one measured iteration, quiet host, zero steal):
with the resident plan preferred, q1 and q4 take 65.9 ms and 176.0 ms over
the index instead of 260 s and 14.7 s through Turso's SQL; with the
coordinator's file-backed store copied into each worker
(`per-observation-worker-copy`, 553 MB, 0.24 s to copy) the worker's setup
is 4.7–4.8 s per observation instead of 71–73 s, and the whole nine-query
cell ran in 2 min 9 s. Every count matched the oracle and no store file was
left behind.

SF0.3 was not measured on this host.

A finite q9 SF0.1 repeat completed 80 oracle-checked queries. Its retained
five-second stack sample at
`/tmp/grust-memory-q9-stack-sample-20260905-01/q9.sample.txt` identifies repeated
adjacency lookups and multiplicity probes inside the anti-neighbor check as the
dominant query path. The first profile-guided follow-up borrowed those fixed
slices once and used existence-only probes when no multiplicity was needed;
absent weighted probes skipped the upper-bound search. Four helper tests covered
both scan directions, raw multigraph oracles, self-loops and work budgets. That
probe implementation and its helper tests were subsequently replaced by the
triangle implementation and its independent oracle/budget regressions below.
The rebuilt executable also passes all 22 counts at SF0.1 and SF0.3; retained
logs are `/tmp/grust-memory-profile-sf01-post-probe-20260905-01` and
`/tmp/grust-memory-profile-sf03-post-probe-20260905-01`. Its SHA-256 is
`74988ffe72bda119f8c47e324e030791c0917dc969e903b82d8ef0f29871f268`.
A 160-query repeat and a separate 200-query sampled repeat finish normally,
with every count checked. The second five-second sample at
`/tmp/grust-memory-q9-post-probe-stack-sample-20260905-01/q9.sample.txt`
shows fewer repeated adjacency lookups, but grouped anti-neighbor probes remain
the dominant sampled query path. All owned query/sample processes were verified
gone afterward. An exact weighted support-triangle replacement is now
implemented and independently reviewed; it trades additional accounted scratch
space for fewer repeated probes. It shares topology with the location-triangle
plan, while preserving that plan's distinct/repeated-vertex formulas. The final
combined gate passes, including later-stage budget sweeps. The optimized rebuild
also passes all 22 counts at both SF0.1 and SF0.3. Its SHA-256 is
`356cfeb47ea3640d0c65c2f3228a2c2b5ce7308a36c4f0967474220d45807e06`;
logs are `/tmp/grust-memory-profile-sf01-post-triangle-20260905-01` and
`/tmp/grust-memory-profile-sf03-post-triangle-20260905-01`.

A 1,000-query q9 repeat completes with every count checked, at
`/tmp/grust-memory-profile-q9-post-triangle-20260905-01`. Its five-second sample
at `/tmp/grust-memory-q9-post-triangle-stack-sample-20260905-01/q9.sample.txt`
now shows oriented triangle traversal and thread-local budget accounting as the
main sampled query work. The repeated anti-neighbor probe path has been removed.
No numerical before/after speedup or diminishing-returns conclusion is claimed.

The shared q3 path also completes 500-query and 1,000-query SF0.3 repeats, with
all counts checked. Logs are `/tmp/grust-memory-profile-q3-post-triangle-20260905-01`
and `/tmp/grust-memory-profile-q3-sampled-post-triangle-20260905-01`; its sample is
`/tmp/grust-memory-q3-post-triangle-stack-sample-20260905-01/q3.sample.txt`.
Support traversal and budget accounting dominate, not an added full-graph scan.
Review confirms the original inverse map is reused, with only an active-domain
validation pass added. Individual timings vary substantially under contention;
neither a regression nor performance parity is established. All these query and
sampling processes finished normally and were verified gone. That
optimized binary is retained at
`/tmp/grust-memory-post-triangle-20260905.WZw77f/profile_memory`.

A separate SF0.3 q2 repeat completes 60 oracle-checked queries and exits normally.
Its log is `/tmp/grust-memory-profile-q2-sf03-20260905-01`; the five-second sample
is `/tmp/grust-memory-q2-stack-sample-20260905-01/q2.sample.txt`. Property/role
filtering and typed adjacency intersections are the next profile-supported
candidates. This uses the same pre-triangle-rewrite binary above. Its query and
sample process IDs were also verified gone; no profiling job remains running.
Cycle role filtering now borrows candidates from a required label in any
retained node mention, while still checking every predicate. Unlabeled roles
retain full scans. The final combined gate passes, including five focused tests
with mixed-label multigraph oracles and work/byte limits.

An SF0.3 q1 repeat likewise completes 60 checked queries. Its retained log and
sample are `/tmp/grust-memory-profile-q1-sf03-20260905-01` and
`/tmp/grust-memory-q1-stack-sample-20260905-01/q1.sample.txt`; property filtering
and accounting dominate its sampled executor work. No general budget mechanism
has been changed on the basis of this sample. The pre-triangle binary is also
retained at `/tmp/grust-memory-post-probe-20260905.AGoAJO/profile_memory`, with
the `74988f…` SHA-256 above. All q1 query/sample processes exited and were reaped.

### Chunked accounting and compact support storage

Both profile-guided experiments are now implemented, separately rebuilt, and
covered by the latest combined test gate:

- q9's two fixed-cost active-mask scans precharge chunks of at most 256 entries.
  Successful work remains exactly 3V, including the unchanged inverse-map
  initialization. End-of-pass checkpoints remain; insufficient work refuses
  before a chunk. Three new tests cover 0/1/255/256/257 entries, mixed/inactive/full
  masks, prior cumulative charges, exact remaining work and expired budgets.
- Stored weighted and forward edges now use checked `u32` multiplicities,
  reducing their sizes from 32/32 to 12/8 bytes. Each grouped count is a subset
  of the index's at-most-`u32::MAX` physical edges. Grouping, loop weights and
  q9 degrees remain wide; consumers widen stored values before arithmetic.
  A size/conversion-boundary regression passes, and the support-allocation
  refusal retains its exact phase with a recalibrated 6,000-byte budget.

The chunk-only gate passes 27 wedge tests, and its optimized executable is
retained at `/tmp/grust-memory-post-chunk-20260905.z4dKvR/profile_memory`, SHA-256
`c3bd42565ba431fdb1e28b7539630f9a21dca6c831c2c34e55883771d1f78ad7`.
A 400-query SF0.3 q9 run completes with every count and every iteration timing
retained at `/tmp/grust-memory-q9-post-chunk-sf03-20260905-01`. Its five-second
sample at `/tmp/grust-memory-q9-post-chunk-stack-sample-20260905-01/q9.sample.txt`
still shows triangle traversal and budget accounting as the main query work.
This is not evidence that all TLS overhead came from the changed scans.

Before these changes, 200-query SF0.3 q9 and q3 runs also finish with every count
checked and every timing retained, using the `356cf…` executable above. Logs are
`/tmp/grust-memory-q9-pre-chunk-sf03-20260905-01` and
`/tmp/grust-memory-q3-pre-compact-sf03-20260905-01`. Known host contention, native
execution and different sampling windows prevent a qualified before/after
comparison. No numerical speedup or diminishing-returns claim follows.

The combined compact executable is retained at
`/tmp/grust-memory-post-compact-20260905.sdiGmJ/profile_memory`, SHA-256
`cc0a591bcca4be8b1d8e2d6edf2afa91b0505fb9e56f4552cf199153259b7682`.
Strict Clippy for Cypher/Turso/PostgreSQL-core passes at
`/tmp/grust-count-compact-clippy-20260905-01`.
The compact executable again matches all 22 pinned counts at both scales; logs
are `/tmp/grust-memory-all-post-compact-sf01-20260905-01` and
`/tmp/grust-memory-all-post-compact-sf03-20260905-01`. Its 400-query SF0.3 q9 run
also completes with every count/timing retained at
`/tmp/grust-memory-q9-post-compact-sf03-20260905-01`; the five-second sample is
`/tmp/grust-memory-q9-post-compact-stack-sample-20260905-01/q9.sample.txt`.
Triangle traversal and budget accounting remain the main sampled query work.
The 400-query q3 run likewise finishes with all counts/timings retained at
`/tmp/grust-memory-q3-post-compact-sf03-20260905-01`. Its first late-attached
sample at `/tmp/grust-memory-q3-post-compact-stack-sample-20260905-01/q3.sample.txt`
captures graph deallocation, not the query kernel, and is excluded from
query-hot-path conclusions. The valid count results are retained unchanged.
A fresh 600-query q3 run completes with all counts/timings retained at
`/tmp/grust-memory-q3-post-compact-resampled-sf03-20260905-01`. Its early-attached
five-second sample at
`/tmp/grust-memory-q3-post-compact-query-sample-20260905-01/q3.sample.txt`
does capture query execution: support-triangle traversal, budget accounting
and support construction dominate, with smaller location-product work.
All build, test, query and sample sessions in this round finished; the known
process IDs were checked and are gone. No detached monitor was launched.

### Wedge-mask preparation and lookup accounting

Wedge masks now initialize all unconditional unlabeled-role bits during the
existing V-sized fill. Labeled roles visit borrowed first-label candidates,
retaining every label conjunct and byte-based lookup/comparison charge. That
mask-only pass did not narrow leaf/center scans. Seven tests cover all 16 role-label
selections against raw-edge/reference oracles, overlapping roles, multiple and
missing labels, unsupported properties, long strings, and exact work/byte
limits. The baseline and mask-only q6 runs complete 400 and 600 checked SF0.3
queries respectively, with every iteration timing retained:
`/tmp/grust-memory-q6-pre-masks-sf03-20260905-01` and
`/tmp/grust-memory-q6-post-masks-sf03-20260905-01`. Their query-phase samples are
`/tmp/grust-memory-q6-pre-masks-stack-sample-20260905-01/q6.sample.txt` and
`/tmp/grust-memory-q6-post-masks-stack-sample-20260905-01/q6.sample.txt`.
After mask preparation changes, budget accounting and the remaining full-domain
scans/grouped adjacency work are still visible; sample proportions alone are
not a speedup measurement.
The mask/accounting q9 run also completes all 200 checked SF0.3 repetitions at
`/tmp/grust-memory-q9-post-masks-sf03-20260905-01`. These earlier query/sample
process IDs were checked again and are gone.

The mask/accounting executable is retained at
`/tmp/grust-memory-post-masks-20260905.dCvpDh/profile_memory`, SHA-256
`74636fd8dee9d41fc6cce68eebb6b5a6d397cc2cdabd32e321a62f42131b146b`.
Strict engine/adapter Clippy passes at
`/tmp/grust-mask-accounting-clippy-20260905-01`.

The [manual support diagnostic](../benchmarks/lsqb/PERFORMANCE.md) separately
times orientation and traversal on isolated/path/star-4096 and clique-128
fixtures, outside their graph/index/support construction. It uses fixed five
iterations per fixture, analytical counts and one shared cooperative read
deadline; it does not launch a watchdog or claim a hard process/output timeout.
All 20 pre-rank iterations pass at
`/tmp/grust-support-pre-rank-diagnostic-20260905-01`. The release test executable
is retained at `/tmp/grust-support-pre-rank-20260905.qtKy2h/grust_cypher_tests`,
SHA-256 `c90fbb06dd79c8e6cf4f27543c6eb2b9001a1d413be7cab5cd51af953888ab07`.
These bounded, non-publication kernel timings do not replace LSQB cohorts.

### Rank-space suffix intersections

Support orientation now materializes dense ranks ordered by simple support
degree and stable graph vertex slot. Forward targets are sorted ranks, so an
edge x→y intersects only x's strict suffix after y: every forward neighbor of y
has rank greater than y. Each distinct-vertex triangle still appears once.
The callback translates all three ranks to original active-domain ordinals,
preserving heterogeneous q3 location weights and q9 role/leaf lookup semantics.
Self-loop formulas, reciprocal/parallel weights and closure-existence semantics
are unchanged.

Construction precharges an O(P log P) rank sort and two P-entry u32 maps, where
P is active-domain size. The inverse map is dropped after forward-list fill;
the callback map remains. Degree arrays also drop after their final use. The
8-byte forward edge and O(P + M) scratch bound remain unchanged, although
cumulative bytes increase by 8P. Three new regressions cover an ordinal-order
counterexample, exact suffix work, and a reversed-domain weighted clique with
unique original-ordinal callbacks and a one-unit-short budget refusal. Existing
q3/q9 multigraph oracles and new rank-map/sort phase refusals pass in the full
875-test gate at `/tmp/grust-rank-full-tests-20260905-01`.
Independent read-only review found no correctness/accounting
blocker. Strict engine/adapter Clippy also passes at
`/tmp/grust-rank-clippy-20260905-01`.

The rank release profiler is retained at
`/tmp/grust-memory-post-rank-20260905.XUdLPx/profile_memory`, SHA-256
`7c7b7c4914c2c805a7c691d3a59d7b409ff10ef44086594a74035d625c6bf03a`.
The release test executable is retained at
`/tmp/grust-support-post-rank-20260905.hSxrMG/grust_cypher_tests`, SHA-256
`35a859fba7785c5dbe4135935d34a5f554907557f3de6dc55047426d4d9e734e`.
All 20 rank kernel checks pass at
`/tmp/grust-support-post-rank-diagnostic-20260905-01`. A fresh run of the retained
pre-rank executable also passes all 20 at
`/tmp/grust-support-pre-rank-diagnostic-20260905-02`. No build overlapped either
run. Sparse fixtures show added rank preparation cost; the dense clique shows
reduced traversal time in this diagnostic, with large scheduling outliers.
Raw per-iteration orientation and traversal intervals remain separate. These
five-repeat fixtures under host contention do not establish a qualified
speedup or justify hiding sparse-graph overhead.

The rank executable matches all 22 pinned counts at SF0.1 and SF0.3; logs are
`/tmp/grust-memory-all-post-rank-sf01-20260905-01` and
`/tmp/grust-memory-all-post-rank-sf03-20260905-01`. These native load-once checks
finish in 6.254 and 17.657 seconds including setup/teardown, respectively;
the totals are not query latency measurements or backend comparisons.
The 600-query SF0.3 q3 run also completes with every count and timing retained
at `/tmp/grust-memory-q3-post-rank-sf03-20260905-01`. Its early-attached
five-second query sample is
`/tmp/grust-memory-q3-post-rank-query-sample-20260905-01/q3.sample.txt`;
it captures query execution, not load or teardown. Both the run and sampler
terminated, and their known process IDs are gone.
The analogous 400-query q9 run finishes with every oracle/count/timing retained
at `/tmp/grust-memory-q9-post-rank-sf03-20260905-01`, with a query-phase sample at
`/tmp/grust-memory-q9-post-rank-query-sample-20260905-01/q9.sample.txt`.
It and its sampler also terminate; the process IDs are gone.

The q3 sample still shows traversal/accounting followed by support construction
and country-weight intersection. Rank sorting appears but is not dominant.
The visitor symbol contains inlined callback work, so its samples cannot all
be assigned to comparisons. Main-thread event waits, the owned deadline
thread's blocked receive and idle worker waits are not query CPU work. Sample
tallies across these windows are not latency ratios.
The q9 sample likewise remains dominated by support traversal/accounting, with
smaller support construction and a visible full-V C-leaf loop. It supports
prioritizing C-label candidate reuse. B-center narrowing belongs to the
non-anti q6 branch and cannot be justified from q9's stack alone; the retained
q6 samples provide that separate context. Role-mask preparation is already
label-narrowed. No detached monitors or owned build/query/sample jobs remain
from this pass. Frozen benchmark/upstream bundles, lockfiles and generated
book artifacts remain unchanged; source packaging/publication is still pending.

### Next bounded experiments (not yet verified)

Candidate reuse for wedge scans and mandatory forest branches is now implemented;
see the [follow-up pass](GRUST_SPEED_CANDIDATES.md). Necessary mandatory-adjacency
prefiltering, prepared borrowed views and sparse source seeding now also pass
their gates; see the [adjacency ledger](GRUST_SPEED_ADJACENCY.md). Remaining
structural hypotheses require separate verification:

- Forest branch edge scans can prepay bounded chunks of physical adjacency
  slots, retaining exact successful scan totals and per-predicate accounting.
  Current q1/reversed-chain samples show substantial budget/TLS work; chunk
  size, early-refusal behavior and deadline cadence need focused tests first.
- The three-way country-weight intersection can specialize when all three
  actual slices are singleton. Keep the same one-unit work charge and checked
  multiplication order; unequal countries return zero, while empty/multi-country
  slices retain the existing merge. This does not assume functional locations
  or unit weights. Test overflow and fallback; the sample does not establish
  singleton frequency or a performance gain. Measure candidate reuse before
  this smaller hypothesis.

A follow-up audit rejects an apparent scalar-scan optimization: a8 and a9
already read label cardinalities without scanning vertices; a11 scans only
Person candidates, since missing/`Value::Null` properties cannot be inferred
from the label index. Its null test must keep `Value::Json(null)` distinct.
Single-pass outliers include runtime scheduling and do not establish an
executor bottleneck. The identified accounting gaps are now closed:
`count_scan.rs` charges label comparison/hash bytes and the null-property map
search factor used by `props_match`; forest candidate-label lookups are also
charged. Five new tests cover long-label hits/misses, empty graphs, cardinality
shortcuts and 4,096-property null probes with reference-result comparisons.
These are policy hardening, not claimed performance improvements; tighter
budgets may refuse queries that were previously undercharged.

Prefer small, idiomatic Rust helpers, explicit invariants and independent
semantic tests; do not disable budget safety or introduce a broad budget
abstraction merely to reduce calls.

The original proposal's times and workload descriptions are hypotheses to
verify, not promises. Property-value equality must not become slot equality
without a valid identity proof; location hops must retain multiplicity rather
than assume functional edges. Active read budgets are thread-local, so adding
parallelism requires deliberate budget propagation and accounting.

Before performance comparison: finish verification, select
qualified immutable comparison cohorts, clear the host-isolation gate, and use
the declared resource/lifecycle/sampling protocol. Retain raw outcomes and
incremental timing records. Build/test commands use `command-progress.py` with
durable output, periodic status and a terminal exit record; it is not the
benchmark query supervisor. Do not launch detached polling loops.
