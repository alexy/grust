# GQL Completion — M1/M2 Checkpoint

Branch: `cypher-gql-full` (pushed to origin). Goal contract: `docs/GQL_GOAL.md`.
Plan: `docs/GrustCypherFull.md`.

## Unit 10a (write-path cutover) — DONE via decision B

The parser accept-set fork (below) was resolved by the human as **B: migrate the
non-standard test statements to standard Cypher and narrow the public accept-set.**
Implemented (all green, pushed):

- **Accept-set gate** (`planner::gate_writable_statement`): the writable-Cypher
  entrypoints (`cypher_mutation_plan*`) now route *acceptance of the mutation
  grammar* through the new standards-conformant parser. Each mutation statement
  (with the trailing `RETURN` split off) must parse on the new parser, so the
  non-standard `DELETE (:pattern)` / `DELETE (:a)-[:R]->(:b)` forms are rejected.
- **Byte-identical plans preserved**: plan *building* still runs through the
  legacy planner (untouched), guarded by `tests/golden/write_golden.json`. The
  gate is **parse-only** (no semantic pass) so the legacy cross-statement
  local-variable bindings and the richer legacy `RETURN` projection are
  unaffected.
- **Parser robustness**: `parse_key`/`parse_map_key` now accept reserved keywords
  as property/map keys (e.g. `{order: 1, limit: 3}`), recovering exact source text
  via the token span — prevents an unintended accept-set regression.
- **Tests migrated** to standard Cypher (`DELETE (pattern)` → `MATCH … DELETE`),
  with rejection of the non-standard forms asserted explicitly. 521 lib (was 517).

Follow-on (not blocking, sequenced for 10b/later): migrate plan *construction*
itself onto the AST (today it re-parses via the legacy planner after the gate);
extend the new read/return projection to the full legacy returning surface so the
gate can eventually cover `RETURN` too.

<details><summary>Original fork write-up (resolved by B)</summary>

### ⚠️ Unit 10a (write-path cutover) — BLOCKED on a parser accept-set fork

Investigated 2026-06-25. The write corpus was scraped from the strict-write
tests into `crates/grust-cypher/tests/golden/write_corpus.json` (181 distinct
planner-argument statements, full write surface) as the parity foundation, on
top of the existing `tests/golden/write_golden.json` plan-snapshot harness.

**The blocker (empirically confirmed, not a guess):** decision (a) assumed the
new lexer/parser/semantics pipeline is a *superset* of the legacy string planner
that just needs its rejections re-imposed. The reality is the reverse in places:
the **legacy planner accepts non-standard Cypher that the new standards-conformant
parser correctly rejects.** Concretely, the new parser errors on forms the 327
strict-write tests pin as accepted:

- `DELETE (:Person {id: 'p1'})` — DELETE by **node pattern** (standard Cypher
  DELETE takes a bound variable/expression, not a pattern).
- `DELETE (:Person {id})-[:KNOWS]->(:Person {id})` — DELETE by **edge pattern**.

(Standalone `CREATE (a)-[:R]->(b)`, `MATCH … SET n.x = n.x + 1`, multi-pattern
`MATCH …, … CREATE (a)-[:R]->(b)`, and `SET n += {…}` all *do* parse on the new
pipeline — the divergence is specifically the pattern-as-DELETE-target forms.)

To keep "byte-identical plans + same accept/reject set" (decision (a)'s hard
constraint) one of these must be chosen — and it is a **product/architecture
decision for the human**, not covered by decision (a):

- **(A) Extend the new parser** to accept Grust's non-standard write forms
  (`DELETE (pattern)`) purely for legacy bug-compat. Preserves byte-identity and
  the public accept-set, but pollutes the clean GQL-conformant grammar — undercuts
  the new pipeline's reason for existing.
- **(B) Migrate the non-standard test statements** to standard Cypher
  (`MATCH (n:Person {id:'p1'}) DELETE n`) and narrow the public accept-set. Changes
  test *inputs* (not just error messages) and is a behavior change for any
  downstream relying on `DELETE (pattern)` — beyond what decision (a) authorized.
- **(C) Keep the legacy planner as the write entrypoint** (no cutover); use the
  new pipeline for reads only. Defers the "one parser" goal for writes.

Per the guardrails ("STOP and surface on a genuine new fork"), 10a is paused at
this decision. Unit 10b (write widening) is fork-adjacent and also paused. The
autonomous loop continues on **independent, pre-authorized** units (Unit T
duration/decimal; Unit 13 transactions) while this awaits the human call.

</details>

---

The M1 (Foundation) record is below. Since then, **M2 (Portable Query Core) read
side** landed on the new pipeline — see the next section.

## M2 progress — portable read query core (additive, on the new pipeline)

