# Grust Book Build Notes

The canonical command is:

```sh
docs/book/build.sh
```

See `docs/book/PUBLISH.md` for the complete artifact, validation, and delivery
contract.

## Cover

The canonical 1024x1536 cover is `cover/grust-cover.png`. Its source blog
headboard, generated portrait art, First Pair Press publisher mask, generation
prompt, and deterministic composition command are documented in
`cover/README.md`.

Recompose the exact typography and publisher seal with:

```sh
uv run --no-project --with pillow python cover/make-cover.py
```

The composer owns the visible title `Grust`, subtitle
`A Rust Property Graph Architecture`, and sole author line `Alexy Khrabrov`.
`book.build.json` installs the same PNG as the first, unnumbered PDF page and as
the EPUB cover image. `docs/book/cover.md` references it for browser HTML.

## Rendered Manuscript

The prepare hook runs `node docs/book/build.mjs`. It writes the rendered
manuscript and seven Mermaid diagram source/PNG pairs under `docs/book/build/`.
Those generated inputs feed PDF, EPUB, MOBI, single-file HTML, and chapter HTML.

## Metadata and EPUB Layout

Stable metadata lives in `docs/book/metadata.yaml`. The visible title remains
`Grust`, while the OPF catalog title and delivery names are versioned. The
creator must be exactly `Alexy Khrabrov` and the publisher must be
`First Pair Press`.

`docs/book/fix_epub_layout.sh` orders the EPUB spine as image cover, visible
navigation/TOC, then preface. `docs/book/check_epub_metadata.py` validates that
order, the metadata, the 1024x1536 image wrapper, and byte identity between the
packaged cover and `cover/grust-cover.png` before MOBI generation.

## Output

Stable PDF, EPUB, MOBI, single-file HTML, chapter HTML, and `VERSION.md` outputs
live in `docs/book/build/dist/`. Versioned delivery paths are generated
symlinks to those stable artifacts. A successful canonical build finishes with
the shared PDF/EPUB/HTML and version-marker contracts passing.
