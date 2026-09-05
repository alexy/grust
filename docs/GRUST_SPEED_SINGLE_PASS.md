# Single-pass wedge and host evidence binding

Updated 2026-09-05. This continues the verified
[candidate traversal slice](GRUST_SPEED_CANDIDATES.md). The goal remains maximum
measured speedup and a qualified Neo4j comparison, not merely passing tests.
The preceding goal turn made implementation/test/profile progress. This slice
is not release or benchmark-completion evidence.

## Implementation

The non-anti wedge now visits each matching B center's grouped T-neighbors once.
For each center it accumulates A degree, weighted C leaves `sum_C(m * L)`, and
overlap `sum_A_and_C(m * m * L)`. Its contribution is
`degree_a * weighted_leaves - overlap`, the exact rearrangement of the previous
per-C sum. All accumulators/products/subtraction are checked `u128`; narrowing
to the final `i64` scalar remains after cancellation. A stack-only named totals
struct replaces the second traversal; there is no additional heap allocation.

Borrowed B/C candidates and complete masks, distinct U type, physical parallel
and reciprocal edges, self-loops, outer-node inequality and work/byte charges
remain. q9's support algorithm is unchanged. New tests cover weighted raw-slot
and reference oracles, all accumulator overflow paths, wide cancellation, and
exact work/allocation limits. One fixture requires 53 work units and 168 bytes;
the former two-pass path needed 60 work units. This is algorithmic work, not a
measured speedup ratio.

New canonical manifests require `host_preflight: {"schema":
"grust-host-preflight-v1"}` and fixed `host-preflight.json`. Both receipt creation
and verification validate the already-captured bytes: exactly three passing
samples, finite CPU totals below the screen threshold, no busy processes,
increasing UTC timestamps and explicit startup-only limitations. Unknown fields,
duplicate keys, failed/partial records and malformed contracts are rejected.
Existing artifact/inventory digests bind the file; no receipt schema changes.
Absent markers preserve historical layouts exactly, not a presumed pass.

Summaries separately label `unrecorded-legacy` versus `recorded-startup-only`,
recheck host bytes against the verified inventory and retain an explicit false
clean-host eligibility flag. Per-query eligibility still means a complete,
successful fixed-plan cohort, not ongoing isolation. No freshness or origin
authenticity is implied. Startup can precede builds. Native/SDK export contracts
and public-site admission need separate work before new publication; frozen
payloads and trust sentinels are not changed.

## Verification

The combined engine/adapter gate passes 885 tests, with eight ignored checks,
at `/tmp/grust-wedge-single-pass-full-tests-20260905-01` (61.980 seconds).
Strict Clippy passes at `/tmp/grust-wedge-single-pass-clippy-20260905-01`.
The LSQB runner passes 118 tests at
`/tmp/grust-wedge-single-pass-runner-tests-20260905-01` and strict Clippy at
`/tmp/grust-wedge-single-pass-runner-clippy-20260905-01`. Workspace/runner
formatting and `git diff --check` pass.

The retained offline gate at
`/tmp/grust-host-binding-offline-tests-20260905-01` passes 91 Python tests
(5 preflight, 12 host-record/receipt, 21 summary, 14 observation-plan and
39 matrix-publication) plus the shell evidence fixtures. Independent review
found no blocker in the new host contract or its integration. All five frozen
receipt paths verify unchanged at
`/tmp/grust-host-binding-frozen-receipts-20260905-01` (four distinct receipts;
the site-admission copy shares the matched-example receipt).

Review found a separate historical SDK audit compatibility gap, repaired in the
follow-up recorded below:
`validate-sdk.py` and `validate-helix-sdk.py` pin the old canonical manifest hash,
while loading today's expanded source manifest. The repair preserves the old
pin and selects an authenticated historical contract independently of current
canonical state. The exact old bytes are in both pinned SDK source revisions
and the frozen matched-example manifest, SHA-256
`1dcae942840f216a83282f45f27e7fe228616e8f51af764689dc4f4fea0de849`.
Do not depend on untracked output directories or waive the digest check.

## Profiler

The release build passes at
`/tmp/grust-wedge-single-pass-release-build-20260905-01` (86.535 seconds).
The new immutable executable is
`/tmp/grust-memory-post-single-pass-20260905.eULdY4/profile_memory`, SHA-256
`56e592e9c64d3cf6ede0826c9038bc6c7cb15f570d1d4dc49d6ab47e295e89ae`.

