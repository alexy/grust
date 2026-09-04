# Publishing Notes

This book currently uses a generated workspace under `docs/book/build/`.
Final distributable artifacts live in `docs/book/build/dist/`:

- `grust.pdf`
- `grust.epub`
- `grust (<version>-<commit>).{pdf,epub,html}` ignored symlinks to stable files
- `grust.mobi`
- `grust.html`
- `grust-chapters/`
- `VERSION.md`

The sister TypeSec project currently uses `docs/book/dist/` directly for its
final artifacts and does not keep a persistent `build/` directory in the book
tree.

## Blog Posts

Every blog post (`docs/blog/grust-<release>/post.md`) is **always** delivered as a
Ulysses **TextPack** as part of publishing it. Build the `.textpack` with the
procedure in [`TEXTPACK.md`](../../TEXTPACK.md): reflow the prose to one line per
paragraph, render the post's `mermaid` diagrams to PNG, and bundle the text plus
images into a single self-contained package that imports cleanly into Ulysses
(including iOS). Keep the built `.textpack` committed at
`docs/blog/grust-<release>/dist/grust-<release>.textpack`, next to the post.

## Book Creation and Typesetting Skill

Use this section as the operational skill for rebuilding the Grust book. The
pipeline is not just "run Pandoc." It has separate source, generated input,
typesetting, EPUB repair, metadata validation, stable distribution naming, and
Kindle conversion stages.

### Source of Truth

Authored inputs live in `docs/book/`:

- `metadata.yaml`: title, title stem, subtitle, sole author, publisher,
  language, rights, and TOC settings.
- `../../cover/grust-cover.png`: canonical 1024x1536 raster cover.
- `../../cover/README.md`: headboard provenance, image-generation prompt, and
  deterministic composition command.
- `cover.md`: browser-reader wrapper for the canonical cover image.
- `manuscript.md`: main manuscript with inline fenced `mermaid` diagrams.
- `epub.css`: EPUB and Kindle-facing CSS.
- `build.mjs`: renders cover placeholders and Mermaid diagrams.
- `build.sh`: runs the full artifact pipeline.
- `../../book.build.json`: shared build configuration and source-owned hooks.
- `fix_epub_layout.sh`: repairs Pandoc's generated EPUB package layout.
- `check_epub_metadata.py`: validates the generated EPUB package and dist
  files.

Generated intermediates live under `docs/book/build/`. Final distributable
artifacts live under `docs/book/build/dist/`.

### Typesetting Model

The book is typeset through two related but separate surfaces:

- PDF prepends a full-page image built from `../../cover/grust-cover.png` to
  the Pandoc-to-Typst body.
- EPUB and MOBI use that same PNG as the package cover image, then apply EPUB
  package post-processing and Calibre conversion.
- Browser HTML uses the image wrapper in `cover.md`; chapter HTML packages a
  byte-identical copy of the referenced cover in its local assets directory.

The current visible cover text is:

- Title: `Grust`
- Subtitle: `A Rust Property Graph Architecture`
- Author: `Alexy Khrabrov`
- Publisher mark: `First Pair Press`

Keep lettering out of the generated portrait art. Exact typography and seal
placement live in `../../cover/make-cover.py`; reproduce the cover with:

```sh
uv run --no-project --with pillow python cover/make-cover.py
```

`pdftotext` does not see text baked into the raster cover. Rasterize the first
PDF page when checking the result visually:

```sh
pdftoppm -f 1 -singlefile -png -r 150 \
  docs/book/build/dist/grust.pdf /tmp/grust-cover
```

The validator checks the EPUB image-cover metadata, 1024x1536 wrapper,
creator/publisher values, reading order, and byte identity with
`../../cover/grust-cover.png`.

Keep code blocks compact in EPUB and MOBI through `epub.css`. Pandoc's syntax
highlighting emits one `<span>` per source line and represents intentional blank
source lines as empty spans; reader defaults can turn those into large gaps.
The stylesheet overrides `div.sourceCode`, `pre`, `pre code`,
`pre > code.sourceCode > span`, and `pre > code.sourceCode > span:empty` so
code uses tight line-height and empty source-line spans do not render as extra
vertical whitespace.

