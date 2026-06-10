# grust-book Build Notes

## Separate Cover Page

Use `docs/book/cover.md` as a standalone cover source and keep it separate from
`docs/book/manuscript.md`. The visible cover text for this book is:

- Title: `grust-book`
- Subtitle: `A Rust Property Graph Architecture`
- Author: `Alexy Khrabrov`
- Rights: `MIT OR Apache-2.0`

The cover file contains two raw blocks:

- A Typst block for the PDF cover.
- An HTML block for the EPUB and MOBI cover.

Keep the visible text synchronized between both blocks. The Typst cover sets
`numbering: none` so the standalone cover page does not show a page number.

## Rendered Manuscript

Run the Mermaid preprocessor before Pandoc:

```sh
cd docs/book
node build.mjs
```

This writes `docs/book/build/manuscript.rendered.md` and diagram images under
`docs/book/build/diagrams/`.

## PDF Build

Render the cover by itself. Do not pass `metadata.yaml` here, because Pandoc
will otherwise add a generated title page before the custom cover.

```sh
pandoc --from markdown+smart \
  --pdf-engine=typst \
  --output "$tmpdir/cover.pdf" \
  cover.md
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
sed '/^```{=typst}$/,/^```$/d' cover.md > "$tmpdir/cover.epub.md"
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
