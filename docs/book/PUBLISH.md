# Publishing Notes

This book currently uses a generated workspace under `docs/book/build/`.
Final distributable artifacts live in `docs/book/build/dist/`:

- `grust.pdf`
- `grust.epub`
- `grust (<version>).epub` ignored symlink to `grust.epub`
- `grust.mobi`
- `VERSION.md`

The sister TypeSec project currently uses `docs/book/dist/` directly for its
final artifacts and does not keep a persistent `build/` directory in the book
tree.

## Book Creation and Typesetting Skill

Use this section as the operational skill for rebuilding the Grust book. The
pipeline is not just "run Pandoc." It has separate source, generated input,
typesetting, EPUB repair, metadata validation, stable distribution naming, and
Kindle conversion stages.

### Source of Truth

Authored inputs live in `docs/book/`:

- `metadata.yaml`: title, title stem, subtitle, author, collaborator credit,
  language, rights, and TOC settings.
- `cover.md`: custom cover source with separate Typst and HTML raw blocks.
- `manuscript.md`: main manuscript with inline fenced `mermaid` diagrams.
- `epub.css`: EPUB and Kindle-facing CSS.
- `build.mjs`: renders cover placeholders and Mermaid diagrams.
- `build.sh`: runs the full artifact pipeline.
- `fix_epub_layout.sh`: repairs Pandoc's generated EPUB package layout.
- `check_epub_metadata.py`: validates the generated EPUB package and dist
  files.

Generated intermediates live under `docs/book/build/`. Final distributable
artifacts live under `docs/book/build/dist/`.

### Typesetting Model

The book is typeset through two related but separate surfaces:

- PDF uses the Typst raw block in `cover.md`, then Pandoc-to-Typst for the
  body, then `typst compile`.
- EPUB and MOBI use the HTML raw block in `cover.md`, Pandoc EPUB output,
  `epub.css`, EPUB package post-processing, and Calibre conversion.

Keep the visible cover text synchronized between the Typst and HTML blocks.
`build.mjs` fills shared placeholders from `metadata.yaml` and the workspace
version in `../../Cargo.toml`. The current rendered cover text is:

- Title: `Grust`
- Version subtitle: `covers grust (<workspace version>)`
- Subtitle: `A Rust Property Graph Architecture`
- Author: `Alexy Khrabrov`
- Collaborator credit: `&` / `Codex with ChatGPT 5.5`

For PDF cover spacing, tune the Typst block directly. `pdftotext` confirms
text, not visual spacing. When spacing matters, rasterize the first page:

```sh
pdftoppm -f 1 -singlefile -png -r 150 \
  docs/book/build/dist/grust.pdf /tmp/grust-cover
```

For EPUB and MOBI cover spacing, tune `epub.css` and the HTML block together.
Avoid `display: flex` on the cover because Kindle rendering is fragile there;
`check_epub_metadata.py` rejects it. Keep the CSS rule that hides Pandoc's
generated wrapper heading around the custom cover:

```css
#grust > h1.unnumbered {
  display: none;
}
```

Keep code blocks compact in EPUB and MOBI through `epub.css`. Pandoc's syntax
highlighting emits one `<span>` per source line and represents intentional blank
source lines as empty spans; reader defaults can turn those into large gaps.
The stylesheet overrides `div.sourceCode`, `pre`, `pre code`,
`pre > code.sourceCode > span`, and `pre > code.sourceCode > span:empty` so
code uses tight line-height and empty source-line spans do not render as extra
vertical whitespace.

### Required Tools

The full pipeline expects:

- `node`
- Mermaid CLI as `mmdc`
- `pandoc`
- `typst`
- `pdfunite`
- Python, preferably through `asdf exec python`
- Python with `pypdf` for PDF page-label repair. `build.sh` uses
  `PDF_PYTHON` when set, otherwise uses the bundled Codex runtime on this Mac.
- Calibre `ebook-convert`

