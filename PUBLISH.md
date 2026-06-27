# Publishing a Grust Release

End-to-end runbook for shipping a Grust release. A release has three deliverables:

1. **Crates** published to crates.io.
2. **The book** rebuilt (`docs/book`).
3. **A release blog post** with a Ulysses **TextPack**.

The authoritative rules live in [`AGENTS.md`](AGENTS.md); this file is the
operational checklist. Detailed sub-procedures: the book pipeline is in
[`docs/book/PUBLISH.md`](docs/book/PUBLISH.md), and blog TextPacks in
[`TEXTPACK.md`](TEXTPACK.md).

> Never publish without an explicit request. Publishing to crates.io is
> irreversible (versions can be yanked, not deleted).

## 0. Name the release

Pick the next crustacean from [`RELEASES.md`](RELEASES.md) in order and mark it as
the current release there (e.g. `Crab — 0.11.0 (YYYY-MM-DD) ← current`).

## 1. Merge and version

- Merge feature branches into `main` (use `--no-ff` for an explicit integration
  point); delete the merged branches (local + remote).
- Bump the workspace version. The middle (minor) number is the default for an
  additive release; new public enum variants are technically breaking, so call
  that out in the notes even within `0.x`. The version appears in
  `[workspace.package]` **and** in every intra-crate path-dep `version = "…"`
  requirement — move them in lockstep:

  ```sh
  sed -i '' 's/0\.10\.0/0.11.0/g' Cargo.toml crates/*/Cargo.toml
  cargo update -w        # refresh the lockfile
  ```

## 2. CHANGELOG

Convert the top `## Unreleased` block into a dated, named entry and leave a fresh
`## Unreleased` placeholder above it:

```
## Unreleased

## 0.11.0 "Crab" - 2026-06-26
```

## 3. Documentation (required every release)

- **Book** — rebuild it so the version and content are current:

  ```sh
  cd docs/book && ./build.sh    # auto-stamps the workspace version
  ```

  The full pipeline (Typst/PDF/EPUB/MOBI, metadata gate) is in
  [`docs/book/PUBLISH.md`](docs/book/PUBLISH.md). Update `manuscript.md` for any
  new public surface before rebuilding.

  `build.sh` always maintains **version+git-hash links for both EPUB and PDF** in
  `docs/book/build/dist/` — `grust (<version>-<hash>).epub` and
  `… .pdf` — recorded as `epub_link` / `pdf_link` in `VERSION.md`.

- **iCloud delivery** — copy the versioned EPUB and PDF to `~/icloud/books`,
  deriving the names from `VERSION.md` (copying the links dereferences them to
  regular files):

  ```sh
  cd docs/book/build/dist
  cp "$(awk -F': ' '/^epub_link:/{print $2}' VERSION.md)" ~/icloud/books/
  cp "$(awk -F': ' '/^pdf_link:/{print $2}'  VERSION.md)" ~/icloud/books/
  ```

  `~/icloud/books` may not be listable (`Operation not permitted`) even when
  exact-path copies succeed — derive filenames from `VERSION.md` and verify with
  exact-path `cmp`, not `ls`.

- **Blog post** — at `docs/blog/grust-<name>/post.md`. Lead with the generic
  backend-neutral property-graph story (the Rust graph API and the multiple
  backends); highlight the release's key innovations; link to the repo docs and
  the book. Keep prose reflowed to one line per paragraph and diagrams referenced
  as `![caption](diagrams/<name>.png)`.

- **TextPack** — **always** build a `.textpack` for the blog post via
  [`TEXTPACK.md`](TEXTPACK.md). Keep it committed at
  `docs/blog/<name>/dist/<name>.textpack`, next to the post, and also hand it to
  the user.

## 4. Validate

```sh
cargo build --workspace --all-features   # expect 0 warnings
cargo test -p grust-core -p grust-cypher -p grust-memory -p grust-turso
cargo publish -p grust-core --dry-run    # validate packaging (no upload, no auth)
```

## 5. Publish crates (crates.io)

Publish in dependency order; `grust-helix` and `grust-ladybug` are
`publish = false` and are skipped. Each `cargo publish` runs a verify build and
waits for the registry index before returning, so a later crate can resolve the
one before it:

```
grust-core → grust-sql-core → grust-memory → grust-cocoindex → grust-falkor →
grust-lancedb → grust-surreal → grust-cypher → grust-postgres-core →
grust-postgres → grust-postgres-pgq → grust-pggraph → grust-sail →
grust-turso → grust-graph
```

```sh
for c in grust-core grust-sql-core grust-memory grust-cocoindex grust-falkor \
         grust-lancedb grust-surreal grust-cypher grust-postgres-core \
         grust-postgres grust-postgres-pgq grust-pggraph grust-sail \
         grust-turso grust-graph; do
  cargo publish -p "$c" || { echo "FAILED at $c"; break; }
done
```

If `cargo publish` is interrupted by a crates.io rate limit, wait and resume from
the crate that failed — already-published crates stay published.

## 6. Verify and tag

- Confirm the registry from **outside** the workspace (so path deps cannot mask
  it):

  ```sh
  cd /tmp && cargo info grust-graph@0.11.0
  ```

- Tag the commit the crates were built from and push it:

  ```sh
  git tag -a v0.11.0 -m 'Grust 0.11.0 "Crab"' && git push origin v0.11.0
  ```

## References

- [`AGENTS.md`](AGENTS.md) — authoritative release workflow rules.
- [`RELEASES.md`](RELEASES.md) — release names.
- [`docs/book/PUBLISH.md`](docs/book/PUBLISH.md) — book build/typesetting pipeline.
- [`TEXTPACK.md`](TEXTPACK.md) — blog-post TextPack preparation.
- [`CHANGELOG.md`](CHANGELOG.md) — release-facing change log.
