# FirstPair Library Contract

slug: grust
shelf: querygraph
default_edition: full

This file is required for FirstPair library delivery. Its key-value header is
read by the centralized publisher; keep those lines simple and unbulleted.

## Ownership

This source repository owns the manuscript, `book.build.json`, version metadata,
source-specific hooks, and canonical build artifacts. The FirstPair repository
at `~/src/firstpair` owns the unified builder, publishing implementation,
public catalog, hosted readers, Blob uploads, iCloud delivery, and production
deployment.

The `slug` is the stable catalog identity and the directory name under
`~/src/firstpair/public/`. The `shelf` is the library grouping. Change either
only together with the corresponding entry in
`~/src/firstpair/public/catalog.json`.

## Unified Build

From the source repository root:

```sh
repo_root="$(git rev-parse --show-toplevel)"
"$HOME/src/firstpair/publishing/scripts/build-library-book.sh" \
  --repo-root "$repo_root"
```

The shared builder reads `book.build.json` and owns toolchain checks, standard
PDF/EPUB/HTML/chapter rendering, versioned artifact links, `VERSION.md`, and
package validation. Keep title-specific assembly, diagrams, EPUB repair,
special formats, and validators in source-owned hooks declared by that config.
A build never publishes or copies artifacts to the public library.

## FirstPair Deployment

Always inspect a non-writing publisher plan before any public action:

```sh
repo_root="$(git rev-parse --show-toplevel)"
cd "$HOME/src/firstpair"
npm run library:publish -- "$repo_root" \
  --dry-run --no-build --no-smoke --no-deploy --no-icloud
```

The publisher reads this file for the slug and shelf. Confirm the resolved
`distDir`, edition, artifacts, catalog directory, and iCloud names in the
printed plan. Do not pass a different `--slug` or `--shelf`; the publisher
rejects identity that conflicts with this contract.

Only run the live command from `~/src/firstpair` when the user has explicitly
asked to publish:

```sh
repo_root="$(git rev-parse --show-toplevel)"
cd "$HOME/src/firstpair"
npm run library:publish -- "$repo_root"
```

That command is outward-facing: it stages the package, updates the catalog,
uploads the book, copies versioned PDF/EPUB files to `~/icloud/books`, runs
catalog/build/smoke checks, deploys production, and verifies the live catalog.
Follow `~/src/firstpair/AGENTS.md` and
`~/src/firstpair/publishing/PUBLISH.md`; do not reproduce or bypass the
central deployment machinery in this repository.

## Maintenance

When build behavior changes, update `book.build.json` and source-owned hooks,
then run the unified build and its validators. When library identity or policy
changes, update this file and the central catalog in the same logical change.
