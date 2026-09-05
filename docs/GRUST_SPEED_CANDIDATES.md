# Candidate traversal optimization

Updated 2026-09-05. This follows the verified rank-space pass in the
[speed ledger](GRUST_SPEED_PROGRESS.md). The prior goal turn made concrete
progress through source changes, test gates and retained diagnostics. This is
continuing optimization work, not release or benchmark-completion evidence.

## Implemented invariants

Wedges retain the B/C required-label slices obtained while preparing role
masks. A private range/slice iterator visits all vertices for an unlabeled
role, or borrowed candidates for a labeled one. There is no candidate Vec,
dynamic dispatch or repeated lookup. C leaf traversal and non-anti B center
traversal still check complete masks and charge each visited candidate. The
full-V mask/leaf initialization and allocations, edge multiplicity arithmetic
and q9 active-domain scan are unchanged.

Mandatory forest branch combination also reuses its already-borrowed seed
label candidates. Outside-seed weights start at zero; optional padding and
prior branch products cannot revive them. Every label/property predicate is
still evaluated in the original predicate pass. Unlabeled roles scan V;
weight initialization, optional scans, root sums and all typed-edge direction,
self-loop and multiplicity logic remain unchanged. No mandatory-adjacency
property prefilter has been added.

Independent review found no correctness/accounting blocker in either change.
The combined engine/adapter gate passes 880 tests with eight ignored checks at
`/tmp/grust-candidate-full-tests-20260905-01`.
Strict engine/adapter Clippy passes at `/tmp/grust-candidate-clippy-20260905-01`.
The LSQB runner passes all 118 tests at
`/tmp/grust-candidate-lsqb-tests-20260905-01`, including plan-registry parity and
real Memory/Turso workers. Runner strict Clippy passes at
`/tmp/grust-candidate-runner-clippy-20260905-01`. Workspace/runner formatting
checks pass. Large-scale diagnostics remain separate from these offline gates.
Tests cover all 16 wedge label selections,
missing/conflicting labels, borrowed candidate domains, exact work/bytes and
per-candidate refusals. Four new forest tests cover full-size initialization,
empty domains, charged zero-degree candidates, later label/property predicates,
optional padding and multigraph/reference parity for all nine direction pairs.

## Retained baseline

The pre-change rank profiler remains immutable at
`/tmp/grust-memory-post-rank-20260905.XUdLPx/profile_memory`, SHA-256
`7c7b7c4914c2c805a7c691d3a59d7b409ff10ef44086594a74035d625c6bf03a`.

- q6: 600 correct SF0.3 repetitions at
  `/tmp/grust-memory-q6-pre-candidates-sf03-20260905-01`.
- q1: 60 correct SF0.3 repetitions at
  `/tmp/grust-memory-q1-pre-branch-candidates-sf03-20260905-01`.
  The attempted sample at
  `/tmp/grust-memory-q1-pre-branch-candidates-query-sample-20260905-01`
  exited 255 because the target had already finished. It is retained, excluded
  from hot-path analysis, and did not leave a process running.
- A fresh 120-repeat q1 run finishes at
  `/tmp/grust-memory-q1-pre-branch-candidates-resampled-sf03-20260905-01`.
  Its successful early query-phase sample is
  `/tmp/grust-memory-q1-pre-branch-candidates-query-sample-20260905-02/q1.sample.txt`.
  Property/string comparison and accounting remain visible. Main/deadline
  thread waits are not query CPU work.

All baseline query/sample processes are terminal and their known PIDs are gone.
Builds and measured queries do not overlap. These are native load-once
diagnostics under known host contention, with per-iteration oracle/timing
records and explicit `publication_eligible: false`, not qualified comparisons.

## Qualification readiness gaps

Both historical native Neo4j timing cohorts remain explicitly excluded by
[NEO4J.md](../benchmarks/lsqb/NEO4J.md) and the
[incident record](../benchmarks/lsqb/PERFORMANCE-INCIDENT-2026-09-05.md).
Their counts/provenance may be retained, but fresh quiet-host timings are
required at both scales. No frozen bundle is modified by this work.

The canonical matrix still requires the complete 12-backend rectangle; a
Memory-only smoke run is not publication evidence. A separately scoped matched
performance diagnostic can use the existing container/worker lifecycle, but
needs explicit audit/summary and sanitized runtime/host capture. Do not weaken
the canonical receipt or manufacture missing backend cells.