The repository pins Python through the root `.tool-versions`. `build.sh` uses
`PANDOC_PYTHON` when set, otherwise prefers `asdf exec python`, and falls back
to `python3`.

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
- `build/dist/grust (<version>).epub` as an ignored symlink to `grust.epub`
- `build/dist/grust.mobi`
- `build/dist/VERSION.md`

### Full Pipeline

The full build performs these stages.

1. Render generated Markdown and diagram assets.

   ```sh
   node build.mjs
   ```

   This reads `[workspace.package].version` from `../../Cargo.toml`, reads
   cover metadata from `metadata.yaml`, writes `build/cover.rendered.md`,
   writes `build/manuscript.rendered.md`, and renders inline Mermaid fences to
   `build/diagrams/diagram-XX.mmd` plus `build/diagrams/diagram-XX.png`.

2. Render the standalone PDF cover.

   ```sh
   pandoc --from markdown+smart \
     --pdf-engine=typst \
     --output "$tmpdir/cover.pdf" \
     build/cover.rendered.md
   ```

   Do not pass `metadata.yaml` to the cover-only render. If metadata is passed
   here, Pandoc can add a generated title page before the custom cover.

3. Render the PDF body through Typst.

   ```sh
   pandoc --from markdown+smart \
     --to typst \
     --metadata-file metadata.yaml \
     --toc --toc-depth=2 \
     --resource-path build \
     --output build/grust-body.typ \
     build/manuscript.rendered.md

   {
     printf '#outline(title: [Contents])\n'
     printf '#pagebreak()\n\n'
     cat build/grust-body.typ
   } > build/grust-body-with-toc.typ

   typst compile build/grust-body-with-toc.typ "$tmpdir/body.pdf"
   ```

4. Merge cover and body.

   ```sh
   pdfunite "$tmpdir/cover.pdf" "$tmpdir/body.pdf" build/dist/grust.pdf
   python fix_pdf_page_labels.py build/dist/grust.pdf
   ```

   The merged PDF should start with the custom cover, followed by Contents,
   Preface, and the numbered body. The cover page label is blank; the Contents
   page starts PDF numbering at 1.

5. Prepare the EPUB cover source.

   ```sh
   sed '/^```{=typst}$/,/^```$/d' build/cover.rendered.md > "$tmpdir/cover.epub.md"
   ```

   This removes the Typst-only raw block before Pandoc builds EPUB chapters.

6. Build the EPUB.

   ```sh
   pandoc --from markdown+smart \
     --metadata-file metadata.yaml \
     --metadata date="$pubdate" \
     --epub-title-page=false \
     --toc --toc-depth=2 \
     --css epub.css \
     --resource-path build \
     --output build/dist/grust.epub \
     "$tmpdir/cover.epub.md" build/manuscript.rendered.md
   ```

   Keep `--epub-title-page=false`; Grust already has a custom cover and should
   not receive Pandoc's generated title page.

7. Repair Pandoc's EPUB layout.

   ```sh
   ./fix_epub_layout.sh build/dist/grust.epub "grust ($version)" "Grust"
   ```

   The fixer moves the custom cover before the nav item in the spine, keeps the
   nav item visible as the TOC page, marks the cover XHTML as frontmatter,
   removes the generated wrapper heading around the cover, and rewrites only
   `EPUB/content.opf` title/title-sort metadata to the versioned Kindle library
   title.

8. Create distribution names.

   `build.sh` writes the canonical EPUB directly to `build/dist/grust.epub`,
   removes old `build/dist/grust (*).epub` symlinks, creates
   `build/dist/grust (<version>).epub -> grust.epub`, and writes
   `build/dist/VERSION.md`.

9. Validate the generated EPUB package.

   ```sh
   asdf exec python check_epub_metadata.py build/dist/grust.epub
   ```

   The full build runs this after the EPUB is fixed and before MOBI conversion.

10. Convert EPUB to MOBI.

    ```sh
    ebook-convert build/dist/grust.epub build/dist/grust.mobi
    ```

### EPUB Invariants

Treat EPUB metadata and layout as build invariants. The checker fails the build
if any of these are wrong:

- `EPUB/content.opf` has the versioned Kindle title
  `<title_stem> (<workspace version>)`.
