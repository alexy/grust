#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

mkdir -p build/dist

node build.mjs
pubdate="$(date -u +%F)"
version="$(
  awk '
    /^\[workspace\.package\]/ { in_workspace_package = 1; next }
    /^\[/ { in_workspace_package = 0 }
    in_workspace_package && /^version[[:space:]]*=/ {
      gsub(/"/, "", $3)
      print $3
      exit
    }
  ' ../../Cargo.toml
)"
title_stem="$(
  awk -F: '
    $1 ~ /^[[:space:]]*title_stem[[:space:]]*$/ {
      value = $2
      sub(/^[[:space:]]*/, "", value)
      sub(/[[:space:]]*$/, "", value)
      gsub(/^["'\''"]|["'\''"]$/, "", value)
      print value
      exit
    }
  ' metadata.yaml
)"
visible_title="$(
  awk -F: '
    $1 ~ /^[[:space:]]*title[[:space:]]*$/ {
      value = $2
      sub(/^[[:space:]]*/, "", value)
      sub(/[[:space:]]*$/, "", value)
      gsub(/^["'\''"]|["'\''"]$/, "", value)
      print value
      exit
    }
  ' metadata.yaml
)"

if [[ -z "$version" ]]; then
  echo "could not read workspace package version from Cargo.toml" >&2
  exit 1
fi

if [[ -z "$title_stem" ]]; then
  echo "could not read title_stem from metadata.yaml" >&2
  exit 1
fi

if [[ -z "$visible_title" ]]; then
  echo "could not read title from metadata.yaml" >&2
  exit 1
fi

kindle_title="$title_stem ($version)"

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
  --output build/grust-body.typ \
  build/manuscript.rendered.md

{
  printf '#outline(title: [Contents])\n'
  printf '#pagebreak()\n\n'
  cat build/grust-body.typ
} > build/grust-body-with-toc.typ

typst compile build/grust-body-with-toc.typ "$tmpdir/body.pdf"
pdfunite "$tmpdir/cover.pdf" "$tmpdir/body.pdf" build/dist/grust.pdf

PDF_PYTHON_CMD=()
if [[ -n "${PDF_PYTHON:-}" ]]; then
  PDF_PYTHON_CMD=("$PDF_PYTHON")
elif [[ -x /Users/alexy/.cache/codex-runtimes/codex-primary-runtime/dependencies/python/bin/python3 ]]; then
  PDF_PYTHON_CMD=(/Users/alexy/.cache/codex-runtimes/codex-primary-runtime/dependencies/python/bin/python3)
else
  PDF_PYTHON_CMD=()
fi

if [[ "${#PDF_PYTHON_CMD[@]}" -eq 0 ]]; then
  echo "python with pypdf not found; cannot set PDF page labels" >&2
  exit 1
fi

"${PDF_PYTHON_CMD[@]}" fix_pdf_page_labels.py build/dist/grust.pdf

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
  --metadata date="$pubdate" \
  --epub-title-page=false \
  --toc --toc-depth=2 \
  --css epub.css \
  --resource-path build \
  --output build/dist/grust.epub \
  "$tmpdir/cover.epub.md" build/manuscript.rendered.md

./fix_epub_layout.sh build/dist/grust.epub "$kindle_title" "$visible_title"
if [[ "grust" != "$title_stem" ]]; then
  cp build/dist/grust.epub "build/dist/$title_stem.epub"
fi
git_hash="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
versioned_stem="$title_stem ($version-$git_hash)"
# Stable version-only EPUB link (Send to Kindle / metadata gate).
find build/dist -maxdepth 1 -name "$title_stem (*).epub" -exec rm -f {} +
find build/dist -maxdepth 1 -name "$title_stem (*).pdf" -exec rm -f {} +
ln -s "$title_stem.epub" "build/dist/$kindle_title.epub"
# Always maintain a version+git-hash link for BOTH EPUB and PDF.
ln -s "grust.epub" "build/dist/$versioned_stem.epub"
ln -s "grust.pdf" "build/dist/$versioned_stem.pdf"
{
  printf 'kindle_name: %s\n' "$kindle_title"
  printf 'built_at: %s\n' "$pubdate"
  printf 'git_hash: %s\n' "$git_hash"
  printf 'epub_file: %s.epub\n' "$title_stem"
  printf 'kindle_link: %s.epub\n' "$kindle_title"
  printf 'epub_link: %s.epub\n' "$versioned_stem"
  printf 'pdf_link: %s.pdf\n' "$versioned_stem"
} > build/dist/VERSION.md
"${PYTHON_CMD[@]}" check_epub_metadata.py build/dist/grust.epub

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

"$EBOOK_CONVERT" build/dist/grust.epub build/dist/grust.mobi

echo "Built:"
echo "  docs/book/build/dist/grust.pdf"
echo "  docs/book/build/dist/grust.epub"
echo "  docs/book/build/dist/$kindle_title.epub -> $title_stem.epub"
echo "  docs/book/build/dist/$versioned_stem.epub -> grust.epub"
echo "  docs/book/build/dist/$versioned_stem.pdf -> grust.pdf"
echo "  docs/book/build/dist/grust.mobi"
echo "  docs/book/build/dist/VERSION.md"