`src/read.rs` is a Memory **reference executor** that runs read-only queries
through lexer → parser → semantics → execution over a `Graph` snapshot
(`MemoryGraphStore::graph()`), returning the existing `CypherResultTable`. The
write planner is untouched. Implemented and tested (unit + `tests/read_conformance.rs`):

- `MATCH` over node patterns and relationship segments (direction, types, inline
  property maps), multi-hop, and **variable-length** `*min..max` (no repeated
  nodes; rel var binds the edge list);
- **OPTIONAL MATCH** with null-padding;
- `WHERE` with a general expression evaluator: arithmetic, comparison,
  three-valued boolean logic, `IN`, `IS [NOT] NULL`, `STARTS/ENDS WITH`,
  `CONTAINS`, `CASE`, and a scalar function registry (string/numeric casts,
  `coalesce`, `size`, …) reusing the crate's `restricted_*_value` helpers;
- `RETURN` with aliases, `*`, `DISTINCT`, `ORDER BY`, `SKIP`, `LIMIT`, and
  **aggregates** (`count/sum/avg/min/max/collect`) with implicit `GROUP BY`;
- **`WITH`** horizon (projection, aggregation, `WHERE`, distinct/order/skip/limit;
  carries node bindings into later `MATCH`), **`UNWIND`**, and **`UNION` / `UNION ALL`**.

The `GqlFeature` manifest was updated so these are `Supported` (portable Memory
reference); `tests/gql/portable_read.json` is the corpus. Still feature-gated:
path variables, subqueries, shortest path, multi-label patterns, map/index
projections.

Not yet done for the read core (deferred): wiring the legacy write entrypoints
onto the new pipeline (below).

## Unit 15 — read pushdown (completed as scoped; extended by PUSHDOWN2)

> **2026-07-04 addendum:** the follow-on goal `docs/GQL_PUSHDOWN2_GOAL.md`
> (branch `pushdown2`) extended this module with row-source and composition
> leaves: catalog procedures (`db.*` as DISTINCT scans), `tvf.range`
> (recursive CTE) and correlated `tvf.keys` (lateral `json_each`),
> uncorrelated **and** correlated `CALL { … }` subqueries (a `LEFT JOIN` of
> two node scans; a correlated inner `WHERE` renders into the join `ON`), and
> endpoint-only `shortestPath`/`allShortestPaths` (recursive walk CTE with
> per-pair minimal-depth selection). Dialect-gated leaves report through
> `ReadPushdown::supported_by` and Sail falls back for them. The section
> below is the original Unit 15 record; the future-work list still holds
> except where the addendum says otherwise.

### Original Unit 15 record (PAUSED at a comprehensive point)

Status: the cleanly-additive predicate/path pushdown work is **complete and
verified**; paused here on request. 16 sub-commits (`c238e25` … `cfa6e3e`) on
`cypher-gql-full`, each green and oracle-checked.

### What pushes down today

`crates/grust-cypher/src/pushdown.rs` lowers a bounded read query's
`MATCH`/`WHERE` filter into SQL via a `SqlDialect` (`SparkDialect`,
`SqliteDialect`); the `RETURN` projection runs through the shared Memory
reference (`read::project_*`), so a pushdown result is **byte-identical to
`read::run_read_query` by construction**. Anything outside the pushable subset
returns `Ok(None)` → the backend falls back to the reference (never a wrong
answer).

- **Patterns:** single node; 1..N relationship segments (out/in/undirected,
  multiple rel types, inline endpoint/edge props); variable-length `*m..n` with
  an anonymous relationship (recursive CTE enumerating simple paths, no repeated
  nodes). Entry points: `plan_node_read`, `plan_segment_read`,
  `plan_var_length_read` (+ `_with_hints` variants).
- **`UNION` / `UNION ALL`:** `plan_read` returns a unified `ReadPushdown` enum —
  a single-query leaf or a `Union { arms, distinct }` of leaves. The backend runs
  each arm and `combine_union`s the result tables (concat + dedup-if-distinct,
  mirroring the reference). Leaves share a uniform text-rows execution contract
  (`to_sql` / `column_count` / `project_text_rows`).
- **`OPTIONAL MATCH`** (mandatory node + one optional directed segment):
  `MATCH (a[:L][{..}]) [WHERE wa] OPTIONAL MATCH (a)-[r?:T [{..}]]->(b[:L][{..}])
  [WHERE wb] RETURN …` lowers to a `LEFT JOIN` of `a` (`n0`) against a **subquery**
  that is the whole optional segment (`e0 ⋈ n1` with all optional conditions), so
  the optional match is atomic — no match → every optional column NULL → `r`/`b`
  null-padded (`PushedBinding::Null`), matching the reference. `wa` references
  only `a`; `wb`/inline props reference only `r`/`b` (else fall back). Undirected
  optional segments fall back.
