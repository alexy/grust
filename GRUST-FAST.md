# GRUST-FAST: the plan to make the Grust read executor fast on pattern counts

Written 2026-09-05 for the Grust maintainers and the Codex session running
`benchmarks/lsqb`. Companion to `BENCHMARK-REVIEW.md`. Based on a read-only
analysis of `grust-cypher/src/read.rs` and `grust-core::GraphIndex` at
`24a6277`. The measure of success is the LSQB SF0.1 and SF0.3 Memory cells in
`benchmarks/lsqb`: every query admitted, every scalar matching the oracle, and
per-query medians in the range of the reference timings the LSQB repository
ships with its expected-output file.

Implementation status and qualification caveats are tracked separately in
[`docs/GRUST_SPEED_PROGRESS.md`](docs/GRUST_SPEED_PROGRESS.md). The estimates
and sketches below are the original proposal, not measured speedup claims.

## 0. Acceptance criteria

1. All nine LSQB queries and all thirteen count attacks run on the Memory
   cell at SF0.1 and SF0.3 with no `performance.rust-row-limit` refusals.
2. Every scalar equals the upstream oracle and the existing clause-by-clause
   executor on `sfexample`, on the attack set, and on randomized small graphs
   (differential test, run in CI).
3. Median query time on the SF0.1 Memory cell is reported for all 22 cases
   under the 8-CPU / 6 GiB container envelope and the W2/R10 rotating
   protocol already used by the published cohorts, so the results are
   comparable with every other cohort on the site without re-measurement.
4. The `ReadQueryPolicy` bounds (candidate work, intermediate bytes, hops,
   rows, timeout) still apply and are still charged; a faster plan is not a
   way around the policy.
5. No change to the parser, the AST, the mutation planner, or the public
   `GraphStore` trait.

## 1. Where the time goes today

The published SF0.1 evidence (432,235 nodes, 2,080,404 edges) has native
Neo4j 2026.07.1 passing all nine LSQB queries with medians between 0.35 s (q2)
and 3.6 s (q9), while the Grust Memory reference runs q4 in 16.5 s, times out
on q2 at 60 s, and refuses the other seven under the 1,000,000-row admission
gate. The upstream LSQB `expected-output.csv` also carries the reference
system's timings at SF0.1: q1 in 0.042 s, q6 (55,607,896 result rows) in
0.023 s, q4 in 0.028 s. Those numbers are the real target. They are only
possible if the engine never materializes a row per match.

The reference executor in `read.rs` does four things that make LSQB slow, and
none of them is a small constant:

1. **`Row = BTreeMap<String, Bound>` with `Bound::Node(Node)` by value**
   (`read.rs:37–44`). Every expansion step clones the whole row, including
   every bound `Node` (id string, label, props map). A q1 chain binds up to
   ten nodes; at 8.7 million final matches for SF0.1 that is tens of millions
   of `BTreeMap` and `Node` clones before `count(*)` ever runs.
2. **No label index.** `node_candidates` (`read.rs:2080`) scans
   `graph.nodes` and calls `node_matches` on every node for every unbound
   pattern start, once per incoming row. q3 starts three separate `MATCH`
   clauses from `(personN:Person)` under a bound `country`; each of those is a
   full 432,235-node scan per country row, 1,343 times over.
