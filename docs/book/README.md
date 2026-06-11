# Grust

This directory contains a short technical book about Grust and a reproducible
artifact pipeline.

Source:

- `cover.md` keeps the PDF/EPUB/MOBI cover page separate from the manuscript.
  Its version subtitle is rendered from `metadata.yaml` and the workspace
  `Cargo.toml`.
- `manuscript.md` keeps Mermaid diagrams inline as fenced `mermaid` blocks.
- `build.mjs` renders the cover template, renders Mermaid blocks to SVG, and
  produces `build/manuscript.rendered.md` for Pandoc.
- `build.sh` renders the cover and body separately for PDF, merges them, then
  builds EPUB and MOBI with the cover before the manuscript. It also creates a
  stable title-stem EPUB and an ignored versioned Send to Kindle symlink.
- `fix_epub_layout.sh` rewrites Pandoc's EPUB defaults so the custom cover is
  first in the reading spine and marked as frontmatter.
- `check_epub_metadata.py` verifies the generated EPUB package metadata, stable
  EPUB, versioned symlink, and dist marker before MOBI conversion.

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
  title sort key, creator, language, date, modified timestamp, navigation
  titles, and generated fallback labels such as `UNTITLED` or `Unknown`.
- It also checks that the stable title-stem EPUB is byte-identical to the
  canonical EPUB, the versioned Send to Kindle path is a symlink to it, and
  `VERSION.md` records the Kindle name, build date, stable EPUB, and symlink.
- The same check also enforces the Kindle-facing layout invariants used by the
  TypeSec checker: cover-then-visible-TOC spine order,
  frontmatter cover XHTML, no generated cover heading, and no flexbox on the
  cover.

Outputs:

- `build/dist/grust.pdf`
- `build/dist/grust.epub`
- `build/dist/grust.epub`
- `build/dist/grust (<version>).epub` ignored symlink to `grust.epub`
- `build/dist/grust.mobi`
- `build/dist/VERSION.md`
