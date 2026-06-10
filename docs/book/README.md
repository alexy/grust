# grust-book

This directory contains a short technical book about Grust and a reproducible
artifact pipeline.

Source:

- `cover.md` keeps the PDF/EPUB/MOBI cover page separate from the manuscript.
  Its version subtitle is rendered from the workspace `Cargo.toml`.
- `manuscript.md` keeps Mermaid diagrams inline as fenced `mermaid` blocks.
- `build.mjs` renders the cover template, renders Mermaid blocks to SVG, and
  produces `build/manuscript.rendered.md` for Pandoc.
- `build.sh` renders the cover and body separately for PDF, merges them, then
  builds EPUB and MOBI with the cover before the manuscript.
- `check_epub_metadata.py` verifies the generated EPUB package metadata before
  MOBI conversion.

Build:

```sh
./build.sh
```

Checks:

- The repository is pinned to Python `3.14.5` through the root
  `.tool-versions` file.
- `build.sh` runs `check_epub_metadata.py` after EPUB generation and before
  MOBI conversion.
- The metadata check inspects the generated EPUB package for real title,
  creator, language, date, modified timestamp, navigation titles, and generated
  fallback labels such as `UNTITLED` or `Unknown`.

Outputs:

- `build/dist/grust-book.pdf`
- `build/dist/grust-book.epub`
- `build/dist/grust-book.mobi`
