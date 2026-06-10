# Cover Build Notes

These notes capture the cover pattern used for the Typesec book so another book,
such as Grust, can use the same workflow.

## Files

- Keep the cover in a separate Markdown file, for example `docs/book/cover.md`.
- Put PDF-specific cover markup in a Typst raw block.
- Put EPUB/MOBI-specific cover markup in an HTML raw block.
- Keep the visible text synchronized between the Typst and HTML blocks.
- On case-insensitive filesystems, avoid placing files such as `COVER.md` next
  to `cover.md`; use a distinct name such as `cover_notes.md`.

## Version Subtitle

Use a placeholder in the cover source instead of hardcoding the version:

```text
covers typesec {{TYPESEC_VERSION}}
```

Then have the build script read the version from the project config and render a
temporary cover before calling Pandoc. For a Rust workspace, read
`[workspace.package].version` from `Cargo.toml`; for another project, use the
equivalent single source of truth.

## Typst Cover Spacing

Typst text boxes include font metrics that can make small gaps look larger than
expected. For tight title/subtitle spacing, use an absolute vertical adjustment
instead of only `em` spacing:

```typst
#text(size: 46pt, weight: "bold", bottom-edge: "bounds")[Grust]
#v(-24pt)
#text(size: 12pt)[covers grust {{GRUST_VERSION}}]
```

Useful tips:

- `bottom-edge: "bounds"` makes the title box hug the glyph bounds more closely.
- A negative `#v(...)` can be appropriate for a small subtitle directly under a
  large title.
- Inspect the rendered PDF visually; `pdftotext` confirms text, not spacing.
- Rasterize the first page with `pdftoppm` when tuning layout:

```sh
pdftoppm -f 1 -singlefile -png -r 150 docs/book/dist/book.pdf /tmp/book-cover
```

## HTML Cover Spacing

For EPUB and MOBI, mirror the Typst layout with inline HTML styles:

```html
<h1 style="font-size: 3.2em; margin: 0 0 0.05em;">Grust</h1>
<p style="font-size: 0.85em; margin: 0 0 1.1em;">covers grust {{GRUST_VERSION}}</p>
```

Use `&amp;` in HTML when the visible text should be `&`.

## Small Credit Lines

For a compact collaborator credit, make both lines smaller and italic, and keep
the second line close to the first:

```typst
#text(size: 11pt, style: "italic")[&]
#v(0.35em)
#text(size: 13pt, style: "italic")[Codex with ChatGPT 5.5]
```

```html
<p style="font-size: 0.85em; font-style: italic; margin: 0 0 0.35em;">&amp;</p>
<p style="font-size: 0.95em; font-style: italic; margin: 0;">Codex with ChatGPT 5.5</p>
```

## Build Shape

Build the cover separately for PDF, then merge it before the body:

```sh
pandoc "$tmpdir/cover.md" -o "$tmpdir/cover.pdf" --pdf-engine=typst
pandoc docs/book/manuscript.md -o "$tmpdir/body.pdf" --pdf-engine=typst --toc --number-sections
pdfunite "$tmpdir/cover.pdf" "$tmpdir/body.pdf" docs/book/dist/book.pdf
```

For EPUB, pass the rendered cover before the manuscript:

```sh
pandoc "$tmpdir/cover.md" docs/book/manuscript.md \
  -o docs/book/dist/book.epub \
  --toc \
  --number-sections
```

Then convert EPUB to MOBI with Calibre:

```sh
/Applications/calibre.app/Contents/MacOS/ebook-convert docs/book/dist/book.epub docs/book/dist/book.mobi
```

## Verification

After rebuilding, verify all generated artifacts:

```sh
pdftotext docs/book/dist/book.pdf - | sed -n '1,12p'
unzip -p docs/book/dist/book.epub '*.xhtml' | rg 'covers .*|font-style|titlepage'
```

Also inspect the rendered cover image when spacing matters:

```sh
pdftoppm -f 1 -singlefile -png -r 150 docs/book/dist/book.pdf /tmp/book-cover
open /tmp/book-cover.png
```

Expect occasional `PDFDoc::markDictionary` warnings from `pdfunite`; they are
not necessarily fatal if the command exits successfully and the generated PDF
opens and extracts text correctly.