### Required Tools

`build.sh` is a thin wrapper over
`~/src/firstpair/publishing/scripts/build-library-book.sh`. FirstPair pins the
Homebrew and npm rendering tools. The checked-in `book.build.json` retains
Grust's `build.mjs`, EPUB repair, PDF page-label repair, and metadata validator;
the `docs/book/python` uv project supplies `pypdf` through the repository's
asdf-pinned Python 3.14.5.

The full pipeline expects:

- `node`
- Mermaid CLI as `mmdc`
- `pandoc`
- `typst`
- `pdfunite`
- asdf Python 3.14.5 with the uv-locked `docs/book/python` project
- Calibre `ebook-convert`

The repository pins Python through the root `.tool-versions`; FirstPair's
`ensure-python-env.sh` runs `uv sync` and invokes the resulting project Python.

On this Mac, Calibre's converter is available at:

```sh
/Applications/calibre.app/Contents/MacOS/ebook-convert
```

If `ebook-convert` is not on `PATH`, run the build with:

```sh
EBOOK_CONVERT=/Applications/calibre.app/Contents/MacOS/ebook-convert ./build.sh
```

### One-Step Build

Run the checked-in build script:

```sh
cd docs/book
./build.sh
```

The script should produce:

- `build/dist/grust.pdf`
- `build/dist/grust.epub`
- versioned PDF, EPUB, HTML, and chapter-package symlinks using
  `<version>-<short-commit>`
- `build/dist/grust.mobi`
- `build/dist/grust.html`
- `build/dist/grust-chapters/`
- `build/dist/VERSION.md`

### Shared Pipeline

The shared builder performs these logical stages; `build.sh` remains a thin
wrapper around the FirstPair implementation.

1. Run `node build.mjs` to render the manuscript and its seven Mermaid diagram
   assets under `build/`. It also renders the browser-cover wrapper consumed by
   the HTML path.
2. Build the Typst body PDF with Contents and numbered sections. Separately,
   build a US-Letter raster-cover page from `../../cover/grust-cover.png`, merge
   it before the body, and repair the PDF page labels.
3. Build `build/dist/grust.epub` with
   `../../cover/grust-cover.png` passed as Pandoc's EPUB cover image and with
   `--epub-title-page=false`.
4. Run `fix_epub_layout.sh` so the spine begins with Pandoc's image-cover XHTML,
   then the visible nav/TOC marked `linear="no"`, then the preface. Rewrite
   only the OPF title/title-sort fields to the versioned Kindle catalog title.
5. Write the full `VERSION.md` manifest and stable/versioned PDF, EPUB, HTML,
   and chapter-package paths.
6. Generate single-file and chapter HTML. The chapter packager resolves the
   reader cover through the configured resource path, copies it into `assets/`,
   and rewrites the chapter reference to that local byte-identical file.
7. Run `check_epub_metadata.py`, convert the validated EPUB to MOBI, and run the
   shared rendered-PDF, HTML-resource, artifact, and manifest contracts.

### EPUB Invariants

Treat EPUB metadata and layout as build invariants. The checker fails the build
if any of these are wrong:

- `EPUB/content.opf` has the versioned Kindle title
  `<title_stem> (<workspace version>)`.
- title sort metadata matches that Kindle title.
- `dc:creator` is exactly `Alexy Khrabrov`; `dc:publisher` is exactly
  `First Pair Press`; language, date, and modified metadata exist.
- NCX and nav expose the visible book title `Grust`.
- OPF, NCX, and nav files do not contain fallback labels such as `UNTITLED` or
  `Unknown`.
- the image cover is first in the reading spine.
- the nav item remains visible after the cover and before the preface.
- the cover XHTML is Pandoc's image wrapper with a 1024x1536 view box.
- the packaged cover image is byte-identical to
  `../../cover/grust-cover.png`.
