# Grust Pushdown 2 Goal — lowering the Full39075 read features into backend SQL

Status: **in progress — PM1 (P0–P2) and PM2 (P3–P4a) done on branch `pushdown2`.** This is the agreed next goal after the
Full39075 completion goal (`docs/GQL_FULL39075_GOAL.md`, done 2026-07-03). It
plans the lowering of the newer read features — table-valued functions,
`CALL { … }` subqueries, and shortest-path matching — into the backend-neutral
read pushdown (`crates/grust-cypher/src/pushdown.rs`, Unit 15), so they
execute in backend SQL instead of only on the Memory reference.

**Do not start this goal casually.** It is sized like Unit 15 (a multi-session
unit with its own milestones), it is dialect-divergent from day one, and every
task is gated by the differential oracle. Read `docs/GQL_M1_CHECKPOINT.md`'s
Unit 15 section first for the existing pushdown architecture and its deferred
niche tails (some of which this goal subsumes).

## Why (and why it can wait)

Today F8/F9/F10 execute on the Memory reference only. That is *correct* — the
pushdown planner returns `None` for any query outside the pushable subset and
callers fall back to the reference — but it means backends materialize a full
graph snapshot for these queries. This goal removes that cost where SQL can
express the query. It should be scheduled when a real workload shows the
Memory-only execution of these features is a bottleneck, not before.

## Architecture invariants (carry over from Unit 15 — non-negotiable)

1. **Byte-identical by construction.** Only the `MATCH`/`WHERE`/row-source
   part of a query is lowered to SQL; the `RETURN`/`WITH` tail always runs
   through the shared reference projection (`read::project_bindings` /
   `project_binding_pipeline`). Pushed results must equal the reference
   *by construction*, never by coincidence.
2. **No silent wrong answers.** A shape the planner cannot push returns
   `None` (reference fallback) or a structured error — never a partially
   pushed query.
3. **Oracle per shape.** Every newly pushable shape lands with differential
   cases in the `grust-turso` embedded-SQLite oracle
   (`crates/grust-turso/tests/read_pushdown_oracle.rs`): reference rows ==
   pushed rows on the same data, including ordering when `ORDER BY` is pushed.
4. **Corpus reuse.** Where a `tests/gql/portable_read.json` case becomes
   pushable, add it (or its shape) to the oracle so the conformance corpus and
   the pushdown corpus cannot drift.
5. **Capability honesty.** If a feature is pushable on one dialect and not
   another, that is expressed in planner dialect gates (and, if user-visible,
   backend descriptor flags) — not by weakening semantics on the weaker
   dialect.
6. **Test floor and guardrails** from `docs/GQL_FULL39075_GOAL.md` apply
   unchanged: counts only grow, no release without explicit request.

## Dialect reality check (informs every estimate)

- **SQLite (Turso oracle, `SqlDialect::Sqlite`)**: recursive CTEs, JSON1
  (`json_each`, `json_extract`), correlated scalar subqueries. No `LATERAL`.
  This is the *lead dialect* — everything lands here first.
- **Spark (Sail, `SqlDialect::Spark`)**: no recursive CTEs (verify against the
  Sail/Spark version in use — this single fact decides whether shortest path
  and unbounded var-length can ever push to Sail); `explode`, `map_keys`,
  higher-order functions available. Spark parity is a *trailing* milestone per
  task, never a blocker for the SQLite slice.

## Sequenced tasks

