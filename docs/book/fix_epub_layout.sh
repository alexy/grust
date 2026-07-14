#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 3 ]]; then
  echo "usage: $0 path/to/book.epub [kindle-title] [visible-title]" >&2
  exit 2
fi

epub="$1"
kindle_title="${2:-grust}"
visible_title="${3:-Grust}"
visible_slug="$(
  printf '%s' "$visible_title" |
    tr '[:upper:]' '[:lower:]' |
    sed -E 's/[^[:alnum:]]+/-/g; s/^-+//; s/-+$//'
)"

if [[ ! -f "$epub" ]]; then
  echo "EPUB not found: $epub" >&2
  exit 2
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

workdir="$tmpdir/work"
mkdir -p "$workdir"
unzip -q "$epub" -d "$workdir"

content_opf="$workdir/EPUB/content.opf"
fixed="$tmpdir/fixed.epub"

perl -0pi -e '
  s#\s*<itemref idref="nav"(?: linear="no")? />##g;
  s#<spine toc="ncx">\s*<itemref idref="cover_xhtml" />#<spine toc="ncx">\n    <itemref idref="cover_xhtml" />\n    <itemref idref="nav" linear="no" />#s;
' "$content_opf"

KINDLE_TITLE="$kindle_title" perl -0pi -e '
  my $title = $ENV{KINDLE_TITLE};
  $title =~ s/&/&amp;/g;
  $title =~ s/</&lt;/g;
  $title =~ s/>/&gt;/g;
  s{<meta\s+refines="\#epub-title-1"\s+property="file-as">.*?</meta>\s*}{}s;
  s{<dc:title([^>]*)>.*?</dc:title>}{<dc:title$1>$title</dc:title>\n    <meta refines="#epub-title-1" property="file-as">$title</meta>}s;
' "$content_opf"

(
  cd "$workdir"
  zip -X0q "$fixed" mimetype
  zip -Xrq "$fixed" META-INF EPUB
)

mv "$fixed" "$epub"
echo "EPUB layout fixed: $epub"