The immutable pre-change profiler remains
`/tmp/grust-memory-post-candidates-20260905.0beMtO/profile_memory`, SHA-256
`cc0a462be5275adb67b30c2d73f7a33f488f68c0e1bf2af3f105c8db87fa129d`.
No qualified cohort is running. The four unrelated embedding workers remain
CPU-active and untouched. No new container or detached watchdog was started.

All 22 pinned oracle counts pass at both scales:

- SF0.1: `/tmp/grust-memory-all-post-single-pass-sf01-20260905-01`,
  7.493 seconds total, 432,235 vertices and 2,080,404 edges.
- SF0.3: `/tmp/grust-memory-all-post-single-pass-sf03-20260905-01`,
  18.012 seconds total, 1,179,535 vertices and 6,183,839 edges.
- q6 SF0.3: all 1,200 repetitions pass at
  `/tmp/grust-memory-q6-post-single-pass-sf03-20260905-01`, 26.626 seconds total.
  The query-phase sample at
  `/tmp/grust-memory-q6-post-single-pass-query-sample-20260905-01/q6.sample.txt`
  completes successfully. It attaches after query iteration 27 starts, not
  during loading. Grouped traversal, TLS/budget access and the checked
  `CenterTotals::add_group` accumulator dominate the sampled query work.

These are native load-once diagnostics with per-iteration oracle/timing records
and explicit `publication_eligible: false`. Total wall durations include setup,
logging and teardown; they are not query-latency comparisons. Sampling windows,
host contention and lifecycle differ from a qualified cohort. The prior
candidate q6 run's 35.767-second total must not be turned into a speedup claim.
Main/deadline thread waits are not query CPU work; raw stack tallies are not
comparative timings.

The fresh startup screen at
`/tmp/grust-count-host-preflight-20260905-06.json` fails all three samples:
948.5%, 963.5% and 971.3% aggregate CPU. No gate was bypassed. The same four
unrelated `et` workers remain untouched. All build, test, lint, query and sampler
sessions started in this slice are terminal, and their known process IDs were
checked absent. Frozen bundles, upstream inputs, lockfiles and generated book
artifacts are unchanged. Source/book manuscript edits remain uncommitted;
packaging, book rebuild and release/publication gates are still pending.

## Next evidence-led work

Investigate bounded amortization of per-physical-slot budget/TLS access in
grouped adjacency traversal, retaining exact successful work charges and
frequent deadline checks even for one huge parallel-edge group. Never disable
accounting or replace it with an unbounded precharge. The accumulator's remaining
call/field traffic is a separate measured hypothesis; inline annotations need
evidence and code-size consideration. q1's property-lookup hotspot and the
candidate slice's mandatory-adjacency prefilter hypothesis also remain open.

For publication, separate native host evidence binding, current-source site
admission, immutable Memory source/image
handoff and ongoing host-isolation proof remain prerequisites. Keep resource
and lifecycle differences explicit. Neither a Neo4j win nor diminishing returns
has been established; the full goal remains active.

## Bounded physical-slot scan follow-up

The subsequent goal turn continues from the single-pass q6 sample rather than
claiming a qualified speedup. Physical scan charges now prepay at most 256 slots
across combined outgoing/incoming traversal. The combined offset increases once
per physical slot, so chunk lengths partition the total exactly on success.
Group charges remain one each before callbacks. Incoming duplicate loop entries
are still charged but not counted twice. The helper holds no borrowed budget
state across callbacks and finishes with a checkpoint, including empty rows.

A tight budget can conservatively refuse a partly affordable chunk. Deadlines
are checked at most 256 cheap scan steps apart even within one huge parallel
group, and more frequently when group callbacks intervene. No allocation,
unbounded precharge or budget bypass was added. q9 still uses the separate
support grouper. Independent review found the proof sound; small empty rows gain
one checkpoint, so profiling must verify the tradeoff.

