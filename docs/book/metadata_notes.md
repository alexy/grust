# EPUB Metadata Notes

These notes come from debugging an EPUB that Amazon Send to Kindle rejected with
an internal error. The same checks should be useful for Grust and other book
projects that generate EPUBs with Pandoc or a similar pipeline.

## What Went Wrong

The generated EPUB was structurally readable, but the package metadata was too
thin. Its `EPUB/content.opf` had an identifier, date, and language, but it did
not have a real title or creator:

- no `dc:title`
- no `dc:creator`
- generated `UNTITLED` labels in the navigation files
- `Unknown` author in Calibre metadata output
- an empty generated `EPUB/text/title_page.xhtml` before the custom cover

Calibre could still inspect and convert the book, but it reported weak metadata
and threw an internal render error while probing generated frontmatter. Amazon's
error message was opaque, but the broken metadata profile was the clearest
portable failure mode.

## Metadata Every EPUB Build Should Carry

Keep stable metadata in a checked-in metadata file, for example:

```yaml
---
title: Typesec
subtitle: Type-Level Security for Agentic AI
author:
  - Alexy Khrabrov
lang: en-US
publisher: Chief Scientist
rights: Copyright Alexy Khrabrov
---
```

Then pass it to Pandoc:

```sh
pandoc cover.md manuscript.md \
  -o dist/book.epub \
  --toc \
  --number-sections \
  --metadata-file metadata.yaml \
  --metadata date="$(date -u +%F)" \
  --epub-title-page=false
```

The build date can stay dynamic, but title, subtitle, author, language,
publisher, and rights should live in source control.

## Why `--epub-title-page=false` Matters

When a project already provides a custom cover or title page, Pandoc can also
generate its own title page. In the failing EPUB, that produced an empty
`EPUB/text/title_page.xhtml` and pushed the real cover into a later chapter file.
Using `--epub-title-page=false` prevents that extra generated page and keeps the
custom cover as the first real content.

## Build Invariants

After creating the EPUB, inspect the generated package rather than trusting the
source Markdown. A good metadata gate should fail the build if:

- `EPUB/content.opf` is missing `dc:title`
- `EPUB/content.opf` is missing `dc:creator`
- `EPUB/content.opf` is missing `dc:language`
- `EPUB/content.opf` is missing `dc:date`
- `EPUB/content.opf` is missing `dcterms:modified`
- `EPUB/toc.ncx` does not have the real book title
- `EPUB/nav.xhtml` does not have the real book title
- any OPF, NCX, or nav file contains `UNTITLED` or `Unknown`
- `EPUB/text/title_page.xhtml` exists only as an empty generated title page

Run this check before converting the EPUB to MOBI, AZW3, or any Kindle-facing
format. That way downstream artifacts cannot be generated from a broken EPUB.

## Practical Verification Commands

Inspect the package metadata:

```sh
unzip -p dist/book.epub EPUB/content.opf
```

Inspect navigation titles:

```sh
unzip -p dist/book.epub EPUB/toc.ncx
unzip -p dist/book.epub EPUB/nav.xhtml
```

List generated files and look for unwanted title pages:

```sh
unzip -l dist/book.epub
```

Ask Calibre how it sees the book:

```sh
/Applications/calibre.app/Contents/MacOS/ebook-meta dist/book.epub
```

Smoke-test Kindle-style conversion:

```sh
/Applications/calibre.app/Contents/MacOS/ebook-convert dist/book.epub /tmp/book.azw3
```

Calibre accepting the EPUB does not prove Amazon will accept it, but it is a
useful local check. The stronger protection is a build-time metadata validator
that rejects weak or generated fallback metadata before any ebook conversion
happens.