- **Multi-pattern `MATCH`** (`(a)-[]->(b), (a)-[]->(c)` and bare cross products
  `(a), (b)`): lowers to a comma-join of every node/edge alias with all
  connectivity + filters in `WHERE`; a variable shared across patterns reuses its
  alias (joined), patterns without a shared variable cross-join. Directed
  segments only. Tried after the single-path planner, so it also handles a single
  pattern that reuses a variable.
- **`WITH` horizon:** `MATCH (n[:L][{..}]) [WHERE] WITH … [WHERE] [UNWIND …]
  RETURN …` pushes the **leading single-node scan + filter** into SQL, then runs
  the `WITH`/`UNWIND`/`RETURN` horizon over the fetched nodes through the shared
  reference pipeline (`read::project_binding_pipeline`) — identical to the
  reference by construction. The tail must not contain a further `MATCH` (graph
  access); only the leading scan is pushed (the horizon, incl. aggregation, runs
  in Rust).
- **`WHERE`** (node and segment paths): comparisons (`=,<>,<,<=,>,>=`),
  `IS [NOT] NULL`, `IN`/`NOT IN`, `STARTS/ENDS/CONTAINS`, boolean `= true/false`,
  `+`/`-`/`*` arithmetic over typed numeric properties, and `AND`/`OR`/`NOT`.
- **Shaping:** `ORDER BY` / `SKIP` / `LIMIT` pushed into SQL — always for
  typed-JSON dialects (SQLite/libSQL `json_extract`), and for Spark when a
  `TypeHints` (from the graph schema) types the sort keys so numeric casts are
  emitted (incl. edge-property keys). Otherwise ordering stays in the reference.
- **Backends:** `SailGraphStore::run_read_query` tries node → segment →
  var-length → reference fallback. `SailTypeHints` derives type hints from the
  applied `GraphSchema`.

### Verification

- **Differential oracle** (`crates/grust-turso/tests/read_pushdown_oracle.rs`):
  executes the generated SQL against embedded SQLite and asserts row-equality vs
  the Memory reference — 10 tests covering every feature above, including a
  prefix-collision graph for var-length and an `UntypedSqlite` dialect that
  simulates the Spark cast path. Recursive-CTE (var-length) runs against real
  SQLite via `rusqlite` (bundled, a grust-turso dev-dep) since `turso` lacks
  `WITH RECURSIVE`.
- **Sail:** a `#[ignore]` live-server differential test (`grust-sail/src/tests.rs`).
- Gate: cypher 507 lib / 3 / 11 (0 failed, 0 warnings); turso 7 + 10; sail 35 /
  26 ignored; facade(`cypher,memory`) + grust-turso compile; `git diff --check`
  clean.

### Future work (none are clean additive predicate work)

1. **`/`, `%`, `^` arithmetic** — *not safely pushable*: integer-vs-float
   division and modulo diverge across engines (SQLite `5/2 = 2`, Spark `5/2 =
   2.5`, reference is float). Correct reference fallback exists. Would need a
   per-dialect division-semantics shim to be provably equal.
2. **Variable-length with a named-relationship edge-list binding**
   (`(a)-[r:T*1..n]->(b) … r`) — needs the recursive CTE to accumulate an edge
   array and reconstruct `r` as a `Value::Json` list matching the reference.
   Niche; anonymous-relationship var-length already pushes.
3. **Path variables** (`MATCH p = …`) — needs path-value reconstruction; niche.
4. **Multi-clause shapes** — `UNION`/`UNION ALL`, `OPTIONAL MATCH` (single
   optional segment), multi-pattern `MATCH`, and the `WITH` horizon are **done**
   (see above). Remaining (all niche): chained/multiple `OPTIONAL MATCH` and
   optional multi-segment; pushing a post-`WITH` `MATCH` (correlated, needs the
   graph); `WITH` after a non-node leading pattern (segment/var-length).
5. **PUSHDOWN2 (2026-07-04)** — subsumed more of this list's spirit: CALL-based
   row sources, subqueries (uncorrelated + correlated-WHERE), and endpoint-only
   shortest paths now push (see the addendum above and
   `docs/GQL_PUSHDOWN2_GOAL.md`). Items 1–3 still stand (the shortest-path leaf
   inherits the same exclusions: no edge-list bindings, no path variables).

Other backends: only Sail wires pushdown into its read entrypoint today. Turso
is used as the oracle but its own `run_read_query` is not wired (and its tagged
JSON storage would need a tagged dialect variant). Postgres/pgGraph/pgq backends
could reuse the same `SqlDialect` IR.

---

## Unit 12 — backend conformance profiles (done)

