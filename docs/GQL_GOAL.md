# Grust GQL/Cypher Completion — Executable Goal

Status: **COMPLETE (2026-07-03) and merged.** This is the historical execution
record derived from `docs/GrustCypherFull.md` after an adversarial multi-agent
review. Its branch names, stop points, test counts, and temporary no-release
guardrails describe that completed effort; they do not override the current
repository rules in `AGENTS.md` or the current profile statement in
`docs/GQL_PROFILE_STATEMENT.md`.

Read `docs/GrustCypherFull.md` for the full rationale of each Unit/Phase. This
file is the order of operations, the gates, and the guardrails.

---

## Historical guardrails (applied to every task in this completed goal)

1. **PUBLISH OVERRIDE.** Do **not** `cargo publish`, `cargo package` for release,
   `cargo info` registry verification, tag/date a release, or convert
   `CHANGELOG.md` "Unreleased" into a dated entry. `AGENTS.md`'s
   "publish as part of the same release workflow" rule is **SUSPENDED** for this
   goal — releases happen only on explicit human request. If a task seems to
   require publishing, stop and report instead.
2. **327-TEST GATE.** Before any task is "done": `cargo test -p grust-cypher
   --lib` must report **≥327 passed, 0 failed**. The test count may only grow,
   never shrink. No test may be deleted or newly `#[ignore]`'d to pass (the
   pre-existing Sail live-server `#[ignore]`s are exempt).
3. **FACADE/SURFACE GATE.** Any task touching the cypher public surface (Units
   2, 5, 7, 10, 11): `cargo check -p grust-graph --features cypher,memory` **and**
   `cargo check -p grust-sail` must pass (add `cargo check -p grust-turso` for
   Unit 10). The `grust-graph` facade's named export list in
   `crates/grust/src/lib.rs` must compile unchanged, or the change is intentional
   and recorded in `CHANGELOG.md` "Unreleased".
4. **GIT HYGIENE.** Each task ends with a clean `git diff --check`. Run
   `docs/book/build.sh` **only** when the task changes public API names, feature
   flags, or user-visible behavior — pure internal refactors (e.g. the Unit 2
   split) must not regenerate binary book artifacts.
5. **NO HELIX/LADYBUG IN FACADE.** Do not re-add `grust-helix` or
   `grust-ladybug` to the `grust-graph` facade or any publish step; both are
   `publish = false`. Conformance/cost artifacts for them stay internal/test-only.
6. **MILESTONE CHECKPOINTS.** At each Release-Milestone boundary (see below), run
   the full gate, write a status report, and **STOP for human review** of
   public-API and semantics choices before proceeding. "Run until done" applies
   **within a stage**, never across the whole goal.

## Per-task Definition of Done (the pass/fail oracle)

The subjective "Done when" prose in `GrustCypherFull.md` is guidance. The
objective gate every task must pass:

```sh
cargo test -p grust-cypher --lib            # >=327 passed, 0 failed, count never shrinks
cargo check -p grust-graph --features cypher,memory
# surface-touching units (2,5,7,10,11) additionally:
cargo check -p grust-sail                   # + cargo check -p grust-turso for Unit 10
git diff --check
docs/book/build.sh                          # ONLY if public API/behavior changed
```

---

## Precondition (do before Task 1)

- Create/switch to feature branch `cypher-gql-full`.
- **COMMIT** the in-flight pre-release working tree (do **not** `git stash`:
  `crates/grust-postgres-pgq/` is untracked but already wired into `Cargo.toml`
  as a workspace member, so stashing leaves the workspace referencing a missing
  crate and nothing builds).

---

## Corrected execution order (DAG, not the prose "Practical Sequencing")

The plan's "Practical Sequencing" and "Release Milestones" prose contradict the
Units' own `Depends on:` lines (they run Unit 10 before its dependency Unit 9,
never schedule Unit 8, and pair Unit 12 with Unit 9 though it needs Unit 11).
**Follow this topologically-sorted DAG, not the prose order.** A task is
ineligible until all its predecessors are green.

| Task | Depends on | Notes |
|---|---|---|
| **P** Precondition | — | branch + commit |
| **U1** Manifest & conformance spine | P | DoD must create `crates/grust-cypher/tests/gql/` (absent today) with a runnable manifest + a support-summary generator. |
| **U2** Module split | U1 | Re-scoped: see decomposition. |
| **U3** Lexer & source spans | U2 | |
| **U4** Typed AST & semantics | U2, U3 | |
| **UT** Property-graph type system | U4 | **NEW** — owns Phase 2 (orphaned in the plan). Feeds U5 + U7. |
| **U5** Shared row model | U4, UT | Two-phase; widest blast radius. |
| **U6** Read-only `MATCH…RETURN` | U5 | |
| **U7** Expression engine | U4, U5, U6, UT | Decomposed by family; owns 3-valued logic. |
| **U8** Query composition | U5, U6, U7 | |
| **U9** Reference pattern matcher | U5, U6, U7, U8 | Decomposed; owns OPTIONAL MATCH. |
| **U10a** Write row-stream rebuild | U5, U6, U7 | **Fast path** — existing strict-write subset only. |
| **U10b** Pattern-driven write widening | U10a, U9 | Multi-row pattern writes. Resolves the 9-vs-10 conflict. |
| **U11** Schema, graph types, catalog | U1, U4, U5, U10a | |
| **U12** Backend conformance profiles | U1, U6, U9, U11 | |
| **U13** Transactions, sessions, control | U8, U10b, U11 | |
| **U14** Procedures, functions, escapes | U7, U8, U12 | |
| **U15** Optimizer & pushdown | U6, U7, U8, U9, U10b, U11, U12 | |
| **U16** Full-profile candidate hardening | all of the above | Terminal. |