Six new tests cover raw-slot semantics, 0/1/255/256/257 boundaries, sparse groups,
parallel/reciprocal/self-loop carry, exact cumulative work and zero bytes,
conservative prepayment before visitor work, nested budgets and final deadline
checks. Deadline expiry uses a test-only hook, not a spin loop or a sleep.
The initial full gate passes 891 tests with eight ignored checks at
`/tmp/grust-wedge-chunk-full-tests-20260905-01`; runner tests pass 118 at
`/tmp/grust-wedge-chunk-runner-tests-20260905-01`. Initial strict engine Clippy
requested idiomatic `is_multiple_of()` rather than manual modulo. After that
correction, the full gate again passes 891 tests with eight ignored checks at
`/tmp/grust-wedge-chunk-full-tests-20260905-02` (43.922 seconds). Strict engine
Clippy passes at `/tmp/grust-wedge-chunk-clippy-20260905-02`. The final runner
gate at `/tmp/grust-wedge-chunk-runner-final-gates-20260905-01` passes all 118
tests, strict Clippy, workspace/runner formatting and `git diff --check`.

The historical SDK repair adds the exact allowlisted 21,840-byte manifest under
`benchmarks/lsqb/contracts/`, a shared strict loader, and narrow loader changes
in the Surreal/Helix auditors. There is no current-manifest, Git or output-tree
fallback. At `/tmp/grust-historical-sdk-audits-20260905-01`, all 13 fixture-free
contract tests and 12 retained-fixture mutation/audit tests pass. Recomputed
baseline/adversarial audit outputs for both backends compare byte-for-byte equal
to all four frozen `audit.json` files. No frozen artifact was rewritten, and
historical host/plan absence remains unknown. Future source profiles require
their own explicit contract and admission.

Temporary export checks at `/tmp/grust-historical-sdk-exports-20260905-01`
pass ten more tests across both backends' baseline and adversarial fixtures,
including inventory hashes, export collisions and client/server build mutations.
The retained build fixtures are authenticated against their frozen inventories;
no server or build is started. Exports are created only in test temporary
directories, never in the frozen evidence trees.

The final scan-chunk release build passes at
`/tmp/grust-wedge-chunk-release-build-20260905-02` (81.716 seconds). Its immutable
profiler is `/tmp/grust-memory-post-scan-chunks-20260905.jopbYa/profile_memory`,
SHA-256 `8d68acc6d2babc23f5e7b3b9d9a3729f61198cd250d7926434a684df557a5cb6`.
The preceding single-pass executable and all earlier diagnostics remain intact.

All 22 pinned counts pass with the scan-chunk executable at both scales:
`/tmp/grust-memory-all-post-scan-chunks-sf01-20260905-01` (6.572 seconds total)
and `/tmp/grust-memory-all-post-scan-chunks-sf03-20260905-01` (17.766 seconds).
The repeated q6 SF0.3 diagnostic at
`/tmp/grust-memory-q6-post-scan-chunks-sf03-20260905-01` passes all 2,400
repetitions in 38.764 seconds total. Its successful query-phase sample is
`/tmp/grust-memory-q6-post-scan-chunks-query-sample-20260905-01/q6.sample.txt`,
attached after iteration 43 starts. The sampled query work remains concentrated
in grouped traversal, TLS/budget access and `CenterTotals::add_group`; the
waiting main/deadline threads are not query CPU. Raw sample tallies and totals
across different repeat counts are not speedup ratios.

All these records remain explicit non-publication native diagnostics, with
per-iteration count/oracle/timing output and no build/test overlap. The retained
host screen `/tmp/grust-count-host-preflight-20260905-07.json` fails all three
samples (974.4%, 965.3%, 963.7% aggregate CPU); the same four unrelated embedding
workers are untouched. All owned build, test, lint, diagnostic and sampler
sessions are terminal and their known PIDs were checked absent. Frozen evidence,
upstream data, lockfiles and generated book artifacts are unchanged. No release
or qualified comparison has been performed, and the goal remains active.

The next accumulator hypothesis has an independent domain-bound proof. For one
center, grouped multiplicities partition incident physical T edges once, so
`degree_a` and `m` are at most `E_T`. Outgoing U leaf counts are at most `E_U`.
Thus `sum_C(m * L) <= E_T * E_U`; distinct T/U types and the global u32 edge
bound imply a weighted total below 2^62. Group multiplicity/degree can therefore
use checked u32, weighted totals checked u64, with u128 overlap, final products,
subtraction and total count. Keep raw scan indices/totals as checked usize:
incoming/outgoing loop copies can require `2 * E_T` scanned slots.
The narrowing's subsequent implementation and verification are recorded below.
Retaining the shared u64 leaf array isolates this change to q6; narrowing that
array separately would save `4V` bytes but also touch q9 and its allocation-budget
expectations.