The readiness audit found an integration gap: `run-grust.sh` writes
`host-preflight.json`, but the publication validator's exact inventory does not
admit it in historical manifests. The subsequent
[single-pass/binding slice](GRUST_SPEED_SINGLE_PASS.md) adds an explicit new
manifest contract and required, validated, hash-bound startup evidence while
preserving historical layouts. Native bundle export still omits host preflight
evidence; startup records do not prove ongoing host isolation.

Before a qualified comparison: freeze and authenticate Memory source/image,
verify dataset receipts and read-only snapshots, pass host isolation, establish
6-GiB SF0.3 fit/READY limits without measurement, then run rotating W2/R10
cohorts sequentially with all 22 outcomes and separate query/setup/recovery/load
records. Disclose Memory's per-worker reload/index build versus Neo4j's retained
server, scalar-consumption/rollback boundary, and actual client/server resource
limits; separately capped containers are not an identical aggregate envelope.
No new servers, containers or qualified cohorts were started in this pass.

## Candidate executable and measurements

The optimized profiler build passes at
`/tmp/grust-candidate-release-build-20260905-01`. Its executable is retained at
`/tmp/grust-memory-post-candidates-20260905.0beMtO/profile_memory`, SHA-256
`cc0a462be5275adb67b30c2d73f7a33f488f68c0e1bf2af3f105c8db87fa129d`.
All 22 pinned counts match at both scales:
`/tmp/grust-memory-all-post-candidates-sf01-20260905-01` and
`/tmp/grust-memory-all-post-candidates-sf03-20260905-01`.
Total diagnostic wall times, including setup/teardown, are 7.164 and 16.506
seconds, not query-latency comparisons.

Repeated SF0.3 checks also finish with every oracle/count/timing retained:

- q6: 1,200 repetitions at
  `/tmp/grust-memory-q6-post-candidates-sf03-20260905-01` (35.767 seconds total),
  with a successful query-phase sample at
  `/tmp/grust-memory-q6-post-candidates-query-sample-20260905-01/q6.sample.txt`.
  Grouped-edge traversal and budget/TLS accounting now dominate the sampled
  query work. Different repetition counts and sampling windows are not a
  matched timing cohort.
- q9: 400 repetitions at
  `/tmp/grust-memory-q9-post-candidates-sf03-20260905-01` (41.024 seconds total).
  No new q9 stack sample was attached in this pass; the rank-era sample remains
  the last direct stack evidence for its support-triangle work.
- q1: 120 repetitions at
  `/tmp/grust-memory-q1-post-branch-candidates-sf03-20260905-01`
  (90.323 seconds total), with an early query-phase sample at
  `/tmp/grust-memory-q1-post-branch-candidates-query-sample-20260905-01/q1.sample.txt`.
  Property-map/string comparisons still dominate. Branch combination is
  smaller in the sample, but raw stack tallies are not a speedup ratio.

All query, build, test and sampler sessions in this pass are terminal, with
their known process IDs checked absent. No detached monitor was started.
The source remains uncommitted; frozen bundles, lockfiles and generated book
artifacts are unchanged. Release/package/publication gates remain pending.

The post-build host screen at
`/tmp/grust-count-host-preflight-20260905-05.json` fails all three samples
(974.8%, 970.3%, 967.3% aggregate CPU), with the same four unrelated embedding
workers. They remain untouched. Docker reports 10 CPUs and 41,987,035,136 bytes
RAM; no resource settings were changed.

## Next evidence-led work

Validate the new host-artifact contract before any new canonical publication
run. A separately scoped Memory/Neo4j performance lane must retain
honest lifecycle/resource boundaries and must not relax the full matrix rules.

For q6, the sample now motivates eliminating the second grouped-neighbor pass.
The existing per-center formula can be rearranged as
`degree_a * weighted_leaves - overlap`, where `weighted_leaves` sums `m(b,c)*L(c)`
over C candidates, and `overlap` sums `m(b,c)^2*L(c)` over A∩C candidates.
All three totals can be accumulated in one merge of physical slots. This
preserves the a≠c exclusion, loops and parallel/reciprocal multiplicities;
keep checked u128 arithmetic and independent raw-edge oracles. It is not
part of the candidate executable above; its subsequent implementation and gates
are tracked in the [single-pass slice](GRUST_SPEED_SINGLE_PASS.md). It leaves
q9's support algorithm unchanged.

For q1, investigate rejecting candidates that lack a selected mandatory typed
adjacency before property evaluation. Do not use optional edges or assume
functional relationships. Keep every predicate for surviving candidates and
charge lookup work. Dense relationships may make the prefilter slower; measure
q1/q4/q7 and reversed/split/optional forest forms separately. No such prefilter
or property-value index is currently implemented.