`grust_cypher::gql` now carries an honest per-backend model: `GqlBackend`,
`GqlBackendDescriptor`, `GqlBackendRole` + `backend_manifest()` /
`cypher_conformance_backends()`. Verified flags: executing Cypher set =
Memory/Sail/Turso; read pushdown = Sail only; Postgres/PostgresPgq = SQL/PGQ
stores (no portable Cypher executor yet); helix/ladybug = internal (publish=false,
out of facade); cocoindex = sync target. A consistency test pins these facts.

---

## Unit T — type system (partial; DECISION NEEDED for duration/decimal)

**Temporal (done):** `Value::DateTime` now orders chronologically (RFC 3339
lexicographic; chronological for a consistent offset) in both `read::value_order`
and `projection::compare_return_values`. Previously two datetimes compared equal.

**Duration / decimal (parked — fork for the user):** making these first-class
requires either (a) new `grust_core::Value` variants — workspace-wide, touches all
8 backends' `Value` matches + serialization, conflicts with additive discipline →
NOT done unsupervised; or (b) a cypher-layer typed representation (Json/String-
tagged) — additive but not first-class. Must be resolved before Unit 16's
full-39075 completeness claim. The loop continues on unblocked units meanwhile.

---

## Unit 11 — schema / graph types (core done)

`grust_cypher::graph_type`: `GraphTypeMode` (Open/Closed) + `validate_node` /
`validate_edge` / `validate_graph` over a `GraphSchema`. Closed graph types reject
undeclared labels/properties; both modes type-check declared properties
(`value_matches_field_type`) and enforce required fields/constraints. A
backend-neutral `ValidateBeforeWrite` hook (changes no backend). Existing
`CypherConstraintRegistry`/`CypherSchemaManager`/DDL remain the named-constraint +
schema-application layer. Catalog/session metadata + per-backend graph-type
enforcement reporting fold into Unit 13/16 (today enforcement is caller-applied
via validate_*).

---

## Unit 10a — write-path rewiring (BLOCKED; golden harness done)

**Done:** a strict-write golden-snapshot harness (`tests/write_golden.rs` +
`tests/golden/write_golden.json`) pins the planner's plan/rejection output for a
20-statement corpus.

**Blocked (decision needed):** routing the legacy `cypher_*` write entrypoints
through the new lexer/parser/semantics pipeline CANNOT be byte-identical — the
new pipeline emits span-bearing structured errors and accepts a broader grammar,
whereas the 327 strict-write tests + the golden pin the legacy planner's exact
error strings and narrow accept-set. Per the abort-on-drift mandate, the cutover
is not done. Options for the human: (a) relax 'byte-identical' to 'same
accept/reject + same plan shape, new structured error messages' and update the
strict-write tests' error expectations (needs explicit OK — touches the 327
suite); (b) keep the legacy planner for writes (two parsers; reads on the new
pipeline); (c) hybrid (new lowering for valid plans, legacy validation/errors).
This blocks Unit 10b and Unit 13 (which depend on it) and Unit 16's completeness.

---

## Unit 16 — full-profile candidate hardening (DONE)

Terminal milestone. `docs/GQL_PROFILE_STATEMENT.md` is the precise, backed profile
statement: realized profile = the 58 `Supported` features; the candidate
`Full39075` claim is backed feature-by-feature against the `GqlFeature` manifest,
with all 16 not-yet-supported items (8 future + 3 planned) enumerated + rationaled
and 5 intentional rejections documented as conformance guards. The
`full_profile_claim_is_backed` test pins the scoped-out set to the manifest so the
doc cannot silently drift. Final state: cypher 525 lib / 3 / 17, core 46, memory
21, turso 7+14, 0 warnings, golden byte-identical, all pushed.

The W1/W2/W3 widenings (below) and the 10a cutover all landed under decision B +
the W-series decisions; W4 kept explicit-id default.

---

## Unit 10b — pattern-driven write widening (audit done; widenings = review decisions)

Full audit in `docs/GQL_U10b_WRITE_WIDENING_AUDIT.md`. Key finding: U10b's core
objective — **multi-row pattern writes** — is *already implemented* by the legacy
planner (`PatchMatchingNodes`/`UpdateMatchingNodeProperty`/`RemoveMatchingNodeProps`/
`DeleteMatchingNodes`, the edge equivalents, `UpsertEdgesFromNodeMatches`, all with
`BoundedMany` cardinality + predicate filters; plus cross-statement local vars).
The Unit 10a gate now fronts it with the standards-conformant parser.

What remains is **not** missing multi-row support — it's a set of accept-set
*expansions* (W1–W4 in the audit), each a product decision with plan-shape or
semantics implications:
- **W1** multiple relationship patterns per write statement (medium-high risk).
- **W2** incoming `<-[:T]-` edge writes (standard Cypher; plan-preserving by
  endpoint swap, but the legacy string edge-detector keys on `->`, so the change
  is more invasive than it looks).
