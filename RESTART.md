# Restart Checkpoint

Generated: 2026-06-17 04:14:05 PDT

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
- Latest completed slice before this checkpoint: Batch DE, restricted string
  predicates in mutating `MATCH ... WHERE`.

## Latest Completed Work

Batch DE added restricted string predicates to bounded mutating Cypher
`MATCH ... WHERE`:

```cypher
MATCH (n:Person)
WHERE n.name STARTS WITH 'Ad' AND NOT n.name ENDS WITH 'bot'
SET n.reviewed = true;
```

Implemented behavior:

- `grust-core` exposes explicit string predicate operators for
  `STARTS WITH`, `ENDS WITH`, and `CONTAINS`, plus negated variants.
- `grust-sail` parses `variable.property STARTS WITH value`,
  `variable.property ENDS WITH value`, and `variable.property CONTAINS value`
  for node, relationship, and endpoint variables.
- String predicate needles must be string literals or string parameters.
- One leading `NOT` inverts the string predicate operator.
- Missing properties, nulls, and non-string values never match positive or
  negated string predicates.
- Sail SQL lowering and Memory execution both use the backend-neutral
  `GraphPropertyPredicate` path.

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

The following commands passed after Batch DE:

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

- `cargo test -p grust-sail --lib`: 184 passed, 25 ignored
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