## Milestone checkpoints (STOP for human review at each)

- **M1 GQL Foundation** — U1–U4. Grammar/AST/spans/feature-IDs; 327 green.
- **M2 Portable Query Core** — UT, U5–U8. Shared rows, types, expression engine,
  bounded read-only `MATCH…RETURN`, Memory execution.
- **M3 Portable Write Core** — U10a. Strict-write subset on shared row machinery.
- **M4 Schema Core** — U11. Graph types, constraints, catalog/session metadata.
- **M5 Pattern Core** — U9, then U10b. Quantified paths, OPTIONAL MATCH, path
  modes; pattern-driven write widening lands here, after U9 exists.
- **M6 Backend Profiles** — U12. Honest per-backend conformance manifests.
- **M7 Full Profile Candidate** — U13–U16. Transactions, procedures, optimizer,
  hardening; precise profile statement.

(M3 now legitimately precedes M5 because U10a depends only on U5–U7, not on U9.)

---

## Decomposition & corrections for the load-bearing units

### U1 — Manifest & conformance spine
- Pick **one** profile enum name and use it everywhere (plan currently mixes
  `GqlConformanceProfile`, `GrustCypherProfile::StrictWrite`,
  `GrustGqlProfile::Full39075`). Recommended: `GqlConformanceProfile` with
  variants `StrictWrite`, `PortableGql`, `Full39075`.
- DoD: `crates/grust-cypher/tests/gql/` exists with ≥1 runnable manifest and the
  support-summary generator reads from it. (Directory does not exist today.)

### U2 — Module split  ·  re-estimate 4–7 days, "structural, test-visibility-sensitive" (not "1–2 days mechanical")
The crate is one 32,960-line `lib.rs`; the 327-test `mod tests` (line 15948)
reaches impl via `use super::*` plus intra-crate private items. The bulk is flat
top-level impl (≈367 top-level fns, ≈85 structs/enums), **not** a big parser
module to lift out (`cypher_parser` is ~37 lines).
- **2a** Relocate the ~17k-line inline `mod tests` into a tests module / per-area
  `#[cfg(test)]` blocks without dropping count.
- **2b** Group flat top-level code into coarse modules (ast-ish, parse, ddl,
  plan, return, execute, compat); re-plumb visibility.
- **2c** Minimal split only — **defer** fine `ast.rs`/`plan.rs`/`execute.rs`
  seams until **after U5** fixes the row model, so seams are cut once against the
  target shape.
- Reword the bogus acceptance criterion. "No backend crate imports private
  parser internals" is vacuous (parser is `pub`). Real gate: **grust-cypher
  exposes a documented public parser/AST API and marks the rest `pub(crate)`;
  `grust-sail`'s `pub use grust_cypher::*;` glob (lib.rs:11) is replaced by an
  explicit named re-export.** The `grust` facade already uses a named ~32-symbol
  list — only Sail's glob needs fixing.

### UT (new) — Property-graph type system  ·  owns Phase 2
Typed temporal / duration / decimal / path / graph values; comparison, ordering,
arithmetic, coercion; null/missing and 3-valued semantics. Either implement here
feeding U5+U7, **or** explicitly scope temporal/duration/decimal **out** of the
initial `Full39075` profile with rationale in U16. Do not leave them as orphan
bullets — a "full 39075" claim is unbacked otherwise.

### U5 — Shared row model  ·  two-phase, high blast radius (gates U6/U8/U9/U10/U11)
- **5a** Introduce `GqlRecord` / `GqlBinding` / `GqlTable` / `GqlScope` and make
  the existing `CypherReturnClause`/`Projection`/`Element`/`Aggregate`,
  `CypherResultTable`, `CypherMutationTableResult` **thin adapters** over it. Old
  tests untouched, output **byte-identical**. Gate on 327-green **plus**
  `RETURN *` ordering / JSON golden snapshots written **before** the swap. **5a
  is the rollback point.**
- **5b** Migrate callers, delete adapters.
- DoD adds the facade + `grust-sail` checks.

### U7 — Expression engine  ·  decompose by family
- Registry scaffold first, then one green sub-task per family (arithmetic,
  boolean, comparison, null, string, numeric, list, map/record, conditional,
  function, property, parameter).
