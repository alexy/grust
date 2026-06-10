#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

mkdir -p build/dist

node build.mjs

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
sed '/^```{=typst}$/,/^```$/d' build/cover.rendered.md > "$tmpdir/cover.epub.md"

pandoc --from markdown+smart \
  --pdf-engine=typst \
  --output "$tmpdir/cover.pdf" \
  build/cover.rendered.md

pandoc --from markdown+smart \
  --to typst \
  --metadata-file metadata.yaml \
  --toc --toc-depth=2 \
  --resource-path build \
  --output build/grust-book-body.typ \
  build/manuscript.rendered.md

typst compile build/grust-book-body.typ "$tmpdir/body.pdf"
pdfunite "$tmpdir/cover.pdf" "$tmpdir/body.pdf" build/dist/grust-book.pdf

PYTHON_CMD=()
if [[ -n "${PANDOC_PYTHON:-}" ]]; then
  PYTHON_CMD=("$PANDOC_PYTHON")
else
  if command -v asdf >/dev/null 2>&1; then
    PYTHON_CMD=(asdf exec python)
  else
    PYTHON_CMD=(python3)
  fi
fi

pandoc --from markdown+smart \
  --metadata-file metadata.yaml \
  --epub-title-page=false \
  --toc --toc-depth=2 \
  --css epub.css \
  --resource-path build \
  --output build/dist/grust-book.epub \
  "$tmpdir/cover.epub.md" build/manuscript.rendered.md

"${PYTHON_CMD[@]}" check_epub_metadata.py build/dist/grust-book.epub

EBOOK_CONVERT="${EBOOK_CONVERT:-}"
if [[ -z "$EBOOK_CONVERT" ]]; then
  if command -v ebook-convert >/dev/null 2>&1; then
    EBOOK_CONVERT="$(command -v ebook-convert)"
  elif [[ -x /Applications/calibre.app/Contents/MacOS/ebook-convert ]]; then
    EBOOK_CONVERT=/Applications/calibre.app/Contents/MacOS/ebook-convert
  else
    echo "ebook-convert not found; cannot produce MOBI" >&2
    exit 1
  fi
fi

"$EBOOK_CONVERT" build/dist/grust-book.epub build/dist/grust-book.mobi

echo "Built:"
echo "  docs/book/build/dist/grust-book.pdf"
echo "  docs/book/build/dist/grust-book.epub"
echo "  docs/book/build/dist/grust-book.mobi"
