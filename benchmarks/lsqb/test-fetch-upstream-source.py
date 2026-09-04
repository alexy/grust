#!/usr/bin/env python3
"""Focused tests for the pinned LSQB archive extractor."""

from __future__ import annotations

import importlib.util
import io
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest


sys.dont_write_bytecode = True
SCRIPT = Path(__file__).with_name("fetch-upstream-source.py")
SPEC = importlib.util.spec_from_file_location("fetch_upstream_source", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot import {SCRIPT}")
FETCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(FETCH)


def archive_with(extra: list[tarfile.TarInfo] | None = None) -> bytes:
    stream = io.BytesIO()
    root = "lsqb-test"
    files = {
        f"{root}/README.md": b"readme\n",
        f"{root}/expected-output/expected-output.csv": b"oracle\n",
        f"{root}/ladybug/run.sh": b"#!/bin/sh\n",
    }
    with tarfile.open(fileobj=stream, mode="w:gz") as archive:
        directory = tarfile.TarInfo(root)
        directory.type = tarfile.DIRTYPE
        archive.addfile(directory)
        for name, content in files.items():
            member = tarfile.TarInfo(name)
            member.size = len(content)
            member.mode = 0o755 if name.endswith(".sh") else 0o644
            archive.addfile(member, io.BytesIO(content))
        for member in extra or []:
            content = b"bad\n"
            if member.isfile():
                member.size = len(content)
                archive.addfile(member, io.BytesIO(content))
            else:
                archive.addfile(member)
    return stream.getvalue()


class ExtractArchiveTests(unittest.TestCase):
    def test_extracts_regular_files_and_preserves_executable_mode(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / "source"
            FETCH.extract_verified_archive(archive_with(), "lsqb-test", destination)
            self.assertEqual((destination / "README.md").read_bytes(), b"readme\n")
            self.assertTrue((destination / "ladybug/run.sh").stat().st_mode & 0o100)

    def test_rejects_parent_traversal(self) -> None:
        member = tarfile.TarInfo("lsqb-test/../escape")
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaises(FETCH.SourceArchiveError):
                FETCH.extract_verified_archive(
                    archive_with([member]), "lsqb-test", Path(temporary) / "source"
                )

    def test_rejects_symbolic_links(self) -> None:
        member = tarfile.TarInfo("lsqb-test/link")
        member.type = tarfile.SYMTYPE
        member.linkname = "/etc/passwd"
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaises(FETCH.SourceArchiveError):
                FETCH.extract_verified_archive(
                    archive_with([member]), "lsqb-test", Path(temporary) / "source"
                )

    def test_rejects_existing_destination(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / "source"
            destination.mkdir()
            with self.assertRaises(FETCH.SourceArchiveError):
                FETCH.extract_verified_archive(archive_with(), "lsqb-test", destination)


if __name__ == "__main__":
    unittest.main()
