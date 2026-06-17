# Restart Checkpoint

Generated: 2026-06-17 04:02:09 PDT

## Current Goal

Continue implementing bounded writable Cypher features from
`docs/CypherWrite.md` on branch `codex/cypher-write`.

## Current Instructions

- Keep committing locally.
- Push is allowed now that GitHub SSH access is available.
- Do not publish crates until the next major checkpoint and explicit release
  direction.
- When publishing the new crates, bump the version, publish in dependency
  order, verify the published crates with `cargo info <crate>@<version>` from
  outside the workspace, and notify the active TypeSec and Lakecat goal threads
  to rebuild against the published versions. Lakecat especially should rebuild
  in GitHub CI after pushing.
- Do not stage or modify untracked `OPUS1.md` unless explicitly asked.

## Branch And Worktree

- Branch: `codex/cypher-write`
- Remote tracking branch: `origin/codex/cypher-write`
- Expected untracked file: `OPUS1.md`
- Latest completed slice before this checkpoint: Batch DC, restricted
  mutating `MATCH ... WHERE variable.property IS NULL` predicates and explicit
  null-check predicate operators.

## Latest Completed Work

Batch DC completed explicit null-check predicates in the bounded mutating
Cypher `MATCH ... WHERE` grammar:

```cypher
MATCH (n:Person)
WHERE n.nickname IS NULL
SET n.needs_nickname = true;
```

Implemented behavior:

- `grust-core` exposes explicit `GraphPredicateOp::IsNull` and
  `GraphPredicateOp::IsNotNull` operators.
- `grust-sail` parses `variable.property IS NULL`,
  `variable.property IS NOT NULL`, `NOT variable.property IS NULL`, and
  `NOT variable.property IS NOT NULL`.
- Null-check syntax lowers to the explicit backend-neutral predicate operators
  with `Value::Null`.
- Node, relationship, and endpoint variables can use the predicate wherever
  ordinary `AND`-joined mutating `WHERE` predicates are accepted.
- `IS NULL` matches missing properties and explicit `Value::Null` properties.
- `IS NOT NULL` requires a present non-null property.
- Ordinary equality and inequality remain missing-safe exact `Value`
  comparisons.
- Core predicate tests plus Sail planner and Memory-facade tests cover the
  semantics.

Files updated:

- `crates/grust-core/src/lib.rs`
- `crates/grust-core/src/tests.rs`
- `crates/grust-sail/src/lib.rs`
- `crates/grust-sail/src/tests.rs`
- `docs/CypherWrite.md`
- `CHANGELOG.md`
- `docs/book/manuscript.md`
- rebuilt book artifacts under `docs/book/build/dist/`

## Verification State

The following commands passed after Batch DC:

```sh
cargo fmt --all
cargo test -p grust-sail --lib
cargo test -p grust-core --lib
cargo test -p grust-memory --lib
cargo check -p grust-graph --features sail,memory
docs/book/build.sh
git diff --check
```

Observed test summaries:

- `cargo test -p grust-sail --lib`: 182 passed, 25 ignored
- `cargo test -p grust-core --lib`: 35 passed
- `cargo test -p grust-memory --lib`: 20 passed

Book rebuild output confirmed:

- `docs/book/build/dist/grust.pdf`
- `docs/book/build/dist/grust.epub`
- `docs/book/build/dist/grust (0.9.0).epub -> grust.epub`
- `docs/book/build/dist/grust.mobi`
- `docs/book/build/dist/VERSION.md`

## Next Safe Continuation

Keep the next slice small and continue preserving these invariants:

- Cypher remains a syntax layer over Grust-owned mutation semantics.
- Writable `RETURN` helpers operate only over already-bound/materialized write
  result rows.
- Mutating `WHERE` stays backend-neutral through `GraphPropertyPredicate`.
- No arbitrary expression engine, cross-variable expressions, path-pattern
  predicates, or list/map expression evaluation should be introduced.

Potential next bounded slices:

- Extend mutating `WHERE` with a tiny, explicit `OR` representation if the core
  predicate model grows disjunction support.
- Add another restricted writable `RETURN` helper from the remaining batches in
  `docs/CypherWrite.md`.

For each slice, update code, tests, `CHANGELOG.md`, `docs/CypherWrite.md`, book
manuscript, rebuilt book artifacts, then commit and push.
