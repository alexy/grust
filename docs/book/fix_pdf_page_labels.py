#!/usr/bin/env python3
"""Set PDF page labels so the cover is unnumbered and the body starts at 1."""

from __future__ import annotations

import sys
from pathlib import Path

from pypdf import PdfReader, PdfWriter


def main() -> int:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} path/to/book.pdf", file=sys.stderr)
        return 2

    pdf_path = Path(sys.argv[1])
    if not pdf_path.exists():
        print(f"PDF not found: {pdf_path}", file=sys.stderr)
        return 2

    reader = PdfReader(pdf_path)
    if len(reader.pages) < 2:
        print("PDF needs at least a cover page and one body page", file=sys.stderr)
        return 2

    writer = PdfWriter()
    for page in reader.pages:
        writer.add_page(page)

    writer.set_page_label(0, 0, prefix="")
    writer.set_page_label(1, len(reader.pages) - 1, style="/D", start=1)

    fixed_path = pdf_path.with_suffix(".labels.pdf")
    with fixed_path.open("wb") as handle:
        writer.write(handle)
    fixed_path.replace(pdf_path)

    labels = PdfReader(pdf_path).page_labels
    if labels[:3] != ["", "1", "2"]:
        print(f"unexpected first page labels: {labels[:3]!r}", file=sys.stderr)
        return 1

    print(f"PDF page labels fixed: {pdf_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
