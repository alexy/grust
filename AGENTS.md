# Agent Notes

## Release Workflow

- **Every release is named and fully documented.** Release names come from `RELEASES.md` (the crustacean list), assigned in order; mark the chosen name as the current release there. Each release MUST be accompanied by fully updated documentation — this includes rebuilding the Grust book (`docs/book`) so it reflects the released surface, and writing a release blog post at `docs/blog/grust-<release-name>.md` (e.g. `docs/blog/grust-crab.md`). The blog post is crisp, links to the repo docs and the book for detail, and highlights the release's key innovations.
- After substantial changes that affect any publishable Grust crate, do not stop at committing or pushing the repository. Verify the workspace and publish the affected crates to crates.io as part of the same release workflow.
- When substantial crate changes add, remove, rename, or materially change public APIs, examples, dependency-facing behavior, or release-facing prose, update the Grust book and rebuild the book artifacts as part of the same work.
- Before publishing, run the appropriate tests and `cargo package --workspace --allow-dirty` to validate the crate tarballs.
- Maintain `CHANGELOG.md` for every release-facing change. Add a dated version entry before committing a release, and keep entries grouped by logical user-visible changes rather than raw commit lists.
- Publish workspace crates in dependency order. Publish `grust-core` first, then backend and adapter crates such as `grust-memory`, `grust-cocoindex`, `grust-falkor`, `grust-helix`, `grust-lancedb`, `grust-pggraph`, `grust-sail`, and `grust-surreal`, and publish the facade package `grust-graph` last.
- After publishing, verify the released versions from outside the workspace with `cargo info <crate>@<version>` so local path dependencies cannot mask registry state.