A separate q1 review proposes testing necessary typed-adjacency existence before
property evaluation only for property-bearing roles with at least two mandatory
incident atoms. The insertion point is after borrowing role candidates and
before `node_matches`; all correctly oriented incident rows must exist, and all
original predicates still run for survivors. Optional adjacency must never
qualify a mandatory candidate. Actual type-hash lookups need their own work
charges; there is no edge-slot scan or candidate allocation in this prefilter.
This is an unimplemented performance hypothesis, not an AST guarantee of
selectivity. It leaves degree-one q4/a3 property leaves and property-free q7/a4
cores unchanged, but dense internal property roles can still regress. q1/a1,
reversed/split/optional forms, q4/q7 controls, and synthetic sparse/dense chains
need independent oracle, budget and profile checks before adopting it. Broader
or single-edge selection would require trustworthy selectivity metadata.

## Narrow accumulator follow-up

The next goal turn implements the proven q6 widths: checked u32 group
multiplicity/degree, checked u64 weighted leaves, and checked u128 overlap,
final product/subtraction and accumulated count. The shared u64 leaf array,
q9 support path, heap allocations and successful work totals are unchanged.

Grouping records outgoing/incoming start offsets, drains the same charged
physical slots, and computes multiplicity once from the consumed span lengths.
Only the incoming self-loop span is excluded. Checked usize addition followed
by u32 conversion enforces the index bound at that boundary. Raw scan indices
and 256-slot chunk accounting remain usize. Independent review found no
correctness blocker; final scalar conversion still precedes SKIP/LIMIT output
suppression. Singleton-heavy groups may trade counter increments for extra
span/conversion work, so benefit remains a profiling question.

The tests cover 5,832 three-group combinations against an independent per-C
formula, globally valid large-product cancellation, compact multiplicity and
accumulator boundaries, exact budgets, and final i64 rejection before pagination.
The engine/adapter gate passes 894 tests with eight ignored checks at
`/tmp/grust-wedge-narrow-full-tests-20260905-01` (50.044 seconds). The runner
passes 118 tests at `/tmp/grust-wedge-narrow-runner-tests-20260905-01`.
Strict engine and runner Clippy pass at
`/tmp/grust-wedge-narrow-clippy-20260905-01` and
`/tmp/grust-wedge-narrow-runner-clippy-20260905-01`; the latter also retains both
formatting checks and `git diff --check`.

The release build passes at `/tmp/grust-wedge-narrow-release-build-20260905-01`
(78.250 seconds). The retained executable is
`/tmp/grust-memory-post-narrow-wedge-20260905.j8we1n/profile_memory`, SHA-256
`cda94ffd12100bc810174840f2a909e7743a2cd0eaefee27d38d2ff82a7ff818`.
The immutable scan-chunk binary above remains the pre-change baseline.

All 22 pinned counts pass at both scales:
`/tmp/grust-memory-all-post-narrow-wedge-sf01-20260905-01` (7.221 seconds total)
and `/tmp/grust-memory-all-post-narrow-wedge-sf03-20260905-01` (17.506 seconds).
The repeated q6 SF0.3 diagnostic at
`/tmp/grust-memory-q6-post-narrow-wedge-sf03-20260905-01` passes all 2,400
repetitions (36.152 seconds total). Its successful query-phase sample is
`/tmp/grust-memory-q6-post-narrow-wedge-query-sample-20260905-01/q6.sample.txt`,
attached after iteration 32 starts. Grouped traversal and TLS/budget access
still dominate sampled query work; the accumulator remains visible. These raw
stack tallies do not establish a comparative improvement.

The retained host screen `/tmp/grust-count-host-preflight-20260905-08.json`
fails all three samples (939.3%, 966.9%, 966.3% aggregate CPU). All runs remain
explicit native non-publication diagnostics with per-iteration oracle checks,
not timing cohorts or a Neo4j comparison. The four unrelated embedding workers
are untouched. All owned build, test, lint, query and sampler sessions for this
slice are terminal, and their known PIDs were checked absent. Frozen evidence,
upstream inputs, lockfiles and generated book artifacts remain unchanged.

The subsequent implementation/test/profile slice is the narrowly gated
[mandatory-adjacency prefilter](GRUST_SPEED_ADJACENCY.md) described above. It was
not present in the retained narrow-wedge executable or these diagnostics.
