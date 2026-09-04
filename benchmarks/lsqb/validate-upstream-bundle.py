#!/usr/bin/env python3
"""Strictly validate a completed upstream LSQB Ladybug evidence bundle."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import sys
from types import ModuleType

sys.dont_write_bytecode = True


UPSTREAM_COMMIT = "242cb2fd31340ca688954cb94794d74c0d5b6f92"
UPSTREAM_ARCHIVE_URL = (
    "https://codeload.github.com/ldbc/lsqb/tar.gz/" + UPSTREAM_COMMIT
)
UPSTREAM_ARCHIVE_SHA256 = (
    "db17ee8b0a8559d6cb7c06e1388e6d89cee2ac924779473ac847965c0c0d37bb"
)
UPSTREAM_ARCHIVE_BYTES = "2861380"
EXPECTED_OUTPUT_SHA256 = (
    "f2467b14cd6a060e8513d5357471ae6cff486c2f5e38074febe08a4cf4db0d3a"
)
RUNNER_IMAGE = "grust-lsqb-upstream:242cb2fd"
WARNING = "These are not LDBC Benchmark Results."
SHA256_PATTERN = re.compile(r"[0-9a-f]{64}")
IMAGE_ID_PATTERN = re.compile(r"sha256:[0-9a-f]{64}")
REVISION_PATTERN = re.compile(r"[0-9a-f]{40}")
KEY_PATTERN = re.compile(r"[a-z][a-z0-9_]*")
UTC_PATTERN = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z")
DOCKER_VERSION_PATTERN = re.compile(
    r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z][0-9A-Za-z.-]*)?"
)
WATCHDOG_SCHEMA = "grust-lsqb-cell-watchdog-completion-v1"
WATCHDOG_PROJECT_PATTERN = re.compile(r"grust-lsqb-upstream-[0-9]+-[0-9]+")
WATCHDOG_FIELDS = {
    "child_exit_status",
    "container_id",
    "container_name",
    "elapsed_wall_ms",
    "project",
    "schema",
    "service",
    "status",
    "timeout_ms",
}

DATASETS = {
    "example": {
        "extracted_manifest_sha256": (
            "e47d935e186ccda58147fc2609d3db1a6f0e218b92384cf63a7161e2c2974def"
        ),
        "archive_sha256": "not-applicable",
        "archive_bytes": "not-applicable",
        "dataset_receipt_sha256": "not-applicable",
    },
    "0.1": {
        "extracted_manifest_sha256": (
            "c0d76ea897df030f901c7436d2d7ee0cd31591db54c3c6c311d79a68fa138085"
        ),
        "archive_sha256": (
            "20b08cfbc0b765bb066135a4c8d99367fb4f0d5c500a63b725e258dcb91b7005"
        ),
        "archive_bytes": "6362514",
        "dataset_receipt_sha256": (
            "0c488602053f3b4fe0ecc93dfb81ff972bacb2907b8740ad714c539ca7584b44"
        ),
    },
    "0.3": {
        "extracted_manifest_sha256": (
            "aeb94da1177ca732b127574116d7624b131113ffc7f6f8e612b0bb2dab31d5f3"
        ),
        "archive_sha256": (
            "4aad6e31047a356d40e8c315916c3fe35a77911024136d69868b39b16f8ccf33"
        ),
        "archive_bytes": "19134337",
        "dataset_receipt_sha256": (
            "56b4e5b1d028a61ea1ef4cfe31f8a435ce5f5687e5d523de6e613fe807a7f394"
        ),
    },
}

ENVIRONMENT_KEYS = (
    "schema",
    "lifecycle_state",
    "warning",
    "started_at_utc",
    "harness_revision",
    "upstream_commit",
    "upstream_archive_url",
    "upstream_archive_sha256",
    "upstream_archive_bytes",
    "expected_output_sha256",
    "runner_image",
    "runner_image_id",
    "runner_image_revision",
    "scale_factor",
    "extracted_manifest_sha256",
    "archive_sha256",
    "archive_bytes",
    "dataset_receipt_sha256",
    "warmup_iterations",
    "measurement_iterations",
    "worker_threads",
    "query_order",
    "timing_boundary",
    "cell_timeout_ms",
    "cpu_model",
    "cpu_model_scope",
    "cpu_limit",
    "memory_limit_bytes",
    "resource_limit_scope",
    "resource_components",
    "docker_engine_version",
    "container_arch",
)

VALIDATION_KEYS = (
    "schema",
    "status",
    "warning",
    "system",
    "threads",
    "scale_factor",
    "measurement_iterations",
    "queries_per_iteration",
    "observation_count",
    "oracle_sha256",
    "oracle_bytes",
    "oracle_counts_sha256",
)

COMPLETE_KEYS = (
    "schema",
    "status",
    "warning",
    "completed_at_utc",
    "harness_revision",
    "runner_image_id",
    "environment_file",
    "environment_sha256",
    "validation_file",
    "validation_sha256",
    "oracle_file",
    "oracle_sha256",
    "watchdog_file",
    "watchdog_sha256",
)


class ValidationError(RuntimeError):
    """The completed output bundle violates its evidence contract."""


def positive_integer(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be a positive integer") from error
    if parsed <= 0 or str(parsed) != value:
        raise argparse.ArgumentTypeError("must be a canonical positive integer")
    return parsed


def validate_static_pins() -> None:
    for scale, dataset in DATASETS.items():
        for key in (
            "extracted_manifest_sha256",
            "archive_sha256",
            "dataset_receipt_sha256",
        ):
            value = dataset[key]
            if value != "not-applicable" and SHA256_PATTERN.fullmatch(value) is None:
                raise ValidationError(
                    f"internal {scale} {key} pin is not a canonical SHA-256 digest"
                )


def load_raw_validator() -> ModuleType:
    path = Path(__file__).with_name("validate-upstream.py")
    specification = importlib.util.spec_from_file_location(
        "grust_validate_upstream", path
    )
    if specification is None or specification.loader is None:
        raise ValidationError(f"cannot load raw-result validator: {path}")
    module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(module)
    return module


def sha256_bytes(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def read_regular_file(path: Path, description: str) -> bytes:
    if path.is_symlink() or not path.is_file():
        raise ValidationError(f"{description} is not a regular non-symlink file: {path}")
    try:
        return path.read_bytes()
    except OSError as error:
        raise ValidationError(f"cannot read {description} {path}: {error}") from error


def validate_inventory(directory: Path, runs: int) -> None:
    if directory.is_symlink() or not directory.is_dir():
        raise ValidationError(f"output directory is not a regular directory: {directory}")
    expected = {
        "environment.tsv",
        "raw-validation.tsv",
        "complete.tsv",
        "expected-output.csv",
        "watchdog.json",
        *(f"upstream-ladybug-run-{run}.csv" for run in range(1, runs + 1)),
    }
    observed: set[str] = set()
    with os.scandir(directory) as entries:
        for entry in entries:
            observed.add(entry.name)
            if entry.is_symlink():
                raise ValidationError(f"output entry must not be a symlink: {entry.name}")
            if not entry.is_file(follow_symlinks=False):
                raise ValidationError(
                    f"output entry must be a regular file: {entry.name}"
                )
    if observed != expected:
        missing = ", ".join(sorted(expected - observed)) or "none"
        extra = ", ".join(sorted(observed - expected)) or "none"
        raise ValidationError(
            f"output inventory mismatch; missing [{missing}], extra [{extra}]"
        )


def parse_tsv(
    path: Path, description: str, expected_keys: tuple[str, ...]
) -> tuple[dict[str, str], bytes]:
    content = read_regular_file(path, description)
    if not content.endswith(b"\n"):
        raise ValidationError(f"{description} must end with a newline")
    if b"\r" in content:
        raise ValidationError(f"{description} must use LF, not CRLF, line endings")
    try:
        text = content.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValidationError(f"{description} is not valid UTF-8: {error}") from error
    lines = text.splitlines()
    if not lines or lines[0] != "field\tvalue":
        raise ValidationError(f"{description} has an invalid TSV header")
    values: dict[str, str] = {}
    keys: list[str] = []
    for line_number, line in enumerate(lines[1:], start=2):
        fields = line.split("\t")
        if len(fields) != 2:
            raise ValidationError(
                f"{description} line {line_number} has {len(fields)} fields; expected 2"
            )
        key, value = fields
        if KEY_PATTERN.fullmatch(key) is None:
            raise ValidationError(
                f"{description} line {line_number} has invalid key {key!r}"
            )
        if key in values:
            raise ValidationError(f"{description} contains duplicate key {key!r}")
        keys.append(key)
        values[key] = value
    if tuple(keys) != expected_keys:
        raise ValidationError(
            f"{description} keys or key order do not match the canonical schema"
        )
    return values, content


def parse_utc(value: str, description: str) -> datetime:
    if UTC_PATTERN.fullmatch(value) is None:
        raise ValidationError(
            f"{description} must use canonical whole-second UTC form: {value!r}"
        )
    try:
        parsed = datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ")
    except ValueError as error:
        raise ValidationError(f"{description} is not a real UTC timestamp") from error
    return parsed.replace(tzinfo=timezone.utc)


def reject_duplicate_json_keys(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ValidationError(f"watchdog completion record contains duplicate key {key!r}")
        result[key] = value
    return result


def validate_watchdog(arguments: argparse.Namespace) -> tuple[dict[str, object], bytes]:
    raw = read_regular_file(arguments.output_dir / "watchdog.json", "watchdog completion record")
    try:
        record = json.loads(raw, object_pairs_hook=reject_duplicate_json_keys)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValidationError(f"watchdog completion record is invalid JSON: {error}") from error
    if not isinstance(record, dict) or set(record) != WATCHDOG_FIELDS:
        raise ValidationError("watchdog completion record has unexpected fields")
    try:
        canonical = (
            json.dumps(
                record,
                allow_nan=False,
                ensure_ascii=False,
                separators=(",", ":"),
                sort_keys=True,
            )
            + "\n"
        ).encode("utf-8")
    except ValueError as error:
        raise ValidationError("watchdog completion record has a non-finite number") from error
    if raw != canonical:
        raise ValidationError("watchdog completion record is not normalized JSON")
    if record["schema"] != WATCHDOG_SCHEMA or record["status"] != "complete":
        raise ValidationError("watchdog completion record does not attest completion")
    timeout_ms = record["timeout_ms"]
    elapsed_wall_ms = record["elapsed_wall_ms"]
    if timeout_ms != arguments.cell_timeout_ms:
        raise ValidationError("watchdog configured timeout does not match the run identity")
    if (
        not isinstance(elapsed_wall_ms, int)
        or isinstance(elapsed_wall_ms, bool)
        or not 0 <= elapsed_wall_ms <= arguments.cell_timeout_ms
    ):
        raise ValidationError("watchdog completion record has an invalid elapsed wall time")
    if record["child_exit_status"] != 0:
        raise ValidationError("watchdog completion record has a nonzero child exit status")
    container_id = record["container_id"]
    if not isinstance(container_id, str) or SHA256_PATTERN.fullmatch(container_id) is None:
        raise ValidationError("watchdog completion record has no immutable container ID")
    project = record["project"]
    if not isinstance(project, str) or WATCHDOG_PROJECT_PATTERN.fullmatch(project) is None:
        raise ValidationError("watchdog completion record has an invalid project")
    if record["container_name"] != f"{project}-ladybug-cell":
        raise ValidationError("watchdog completion record has the wrong container name")
    if record["service"] != "upstream":
        raise ValidationError("watchdog completion record has the wrong service")
    return record, raw


def compare_values(
    actual: dict[str, str], expected: dict[str, str], description: str
) -> None:
    for key, expected_value in expected.items():
        if actual[key] != expected_value:
            raise ValidationError(
                f"{description} {key!r} mismatch: expected {expected_value!r}, "
                f"received {actual[key]!r}"
            )


def expected_environment(arguments: argparse.Namespace) -> dict[str, str]:
    dataset = DATASETS[arguments.scale]
    return {
        "schema": "grust-lsqb-upstream-identity-v1",
        "lifecycle_state": "prepared",
        "warning": WARNING,
        "started_at_utc": arguments.started_at_utc,
        "harness_revision": arguments.harness_revision,
        "upstream_commit": UPSTREAM_COMMIT,
        "upstream_archive_url": UPSTREAM_ARCHIVE_URL,
        "upstream_archive_sha256": UPSTREAM_ARCHIVE_SHA256,
        "upstream_archive_bytes": UPSTREAM_ARCHIVE_BYTES,
        "expected_output_sha256": EXPECTED_OUTPUT_SHA256,
        "runner_image": RUNNER_IMAGE,
        "runner_image_id": arguments.runner_image_id,
        "runner_image_revision": arguments.harness_revision,
        "scale_factor": arguments.scale,
        **dataset,
        "warmup_iterations": "0",
        "measurement_iterations": str(arguments.runs),
        "worker_threads": str(arguments.threads),
        "query_order": "fixed-q1-through-q9",
        "timing_boundary": "upstream-reported-query-wall-clock",
        "cell_timeout_ms": str(arguments.cell_timeout_ms),
        "cpu_model": arguments.cpu_model,
        "cpu_model_scope": arguments.cpu_model_scope,
        "cpu_limit": str(arguments.cpu_limit),
        "memory_limit_bytes": str(arguments.memory_limit_bytes),
        "resource_limit_scope": "per-container",
        "resource_components": "1",
        "docker_engine_version": arguments.docker_engine_version,
        "container_arch": arguments.container_arch,
    }


def validation_keys(runs: int) -> tuple[str, ...]:
    keys = list(VALIDATION_KEYS)
    for run in range(1, runs + 1):
        keys.extend(
            (
                f"raw_file_{run}",
                f"raw_sha256_{run}",
                f"raw_bytes_{run}",
                f"raw_rows_{run}",
            )
        )
    return tuple(keys)


def canonical_validation(
    arguments: argparse.Namespace, raw_validator: ModuleType
) -> bytes:
    try:
        oracle, oracle_sha256, oracle_bytes = raw_validator.load_oracle(
            arguments.output_dir / "expected-output.csv", arguments.scale
        )
        artifacts: list[tuple[str, str, int]] = []
        for run in range(1, arguments.runs + 1):
            path = arguments.output_dir / f"upstream-ladybug-run-{run}.csv"
            digest, byte_length = raw_validator.validate_raw_file(
                path, run, arguments.scale, arguments.threads, oracle
            )
            artifacts.append((path.name, digest, byte_length))
        return raw_validator.render_validation(
            arguments.scale,
            arguments.runs,
            arguments.threads,
            oracle_sha256,
            oracle_bytes,
            raw_validator.oracle_counts_sha256(arguments.scale, oracle),
            artifacts,
        )
    except (OSError, raw_validator.ValidationError) as error:
        raise ValidationError(f"raw result revalidation failed: {error}") from error


def validate_bundle(arguments: argparse.Namespace) -> None:
    validate_static_pins()
    if arguments.threads != arguments.cpu_limit:
        raise ValidationError("worker threads must exactly equal the CPU limit")
    validate_inventory(arguments.output_dir, arguments.runs)

    environment, environment_bytes = parse_tsv(
        arguments.output_dir / "environment.tsv",
        "environment identity",
        ENVIRONMENT_KEYS,
    )
    compare_values(environment, expected_environment(arguments), "environment identity")

    validation, validation_bytes = parse_tsv(
        arguments.output_dir / "raw-validation.tsv",
        "raw validation receipt",
        validation_keys(arguments.runs),
    )
    raw_validator = load_raw_validator()
    canonical = canonical_validation(arguments, raw_validator)
    if validation_bytes != canonical:
        raise ValidationError(
            "raw validation receipt is not canonical for the read-only raw results"
        )
    if validation["status"] != "pass":
        raise ValidationError("raw validation receipt does not record pass status")

    _watchdog, watchdog_bytes = validate_watchdog(arguments)

    complete, _complete_bytes = parse_tsv(
        arguments.output_dir / "complete.tsv", "completion receipt", COMPLETE_KEYS
    )
    expected_complete = {
        "schema": "grust-lsqb-upstream-complete-v1",
        "status": "complete",
        "warning": WARNING,
        "completed_at_utc": arguments.completed_at_utc,
        "harness_revision": arguments.harness_revision,
        "runner_image_id": arguments.runner_image_id,
        "environment_file": "environment.tsv",
        "environment_sha256": sha256_bytes(environment_bytes),
        "validation_file": "raw-validation.tsv",
        "validation_sha256": sha256_bytes(validation_bytes),
        "oracle_file": "expected-output.csv",
        "oracle_sha256": EXPECTED_OUTPUT_SHA256,
        "watchdog_file": "watchdog.json",
        "watchdog_sha256": sha256_bytes(watchdog_bytes),
    }
    compare_values(complete, expected_complete, "completion receipt")

    started = parse_utc(environment["started_at_utc"], "start timestamp")
    completed = parse_utc(complete["completed_at_utc"], "completion timestamp")
    if completed < started:
        raise ValidationError("completion timestamp precedes the start timestamp")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--harness-revision", required=True)
    parser.add_argument("--runner-image-id", required=True)
    parser.add_argument("--scale", required=True, choices=tuple(DATASETS))
    parser.add_argument("--runs", required=True, type=positive_integer)
    parser.add_argument("--threads", required=True, type=positive_integer)
    parser.add_argument("--started-at-utc", required=True)
    parser.add_argument("--completed-at-utc", required=True)
    parser.add_argument("--cpu-model", required=True)
    parser.add_argument(
        "--cpu-model-scope",
        required=True,
        choices=("execution-container", "explicit-override"),
    )
    parser.add_argument("--cpu-limit", required=True, type=positive_integer)
    parser.add_argument("--memory-limit-bytes", required=True, type=positive_integer)
    parser.add_argument("--cell-timeout-ms", required=True, type=positive_integer)
    parser.add_argument("--docker-engine-version", required=True)
    parser.add_argument("--container-arch", required=True, choices=("arm64", "amd64"))
    arguments = parser.parse_args()
    if REVISION_PATTERN.fullmatch(arguments.harness_revision) is None:
        parser.error("--harness-revision must be exactly 40 lowercase hex characters")
    if IMAGE_ID_PATTERN.fullmatch(arguments.runner_image_id) is None:
        parser.error("--runner-image-id must be sha256 followed by 64 lowercase hex characters")
    for name in ("cpu_model", "docker_engine_version"):
        value = getattr(arguments, name)
        if not value or any(character in value for character in "\t\r\n"):
            parser.error(f"--{name.replace('_', '-')} must be a nonempty one-line value")
    if DOCKER_VERSION_PATTERN.fullmatch(arguments.docker_engine_version) is None:
        parser.error("--docker-engine-version must be a concrete semantic version")
    parse_utc(arguments.started_at_utc, "--started-at-utc")
    parse_utc(arguments.completed_at_utc, "--completed-at-utc")
    return arguments


def main() -> int:
    try:
        arguments = parse_args()
        validate_bundle(arguments)
    except (OSError, ValidationError) as error:
        print(f"validate-upstream-bundle.py: {error}", file=sys.stderr)
        return 1
    print(f"Validated upstream output bundle: {arguments.output_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