- **W3** cross-variable numeric `SET` (`SET a.x = b.y + 1`) — deliberately rejected.
- **W4** default-on generated ids (already available via `CypherNodeIdPolicy`,
  off by default) — a policy default flip.

Per the guardrails (do not drift plans / expand the public accept-set without an
explicit decision), **no speculative relaxations applied**; W1–W4 are folded into
the Unit 16 human review. This is the loop's terminal stop.

---

## Unit 13 — transactions / sessions / control (language + capability done)

Fork-independent slice landed (all green, pushed). The executable substance
(atomic begin/commit/rollback) is a storage-layer concern delegated to each
backend's `GraphMutationAtomicity`; only the language surface + capability
reporting is independent of the write-path cutover fork, so that is what's built:

- **`grust_cypher::transaction`** — `TransactionCommand::{Start(Option<AccessMode>),
  Commit, Rollback}` with `TransactionCommand::parse(src) -> Result<Option<_>>`:
  recognizes `START TRANSACTION [READ ONLY|READ WRITE]` / `BEGIN` / `COMMIT` /
  `ROLLBACK`, returns `Ok(None)` for non-transaction sources (query parsing still
  applies), `Err` for malformed ones. Keywords are intentionally **not reserved**
  in the lexer (recognized at statement level), so `start`/`commit`/`read`/… stay
  usable as identifiers/property names — no regression to existing queries.
- **Capability reporting** — `GqlBackend::transactional()` + new
  `GqlBackendDescriptor::transactional` (verified against each backend's
  `mutation_atomicity()`: Turso/Postgres/Postgres-PGQ = `Transactional`;
  Memory/Sail/Helix/Ladybug/CocoIndex = not) + `transactional_backends()`.
- Manifest: `TransactionControl` → `Supported` (parse + capability); `SessionControl`
  → `Planned`. Tests: +4 lib (521 total).

**Deferred (write-path-coupled / cross-cutting):** executing a `BEGIN … COMMIT`
batch atomically (needs uniform store begin/commit/rollback + the write path);
session commands (`SET`/`RESET`/`USE`). Sequenced after the Unit 10 cutover fork.

---

## Unit T — temporal / duration / decimal type system (core done)

Decision (a): first-class `grust_core::Value` variants. Landed (all green, pushed):
- **`grust_core::Decimal`** — dependency-free fixed-point `mantissa(i128) × 10^−scale`
  (SQL DECIMAL(38,s)); lossless within 38 digits, value-normalized (`1.50==1.5`),
  parse/canonical-display, serde-as-string, value Ord/Eq, checked `+`/`-`/`*`, `to_f64`.
- **`grust_core::Duration`** — ISO 8601 month/day/second/nanos model (`P1Y2M10DT2H30M`);
  years→12mo, weeks→7d, fractional seconds→nanos; structural total Ord, `checked_add`,
  `negated`, parse/iso-display, serde-as-string.
- **`Value::Decimal`/`Value::Duration`** variants + `Value::decimal`/`duration`
  constructors + `as_decimal`/`as_duration`; **every** workspace exhaustive `Value`
  match fixed behavior-preservingly (legacy strict-write paths treat them as
  unsupported; all backends serialize them as canonical/ISO strings).
- **Read executor**: `decimal(...)`/`duration(...)` functions; lossless decimal
  `+`/`-`/`*` (ints coerce exactly, floats → f64 path); duration `+`/`-`; exact
  decimal/duration equality + ordering in `WHERE`/`ORDER BY`.
- Manifest: `TemporalValues`/`DurationValues`/`DecimalValues` → `Supported`.
- Tests: grust-core 40 (+5 type tests); read_conformance 17 (+2).

**Deferred tail (niche, dialect-divergent):** decimal/duration **pushdown** into
SQL (DECIMAL / INTERVAL casting) and its differential oracle — analogous to the
Unit 15 pushdown tails. The portable reference handles them; backends fall back to
the reference for these types until the dialect lowering is added.

---

## Unit 14 — functions/procedures/escapes (done)

**Functions (done):** read-path scalar registry expanded with sqrt/exp/ln/log/
log10/sin/cos/tan (numeric→Float, null-propagating), via `unary_float_fn`;
usable in WHERE/RETURN; conformance-tested.

**Escapes (done earlier):** backtick-quoted identifiers are handled by the M1
lexer.

