# Restart Checkpoint

Generated: 2026-06-16 18:48:57 PDT

## Current Goal

Continue implementing the new parts of `docs/CypherWrite.md` on branch
`codex/cypher-write`, without publishing crates until explicitly instructed.

## Branch And Worktree

- Branch: `codex/cypher-write`
- Remote status at checkpoint: `ahead 6` relative to
  `origin/codex/cypher-write`
- Worktree is intentionally dirty with accumulated writable Cypher work.
- Untracked file present: `OPUS1.md`
- No commit, push, tag, or crate publish was performed during the latest
  continuation work.

Files shown modified at checkpoint:

- `CHANGELOG.md`
- `Cargo.lock`
- `crates/grust-core/Cargo.toml`
- `crates/grust-core/src/lib.rs`
- `crates/grust-core/src/tests.rs`
- `crates/grust-memory/src/lib.rs`
- `crates/grust-memory/src/tests.rs`
- `crates/grust-sail/Cargo.toml`
- `crates/grust-sail/src/lib.rs`
- `crates/grust-sail/src/tests.rs`
- `crates/grust/src/lib.rs`
- `docs/CypherWrite.md`
- `docs/book/build/dist/VERSION.md`
- `docs/book/build/dist/grust.epub`
- `docs/book/build/dist/grust.mobi`
- `docs/book/build/dist/grust.pdf`
- `docs/book/manuscript.md`
- `docs/sail-backend-proposal.md`

## Latest Completed Work

The latest continuation batches extended restricted writable Cypher `RETURN`
over the existing materialized write-result table. Each slice has code,
focused Memory-facade coverage, changelog entry, `docs/CypherWrite.md` entry,
book manuscript update, and rebuilt book artifacts.

Completed recent batches:

- Batch CF: `left(variable.property, length)` and
  `right(variable.property, length)`
- Batch CG: `reverse(variable.property)`
- Batch CH: `split(variable.property, delimiter)`
- Batch CI: `isEmpty(variable.property)`
- Batch CJ: `toString(variable.property)`
- Batch CK: `abs(variable.property)`

Latest implemented code markers:

- `crates/grust-sail/src/lib.rs`
  - `CypherReturnTarget::PropertyAbs`
  - `parse_return_abs_projection`
  - `restricted_abs_value`
- `crates/grust-sail/src/tests.rs`
  - `sail_cypher_returning_projects_restricted_abs_on_memory_facade`
- `docs/CypherWrite.md`
  - `### Batch CK: Restricted Numeric Absolute-Value Projections`
- `CHANGELOG.md`
  - Unreleased entry for `abs(variable.property)`
- `docs/book/manuscript.md`
  - supported restricted-function list includes `abs(n.score)`

## Verification State

The following commands passed after Batch CK:

```sh
cargo fmt --all
cargo test -p grust-sail --lib
cargo test -p grust-core --lib
cargo test -p grust-memory --lib
cargo check -p grust-graph --features sail,memory
git diff --check
docs/book/build.sh
```

Observed test summaries:

- `cargo test -p grust-sail --lib`: 167 passed, 25 ignored
- `cargo test -p grust-core --lib`: 35 passed
- `cargo test -p grust-memory --lib`: 20 passed

Book rebuild output confirmed:

- `docs/book/build/dist/grust.pdf`
- `docs/book/build/dist/grust.epub`
- `docs/book/build/dist/grust (0.9.0).epub -> grust.epub`
- `docs/book/build/dist/grust.mobi`
- `docs/book/build/dist/VERSION.md`

## Next Safe Continuation

Continue only if asked. The next slice should stay small and should preserve
the current invariant:

- Cypher remains a syntax layer.
- Writable `RETURN` helpers operate only over already-bound/materialized write
  result rows.
- No arbitrary expression engine, cross-variable expressions, path-pattern
  predicates, or list/map expression evaluation should be introduced.
- Add a dedicated target, parser helper, evaluator, aggregate/count wiring,
  Memory-facade tests, changelog entry, `docs/CypherWrite.md` batch, book
  manuscript update, and book rebuild for each new slice.

Potential next bounded numeric/property slices:

- `ceil(variable.property)` / `floor(variable.property)` over numeric values.
- `round(variable.property)` over numeric values, but only after deciding exact
  integer/float return semantics and tie behavior.
- `sign(variable.property)` over numeric values, returning `-1`, `0`, or `1`.

Recommended next batch:

- Batch CL: restricted `ceil(variable.property)` and
  `floor(variable.property)` projections and aggregate bodies over numeric
  property values.
- Keep missing/null as `null`.
- Reject strings, booleans, arrays, maps, nested expressions, paths, and
  non-finite numeric results.
- Verify with the same command set listed above.

## Important Constraints

- Do not publish crates until explicitly instructed.
- Do not tag, commit, or push unless explicitly asked.
- Do not revert unrelated dirty work.
- Treat `OPUS1.md` as an existing untracked review artifact unless the user
  asks otherwise.
