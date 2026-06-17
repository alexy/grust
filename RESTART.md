# Restart Checkpoint

Generated: 2026-06-17 04:22:21 PDT

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
- Latest completed slice before this checkpoint: Batch DF, restricted `IN`
  predicates in mutating `MATCH ... WHERE`.

## Latest Completed Work

Batch DF added restricted membership predicates to bounded mutating Cypher
`MATCH ... WHERE`:

```cypher
MATCH (n:Person)
WHERE n.team IN ['eng', 'data'] AND NOT n.status IN ['blocked']
SET n.reviewed = true;
```

Implemented behavior:

- `grust-core` exposes explicit `GraphPredicateOp::In` and
  `GraphPredicateOp::NotIn` operators.
- `grust-sail` parses `variable.property IN [literal_or_parameter, ...]` and
  `variable.property IN $parameter` for node, relationship, and endpoint
  variables.
- List-valued parameters accept `StringArray`, `IntArray`, `FloatArray`, or
  JSON arrays of scalar string, integer, float, or boolean values.
- List literal items are restricted to scalar string, integer, float, or
  boolean literals/parameters; nulls, maps, nested lists, arbitrary expressions,
  property references, and cross-variable expressions remain deferred.
- One leading `NOT` lowers membership to `NotIn`.
- Missing properties never match either positive or negated membership
  predicates.
- Sail SQL lowering reuses existing type-aware equality conditions, and Memory
  execution uses the backend-neutral `GraphPropertyPredicate` path.

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

The following commands passed after Batch DF:

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

- `cargo test -p grust-sail --lib`: 185 passed, 25 ignored
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