**Procedures/CALL (done):** read-only catalog procedures `db.labels()`,
`db.relationshipTypes()`, `db.propertyKeys()` parse via `parse_call` (new
`ast::CallClause`, `Clause::Call`) and execute in the read reference
(`read::call_procedure`) over a `Graph` snapshot — distinct, deterministically
sorted values. Two forms: standalone `CALL db.labels()` (the YIELD shape is the
result table) and `CALL … YIELD col [AS alias] [WHERE …]` feeding downstream
`WHERE`/`RETURN`/aggregation. `ProcedureCall` feature is now `Supported`;
procedure *arguments* and non-catalog procedures stay feature-tagged
unsupported. Binary string functions (substring/replace/split/left/right) remain
an easy additive follow-up.

---

### Implementation history (chronological)

`src/pushdown.rs` is the backend-neutral lowering. `plan_node_read(cypher,
params)` lowers the **pushable subset** — a single node pattern
`MATCH (var[:Label] [{k: lit}]) [WHERE pred] RETURN …` where `pred` is a
conjunction/disjunction/negation of property comparisons (`=,<>,<,<=,>,>=`) vs
int/float/string literals (or a parameter resolving to one) and `IS [NOT] NULL`
— into a `NodeReadPushdown`. `to_sql(&dyn SqlDialect)` renders the scan + filter
(`SparkDialect` and `SqliteDialect` provided); the `RETURN` projection is **not**
pushed — it runs through the shared reference (`read::project_nodes`), so a
pushdown result is byte-identical to `read::run_read_query` **by construction**.
Anything outside the subset returns `Ok(None)` → the backend falls back to the
reference rather than risk a wrong answer.

- **Oracle:** `crates/grust-turso/tests/read_pushdown_oracle.rs` executes the
  `SqliteDialect` SQL against an **embedded** in-memory SQLite engine (the `turso`
  crate, no server) over an untagged `grust_nodes` table and asserts row equality
  vs the Memory reference across 17 queries + parameters. Automated, CI-green.
- **Sail:** `SailGraphStore::run_read_query` pushes the filter into Spark SQL for
  the pushable subset and falls back to `read_graph()` + reference otherwise; a
  `#[ignore]` live-server differential test pins row equality.

**Milestone 2 — relationship segments.** The pushable subset now also covers a
single **directed relationship segment** `(a[:LA] [{..}])-[r?:T [{..}]]->(b[:LB]
[{..}])` (and the `<-[..]-` incoming form): `plan_segment_read` →
`SegmentReadPushdown` lowers it to a `grust_edges`/`grust_nodes` join (multiple
rel types → `edge_type IN (…)`, inline endpoint/edge props + `WHERE` over
`a`/`r`/`b`). The backend executes the join and returns the selected columns as
**text rows**; `project_text_rows` reconstructs the `(a, r, b)` bindings (parsing
the JSON `props` columns) and runs the shared reference projection. The Turso
oracle gained an edge table + 10 segment queries (compared as a row **multiset**,
since join order is backend-defined); Sail's live test gained segment + a
multi-hop fallback case.

