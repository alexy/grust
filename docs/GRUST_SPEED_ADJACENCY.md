# Mandatory-adjacency prefilter experiment

Updated 2026-09-05. This continues the
[narrow-wedge diagnostics](GRUST_SPEED_SINGLE_PASS.md). It is an implementation
and verification ledger, not release completion or qualified benchmark evidence.

## Hypothesis and scope

The q1 and reversed-chain samples place much of query CPU work in property-map
lookups and string comparisons. Before evaluating a property-bearing role with
at least two mandatory incident atoms, test whether every correctly oriented
typed adjacency row is nonempty. Missing any required edge proves its weight
zero regardless of labels or properties. Undirected atoms try outgoing first
and incoming only on a miss. Optional leaves never participate.

The implementation borrows the existing index and AST. Each actual type lookup
is charged before access, with no edge-slot scan or candidate allocation. All
original predicates and branch calculations remain for survivors. Full weight
array initialization and optional/root accounting remain. Degree-one and
property-free roles do not enable the filter; no query-name shortcut is used.
This is a structural heuristic, not a selectivity estimate. Dense roles can pay
extra lookups without rejecting candidates, and ordinary predicates still decide
which existing edges and endpoints actually match.

## Pre-change evidence

The immutable baseline is
`/tmp/grust-memory-post-narrow-wedge-20260905.j8we1n/profile_memory`, SHA-256
`cda94ffd12100bc810174840f2a909e7743a2cd0eaefee27d38d2ff82a7ff818`.
Its all-22 SF0.1/SF0.3 diagnostics and gates are recorded in the preceding ledger.
The earlier q1 profile at
`/tmp/grust-memory-q1-post-branch-candidates-query-sample-20260905-01/q1.sample.txt`
also predates this filter; intervening wedge changes do not change the q1 path.

The reversed-chain baseline passes all 60 repetitions at
`/tmp/grust-memory-a1-pre-adjacency-prefilter-sf03-20260905-02` (51.540 seconds
total, including loading, logging and teardown). Its successful query-phase
sample at
`/tmp/grust-memory-a1-pre-adjacency-prefilter-query-sample-20260905-01/a1.sample.txt`
attaches after iteration 13 starts. Property-map lookups and string comparisons
dominate query leaf samples. Waiting threads are not query CPU work.

Attempt `...sf03-20260905-01` used the nonexistent query ID `a1` and exited 1
before query execution. It is retained as a failed attempt, not benchmark data;
the successful retry uses the pinned ID `a1-reversed-chain`. All baseline query
and sampler jobs are terminal, with their known PIDs checked absent.

Host screen 08 still fails, as recorded in the preceding ledger. These are
explicit non-publication native diagnostics, with oracle checks on every
iteration; no timing ratio, clean-host claim or Neo4j win follows from them.
Frozen evidence, upstream inputs and generated book artifacts remain unchanged.

## Verification

Independent review found no correctness or accounting blocker. Nine focused
tests include 24 generated multigraphs in all nine direction pairs and 36
direction/endpoint placements, checked against independent physical-edge counts
and the reference executor. Other checks cover all mandatory atoms, repeated
mentions, edge/neighbor/property predicates, optional padding, disabled roles,
full weight initialization, exact lookup costs, long types and in-helper
deadline expiry without sleeps or spin loops. First-miss and all-present lookup
costs are pinned; a second-atom-missing exact-cost assertion remains a nonblocking
coverage improvement.

The engine/adapter gate passes 903 tests with eight ignored checks at
`/tmp/grust-adjacency-prefilter-full-tests-20260905-01` (54.269 seconds).
Strict engine Clippy passes at
`/tmp/grust-adjacency-prefilter-clippy-20260905-01`. The combined runner gate at
`/tmp/grust-adjacency-prefilter-runner-gates-20260905-01` passes 118 tests,
strict Clippy, both formatting checks and `git diff --check` (37.310 seconds).

The release build at `/tmp/grust-adjacency-prefilter-release-build-20260905-01`
passes in 82.138 seconds. The immutable executable is
`/tmp/grust-memory-post-adjacency-prefilter-20260905.KFslkr/profile_memory`, SHA-256
`86fbde402c4b36ee377dfdbef5b67bcc700abe907ef720b88b10450ef5a446e5`.
All 22 pinned counts pass at both scales:
`/tmp/grust-memory-all-post-adjacency-prefilter-sf01-20260905-01`
(7.111 seconds total) and
`/tmp/grust-memory-all-post-adjacency-prefilter-sf03-20260905-01`
(16.841 seconds total). This includes the degree-one/property-free controls as
well as the affected q1/a1 paths. Totals include load/logging/teardown and are
not timing comparisons.