- title sort metadata matches that Kindle title.
- `dc:creator`, `dc:language`, `dc:date`, and `dcterms:modified` exist.
- NCX and nav expose the visible book title `Grust`.
- OPF, NCX, and nav files do not contain fallback labels such as `UNTITLED` or
  `Unknown`.
- the custom cover is first in the reading spine.
- the nav item remains visible after the cover and before the preface.
- the cover XHTML is marked frontmatter.
- the cover XHTML starts with the custom titlepage section.
- Pandoc did not leave a generated top-level cover heading.
- Pandoc did not leave an empty generated `title_page.xhtml`.
- the cover does not use flexbox.
- `grust (<version>).epub` exists as a symlink to `grust.epub`.
- `VERSION.md` records the Kindle name, EPUB build date, stable EPUB filename,
  and versioned symlink filename.

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
test "$(readlink 'build/dist/grust (0.6.7).epub')" = "grust.epub"
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

If the PDF has two title pages, check whether `metadata.yaml` was passed to the
cover-only PDF render.

If the EPUB has duplicate title pages or an extra `Grust` heading before
the cover, check all three layers:

- The Typst raw block must be stripped from the EPUB cover input.
- The EPUB Pandoc command must include `--epub-title-page=false`.
- `epub.css` must keep the wrapper-heading suppression rule.

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
  derives the Kindle library title as `<title_stem> (<version>)`, currently
  `grust (0.6.7)`.
- The EPUB build uses `--epub-title-page=false` because Grust has its own custom
  cover.
- `docs/book/fix_epub_layout.sh` applies the same post-Pandoc layout repair
  pattern TypeSec uses: cover first in the spine, nav visible as the TOC page, and the
  cover XHTML marked as frontmatter. It also rewrites only
  `EPUB/content.opf`'s `dc:title` and title sort metadata to the Kindle library
  title.
- `docs/book/check_epub_metadata.py` validates `EPUB/content.opf`,
  `EPUB/toc.ncx`, and `EPUB/nav.xhtml` inside the generated EPUB. It reads the
  workspace version with `tomllib`, reads `title_stem` from `metadata.yaml`, and
  expects OPF `dc:title` and title sort metadata to be
  `grust (<version>)` while NCX/nav/cover titles remain `Grust`.
- The same checker verifies that `build/dist/VERSION.md` records the
  Kindle/catalog name, EPUB build date, stable EPUB name, and versioned symlink,
  and that the version-suffixed Send to Kindle path is a symlink to
  `grust.epub`.
- The layout fix and check run immediately after `build/dist/grust.epub` is
  created and before `ebook-convert` creates `build/dist/grust.mobi`.
- The repository pins asdf Python `3.14.5` in `.tool-versions`, and the build
  prefers `asdf exec python` for the checker.

The checker fails the build if the EPUB is missing or changes the expected
versioned Kindle `dc:title`, title sort metadata, `dc:creator`, `dc:language`,
`dc:date`, or `dcterms:modified`; if the cover is not first in the reading
spine; if the nav
is not visible after the cover; if NCX or nav files do not expose the plain visible book
title; if the cover XHTML is not frontmatter; if the first cover section is not
the custom titlepage; if Pandoc left a generated cover heading; if the cover uses
flexbox; if package/navigation files contain fallback labels such as `UNTITLED`
or `Unknown`; or if Pandoc generated an empty `EPUB/text/title_page.xhtml`.

The equivalent TypeSec pipeline already follows the same sequence even though
its artifact directory remains `docs/book/dist/`: checked-in metadata file,
Pandoc `--metadata-file`, `--epub-title-page=false`, generated EPUB layout fix,
generated EPUB metadata/layout validation, then MOBI/Kindle conversion only
after validation passes.

The two projects do not need identical checker implementations. TypeSec's
`docs/book/check_epub_metadata.sh` is a shell script because its pipeline is
flatter and the script asserts exact TypeSec strings, reading-spine placement,
cover frontmatter, and Kindle-layout constraints after
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