- **Add explicit 3-valued boolean logic deliverable:** TRUE/FALSE/UNKNOWN truth
  tables for AND/OR/NOT, UNKNOWN propagation in comparisons, WHERE/FILTER
  "keep only TRUE" rule, Memory as reference.
- Replace the soft "Memory and Sail agree" DoD with (a) a tested
  **compile-with-zero-backend-deps** gate on the reference evaluator and (b) a
  minimal **plan-equivalence / differential harness** pulled forward from
  Phase 12 asserting reference-vs-pushdown row identity. Keeps the expression
  engine from quietly becoming a second runtime.

### U9 — Reference pattern matcher  ·  decompose; owns OPTIONAL MATCH
- **9a** node / label / property match · **9b** edge + direction · **9c** bounded
  paths / path vars · **9d** alternation / conjunction · **9e** cardinality &
  pushdown descriptors.
- **Add OPTIONAL MATCH** with null-padding semantics and its interaction with
  downstream WHERE/aggregation; Memory reference behavior + conformance cases.
- Manifest invariant: **no backend may advertise Native/Hybrid path support until
  the matching Memory reference path test passes.**

### U10 — Write core  ·  split to resolve the 9-vs-10 contradiction
- **U10a** Row-stream rebuild of the **existing strict-write subset only**
  (depends U5–U7; no pattern-matched multi-row writes). Edit the scope prose that
  couples writes to "pattern matching" (GrustCypherFull.md line ~275).
- **U10b** Pattern-driven write widening (depends U10a + U9).
- Add **Turso** to the parity invariant (`CypherMutationExecutor` is implemented
  by grust-memory, grust-sail, **and grust-turso**) or scope it out explicitly.
  DoD adds `cargo check -p grust-sail` and `cargo check -p grust-turso`.
- Cross-reference `CypherWrite.md`'s explicit current-rejection list; enumerate
  which v1 rejections are relaxed vs kept, so "widen" is auditable.

### U11 — Schema / catalog
- Concrete starting point: `CypherConstraintRegistry` / `CypherSchemaManager`.
- Add open-vs-typed/closed-graph distinction and write-time type-violation
  behavior; capability model reports graph-type enforcement per backend (not just
  unique/required).

### U12 — Backend conformance profiles  ·  fix the stale matrix
- Add `grust-turso` and `grust-postgres` / `grust-postgres-pgq` to the manifest
  (tie `grust-postgres-pgq` to the Phase 12 SQL/PGQ shared-pattern test — it's the
  only PGQ-native backend and is currently missing).
- Annotate `grust-helix` / `grust-ladybug` as `publish=false` / out-of-facade
  (internal-only). Carve `grust-cocoindex` out of the executing-backend
  conformance set (it's a sync/export target, not a query backend).
- Prefer publishable backends for the "≥2 non-Sail persistent backends" gate.

---

## Doc-hygiene fixes (do as encountered, not a separate phase)

- Add a **Units ↔ Phases ↔ Milestones** correspondence table near the top of
  `GrustCypherFull.md`.
- Resolve grust-cypher → grust-gql rename: state whether/when, and which Unit owns
  it.
- Reconcile the conformance harness as **either** a `conformance.rs` module **or**
  a separate `grust-cypher-conformance` crate — identically in both Cypher docs
  (if a crate, it slots into the AGENTS.md publish order).
- Pick one canonical Procedures-vs-Transactions order across Units and Phases.
- Keep the intro's "controlling property graphs" wording — it correctly refers to
  GQL session/transaction/catalog control (U13/Phase 10). The real gap is no
  access-control/security DDL; note it out-of-profile.
- **AGENTS.md publish list is stale:** drop `grust-helix` (now publish=false); add
  `grust-sql-core`, `grust-postgres-core`, `grust-postgres`, `grust-postgres-pgq`,
  `grust-turso` in dependency order. Reconcile its auto-publish mandate against
  RESTART.md's "publish only when explicitly requested" in one place. **Defer the
  actual publish to a human-requested release step.**

---

## Top risks (carry forward)

1. AGENTS.md auto-publish would irreversibly publish after every Unit — neutralized
   by Guardrail 1.
2. U5 big-bang row swap with no rollback — neutralized by the 5a adapter phase +
   pre-swap golden snapshots.
3. U2 mis-scaled monolith split — neutralized by re-estimate + leaf-first
   decomposition with green checkpoints.
4. Public-surface churn breaking the facade / Sail glob / Turso — neutralized by
   the facade/surface gate.
5. Prose ordering contradicts the DAG — neutralized by following the table above.
6. Orphaned mandatory ISO areas (3VL, OPTIONAL MATCH null-padding, temporal/
   decimal) — neutralized by assigning them to U7 / U9 / UT or scoping out in U16.
7. Expression engine becoming a second runtime — neutralized by U7's
   zero-backend-deps gate + differential harness.