Both post-change SF0.3 repeated diagnostics pass all 60 oracle checks:

- q1: `/tmp/grust-memory-q1-post-adjacency-prefilter-sf03-20260905-01`,
  32.843 seconds total. Sample:
  `/tmp/grust-memory-q1-post-adjacency-prefilter-query-sample-20260905-01/q1.sample.txt`,
  attached after iteration 5 starts.
- Reversed chain:
  `/tmp/grust-memory-a1-post-adjacency-prefilter-sf03-20260905-01`,
  31.118 seconds total. Sample:
  `/tmp/grust-memory-a1-post-adjacency-prefilter-query-sample-20260905-01/a1.sample.txt`,
  attached after iteration 3 starts.

In both query-phase samples, property lookup/string comparison is less prominent
than in the prefilter-free profiles. Repeated type hashing, adjacency lookup,
the prefilter and forest traversal now dominate sampled query work. This supports
the intended change in work distribution, not a qualified latency ratio under
the still-contended host. No build/test overlapped either diagnostic. All owned
build, test, lint, query and sampler sessions are terminal; known PIDs are checked
absent before handoff. No detached monitor or new container was started.

## Next work

Investigate a small borrowed typed-adjacency view, resolving each mandatory atom's
type once instead of hashing it for every candidate. Keep the standard randomized
hasher, snapshot lifetime, dense/sparse representation and empty/invalid-slot
behavior; account preparation, retained handles and per-row probes separately.
Limit the first integration to this prefilter rather than changing every reader.
This is a new profile-led hypothesis, not implemented in the executable above.
The independent read-only proposal is a `Copy` borrowed `TypedAdjacencyView`
wrapping an optional reference to the private typed rows. One index lookup
creates the view; its incoming/outgoing methods delegate to the unchanged CSR
lookup, with absent types returning empty rows. An enabled role can retain a
charged O(incident-atoms) vector of views and role-relative directions. Preserve
the zero-allocation disabled path and explicitly test preparation-byte limits,
one-time hash charges, per-probe deadlines and missing/invalid vertex behavior.
The subsequent implementation is recorded below.
Dense/sparse synthetic performance controls still need assessment before general
performance claims, even though the correctness oracles include both sparse
missing-edge and multigraph cases.

This goal turn made implementation/test/profile progress. The full objective
remains active: fresh qualified container comparisons, ongoing host isolation,
6-GiB resource-fit evidence, clean source/image provenance and release/book/
publication gates are still required. Neither a Neo4j win nor diminishing returns
has been established. Frozen evidence, upstream inputs, lockfiles and generated
book artifacts remain unchanged; source and manuscript edits are uncommitted.

## Prepared borrowed views follow-up

The next goal turn classifies the preceding turn as progress and implements
`TypedGraphIndex::adjacency` plus public/root-prelude `TypedAdjacencyView`.
The copyable view wraps an optional borrowed reference to the existing typed
rows. Incoming/outgoing accessors retain dense/sparse lookup and return slices
with the index lifetime, independent of the type string or temporary view.
Missing types and invalid vertex slots remain empty. Randomized hashing, CSR
construction, physical edge identity and snapshot ownership are unchanged.

Only the mandatory prefilter caches these views in this first integration.
Each enabled role with candidates prepares a vector of view/direction pairs;
disabled or empty-candidate roles allocate and resolve nothing. Preparation
precharges the vector payload and each type hash/comparison once. Each actual
row probe then costs one work unit and checks the deadline, including absent
rows; undirected incoming probes happen only after outgoing misses. Original
node predicates, child-branch traversal and optional evaluation remain unchanged.
There is no candidate/edge-slot copy or uncharged work loop. Independent review
found no production correctness or accounting blocker.