- Pandoc did not leave an empty generated `title_page.xhtml`.
- both `grust (<version>).epub` (the Kindle catalog link) and
  `grust (<version>-<short-commit>).epub` (the provenance-stamped handoff link)
  exist as symlinks to `grust.epub`.
- `VERSION.md` records the Kindle name, EPUB build date, source commit, stable
  EPUB filename, Kindle link, and provenance-stamped EPUB link.

Do not bypass the checker for a release artifact. Fix the source, metadata,
CSS, layout fixer, or build script instead.

### Verification

After rebuilding, inspect the generated artifacts:

```sh
pdftotext build/dist/grust.pdf - | head -80
python - <<'PY'
from pypdf import PdfReader
print(PdfReader("build/dist/grust.pdf").page_labels[:6])
PY
unzip -p build/dist/grust.epub EPUB/content.opf | head -80
unzip -p build/dist/grust.epub EPUB/nav.xhtml | head -80
version="$(awk -F\" '/^version = / { print $2; exit }' ../../Cargo.toml)"
version_stamp="$(awk -F': ' '$1 == "version_stamp" { print $2; exit }' build/dist/VERSION.md)"
test "$(readlink "build/dist/grust (${version}).epub")" = "grust.epub"
test "$(readlink "build/dist/grust (${version_stamp}).epub")" = "grust.epub"
cat build/dist/VERSION.md
```

For source updates, search the generated EPUB package for the headings,
methods, or examples that changed. EPUB extraction can split exact Rust tokens,
so section headings and method names are often more stable than full snippets.

Useful stable headings for cross-references include:

- `The Shape of Grust`
- `The Core Property Graph`
- `Building Graphs`
- `Loading and Saving Graph Documents`
- `Traversal as an Intermediate Representation`
- `The Store Contract`
- `Backend Architecture`
- `Schema and Validation Direction`
- `Design Tradeoffs`
- `Where Grust Can Grow`

### Common Failures

If Mermaid rendering fails with a Chromium sandbox launch error such as
`FATAL:content/browser/sandbox_parameters_mac.mm:67`, treat diagram PNGs as
incomplete until `mmdc` succeeds and the diagrams are visibly verified.

If the PDF has two title pages, confirm `pdf.coverImage` is the only cover-page
mechanism and that the body render is not generating its own title page.

If the EPUB has duplicate title pages, confirm `epub.coverImage` points to the
canonical PNG, `epub.includeRenderedCover` is `false`, and Pandoc still receives
`--epub-title-page=false`.

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
- Pandoc writes a Typst body file to `build/grust-body.typ`.
- `build.sh` prepends a Typst `#outline(title: [Contents])` page and page break
  to create `build/grust-body-with-toc.typ`.
- The final PDF, EPUB, and MOBI are written to `build/dist/`.

This has a few advantages:

- All generated files live under one generated tree.
- Cleanup is simple: removing `docs/book/build/` resets the generated state.
- Authored source files stay separate from rendered Markdown, Typst output, and
  diagram assets.
- Git can ignore all intermediates while explicitly tracking only
  `build/dist/grust.{pdf,epub,mobi}`.
- The rendered diagram PNGs remain separately available for reuse in blog
  posts, release notes, or other publication workflows.

The main downside is naming clarity. `build/dist/` is reasonable internally,
but it is less obvious than a top-level `dist/` directory and differs from the
TypeSec book layout.

## EPUB Metadata Gate

Both Grust and TypeSec should treat EPUB metadata as a build invariant, not as a
post-hoc Kindle debugging step. The stable metadata should live in a checked-in
`metadata.yaml`, and the generated EPUB package should be inspected before any
MOBI, AZW3, Send to Kindle, or other Kindle-facing artifact is produced.

Grust already implements this shape:

- `docs/book/metadata.yaml` already carried the stable book metadata before the
  checker was added.
