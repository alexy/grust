#!/usr/bin/env python3
"""Fail a book build when generated EPUB metadata is weak or synthetic."""

from __future__ import annotations

import re
import sys
import zipfile
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


def first_xhtml_h1(source: str) -> str | None:
    match = re.search(r"<h1\b[^>]*>(.*?)</h1>", source, flags=re.DOTALL)
    if match is None:
        return None
    text = strip_tags(match.group(1))
    return text or None


def xhtml_text(source: str) -> str:
    collector = TextCollector()
    collector.feed(source)
    return " ".join(collector.parts)


def validate(epub_path: Path) -> list[str]:
    errors: list[str] = []
    with zipfile.ZipFile(epub_path) as epub:
        names = set(epub.namelist())
        opf_source = read_zip_text(epub, "EPUB/content.opf")
        toc_source = read_zip_text(epub, "EPUB/toc.ncx")
        nav_source = read_zip_text(epub, "EPUB/nav.xhtml")

        for package_name, source in {
            "EPUB/content.opf": opf_source,
            "EPUB/toc.ncx": toc_source,
            "EPUB/nav.xhtml": nav_source,
        }.items():
            if re.search(r"\b(UNTITLED|Unknown)\b", source, flags=re.IGNORECASE):
                errors.append(f"{package_name} contains fallback metadata")

        title = tag_text(opf_source, "dc:title")
        creator = tag_text(opf_source, "dc:creator")
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
            doc_title = re.search(
                r"<docTitle\b[^>]*>\s*<text\b[^>]*>(.*?)</text>\s*</docTitle>",
                toc_source,
                flags=re.DOTALL,
            )
            toc_title = strip_tags(doc_title.group(1)) if doc_title else None
            if toc_title != title:
                errors.append(f"EPUB/toc.ncx title is {toc_title!r}, expected {title!r}")

            nav_title = tag_text(nav_source, "title")
            nav_heading = first_xhtml_h1(nav_source)
            if title not in {nav_title, nav_heading}:
                errors.append(
                    f"EPUB/nav.xhtml does not expose the book title {title!r}"
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
    epub_path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("build/dist/grust-book.epub")
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