**`IN` predicates** (`prop IN [literals]`, and `NOT … IN`) are pushed on both the
node and segment paths for non-empty homogeneous int/float/string lists (SQL
3-valued `IN` matches the reference's membership semantics).

**`ORDER BY` / `SKIP` / `LIMIT` pushdown** (single-node path): pushed into SQL
**only for dialects whose JSON extraction is natively typed** — `SqlDialect::orders_json_typed()`
is `true` for SQLite/libSQL (`json_extract` → INTEGER/REAL/TEXT) and `false` for
Spark (`GET_JSON_OBJECT` → text, where numeric `ORDER BY` would sort
lexicographically). Gated on no aggregate/`DISTINCT`, every sort key resolving
(through `RETURN` aliases) to a scan-var property/label, and `SKIP`/`LIMIT`
resolving to non-negative integers; emitted with `NULLS LAST` (asc) /
`NULLS FIRST` (desc) to match the reference (NULL = max). When pushed, the Rust
projection drops order/skip/limit; otherwise it keeps them. The Turso oracle now
verifies pushed ordering by **exact row sequence** (tie-free fixture columns);
Spark keeps ordering in the reference projection pending schema-aware typed-table
pushdown.

**Schema-aware ordering for untyped-JSON dialects (Spark).** A `TypeHints` trait
(vocabulary `ScalarKind`; built from the backend's `GraphSchema`) is resolved
into the plan at planning time (`plan_node_read_with_hints` /
`plan_segment_read_with_hints`). On a dialect whose JSON extraction is untyped
(`orders_json_typed() == false`, e.g. Spark `GET_JSON_OBJECT`), `pushes_ordering`
allows pushdown only when every sort key's type is known, and `to_sql` casts
numeric keys (`CAST(… AS BIGINT/DOUBLE)`) so the SQL order matches the reference;
otherwise ordering stays in the reference projection. `SailGraphStore` derives
`SailTypeHints` from its applied schema. The Turso oracle simulates this via an
`UntypedSqlite` test dialect (casts executed by real SQLite), asserting
exact-sequence equality. `ORDER BY`/`SKIP`/`LIMIT` pushdown now covers **both the
node and the relationship-segment paths**.

**Multi-segment and undirected paths.** The segment planner generalized to a
chain of K segments (`SegmentReadPushdown` now holds `node_labels: Vec<…>` +
`segments: Vec<SegSpec>`; nodes aliased `n0..nK`, edges `e0..e{K-1}`). It pushes
`(a)-[]->(b)-[]->(c)…` (chained joins, any per-segment direction) and
**undirected** segments (`(a)-[]-(b)`, OR join matching either orientation, both
orientations emitted like the reference). Repeated variables across positions and
path variables fall back to the reference. Operands/ordering are indexed by node
position / segment. Edge-property `ORDER BY` on untyped dialects uses
`TypeHints::edge_property_kind` when the segment has a single typed relationship.

**Variable-length paths.** `(a)-[:T*m..n]->(b)` with an **anonymous**
relationship lowers (via `plan_var_length_read` / `VarLengthReadPushdown`) to a
**recursive CTE** that enumerates simple paths (no repeated nodes, matching the
reference) of length `[min, max]`, then joins the start/end nodes. The visited
set is a U+001F-delimited string (the separator Grust already reserves in keys),
tested with `instr(...)` so prefix-colliding ids (`p1`/`p11`) don't collide. min
defaults to 1, max is open if unspecified; direction may be out/in/undirected.
Named relationships (edge-list binding) and path variables fall back. The
embedded `turso` engine does **not** support `WITH RECURSIVE`, so the oracle
verifies this path against real SQLite (`rusqlite`, bundled, a grust-turso
dev-dependency); the Spark rendering is golden-tested and depends on the engine's
recursive-CTE support.

**String predicates.** `STARTS WITH` / `ENDS WITH` / `CONTAINS` with a non-empty
string needle are pushed on both the node and segment paths via a
`SqlDialect::string_predicate` (Spark `STARTSWITH`/`ENDSWITH`/`CONTAINS`; SQLite
`instr`/`substr`, literal and NULL-propagating). Matches the reference for
string-typed properties; a non-string property value *errors* in the reference
but *filters* under pushdown (documented caveat — pushed SQL can't abort the
query). Empty needles fall back.

**Boolean comparisons.** `prop = true|false` / `prop <> true|false` are pushed on
both paths via `SqlDialect::bool_literal_sql` (SQLite compares the `json_extract`
integer `1`/`0`; Spark the `GET_JSON_OBJECT` text `'true'`/`'false'`). Ordered
comparisons against booleans and bool inline-map props fall back.

**Arithmetic predicates (node path): the safe subset.** `+`/`-`/`*` over typed
numeric properties (`n.age + 1 > 40`, `n.score * 2 >= 15.0`) are pushed: the
comparison operand becomes a small `ArithExpr` tree (property/literal/`+`·`-`·`*`),
each property cast to its declared type from `TypeHints` (so it computes
numerically on untyped-JSON dialects; the cast is harmless on typed). `+`/`-`/`*`
of ints/floats match SQL's promotion and the reference's f64-then-narrow result.
`/`, `%`, `^` are **excluded** (integer-vs-float division diverges: SQLite
`5/2 = 2`, Spark `5/2 = 2.5`, reference is float), as are unknown-typed and
string properties — those fall back. Requires type hints (a property's type must
be known), so a schemaless backend falls back. Applies to **both the node and
segment paths** (segment operands resolved through the variable→role map, node
kinds keyed by endpoint label, edge kinds by single relationship type).

Deferred (each gated by the oracle): `/`·`%`·`^` arithmetic (dialect-divergent);
variable-length with an edge-list (named relationship) binding; path variables;
`OPTIONAL MATCH` / `WITH` / `UNION` / multi-pattern `MATCH` (multi-clause shapes).

---

# M1 (Foundation) Checkpoint

This was a **STOP-for-review checkpoint** at the M1 (Foundation) milestone
boundary, as required by Guardrail 6 of `GQL_GOAL.md`. No publish, all work
committed on the feature branch. Nothing on `main` was touched.

## What landed (all additive, all green)

Seven commits, each independently green against the gate:

| Commit | Unit | Summary |
|---|---|---|
| `f002757` | Precondition | Checkpoint in-flight pre-release tree (incl. untracked `grust-postgres-pgq`) so the workspace builds |
| `e4e34b2` | — | `docs/GQL_GOAL.md` executable goal |
| `9048bca` | **1** | `src/gql.rs`: conformance spine — `GqlConformanceProfile`, `GqlFeature` taxonomy (74 features), structured `GqlError`, `feature_manifest()`/`support_summary()`, `tests/gql/*.json` corpus + integration test |
| `0dc490c` | **2a** | Relocated the ~17k-line inline `mod tests` into `src/tests.rs`; `lib.rs` 32,969 → 15,957 lines, verbatim, byte-identical tests |
| `143ceb4` | **3** | `src/lexer.rs`: span-bearing tokenizer (comments, keywords, quoted idents, params, string/numeric families, arrows, `..`, `;`-split), `LexError` → structured `gql_syntax` |
| `dff4fd8` | **4·1** | `src/ast.rs`: typed AST (statements, clauses, patterns, `Expr` tree with Pratt binding powers) |
| `1a356d3` | **4·2** | `src/parser.rs`: recursive-descent lexer→AST parser; span-bearing + feature-tagged (`CALL`→`Unsupported(ProcedureCall)`) errors |
| `bf9eb4a` | **4·3** | `src/semantics.rs`: scope/binding + element-kind checks + WITH-horizon + feature gates over the AST |

### Gate status (re-run to confirm)
```sh
cargo test -p grust-cypher --lib              # 412 passed, 0 failed (was 327 at baseline; +85 new)
cargo test -p grust-cypher --test gql_conformance   # 3 passed
cargo check -p grust-graph --features cypher,memory  # OK
cargo check -p grust-sail                      # OK
git diff --check                               # clean
```
No new compiler warnings. The 327 pre-existing tests are unchanged and still
pass; the +85 are new unit/integration tests for the new modules.

## Design stance for the night: additive only

Every change is a **new module** alongside the existing 16k-line `lib.rs`
implementation. The hand-written `cypher_*` parser/planner entrypoints are
**untouched**, so the strict-write surface and the Memory/Sail behavior are
provably unchanged (same 327 tests, byte-identical). The new lexer → AST →
parser → semantics pipeline is fully tested in isolation but is **not yet wired
into the production path**.

## Reviewability refactor (done after the checkpoint, on request)

The monoliths were subsequently decomposed (the user explicitly greenlit it):

- **`tests.rs` (17k) → `tests/` dir**: `mod.rs` (shared imports + helper) + seven
  themed submodules (≤4k each). Submodules reach crate internals via `use super::*`
  chained through `tests/mod.rs`. Verbatim split + rustfmt; 327 tests unchanged.
- **`lib.rs` (16k) → 176-line root + 9 modules** (ddl, parse, primitives, planner,
  eval_rows, restricted_values, projection, where_clause, returning; each ≤3186
  lines). Items moved at top-level boundaries; cross-module items raised to
  `pub(crate)` (functions, async fns, struct fields inside `struct {}` only, and
  the impl methods the compiler flagged). Public API unchanged.

Gate after refactor: 412 lib + 3 integration tests pass, 0 warnings,
facade(cypher,memory)/sail/turso compile.

## Deferred to review — DO NOT do these unsupervised

Done since the checkpoint:

- **grust-sail / grust-graph re-export narrowing** (commit on this branch).
  `grust-sail` now uses an internal `use grust_cypher::*` + explicit
  `pub use grust_cypher::{portable surface}`; the facade's `sail` feature enables
  `cypher`, the Cypher language surface is re-exported once (from the `cypher`
  block), and the `sail` blocks list only Sail-native items. This also fixed
  building the facade with `cypher`+`sail` together and removed dead
  `helix`/`ladybug` facade blocks. CHANGELOG Unreleased updated.

Still reserved for a human checkpoint (the monolith-integrating / highest-blast-
radius steps), in recommended order:

1. **Units 3/4 — wire the new pipeline into the production path.** Make the
   legacy `cypher_*` entrypoints compatibility wrappers over
   lexer→parser→semantics→(existing logical plans). This is where the new code
   becomes load-bearing and where behavior could drift — needs the AST→plan
   lowering (the remaining part of Unit 4) and careful diffing against the tests.
2. **Unit 5 — shared row model (highest blast radius).** Mandated two-phase:
   (5a) introduce `GqlRecord/GqlBinding/GqlTable/GqlScope`, make the existing
   `CypherResultTable`/`CypherReturn*` structs thin adapters, gated on **RETURN\***
   **ordering/JSON golden snapshots written BEFORE the swap** (not yet generated —
   the returning-execution API is woven through internal Memory-facade test
   helpers and faithful snapshots should be produced under review, as part of 5a);
   (5b) migrate callers, delete adapters.

## Suggested next session

Greenlight item 1 (wire the pipeline) or item 2·5a (row-model adapters + golden
snapshots) first — both unblock the most downstream work. Re-run the gate
between every sub-step. Keep the 327-count floor and the facade/Sail checks as
hard gates. Publishing remains suspended until explicitly requested.
