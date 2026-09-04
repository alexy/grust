# Publishing a Grust Release

This is the Grust-specific release checklist. [`AGENTS.md`](AGENTS.md) owns the
release rules, [`FIRSTPAIR.md`](FIRSTPAIR.md) owns the book handoff, and the
shared implementation and operational policy live in `~/src/firstpair`.
Do not copy or improvise FirstPair deployment steps here.

A complete release delivers:

1. all affected publishable crates on crates.io;
2. a rebuilt and verified Grust book;
3. a named release post and provenance-stamped TextPack;
4. a release tag that identifies the crate source.

Publishing crates, delivering the TextPack, and deploying the book are
outward-facing and require an explicit request. Crates.io versions cannot be
deleted.

## 1. Establish the release

- Start from the real repository root and read `AGENTS.md`, `FIRSTPAIR.md`, and
  this file.
- Reconcile every pending feature branch into `main`, using an explicit merge
  commit when integration history matters. Resolve all open release work before
  choosing the source commit.
- Pick the next unused crustacean in [`RELEASES.md`](RELEASES.md), add its
  version and date, and mark only that entry `← current`.
- Use a lockstep workspace version for a normal minor release. Update
  `[workspace.package]`, every publishable package version, and all
  intra-workspace dependency requirements. Internal `publish = false` packages
  may have their own package version, but their Grust dependency requirements
  must still resolve to the release line.
- Refresh `Cargo.lock` with `cargo update -w`.

For a historical scoped patch, record exactly which crates shipped and which
did not. Do not retroactively describe it as a lockstep workspace release.

## 2. Close the release documentation

- Leave a fresh empty `## Unreleased` section at the top of
  [`CHANGELOG.md`](CHANGELOG.md), followed by a dated, named version section.
  Group changes by user-visible behavior rather than commit history.
- Update README examples, feature lists, backend capability statements, and the
  authored book source at `docs/book/manuscript.md`.
- Write `docs/blog/grust-<name>/post.md`. Start with the backend-neutral Rust
  property-graph story and then describe the release's important changes and
  honest limitations. Link to repository documentation and the stable
  FirstPair book routes. Keep local images under a sibling `diagrams/`
  directory as committed `.mmd` plus rendered `.png` files.
- Reconcile completed goal documents and handoff notes so they cannot be
  mistaken for active work.

## 3. Verify source and packages

Run the checks appropriate to the changed surface, including the required
workspace package gate:

```sh
cargo fmt --all -- --check
cargo build --workspace --all-features
cargo test --workspace --all-features
cargo clippy --workspace --all-features --all-targets -- -D warnings
cargo package --workspace --allow-dirty
scripts/verify-package-attribution.sh
git diff --check
```

Run [`scripts/integration-test.sh`](scripts/integration-test.sh) for affected
live backends. A release that changes backend behavior should record the exact
profile, service/image revisions, and any intentionally unrun backend. An
unavailable service is not a passing test.

If a private workspace member cannot legally be packaged, fix its manifest or
workspace packaging configuration. Do not silently replace the authoritative
`cargo package --workspace --allow-dirty` gate with a smaller package set.

## 4. Create the pushed source handoff

Commit the release source, documentation, and any benchmark evidence, then push
the exact branch that will be released. The repository must be clean, attached
to an upstream, and exactly synchronized with its remote before either
FirstPair's book publisher or blog stamper may run.

Build the book from the repository root with the source-owned wrapper:

```sh
repo_root="$(git rev-parse --show-toplevel)"
"$HOME/src/firstpair/publishing/scripts/build-library-book.sh" \
  --repo-root "$repo_root"
```

Verify the generated package and `docs/book/build/dist/VERSION.md`, then commit
and push the tracked book artifacts. Do not manually copy book files to iCloud;
the centralized publisher owns versioned links, catalog staging, Blob upload,
iCloud delivery, deployment, and live verification.

## 5. Stamp the release post

Blog provenance also requires clean, pushed inputs. After the finished post and
every bundled image are committed and pushed, stamp the TextPack from the
owning repository:

```sh
REPO_ROOT="$PWD" \
BLOG_DOMAIN=querygraph.ai \
BLOG_TAGS=rust,graphs,grust \
BLOG_EXCERPT="Grust's latest backend-neutral property-graph release." \
"$HOME/src/firstpair/publishing/scripts/stamp-versioned-blog.sh" \
  "docs/blog/grust-<name>"
```

This creates the stable `.textpack`, its versioned link, and `dist/VERSION.md`
with `omnighost-textpack-v1` provenance. Verify them, then commit and push the
generated `dist/` handoff. The detailed and authoritative process is in
`~/src/firstpair/AGENTS.md`; [`TEXTPACK.md`](TEXTPACK.md) is only the local
source-layout reminder.

## 6. Publish crates in dependency order

Publish every affected publishable package in dependency order. For a lockstep
release of the current workspace, the order is:

```text
grust-core -> grust-sql-core -> grust-memory -> grust-cocoindex ->
grust-falkor -> grust-lancedb -> grust-surreal -> grust-cypher ->
grust-postgres-core -> grust-postgres -> grust-postgres-pgq -> grust-pggraph ->
grust-sail -> grust-turso -> grust-graph
```

`grust-helix`, `grust-ladybug`, `querygraph-memory`, and examples are currently
`publish = false`; test them, but do not send them to crates.io. Publish the
facade last. If crates.io temporarily rejects a later upload, resume at that
crate rather than republishing successful versions.

Afterward, verify every released crate from outside the workspace so local path
dependencies cannot mask registry state:

```sh
tmp_dir="$(mktemp -d)"
cd "$tmp_dir"
cargo info grust-core@<version>
cargo info grust-graph@<version>
# Repeat for every published package.
```

## 7. Tag and deliver

Tag the exact commit from which the crates were packaged and push the tag:

```sh
git tag -a "v<version>" -m 'Grust <version> "<Name>"'
git push origin "v<version>"
```

Inspect the non-writing FirstPair book plan from `~/src/firstpair` exactly as
specified by [`FIRSTPAIR.md`](FIRSTPAIR.md). Only then run the authorized live
book command. Deliver the already committed TextPack with FirstPair's
`publish-versioned-blog.sh`; it validates the marker and provenance before
copying the versioned archive.

Finish by checking that `main`, the release tag, crates.io packages, FirstPair
catalog routes, book artifacts, and the TextPack marker all identify the same
release source and version.
