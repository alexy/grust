# Grust — agent context & session handoff

Grust is a backend-neutral **property-graph** library for Rust: one query/mutation
model over many backends (Memory, Sail/Spark, Postgres, pgGraph, Turso, SurrealDB,
Falkor, LanceDB, CocoIndex, …). The `grust-cypher` crate is the Cypher→GQL
language layer. Release/publish rules live in `AGENTS.md` — read it before any
release work.

---

## ACTIVE WORK — GQL/Cypher completion (branch `cypher-gql-full`)

We are executing the GQL completion goal. **Everything below is on branch
`cypher-gql-full`** (pushed to `origin`), **not `main`**. ~29 commits ahead of
`main`, nothing on `main` touched.

- **Goal contract (read first):** `docs/GQL_GOAL.md` — the executable plan:
  guardrails, the corrected dependency DAG, decomposed units, milestone
  checkpoints. Derived from `docs/GrustCypherFull.md` after a multi-agent review.
- **Progress checkpoint:** `docs/GQL_M1_CHECKPOINT.md` — what landed in M1
  (Foundation), M2 (read query core), and Unit 15 (read pushdown), and what's
  deferred. **Read its "Unit 15 — read pushdown (PAUSED)" section first** for the
  current consolidated status + future-work list.

### Status: M1 done, M2 (read side) done, Unit 15 (read pushdown) done & paused
- **M1 Foundation** (additive, alongside the untouched legacy planner):
  `gql` (conformance spine: `GqlFeature` taxonomy + structured `GqlError`),
  `lexer` (span-bearing), `ast`, `parser` (recursive descent), `semantics`
  (scope/binding/kind + feature gates).
- **Reviewability refactor:** the former 33k-line `lib.rs` and 17k-line
  `tests.rs` were split into cohesive modules + a `tests/` dir (see map below).
  Public API unchanged; internals are `pub(crate)`.
- **Facade cleanup:** `grust-sail` no longer globs `pub use grust_cypher::*`;
  `grust-graph`'s `sail` feature now implies `cypher` and re-exports the Cypher
  surface once. `--features cypher,memory,sail` now builds.
- **M2 read core (`src/read.rs`)** — a Memory **reference executor** over a
  `Graph` snapshot (`run_read_query(&Graph, cypher, &CypherParameters)`):
  MATCH / OPTIONAL MATCH (null-padding) / multi-hop / variable-length `*m..n` /
  path variables; WHERE with a general expression engine (3-valued boolean,
  comparison, IN, IS NULL, STARTS/ENDS WITH, CONTAINS, CASE, arithmetic, scalar
  + list + element-introspection + path functions); RETURN with aliases, `*`,
  DISTINCT, ORDER BY, SKIP, LIMIT, aggregates (count/sum/avg/min/max/collect) +
  implicit GROUP BY; WITH horizon; UNWIND; UNION/UNION ALL. The `GqlFeature`
  manifest marks these `Supported`; corpus in `tests/gql/portable_read.json`.

- **Unit 15 read pushdown (`src/pushdown.rs`)** — DONE & paused. Backend-neutral
  lowering of the bounded read filter into SQL (`SqlDialect`: Spark + SQLite);
  `RETURN` runs through the shared reference so results are byte-identical by
  construction. Covers single node, 1..N relationship segments (out/in/undirected,
  multi-type, inline props), variable-length `*m..n` (anonymous rel, recursive
  CTE); `WHERE` with comparisons / `IS NULL` / `IN` / string preds / boolean /
  `+`·`-`·`*` arithmetic / `AND·OR·NOT`; `ORDER BY`/`SKIP`/`LIMIT` into SQL
  (typed-JSON always, Spark via schema `TypeHints`). Wired into
  `SailGraphStore::run_read_query`. Differential oracle in
  `grust-turso/tests/read_pushdown_oracle.rs` (embedded SQLite incl. `rusqlite`
  for recursive CTEs). **See the checkpoint's Unit 15 section for the full
  future-work list.**

### AUTONOMOUS LOOP — running; Unit 10a forked (2026-06-25)
The 3 earlier decisions were answered (1a / 2a / 3-procedures) and the self-paced
`/loop` resumed. Landed since (all green, pushed):
- **Unit 14 DONE** — read-only catalog procedures `CALL [YIELD]` (`db.labels`,
  `db.relationshipTypes`, `db.propertyKeys`): `ast::CallClause`/`Clause::Call`,
  `parser::parse_call`, `read::call_procedure`; `ProcedureCall` now `Supported`.
  (Earlier: function expansion sqrt/exp/ln/log/log10/sin/cos/tan.)

**Unit 10a write cutover is BLOCKED on a NEW fork** (not the one decision (a)
answered). Decision (a) assumed the new pipeline is a *superset* needing its
rejections re-imposed; empirically it's the reverse in places — the legacy
planner **accepts non-standard Cypher the new standards-conformant parser
rejects** (`DELETE (:Person {id})` / `DELETE (:a)-[:R]->(:b)`: DELETE by
*pattern*, pinned by the 327 tests). "Byte-identical + same accept-set" needs a
human product call: **(A)** extend the new parser with the legacy quirks;
**(B)** migrate those tests to standard Cypher and narrow the public accept-set;
**(C)** keep the legacy write entrypoint. See the checkpoint's top "Unit 10a …
BLOCKED" section + `tests/golden/write_corpus.json` (181-statement parity corpus).

**Unit T DONE (core)** — first-class `grust_core::Decimal` + `Duration` types and
`Value::Decimal`/`Value::Duration` variants wired workspace-wide (every exhaustive
`Value` match fixed behavior-preservingly; all backends serialize as canonical/ISO
strings). Read executor: `decimal()`/`duration()` constructors, lossless decimal
`+`/`-`/`*`, duration `+`/`-`, exact comparison/ordering. `TemporalValues`/
`DurationValues`/`DecimalValues` → `Supported`. Deferred niche tail: decimal/
duration SQL pushdown + oracle (dialect-divergent; reference handles them today).

