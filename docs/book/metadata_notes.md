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
title_stem: typesec
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

The build date can stay dynamic, but title, title stem, subtitle, author,
language, publisher, and rights should live in source control. The title stem is
the short catalog and distribution stem. It can differ from the visible book
title, and it should be stable across releases.

For books that are repeatedly sent to Kindle during development, the title shown
in the Kindle library should include the current project version. This is only
the library/catalog title, not the visible title inside the book. Keep the
visible book title in the checked-in metadata file, then post-process only
`dc:title` and its title-sort metadata in `EPUB/content.opf` after Pandoc builds
the EPUB:

```sh
version="0.4.0"
title_stem="grust"
kindle_title="$title_stem ($version)"

KINDLE_TITLE="$kindle_title" perl -0pi -e '
  my $title = $ENV{KINDLE_TITLE};
  s{<meta\s+refines="\#epub-title-1"\s+property="file-as">.*?</meta>\s*}{}s;
  s{<dc:title([^>]*)>.*?</dc:title>}{<dc:title$1>$title</dc:title>\n    <meta refines="#epub-title-1" property="file-as">$title</meta>}s;
' EPUB/content.opf
```

For the current Grust release, that means Kindle should see this package title:

```xml
<dc:title id="epub-title-1">grust (0.4.0)</dc:title>
<meta refines="#epub-title-1" property="file-as">grust (0.4.0)</meta>
```

while `EPUB/nav.xhtml`, `EPUB/toc.ncx`, and the cover XHTML still display the
plain visible title `Grust`.

For distribution files, keep one stable title-stem EPUB under version control
and make the versioned Kindle fallback name a generated symlink. This avoids
adding a new tracked EPUB filename for every release while still giving Send to
Kindle a versioned filename if it falls back from package metadata:

```sh
cp build/dist/grust.epub "build/dist/$title_stem.epub"
find build/dist -maxdepth 1 -name "$title_stem (*).epub" -exec rm -f {} +
ln -s "$title_stem.epub" "build/dist/$kindle_title.epub"
{
  printf 'kindle_name: %s\n' "$kindle_title"
  printf 'built_at: %s\n' "$pubdate"
  printf 'epub_file: %s.epub\n' "$title_stem"
  printf 'kindle_link: %s.epub\n' "$kindle_title"
} > build/dist/VERSION.md
```

The repository should track the stable stem file and marker, for example
`grust.epub` and `VERSION.md`, and ignore version-suffixed artifacts such as
`grust (0.4.0).epub`.

For Grust, that means the book-local ignore rules should look like:

```gitignore
build/dist/*
!build/dist/grust.pdf
!build/dist/grust.epub
!build/dist/grust.epub
!build/dist/grust.mobi
!build/dist/VERSION.md
```

## How Grust Handles This Now

Grust already had the most important piece before the checker was added: its
stable EPUB metadata lives in `docs/book/metadata.yaml`. That file is the source
of truth for title, subtitle, author, coauthor credit, language, rights, and TOC
settings. `docs/book/build.sh` passes the same file to Pandoc for both the Typst
body render and the EPUB render. The Grust EPUB command also uses
`--epub-title-page=false`, so Pandoc does not add a generated title page in front
of the custom cover.

Grust's build script reads `title_stem` from `docs/book/metadata.yaml` and
`[workspace.package].version` from the workspace `Cargo.toml`, constructs
`kindle_title="$title_stem ($version)"`, and passes that value to
`docs/book/fix_epub_layout.sh`. The fixer rewrites only
`EPUB/content.opf`'s `dc:title` and title sort metadata; it does not change the
visible title in the cover, navigation document, or NCX table of contents. The
checker reads the same workspace version with Python's standard-library
`tomllib`, reads the same `title_stem`, and expects both OPF title fields to
match `<title_stem> (<version>)`.

Grust has two extra protections beyond the metadata file. First,
`docs/book/fix_epub_layout.sh` applies the same kind of post-Pandoc EPUB layout
repair that TypeSec uses: the custom cover becomes the first spine item, the nav
document remains a visible TOC page after the cover, and the cover XHTML is marked as frontmatter instead
of bodymatter. Second, `docs/book/check_epub_metadata.py` reads the generated
EPUB package and validates the artifact itself, including the stable
title-stem EPUB, ignored versioned Send to Kindle symlink, and `VERSION.md`
marker. The build runs the fixer, creates `grust.epub`, recreates the
version-suffixed symlink to it, writes the marker, and then runs the checker
after creating `build/dist/grust.epub` and before converting the EPUB to
MOBI. That order matters because it prevents Kindle-facing formats from being
generated from an EPUB with weak metadata, mismatched dist markers, synthetic
fallback labels, or fragile cover layout.

