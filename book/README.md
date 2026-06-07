# grust-book

This directory contains a short technical book about Grust and a reproducible
artifact pipeline.

Source:

- `cover.md` keeps the PDF/EPUB/MOBI cover page separate from the manuscript.
- `manuscript.md` keeps Mermaid diagrams inline as fenced `mermaid` blocks.
- `build.mjs` renders those blocks to SVG and produces
  `build/manuscript.rendered.md` for Pandoc.
- `build.sh` renders the cover and body separately for PDF, merges them, then
  builds EPUB and MOBI with the cover before the manuscript.

Build:

```sh
./build.sh
```

Outputs:

- `build/dist/grust-book.pdf`
- `build/dist/grust-book.epub`
- `build/dist/grust-book.mobi`
