# Agent Notes

## FirstPair Book Delivery

`FIRSTPAIR.md` is the required contract for this repository's unified book
build and FirstPair library deployment. Read and maintain it before changing or
delivering the book; it owns the catalog slug, shelf, and all source-side
handoff guidance. The shared implementation and authoritative operational rules
live in `~/src/firstpair`. Do not duplicate that deployment procedure here.

## Release Workflow

The step-by-step operational runbook is [`PUBLISH.md`](PUBLISH.md); the rules
below are authoritative.

- **Every release is named and fully documented.** Release names come from `RELEASES.md` (the crustacean list), assigned in order; mark the chosen name as the current release there. Each release MUST be accompanied by fully updated documentation — this includes rebuilding the Grust book (`docs/book`) so it reflects the released surface, and writing a release blog post at `docs/blog/grust-<release-name>/post.md` (e.g. `docs/blog/grust-crab/post.md`), with any diagrams under a sibling `diagrams/` directory. The post leads with the generic backend-neutral property-graph story (the Rust graph API and the multiple backends), links to the repo docs and the book for detail, and highlights the release's key innovations — it is not narrowed to any single backend or language feature.
- After substantial changes that affect any publishable Grust crate, do not stop at committing or pushing the repository. Verify the workspace and publish the affected crates to crates.io as part of the same release workflow.
- When substantial crate changes add, remove, rename, or materially change public APIs, examples, dependency-facing behavior, or release-facing prose, update the Grust book and rebuild the book artifacts as part of the same work.
- Before publishing, run the appropriate tests and `cargo package --workspace --allow-dirty` to validate the crate tarballs.
- Maintain `CHANGELOG.md` for every release-facing change. Add a dated version entry before committing a release, and keep entries grouped by logical user-visible changes rather than raw commit lists.
- Publish workspace crates in dependency order. Publish `grust-core` first, then backend and adapter crates such as `grust-memory`, `grust-cocoindex`, `grust-falkor`, `grust-helix`, `grust-lancedb`, `grust-pggraph`, `grust-sail`, and `grust-surreal`, and publish the facade package `grust-graph` last.
- After publishing, verify the released versions from outside the workspace with `cargo info <crate>@<version>` so local path dependencies cannot mask registry state.

## Benchmark Neutrality

- The benchmarks in this repository (`benchmarks/lsqb`) and the companion
  strain benchmark (`querygraph/adversarial-graph`) are neutral at every
  level: workload design, harness code, evidence, documentation, commit
  messages, and site prose. They measure and report; they do not pursue an
  outcome for any system, including Grust.
- Do not write, commit, or publish strategy about making one system look
  better or worse than another, in any file or commit message, at any time.
  State targets as engineering properties (exact answers, admitted queries,
  disclosed execution classes, resource envelopes, protocol parity), never as
  a contest against a named engine.
- Comparisons are evidence: same dataset, protocol, envelope, and execution
  class, with every outcome (pass, mismatch, unsupported, unavailable,
  timeout, error, not applicable) kept distinct and every failure retained.
  A faster or slower number is reported with its boundary, not framed as a
  win or a loss.
- If a document is found to violate this, fix the document and remove the
  violation from history; do not leave it in place with a disclaimer.

## File Discipline

- Prefer keeping source and documentation files under 500 lines, and try to keep
  them under 1000 lines when practical. Do not treat either number as a hard
  limit when splitting would harm the logic or make the code harder to follow.
  If an existing file is already over the practical limit, prefer adding new
  related code to a focused module/file instead of making the oversized file
  larger.
- When a change genuinely belongs in an oversized file, keep the edit tightly
  scoped and avoid opportunistic reformatting or refactors.
