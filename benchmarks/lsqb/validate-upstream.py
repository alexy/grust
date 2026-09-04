#!/usr/bin/env python3
"""Validate raw upstream LSQB Ladybug observations and emit their hashes."""

from __future__ import annotations

import argparse
import csv
from decimal import Decimal, InvalidOperation
import hashlib
import os
from pathlib import Path
import re
import sys
import tempfile


PINNED_ORACLE_SHA256 = (
    "f2467b14cd6a060e8513d5357471ae6cff486c2f5e38074febe08a4cf4db0d3a"
)
EXPECTED_SYSTEM = "Ladybug-0.19.0"
QUERIES = tuple(range(1, 10))
SUPPORTED_SCALES = ("example", "0.1", "0.3")
THREADS_PATTERN = re.compile(r"([1-9][0-9]*) threads")
INTEGER_PATTERN = re.compile(r"0|[1-9][0-9]*")
TIMING_PATTERN = re.compile(r"(0|[1-9][0-9]*)\.[0-9]{4}")


class ValidationError(RuntimeError):
    """The upstream evidence does not satisfy its publication contract."""


def positive_integer(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be a positive integer") from error
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return parsed


def sha256_bytes(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def read_regular_file(path: Path, description: str) -> bytes:
    if path.is_symlink() or not path.is_file():
        raise ValidationError(f"{description} is not a regular non-symlink file: {path}")
    try:
        return path.read_bytes()
    except OSError as error:
        raise ValidationError(f"cannot read {description} {path}: {error}") from error


def rows_from_bytes(content: bytes, description: str) -> list[list[str]]:
    if not content.endswith(b"\n"):
        raise ValidationError(f"{description} must end with a newline")
    if b"\r" in content:
        raise ValidationError(f"{description} must use LF, not CRLF, line endings")
    try:
        text = content.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValidationError(f"{description} is not valid UTF-8: {error}") from error
    try:
        return list(csv.reader(text.splitlines(), delimiter="\t", strict=True))
    except csv.Error as error:
        raise ValidationError(f"{description} is not valid TSV: {error}") from error


def load_oracle(path: Path, scale: str) -> tuple[dict[int, int], str, int]:
    content = read_regular_file(path, "oracle")
    actual_sha256 = sha256_bytes(content)
    if actual_sha256 != PINNED_ORACLE_SHA256:
        raise ValidationError(
            "oracle SHA-256 mismatch: "
            f"expected {PINNED_ORACLE_SHA256}, received {actual_sha256}"
        )

    counts: dict[int, int] = {}
    for line_number, row in enumerate(rows_from_bytes(content, "oracle"), start=1):
        if len(row) != 6:
            raise ValidationError(
                f"oracle line {line_number} has {len(row)} fields; expected 6"
            )
        if row[0] != "expected" or row[1] != "":
            raise ValidationError(f"oracle line {line_number} has invalid identity fields")
        if row[2] != scale:
            continue
        if not INTEGER_PATTERN.fullmatch(row[3]):
            raise ValidationError(f"oracle line {line_number} has invalid query number")
        query = int(row[3])
        if query not in QUERIES:
            raise ValidationError(f"oracle line {line_number} has query outside q1-q9")
        if query in counts:
            raise ValidationError(f"oracle contains duplicate q{query} for scale {scale}")
        if not INTEGER_PATTERN.fullmatch(row[5]):
            raise ValidationError(f"oracle line {line_number} has invalid expected count")
        counts[query] = int(row[5])

    if tuple(sorted(counts)) != QUERIES:
        missing = ", ".join(f"q{query}" for query in QUERIES if query not in counts)
        raise ValidationError(f"oracle for scale {scale} is incomplete; missing {missing}")
    return counts, actual_sha256, len(content)


def validate_timing(value: str, description: str) -> None:
    if TIMING_PATTERN.fullmatch(value) is None:
        raise ValidationError(
            f"{description} timing must be canonical nonnegative seconds "
            f"with four decimal places: {value!r}"
        )
    try:
        timing = Decimal(value)
    except InvalidOperation as error:
        raise ValidationError(
            f"{description} has a non-numeric timing value: {value!r}"
        ) from error
    if not timing.is_finite() or timing < 0:
        raise ValidationError(
            f"{description} timing must be finite and nonnegative: {value!r}"
        )


def validate_raw_file(
    path: Path,
    run: int,
    scale: str,
    expected_threads: int,
    oracle: dict[int, int],
) -> tuple[str, int]:
    content = read_regular_file(path, f"run {run} result")
    rows = rows_from_bytes(content, f"run {run} result")
    if len(rows) != len(QUERIES):
        raise ValidationError(
            f"run {run} has {len(rows)} observations; expected {len(QUERIES)}"
        )

    observed_threads: int | None = None
    for index, row in enumerate(rows, start=1):
        description = f"run {run} row {index}"
        if len(row) != 6:
            raise ValidationError(
                f"{description} has {len(row)} fields; expected 6"
            )
        system, threads_text, observed_scale, query_text, timing, count_text = row
        if system != EXPECTED_SYSTEM:
            raise ValidationError(
                f"{description} system must be {EXPECTED_SYSTEM!r}, received {system!r}"
            )
        threads_match = THREADS_PATTERN.fullmatch(threads_text)
        if threads_match is None:
            raise ValidationError(
                f"{description} thread field must be a positive '<N> threads' value"
            )
        row_threads = int(threads_match.group(1))
        if row_threads != expected_threads:
            raise ValidationError(
                f"{description} thread count must be {expected_threads}, "
                f"received {row_threads}"
            )
        if observed_threads is None:
            observed_threads = row_threads
        elif row_threads != observed_threads:
            raise ValidationError(f"run {run} uses inconsistent thread counts")
        if observed_scale != scale:
            raise ValidationError(
                f"{description} scale must be {scale!r}, received {observed_scale!r}"
            )
        expected_query = QUERIES[index - 1]
        if query_text != str(expected_query):
            raise ValidationError(
                f"{description} must be q{expected_query}, received {query_text!r}"
            )
        validate_timing(timing, description)
        if not INTEGER_PATTERN.fullmatch(count_text):
            raise ValidationError(f"{description} has invalid integer count {count_text!r}")
        observed_count = int(count_text)
        if observed_count != oracle[expected_query]:
            raise ValidationError(
                f"{description} q{expected_query} count mismatch: "
                f"expected {oracle[expected_query]}, received {observed_count}"
            )

    if observed_threads is None:
        raise ValidationError(f"run {run} contains no thread count")
    return sha256_bytes(content), len(content)


def oracle_counts_sha256(scale: str, oracle: dict[int, int]) -> str:
    manifest = bytearray(b"grust-lsqb-upstream-count-oracle-v1\0")
    manifest.extend(f"scale\t{scale}\n".encode())
    for query in QUERIES:
        manifest.extend(f"q{query}\t{oracle[query]}\n".encode())
    return sha256_bytes(bytes(manifest))


def render_validation(
    scale: str,
    runs: int,
    threads: int,
    oracle_sha256: str,
    oracle_bytes: int,
    oracle_counts_digest: str,
    artifacts: list[tuple[str, str, int]],
) -> bytes:
    fields = [
        ("schema", "grust-lsqb-upstream-validation-v1"),
        ("status", "pass"),
        ("warning", "These are not LDBC Benchmark Results."),
        ("system", EXPECTED_SYSTEM),
        ("threads", str(threads)),
        ("scale_factor", scale),
        ("measurement_iterations", str(runs)),
        ("queries_per_iteration", str(len(QUERIES))),
        ("observation_count", str(runs * len(QUERIES))),
        ("oracle_sha256", oracle_sha256),
        ("oracle_bytes", str(oracle_bytes)),
        ("oracle_counts_sha256", oracle_counts_digest),
    ]
    for run, (name, digest, byte_length) in enumerate(artifacts, start=1):
        fields.extend(
            (
                (f"raw_file_{run}", name),
                (f"raw_sha256_{run}", digest),
                (f"raw_bytes_{run}", str(byte_length)),
                (f"raw_rows_{run}", str(len(QUERIES))),
            )
        )
    return ("field\tvalue\n" + "".join(f"{key}\t{value}\n" for key, value in fields)).encode()


def write_atomic(path: Path, content: bytes) -> None:
    if path.exists() or path.is_symlink():
        raise ValidationError(f"refusing to overwrite validation output: {path}")
    path.parent.mkdir(parents=False, exist_ok=True)
    temporary_name: str | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="wb",
            prefix=f".{path.name}.",
            dir=path.parent,
            delete=False,
        ) as temporary:
            temporary_name = temporary.name
            temporary.write(content)
            temporary.flush()
            os.fsync(temporary.fileno())
        os.chmod(temporary_name, 0o644)
        os.replace(temporary_name, path)
        temporary_name = None
    finally:
        if temporary_name is not None:
            try:
                os.unlink(temporary_name)
            except FileNotFoundError:
                pass


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--runs", required=True, type=positive_integer)
    parser.add_argument("--threads", required=True, type=positive_integer)
    parser.add_argument("--scale", required=True, choices=SUPPORTED_SCALES)
    parser.add_argument("--oracle", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument(
        "--check-existing",
        action="store_true",
        help="recompute and compare an existing validation receipt without writing",
    )
    return parser.parse_args()


def main() -> int:
    arguments = parse_args()
    output_directory = arguments.output_dir
    if output_directory.is_symlink() or not output_directory.is_dir():
        print(
            f"validate-upstream.py: output directory is not a regular directory: "
            f"{output_directory}",
            file=sys.stderr,
        )
        return 1
    output = arguments.output or output_directory / "raw-validation.tsv"
    try:
        if output.parent.resolve() != output_directory.resolve():
            raise ValidationError("validation output must be directly inside output directory")
        expected_files = [
            output_directory / f"upstream-ladybug-run-{run}.csv"
            for run in range(1, arguments.runs + 1)
        ]
        observed_files = sorted(output_directory.glob("upstream-ladybug-run-*.csv"))
        if {path.name for path in observed_files} != {path.name for path in expected_files}:
            expected_names = ", ".join(path.name for path in expected_files)
            observed_names = ", ".join(path.name for path in observed_files) or "none"
            raise ValidationError(
                f"raw result set mismatch; expected [{expected_names}], "
                f"observed [{observed_names}]"
            )

        oracle, oracle_sha256, oracle_bytes = load_oracle(
            arguments.oracle, arguments.scale
        )
        artifacts: list[tuple[str, str, int]] = []
        for run, path in enumerate(expected_files, start=1):
            digest, byte_length = validate_raw_file(
                path,
                run,
                arguments.scale,
                arguments.threads,
                oracle,
            )
            artifacts.append((path.name, digest, byte_length))

        content = render_validation(
            arguments.scale,
            arguments.runs,
            arguments.threads,
            oracle_sha256,
            oracle_bytes,
            oracle_counts_sha256(arguments.scale, oracle),
            artifacts,
        )
        if arguments.check_existing:
            existing = read_regular_file(output, "validation receipt")
            if existing != content:
                raise ValidationError(
                    "existing validation receipt is not the canonical receipt for "
                    "the raw results"
                )
        else:
            write_atomic(output, content)
    except (OSError, ValidationError) as error:
        print(f"validate-upstream.py: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