Eight new core tests cover direct-scan and pointer parity for dense/sparse rows,
physical identities, missing/empty/invalid rows, short-lived type strings,
copy-on-write snapshot ownership, public exports and generated multigraphs. A
compile-fail rustdoc demonstrates that a view cannot escape its index lifetime.
The semantic prefilter tests retain their raw/reference oracles. A separate
budget suite covers exact preparation bytes/work, repeated view reuse, long
types, undirected short-circuiting, the previously missing second-atom-miss case,
disabled/empty candidate paths and deadline expiry inside the helper.

The release build passes at `/tmp/grust-adjacency-view-release-build-20260905-01`
(92.623 seconds). Its retained executable is
`/tmp/grust-memory-post-adjacency-view-20260905.mqFQ3W/profile_memory`, SHA-256
`1ee9cbd57750061f021da78a4291355a29382ec6693a1149fc98ec12ebce6d26`.
The preceding prefilter executable remains immutable for comparison. The full
gate at `/tmp/grust-adjacency-view-full-tests-20260905-01` passes 1,013
core/Memory/engine/adapter tests (eight ignored checks), plus the core lifetime
compile-fail doctest (139.818 seconds total). This comprises 106 core/Memory
tests and 907 engine/adapter tests. The runner passes 118 tests at
`/tmp/grust-adjacency-view-runner-tests-20260905-01` (79.879 seconds).
Strict core/Memory/engine Clippy passes at
`/tmp/grust-adjacency-view-clippy-20260905-01`; runner Clippy, both formatting
checks and `git diff --check` pass at
`/tmp/grust-adjacency-view-runner-final-gates-20260905-01`.

All 22 pinned counts pass at both scales:
`/tmp/grust-memory-all-post-adjacency-view-sf01-20260905-01`
(7.211 seconds total) and
`/tmp/grust-memory-all-post-adjacency-view-sf03-20260905-01`
(18.387 seconds total). The repeated SF0.3 diagnostics each pass all 120 oracle
checks: q1 at `/tmp/grust-memory-q1-post-adjacency-view-sf03-20260905-01`
(40.714 seconds total), and reversed chain at
`/tmp/grust-memory-a1-post-adjacency-view-sf03-20260905-01`
(39.191 seconds total). Successful query-phase samples attach after iterations
6 and 23 respectively:
`/tmp/grust-memory-q1-post-adjacency-view-query-sample-20260905-01/q1.sample.txt`
and `/tmp/grust-memory-a1-post-adjacency-view-query-sample-20260905-01/a1.sample.txt`.
Type hashing is much less prominent; forest traversal, row lookup and budget/TLS
access now dominate query samples. Repeat counts differ from the preceding
60-iteration runs, totals include setup/logging/teardown, and contention remains:
none of these numbers is a qualified speedup ratio. All owned build, test, lint,
query and sampler sessions for this Rust slice are terminal, with known PIDs
checked absent. Frozen inputs/evidence and generated book artifacts are unchanged.

Startup host screen `/tmp/grust-count-host-preflight-20260905-09.json` still
fails (891.1%, 960.4%, 966.1% aggregate CPU). The same four unrelated embedding
workers remain untouched. A request for a quiet measurement window is pending;
no qualified cohort is started or host gate bypassed.

The next structural hypothesis is to seed enabled roles from a strictly shorter
borrowed sparse active-source slice for one mandatory directed atom. Sparse CSR
already owns sorted unique source slots; dense rows would expose no cheap list,
and absent types would expose an empty list. Retain label/property predicates,
all mandatory probes, full-domain charges, and optional/branch zero invariance.
Keep label candidates on equal lengths and skip undirected seed selection in
the first implementation. Charge each inspected atom's metadata without copying
or intersecting candidate lists. This reduces candidate count rather than merely
making each binary search slightly cheaper. The following slice implements it.

## Sparse source candidate follow-up

The core view now exposes borrowed outgoing/incoming sparse source slices.
These are the existing sorted unique nonempty-row slots: no index storage,
construction or edge multiplicity changes. Dense storage returns `None` and an
absent type returns `Some(&[])`. Five focused core tests check backing-slice
identity, nonempty-row equivalence, threshold behavior, physical multigraph rows,
missing types and lifetimes independent of temporary views/type strings.

Enabled forest roles compare each prepared mandatory directed source list to
the current label/full-domain seed and retain only strictly shorter choices.
Ties preserve the existing slice; undirected and dense atoms provide no seed.
Each inspected atom consumes one metadata work unit, including after an empty
seed wins. Every original row probe and label/property predicate remains for
survivors. Branch combination reuses the chosen slice; full-V initialization,
optional traversal and root summation retain their charges. Selection allocates
no storage and never materializes an intersection.

