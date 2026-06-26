# GQL Completion — M1/M2 Checkpoint

Branch: `cypher-gql-full` (pushed to origin). Goal contract: `docs/GQL_GOAL.md`.
Plan: `docs/GrustCypherFull.md`.

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

## Unit 15 — read pushdown (PAUSED at a comprehensive point)

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
4. **Multi-clause shapes** — `UNION`/`UNION ALL` and `OPTIONAL MATCH` (single
   optional segment) are **done** (see above). Remaining: multi-pattern `MATCH`
   (comma patterns → cross/natural join over a global alias set); `WITH` horizon
   (sub-plan composition / CTE); chained/multiple `OPTIONAL MATCH` and optional
   multi-segment. These need a general pattern-join planner; substantial.

Other backends: only Sail wires pushdown into its read entrypoint today. Turso
is used as the oracle but its own `run_read_query` is not wired (and its tagged
JSON storage would need a tagged dialect variant). Postgres/pgGraph/pgq backends
could reuse the same `SqlDialect` IR.

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
