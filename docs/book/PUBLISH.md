# Publishing Notes

This book currently uses a generated workspace under `docs/book/build/`.
Final distributable artifacts live in `docs/book/build/dist/`:

- `grust-book.pdf`
- `grust-book.epub`
- `grust-book.mobi`

The sister TypeSec project currently uses `docs/book/dist/` directly for its
final artifacts and does not keep a persistent `build/` directory in the book
tree.

## Why Grust Uses `build/dist`

Grust has a richer book pipeline than TypeSec. The source manuscript keeps
Mermaid diagrams inline as fenced `mermaid` blocks. Before Pandoc runs,
`build.mjs` renders those diagrams and writes generated inputs under
`docs/book/build/`:

- `build/manuscript.rendered.md`
- `build/diagrams/*.mmd`
- `build/diagrams/*.png`

The shell build then uses those generated inputs:

- Pandoc reads `build/manuscript.rendered.md`.
- Pandoc uses `--resource-path build` so the rendered manuscript can refer to
  generated diagram images as `diagrams/diagram-XX.png`.
- Pandoc writes a Typst body file to `build/grust-book-body.typ`.
- The final PDF, EPUB, and MOBI are written to `build/dist/`.

This has a few advantages:

- All generated files live under one generated tree.
- Cleanup is simple: removing `docs/book/build/` resets the generated state.
- Authored source files stay separate from rendered Markdown, Typst output, and
  diagram assets.
- Git can ignore all intermediates while explicitly tracking only
  `build/dist/grust-book.{pdf,epub,mobi}`.
- The rendered diagram PNGs remain separately available for reuse in blog
  posts, release notes, or other publication workflows.

The main downside is naming clarity. `build/dist/` is reasonable internally,
but it is less obvious than a top-level `dist/` directory and differs from the
TypeSec book layout.

## Why TypeSec Uses `dist`

TypeSec has a simpler book pipeline. Its build script renders:

- `docs/book/cover.md`
- `docs/book/typesec.md`

Temporary cover and body PDFs are created in `mktemp` and removed after the
build. There is no generated manuscript, no rendered diagram directory, and no
persistent Typst output. Because the only durable outputs are the final
distributable files, `docs/book/dist/` is enough.

## Converting Grust Toward the TypeSec Layout

To make Grust publish final artifacts in `docs/book/dist/` while preserving its
generated intermediates under `docs/book/build/`:

1. Update `docs/book/build.sh` to create `dist/` instead of `build/dist/`.
2. Change the PDF output from `build/dist/grust-book.pdf` to
   `dist/grust-book.pdf`.
3. Change the EPUB output from `build/dist/grust-book.epub` to
   `dist/grust-book.epub`.
4. Change the MOBI conversion from `build/dist/grust-book.mobi` to
   `dist/grust-book.mobi`.
5. Update the final echo output in `build.sh`.
6. Update `README.md`, `README_book.md`, and this file to document `dist/`.
7. Update `.gitignore` to ignore `build/` intermediates while tracking
   `dist/grust-book.{pdf,epub,mobi}`.
8. Move the already-built final artifacts from `build/dist/` to `dist/`.

This would give Grust the same visible final-artifact layout as TypeSec while
still keeping the diagram/render workspace under `build/`.

## Converting TypeSec Toward the Grust Layout

To make TypeSec use a Grust-style `build/dist/` layout:

1. Update TypeSec's `docs/book/build.sh` to create `docs/book/build/dist`.
2. Change the PDF, EPUB, and MOBI outputs from `docs/book/dist/typesec.*` to
   `docs/book/build/dist/typesec.*`.
3. Add a `docs/book/.gitignore` that ignores `build/*` but unignores
   `build/dist/typesec.{pdf,epub,mobi}`.
4. Update TypeSec's `README_book.md` to document `build/dist`.
5. Move the tracked final artifacts from `docs/book/dist/` to
   `docs/book/build/dist/`.

That conversion would be mostly organizational today, because TypeSec does not
currently have persistent generated intermediates. It would become more useful
if TypeSec later adds generated diagrams, rendered manuscripts, preview images,
or other reusable book assets.

## Diagram PNG Availability

In the current Grust pipeline, rendered diagram PNGs are separately available
after a successful build at:

```text
docs/book/build/diagrams/
```

Those files are useful for blog workflows because they are already rendered from
the same Mermaid sources embedded in the book. The tradeoff is that they are
treated as generated intermediates and are ignored by git. If a blog post needs
stable checked-in image assets, copy the selected PNGs into a deliberate
publication asset directory, such as a blog-specific `images/` folder, instead
of relying on `build/diagrams/` as a permanent source.