The repository is pinned to asdf Python `3.14.5` in the root `.tool-versions`
file. `build.sh` prefers `asdf exec python` for the metadata check, with
`python3` only as a fallback. This avoids accidentally using a broken or
machine-specific Homebrew Python when the build is run from the Grust checkout.

For Grust, the normal verification command is:

```sh
cd docs/book
asdf exec python check_epub_metadata.py build/dist/grust.epub
```

The full build runs the same check automatically:

```sh
cd docs/book
./build.sh
```

Grust uses a Python checker rather than copying TypeSec's shell checker because
the Grust repository now has an asdf-pinned Python, and the check is naturally a
small ZIP/package inspection program: open the EPUB, read OPF/NCX/nav files,
collect all failures, and report them together. Python's standard library keeps
that logic explicit without a chain of temporary files, `unzip`, `grep`, and
flattened XML snippets. The script deliberately avoids third-party packages.

TypeSec's `docs/book/check_epub_metadata.sh` is still a good fit for TypeSec's
simpler pipeline. It uses shell tools to assert exact TypeSec metadata, spine,
cover, and Kindle-layout invariants after TypeSec's EPUB layout fixer runs.
Grust now enforces the same categories: exact Grust title/creator/language/date
metadata, cover-then-visible-TOC spine order, NCX/nav titles,
frontmatter cover XHTML, a first custom titlepage section, stable title-stem
EPUB output, ignored versioned symlink, `VERSION.md` dist marker, no generated
cover heading, no flexbox on the cover, no fallback metadata, and no generated
empty title page.

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
- the first readable spine item is not the custom cover XHTML
- the navigation document is not visible after the cover and before the preface
- the Kindle-facing OPF title does not match `<title_stem> (<version>)`
- the OPF title lacks a matching `file-as` refinement
- the stable title-stem EPUB, such as `grust.epub`, is missing
- the stable title-stem EPUB is not byte-identical to the canonical EPUB
- the versioned Send to Kindle path, such as `grust (0.4.0).epub`, is missing
- the versioned Send to Kindle path is not a symlink to the stable title-stem EPUB
- `VERSION.md` does not include the generated Kindle name
- `VERSION.md` does not include the dist build date
- `VERSION.md` does not include the stable EPUB filename
- `VERSION.md` does not include the versioned symlink filename
- `EPUB/toc.ncx` does not have the real book title
- `EPUB/nav.xhtml` does not have the real book title
- the cover XHTML is not frontmatter
- the cover XHTML contains a generated wrapper heading before the custom cover
- the cover XHTML uses `display: flex`
- any OPF, NCX, or nav file contains `UNTITLED` or `Unknown`
- `EPUB/text/title_page.xhtml` exists only as an empty generated title page

Run this check before converting the EPUB to MOBI, AZW3, or any Kindle-facing
format. That way downstream artifacts cannot be generated from a broken EPUB.

## Practical Verification Commands

Inspect the package metadata:

```sh
unzip -p build/dist/grust.epub EPUB/content.opf
```

Inspect navigation titles:

```sh
unzip -p build/dist/grust.epub EPUB/toc.ncx
unzip -p build/dist/grust.epub EPUB/nav.xhtml
```

Check the stable stem file, ignored versioned symlink, and marker:

```sh
cmp -s build/dist/grust.epub build/dist/grust.epub
readlink "build/dist/grust (0.4.0).epub"
cat build/dist/VERSION.md
```

List generated files and look for unwanted title pages:

```sh
unzip -l build/dist/grust.epub
```

Ask Calibre how it sees the book:

```sh
/Applications/calibre.app/Contents/MacOS/ebook-meta build/dist/grust.epub
```

Smoke-test Kindle-style conversion:

```sh
/Applications/calibre.app/Contents/MacOS/ebook-convert build/dist/grust.epub /tmp/book.azw3
```

Calibre accepting the EPUB does not prove Amazon will accept it, but it is a
useful local check. The stronger protection is a build-time metadata validator
that rejects weak or generated fallback metadata before any ebook conversion
happens.
