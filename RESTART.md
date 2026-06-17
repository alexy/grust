# Restart Checkpoint

Generated: 2026-06-17 04:07:56 PDT

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
- Latest completed slice before this checkpoint: Batch DD, parenthesized
  mutating `MATCH ... WHERE` predicate terms and `AND` groups.

## Latest Completed Work

Batch DD added parentheses around otherwise-supported bounded mutating Cypher
`MATCH ... WHERE` predicate terms and `AND` groups:

```cypher
MATCH (n:Person)
WHERE (n.status = 'inactive' AND n.score >= 10) AND NOT (n.active = true)
SET n.archived = true;
```

Implemented behavior:

- `grust-sail` splits mutating `WHERE` clauses only on top-level `AND`.
- Enclosing parentheses around supported predicate terms are stripped before
  lowering.
- Enclosing parentheses around supported `AND` groups are recursively
  flattened into the same backend-neutral predicate vectors as the
  unparenthesized form.
- `NOT (supported predicate)` is accepted and still lowers through the existing
  operator-inversion path.
- Parentheses remain semantic-free: `OR`, nested `NOT`, function calls,
  pattern predicates, list predicates, cross-variable comparisons, and
  arbitrary expressions are still rejected when parenthesized.
- Planner and Memory-facade tests cover parenthesized node, edge, endpoint,
  and negated predicates.

Files updated:

- `crates/grust-sail/src/lib.rs`
- `crates/grust-sail/src/tests.rs`
- `docs/CypherWrite.md`
- `CHANGELOG.md`
- `docs/book/manuscript.md`
- rebuilt book artifacts under `docs/book/build/dist/`

## Verification State

The following commands passed after Batch DD:

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

- `cargo test -p grust-sail --lib`: 183 passed, 25 ignored
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