| Task | Feature slice | Depends on | Sketch |
|---|---|---|---|
| **P0** | Inventory + fallback pin | — | **Done.** `full39075_read_features_fall_back_to_the_reference` pins subqueries, correlated TVF args, and shortest-path shapes to `Ok(None)`. Pinning found and fixed a real bug: the F10 `shortestPath(…)` wrapper was not rejected by the lowerer guards, so a bare wrapped var-length pattern lowered as a plain var-length scan (wrong rows on Sail). Every pattern guard now rejects `shortest`. |
| **P1** | Catalog procedures as SQL | P0 | **Done.** `ProcedureReadPushdown` leaf: `db.labels`/`db.relationshipTypes` as DISTINCT scans (both dialects), `db.propertyKeys` via `json_each` (SQLite-gated through `SqlDialect::json_props_keys_scan`; Spark falls back via the new `ReadPushdown::supported_by`). YIELD/WHERE/tail run through `read::project_procedure_pipeline`. Oracle-backed. |
| **P2** | `tvf.range` row source | P1 | **Done (rescoped).** `tvf.range` with constant/parameter integer args → guarded recursive CTE (SQLite-gated through `SqlDialect::integer_series_sql`; empty ranges and negative steps match the reference; zero step falls back so the structured error stays identical). Spark parity deferred pending Sail `sequence`/`explode` verification. `tvf.keys` is inherently correlated (keys of a bound element), so it moves to P4's correlated scope and stays reference-only for now — pinned by the P0 test. |
| **P3** | Uncorrelated subqueries | P0 | **Done.** `SubqueryReadPushdown`: leading `CALL { … }` (single scan) and `MATCH × CALL { … }` (a `LEFT JOIN ON 1=1` of the two scans — LEFT, not CROSS, so an inner-aggregate over an empty inner scan still yields its one row per outer row, exactly like the reference). Inner pipeline, subquery-RETURN join, and outer tail run through `read::project_subquery_join_pipeline`; correlation (including same-name shadowing) falls back conservatively. Both dialects; oracle-backed. The oracle work also exposed and fixed a latent **reference** bug: `dedup_bindings` re-evaluated pre-projection expressions against post-projection rows, so `DISTINCT` over computed items errored. |
| **P4** | Correlated subqueries (bounded) | P3 | **P4a done:** correlated `tvf.keys(n)` over the outer scan variable → lateral `json_each` join (`SqlDialect::lateral_json_keys_sql`, SQLite-gated; stored props are sorted JSON so key order matches the reference). **P4b deferred to PM3:** the P3 machinery generalizes — render the inner `WHERE`'s cross-scope predicate into the `LEFT JOIN ON` clause (o-/i-qualified operands, reusing the segment predicate machinery) and the whole inner pipeline stays in the reference, aggregates included; no SQL-side aggregate is ever needed. Needs a two-scope qualified predicate renderer. |
| **P5** | Shortest path (SQLite first) | P0 | Recursive CTE BFS with visited-set tracking (path string or JSON array), minimal-length selection per endpoint pair (`MIN(depth)` join), tie handling for `allShortestPaths`. Deterministic ordering must match the reference's edge-order determinism — expect this to be the hardest equivalence argument in the goal. Spark: only if recursive CTEs exist in the Sail version; otherwise document as reference-only. |
| **P6** | Oracle + corpus expansion | each of P1–P5 | Differential oracle cases per shape; promote pushable `portable_read.json` cases into the oracle. |

## Milestones

- **PM1 Row sources (P0–P2):** **done (2026-07-04)** — catalog procedures push
  on both dialects (propertyKeys and tvf.range SQLite-gated); oracle green
  (+2 differential tests over the turso and rusqlite engines).
- **PM2 Subqueries (P3–P4):** **done (2026-07-04, P4b deferred to PM3)** —
  uncorrelated subqueries push on both dialects, correlated `tvf.keys` on
  SQLite; oracle green (+2 differential tests). Correlated subqueries via
  LEFT-JOIN-ON follow in PM3 alongside shortest path.
- **PM3 Shortest path (P5):** SQLite recursive-CTE shortest path with the
  determinism-equivalence argument written down; Spark explicitly scoped in
  or out based on the recursive-CTE check.
- **PM4 Claim + docs:** update `docs/GQL_M1_CHECKPOINT.md` Unit 15 status,
  the profile statement's per-backend section, and backend descriptors if any
  user-visible capability changed. STOP for human review before merging.

## Rough size

PM1 ≈ one focused session. PM2 ≈ one to two sessions. PM3 ≈ two-plus sessions
(the equivalence argument, not the SQL, is the cost). Comparable overall to
the original Unit 15.