3. **Clause-by-clause nested loops with no join ordering.** Multi-pattern
   `MATCH` and successive `MATCH` clauses expand in source order from the
   pattern start and only check the far end against an existing binding when
   they reach it (`expand_fixed_edges`, "consistency with an already-bound
   next-node variable"). q3 therefore builds the cross product of persons per
   country three times before the `knows` triangle filters it; that is the
   32,030,444 logical rows Codex's manifest records.
4. **`count(*)` after materialization.** `grouped_project` receives the full
   `Vec<Row>` and counts it. For q6 that would be 55.6 million rows.

The CSR adjacency (`CompressedAdjacency`, `read.rs:1310–1440`) is already
there and is the right structure. It is rebuilt on every query
(`NodeIndex::build` inside `execute_single`) and it has no per-relationship-type
or per-label partitioning, so an expansion over `:Message_hasTag_Tag` walks
every outgoing edge of the vertex and filters by label.

## 2. What to build

Five changes, in dependency order. Everything stays inside `grust-cypher` and
`grust-core`; the parser, AST, policy layer, and the existing executor are
untouched, and the existing executor remains the differential oracle.

### 2.1 A typed CSR built once per graph snapshot

Extend `grust_core::GraphIndex` (or add `grust_core::TypedGraphIndex`):

- `vertices_by_label: HashMap<Label, Vec<u32>>` (sorted).
- Per relationship type, forward and reverse CSR: `offsets: Vec<u32>`,
  `targets: Vec<u32>`, with each vertex's target slice **sorted**. Sorted
  slices give O(log d) membership tests and linear-time intersection.
- Property-filtered vertex sets are derived once per query
  (`Message {kind: 'Comment'}` becomes a bitset or sorted `Vec<u32>`), never
  re-evaluated per row.
- Build cost at SF0.1: about 2 million edge appends plus one sort per
  vertex slice, roughly 100–200 ms once, then shared through an `Arc` by
  every query on that snapshot. `MemoryGraphStore` already keeps
  `outgoing_edges`/`incoming_edges` BTree maps; those can be replaced by this
  index so there is one adjacency structure, not two.

Expose `execute_read_query_indexed(&Graph, &TypedGraphIndex, …)` alongside
the existing `execute_read_query(&Graph, …)` so the benchmark's Memory cell
builds the index inside its load interval and queries pay nothing for it.

### 2.2 Slot rows: `u32` vertex and edge indexes, no clones

Compile the query's variables to slots at plan time. A binding row becomes
`Vec<u32>` (or `SmallVec<[u32; 8]>`), node variables hold vertex indexes,
relationship variables hold edge indexes, and a value is materialized only
when an expression reads a property, `id()`, or returns the element. For
LSQB, no expression ever does except `id(tag1) <> id(tag2)` and
`tag1.TagId <> tag2.TagId`, both of which compare vertex indexes.

This removes the `clone_row`/`clone_node` cost entirely and cuts a row from
hundreds of bytes plus heap allocations to a few words. It is a
representation change, not a semantic one; the existing `Bound`-based
executor can be kept for the general path and used as the oracle.

### 2.3 Count-only pipelines: never materialize what is only counted

When the `RETURN` is a single aggregate with no grouping keys (`count(*)`,
`count(x)`, `sum`, `min`, `max` over a simple expression) and no `ORDER BY`,
`SKIP`, `LIMIT`, `DISTINCT`, or `WITH` boundary, the plan is a pipeline that
expands depth-first and increments a counter at the leaf. No `Vec<Row>` is
built at any depth. This alone makes q6's 55.6 million matches a counting
loop over the `knows` and `hasInterest` CSRs.

Two exact refinements make most LSQB queries O(V + E) rather than
O(matches):

- **Factorized counting for acyclic patterns.** If the pattern graph is a
  tree and every variable is bound exactly once, the count of matches is the
  sum over the root of the product of the counts of each child subtree. q4
  is `Σ over message of hasTag(m) · hasCreator(m) · likes(m) · replyOf(m)`,
  which is one pass over Messages with four degree lookups. q1 is a nine-hop
  chain: nine sparse matrix-vector passes over the typed CSRs, each
  O(E_type), and the answer 8,773,828 falls out of the root sum. The
  reversed-chain attack a1 is the same computation from the other end.
- **Worst-case-optimal intersection for cyclic patterns.** q2 (comment by p1
  replying to a post by p2, with p1 knows p2), q3 (a `knows` triangle with
  a shared country), q5, q6, q9 contain cycles or shared endpoints. Order the
  variables and enumerate each next variable as the intersection of the
  sorted adjacency slices of the already-bound neighbours (leapfrog
  triejoin). For q3: enumerate `knows` triangles by intersection (18,135
  `knows` edges at SF0.1, so trivially fast), then check the shared country
  through the functional `isLocatedIn` and `isPartOf` hops (one lookup each,
  since every Person has one City and every City one Country). That replaces
  the 32-million-row cross product with about 30,456 triangle checks.

Cypher's relationship-uniqueness rule (no relationship bound twice within one
`MATCH`) must be honoured. Factorized counting is valid when the pattern's
relationships have pairwise-distinct types, which holds for every LSQB query;
when two segments share a type, the planner falls back to the enumerating
pipeline with an explicit distinctness check. The planner must prove the
condition, not assume it.

### 2.4 Anti-joins and inequalities as index probes

- `WHERE NOT (a)-[:T]->(b)` with `a` and `b` bound is a binary search in
  `a`'s sorted `T` slice. q8's `NOT (comment)-[:hasTag]->(tag1)` and q9's
  `NOT (person1)-[:knows]->(person3)` become O(log d) probes inside the
  pipeline instead of the `OPTIONAL MATCH … IS NULL` rewrite that
  materializes the optional side. The rewrite that `benchmarks/lsqb` applies
  for the adapter can stay as source; the planner recognises the
  `OPTIONAL MATCH (a)-[h:T]->(b) WITH … WHERE h IS NULL` shape and lowers
  it to the same probe.
