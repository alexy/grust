# grust-book Build Notes

## Separate Cover Page

Use `docs/book/cover.md` as a standalone cover source and keep it separate from
`docs/book/manuscript.md`. The build renders it to
`docs/book/build/cover.rendered.md`, filling metadata from `metadata.yaml` and
the version subtitle from `[workspace.package].version` in the workspace
`Cargo.toml`. The visible cover text for this book is:

- Title: `grust-book`
- Version subtitle: `covers grust 0.4.0`
- Subtitle: `A Rust Property Graph Architecture`
- Author: `Alexy Khrabrov`
- Coauthor credit: `&` / `Codex with ChatGPT 5.5`

The cover file contains two raw blocks:

- A Typst block for the PDF cover.
- An HTML block for the EPUB and MOBI cover.

Keep the template placeholders synchronized between both blocks. The Typst
cover sets `numbering: none` so the standalone cover page does not show a page
number.

## Rendered Manuscript

Run the Mermaid preprocessor before Pandoc:

```sh
cd docs/book
node build.mjs
```

This writes `docs/book/build/cover.rendered.md`,
`docs/book/build/manuscript.rendered.md`, and diagram images under
`docs/book/build/diagrams/`.

## PDF Build

Render the cover by itself. Do not pass `metadata.yaml` here, because Pandoc
will otherwise add a generated title page before the custom cover.

```sh
pandoc --from markdown+smart \
  --pdf-engine=typst \
  --output "$tmpdir/cover.pdf" \
  build/cover.rendered.md
```

Render the body separately, with the table of contents:

```sh
pandoc --from markdown+smart \
  --to typst \
  --metadata-file metadata.yaml \
  --toc --toc-depth=2 \
  --resource-path build \
  --output build/grust-book-body.typ \
  build/manuscript.rendered.md

typst compile build/grust-book-body.typ "$tmpdir/body.pdf"
```

Merge the cover before the body:

```sh
pdfunite "$tmpdir/cover.pdf" "$tmpdir/body.pdf" build/dist/grust-book.pdf
```

This ensures the PDF starts with the full standalone cover page, followed by
the Preface and numbered body.

## EPUB and MOBI Build

For EPUB, strip the Typst-only raw block into a temporary cover source. Without
this, Pandoc can still use the Typst block while constructing EPUB chapters and
create an unwanted wrapper heading.

```sh
sed '/^```{=typst}$/,/^```$/d' build/cover.rendered.md > "$tmpdir/cover.epub.md"
```

Pass the filtered cover before the rendered manuscript. Keep
`--epub-title-page=false`; otherwise Pandoc adds its own generated title page in
addition to the custom cover. The `epub.css` file styles `.cover-page` and hides
Pandoc's generated wrapper heading with `#grust-book > h1.unnumbered`.

```sh
pandoc --from markdown+smart \
  --metadata-file metadata.yaml \
  --epub-title-page=false \
  --toc --toc-depth=2 \
  --css epub.css \
  --resource-path build \
  --output build/dist/grust-book.epub \
  "$tmpdir/cover.epub.md" build/manuscript.rendered.md
```

Validate the generated EPUB package before creating Kindle-facing artifacts:

```sh
asdf exec python check_epub_metadata.py build/dist/grust-book.epub
```

The checker reads `EPUB/content.opf`, `EPUB/toc.ncx`, and `EPUB/nav.xhtml` from
the generated EPUB. It fails the build if required metadata is missing, if the
navigation files do not expose the real book title, if fallback labels such as
`UNTITLED` or `Unknown` appear, or if Pandoc produced an empty generated title
page.

Convert the EPUB to MOBI:

```sh
ebook-convert build/dist/grust-book.epub build/dist/grust-book.mobi
```

On this machine, Calibre's converter is available at:

```sh
/Applications/calibre.app/Contents/MacOS/ebook-convert
```

## One-Step Build

The checked-in build script performs the full flow:

```sh
cd docs/book
./build.sh
```

Outputs:

- `docs/book/build/dist/grust-book.pdf`
- `docs/book/build/dist/grust-book.epub`
- `docs/book/build/dist/grust-book.mobi`
