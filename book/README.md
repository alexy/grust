# grust-book

This directory contains a short technical book about Grust and a reproducible
artifact pipeline.

Source:

- `manuscript.md` keeps Mermaid diagrams inline as fenced `mermaid` blocks.
- `build.mjs` renders those blocks to SVG and produces
  `build/manuscript.rendered.md` for Pandoc.
- `build.sh` uses Pandoc and Typst to produce PDF, then Pandoc and Calibre to
  produce EPUB and MOBI.

Build:

```sh
./build.sh
```

Outputs:

- `build/dist/grust-book.pdf`
- `build/dist/grust-book.epub`
- `build/dist/grust-book.mobi`
