# Grust Build Notes

## Separate Cover Page

Use `docs/book/cover.md` as a standalone cover source and keep it separate from
`docs/book/manuscript.md`. The build renders it to
`docs/book/build/cover.rendered.md`, filling metadata from `metadata.yaml` and
the version subtitle from `title_stem` plus `[workspace.package].version` in the
workspace `Cargo.toml`. The visible cover text for this book is:

- Title: `Grust`
- Version subtitle: `covers grust (0.7.0)`
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
  --output build/grust-body.typ \
  build/manuscript.rendered.md

typst compile build/grust-body.typ "$tmpdir/body.pdf"
```

Merge the cover before the body:

```sh
pdfunite "$tmpdir/cover.pdf" "$tmpdir/body.pdf" build/dist/grust.pdf
python fix_pdf_page_labels.py build/dist/grust.pdf
```

This ensures the PDF starts with the full standalone cover page, followed by
the Contents page, Preface, and numbered body. The cover page label is blank;
the Contents page starts PDF numbering at 1.

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
Pandoc's generated wrapper heading with `#grust > h1.unnumbered`.

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

Fix Pandoc's generated EPUB layout before validation. This makes the custom
cover the first spine item, keeps the nav as a visible TOC page, changes the cover XHTML
to frontmatter, removes Pandoc's wrapper heading around the cover, and rewrites
OPF `dc:title` and title sort metadata to the versioned Kindle library title:

```sh
./fix_epub_layout.sh build/dist/grust.epub "grust ($version)" "Grust"
```

Validate the generated EPUB package before creating Kindle-facing artifacts:

```sh
asdf exec python check_epub_metadata.py build/dist/grust.epub
```

The checker reads `EPUB/content.opf`, `EPUB/toc.ncx`, and `EPUB/nav.xhtml` from
the generated EPUB. It fails the build if required metadata is missing, if OPF
`dc:title` or title sort metadata does not match
`<title_stem> (<workspace version>)`, if the navigation files do not expose the
plain visible book title, if the versioned Send to Kindle path is missing or is
not a symlink to the stable title-stem EPUB, if the stable title-stem EPUB is not
byte-identical to the canonical EPUB, if `VERSION.md` does not include the
Kindle name, EPUB build date, stable EPUB, and versioned symlink, if fallback
labels such as `UNTITLED` or `Unknown`
appear, if the cover is not first in the spine, if the cover XHTML is not
frontmatter, if Pandoc left a generated cover heading, if the cover uses
flexbox, or if Pandoc produced an empty generated title page.

Convert the EPUB to MOBI:

```sh
ebook-convert build/dist/grust.epub build/dist/grust.mobi
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

- `docs/book/build/dist/grust.pdf`
- `docs/book/build/dist/grust.epub`
- `docs/book/build/dist/grust.epub`
- `docs/book/build/dist/grust (0.7.0).epub` ignored symlink to `grust.epub`
- `docs/book/build/dist/grust.mobi`
- `docs/book/build/dist/VERSION.md`