**Unit 13 DONE (fork-independent slice)** — `grust_cypher::transaction`:
`TransactionCommand::parse` for `START TRANSACTION`/`BEGIN`/`COMMIT`/`ROLLBACK`
(keywords intentionally NOT reserved) + honest per-backend atomicity capability
reporting (`GqlBackend::transactional`, descriptor flag, `transactional_backends`).
Manifest `TransactionControl`→Supported, `SessionControl`→Planned. Deferred
(write-path-coupled): atomic batch execution + session SET/RESET/USE.

### AUTONOMOUS LOOP — STOPPED at the terminal review gate (2026-06-25)
Every unit doable without the human fork decision is **done & pushed**:
Unit 14 (procedures), Unit T (decimal/duration), Unit 13 (transaction surface).
Remaining work all requires the human:
- **Unit 10a/10b** — BLOCKED on the A/B/C parser-accept-set fork (see top section).
- **Unit 16** (full-39075 hardening) — terminal review milestone; also gated on
  10a/10b (can't claim the full write profile while the cutover is forked).

The loop stopped here per its "STOP for human review at Unit 16 / surface on a
genuine fork" contract. **To resume: answer A/B/C for Unit 10a**, then 10a→10b→16.
Current: cypher 521 lib / 3 / 17, core 40, turso 7+14, 0 warnings, branch clean & pushed.

### NEXT — decision point (pick a track when resuming)
The safe additive read work (incl. Unit 15 pushdown) is done. Remaining:
1. **Unit 15 pushdown tails** — `UNION`/`UNION ALL`, `OPTIONAL MATCH`,
   multi-pattern `MATCH`, and the `WITH` horizon are all **done**. Remaining are
   niche/low-value: chained/multi-segment `OPTIONAL`; post-`WITH` `MATCH`
   (correlated); `/`·`%`·`^` arithmetic (dialect-divergent); named-rel-edge-list
   var-length; path variables. See checkpoint Unit 15.
2. **Write-path rewiring (Unit 10)** — make the legacy `cypher_*` write
   entrypoints run through the new pipeline. **Review-flagged as highest-risk**
   (behavior could drift across the 327 strict-write tests); GQL_GOAL.md mandates
   golden snapshots first + a milestone checkpoint. Do NOT do this unsupervised.
3. **Type system (Unit T)** — temporal/duration/decimal values.
4. **Transactions / catalog / procedures (Units 11/13/14).**

---

## GUARDRAILS (do not violate without explicit ask)
1. **No publish/release.** Do NOT `cargo publish`, `cargo package` for release,
   `cargo info` verify, tag/date a release, or convert `CHANGELOG.md`
   "Unreleased" into a dated entry. `AGENTS.md`'s auto-publish rule is
   **suspended** for this goal. CHANGELOG "Unreleased" notes are fine.
2. **Test floor:** `cargo test -p grust-cypher --lib` must stay green and the
   count must only grow (517 right now; 327 of those are the original strict-write
   suite — never delete/`#[ignore]` to pass). 
3. **Additive discipline:** the legacy strict-write planner and the stable
   Memory/Sail behavior are not to be changed destructively. New work lands as
   new modules; the write path is rewired only under review (track 2 above).
4. **Commit cadence:** one green, self-contained commit per sub-step; end commit
   messages with the `Co-Authored-By: Claude Opus 4.8` trailer. Branch only —
   no commits to `main`. Push when asked.

## VERIFY (the gate — run between steps)
```sh
cd ~/src/grust
cargo test  -p grust-cypher                         # 515 lib + 3 + 13 integration, 0 failed
cargo build -p grust-cypher --lib 2>&1 | grep -c warning:   # expect 0
cargo check -p grust-graph --features cypher,memory         # facade
cargo check -p grust-sail                                   # surface-touching units
cargo test  -p grust-turso                                  # 7 lib + 14 pushdown oracle (Unit 15)
git  diff --check                                           # whitespace clean
```
No external services needed for the cypher work (Memory reference + unit tests).
Two pre-existing crawler/Sail live-server tests are `#[ignore]` by design.

## grust-cypher module map (`crates/grust-cypher/src/`)
- `lib.rs` (~190 lines) — crate root: module wiring, re-exports, option/result
  types, the two top-level plan entrypoints.
- New pipeline: `gql.rs`, `lexer.rs`, `ast.rs`, `parser.rs`, `semantics.rs`,
  `read.rs` (the Memory read executor), `pushdown.rs` (backend-neutral read
  pushdown → SQL; Unit 15).
- Legacy (strict-write) split: `ddl.rs`, `parse.rs`, `primitives.rs`,
  `planner.rs`, `eval_rows.rs`, `restricted_values.rs`, `projection.rs`,
  `where_clause.rs`, `returning.rs`.
- Tests: `tests/` dir (per-area submodules) + integration `tests/gql_conformance.rs`,
  `tests/read_conformance.rs`, corpus `tests/gql/*.json`.

Verdun (`../../verdun`) is a local path dep for the Rust crawler only; not
relevant to grust-cypher.

## Resuming in CLI / remote
- `git switch cypher-gql-full` (already the working branch; pushed to origin).
- Read `docs/GQL_GOAL.md` (guardrails + DAG) and `docs/GQL_M1_CHECKPOINT.md`
  (latest progress) before continuing.
- Use the in-repo task list / pick a NEXT track above; keep the commit-per-green
  cadence and run the VERIFY gate between steps.
