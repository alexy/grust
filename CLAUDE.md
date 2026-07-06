# Grust — agent context & session handoff

Grust is a backend-neutral **property-graph** library for Rust: one query/mutation
model over many backends (Memory, Sail/Spark, Postgres, pgGraph, Turso, SurrealDB,
Falkor, LanceDB, CocoIndex, …). The `grust-cypher` crate is the Cypher→GQL
language layer. Release/publish rules live in `AGENTS.md` — read it before any
release work.

---

## STATUS — GQL/Cypher goals COMPLETE (branch `full39075`, 2026-07-03)

Both GQL goals are **done**:

1. **GQL completion goal** (`docs/GQL_GOAL.md`, was branch `cypher-gql-full`):
   merged to `main`. New lexer→parser→semantics pipeline, Memory read
   reference executor, read pushdown (Sail/SQLite + differential oracle),
   decimal/duration/path types, procedures, transaction surface, write
   accept-set cutover (decision B) + W1–W3 write widenings, profile statement.
2. **Full39075 completion goal** (`docs/GQL_FULL39075_GOAL.md`): **all of
   F1–F11 + the FM5 claim flip are done** on branch `full39075` (local).
   F1 index DDL, F2 graph type DDL, F3 catalog metadata, F4 `USE <graph>`,
   F5 session control, F6 `Value::Path`, F7 `Value::Graph`, F8 `CALL { … }`
   subqueries, F9 table-valued functions (`CALL name(args)`, `tvf.range`,
   `tvf.keys`), F10 `shortestPath`/`allShortestPaths`, F11 native passthrough
   (`NativeQuery` + Falkor/Surreal catalog entries + `run_native_cypher` /
   `run_native_surrealql` escape hatches).

**The realized profile is `Full39075`**: 69 of 74 manifest features
`Supported`; the other 5 are intentional strict-write rejections. Pinned by
`gql::tests::full_profile_claim_is_backed`; stated in
`docs/GQL_PROFILE_STATEMENT.md`. Book/CHANGELOG updated; book artifacts
rebuilt. **No release performed (guardrail 1).** Branch `full39075` is ready
for human review / merge to `main`.

### NEXT (if resuming)
- **PUSHDOWN2 is merged to `main`** (`docs/GQL_PUSHDOWN2_GOAL.md`): row
  sources, subqueries, and endpoint-only shortest paths push to SQL,
  oracle-backed. Remaining niche tails live in `docs/GQL_M1_CHECKPOINT.md`
  Unit 15 (edge-list bindings, path variables, `/`·`%`·`^` arithmetic, Spark
  parity for the SQLite-gated leaves).
- **Postgres executor goal is implementation-complete**
  (`docs/GQL_POSTGRES_EXECUTOR_GOAL.md`, branch `postgres-executor`): the
  executing set is Memory/Sail/Turso/**Postgres**, proven by the gated live
  suite (`GRUST_PG_URL="host=127.0.0.1 user=alexy dbname=grust_test" cargo
  test -p grust-postgres-core -- --ignored`; a local Homebrew PG 17 serves
  grust_test). Awaiting human review before merge. Follow-ups in the goal
  doc: PGQ executor delegation, shortest-path ordinal migration, PG
  type-hints wiring.

---

## GUARDRAILS (do not violate without explicit ask)
1. **No publish/release.** Do NOT `cargo publish`, `cargo package` for release,
   `cargo info` verify, tag/date a release, or convert `CHANGELOG.md`
   "Unreleased" into a dated entry. `AGENTS.md`'s auto-publish rule is
   **suspended** for this goal. CHANGELOG "Unreleased" notes are fine.
2. **Test floor:** `cargo test -p grust-cypher --lib` must stay green and the
   count must only grow (574 right now; 327 of those are the original strict-write
   suite — never delete/`#[ignore]` to pass). 
3. **Additive discipline:** the legacy strict-write planner and the stable
   Memory/Sail behavior are not to be changed destructively. New work lands as
   new modules; the write path is rewired only under review (track 2 above).
4. **Commit cadence:** one green, self-contained commit per sub-step; end commit
   messages with a `Co-Authored-By` trailer for the model used. Branch only —
   no commits to `main`. Push when asked.

## VERIFY (the gate — run between steps)
```sh
cd ~/src/grust
cargo test  -p grust-cypher                         # 574 lib + 3 + 17 integration, 0 failed
cargo build -p grust-cypher --lib 2>&1 | grep -c warning:   # expect 0
cargo check -p grust-graph --features cypher,memory         # facade
cargo check -p grust-sail                                   # surface-touching units
cargo test  -p grust-turso                                  # 12 lib + 14 pushdown oracle (Unit 15)
git  diff --check                                           # whitespace clean
```
No external services needed for the cypher work (Memory reference + unit tests).
Two pre-existing crawler/Sail live-server tests are `#[ignore]` by design.

## grust-cypher module map (`crates/grust-cypher/src/`)
- `lib.rs` (~190 lines) — crate root: module wiring, re-exports, option/result
  types, the two top-level plan entrypoints.
- New pipeline: `gql.rs` (manifest + backend catalog + native passthrough),
  `lexer.rs`, `ast.rs`, `parser.rs`, `semantics.rs`, `read.rs` (the Memory read
  executor incl. subqueries/TVFs/shortest path), `pushdown.rs` (backend-neutral
  read pushdown → SQL; Unit 15), `catalog.rs`, `graph_type_ddl.rs`,
  `session.rs`, `transaction.rs`, `graph_type.rs`.
- Legacy (strict-write) split: `ddl.rs`, `parse.rs`, `primitives.rs`,
  `planner.rs`, `eval_rows.rs`, `restricted_values.rs`, `projection.rs`,
  `where_clause.rs`, `returning.rs`.
- Tests: `tests/` dir (per-area submodules) + integration `tests/gql_conformance.rs`,
  `tests/read_conformance.rs`, corpus `tests/gql/*.json`.

Verdun (`../../verdun`) is a local path dep for the Rust crawler only; not
relevant to grust-cypher.

## Resuming in CLI / remote
- `git switch full39075` (the completed working branch; merge to `main` is a
  human call).
- Read `docs/GQL_FULL39075_GOAL.md` (completed plan + records) and
  `docs/GQL_PROFILE_STATEMENT.md` (the realized-profile claim) first.
- Keep the commit-per-green cadence and run the VERIFY gate between steps.
