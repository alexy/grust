# Restart Checkpoint

Generated: 2026-06-18 PDT

## Current State

The long-running writable Cypher implementation goal from `docs/CypherWrite.md`
is complete in the working tree. The repository is currently on branch `main`
with broad, intentionally dirty Cypher, docs, and book-artifact changes.

Do not treat this file as a release marker. No staging, commit, package,
publish, or registry verification has been done for this checkpoint.

## Completed Cypher Slice

The parser, planner, DDL helpers, constraint registry, restricted returning
evaluator, and generic returning executor now live in `grust-cypher`.
`grust-sail` keeps Sail SQL lowering, Arrow IPC staging, SparkConnect execution,
and compatibility wrappers. The `grust-graph` facade exposes a `cypher` feature
for using the shared Cypher layer without enabling full Sail support.

`docs/CypherWrite.md` has implementation status for every batch. The latest
completed families include nested negated same-property `OR` groups that lower
to bounded `AND` predicate vectors across supported ordered, null, string,
equality, and membership combinations, plus the matched/deleted relationship
path return batches through DP.

## Verification State

The following checks passed after the goal completion audit:

```sh
cargo test -p grust-cypher --lib --quiet
cargo check -p grust-graph --features cypher,memory --quiet
docs/book/build.sh
git diff --check
```

Observed Cypher test summary:

- `cargo test -p grust-cypher --lib --quiet`: 327 passed

Book rebuild output confirmed current distribution artifacts under
`docs/book/build/dist/`, with `VERSION.md` recording `grust (0.9.0)` and
`built_at: 2026-06-18`.

## Known Open Areas

- Release prep remains undone: convert `CHANGELOG.md` `Unreleased` into a dated
  release entry, run the package workflow, publish in dependency order only
  when explicitly requested, and verify registry state from outside the
  workspace.
- Proposal and review documents have been marked as historical/backlog where
  appropriate, but some design notes still need a deeper code-against-doc
  reconciliation before being used as implementation guidance.
- Cypher’s portable execution path is strongest for `MemoryGraphStore` and Sail.
  Other backends can consume planned `GraphMutationPlan` values through their
  `GraphMutationStore` implementations, but they do not all expose direct text
  Cypher execution helpers or identical backend-native constraint enforcement.

## Next Safe Continuation

For release cleanup, start by reviewing `git status --short`, then run:

```sh
cargo test -p grust-cypher --lib --quiet
cargo check -p grust-graph --features cypher,memory --quiet
docs/book/build.sh
git diff --check
```

For more Cypher feature work, extend `docs/CypherWrite.md` with a new explicit
batch before editing code, keep tests in `crates/grust-cypher` when the behavior
is parser/planner/generic-executor logic, and leave Sail-specific SQL or live
SparkConnect tests in `crates/grust-sail`.