- `docs/book/build.sh` passes `--metadata-file metadata.yaml` to Pandoc.
- `docs/book/metadata.yaml` carries `title_stem: "grust"` for catalog and upload
  surfaces.
- `docs/book/build.sh` reads `[workspace.package].version` from `Cargo.toml` and
  derives the Kindle library title as `<title_stem> (<version>)`, such as
  `grust (0.13.0)` for the current workspace version.
- The EPUB build uses `--epub-title-page=false` and the canonical PNG as its
  Pandoc cover image.
- `docs/book/fix_epub_layout.sh` applies the same post-Pandoc layout repair
  pattern TypeSec uses: image cover first in the spine, nav visible as the TOC
  page with `linear="no"`, then the preface. It also rewrites only
  `EPUB/content.opf`'s `dc:title` and title sort metadata to the Kindle library
  title.
- `docs/book/check_epub_metadata.py` validates `EPUB/content.opf`,
  `EPUB/toc.ncx`, and `EPUB/nav.xhtml` inside the generated EPUB. It reads the
  workspace version with `tomllib`, reads `title_stem` from `metadata.yaml`, and
  expects OPF `dc:title` and title sort metadata to be
  `grust (<version>)` while NCX/nav titles remain `Grust`.
- The same checker verifies that `build/dist/VERSION.md` records the
  Kindle/catalog name, EPUB build date, source commit, stable EPUB name, and
  versioned symlink, and that the version-and-source-suffixed Send to Kindle path is a symlink to
  `grust.epub`.
- The layout fix and check run immediately after `build/dist/grust.epub` is
  created and before `ebook-convert` creates `build/dist/grust.mobi`.
- The repository pins asdf Python `3.14.5` in `.tool-versions`, and the build
  prefers `asdf exec python` for the checker.

The checker fails the build if the EPUB is missing or changes the expected
versioned Kindle `dc:title`, title sort metadata, exact creator/publisher,
`dc:language`, `dc:date`, or `dcterms:modified`; if the cover is not first in the reading
spine; if the nav
is not visible after the cover; if NCX or nav files do not expose the plain visible book
title; if the cover XHTML is not Pandoc's 1024x1536 image wrapper; if the
packaged cover differs from `cover/grust-cover.png`; if package/navigation files
contain fallback labels such as `UNTITLED` or `Unknown`; or if Pandoc generated
an empty `EPUB/text/title_page.xhtml`.

The equivalent TypeSec pipeline already follows the same sequence even though
its artifact directory remains `docs/book/dist/`: checked-in metadata file,
Pandoc `--metadata-file`, `--epub-title-page=false`, generated EPUB layout fix,
generated EPUB metadata/layout validation, then MOBI/Kindle conversion only
after validation passes.

The two projects do not need identical checker implementations. TypeSec's
`docs/book/check_epub_metadata.sh` is a shell script because its pipeline is
flatter and the script asserts exact TypeSec strings, reading-spine placement,
image-cover geometry and bytes, and Kindle-layout constraints after
`docs/book/fix_epub_layout.sh` runs. Grust uses
`docs/book/check_epub_metadata.py` because Grust already pins a reliable Python
with asdf and the checker benefits from treating the EPUB as a ZIP package:
read the OPF/NCX/nav/cover members directly, inspect required metadata and
layout fields, detect fallback metadata, and report all failures in one pass
without shell temporary file plumbing. It uses only the Python standard library.

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
2. Change the PDF output from `build/dist/grust.pdf` to
   `dist/grust.pdf`.
3. Change the EPUB output from `build/dist/grust.epub` to
   `dist/grust.epub`.
4. Change the MOBI conversion from `build/dist/grust.mobi` to
   `dist/grust.mobi`.
5. Update the final echo output in `build.sh`.
6. Update `README.md`, `README_book.md`, and this file to document `dist/`.
7. Update `.gitignore` to ignore `build/` intermediates while tracking
   `dist/grust.{pdf,epub,mobi}`.
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
