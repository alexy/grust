#!/usr/bin/env python3
"""Fail a book build when generated EPUB metadata is weak or synthetic."""

from __future__ import annotations

import re
import sys
import tomllib
import zipfile
from filecmp import cmp
from html.parser import HTMLParser
from pathlib import Path


class TextCollector(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.parts: list[str] = []

    def handle_data(self, data: str) -> None:
        text = data.strip()
        if text:
            self.parts.append(text)


def read_zip_text(epub: zipfile.ZipFile, name: str) -> str:
    try:
        return epub.read(name).decode("utf-8")
    except KeyError:
        raise ValueError(f"missing {name}") from None


def strip_tags(source: str) -> str:
    return re.sub(r"<[^>]+>", "", source).strip()


def tag_text(source: str, tag: str) -> str | None:
    match = re.search(rf"<{tag}\b[^>]*>(.*?)</{tag}>", source, flags=re.DOTALL)
    if match is None:
        return None
    text = strip_tags(match.group(1))
    return text or None


def meta_property(source: str, property_name: str) -> str | None:
    match = re.search(
        rf'<meta\b[^>]*\bproperty="{re.escape(property_name)}"[^>]*>(.*?)</meta>',
        source,
        flags=re.DOTALL,
    )
    if match is None:
        return None
    text = strip_tags(match.group(1))
    return text or None


def title_sort(source: str) -> str | None:
    match = re.search(
        r'<meta\b[^>]*\brefines="#epub-title-1"[^>]*\bproperty="file-as"[^>]*>(.*?)</meta>',
        source,
        flags=re.DOTALL,
    )
    if match is None:
        return None
    text = strip_tags(match.group(1))
    return text or None


def first_xhtml_h1(source: str) -> str | None:
    match = re.search(r"<h1\b[^>]*>(.*?)</h1>", source, flags=re.DOTALL)
    if match is None:
        return None
    text = strip_tags(match.group(1))
    return text or None


def compact(source: str) -> str:
    return re.sub(r"\s+", " ", source).strip()


def require_pattern(
    errors: list[str], pattern: str, source: str, message: str, flags: int = 0
) -> None:
    if re.search(pattern, source, flags) is None:
        errors.append(message)


def reject_pattern(
    errors: list[str], pattern: str, source: str, message: str, flags: int = 0
) -> None:
    if re.search(pattern, source, flags) is not None:
        errors.append(message)


def xhtml_text(source: str) -> str:
    collector = TextCollector()
    collector.feed(source)
    return " ".join(collector.parts)


def read_metadata_value(source: str, key: str) -> str:
    match = re.search(
        rf'^\s*{re.escape(key)}\s*:\s*["\']?([^"\'\n]+?)["\']?\s*$',
        source,
        flags=re.MULTILINE,
    )
    if match is None:
        raise ValueError(f"missing {key} in metadata.yaml")
    return match.group(1)


def pandoc_slug(value: str) -> str:
    slug = re.sub(r"[^0-9A-Za-z]+", "-", value.lower()).strip("-")
    return slug or "section"


def expected_catalog_metadata() -> tuple[str, str, str, str]:
    cargo_toml = Path(__file__).resolve().parents[2] / "Cargo.toml"
    metadata_yaml = Path(__file__).resolve().parent / "metadata.yaml"
    with cargo_toml.open("rb") as handle:
        cargo = tomllib.load(handle)
    version = cargo.get("workspace", {}).get("package", {}).get("version")
    if not version:
        raise ValueError(f"missing [workspace.package] version in {cargo_toml}")
    metadata_source = metadata_yaml.read_text(encoding="utf-8")
    title_stem = read_metadata_value(metadata_source, "title_stem")
    visible_title = read_metadata_value(metadata_source, "title")
    return title_stem, f"{title_stem} ({version})", visible_title, pandoc_slug(visible_title)


def validate(epub_path: Path) -> list[str]:
    errors: list[str] = []
    title_stem, kindle_title, visible_title, visible_slug = expected_catalog_metadata()
    dist_dir = epub_path.parent
    stable_copy = dist_dir / f"{title_stem}.epub"
    upload_link = dist_dir / f"{kindle_title}.epub"
    version_marker = dist_dir / "VERSION.md"
    with zipfile.ZipFile(epub_path) as epub:
        names = set(epub.namelist())
        opf_source = read_zip_text(epub, "EPUB/content.opf")
        toc_source = read_zip_text(epub, "EPUB/toc.ncx")
        nav_source = read_zip_text(epub, "EPUB/nav.xhtml")
        cover_source = read_zip_text(epub, "EPUB/text/cover.xhtml")
        opf_flat = compact(opf_source)
        toc_flat = compact(toc_source)

        for package_name, source in {
            "EPUB/content.opf": opf_source,
            "EPUB/toc.ncx": toc_source,
            "EPUB/nav.xhtml": nav_source,
        }.items():
            if re.search(r"\b(UNTITLED|Unknown)\b", source, flags=re.IGNORECASE):
                errors.append(f"{package_name} contains fallback metadata")

        title = tag_text(opf_source, "dc:title")
        creator = tag_text(opf_source, "dc:creator")
        publisher = tag_text(opf_source, "dc:publisher")
        language = tag_text(opf_source, "dc:language")
        date = tag_text(opf_source, "dc:date")
        modified = meta_property(opf_source, "dcterms:modified")

        required = {
            "dc:title": title,
            "dc:creator": creator,
            "dc:language": language,
            "dc:date": date,
            "dcterms:modified": modified,
        }
        for field, value in required.items():
            if not value:
                errors.append(f"EPUB/content.opf is missing {field}")

        if title:
            if title != kindle_title:
                errors.append(
                    f"EPUB/content.opf dc:title is {title!r}, expected {kindle_title!r}"
                )
            sort_title = title_sort(opf_source)
            if sort_title != kindle_title:
                errors.append(
                    f"EPUB/content.opf title file-as is {sort_title!r}, expected {kindle_title!r}"
                )

            doc_title = re.search(
                r"<docTitle\b[^>]*>\s*<text\b[^>]*>(.*?)</text>\s*</docTitle>",
                toc_source,
                flags=re.DOTALL,
            )
            toc_title = strip_tags(doc_title.group(1)) if doc_title else None
            if toc_title != visible_title:
                errors.append(
                    f"EPUB/toc.ncx title is {toc_title!r}, expected {visible_title!r}"
                )

            nav_title = tag_text(nav_source, "title")
            nav_heading = first_xhtml_h1(nav_source)
            if visible_title not in {nav_title, nav_heading}:
                errors.append(
                    f"EPUB/nav.xhtml does not expose the book title {visible_title!r}"
                )

        if creator and creator != "Alexy Khrabrov":
            errors.append(
                f"EPUB/content.opf dc:creator is {creator!r}, expected 'Alexy Khrabrov'"
            )
        if publisher != "First Pair Press":
            errors.append(
                f"EPUB/content.opf dc:publisher is {publisher!r}, expected 'First Pair Press'"
            )
        if language and language != "en-US":
            errors.append(
                f"EPUB/content.opf dc:language is {language!r}, expected 'en-US'"
            )
        if date and re.fullmatch(r"\d{4}-\d{2}-\d{2}", date) is None:
            errors.append(
                f"EPUB/content.opf dc:date is {date!r}, expected YYYY-MM-DD"
            )

        if not stable_copy.exists():
            errors.append(f"missing stable title-stem EPUB {stable_copy}")
        elif epub_path.resolve() != stable_copy.resolve() and not cmp(epub_path, stable_copy, shallow=False):
            errors.append(
                f"stable title-stem EPUB {stable_copy} is not byte-identical to {epub_path}"
            )
        if not upload_link.exists():
            errors.append(f"missing Send to Kindle upload link {upload_link}")
        elif not upload_link.is_symlink():
            errors.append(f"Send to Kindle upload path {upload_link} is not a symlink")
        elif upload_link.resolve() != stable_copy.resolve():
            errors.append(
                f"Send to Kindle upload link {upload_link} does not resolve to {stable_copy}"
            )

        if not version_marker.exists():
            errors.append(f"missing dist marker {version_marker}")
        else:
            marker = version_marker.read_text(encoding="utf-8")
            expected_name_line = f"kindle_name: {kindle_title}"
            if expected_name_line not in marker.splitlines():
                errors.append(
                    f"{version_marker} does not include {expected_name_line!r}"
                )
            expected_file_line = f"epub_file: {title_stem}.epub"
            if expected_file_line not in marker.splitlines():
                errors.append(
                    f"{version_marker} does not include {expected_file_line!r}"
                )
            expected_link_line = f"kindle_link: {kindle_title}.epub"
            if expected_link_line not in marker.splitlines():
                errors.append(
                    f"{version_marker} does not include {expected_link_line!r}"
                )
            if date:
                built_at = next(
                    (
                        line.removeprefix("built_at: ")
                        for line in marker.splitlines()
                        if line.startswith("built_at: ")
                    ),
                    "",
                )
                if not built_at.startswith(date):
                    errors.append(
                        f"{version_marker} built_at {built_at!r} does not start with {date!r}"
                    )

        require_pattern(
            errors,
            r'<meta name="cover" content="[^"]+" />',
            opf_flat,
            "EPUB/content.opf is missing cover metadata",
        )
        require_pattern(
            errors,
            r'<item properties="cover-image"[^>]*href="media/[^"]+"',
            opf_flat,
            "EPUB/content.opf is missing the cover-image manifest item",
        )
        require_pattern(
            errors,
            r'<spine toc="ncx">\s*<itemref idref="cover_xhtml" />\s*<itemref idref="nav" linear="no" />\s*<itemref idref="ch001_xhtml" />',
            opf_flat,
            "EPUB/content.opf reading spine is not image cover, visible TOC, then preface",
        )
        require_pattern(
            errors,
            rf"<docTitle>\s*<text>{re.escape(visible_title)}</text>\s*</docTitle>",
            toc_flat,
            f"EPUB/toc.ncx title is not {visible_title}",
        )
        require_pattern(
            errors,
            rf"<title>{re.escape(visible_title)}</title>",
            nav_source,
            f"EPUB/nav.xhtml document title is not {visible_title}",
        )
        require_pattern(
            errors,
            rf"<h1[^>]*>{re.escape(visible_title)}</h1>",
            nav_source,
            f"EPUB/nav.xhtml table-of-contents heading is not {visible_title}",
        )
        require_pattern(
            errors,
            r'<body id="cover">',
            cover_source,
            "EPUB/text/cover.xhtml is not the Pandoc image cover",
        )
        require_pattern(
            errors,
            r'<div id="cover-image">',
            cover_source,
            "EPUB/text/cover.xhtml is missing its image wrapper",
        )
        require_pattern(
            errors,
            r'<svg[^>]*viewBox="0 0 1024 1536"',
            cover_source,
            "EPUB/text/cover.xhtml has the wrong image geometry",
        )
        require_pattern(
            errors,
            r'<image[^>]*xlink:href="\.\./media/[^"]+"',
            cover_source,
            "EPUB/text/cover.xhtml does not reference its image",
        )

        cover_item = re.search(
            r'<item properties="cover-image"[^>]*href="([^"]+)"', opf_source
        )
        if cover_item is not None:
            packaged_cover = f"EPUB/{cover_item.group(1)}"
            expected_cover = Path(__file__).resolve().parents[2] / "cover" / "grust-cover.png"
            if packaged_cover not in names:
                errors.append(f"missing packaged cover image {packaged_cover}")
            elif not expected_cover.is_file():
                errors.append(f"missing source cover image {expected_cover}")
            elif epub.read(packaged_cover) != expected_cover.read_bytes():
                errors.append(
                    f"{packaged_cover} differs from {expected_cover}"
                )

        title_page_names = sorted(
            name for name in names if name.startswith("EPUB/text/") and name.endswith("title_page.xhtml")
        )
        for name in title_page_names:
            visible_text = xhtml_text(read_zip_text(epub, name))
            if len(visible_text) < 20:
                errors.append(f"{name} looks like an empty generated title page")

    return errors


def main() -> int:
    epub_path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("build/dist/grust.epub")
    if not epub_path.exists():
        print(f"EPUB not found: {epub_path}", file=sys.stderr)
        return 2

    try:
        errors = validate(epub_path)
    except (zipfile.BadZipFile, ValueError) as exc:
        print(f"Invalid EPUB metadata package: {exc}", file=sys.stderr)
        return 2

    if errors:
        print(f"EPUB metadata check failed for {epub_path}:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1

    print(f"EPUB metadata check passed: {epub_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