Ten engine tests cover strict-shorter/tie pointer identity, role reversal,
absent/dense/undirected atoms, labels and property conflicts, other mandatory
checks, physical multiplicity, optional padding and exact budget phases. Twelve
sparse multigraphs across six direction combinations agree with an independent
weighted raw-edge oracle and both reference/indexed execution. Adding isolated
label candidates must cost exactly four times the added vertex count for three
weight arrays and one root sum, without extra candidate or branch visits.
Independent review found no semantic, lifetime, budget or readability blocker.

The optimized build passes at `/tmp/grust-sparse-seed-release-build-20260905-01`
(95.594 seconds). The retained binary is
`/tmp/grust-memory-post-sparse-seed-20260905.fj897f/profile_memory`, SHA-256
`740d54949848e4d0330586e24feef31aec2a884a9a54862dfc59e210fe079001`.
The full gate `/tmp/grust-sparse-seed-full-tests-20260905-01` passes 1,028
library/integration tests (111 core/Memory and 917 engine/adapter), eight ignored
checks, and the core lifetime doctest (77.678 seconds). The runner passes all
118 tests at `/tmp/grust-sparse-seed-runner-tests-20260905-01` (35.532 seconds).
Strict Clippy passes for core/Memory/engine/adapters at
`/tmp/grust-sparse-seed-clippy-20260905-01` (16.820 seconds); runner Clippy and
both formatting/diff checks pass at
`/tmp/grust-sparse-seed-runner-final-gates-20260905-01` (10.509 seconds).
All build/test/lint sessions are terminal. All 22 pinned counts pass at SF0.1
and SF0.3 in `/tmp/grust-memory-all-post-sparse-seed-sf01-20260905-01`
(6.772 seconds total) and `/tmp/grust-memory-all-post-sparse-seed-sf03-20260905-01`
(17.634 seconds total). The SF0.3 native peak footprint is 3,481,116,608 bytes;
this does not prove a Linux container fits its 6-GiB limit.

Both SF0.3 repeated diagnostics complete all 120 oracle-checked iterations:
`/tmp/grust-memory-q1-post-sparse-seed-sf03-20260905-01` (38.386 seconds total)
and `/tmp/grust-memory-a1-post-sparse-seed-sf03-20260905-01` (38.881 seconds).
The query-phase samples attach at iterations 6 and 22, respectively:
`/tmp/grust-memory-q1-post-sparse-seed-query-sample-20260905-01/q1.sample.txt`
and `/tmp/grust-memory-a1-post-sparse-seed-query-sample-20260905-01/a1.sample.txt`.
In q1, query leaves include TLS lookup (925), forest execution (691), budget
LocalKey access (453), CSR row lookup (405), mandatory probes (372) and the
budget closure (258). Reversed-chain leaves include forest execution (779),
mandatory probes (643), the budget closure (510), TLS lookup (494), LocalKey
access (424), predicates (261) and CSR row lookup (258). Idle thread waits are
not query CPU work. Raw tallies and setup-inclusive diagnostic totals are not
latency ratios or clean-host speedup evidence.

All owned build, test, lint, query and sampler sessions in this slice are
terminal and known PIDs are absent. Frozen Neo4j/upstream bundles, lockfiles
and generated book artifacts remain untouched. Source/release qualification
and Linux resource-fit verification are still pending.

The next read-only-reviewed hypothesis is bounded prepayment of mandatory
child-branch neighbor scans: borrow `neighbors.chunks(256)` and charge each
chunk's physical length before the unchanged loop/predicate/count body. This
is implemented in the following slice. Success must retain exact slot totals, including
skipped incoming copies of undirected loops; early refusal may reserve up to
255 later slots before a predicate error, as documented for the wedge helper.
Test boundary lengths through 513, partial tails, rejected predicates, zero
child weights, no added allocation and deadline cadence. Optional execution,
candidate checks and full-domain charges must remain unchanged.

Host screen `/tmp/grust-count-host-preflight-20260905-10.json` fails all three
samples (968.3%, 975.2%, 974.7% aggregate CPU). The same four unrelated `et`
workers remain untouched. This is not clean-host performance evidence, and no
comparison qualification is inferred from source-level improvements.

