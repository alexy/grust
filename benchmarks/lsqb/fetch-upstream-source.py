#!/usr/bin/env python3
"""Fetch and safely extract the byte-pinned upstream LSQB source archive."""

from __future__ import annotations

import argparse
import hashlib
import io
import os
from pathlib import Path, PurePosixPath
import shutil
import sys
import tarfile
import tempfile
from urllib.parse import urlparse
from urllib.request import Request, urlopen


MAX_EXTRACTED_BYTES = 64 * 1024 * 1024
REQUIRED_PATHS = (
    "README.md",
    "expected-output/expected-output.csv",
    "ladybug/run.sh",
)


class SourceArchiveError(RuntimeError):
    """The pinned source archive could not be authenticated or extracted."""


def download_verified(url: str, expected_bytes: int, expected_sha256: str) -> bytes:
    """Download an HTTPS object and authenticate its exact bytes."""
    if urlparse(url).scheme != "https":
        raise SourceArchiveError("source URL must use HTTPS")
    if expected_bytes <= 0:
        raise SourceArchiveError("expected byte length must be positive")
    if len(expected_sha256) != 64 or any(
        character not in "0123456789abcdef" for character in expected_sha256
    ):
        raise SourceArchiveError("expected SHA-256 must be 64 lowercase hex digits")

    request = Request(url, headers={"User-Agent": "grust-lsqb-source-fetch/1"})
    try:
        with urlopen(request, timeout=120) as response:  # noqa: S310 - HTTPS checked above.
            final_url = response.geturl()
            if urlparse(final_url).scheme != "https":
                raise SourceArchiveError("source download redirected away from HTTPS")
            declared_length = response.headers.get("Content-Length")
            if declared_length is not None and declared_length.isdecimal():
                if int(declared_length) != expected_bytes:
                    raise SourceArchiveError(
                        "source archive Content-Length mismatch: "
                        f"expected {expected_bytes}, received {declared_length}"
                    )
            chunks: list[bytes] = []
            observed_bytes = 0
            while True:
                chunk = response.read(64 * 1024)
                if not chunk:
                    break
                observed_bytes += len(chunk)
                if observed_bytes > expected_bytes:
                    raise SourceArchiveError(
                        "source archive is larger than the pinned byte length"
                    )
                chunks.append(chunk)
    except SourceArchiveError:
        raise
    except OSError as error:
        raise SourceArchiveError(f"source download failed: {error}") from error

    archive = b"".join(chunks)
    if len(archive) != expected_bytes:
        raise SourceArchiveError(
            "source archive byte-length mismatch: "
            f"expected {expected_bytes}, received {len(archive)}"
        )
    actual_sha256 = hashlib.sha256(archive).hexdigest()
    if actual_sha256 != expected_sha256:
        raise SourceArchiveError(
            "source archive SHA-256 mismatch: "
            f"expected {expected_sha256}, received {actual_sha256}"
        )
    return archive


def _member_parts(member: tarfile.TarInfo, archive_root: str) -> tuple[str, ...]:
    name = member.name.rstrip("/")
    if not name or "\\" in name or "\x00" in name:
        raise SourceArchiveError(f"unsafe archive member name: {member.name!r}")
    raw_parts = name.split("/")
    if any(part in ("", ".", "..") for part in raw_parts):
        raise SourceArchiveError(f"unsafe archive member path: {member.name!r}")
    path = PurePosixPath(name)
    parts = path.parts
    if path.is_absolute():
        raise SourceArchiveError(f"unsafe archive member path: {member.name!r}")
    if parts[0] != archive_root:
        raise SourceArchiveError(
            f"archive member is outside expected root {archive_root!r}: {member.name!r}"
        )
    return tuple(parts[1:])


def extract_verified_archive(
    archive: bytes, archive_root: str, destination: Path
) -> None:
    """Extract regular files/directories only, beneath one exact archive root."""
    if not archive_root or "/" in archive_root or archive_root in (".", ".."):
        raise SourceArchiveError("archive root must be one safe path component")
    if destination.exists() or destination.is_symlink():
        raise SourceArchiveError(f"destination already exists: {destination}")
    parent = destination.parent
    if not parent.is_dir():
        raise SourceArchiveError(f"destination parent is not a directory: {parent}")

    try:
        source_tar = tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz")
    except (OSError, tarfile.TarError) as error:
        raise SourceArchiveError(f"source archive is not a valid gzip tar: {error}") from error

    with source_tar:
        members = source_tar.getmembers()
        if not members:
            raise SourceArchiveError("source archive is empty")

        seen: set[tuple[str, ...]] = set()
        extracted_bytes = 0
        validated: list[tuple[tarfile.TarInfo, tuple[str, ...]]] = []
        for member in members:
            parts = _member_parts(member, archive_root)
            if parts in seen:
                raise SourceArchiveError(f"duplicate archive member: {member.name!r}")
            seen.add(parts)
            if not (member.isdir() or member.isfile()):
                raise SourceArchiveError(
                    f"archive member is not a regular file or directory: {member.name!r}"
                )
            if member.isfile():
                extracted_bytes += member.size
                if extracted_bytes > MAX_EXTRACTED_BYTES:
                    raise SourceArchiveError(
                        "source archive exceeds the extracted-byte safety limit"
                    )
            validated.append((member, parts))

        for required in REQUIRED_PATHS:
            if tuple(PurePosixPath(required).parts) not in seen:
                raise SourceArchiveError(
                    f"source archive is missing required path: {required}"
                )

        temporary_root = Path(
            tempfile.mkdtemp(prefix=".grust-lsqb-source-", dir=parent)
        )
        staging = temporary_root / "source"
        staging.mkdir()
        directory_modes: list[tuple[Path, int]] = []
        try:
            for member, parts in validated:
                if not parts:
                    continue
                target = staging.joinpath(*parts)
                if member.isdir():
                    target.mkdir(parents=True, exist_ok=True)
                    if target.is_symlink() or not target.is_dir():
                        raise SourceArchiveError(
                            f"archive directory collides with another member: {member.name!r}"
                        )
                    directory_modes.append((target, member.mode & 0o777))
                    continue
                target.parent.mkdir(parents=True, exist_ok=True)
                source = source_tar.extractfile(member)
                if source is None:
                    raise SourceArchiveError(
                        f"cannot read regular archive member: {member.name!r}"
                    )
                with source, target.open("xb") as output:
                    shutil.copyfileobj(source, output)
                target.chmod(member.mode & 0o777)

            for directory, mode in sorted(
                directory_modes, key=lambda item: len(item[0].parts), reverse=True
            ):
                directory.chmod(mode)
            os.replace(staging, destination)
        except SourceArchiveError:
            raise
        except OSError as error:
            raise SourceArchiveError(f"cannot extract source archive: {error}") from error
        finally:
            shutil.rmtree(temporary_root, ignore_errors=True)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--expected-bytes", required=True, type=int)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--archive-root", required=True)
    parser.add_argument("--destination", required=True, type=Path)
    return parser.parse_args()


def main() -> int:
    arguments = parse_args()
    try:
        archive = download_verified(
            arguments.url,
            arguments.expected_bytes,
            arguments.expected_sha256,
        )
        extract_verified_archive(
            archive,
            arguments.archive_root,
            arguments.destination,
        )
    except SourceArchiveError as error:
        print(f"fetch-upstream-source.py: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
