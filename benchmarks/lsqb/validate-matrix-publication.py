#!/usr/bin/env python3
"""Issue or verify a complete LSQB matrix publication receipt."""

from __future__ import annotations

import argparse
import csv
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
from typing import Any

import host_evidence


RECEIPT_NAME = "publication-receipt.json"
MANIFEST_NAME = "evidence-manifest-v2.json"
RECEIPT_SCHEMA = "grust-lsqb-matrix-publication-v1"
WATCHDOG_COMPLETION_SCHEMA = "grust-lsqb-cell-watchdog-completion-v1"
WATCHDOG_TIMEOUT_MARKERS = (
    b'"schema":"grust-lsqb-cell-watchdog-v1"',
    f'"schema":"{WATCHDOG_COMPLETION_SCHEMA}"'.encode(),
)
REVISION_RE = re.compile(r"^[0-9a-f]{40}$")
IMAGE_ID_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
PINNED_IMAGE_RE = re.compile(r"@sha256:[0-9a-f]{64}$")
NONCONCRETE_VALUES = {
    "unknown",
    "not reported",
    "unreported",
    "unresolved",
    "unspecified",
    "none",
    "n/a",
    "not applicable",
    "not used",
}
SUITES = ("baseline", "adversarial")
IMAGE_HEADER = (
    "suite",
    "backend",
    "feature",
    "runner_image",
    "runner_image_id",
    "service_image",
    "service_image_id",
)
EXTERNAL_ATTESTATION_FIELDS = frozenset(
    {
        "architecture",
        "backend",
        "container_id",
        "cpuset_cpus",
        "endpoint_host",
        "endpoint_port",
        "image_id",
        "memory_bytes",
        "memory_swap_bytes",
        "nano_cpus",
        "os",
        "phase",
        "platform_manifest_digest",
        "published_bindings",
        "restart_count",
        "runtime_image_id",
        "running",
        "started_at",
    }
)
EXTERNAL_BINDING_FIELDS = frozenset(
    {"container_port", "host_ip", "host_port", "protocol"}
)
EXTERNAL_STATIC_FIELDS = frozenset({"backend", "mode", "reason"})
CONTAINER_ID_RE = re.compile(r"^[0-9a-f]{64}$")
MATRIX_PROJECT_RE = re.compile(r"^grust-lsqb-matrix-[0-9]+-[0-9]+$")
WATCHDOG_FIELDS = frozenset(
    {
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
)
PLATFORM_VALUE_RE = re.compile(r"^[a-z0-9][a-z0-9._-]{0,63}$")
STARTED_AT_RE = re.compile(
    r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T"
    r"[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]{1,9})?Z$"
)
TIMING_V3_FIELDS = frozenset(
    {
        "boundary",
        "cell_timeout_ms",
        "measurement_iterations",
        "query_kill_reap_timeout_ms",
        "query_order",
        "query_reap_grace_ms",
        "query_recovery_timeout_ms",
        "query_timeout_ms",
        "timeout_enforcement",
        "warmup_iterations",
        "worker_ready_timeout_ms",
    }
)
OBSERVATION_V3_REQUIRED_FIELDS = frozenset(
    {
        "elapsed_ns",
        "iteration",
        "outcome",
        "query_position",
        "recovery_ns",
        "setup_ns",
        "termination",
    }
)
OBSERVATION_V3_FIELDS = OBSERVATION_V3_REQUIRED_FIELDS | {"actual_count", "detail", "plan"}
OBSERVATION_PLAN_CLASSES = {
    "clause-pipeline": {"in-process-reference", "backend-materialize-rust-reference"},
    "count-factorized": {"in-process-reference", "backend-resident-index-rust-count"},
    "sql-row-source": {"backend-row-source-rust-projection"},
    "sql-count": {"backend-native-aggregate"},
    "backend-native": {"backend-native-aggregate"},
}
OBSERVATION_PLAN_BACKENDS = {
    "count-factorized": {"memory", "turso"},
    "sql-count": {"turso", "postgres"},
}
# Durable stores whose worker may build a resident typed index of the store's
# own contents outside the query boundary and run the count plan over it.
RESIDENT_INDEX_BACKENDS = {"turso"}
EXECUTION_PLAN_REGISTRY_SCHEMA = "grust-lsqb-execution-plan-registry-v1"
EXECUTION_PLAN_ENTRY_FIELDS = frozenset(
    {
        "adapter_sha256",
        "backend_query_sha256",
        "execution_class",
        "plan",
        "rust_rows",
        "source_sha256",
    }
)
LOAD_STRATEGY = {
    "memory": "per-observation-worker-reload",
    "turso": "per-observation-worker-reload",
    "ladybug": "per-observation-worker-reload",
    "lancedb": "per-observation-worker-reload",
    "sail": "once-worker-attach",
    "postgres": "once-worker-attach",
    "falkor": "once-worker-attach",
    "surreal": "once-worker-attach",
    "pggraph": "once-worker-attach",
    "postgres-pgq": "once-worker-attach",
    "helix": "once-worker-attach",
    "helix-sdk": "once-worker-attach",
    "surreal-sdk": "once-worker-attach",
}
RECOVERY_CONTRACT = {
    "memory": "process-group-absent",
    "turso": "process-group-absent",
    "ladybug": "process-group-absent",
    "lancedb": "process-group-absent",
    "postgres": "postgres-session-absent",
    "pggraph": "postgres-session-absent",
    "postgres-pgq": "postgres-session-absent",
    "falkor": "falkor-server-deadline",
    "sail": "fail-closed",
    "surreal": "fail-closed",
    "helix": "fail-closed",
    "helix-sdk": "fail-closed",
    "surreal-sdk": "fail-closed",
}


class PublicationError(Exception):
    """A publication bundle is incomplete or internally inconsistent."""


def reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise PublicationError(f"JSON contains duplicate key: {key}")
        result[key] = value
    return result


def read_regular_file(path: Path, label: str) -> bytes:
    try:
        metadata = path.lstat()
    except FileNotFoundError as error:
        raise PublicationError(f"missing {label}: {path}") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise PublicationError(f"{label} is not a regular non-symlink file: {path}")

    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        opened = os.fstat(descriptor)
        if not stat.S_ISREG(opened.st_mode):
            raise PublicationError(f"{label} changed while it was opened: {path}")
        chunks: list[bytes] = []
        while True:
            chunk = os.read(descriptor, 1024 * 1024)
            if not chunk:
                break
            chunks.append(chunk)
        return b"".join(chunks)
    finally:
        os.close(descriptor)


def load_json(path: Path, label: str) -> tuple[dict[str, Any], bytes]:
    raw = read_regular_file(path, label)
    try:
        value = json.loads(raw, object_pairs_hook=reject_duplicate_keys)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise PublicationError(f"invalid JSON in {label}: {path}: {error}") from error
    if not isinstance(value, dict):
        raise PublicationError(f"{label} must contain one JSON object: {path}")
    return value, raw


def sha256(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def canonical_json(value: dict[str, Any]) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8")


def normalized_json_line(value: dict[str, Any]) -> str:
    return json.dumps(
        value,
        allow_nan=False,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    ) + "\n"


def require(condition: bool, message: str) -> None:
    if not condition:
        raise PublicationError(message)


def concrete_string(value: Any) -> bool:
    if not isinstance(value, str):
        return False
    normalized = value.strip().lower()
    return (
        normalized != ""
        and normalized not in NONCONCRETE_VALUES
        and not normalized.startswith("intentionally omitted")
    )


def load_manifest(directory: Path) -> tuple[dict[str, Any], str]:
    path = directory / MANIFEST_NAME
    manifest, raw = load_json(path, "canonical evidence manifest")
    require(
        manifest.get("schema") == "grust-lsqb-evidence-manifest-v2",
        "canonical evidence manifest has an unexpected schema",
    )
    backends = manifest.get("backends")
    require(isinstance(backends, list) and len(backends) > 0, "manifest has no backends")
    manifest_execution_plans(manifest)
    requires_host_preflight(manifest)
    return manifest, sha256(raw)


def requires_host_preflight(manifest: dict[str, Any]) -> bool:
    try:
        return host_evidence.required(manifest)
    except ValueError as error:
        raise PublicationError(str(error)) from error


def manifest_backends(manifest: dict[str, Any]) -> list[dict[str, Any]]:
    backends = manifest["backends"]
    result: list[dict[str, Any]] = []
    identifiers: set[str] = set()
    for entry in backends:
        require(isinstance(entry, dict), "manifest backend entry is not an object")
        identifier = entry.get("id")
        require(
            isinstance(identifier, str)
            and identifier
            and re.fullmatch(r"[a-z0-9-]+", identifier) is not None,
            "manifest backend has an invalid id",
        )
        require(identifier not in identifiers, f"duplicate manifest backend: {identifier}")
        identifiers.add(identifier)
        require(
            entry.get("service_contract") in {"none", "configured", "external"},
            f"invalid service contract for {identifier}",
        )
        resource_components = entry.get("resource_components")
        require(
            isinstance(resource_components, int)
            and not isinstance(resource_components, bool)
            and resource_components > 0,
            f"invalid resource component count for {identifier}",
        )
        if entry.get("service_contract") == "external":
            require(
                resource_components == 2,
                f"external service contract must declare two components: {identifier}",
            )
        feature = entry.get("feature")
        require(feature is None or feature == identifier, f"invalid feature for {identifier}")
        result.append(entry)
    return result


def manifest_queries(manifest: dict[str, Any]) -> dict[str, dict[str, Any]]:
    tracks = manifest.get("tracks")
    require(isinstance(tracks, dict), "manifest has no query tracks")
    queries: dict[str, dict[str, Any]] = {}
    for track in SUITES:
        track_entry = tracks.get(track)
        require(isinstance(track_entry, dict), f"manifest has no {track} track")
        track_queries = track_entry.get("queries")
        require(isinstance(track_queries, dict), f"manifest has no {track} queries")
        for query_id, query in track_queries.items():
            require(
                isinstance(query_id, str) and query_id and query_id not in queries,
                f"manifest has a duplicate or invalid query id: {query_id!r}",
            )
            require(isinstance(query, dict), f"manifest query is not an object: {query_id}")
            queries[query_id] = query
    return queries


def manifest_execution_plans(
    manifest: dict[str, Any],
) -> dict[str, dict[str, dict[str, Any]]]:
    """Validate and return the optional, immutable optimized-plan registry."""
    if "execution_plans" not in manifest:
        return {}
    registry = manifest["execution_plans"]
    require(isinstance(registry, dict), "manifest execution plan registry is not an object")
    require(
        set(registry) == {"schema", "entries"}
        and registry.get("schema") == EXECUTION_PLAN_REGISTRY_SCHEMA,
        "manifest execution plan registry has an unexpected schema or fields",
    )
    entries = registry.get("entries")
    require(isinstance(entries, dict) and entries, "manifest execution plan registry is empty")
    backend_ids = {entry["id"] for entry in manifest_backends(manifest)}
    canonical_queries = manifest_queries(manifest)
    require(
        set(entries) <= {"memory", "turso", "postgres"}
        and set(entries) <= backend_ids,
        "manifest execution plan registry has an unsupported backend",
    )
    for backend, backend_entries in entries.items():
        require(
            isinstance(backend_entries, dict) and backend_entries,
            f"manifest execution plan registry has no entries for {backend}",
        )
        for query_id, entry in backend_entries.items():
            require(
                isinstance(query_id, str) and query_id in canonical_queries,
                f"manifest execution plan registry has an unknown query: {backend}/{query_id}",
            )
            require(
                isinstance(entry, dict) and set(entry) == EXECUTION_PLAN_ENTRY_FIELDS,
                f"manifest execution plan registry has wrong fields: {backend}/{query_id}",
            )
            canonical = canonical_queries[query_id]
            require(
                entry.get("source_sha256") == canonical.get("source_sha256")
                and entry.get("adapter_sha256") == canonical.get("adapter_sha256")
                and isinstance(entry.get("source_sha256"), str)
                and re.fullmatch(r"[0-9a-f]{64}", entry["source_sha256"])
                is not None
                and isinstance(entry.get("adapter_sha256"), str)
                and re.fullmatch(r"[0-9a-f]{64}", entry["adapter_sha256"])
                is not None,
                f"manifest execution plan registry query hashes differ: {backend}/{query_id}",
            )
            if backend == "memory":
                require(
                    entry.get("plan") == "count-factorized"
                    and entry.get("execution_class") == "in-process-reference"
                    and entry.get("rust_rows")
                    == {"kind": "not-materialized", "rows": 0}
                    and entry.get("backend_query_sha256") is None,
                    f"invalid count-factorized registry entry: {backend}/{query_id}",
                )
            elif (
                backend in RESIDENT_INDEX_BACKENDS
                and entry.get("plan") == "count-factorized"
            ):
                require(
                    entry.get("execution_class") == "backend-resident-index-rust-count"
                    and entry.get("rust_rows")
                    == {"kind": "not-materialized", "rows": 0}
                    and entry.get("backend_query_sha256") is None,
                    f"invalid resident-index registry entry: {backend}/{query_id}",
                )
            else:
                require(
                    entry.get("plan") == "sql-count"
                    and entry.get("execution_class") == "backend-native-aggregate"
                    and entry.get("rust_rows") is None
                    and isinstance(entry.get("backend_query_sha256"), str)
                    and re.fullmatch(r"[0-9a-f]{64}", entry["backend_query_sha256"])
                    is not None,
                    f"invalid sql-count registry entry: {backend}/{query_id}",
                )
    return entries


def expected_layout(
    manifest: dict[str, Any], scale: str, include_receipt: bool
) -> tuple[set[str], list[str], set[str]]:
    datasets = manifest.get("datasets")
    require(isinstance(datasets, dict) and scale in datasets, f"unsupported scale: {scale}")
    backends = manifest_backends(manifest)
    backend_ids = [entry["id"] for entry in backends]

    artifacts = {"images.tsv", MANIFEST_NAME}
    if requires_host_preflight(manifest):
        artifacts.add(host_evidence.FILENAME)
    artifacts.update(f"matrix-{suite}-sf{scale}.json" for suite in SUITES)
    artifacts.update(
        f"components/{suite}-{backend}-sf{scale}.json"
        for suite in SUITES
        for backend in backend_ids
    )
    watchdogs = {
        f"watchdogs/{suite}-{backend}.json"
        for suite in SUITES
        for backend in backend_ids
    }
    artifacts.update(watchdogs)
    if scale == "example":
        artifacts.add("policy-portable-sfexample.json")
        artifacts.add("watchdogs/policy-portable.json")

    logs = {"logs/build-core.log"}
    logs.update(
        f"logs/build-{entry['id']}.log" for entry in backends if entry.get("feature")
    )
    logs.update(
        f"logs/{suite}-{backend}.log" for suite in SUITES for backend in backend_ids
    )
    logs.update(
        f"logs/{suite}-{entry['id']}-service.log"
        for suite in SUITES
        for entry in backends
        if entry["service_contract"] in {"configured", "external"}
    )
    if scale == "example":
        logs.add("logs/policy-portable.log")

    files = artifacts | logs
    if include_receipt:
        files.add(RECEIPT_NAME)
    return files, sorted(artifacts), {"components", "logs", "watchdogs"}


def scan_output(
    output_directory: Path, expected_files: set[str], expected_directories: set[str]
) -> dict[str, bytes]:
    try:
        root_metadata = output_directory.lstat()
    except FileNotFoundError as error:
        raise PublicationError(f"output directory does not exist: {output_directory}") from error
    if stat.S_ISLNK(root_metadata.st_mode) or not stat.S_ISDIR(root_metadata.st_mode):
        raise PublicationError(
            f"output directory is not a regular non-symlink directory: {output_directory}"
        )

    observed_directories: set[str] = set()
    observed_files: dict[str, bytes] = {}

    def visit(directory: Path, relative: Path) -> None:
        try:
            entries = list(os.scandir(directory))
        except OSError as error:
            raise PublicationError(f"cannot scan output directory: {directory}: {error}") from error
        for entry in entries:
            relative_path = (relative / entry.name).as_posix()
            metadata = entry.stat(follow_symlinks=False)
            if stat.S_ISLNK(metadata.st_mode):
                raise PublicationError(f"publication output contains a symlink: {relative_path}")
            if stat.S_ISDIR(metadata.st_mode):
                observed_directories.add(relative_path)
                visit(Path(entry.path), relative / entry.name)
            elif stat.S_ISREG(metadata.st_mode):
                observed_files[relative_path] = read_regular_file(
                    Path(entry.path), f"output file {relative_path}"
                )
            else:
                raise PublicationError(
                    f"publication output contains a special file: {relative_path}"
                )

    visit(output_directory, Path())
    if observed_directories != expected_directories:
        missing = sorted(expected_directories - observed_directories)
        extra = sorted(observed_directories - expected_directories)
        raise PublicationError(f"output directory set mismatch; missing={missing}, extra={extra}")
    observed_names = set(observed_files)
    if observed_names != expected_files:
        missing = sorted(expected_files - observed_names)
        extra = sorted(observed_names - expected_files)
        raise PublicationError(f"output file set mismatch; missing={missing}, extra={extra}")
    return observed_files


def parse_images(raw: bytes, manifest: dict[str, Any]) -> dict[tuple[str, str], dict[str, str]]:
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise PublicationError(f"images.tsv is not UTF-8: {error}") from error
    rows = list(csv.reader(io.StringIO(text, newline=""), delimiter="\t"))
    require(rows and tuple(rows[0]) == IMAGE_HEADER, "images.tsv has an unexpected header")
    backends = manifest_backends(manifest)
    expected_order = [(suite, entry["id"]) for entry in backends for suite in SUITES]
    require(len(rows) == len(expected_order) + 1, "images.tsv has an unexpected row count")

    parsed: dict[tuple[str, str], dict[str, str]] = {}
    observed_order: list[tuple[str, str]] = []
    catalog = {entry["id"]: entry for entry in backends}
    for fields in rows[1:]:
        require(len(fields) == len(IMAGE_HEADER), "images.tsv contains a malformed row")
        row = dict(zip(IMAGE_HEADER, fields, strict=True))
        require(all(value != "" for value in row.values()), "images.tsv contains an empty field")
        key = (row["suite"], row["backend"])
        require(key not in parsed, f"images.tsv contains a duplicate row: {key}")
        require(key[1] in catalog and key[0] in SUITES, f"images.tsv has unknown cell: {key}")
        entry = catalog[key[1]]
        expected_feature = entry.get("feature") or "core"
        expected_runner = (
            f"grust-lsqb-matrix-{entry['id']}:0.13"
            if entry.get("feature")
            else "grust-lsqb-matrix-core:0.13"
        )
        require(row["feature"] == expected_feature, f"wrong feature in images.tsv for {key}")
        require(row["runner_image"] == expected_runner, f"wrong runner tag for {key}")
        require(IMAGE_ID_RE.fullmatch(row["runner_image_id"]) is not None, f"bad runner ID for {key}")
        service_contract = entry["service_contract"]
        if service_contract == "configured":
            require(
                PINNED_IMAGE_RE.search(row["service_image"]) is not None,
                f"configured service image is not digest-pinned for {key}",
            )
            require(
                IMAGE_ID_RE.fullmatch(row["service_image_id"]) is not None,
                f"bad configured service image ID for {key}",
            )
        elif service_contract == "external":
            no_service = (
                row["service_image"] == "none"
                and row["service_image_id"] == "none"
            )
            pinned_service = (
                PINNED_IMAGE_RE.search(row["service_image"]) is not None
                and IMAGE_ID_RE.fullmatch(row["service_image_id"]) is not None
            )
            require(
                no_service or pinned_service,
                f"external service identity is partial or mutable for {key}",
            )
        else:
            require(
                row["service_image"] == "none" and row["service_image_id"] == "none",
                f"non-service cell claims a service image for {key}",
            )
        parsed[key] = row
        observed_order.append(key)
    require(observed_order == expected_order, "images.tsv rows are not in canonical order")

    by_runner: dict[str, str] = {}
    for row in parsed.values():
        previous = by_runner.setdefault(row["runner_image"], row["runner_image_id"])
        require(previous == row["runner_image_id"], "one runner tag maps to multiple image IDs")
    return parsed


def parse_normalized_jsonl(raw: bytes, label: str) -> list[dict[str, Any]]:
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise PublicationError(f"{label} is not UTF-8: {error}") from error
    require(text != "" and text.endswith("\n"), f"{label} is not normalized JSONL")

    records: list[dict[str, Any]] = []
    for index, line in enumerate(text[:-1].split("\n"), start=1):
        require(line != "", f"{label} contains an empty line")
        try:
            value = json.loads(line, object_pairs_hook=reject_duplicate_keys)
        except json.JSONDecodeError as error:
            raise PublicationError(
                f"invalid JSON on line {index} of {label}: {error}"
            ) from error
        require(isinstance(value, dict), f"{label} line {index} is not a JSON object")
        records.append(value)

    try:
        normalized = "".join(normalized_json_line(record) for record in records)
    except ValueError as error:
        raise PublicationError(f"{label} contains a non-finite JSON number") from error
    require(text == normalized, f"{label} is not normalized JSONL")
    return records


def positive_port(value: Any) -> bool:
    return (
        isinstance(value, int)
        and not isinstance(value, bool)
        and 1 <= value <= 65535
    )


def validate_external_bindings(
    bindings: Any, endpoint_port: int, label: str
) -> None:
    require(
        isinstance(bindings, list) and len(bindings) > 0,
        f"{label} has no published endpoint binding",
    )
    identities: list[tuple[int, str, int, str]] = []
    for index, binding in enumerate(bindings):
        binding_label = f"{label} binding {index}"
        require(isinstance(binding, dict), f"{binding_label} is not an object")
        require(
            set(binding) == EXTERNAL_BINDING_FIELDS,
            f"{binding_label} has unexpected fields",
        )
        container_port = binding["container_port"]
        host_ip = binding["host_ip"]
        host_port = binding["host_port"]
        protocol = binding["protocol"]
        require(positive_port(container_port), f"{binding_label} has an invalid container port")
        require(
            host_port == endpoint_port and positive_port(host_port),
            f"{binding_label} does not match the qualified endpoint port",
        )
        require(protocol == "tcp", f"{binding_label} is not a TCP binding")
        require(
            isinstance(host_ip, str) and host_ip in {"0.0.0.0", "::"},
            f"{binding_label} is not published on an externally reachable host IP",
        )
        identities.append((container_port, host_ip, host_port, protocol))
    require(identities == sorted(identities), f"{label} bindings are not sorted")
    require(len(identities) == len(set(identities)), f"{label} has duplicate bindings")


def validate_external_attestation_record(
    record: dict[str, Any],
    backend: str,
    phase: str,
    image: str,
    image_id: str,
    environment: dict[str, Any],
    label: str,
) -> None:
    require(
        set(record) == EXTERNAL_ATTESTATION_FIELDS,
        f"{label} has unexpected attestation fields",
    )
    require(record["backend"] == backend, f"{label} has the wrong backend")
    require(record["phase"] == phase, f"{label} has the wrong phase")
    require(
        isinstance(record["container_id"], str)
        and CONTAINER_ID_RE.fullmatch(record["container_id"]) is not None,
        f"{label} has an invalid container ID",
    )
    require(
        record["image_id"] == image_id,
        f"{label} image ID does not match images.tsv and the report",
    )
    require(
        isinstance(image, str) and PINNED_IMAGE_RE.search(image) is not None,
        f"{label} cannot bind to an invalid platform image",
    )
    platform_manifest_digest = image.rsplit("@", 1)[1]
    require(
        record["platform_manifest_digest"] == platform_manifest_digest,
        f"{label} platform manifest does not match images.tsv",
    )
    runtime_image_id = record["runtime_image_id"]
    require(
        isinstance(runtime_image_id, str)
        and IMAGE_ID_RE.fullmatch(runtime_image_id) is not None,
        f"{label} has an invalid local runtime image ID",
    )
    require(
        runtime_image_id in {image_id, platform_manifest_digest},
        f"{label} local runtime image ID is neither the config nor platform manifest digest",
    )

    cpu_limit = environment.get("cpu_limit")
    memory_limit = environment.get("memory_limit_bytes")
    require(
        isinstance(cpu_limit, str)
        and re.fullmatch(r"[1-9][0-9]*", cpu_limit) is not None,
        f"{label} cannot bind to an invalid report CPU limit",
    )
    require(
        isinstance(memory_limit, int)
        and not isinstance(memory_limit, bool)
        and memory_limit > 0,
        f"{label} cannot bind to an invalid report memory limit",
    )
    require(
        record["nano_cpus"] == int(cpu_limit) * 1_000_000_000,
        f"{label} CPU limit does not match the report environment",
    )
    require(
        record["memory_bytes"] == memory_limit,
        f"{label} memory limit does not match the report environment",
    )
    require(
        record["memory_swap_bytes"] == memory_limit,
        f"{label} memory+swap limit does not match the report environment",
    )
    require(record["cpuset_cpus"] == "", f"{label} has an unexpected CPU set")

    container_os = environment.get("container_os")
    container_arch = environment.get("container_arch")
    require(
        isinstance(container_os, str)
        and PLATFORM_VALUE_RE.fullmatch(container_os) is not None,
        f"{label} cannot bind to an invalid report container OS",
    )
    require(
        isinstance(container_arch, str)
        and PLATFORM_VALUE_RE.fullmatch(container_arch) is not None,
        f"{label} cannot bind to an invalid report container architecture",
    )
    require(record["os"] == container_os, f"{label} OS does not match the report environment")
    require(
        record["architecture"] == container_arch,
        f"{label} architecture does not match the report environment",
    )
    require(record["running"] is True, f"{label} does not attest a running container")
    require(
        isinstance(record["restart_count"], int)
        and not isinstance(record["restart_count"], bool)
        and record["restart_count"] >= 0,
        f"{label} has an invalid restart count",
    )
    require(
        isinstance(record["started_at"], str)
        and STARTED_AT_RE.fullmatch(record["started_at"]) is not None
        and not record["started_at"].startswith("0001-"),
        f"{label} has an invalid start timestamp",
    )

    endpoint_port = record["endpoint_port"]
    require(
        record["endpoint_host"] == "host.docker.internal",
        f"{label} exposes endpoint data other than the qualified host",
    )
    require(positive_port(endpoint_port), f"{label} has an invalid endpoint port")
    validate_external_bindings(record["published_bindings"], endpoint_port, label)


def validate_external_service_logs(
    files: dict[str, bytes],
    manifest: dict[str, Any],
    images: dict[tuple[str, str], dict[str, str]],
    matrices: dict[str, dict[str, Any]],
) -> None:
    external_backends = [
        entry["id"]
        for entry in manifest_backends(manifest)
        if entry["service_contract"] == "external"
    ]
    qualified_inventory: dict[str, dict[str, Any]] = {}
    for suite in SUITES:
        matrix = matrices[suite]
        environment = matrix.get("environment")
        require(isinstance(environment, dict), f"{suite} matrix has no environment")
        cells = {
            cell["backend"]["name"]: cell
            for cell in matrix["backends"]
        }
        for backend in external_backends:
            relative = f"logs/{suite}-{backend}-service.log"
            label = f"external service log {suite}/{backend}"
            records = parse_normalized_jsonl(files[relative], label)
            image = images[(suite, backend)]
            cell = cells[backend]
            report_image_id = cell["backend"].get("image_id")
            if image["service_image"] == "none":
                require(len(records) == 1, f"{label} must contain exactly one static record")
                require(
                    set(records[0]) == EXTERNAL_STATIC_FIELDS,
                    f"{label} has unexpected static fields",
                )
                if cell.get("setup_outcome") == "unavailable":
                    expected = {
                        "backend": backend,
                        "mode": "unavailable",
                        "reason": "no-qualified-external-docker-service",
                    }
                else:
                    require(
                        cell.get("setup_outcome") == "unsupported",
                        f"{label} has no valid static report outcome",
                    )
                    expected = {
                        "backend": backend,
                        "mode": "unsupported",
                        "reason": "performance.materialization-disallowed",
                    }
                require(records[0] == expected, f"{label} does not match the report outcome")
                continue

            require(
                report_image_id == image["service_image_id"],
                f"{label} cannot bind its image ID to images.tsv and the report",
            )
            require(
                len(records) == 2,
                f"{label} must contain exactly pre-run and post-run attestations",
            )
            for record, phase in zip(records, ("pre-run", "post-run"), strict=True):
                validate_external_attestation_record(
                    record,
                    backend,
                    phase,
                    image["service_image"],
                    image["service_image_id"],
                    environment,
                    label,
                )
            before = {key: value for key, value in records[0].items() if key != "phase"}
            after = {key: value for key, value in records[1].items() if key != "phase"}
            require(
                before == after,
                f"{label} external container changed between pre-run and post-run",
            )
            previous = qualified_inventory.setdefault(backend, before)
            require(
                previous == before,
                f"cross-track external service inventory differs: {backend}",
            )


def positive_integer(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def nonnegative_integer(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0



CELL_TERMINATION_FIELDS = frozenset({"query_id", "phase", "iteration", "reason_code", "detail"})
QUIESCENCE_UNPROVEN = "backend.quiescence-unproven"


def is_terminal_observation(lifecycle: Any, query: Any, phase: str, observation: Any) -> bool:
    terminated = lifecycle.get("terminated") if isinstance(lifecycle, dict) else None
    if not isinstance(terminated, dict) or not isinstance(query, dict):
        return False
    return (
        terminated.get("query_id") == query.get("id")
        and terminated.get("phase") == ("warmup" if phase == "warmups" else "measurement")
        and terminated.get("iteration") == observation.get("iteration")
    )


def validate_cell_termination(cell: dict, terminated: Any, path: str) -> None:
    """A declared early stop: the named observation is the cell's last, and
    every query left short of the sampling contract is an explicit error with
    the same reason code. Undeclared short cells stay invalid."""
    require(
        cell.get("setup_outcome") == "pass"
        and isinstance(terminated, dict)
        and set(terminated) == CELL_TERMINATION_FIELDS,
        f"malformed cell termination: {path}",
    )
    require(
        terminated["reason_code"] == QUIESCENCE_UNPROVEN
        and terminated["phase"] in ("warmup", "measurement")
        and nonnegative_integer(terminated["iteration"])
        and terminated["iteration"] >= 1
        and isinstance(terminated["detail"], str)
        and terminated["detail"] != "",
        f"cell termination has an invalid reason, phase, iteration or detail: {path}",
    )
    queries = cell.get("queries")
    require(isinstance(queries, list), f"terminated cell has no queries: {path}")
    named = [q for q in queries if isinstance(q, dict) and q.get("id") == terminated["query_id"]]
    require(len(named) == 1, f"cell termination names an unknown query: {path}")
    phase_key = "warmups" if terminated["phase"] == "warmup" else "measurements"
    observations = named[0].get(phase_key)
    require(
        isinstance(observations, list)
        and observations
        and isinstance(observations[-1], dict)
        and observations[-1].get("iteration") == terminated["iteration"]
        and observations[-1].get("outcome") == "error"
        and observations[-1].get("detail") == terminated["detail"],
        f"cell termination does not name its terminal error observation: {path}",
    )
    require(
        named[0].get("outcome") == "error"
        and named[0].get("reason_code") == QUIESCENCE_UNPROVEN
        and named[0].get("detail") == terminated["detail"],
        f"terminating query is not an explicit quiescence error: {path}",
    )
    terminal = observations[-1]

    def after(observation: Any) -> bool:
        return isinstance(observation, dict) and (
            observation.get("iteration", 0) > terminated["iteration"]
            or (observation.get("iteration") == terminated["iteration"]
                and observation.get("query_position", 0) > terminal.get("query_position", 0))
        )

    for query in queries:
        if not isinstance(query, dict):
            continue
        warmups = query.get("warmups") or []
        measurements = query.get("measurements") or []
        later = (
            (bool(measurements) or any(after(o) for o in warmups))
            if terminated["phase"] == "warmup"
            else any(after(o) for o in measurements)
        )
        require(not later, f"cell has observations after its declared termination: {path}")
    for query in queries:
        if not isinstance(query, dict) or query.get("reason_code") != QUIESCENCE_UNPROVEN:
            continue
        require(
            query.get("outcome") == "error" and isinstance(query.get("detail"), str) and query["detail"],
            f"quiescence-unproven query is not an explicit error: {path}",
        )

def validate_v3_timeout_contract(report: dict[str, Any], path: str) -> None:
    timing = report.get("timing")
    require(isinstance(timing, dict), f"missing timing contract: {path}")
    require(set(timing) == TIMING_V3_FIELDS, f"wrong schema-v3 timing fields: {path}")
    for field in (
        "measurement_iterations",
        "query_kill_reap_timeout_ms",
        "query_recovery_timeout_ms",
        "query_timeout_ms",
        "worker_ready_timeout_ms",
        "cell_timeout_ms",
    ):
        require(positive_integer(timing.get(field)), f"invalid {field}: {path}")
    require(
        nonnegative_integer(timing.get("warmup_iterations")),
        f"invalid warmup_iterations: {path}",
    )
    require(
        nonnegative_integer(timing.get("query_reap_grace_ms")),
        f"invalid query_reap_grace_ms: {path}",
    )
    require(
        timing.get("timeout_enforcement") == "coordinator-process-group",
        f"matrix lacks hard process-group query enforcement: {path}",
    )
    require(
        timing.get("boundary") == "coordinator-go-to-result-consumed",
        f"wrong query timing boundary: {path}",
    )
    require(timing.get("query_order") == "rotating", f"wrong query order: {path}")
    timeout_ns = timing["query_timeout_ms"] * 1_000_000
    ready_ns = timing["worker_ready_timeout_ms"] * 1_000_000
    term_grace_ns = timing["query_reap_grace_ms"] * 1_000_000

    cells = report.get("backends")
    require(isinstance(cells, list), f"missing backend cells: {path}")
    for cell in cells:
        require(isinstance(cell, dict), f"malformed backend cell: {path}")
        identity = cell.get("backend")
        backend = identity.get("name") if isinstance(identity, dict) else None
        require(isinstance(backend, str), f"backend cell has no identity: {path}")
        lifecycle = cell.get("lifecycle")
        require(
            isinstance(lifecycle, dict)
            and set(lifecycle) in (
                {"load_strategy", "recovery_contract"},
                {"load_strategy", "recovery_contract", "terminated"},
            ),
            f"backend cell has a malformed lifecycle: {path}",
        )
        executed = cell.get("setup_outcome") == "pass"
        if "terminated" in lifecycle:
            validate_cell_termination(cell, lifecycle["terminated"], path)
        expected_load = LOAD_STRATEGY.get(backend) if executed else "not-executed"
        expected_recovery = RECOVERY_CONTRACT.get(backend) if executed else "not-applicable"
        require(expected_load is not None, f"unknown backend lifecycle: {path}")
        require(
            lifecycle.get("load_strategy") == expected_load,
            f"backend load strategy is untruthful: {path}",
        )
        require(
            lifecycle.get("recovery_contract") == expected_recovery,
            f"backend recovery contract is untruthful: {path}",
        )
        queries = cell.get("queries")
        require(isinstance(queries, list), f"backend cell has no queries: {path}")
        for query in queries:
            require(isinstance(query, dict), f"malformed query outcome: {path}")
            for phase in ("warmups", "measurements"):
                observations = query.get(phase)
                require(isinstance(observations, list), f"malformed observation phase: {path}")
                for observation in observations:
                    require(isinstance(observation, dict), f"malformed observation: {path}")
                    require(
                        OBSERVATION_V3_REQUIRED_FIELDS <= set(observation)
                        and set(observation) <= OBSERVATION_V3_FIELDS,
                        f"wrong schema-v3 observation fields: {path}",
                    )
                    # Absence is immutable legacy evidence, not an inferred
                    # plan. An explicit value must name a compatible executor.
                    if "plan" in observation:
                        plan = observation["plan"]
                        require(
                            isinstance(plan, str) and plan in OBSERVATION_PLAN_CLASSES,
                            f"invalid observation plan: {path}",
                        )
                        execution = query.get("execution")
                        execution_class = execution.get("class") if isinstance(execution, dict) else None
                        require(
                            isinstance(execution_class, str)
                            and execution_class in OBSERVATION_PLAN_CLASSES[plan],
                            f"observation plan does not match execution class: {path}",
                        )
                        allowed_backends = OBSERVATION_PLAN_BACKENDS.get(plan)
                        require(
                            allowed_backends is None or backend in allowed_backends,
                            f"observation plan does not match backend: {path}",
                        )
                    for field in ("setup_ns", "elapsed_ns", "recovery_ns"):
                        require(
                            nonnegative_integer(observation.get(field)),
                            f"invalid observation {field}: {path}",
                        )
                    require(
                        observation["setup_ns"] <= ready_ns,
                        f"observation setup exceeds READY timeout: {path}",
                    )
                    termination = observation.get("termination")
                    outcome = observation.get("outcome")
                    require(
                        termination
                        in {
                            "normal-exit",
                            "backend-timeout",
                            "deadline-observed-exit",
                            "deadline-sigterm",
                            "deadline-sigkill",
                        },
                        f"invalid observation termination: {path}",
                    )
                    if is_terminal_observation(lifecycle, query, phase, observation):
                        # The declared terminal observation ended the cell
                        # precisely because recovery could not be proven; it
                        # is an error by declaration, not a derived timeout.
                        require(outcome == "error", f"terminal observation is not an error: {path}")
                        continue
                    if termination == "normal-exit":
                        require(outcome != "timeout", f"normal exit claims timeout: {path}")
                        require(
                            observation["elapsed_ns"] <= timeout_ns,
                            f"normal observation exceeds deadline: {path}",
                        )
                        if outcome == "error":
                            require(
                                lifecycle["recovery_contract"]
                                in {"process-group-absent", "postgres-session-absent"},
                                f"unacknowledged error lacks a recovery proof: {path}",
                            )
                    elif termination == "backend-timeout":
                        require(outcome == "timeout", f"backend timeout outcome differs: {path}")
                        require(
                            observation["elapsed_ns"] <= timeout_ns,
                            f"backend timeout exceeds coordinator deadline: {path}",
                        )
                        require(
                            lifecycle["recovery_contract"]
                            in {
                                "process-group-absent",
                                "postgres-session-absent",
                                "falkor-server-deadline",
                            },
                            f"backend timeout lacks an acknowledged recovery contract: {path}",
                        )
                    else:
                        require(outcome == "timeout", f"deadline termination is not timeout: {path}")
                        require(
                            observation["elapsed_ns"] >= timeout_ns,
                            f"hard timeout predates its configured deadline: {path}",
                        )
                        if termination == "deadline-sigkill":
                            require(
                                observation["recovery_ns"] >= term_grace_ns,
                                f"SIGKILL timeout omits the TERM grace: {path}",
                            )
                        require(
                            lifecycle["recovery_contract"]
                            in {"process-group-absent", "postgres-session-absent"},
                            f"forced timeout lacks a provable backend recovery: {path}",
                        )


def report_identity(
    report: dict[str, Any],
    path: str,
    revision: str,
    scale: str,
    track: str,
    expected_schema: int | None = None,
) -> int:
    suite = report.get("suite")
    environment = report.get("environment")
    dataset = report.get("dataset")
    timing = report.get("timing")
    schema = report.get("schema_version")
    require(schema in {2, 3}, f"wrong schema version: {path}")
    if expected_schema is not None:
        require(schema == expected_schema, f"mixed matrix schema versions: {path}")
    require(isinstance(suite, dict) and suite.get("track") == track, f"wrong suite: {path}")
    require(
        isinstance(environment, dict) and environment.get("grust_revision") == revision,
        f"source revision mismatch: {path}",
    )
    require(
        isinstance(dataset, dict) and dataset.get("scale_factor") == scale,
        f"scale mismatch: {path}",
    )
    cell_timeout_ms = timing.get("cell_timeout_ms") if isinstance(timing, dict) else None
    require(
        isinstance(cell_timeout_ms, int)
        and not isinstance(cell_timeout_ms, bool)
        and cell_timeout_ms > 0,
        f"missing or invalid hard cell watchdog timeout: {path}",
    )
    if schema == 3:
        validate_v3_timeout_contract(report, path)
    return schema


def optimized_plan_entry_matches_query(
    query: dict[str, Any], entry: dict[str, Any] | None
) -> bool:
    if entry is None:
        return False
    execution = query.get("execution")
    return (
        isinstance(execution, dict)
        and query.get("source_sha256") == entry["source_sha256"]
        and query.get("adapter_sha256") == entry["adapter_sha256"]
        and execution.get("class") == entry["execution_class"]
        and execution.get("backend_query_sha256") == entry["backend_query_sha256"]
        and query.get("rust_rows") == entry["rust_rows"]
    )


def validate_report_execution_plans(
    report: dict[str, Any], manifest: dict[str, Any], path: str
) -> None:
    """Bind optimized query shapes and every executed sample to the registry."""
    if report.get("schema_version") != 3:
        return
    registry = manifest_execution_plans(manifest)
    cells = report.get("backends")
    require(isinstance(cells, list), f"matrix has no backend cells: {path}")
    for cell in cells:
        require(isinstance(cell, dict), f"malformed backend cell: {path}")
        identity = cell.get("backend")
        backend = identity.get("name") if isinstance(identity, dict) else None
        require(isinstance(backend, str), f"backend cell has no identity: {path}")
        backend_registry = registry.get(backend, {})
        queries = cell.get("queries")
        require(isinstance(queries, list), f"backend cell has no queries: {path}")
        for query in queries:
            require(isinstance(query, dict), f"malformed query outcome: {path}")
            query_id = query.get("id")
            entry = backend_registry.get(query_id) if isinstance(query_id, str) else None
            matches = optimized_plan_entry_matches_query(query, entry)
            execution = query.get("execution")
            execution_class = execution.get("class") if isinstance(execution, dict) else None
            rust_rows = query.get("rust_rows")
            phases = (query.get("warmups"), query.get("measurements"))
            observations = [
                observation
                for phase in phases
                if isinstance(phase, list)
                for observation in phase
                if isinstance(observation, dict)
            ]
            optimized_plans = {
                observation.get("plan")
                for observation in observations
                if observation.get("plan") in OBSERVATION_PLAN_BACKENDS
            }
            optimized_shape_claimed = (
                isinstance(rust_rows, dict)
                and rust_rows.get("kind") == "not-materialized"
            ) or (
                backend in {"turso", "postgres"}
                and execution_class == "backend-native-aggregate"
            )
            require(
                not optimized_plans or matches,
                f"optimized observation plan is not authorized by the manifest: {path}",
            )
            require(
                not optimized_shape_claimed or matches,
                f"optimized query shape is not authorized by the manifest: {path}",
            )
            if matches and observations:
                required_plan = entry["plan"]
                require(
                    all(observation.get("plan") == required_plan for observation in observations),
                    f"optimized query does not use one plan for every observation: {path}",
                )


def validate_reports(
    output_directory: Path,
    manifest: dict[str, Any],
    revision: str,
    scale: str,
    images: dict[tuple[str, str], dict[str, str]],
) -> tuple[
    dict[str, dict[str, Any]],
    dict[str, dict[str, dict[str, Any]]],
    bool | None,
]:
    backends = manifest_backends(manifest)
    backend_ids = [entry["id"] for entry in backends]
    catalog = {entry["id"]: entry for entry in backends}
    matrices: dict[str, dict[str, Any]] = {}
    components: dict[str, dict[str, dict[str, Any]]] = {suite: {} for suite in SUITES}
    report_schema: int | None = None

    for suite in SUITES:
        matrix_path = output_directory / f"matrix-{suite}-sf{scale}.json"
        matrix, _ = load_json(matrix_path, f"{suite} matrix")
        schema = report_identity(
            matrix, matrix_path.name, revision, scale, suite, report_schema
        )
        if report_schema is None:
            report_schema = schema
        validate_report_execution_plans(matrix, manifest, matrix_path.name)
        require(matrix.get("complete") is True, f"matrix is not complete: {matrix_path.name}")
        require(isinstance(matrix.get("valid"), bool), f"matrix has no validity result: {matrix_path.name}")
        cells = matrix.get("backends")
        require(isinstance(cells, list), f"matrix has no backend cells: {matrix_path.name}")
        observed_backends = [
            cell.get("backend", {}).get("name") if isinstance(cell, dict) else None
            for cell in cells
        ]
        require(observed_backends == backend_ids, f"matrix backend order is not canonical: {matrix_path.name}")

        for index, backend in enumerate(backend_ids):
            relative = f"components/{suite}-{backend}-sf{scale}.json"
            component, _ = load_json(output_directory / relative, f"component {suite}/{backend}")
            report_identity(component, relative, revision, scale, suite, report_schema)
            validate_report_execution_plans(component, manifest, relative)
            require(component.get("complete") is False, f"component claims completeness: {relative}")
            require(isinstance(component.get("valid"), bool), f"component has no validity result: {relative}")
            component_cells = component.get("backends")
            require(
                isinstance(component_cells, list)
                and len(component_cells) == 1
                and isinstance(component_cells[0], dict),
                f"component does not contain exactly one backend: {relative}",
            )
            cell = component_cells[0]
            require(cell.get("backend", {}).get("name") == backend, f"wrong backend: {relative}")
            require(cell == cells[index], f"matrix/component cell mismatch: {relative}")
            for key in component:
                if key not in {"backends", "complete", "valid"}:
                    require(component[key] == matrix.get(key), f"matrix/component identity mismatch: {relative}")

            query_outcomes = cell.get("queries")
            require(
                isinstance(query_outcomes, list)
                and all(isinstance(query, dict) for query in query_outcomes),
                f"backend cell has malformed query outcomes: {relative}",
            )
            require(
                all(
                    query.get("reason_code") != "runner.feature-not-compiled"
                    for query in query_outcomes
                ),
                f"compiled canonical runner reports a missing feature: {relative}",
            )

            backend_identity = cell["backend"]
            image = images[(suite, backend)]
            require(
                backend_identity.get("runner_image") == image["runner_image"]
                and backend_identity.get("runner_image_id") == image["runner_image_id"],
                f"runner image identity mismatch: {relative}",
            )
            service_image = backend_identity.get("image")
            service_image_id = backend_identity.get("image_id")
            expected_components = (
                catalog[backend]["resource_components"]
                if image["service_image"] != "none"
                else 1
            )
            require(
                backend_identity.get("resource_components") == expected_components,
                f"resource component count disagrees with images.tsv: {relative}",
            )
            service_contract = catalog[backend]["service_contract"]
            if service_contract == "configured":
                require(
                    service_image == image["service_image"]
                    and service_image_id == image["service_image_id"],
                    f"service image identity mismatch: {relative}",
                )
            elif service_contract == "external":
                if image["service_image"] == "none":
                    setup_outcome = cell.get("setup_outcome")
                    queries = cell.get("queries")
                    require(
                        setup_outcome in {"unavailable", "unsupported"},
                        f"external service execution or invalid static outcome has no immutable identity: {relative}",
                    )
                    require(
                        backend_identity.get("service_version") is None
                        and service_image is None
                        and service_image_id is None
                        and backend_identity.get("worker_threads") is None,
                        f"nonexecuted external service has partial identity: {relative}",
                    )
                    require(
                        isinstance(queries, list) and len(queries) > 0,
                        f"nonexecuted external service has no query outcomes: {relative}",
                    )
                    if setup_outcome == "unavailable":
                        require(
                            all(
                                isinstance(query, dict)
                                and query.get("reason_code") == "backend.service-unavailable"
                                for query in queries
                            ),
                            f"default-unavailable external service has the wrong reason: {relative}",
                        )
                    else:
                        require(
                            scale != "example"
                            and catalog[backend].get("query_capability") == "materialize",
                            f"external static unsupported outcome is invalid for this scale or capability: {relative}",
                        )
                        require(
                            all(
                                isinstance(query, dict)
                                and query.get("reason_code")
                                == "performance.materialization-disallowed"
                                for query in queries
                            ),
                            f"external static unsupported outcome has the wrong reason: {relative}",
                        )
                else:
                    require(
                        cell.get("setup_outcome") in {"pass", "error"},
                        f"qualified external service has an invalid setup outcome: {relative}",
                    )
                    require(
                        concrete_string(backend_identity.get("service_version")),
                        f"qualified external service has no concrete version: {relative}",
                    )
                    require(
                        service_image == image["service_image"]
                        and service_image_id == image["service_image_id"],
                        f"external service image identity mismatch: {relative}",
                    )
                    worker_threads = backend_identity.get("worker_threads")
                    require(
                        worker_threads is None
                        or (
                            isinstance(worker_threads, int)
                            and not isinstance(worker_threads, bool)
                            and worker_threads > 0
                        ),
                        f"qualified external service has invalid worker threads: {relative}",
                    )
            else:
                require(
                    service_image is None and service_image_id is None,
                    f"non-service component claims a service image: {relative}",
                )
            components[suite][backend] = component
        matrices[suite] = matrix

    for key in ("environment", "dataset", "timing"):
        require(
            matrices["baseline"].get(key) == matrices["adversarial"].get(key),
            f"cross-track {key} differs",
        )

    for backend in backend_ids:
        baseline_identity = components["baseline"][backend]["backends"][0]["backend"]
        adversarial_identity = components["adversarial"][backend]["backends"][0]["backend"]
        require(
            baseline_identity == adversarial_identity,
            f"cross-track backend identity differs: {backend}",
        )

    policy_valid: bool | None = None
    if scale == "example":
        policy_path = output_directory / "policy-portable-sfexample.json"
        policy, _ = load_json(policy_path, "policy report")
        policy_environment = policy.get("environment")
        require(policy.get("schema_version") == 2, "policy report has a wrong schema version")
        require(isinstance(policy.get("valid"), bool), "policy report has no validity result")
        policy_valid = policy["valid"]
        require(policy.get("suite", {}).get("track") == "policy", "policy report has a wrong track")
        require(
            isinstance(policy_environment, dict)
            and policy_environment.get("grust_revision") == revision
            and policy_environment.get("scale_factor") == scale,
            "policy report source or scale identity differs",
        )
        core_rows = [row for row in images.values() if row["feature"] == "core"]
        core_identities = {(row["runner_image"], row["runner_image_id"]) for row in core_rows}
        require(len(core_identities) == 1, "core runner image identity is inconsistent")
        core_image, core_image_id = next(iter(core_identities))
        require(
            policy_environment.get("container_image") == core_image
            and policy_environment.get("container_image_id") == core_image_id,
            "policy report runner image does not match images.tsv",
        )
        postgres_image = images[("baseline", "postgres")]["service_image"]
        require(
            policy_environment.get("postgres_image") == postgres_image,
            "policy report PostgreSQL image does not match images.tsv",
        )
        matrix_environment = matrices["baseline"]["environment"]
        cross_environment = {
            "grust_revision": "grust_revision",
            "container_os": "container_os",
            "container_arch": "container_arch",
            "docker_engine_version": "docker_engine_version",
            "docker_cpus": "cpu_limit",
            "resource_limit_scope": "resource_limit_scope",
            "host_cpu": "cpu_model",
        }
        for policy_key, matrix_key in cross_environment.items():
            require(
                policy_environment.get(policy_key) == matrix_environment.get(matrix_key),
                f"policy/matrix environment mismatch: {policy_key}",
            )
        require(
            policy_environment.get("docker_memory_bytes")
            == str(matrix_environment.get("memory_limit_bytes")),
            "policy/matrix environment mismatch: docker_memory_bytes",
        )
        validate_policy_outcomes(policy, manifest)
    return matrices, components, policy_valid


def validate_watchdog_records(
    files: dict[str, bytes],
    manifest: dict[str, Any],
    scale: str,
    components: dict[str, dict[str, dict[str, Any]]],
    policy_valid: bool | None,
) -> dict[str, Any]:
    expected: list[tuple[str, str]] = [
        (f"watchdogs/{suite}-{entry['id']}.json", f"{suite}-{entry['id']}")
        for suite in SUITES
        for entry in manifest_backends(manifest)
    ]
    if scale == "example":
        require(isinstance(policy_valid, bool), "example bundle has no policy validity")
        expected.append(("watchdogs/policy-portable.json", "policy"))

    projects: set[str] = set()
    timeouts: set[int] = set()
    container_ids: set[str] = set()
    for relative, cell in expected:
        label = f"cell watchdog completion record {cell}"
        records = parse_normalized_jsonl(files[relative], label)
        require(len(records) == 1, f"{label} must contain exactly one record")
        record = records[0]
        require(set(record) == WATCHDOG_FIELDS, f"{label} has unexpected fields")
        require(record["schema"] == WATCHDOG_COMPLETION_SCHEMA, f"{label} has the wrong schema")
        require(record["status"] == "complete", f"{label} is not complete")

        timeout_ms = record["timeout_ms"]
        elapsed_wall_ms = record["elapsed_wall_ms"]
        child_exit_status = record["child_exit_status"]
        require(
            isinstance(timeout_ms, int)
            and not isinstance(timeout_ms, bool)
            and timeout_ms > 0,
            f"{label} has an invalid configured timeout",
        )
        require(
            isinstance(elapsed_wall_ms, int)
            and not isinstance(elapsed_wall_ms, bool)
            and 0 <= elapsed_wall_ms <= timeout_ms,
            f"{label} has an invalid elapsed wall time",
        )
        require(
            isinstance(child_exit_status, int)
            and not isinstance(child_exit_status, bool)
            and child_exit_status in (0, 1),
            f"{label} has an invalid child exit status",
        )
        project = record["project"]
        require(
            isinstance(project, str) and MATRIX_PROJECT_RE.fullmatch(project) is not None,
            f"{label} has an invalid Compose project",
        )
        expected_name = (
            f"{project}-policy-cell"
            if cell == "policy"
            else f"{project}-{cell}-cell"
        )
        require(record["container_name"] == expected_name, f"{label} has the wrong container name")
        require(record["service"] == "benchmark", f"{label} has the wrong Compose service")
        container_id = record["container_id"]
        require(
            isinstance(container_id, str)
            and CONTAINER_ID_RE.fullmatch(container_id) is not None,
            f"{label} has no immutable container ID",
        )
        require(container_id not in container_ids, f"duplicate watchdog container ID: {container_id}")

        if cell != "policy":
            suite, backend = cell.split("-", 1)
            component = components[suite][backend]
            report_timeout = component.get("timing", {}).get("cell_timeout_ms")
            require(
                timeout_ms == report_timeout,
                f"{label} timeout does not match the component report",
            )
            expected_child_exit_status = 0 if component.get("valid") is True else 1
        else:
            expected_child_exit_status = 0 if policy_valid is True else 1
        require(
            child_exit_status == expected_child_exit_status,
            f"{label} child exit status does not match report validity",
        )
        projects.add(project)
        timeouts.add(timeout_ms)
        container_ids.add(container_id)

    require(len(projects) == 1, "cell watchdog records do not share one Compose project")
    require(len(timeouts) == 1, "cell watchdog records do not share one configured timeout")
    return {
        "cell_count": len(expected),
        "project": next(iter(projects)),
        "schema": WATCHDOG_COMPLETION_SCHEMA,
        "timeout_ms": next(iter(timeouts)),
    }


def validate_policy_outcomes(policy: dict[str, Any], manifest: dict[str, Any]) -> None:
    canonical = manifest["policy"]
    suite = policy["suite"]
    environment = policy["environment"]
    require(policy.get("warning") == manifest["warning"], "policy warning differs")
    expected_suite = {
        "name": canonical["suite_name"],
        "track": "policy",
        "source_url": manifest["suite"]["source_url"],
        "source_commit": manifest["suite"]["source_commit"],
        "source_tree": manifest["suite"]["source_tree"],
        "query_tree": manifest["suite"]["query_tree"],
        "example_dataset_tree": canonical["example_dataset_tree"],
        "license": manifest["suite"]["license"],
        "classification": canonical["classification"],
    }
    require(suite == expected_suite, "policy suite identity differs from the manifest")
    require(environment.get("backend") == canonical["environment"]["backend"], "wrong policy backend")
    require(environment.get("repetitions") == 1, "policy report must contain one repetition")
    require(
        environment.get("rust_version") == canonical["environment"]["rust_version"],
        "wrong policy Rust version",
    )
    require(policy.get("policy") == canonical["limits"], "policy limits differ from the manifest")
    graph = policy.get("graph")
    dataset = manifest["datasets"]["example"]
    require(
        isinstance(graph, dict)
        and graph.get("nodes") == dataset["nodes"]
        and graph.get("edges") == dataset["edges"],
        "policy graph size differs from the manifest",
    )
    runs = policy.get("runs")
    require(isinstance(runs, list) and len(runs) == 1, "policy run count differs")
    require(runs[0].get("repetition") == 1, "policy repetition identity differs")
    attacks = runs[0].get("attacks")
    require(isinstance(attacks, list), "policy attacks are not an array")
    require(
        [attack.get("id") for attack in attacks if isinstance(attack, dict)]
        == canonical["attack_order"],
        "policy attack order differs from the manifest",
    )
    all_passed = True
    for attack in attacks:
        require(isinstance(attack, dict), "policy attack is not an object")
        expected = canonical["attacks"][attack["id"]]
        require(attack.get("source_sha256") == expected["source_sha256"], "policy attack source differs")
        require(attack.get("overrides") == expected["overrides"], "policy attack overrides differ")
        require(
            attack.get("expected_rejection") == expected["expected_rejection"],
            "policy expected rejection differs",
        )
        actual = attack.get("actual_rejection")
        elapsed = attack.get("elapsed_ns")
        require(isinstance(actual, str) and actual != "", "policy actual rejection is missing")
        require(isinstance(elapsed, int) and not isinstance(elapsed, bool) and elapsed >= 0, "bad policy timing")
        expected_status = "pass" if actual == expected["expected_rejection"] else "fail"
        require(attack.get("status") == expected_status, "policy attack status is not truthful")
        error = attack.get("error")
        if actual == "accepted":
            require(error is None, "accepted policy attack unexpectedly has an error")
        else:
            require(isinstance(error, str) and error != "", "rejected policy attack has no error")
        all_passed = all_passed and expected_status == "pass"
    require(policy["valid"] == all_passed, "policy validity flag is not derived from its attacks")


def run_validator(command: list[str], label: str) -> None:
    result = subprocess.run(command, check=False, capture_output=True, text=True)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise PublicationError(f"{label} rejected publication evidence: {detail}")


def run_semantic_validators(script_directory: Path, output_directory: Path, scale: str) -> None:
    manifest, _ = load_manifest(output_directory)
    backend_ids = [entry["id"] for entry in manifest_backends(manifest)]
    with tempfile.TemporaryDirectory(prefix="grust-lsqb-publication-validator.") as temporary:
        tools_directory = Path(temporary)
        for name in (
            "merge-reports.sh",
            "output-safety.sh",
            "validate-evidence.sh",
            "validate-policy.sh",
        ):
            source = script_directory / name
            destination = tools_directory / name
            shutil.copyfile(source, destination)
            destination.chmod(0o755)
        shutil.copyfile(output_directory / MANIFEST_NAME, tools_directory / MANIFEST_NAME)
        for suite in SUITES:
            matrix = output_directory / f"matrix-{suite}-sf{scale}.json"
            components = [
                output_directory / f"components/{suite}-{backend}-sf{scale}.json"
                for backend in backend_ids
            ]
            run_validator(
                [
                    str(tools_directory / "validate-evidence.sh"),
                    str(matrix),
                    *map(str, components),
                ],
                f"{suite} evidence validator",
            )
        if scale == "example":
            policy, _ = load_json(
                output_directory / "policy-portable-sfexample.json", "policy report"
            )
            if policy.get("valid") is True:
                run_validator(
                    [
                        str(tools_directory / "validate-policy.sh"),
                        str(output_directory / "policy-portable-sfexample.json"),
                    ],
                    "policy validator",
                )


def validate_repository(repository: Path, revision: str) -> None:
    if repository.is_symlink() or not repository.is_dir():
        raise PublicationError(f"repository is not a regular directory: {repository}")
    head = subprocess.run(
        ["git", "-C", str(repository), "rev-parse", "HEAD"],
        check=False,
        capture_output=True,
        text=True,
    )
    require(head.returncode == 0, f"cannot resolve repository HEAD: {head.stderr.strip()}")
    require(head.stdout.strip() == revision, "repository HEAD differs from publication revision")
    status_result = subprocess.run(
        ["git", "-C", str(repository), "status", "--porcelain=v1", "--untracked-files=normal"],
        check=False,
        capture_output=True,
        text=True,
    )
    require(status_result.returncode == 0, f"cannot inspect repository status: {status_result.stderr.strip()}")
    require(status_result.stdout == "", "publication repository is dirty")


def build_receipt(
    files: dict[str, bytes],
    artifacts: list[str],
    manifest_sha256: str,
    revision: str,
    scale: str,
    backend_count: int,
    suite_valid: dict[str, bool],
    policy_valid: bool | None,
    watchdog: dict[str, Any],
) -> dict[str, Any]:
    inventory = [
        {"path": path, "bytes": len(raw), "sha256": sha256(raw)}
        for path, raw in sorted(files.items())
        if path != RECEIPT_NAME
    ]
    digests = {entry["path"]: entry["sha256"] for entry in inventory}
    return {
        "schema": RECEIPT_SCHEMA,
        "status": "complete",
        "mode": "publication",
        "warning": "These are not LDBC Benchmark Results.",
        "source_revision": revision,
        "scale_factor": scale,
        "suite_order": list(SUITES),
        "backend_count": backend_count,
        "suite_valid": suite_valid,
        "policy_valid": policy_valid,
        "watchdog": watchdog,
        "all_required_outcomes_valid": all(suite_valid.values())
        and (policy_valid is None or policy_valid),
        "evidence_manifest_sha256": manifest_sha256,
        "artifact_sha256": {path: digests[path] for path in artifacts},
        "output_file_count": len(inventory),
        "output_bytes": sum(entry["bytes"] for entry in inventory),
        "output_inventory": inventory,
    }


def write_atomic(path: Path, content: bytes, label: str) -> None:
    if path.exists() or path.is_symlink():
        raise PublicationError(f"refusing to overwrite {label}: {path}")
    try:
        parent_metadata = path.parent.lstat()
    except OSError as error:
        raise PublicationError(f"cannot inspect {label} parent: {path.parent}: {error}") from error
    if stat.S_ISLNK(parent_metadata.st_mode) or not stat.S_ISDIR(parent_metadata.st_mode):
        raise PublicationError(
            f"{label} parent is not a regular non-symlink directory: {path.parent}"
        )
    parent_identity = (parent_metadata.st_dev, parent_metadata.st_ino)
    temporary_name: str | None = None
    installed_identity: tuple[int, int] | None = None
    complete = False
    try:
        with tempfile.NamedTemporaryFile(
            mode="wb", prefix=f".{path.name}.", dir=path.parent, delete=False
        ) as temporary:
            temporary_name = temporary.name
            temporary.write(content)
            temporary.flush()
            os.fsync(temporary.fileno())
        os.chmod(temporary_name, 0o644)
        current_parent = path.parent.lstat()
        if (
            stat.S_ISLNK(current_parent.st_mode)
            or not stat.S_ISDIR(current_parent.st_mode)
            or (current_parent.st_dev, current_parent.st_ino) != parent_identity
        ):
            raise PublicationError(f"{label} parent was replaced during creation: {path.parent}")
        temporary_metadata = os.lstat(temporary_name)
        installed_identity = (temporary_metadata.st_dev, temporary_metadata.st_ino)
        try:
            os.link(temporary_name, path, follow_symlinks=False)
        except FileExistsError as error:
            raise PublicationError(f"refusing to overwrite {label}: {path}") from error
        installed_metadata = path.lstat()
        current_parent = path.parent.lstat()
        if (
            stat.S_ISLNK(installed_metadata.st_mode)
            or not stat.S_ISREG(installed_metadata.st_mode)
            or (installed_metadata.st_dev, installed_metadata.st_ino) != installed_identity
            or stat.S_ISLNK(current_parent.st_mode)
            or not stat.S_ISDIR(current_parent.st_mode)
            or (current_parent.st_dev, current_parent.st_ino) != parent_identity
        ):
            raise PublicationError(f"{label} was replaced during atomic installation: {path}")
        os.unlink(temporary_name)
        temporary_name = None
        directory_fd = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
        complete = True
    finally:
        if temporary_name is not None:
            try:
                os.unlink(temporary_name)
            except FileNotFoundError:
                pass
        if not complete and installed_identity is not None:
            try:
                installed_metadata = path.lstat()
                if (
                    stat.S_ISREG(installed_metadata.st_mode)
                    and (installed_metadata.st_dev, installed_metadata.st_ino)
                    == installed_identity
                ):
                    path.unlink()
            except FileNotFoundError:
                pass


def remove_if_exact(path: Path, content: bytes) -> None:
    """Remove only the regular file this invocation atomically installed."""
    try:
        if read_regular_file(path, "incomplete publication output") != content:
            return
        path.unlink()
        directory_fd = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    except (OSError, PublicationError):
        return


def inspect_bundle(
    script_directory: Path,
    output_directory: Path,
    revision: str,
    scale: str,
    include_receipt: bool,
    semantic: bool = True,
) -> tuple[dict[str, Any], dict[str, bytes]]:
    require(REVISION_RE.fullmatch(revision) is not None, "source revision is not a clean 40-hex commit")
    manifest, manifest_digest = load_manifest(output_directory)
    expected_files, artifacts, expected_directories = expected_layout(
        manifest, scale, include_receipt
    )
    files = scan_output(output_directory, expected_files, expected_directories)
    if requires_host_preflight(manifest):
        try:
            host_evidence.validate_record(files[host_evidence.FILENAME])
        except ValueError as error:
            raise PublicationError(str(error)) from error
    require(
        all(
            not any(marker in raw for marker in WATCHDOG_TIMEOUT_MARKERS)
            for path, raw in files.items()
            if path.startswith("logs/")
        ),
        "hard cell watchdog timeout logs are not publishable",
    )
    images = parse_images(files["images.tsv"], manifest)
    matrices, components, policy_valid = validate_reports(
        output_directory, manifest, revision, scale, images
    )
    validate_external_service_logs(files, manifest, images, matrices)
    watchdog = validate_watchdog_records(
        files, manifest, scale, components, policy_valid
    )
    if semantic:
        run_semantic_validators(script_directory, output_directory, scale)
    receipt = build_receipt(
        files,
        artifacts,
        manifest_digest,
        revision,
        scale,
        len(manifest_backends(manifest)),
        {suite: matrices[suite]["valid"] for suite in SUITES},
        policy_valid,
        watchdog,
    )
    return receipt, files


def issue_receipt(arguments: argparse.Namespace, script_directory: Path) -> Path:
    revision = arguments.revision
    require(REVISION_RE.fullmatch(revision) is not None, "source revision is not a clean 40-hex commit")
    validate_repository(arguments.repository, revision)
    _, manifest_raw = load_json(
        script_directory / MANIFEST_NAME, "source canonical evidence manifest"
    )
    bundled_manifest = arguments.output_dir / MANIFEST_NAME
    receipt_path = arguments.output_dir / RECEIPT_NAME
    receipt_raw: bytes | None = None
    manifest_written = False
    receipt_written = False
    try:
        write_atomic(bundled_manifest, manifest_raw, "bundled evidence manifest")
        manifest_written = True
        receipt, _ = inspect_bundle(
            script_directory,
            arguments.output_dir,
            revision,
            arguments.scale,
            include_receipt=False,
        )
        require(
            receipt["evidence_manifest_sha256"] == sha256(manifest_raw),
            "bundled evidence manifest differs from the source manifest",
        )
        validate_repository(arguments.repository, revision)
        receipt_raw = canonical_json(receipt)
        write_atomic(receipt_path, receipt_raw, "publication receipt")
        receipt_written = True
        validate_repository(arguments.repository, revision)
        verify_receipt(arguments.output_dir, script_directory)
    except BaseException:
        if receipt_written and receipt_raw is not None:
            remove_if_exact(receipt_path, receipt_raw)
        if manifest_written:
            remove_if_exact(bundled_manifest, manifest_raw)
        raise
    return receipt_path


def verify_receipt(output_directory: Path, script_directory: Path) -> Path:
    receipt_path = output_directory / RECEIPT_NAME
    observed, raw = load_json(receipt_path, "publication receipt")
    require(raw == canonical_json(observed), "publication receipt is not canonical JSON")
    revision = observed.get("source_revision")
    scale = observed.get("scale_factor")
    require(isinstance(revision, str), "publication receipt has no source revision")
    require(isinstance(scale, str), "publication receipt has no scale factor")
    expected, _ = inspect_bundle(
        script_directory,
        output_directory,
        revision,
        scale,
        include_receipt=True,
        semantic=False,
    )
    require(observed == expected, "publication receipt does not match the output inventory")
    return receipt_path


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    issue = subparsers.add_parser("create", help="validate a clean run and issue its receipt")
    issue.add_argument("--output-dir", required=True, type=Path)
    issue.add_argument("--scale", required=True)
    issue.add_argument("--revision", required=True)
    issue.add_argument("--repository", required=True, type=Path)
    verify = subparsers.add_parser("verify", help="verify an existing publication receipt")
    verify.add_argument("--output-dir", required=True, type=Path)
    return parser.parse_args()


def main() -> int:
    arguments = parse_arguments()
    script_directory = Path(__file__).resolve().parent
    try:
        if arguments.command == "create":
            receipt_path = issue_receipt(arguments, script_directory)
        else:
            receipt_path = verify_receipt(arguments.output_dir, script_directory)
        digest = sha256(read_regular_file(receipt_path, "publication receipt"))
        print(f"{digest}  {receipt_path}")
    except (OSError, PublicationError) as error:
        print(f"validate-matrix-publication.py: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