## Mandatory edge-scan prepayment follow-up

Mandatory child-branch scans now borrow chunks of at most 256 physical slots
and precharge each chunk before the unchanged loop exclusion, property check
and capped addition. No scratch buffer or generic batching abstraction is added.
All slots still count toward successful scan work, including incoming copies
of undirected loops; counts include each physical loop only once. Empty rows
add no scan charge, and OPTIONAL retains its original per-edge accounting.

Nine focused tests cover 0/1/255/256/257/511/512/513-slot boundaries in both
directions; exact successful work and unchanged bytes; refused first, second
and partial-tail reservations; predicates failing after a prepaid chunk; zero
child weights; independent raw multigraph counts; OPTIONAL accounting/padding;
active deadlines; and scalar aliases/SKIP/LIMIT. The three-node, N-parallel-edge
chain consumes exactly 36 + N work units on success, including at chunk tails.
Errors propagate through the indexed entrypoint without an unbounded retry.

The optimized build at `/tmp/grust-tree-chunk-release-build-20260905-01` passes
(76.608 seconds). The retained executable is
`/tmp/grust-memory-post-tree-chunk-20260905.JZiZmr/profile_memory`, SHA-256
`62971f746dea06b093104bac05f4359b660af5348d6a1e13e5465c01e5642916`.
The full gate at `/tmp/grust-tree-chunk-full-tests-20260905-01` passes 1,037
library/integration tests (111 core/Memory and 926 engine/adapter), eight ignored
checks, and one lifetime doctest (58.635 seconds). The runner passes 118 tests
at `/tmp/grust-tree-chunk-runner-tests-20260905-01` (27.472 seconds). Lint and
query-profile verification are pending for this slice.

## Docker readiness and cancellation audit

A read-only inventory finds an ARM64 Docker VM with 10 CPUs and 41,987,035,136
bytes of RAM, approximately 234 GiB free host disk, and three existing running
containers. The cached Rust Bookworm image is
`sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97`.
It is not the canonical Trixie recipe. A named Cargo container could build the
profiling example without detached BuildKit work, then a separate 8-CPU/6-GiB
container could test resource fit. Source freezing, cache/native-dependency
checks, immutable Linux executable identity and actual limit inspection are
still required; no container was started by this audit.

Root review found and repaired a prerequisite safety gap in `cell-watchdog.py`:
error/exception paths previously stopped only the CLI group, and default SIGTERM
could bypass cleanup. Signals now latch through spawning; controlled cleanup
reaps the owned CLI group before late discovery, pins one immutable container
ID, and revalidates its name and Compose labels for kill/removal. Mismatched
discovery is never retried, replacement IDs are never adopted, and cleanup
failures remain errors. Unexpected exceptions clean up then re-raise.

All 16 existing watchdog checks and 12 new interruption checks pass at
`/tmp/grust-watchdog-offline-20260905.Jygn1v/direct-tests.log`. The initial
unittest discovery invocation matched no hyphenated modules (exit 5); it is
retained separately and is not counted as verification. Direct execution of
both files passed. No real Docker commands or containers were used.

The outer logger now also latches spawn/cleanup signals, always cleans residual
owned group members after leader exit, restores partially installed handlers,
and records interruption after late completion signals. Its default grace is
still five seconds; `--termination-grace-seconds 60` allows nested watchdog
cleanup before escalation. Fourteen logger tests pass at
`/tmp/grust-command-progress-cancellation-tests-20260905-01` (0.438 seconds).
The real-signal nested integration test passes both SIGTERM and SIGINT cases at
`/tmp/grust-nested-cancellation-tests-20260905-01` (2.127 seconds). It runs the
actual logger/watchdog entrypoints with fake Docker, checks both child layers
and all three owned process groups are gone, preserves terminal records, and
verifies only the fake target was removed. The unrelated fake container is
unchanged; no real Docker container was started or removed.

The one-shot absent lookup cannot exclude Docker daemon creation after CLI
termination. New diagnostic flows must create/attest a stopped container before
starting it. Configured cleanup timeouts total about 49 seconds plus an in-flight
Docker call and record I/O; SIGKILL, daemon failure and blocked I/O remain
limitations, not cleanup promises. No current orphan was discovered or Docker
work started by this audit.