- `id(x) <> id(y)` and `x.Id <> y.Id` on distinct vertices are `u32`
  comparisons on the slot row.

### 2.5 Parallel expansion over start vertices

The pipeline is embarrassingly parallel over the root variable's candidate
set. Split `vertices_by_label[root]` into chunks with `rayon`, run the
pipeline per chunk with a local counter, and sum. Neo4j Community executes
one query on one core; the benchmark container has eight. This is the one
change that is a constant factor rather than a complexity change, but it is
an 8× constant on the machine the evidence is taken on.

## 3. Expected outcome

Per-query work at SF0.1 under the plan above, with the published native
comparator's median from the site's SF0.1 bundle for scale:

| Query | Shape | Plan | Estimated work at SF0.1 | Published native median |
|---|---|---|---|---|
| q1 | 9-hop chain | factorized DP | 9 typed-CSR passes, ~2M edge visits | 1.18 s |
| q2 | cycle p1–p2 with comment/post | intersection | 215k replyOf edges × log d | 0.35 s |
| q3 | knows triangle + country | intersection + functional hops | ~30k triangles | 0.53 s |
| q4 | star on message | factorized | 388k messages × 4 lookups | 1.98 s |
| q5 | tag–message–comment–tag, tag ≠ tag | pipelined | 253k hasTag × replies | 1.62 s |
| q6 | 2-hop knows + interest | factorized DP | 18k + 39k edges | 3.31 s |
| q7 | star with OPTIONAL | factorized, null-padded | as q4 | 2.55 s |
| q8 | q5 + anti-join | pipelined + probe | as q5 + log d | 2.51 s |
| q9 | q6 + anti-join | pipelined + probe | as q6 + log d | 3.61 s |

Every row is single- or double-digit milliseconds single-threaded on this
hardware, which matches the upstream reference timings the LSQB repository
ships (q1 0.042 s, q4 0.028 s, q6 0.023 s at SF0.1). At SF0.3 (6.2 million
edges) the same plans are roughly 3× that. The counts stay exact: the same
scalar oracle the harness already enforces.

The attacks split the same way. a1 (reversed chain) is the q1 DP from the
other end, so it costs what q1 costs. a2–a5 are
reorderings the join planner makes irrelevant. a6–a13 are parser and
semantics probes that already take Neo4j under 10 ms; the pipeline handles
them the same way it does today.

## 4. Where it lands in the benchmark

- The Memory cell stays `in-process-reference`. Add a `plan` field to the
  observation (`clause-pipeline`, `count-pipeline`, `count-factorized`,
  `count-intersection`) so a reader can see which path produced the scalar.
- The 1,000,000-row admission gate applies to plans that materialize Rust
  rows. A count-only plan materializes none; record `rust_rows.kind =
  "not-materialized"` and let it run. Keep the gate for row-producing plans.
- Turso and PostgreSQL can compute every LSQB count as one SQL aggregate
  (`SELECT count(*) FROM hasTag JOIN replyOf …`). Lowering count-only
  patterns to SQL makes them `backend-native-aggregate` like FalkorDB and
  gives two more backends that run all nine queries at SF0.1 and SF0.3.
- Validation is differential: the new planner must produce the same scalar as
  the existing clause-by-clause executor on `sfexample`, on random small
  graphs, and on every query in `attacks/` before it is allowed to answer.
  The repository already does this for SQL pushdown; reuse that harness.

## 5. Order of work

| Step | Change | Effort | Expected effect on the SF0.1 Memory cell |
|---|---|---|---|
| 0 | Label index; typed CSR built once at load; property-filtered sets | 1 day | q4 16.5 s → seconds; q2 completes |
| 1 | Slot rows with `u32` bindings; count-only pipeline; greedy join order from label and degree statistics | 2–3 days | all nine queries admitted and under 1 s |
| 2 | Factorized counting; sorted-slice intersection; anti-join probes; `rayon` over roots | 3–5 days | 10–100 ms per query |
| 3 | Plan field in reports; gate exemption for non-materializing plans; SQL count pushdown for Turso and PostgreSQL | 1–2 days | three backends run all nine queries at SF0.1 and SF0.3 |

Step 0 is a day and already changes the published picture. Steps 1 and 2 are
where the executor stops materializing rows. Nothing here touches the
harness, the receipts, or the parser.
