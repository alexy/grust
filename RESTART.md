# Restart Checkpoint

Generated: 2026-06-17 04:25:24 PDT

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
- Latest completed slice before this checkpoint: Batch DG, restricted
  same-property equality `OR` predicates in mutating `MATCH ... WHERE`.

## Latest Completed Work

Batch DG added restricted same-property equality `OR` predicates to bounded
mutating Cypher
`MATCH ... WHERE`:

```cypher
MATCH (n:Person)
WHERE n.status = 'active' OR n.status = 'pending'
SET n.reviewed = true;
```

Implemented behavior:

- `grust-sail` splits each top-level `AND` term on top-level `OR`.
- Accepted `OR` groups must consist only of equality predicates over the same
  matched variable and property.
- Literal and parameter equality values are accepted only when they are scalar
  string, integer, float, or boolean values compatible with restricted
  membership predicates.
- Accepted groups fold to one backend-neutral `GraphPredicateOp::In`
  predicate, reusing Sail SQL membership lowering and Memory execution.
- Mixed-property, mixed-variable, non-equality, null-valued, nested-`NOT`,
  function, pattern, arbitrary-expression, and general boolean-expression `OR`
  forms remain deferred and rejected.
- Missing properties keep the existing folded-membership behavior and do not
  match.

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

The following commands passed after Batch DG:

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

- `cargo test -p grust-sail --lib`: 187 passed, 25 ignored
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

- Extend mutating `WHERE` with another restricted boolean-expression slice only
  if it can still lower through backend-neutral predicates without adding an
  arbitrary expression engine.
- Add another restricted writable `RETURN` helper from the remaining batches in
  `docs/CypherWrite.md`.

For each slice, update code, tests, `CHANGELOG.md`, `docs/CypherWrite.md`, book
manuscript, rebuilt book artifacts, then commit and push.
