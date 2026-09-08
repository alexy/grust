#!/usr/bin/env python3
"""Mutation tests for the matrix publication receipt."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


sys.dont_write_bytecode = True
SCRIPT_DIRECTORY = Path(__file__).resolve().parent
MODULE_PATH = SCRIPT_DIRECTORY / "validate-matrix-publication.py"
SPEC = importlib.util.spec_from_file_location("matrix_publication", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
PUBLICATION = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PUBLICATION
SPEC.loader.exec_module(PUBLICATION)


def write_json(path: Path, value: dict) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def passing_host_preflight() -> dict:
    return {
        "schema": "grust-host-preflight-v1",
        "startup_screen_passed": True,
        "clean_host_performance_eligible": False,
        "limitation": "startup screen only; ongoing contention monitoring required",
        "samples": [
            {"total_cpu_percent": 20.5, "busy_processes": [],
             "startup_screen_passed": True,
             "observed_at": f"2026-09-05T12:00:0{index}+00:00"}
            for index in range(3)
        ],
    }


def image_id(label: str) -> str:
    return "sha256:" + hashlib.sha256(label.encode()).hexdigest()


def write_jsonl(path: Path, records: list[dict]) -> None:
    path.write_text(
        "".join(PUBLICATION.normalized_json_line(record) for record in records),
        encoding="utf-8",
    )


def read_jsonl(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]


def external_attestations(
    backend: str, service_image: str, service_image_id: str
) -> list[dict]:
    platform_manifest_digest = service_image.rsplit("@", 1)[1]
    stable = {
        "architecture": "arm64",
        "backend": backend,
        "container_id": hashlib.sha256(f"{backend}-container".encode()).hexdigest(),
        "cpuset_cpus": "",
        "endpoint_host": "host.docker.internal",
        "endpoint_port": 15432,
        "image_id": service_image_id,
        "memory_bytes": 6442450944,
        "memory_swap_bytes": 6442450944,
        "nano_cpus": 8_000_000_000,
        "os": "linux",
        "platform_manifest_digest": platform_manifest_digest,
        "published_bindings": [
            {
                "container_port": 5432,
                "host_ip": "0.0.0.0",
                "host_port": 15432,
                "protocol": "tcp",
            }
        ],
        "restart_count": 0,
        "runtime_image_id": platform_manifest_digest,
        "running": True,
        "started_at": "2026-09-04T12:00:00.000000000Z",
    }
    return [{**stable, "phase": phase} for phase in ("pre-run", "post-run")]


def make_repository(root: Path) -> tuple[Path, str]:
    repository = root / "repository"
    repository.mkdir()
    subprocess.run(["git", "init", "-q", str(repository)], check=True)
    (repository / "seed").write_text("fixture\n", encoding="utf-8")
    subprocess.run(["git", "-C", str(repository), "add", "seed"], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(repository),
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-q",
            "-m",
            "fixture",
        ],
        check=True,
    )
    revision = subprocess.run(
        ["git", "-C", str(repository), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    return repository, revision


def make_bundle(root: Path, revision: str, scale: str = "example") -> Path:
    output = root / "publication"
    (output / "components").mkdir(parents=True)
    (output / "logs").mkdir()
    (output / "watchdogs").mkdir()
    manifest = json.loads((SCRIPT_DIRECTORY / "evidence-manifest-v2.json").read_text())
    backends = manifest["backends"]
    runner_rows: list[list[str]] = []
    cells: dict[tuple[str, str], dict] = {}

    for backend_entry in backends:
        backend = backend_entry["id"]
        feature = backend_entry.get("feature") or "core"
        runner = (
            f"grust-lsqb-matrix-{backend}:0.13"
            if backend_entry.get("feature")
            else "grust-lsqb-matrix-core:0.13"
        )
        runner_id = image_id(runner)
        if backend == "cocoindex":
            service_image = "none"
            service_image_id = "none"
            resource_components = 1
            service_version = None
            backend_image = None
            backend_image_id = None
            setup_outcome = "not_applicable"
            setup_detail = "CocoIndex is not a query backend"
            load_ns = None
            reason_code = "adapter.export-only"
        elif backend_entry["service_contract"] == "configured":
            platform = backend_entry["service_identity"]["platforms"]["arm64"]
            service_image = platform["image"]
            service_image_id = platform["config_id"]
            resource_components = 2
            service_version = backend_entry["service_identity"]["version"]
            backend_image = service_image
            backend_image_id = service_image_id
            setup_outcome = "pass"
            setup_detail = None
            load_ns = 1
            reason_code = None
        elif backend_entry["service_contract"] == "external":
            service_image = "none"
            service_image_id = "none"
            resource_components = 1
            service_version = None
            backend_image = None
            backend_image_id = None
            if scale != "example" and backend_entry["query_capability"] == "materialize":
                setup_outcome = "unsupported"
                setup_detail = "downloaded scales disallow backend materialization"
                reason_code = "performance.materialization-disallowed"
            else:
                setup_outcome = "unavailable"
                setup_detail = "no qualified external service was supplied"
                reason_code = "backend.service-unavailable"
            load_ns = None
        else:
            service_image = "none"
            service_image_id = "none"
            resource_components = 1
            service_version = backend_entry.get("runtime_version")
            backend_image = None
            backend_image_id = None
            setup_outcome = "pass"
            setup_detail = None
            load_ns = 1
            reason_code = None
        for suite in PUBLICATION.SUITES:
            runner_rows.append(
                [
                    suite,
                    backend,
                    feature,
                    runner,
                    runner_id,
                    service_image,
                    service_image_id,
                ]
            )
            cells[(suite, backend)] = {
                "backend": {
                    "name": backend,
                    "adapter": backend_entry["adapter"],
                    "adapter_version": backend_entry["adapter_version"],
                    "runner_image": runner,
                    "runner_image_id": runner_id,
                    "resource_components": resource_components,
                    "service_version": service_version,
                    "image": backend_image,
                    "image_id": backend_image_id,
                    "worker_threads": None,
                },
                "lifecycle": {
                    "load_strategy": (
                        PUBLICATION.LOAD_STRATEGY[backend]
                        if setup_outcome == "pass"
                        else "not-executed"
                    ),
                    "recovery_contract": (
                        PUBLICATION.RECOVERY_CONTRACT[backend]
                        if setup_outcome == "pass"
                        else "not-applicable"
                    ),
                },
                "setup_outcome": setup_outcome,
                "setup_detail": setup_detail,
                "load_ns": load_ns,
                "queries": (
                    [{"reason_code": reason_code, "warmups": [], "measurements": []}]
                    if backend_entry["service_contract"] == "external"
                    else []
                ),
            }

    lines = ["\t".join(PUBLICATION.IMAGE_HEADER)]
    lines.extend("\t".join(row) for row in runner_rows)
    (output / "images.tsv").write_text("\n".join(lines) + "\n", encoding="utf-8")

    environment = {
        "grust_revision": revision,
        "container_os": "linux",
        "container_arch": "arm64",
        "docker_engine_version": "fixture-engine",
        "cpu_model": "Fixture CPU",
        "cpu_limit": "8",
        "memory_limit_bytes": 6442450944,
        "resource_limit_scope": "per-container",
    }
    dataset = {"scale_factor": scale, "fixture": True}
    timing = {
        "warmup_iterations": 0,
        "measurement_iterations": 1,
        "query_timeout_ms": 30_000,
        "worker_ready_timeout_ms": 1_200_000,
        "query_reap_grace_ms": 1_000,
        "query_kill_reap_timeout_ms": 5_000,
        "query_recovery_timeout_ms": 10_000,
        "cell_timeout_ms": 3_600_000,
        "timeout_enforcement": "coordinator-process-group",
        "query_order": "rotating",
        "boundary": "coordinator-go-to-result-consumed",
    }
    backend_ids = [entry["id"] for entry in backends]
    for suite in PUBLICATION.SUITES:
        shared = {
            "schema_version": 3,
            "warning": "These are not LDBC Benchmark Results.",
            "experiment_id": f"lsqb-{suite}-sf{scale}",
            "suite": {"track": suite},
            "environment": environment,
            "dataset": dataset,
            "timing": timing,
        }
        matrix = {
            **shared,
            "backends": [cells[(suite, backend)] for backend in backend_ids],
            "complete": True,
            "valid": True,
        }
        write_json(output / f"matrix-{suite}-sf{scale}.json", matrix)
        for backend in backend_ids:
            component = {
                **shared,
                "backends": [cells[(suite, backend)]],
                "complete": False,
                "valid": True,
            }
            write_json(
                output / "components" / f"{suite}-{backend}-sf{scale}.json",
                component,
            )

    core_runner = "grust-lsqb-matrix-core:0.13"
    postgres_entry = next(entry for entry in backends if entry["id"] == "postgres")
    postgres_image = postgres_entry["service_identity"]["platforms"]["arm64"]["image"]
    canonical_policy = manifest["policy"]
    policy = {
        "schema_version": 2,
        "warning": "These are not LDBC Benchmark Results.",
        "suite": {
            "name": canonical_policy["suite_name"],
            "track": "policy",
            "source_url": manifest["suite"]["source_url"],
            "source_commit": manifest["suite"]["source_commit"],
            "source_tree": manifest["suite"]["source_tree"],
            "query_tree": manifest["suite"]["query_tree"],
            "example_dataset_tree": canonical_policy["example_dataset_tree"],
            "license": manifest["suite"]["license"],
            "classification": canonical_policy["classification"],
        },
        "environment": {
            "grust_revision": revision,
            "backend": "portable-policy",
            "scale_factor": "example",
            "repetitions": 1,
            "rust_version": canonical_policy["environment"]["rust_version"],
            "container_image": core_runner,
            "container_image_id": image_id(core_runner),
            "container_os": "linux",
            "container_arch": "arm64",
            "docker_engine_version": "fixture-engine",
            "docker_cpus": "8",
            "docker_memory_bytes": "6442450944",
            "resource_limit_scope": "per-container",
            "postgres_image": postgres_image,
            "host_cpu": "Fixture CPU",
        },
        "graph": {
            "nodes": manifest["datasets"]["example"]["nodes"],
            "edges": manifest["datasets"]["example"]["edges"],
        },
        "policy": canonical_policy["limits"],
        "runs": [
            {
                "repetition": 1,
                "attacks": [
                    {
                        "id": attack_id,
                        "source_sha256": canonical_policy["attacks"][attack_id][
                            "source_sha256"
                        ],
                        "overrides": canonical_policy["attacks"][attack_id]["overrides"],
                        "expected_rejection": canonical_policy["attacks"][attack_id][
                            "expected_rejection"
                        ],
                        "actual_rejection": canonical_policy["attacks"][attack_id][
                            "expected_rejection"
                        ],
                        "elapsed_ns": 1,
                        "status": "pass",
                        "error": "fixture rejection evidence",
                    }
                    for attack_id in canonical_policy["attack_order"]
                ],
            }
        ],
        "valid": True,
    }
    if scale == "example":
        write_json(output / "policy-portable-sfexample.json", policy)

    watchdog_project = "grust-lsqb-matrix-123-456"
    for suite in PUBLICATION.SUITES:
        for backend in backend_ids:
            write_jsonl(
                output / "watchdogs" / f"{suite}-{backend}.json",
                [
                    {
                        "child_exit_status": 0,
                        "container_id": hashlib.sha256(
                            f"{suite}-{backend}-watchdog".encode()
                        ).hexdigest(),
                        "container_name": (
                            f"{watchdog_project}-{suite}-{backend}-cell"
                        ),
                        "elapsed_wall_ms": 100,
                        "project": watchdog_project,
                        "schema": PUBLICATION.WATCHDOG_COMPLETION_SCHEMA,
                        "service": "benchmark",
                        "status": "complete",
                        "timeout_ms": timing["cell_timeout_ms"],
                    }
                ],
            )
    if scale == "example":
        write_jsonl(
            output / "watchdogs" / "policy-portable.json",
            [
                {
                    "child_exit_status": 0 if policy["valid"] else 1,
                    "container_id": hashlib.sha256(b"policy-watchdog").hexdigest(),
                    "container_name": f"{watchdog_project}-policy-cell",
                    "elapsed_wall_ms": 100,
                    "project": watchdog_project,
                    "schema": PUBLICATION.WATCHDOG_COMPLETION_SCHEMA,
                    "service": "benchmark",
                    "status": "complete",
                    "timeout_ms": timing["cell_timeout_ms"],
                }
            ],
        )

    expected_files, artifacts, _ = PUBLICATION.expected_layout(
        manifest, scale, include_receipt=False
    )
    host_required = PUBLICATION.requires_host_preflight(manifest)
    assert len(artifacts) == (54 if scale == "example" else 52) + host_required
    assert len(expected_files) == (102 if scale == "example" else 99) + host_required
    if host_required:
        write_json(output / "host-preflight.json", passing_host_preflight())
    for relative in expected_files:
        path = output / relative
        if relative.startswith("logs/"):
            path.write_bytes(b"")
    for backend_entry in backends:
        if backend_entry["service_contract"] != "external":
            continue
        backend = backend_entry["id"]
        for suite in PUBLICATION.SUITES:
            setup_outcome = cells[(suite, backend)]["setup_outcome"]
            if setup_outcome == "unavailable":
                record = {
                    "backend": backend,
                    "mode": "unavailable",
                    "reason": "no-qualified-external-docker-service",
                }
            else:
                assert setup_outcome == "unsupported"
                record = {
                    "backend": backend,
                    "mode": "unsupported",
                    "reason": "performance.materialization-disallowed",
                }
            write_jsonl(output / "logs" / f"{suite}-{backend}-service.log", [record])
    return output


def qualify_external_backend(
    output: Path, backend: str = "sail", setup_outcome: str = "pass"
) -> None:
    service_image = (
        "registry.example/grust-external:1@sha256:" + "b" * 64
    )
    service_image_id = "sha256:" + "c" * 64
    rows = [line.split("\t") for line in (output / "images.tsv").read_text().splitlines()]
    for row in rows[1:]:
        if row[1] == backend:
            row[5] = service_image
            row[6] = service_image_id
    (output / "images.tsv").write_text(
        "\n".join("\t".join(row) for row in rows) + "\n", encoding="utf-8"
    )

    for suite in PUBLICATION.SUITES:
        matrix_path = output / f"matrix-{suite}-sfexample.json"
        component_path = output / "components" / f"{suite}-{backend}-sfexample.json"
        matrix = json.loads(matrix_path.read_text())
        component = json.loads(component_path.read_text())
        cell = next(
            candidate for candidate in matrix["backends"]
            if candidate["backend"]["name"] == backend
        )
        for target in (cell, component["backends"][0]):
            target["backend"].update(
                {
                    "resource_components": 2,
                    "service_version": "fixture-service-1.0",
                    "image": service_image,
                    "image_id": service_image_id,
                    "worker_threads": 8,
                }
            )
            target["setup_outcome"] = setup_outcome
            target["setup_detail"] = (
                None if setup_outcome == "pass" else "qualified external service failed"
            )
            target["load_ns"] = 1 if setup_outcome == "pass" else None
            target["lifecycle"] = {
                "load_strategy": (
                    PUBLICATION.LOAD_STRATEGY[backend]
                    if setup_outcome == "pass"
                    else "not-executed"
                ),
                "recovery_contract": (
                    PUBLICATION.RECOVERY_CONTRACT[backend]
                    if setup_outcome == "pass"
                    else "not-applicable"
                ),
            }
        valid = setup_outcome != "error"
        matrix["valid"] = valid
        component["valid"] = valid
        write_json(matrix_path, matrix)
        write_json(component_path, component)
        watchdog_path = output / "watchdogs" / f"{suite}-{backend}.json"
        watchdog = read_jsonl(watchdog_path)[0]
        watchdog["child_exit_status"] = 0 if valid else 1
        write_jsonl(watchdog_path, [watchdog])
        write_jsonl(
            output / "logs" / f"{suite}-{backend}-service.log",
            external_attestations(backend, service_image, service_image_id),
        )


def make_pass_without_external_identity(output: Path, backend: str = "sail") -> None:
    for suite in PUBLICATION.SUITES:
        matrix_path = output / f"matrix-{suite}-sfexample.json"
        component_path = output / "components" / f"{suite}-{backend}-sfexample.json"
        matrix = json.loads(matrix_path.read_text())
        component = json.loads(component_path.read_text())
        cell = next(
            candidate for candidate in matrix["backends"]
            if candidate["backend"]["name"] == backend
        )
        for target in (cell, component["backends"][0]):
            target["setup_outcome"] = "pass"
            target["setup_detail"] = None
            target["load_ns"] = 1
            target["lifecycle"] = {
                "load_strategy": PUBLICATION.LOAD_STRATEGY[backend],
                "recovery_contract": PUBLICATION.RECOVERY_CONTRACT[backend],
            }
        write_json(matrix_path, matrix)
        write_json(component_path, component)


def declare_cell(output: Path, suite: str, backend: str, scale: str = "example") -> dict:
    """Turn one measured cell of a bundle into a declared memory-exceeded one.

    The component report goes, its watchdog record gains the container's own
    OOM exit, a declaration takes the component's place, and the matrix loses
    the cell: not complete any more, accounted for instead.
    """
    component_path = output / f"components/{suite}-{backend}-sf{scale}.json"
    component = json.loads(component_path.read_text())
    component_path.unlink()

    watchdog_path = output / "watchdogs" / f"{suite}-{backend}.json"
    record = read_jsonl(watchdog_path)[0]
    record["child_exit_status"] = PUBLICATION.CONTAINER_OOM_EXIT_STATUS
    record["container_termination"] = {
        "exit_code": PUBLICATION.CONTAINER_OOM_EXIT_STATUS,
        "oom_killed": True,
    }
    write_jsonl(watchdog_path, [record])

    identity = component["backends"][0]["backend"]
    declaration = {
        "backend": backend,
        "cell_timeout_ms": component["timing"]["cell_timeout_ms"],
        "component": f"{suite}-{backend}-sf{scale}.json",
        "declared_by": "run-grust.sh",
        "limitation": (
            "The cell container was terminated by its memory limit before the "
            "runner wrote a component report; no query in this cell was observed."
        ),
        "memory_limit_bytes": component["environment"]["memory_limit_bytes"],
        "publication_qualified": False,
        "runner_image": identity["runner_image"],
        "runner_image_id": identity["runner_image_id"],
        "scale": scale,
        "schema": PUBLICATION.TERMINATION_SCHEMA,
        "suite": suite,
        "watchdog": record,
    }
    (output / PUBLICATION.TERMINATION_DIRECTORY).mkdir(exist_ok=True)
    write_json(output / PUBLICATION.TERMINATION_DIRECTORY / f"{suite}-{backend}.json", declaration)

    matrix_path = output / f"matrix-{suite}-sf{scale}.json"
    matrix = json.loads(matrix_path.read_text())
    matrix["backends"] = [
        cell for cell in matrix["backends"] if cell["backend"]["name"] != backend
    ]
    matrix["complete"] = False
    matrix["accounted"] = True
    matrix["declared_terminations"] = [
        {
            "backend": backend,
            "cell_timeout_ms": declaration["cell_timeout_ms"],
            "limitation": declaration["limitation"],
            "memory_limit_bytes": declaration["memory_limit_bytes"],
            "reason_code": PUBLICATION.MEMORY_EXCEEDED_REASON,
            "runner_image": declaration["runner_image"],
            "runner_image_id": declaration["runner_image_id"],
            "scale": scale,
            "suite": suite,
            "watchdog": record,
        }
    ]
    write_json(matrix_path, matrix)
    return declaration


class DeclaredCellPublicationTests(unittest.TestCase):
    """A cell whose container exceeded its memory limit, through the bundle."""

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="grust-declared-test.")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.repository, self.revision = make_repository(self.root)
        self.output = make_bundle(self.root, self.revision)
        self.declaration = declare_cell(self.output, "baseline", "turso")

    def issue(self) -> None:
        arguments = argparse.Namespace(
            revision=self.revision, repository=self.repository,
            output_dir=self.output, scale="example",
        )
        with mock.patch.object(PUBLICATION, "run_semantic_validators"):
            PUBLICATION.issue_receipt(arguments, SCRIPT_DIRECTORY)

    def verify(self) -> None:
        with mock.patch.object(PUBLICATION, "run_semantic_validators"):
            PUBLICATION.verify_receipt(self.output, SCRIPT_DIRECTORY)

    def assert_rejected(self, phrase: str) -> None:
        with self.assertRaisesRegex(PUBLICATION.PublicationError, phrase):
            self.issue()

    def test_declared_bundle_is_accounted_for_and_names_its_cell(self) -> None:
        self.issue()
        self.verify()
        receipt = json.loads((self.output / PUBLICATION.RECEIPT_NAME).read_text())
        self.assertEqual(receipt["status"], "accounted")
        self.assertEqual(
            receipt["declared_terminations"],
            [{"suite": "baseline", "backend": "turso",
              "reason_code": PUBLICATION.MEMORY_EXCEEDED_REASON}],
        )
        inventory = {entry["path"] for entry in receipt["output_inventory"]}
        self.assertIn("terminations/baseline-turso.json", inventory)
        self.assertNotIn("components/baseline-turso-sfexample.json", inventory)
        # The declared cell's watchdog record is still hashed into the receipt.
        self.assertIn("watchdogs/baseline-turso.json", inventory)

    def test_a_declaration_without_its_oom_proof_is_refused(self) -> None:
        path = self.output / PUBLICATION.TERMINATION_DIRECTORY / "baseline-turso.json"
        declaration = json.loads(path.read_text())
        declaration["watchdog"]["container_termination"]["oom_killed"] = False
        write_json(path, declaration)
        self.assert_rejected("does not prove a container memory termination")

    def test_a_declaration_that_claims_publication_is_refused(self) -> None:
        path = self.output / PUBLICATION.TERMINATION_DIRECTORY / "baseline-turso.json"
        declaration = json.loads(path.read_text())
        declaration["publication_qualified"] = True
        write_json(path, declaration)
        self.assert_rejected("claims publication qualification")

    def test_a_watchdog_record_that_differs_from_its_declaration_is_refused(self) -> None:
        path = self.output / "watchdogs" / "baseline-turso.json"
        record = read_jsonl(path)[0]
        record["elapsed_wall_ms"] += 1
        write_jsonl(path, [record])
        self.assert_rejected("differs from its declaration")

    def test_a_declared_cell_may_not_also_have_a_component_report(self) -> None:
        source = self.output / "components/baseline-memory-sfexample.json"
        shutil.copyfile(source, self.output / "components/baseline-turso-sfexample.json")
        self.assert_rejected("output file set mismatch")

    def test_a_matrix_that_hides_the_declaration_is_refused(self) -> None:
        matrix_path = self.output / "matrix-baseline-sfexample.json"
        matrix = json.loads(matrix_path.read_text())
        matrix.pop("declared_terminations")
        matrix.pop("accounted")
        matrix["complete"] = True
        write_json(matrix_path, matrix)
        self.assert_rejected("not accounted for")

    def test_a_declaration_for_another_scale_is_refused(self) -> None:
        path = self.output / PUBLICATION.TERMINATION_DIRECTORY / "baseline-turso.json"
        declaration = json.loads(path.read_text())
        declaration["scale"] = "0.3"
        write_json(path, declaration)
        self.assert_rejected("names another scale")

    def test_an_undeclared_matrix_may_not_claim_declarations(self) -> None:
        matrix_path = self.output / "matrix-adversarial-sfexample.json"
        matrix = json.loads(matrix_path.read_text())
        matrix["accounted"] = True
        write_json(matrix_path, matrix)
        self.assert_rejected("declares a cell the bundle does not")


class MatrixPublicationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="grust-publication-test.")
        self.root = Path(self.temporary.name)
        self.repository, self.revision = make_repository(self.root)
        self.output = make_bundle(self.root, self.revision)
        self.issue(self.output)

    def issue(self, output: Path, scale: str = "example") -> None:
        arguments = argparse.Namespace(
            revision=self.revision,
            repository=self.repository,
            output_dir=output,
            scale=scale,
        )
        with mock.patch.object(PUBLICATION, "run_semantic_validators"):
            PUBLICATION.issue_receipt(arguments, SCRIPT_DIRECTORY)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def verify(self, directory: Path | None = None) -> None:
        with mock.patch.object(PUBLICATION, "run_semantic_validators"):
            PUBLICATION.verify_receipt(directory or self.output, SCRIPT_DIRECTORY)

    def assert_rejected(self, directory: Path, phrase: str) -> None:
        with self.assertRaisesRegex(PUBLICATION.PublicationError, phrase):
            self.verify(directory)

    def clone(self, name: str) -> Path:
        destination = self.root / name
        shutil.copytree(self.output, destination)
        return destination

    def test_clean_bundle_is_canonical_and_verifiable(self) -> None:
        self.verify()
        receipt = json.loads((self.output / PUBLICATION.RECEIPT_NAME).read_text())
        self.assertEqual(receipt["schema"], PUBLICATION.RECEIPT_SCHEMA)
        self.assertEqual(receipt["mode"], "publication")
        self.assertEqual(receipt["status"], "complete")
        self.assertEqual(receipt["source_revision"], self.revision)
        self.assertEqual(receipt["output_file_count"], 103)
        self.assertEqual(len(receipt["artifact_sha256"]), 55)
        self.assertEqual(receipt["suite_valid"], {"baseline": True, "adversarial": True})
        self.assertTrue(receipt["policy_valid"])
        self.assertTrue(receipt["all_required_outcomes_valid"])
        self.assertNotIn("all_outcomes_passed", receipt)
        self.assertEqual(receipt["watchdog"]["cell_count"], 25)
        self.assertEqual(
            receipt["watchdog"]["schema"],
            PUBLICATION.WATCHDOG_COMPLETION_SCHEMA,
        )
        inventory = {entry["path"] for entry in receipt["output_inventory"]}
        for suite in PUBLICATION.SUITES:
            for backend in ("sail", "postgres-pgq", "helix"):
                self.assertIn(f"logs/{suite}-{backend}-service.log", inventory)
                self.assertIn(f"watchdogs/{suite}-{backend}.json", inventory)

    def test_a_failed_undeclared_cell_may_carry_the_containers_own_exit(self) -> None:
        # FalkorDB's declared quiescence termination exits non-zero for a
        # reason that is not memory, so its watchdog record carries
        # `oom_killed: false`. That must be admitted, not refused as an
        # unexpected field, and it must not become a memory declaration.
        fresh = self.root / "undeclared-failure-root"
        fresh.mkdir()
        clone = make_bundle(fresh, self.revision)
        path = clone / "watchdogs" / "adversarial-falkor.json"
        record = read_jsonl(path)[0]
        record["child_exit_status"] = 1
        record["container_termination"] = {"exit_code": 1, "oom_killed": False}
        write_jsonl(path, [record])
        component_path = clone / "components/adversarial-falkor-sfexample.json"
        component = json.loads(component_path.read_text())
        component["valid"] = False
        write_json(component_path, component)
        matrix_path = clone / "matrix-adversarial-sfexample.json"
        matrix = json.loads(matrix_path.read_text())
        matrix["valid"] = False
        write_json(matrix_path, matrix)
        arguments = argparse.Namespace(revision=self.revision, repository=self.repository,
                                       output_dir=clone, scale="example")
        with mock.patch.object(PUBLICATION, "run_semantic_validators"):
            PUBLICATION.issue_receipt(arguments, SCRIPT_DIRECTORY)
        receipt = json.loads((clone / PUBLICATION.RECEIPT_NAME).read_text())
        self.assertNotIn("declared_terminations", receipt)
        self.assertEqual(receipt["status"], "complete")

    def test_isolated_semantic_validator_copies_merge_dependencies(self) -> None:
        def assert_tools_are_complete(command: list[str], _label: str) -> None:
            tools_directory = Path(command[0]).parent
            self.assertTrue((tools_directory / "merge-reports.sh").is_file())
            self.assertTrue((tools_directory / "output-safety.sh").is_file())

        with mock.patch.object(
            PUBLICATION, "run_validator", side_effect=assert_tools_are_complete
        ):
            PUBLICATION.run_semantic_validators(SCRIPT_DIRECTORY, self.output, "example")

    def test_missing_watchdog_completion_record_is_not_publishable(self) -> None:
        case_root = self.root / "missing-watchdog-completion"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        (output / "watchdogs/baseline-memory.json").unlink()

        with self.assertRaisesRegex(PUBLICATION.PublicationError, "output file set mismatch"):
            self.issue(output)

    def test_timeout_watchdog_completion_record_is_not_publishable(self) -> None:
        case_root = self.root / "timeout-watchdog-completion"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        record_path = output / "watchdogs/baseline-memory.json"
        record = read_jsonl(record_path)[0]
        record["status"] = "timeout"
        record["elapsed_wall_ms"] = record["timeout_ms"]
        record["child_exit_status"] = 143
        write_jsonl(record_path, [record])

        with self.assertRaisesRegex(PUBLICATION.PublicationError, "is not complete"):
            self.issue(output)

    def test_watchdog_child_exit_status_must_match_report_validity(self) -> None:
        case_root = self.root / "watchdog-exit-validity"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        record_path = output / "watchdogs/baseline-memory.json"
        record = read_jsonl(record_path)[0]
        record["child_exit_status"] = 1
        write_jsonl(record_path, [record])

        with self.assertRaisesRegex(
            PUBLICATION.PublicationError,
            "child exit status does not match report validity",
        ):
            self.issue(output)

        out_of_range_root = self.root / "watchdog-exit-range"
        out_of_range_root.mkdir()
        out_of_range = make_bundle(out_of_range_root, self.revision)
        out_of_range_path = out_of_range / "watchdogs/baseline-memory.json"
        out_of_range_record = read_jsonl(out_of_range_path)[0]
        out_of_range_record["child_exit_status"] = 7
        write_jsonl(out_of_range_path, [out_of_range_record])

        with self.assertRaisesRegex(PUBLICATION.PublicationError, "invalid child exit status"):
            self.issue(out_of_range)

    def test_watchdog_record_requires_an_immutable_container_id(self) -> None:
        case_root = self.root / "watchdog-container-id"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        record_path = output / "watchdogs/baseline-memory.json"
        record = read_jsonl(record_path)[0]
        record["container_id"] = None
        write_jsonl(record_path, [record])

        with self.assertRaisesRegex(PUBLICATION.PublicationError, "immutable container ID"):
            self.issue(output)

    def test_missing_hard_cell_watchdog_is_not_publishable(self) -> None:
        case_root = self.root / "missing-cell-watchdog"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        matrix_path = output / "matrix-baseline-sfexample.json"
        matrix = json.loads(matrix_path.read_text())
        del matrix["timing"]["cell_timeout_ms"]
        write_json(matrix_path, matrix)

        with self.assertRaisesRegex(
            PUBLICATION.PublicationError, "hard cell watchdog timeout"
        ):
            self.issue(output)

    def test_mixed_v2_v3_matrices_are_not_publishable(self) -> None:
        case_root = self.root / "mixed-v2-v3-matrix"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        matrix_path = output / "matrix-baseline-sfexample.json"
        matrix = json.loads(matrix_path.read_text())
        matrix["schema_version"] = 2
        write_json(matrix_path, matrix)

        with self.assertRaisesRegex(PUBLICATION.PublicationError, "mixed matrix schema versions"):
            self.issue(output)

    def test_missing_query_enforcement_is_not_publishable(self) -> None:
        case_root = self.root / "missing-query-enforcement"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        matrix_path = output / "matrix-baseline-sfexample.json"
        matrix = json.loads(matrix_path.read_text())
        del matrix["timing"]["timeout_enforcement"]
        write_json(matrix_path, matrix)

        with self.assertRaisesRegex(
            PUBLICATION.PublicationError, "schema-v3 timing fields"
        ):
            self.issue(output)

    def test_hard_cell_watchdog_timeout_log_is_not_publishable(self) -> None:
        case_root = self.root / "watchdog-timeout-log"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        run_log = output / "logs/baseline-memory.log"
        run_log.write_text(
            '{"container":"fixture","schema":"grust-lsqb-cell-watchdog-v1",'
            '"status":"timeout","timeout_ms":1}\n',
            encoding="utf-8",
        )

        with self.assertRaisesRegex(
            PUBLICATION.PublicationError, "timeout logs are not publishable"
        ):
            self.issue(output)

    def test_progress_and_heartbeat_lines_remain_publishable(self) -> None:
        case_root = self.root / "progress-log"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        run_log = output / "logs/baseline-memory.log"
        run_log.write_text(
            'grust-lsqb-progress {"event":"load_chunk_complete","chunks":3,'
            '"nodes":20000,"edges":10000,"elapsed_ms":1000}\n'
            "grust-lsqb-progress "
            '{"event":"query_start","backend":"memory","suite":"baseline",'
            '"scale":"example","phase":"measurement","iteration":1,'
            '"iteration_total":1,"query_position":1,"query_total":9,'
            '"query_id":"q1"}\n'
            "grust-lsqb-progress "
            '{"event":"query_ready","backend":"memory","suite":"baseline",'
            '"scale":"example","phase":"measurement","iteration":1,'
            '"iteration_total":1,"query_position":1,"query_total":9,'
            '"query_id":"q1","setup_ns":314}\n'
            "cell-watchdog.py: heartbeat "
            "container=grust-lsqb-matrix-123-456-baseline-memory-cell "
            "elapsed_ms=30000 remaining_ms=3570000\n"
            "grust-lsqb-progress "
            '{"event":"query_finish","backend":"memory","suite":"baseline",'
            '"scale":"example","phase":"measurement","iteration":1,'
            '"iteration_total":1,"query_position":1,"query_total":9,'
            '"query_id":"q1","outcome":"pass","elapsed_ns":42}\n',
            encoding="utf-8",
        )

        self.issue(output)
        self.verify(output)

    def test_atomic_writer_refuses_existing_and_broken_symlink_outputs(self) -> None:
        existing = self.root / "existing-output"
        existing.write_bytes(b"original\n")
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "refusing to overwrite"):
            PUBLICATION.write_atomic(existing, b"replacement\n", "fixture output")
        self.assertEqual(existing.read_bytes(), b"original\n")

        broken = self.root / "broken-output"
        broken.symlink_to(self.root / "missing-target")
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "refusing to overwrite"):
            PUBLICATION.write_atomic(broken, b"replacement\n", "fixture output")
        self.assertTrue(broken.is_symlink())

    def test_atomic_writer_rejects_symlink_parent(self) -> None:
        real_parent = self.root / "real-parent"
        real_parent.mkdir()
        linked_parent = self.root / "linked-parent"
        linked_parent.symlink_to(real_parent, target_is_directory=True)
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "non-symlink directory"):
            PUBLICATION.write_atomic(
                linked_parent / "receipt.json", b"{}\n", "fixture output"
            )
        self.assertFalse((real_parent / "receipt.json").exists())

    def test_downloaded_external_static_outcomes_are_exact(self) -> None:
        case_root = self.root / "downloaded-static"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision, scale="0.1")
        self.issue(output, scale="0.1")
        self.verify(output)

        matrix = json.loads((output / "matrix-baseline-sf0.1.json").read_text())
        by_backend = {cell["backend"]["name"]: cell for cell in matrix["backends"]}
        self.assertEqual(by_backend["sail"]["setup_outcome"], "unavailable")
        for backend in ("postgres-pgq", "helix"):
            self.assertEqual(by_backend[backend]["setup_outcome"], "unsupported")
            self.assertEqual(by_backend[backend]["backend"]["resource_components"], 1)
            self.assertEqual(
                {query["reason_code"] for query in by_backend[backend]["queries"]},
                {"performance.materialization-disallowed"},
            )

        bad_log = self.root / "downloaded-static-wrong-log"
        shutil.copytree(output, bad_log)
        service_log = bad_log / "logs/baseline-postgres-pgq-service.log"
        records = read_jsonl(service_log)
        records[0]["mode"] = "unavailable"
        records[0]["reason"] = "no-qualified-external-docker-service"
        write_jsonl(service_log, records)
        self.assert_rejected(bad_log, "does not match the report outcome")

        bad_root = self.root / "downloaded-static-wrong-reason"
        bad_root.mkdir()
        bad_output = make_bundle(bad_root, self.revision, scale="0.1")
        for suite in PUBLICATION.SUITES:
            matrix_path = bad_output / f"matrix-{suite}-sf0.1.json"
            component_path = (
                bad_output / "components" / f"{suite}-postgres-pgq-sf0.1.json"
            )
            matrix = json.loads(matrix_path.read_text())
            component = json.loads(component_path.read_text())
            next(
                cell
                for cell in matrix["backends"]
                if cell["backend"]["name"] == "postgres-pgq"
            )["queries"][0]["reason_code"] = "backend.service-unavailable"
            component["backends"][0]["queries"][0]["reason_code"] = (
                "backend.service-unavailable"
            )
            write_json(matrix_path, matrix)
            write_json(component_path, component)
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "wrong reason"):
            self.issue(bad_output, scale="0.1")

    def test_default_external_unavailable_reason_is_exact(self) -> None:
        case_root = self.root / "default-unavailable-wrong-reason"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        for suite in PUBLICATION.SUITES:
            matrix_path = output / f"matrix-{suite}-sfexample.json"
            component_path = output / "components" / f"{suite}-sail-sfexample.json"
            matrix = json.loads(matrix_path.read_text())
            component = json.loads(component_path.read_text())
            next(
                cell
                for cell in matrix["backends"]
                if cell["backend"]["name"] == "sail"
            )["queries"][0]["reason_code"] = "backend.setup"
            component["backends"][0]["queries"][0]["reason_code"] = (
                "backend.setup"
            )
            write_json(matrix_path, matrix)
            write_json(component_path, component)
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "wrong reason"):
            self.issue(output)

    def test_compiled_canonical_runner_cannot_claim_missing_feature(self) -> None:
        case_root = self.root / "compiled-runner-feature-gap"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        for suite in PUBLICATION.SUITES:
            matrix_path = output / f"matrix-{suite}-sfexample.json"
            component_path = output / "components" / f"{suite}-ladybug-sfexample.json"
            matrix = json.loads(matrix_path.read_text())
            component = json.loads(component_path.read_text())
            next(
                cell
                for cell in matrix["backends"]
                if cell["backend"]["name"] == "ladybug"
            )["queries"] = [
                {
                    "reason_code": "runner.feature-not-compiled",
                    "warmups": [],
                    "measurements": [],
                }
            ]
            component["backends"][0]["queries"] = [
                {
                    "reason_code": "runner.feature-not-compiled",
                    "warmups": [],
                    "measurements": [],
                }
            ]
            write_json(matrix_path, matrix)
            write_json(component_path, component)

        with self.assertRaisesRegex(
            PUBLICATION.PublicationError,
            "compiled canonical runner reports a missing feature",
        ):
            self.issue(output)

    def test_qualified_external_service_pass_is_publishable(self) -> None:
        case_root = self.root / "external-pass"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        qualify_external_backend(output)
        self.issue(output)
        PUBLICATION.verify_receipt(output, SCRIPT_DIRECTORY)

        ipv6_root = self.root / "external-pass-ipv6-wildcard"
        ipv6_root.mkdir()
        ipv6_output = make_bundle(ipv6_root, self.revision)
        qualify_external_backend(ipv6_output)
        for suite in PUBLICATION.SUITES:
            service_log = ipv6_output / "logs" / f"{suite}-sail-service.log"
            records = read_jsonl(service_log)
            for record in records:
                record["published_bindings"][0]["host_ip"] = "::"
            write_jsonl(service_log, records)
        self.issue(ipv6_output)
        PUBLICATION.verify_receipt(ipv6_output, SCRIPT_DIRECTORY)

    def test_qualified_external_service_failure_is_publishable(self) -> None:
        case_root = self.root / "external-failure"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        qualify_external_backend(output, setup_outcome="error")
        self.issue(output)
        receipt = json.loads((output / PUBLICATION.RECEIPT_NAME).read_text())
        self.assertEqual(receipt["suite_valid"], {"baseline": False, "adversarial": False})
        self.assertFalse(receipt["all_required_outcomes_valid"])

    def test_qualified_external_service_cannot_be_neutral_unavailable(self) -> None:
        case_root = self.root / "external-unavailable"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        qualify_external_backend(output, setup_outcome="unavailable")
        with self.assertRaisesRegex(
            PUBLICATION.PublicationError,
            "qualified external service has an invalid setup outcome",
        ):
            self.issue(output)

    def test_default_external_log_is_one_exact_normalized_record(self) -> None:
        relative = Path("logs/baseline-sail-service.log")

        noncanonical = self.clone("external-static-noncanonical")
        record = read_jsonl(noncanonical / relative)[0]
        noncanonical_record = {
            "reason": record["reason"],
            "mode": record["mode"],
            "backend": record["backend"],
        }
        (noncanonical / relative).write_text(
            json.dumps(noncanonical_record, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
        self.assert_rejected(noncanonical, "not normalized JSONL")

        leaked = self.clone("external-static-secret")
        records = read_jsonl(leaked / relative)
        records[0]["endpoint"] = "postgres://user:super-secret@example.invalid/db"
        write_jsonl(leaked / relative, records)
        self.assert_rejected(leaked, "unexpected static fields")

        extra = self.clone("external-static-extra-line")
        records = read_jsonl(extra / relative)
        records.append(dict(records[0]))
        write_jsonl(extra / relative, records)
        self.assert_rejected(extra, "exactly one static record")

        wrong = self.clone("external-static-wrong-reason")
        records = read_jsonl(wrong / relative)
        records[0]["reason"] = "operator-asserted"
        write_jsonl(wrong / relative, records)
        self.assert_rejected(wrong, "does not match the report outcome")

    def test_qualified_external_log_binds_full_container_inventory(self) -> None:
        case_root = self.root / "external-attestation-base"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        qualify_external_backend(output)
        self.issue(output)
        relative = Path("logs/baseline-sail-service.log")

        mutations = (
            ("image-id", 0, "image_id", image_id("wrong-service"), "image ID"),
            (
                "platform-manifest",
                0,
                "platform_manifest_digest",
                image_id("wrong-platform-manifest"),
                "platform manifest",
            ),
            (
                "runtime-image-malformed",
                0,
                "runtime_image_id",
                "local-image",
                "invalid local runtime image ID",
            ),
            (
                "runtime-image-unrelated",
                0,
                "runtime_image_id",
                image_id("unrelated-runtime-image"),
                "neither the config nor platform manifest",
            ),
            ("cpu", 0, "nano_cpus", 7_000_000_000, "CPU limit"),
            ("memory", 0, "memory_bytes", 1, "memory limit"),
            ("memory-swap", 0, "memory_swap_bytes", 1, "memory\\+swap limit"),
            ("cpuset", 0, "cpuset_cpus", "0-3", "unexpected CPU set"),
            ("os", 0, "os", "windows", "OS does not match"),
            ("architecture", 0, "architecture", "amd64", "architecture does not match"),
            ("not-running", 0, "running", False, "running container"),
            ("bad-start", 0, "started_at", "super-secret", "invalid start timestamp"),
            ("zero-start", 0, "started_at", "0001-01-01T00:00:00Z", "invalid start timestamp"),
            ("bad-endpoint-port", 0, "endpoint_port", 0, "invalid endpoint port"),
            ("container-changed", 1, "container_id", "d" * 64, "changed between"),
            ("restarted", 1, "restart_count", 1, "changed between"),
            (
                "start-changed",
                1,
                "started_at",
                "2026-09-04T12:00:01.000000000Z",
                "changed between",
            ),
            (
                "endpoint-secret",
                0,
                "endpoint_host",
                "user:super-secret@host.docker.internal",
                "qualified host",
            ),
        )
        for name, record_index, field, value, phrase in mutations:
            with self.subTest(name=name):
                mutated = self.root / f"external-attestation-{name}"
                shutil.copytree(output, mutated)
                records = read_jsonl(mutated / relative)
                records[record_index][field] = value
                write_jsonl(mutated / relative, records)
                self.assert_rejected(mutated, phrase)

        unexpected = self.root / "external-attestation-unexpected-field"
        shutil.copytree(output, unexpected)
        records = read_jsonl(unexpected / relative)
        records[0]["endpoint"] = "http://user:super-secret@host.docker.internal:15432"
        write_jsonl(unexpected / relative, records)
        self.assert_rejected(unexpected, "unexpected attestation fields")

        extra = self.root / "external-attestation-extra-line"
        shutil.copytree(output, extra)
        records = read_jsonl(extra / relative)
        records.append(dict(records[-1]))
        write_jsonl(extra / relative, records)
        self.assert_rejected(extra, "exactly pre-run and post-run")

        noncanonical = self.root / "external-attestation-noncanonical"
        shutil.copytree(output, noncanonical)
        records = read_jsonl(noncanonical / relative)
        first = dict(reversed(list(records[0].items())))
        (noncanonical / relative).write_text(
            json.dumps(first, separators=(",", ":"))
            + "\n"
            + PUBLICATION.normalized_json_line(records[1]),
            encoding="utf-8",
        )
        self.assert_rejected(noncanonical, "not normalized JSONL")

        wrong_phase = self.root / "external-attestation-phase-order"
        shutil.copytree(output, wrong_phase)
        records = read_jsonl(wrong_phase / relative)
        records.reverse()
        write_jsonl(wrong_phase / relative, records)
        self.assert_rejected(wrong_phase, "wrong phase")

        unsorted = self.root / "external-attestation-unsorted-bindings"
        shutil.copytree(output, unsorted)
        records = read_jsonl(unsorted / relative)
        for record in records:
            record["published_bindings"] = [
                {
                    "container_port": 6432,
                    "host_ip": "0.0.0.0",
                    "host_port": record["endpoint_port"],
                    "protocol": "tcp",
                },
                *record["published_bindings"],
            ]
        write_jsonl(unsorted / relative, records)
        self.assert_rejected(unsorted, "bindings are not sorted")

        secret_ip = self.root / "external-attestation-secret-binding"
        shutil.copytree(output, secret_ip)
        records = read_jsonl(secret_ip / relative)
        for record in records:
            record["published_bindings"][0]["host_ip"] = "user:super-secret@127.0.0.1"
        write_jsonl(secret_ip / relative, records)
        self.assert_rejected(secret_ip, "externally reachable host IP")

        loopback = self.root / "external-attestation-loopback-binding"
        shutil.copytree(output, loopback)
        records = read_jsonl(loopback / relative)
        for record in records:
            record["published_bindings"][0]["host_ip"] = "127.0.0.1"
        write_jsonl(loopback / relative, records)
        self.assert_rejected(loopback, "externally reachable host IP")

        wrong_port = self.root / "external-attestation-wrong-binding-port"
        shutil.copytree(output, wrong_port)
        records = read_jsonl(wrong_port / relative)
        for record in records:
            record["published_bindings"][0]["host_port"] = 15433
        write_jsonl(wrong_port / relative, records)
        self.assert_rejected(wrong_port, "qualified endpoint port")

        cross_track = self.root / "external-attestation-cross-track-container"
        shutil.copytree(output, cross_track)
        adversarial_log = cross_track / "logs/adversarial-sail-service.log"
        records = read_jsonl(adversarial_log)
        for record in records:
            record["container_id"] = "e" * 64
            record["started_at"] = "2026-09-04T12:01:00.000000000Z"
        write_jsonl(adversarial_log, records)
        self.assert_rejected(cross_track, "cross-track external service inventory")

    def test_qualified_external_log_accepts_legacy_config_runtime_id(self) -> None:
        case_root = self.root / "external-attestation-legacy-runtime-id"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        qualify_external_backend(output)
        service_image_id = "sha256:" + "c" * 64
        for suite in PUBLICATION.SUITES:
            service_log = output / "logs" / f"{suite}-sail-service.log"
            records = read_jsonl(service_log)
            for record in records:
                record["runtime_image_id"] = service_image_id
            write_jsonl(service_log, records)

        self.issue(output)

    def test_partial_or_mutable_external_image_identity_is_rejected(self) -> None:
        partial = self.clone("partial-external-image")
        rows = (partial / "images.tsv").read_text().splitlines()
        for index, line in enumerate(rows):
            fields = line.split("\t")
            if len(fields) == len(PUBLICATION.IMAGE_HEADER) and fields[1] == "sail":
                fields[5] = "registry.example/grust-external:1@sha256:" + "b" * 64
                rows[index] = "\t".join(fields)
                break
        (partial / "images.tsv").write_text("\n".join(rows) + "\n")
        self.assert_rejected(partial, "external service identity is partial or mutable")

        mutable = self.clone("mutable-external-image")
        rows = (mutable / "images.tsv").read_text().splitlines()
        for index, line in enumerate(rows):
            fields = line.split("\t")
            if len(fields) == len(PUBLICATION.IMAGE_HEADER) and fields[1] == "sail":
                fields[5] = "registry.example/grust-external:latest"
                fields[6] = "sha256:" + "c" * 64
                rows[index] = "\t".join(fields)
                break
        (mutable / "images.tsv").write_text("\n".join(rows) + "\n")
        self.assert_rejected(mutable, "external service identity is partial or mutable")

    def test_external_pass_without_identity_is_rejected(self) -> None:
        mutated = self.clone("external-pass-without-identity")
        make_pass_without_external_identity(mutated)
        self.assert_rejected(mutated, "has no immutable identity")

    def test_cross_track_external_service_drift_is_rejected(self) -> None:
        case_root = self.root / "external-drift"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        qualify_external_backend(output)
        replacement = "sha256:" + "d" * 64
        matrix_path = output / "matrix-adversarial-sfexample.json"
        component_path = output / "components/adversarial-sail-sfexample.json"
        matrix = json.loads(matrix_path.read_text())
        component = json.loads(component_path.read_text())
        next(
            cell for cell in matrix["backends"] if cell["backend"]["name"] == "sail"
        )["backend"]["image_id"] = replacement
        component["backends"][0]["backend"]["image_id"] = replacement
        write_json(matrix_path, matrix)
        write_json(component_path, component)
        rows = (output / "images.tsv").read_text().splitlines()
        for index, line in enumerate(rows):
            fields = line.split("\t")
            if fields[0] == "adversarial" and fields[1] == "sail":
                fields[6] = replacement
                rows[index] = "\t".join(fields)
                break
        (output / "images.tsv").write_text("\n".join(rows) + "\n")
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "cross-track backend identity"):
            self.issue(output)

    def test_mutated_file_hash_is_rejected(self) -> None:
        mutated = self.clone("mutated-hash")
        (mutated / "logs" / "build-core.log").write_text("changed\n")
        self.assert_rejected(mutated, "receipt does not match")

    def test_unlisted_artifact_is_rejected(self) -> None:
        mutated = self.clone("extra-artifact")
        (mutated / "manual.json").write_text("{}\n")
        self.assert_rejected(mutated, "output file set mismatch")

    def test_symlink_is_rejected(self) -> None:
        mutated = self.clone("symlink")
        target = mutated / "logs" / "build-core.log"
        target.unlink()
        target.symlink_to(mutated / "images.tsv")
        self.assert_rejected(mutated, "contains a symlink")

    def test_bad_or_discovery_revision_is_rejected(self) -> None:
        mutated = self.clone("discovery")
        receipt_path = mutated / PUBLICATION.RECEIPT_NAME
        receipt = json.loads(receipt_path.read_text())
        receipt["source_revision"] += "-discovery"
        write_json(receipt_path, receipt)
        self.assert_rejected(mutated, "clean 40-hex")

    def test_incoherent_runner_image_id_is_rejected(self) -> None:
        mutated = self.clone("wrong-runner-id")
        manifest = mutated / "images.tsv"
        rows = manifest.read_text().splitlines()
        fields = rows[1].split("\t")
        fields[4] = "sha256:" + "f" * 64
        rows[1] = "\t".join(fields)
        manifest.write_text("\n".join(rows) + "\n")
        self.assert_rejected(mutated, "runner tag maps to multiple image IDs")

    def test_manual_one_cell_output_is_rejected(self) -> None:
        mutated = self.clone("manual-cell")
        (mutated / "components" / "baseline-memory-sfexample.json").unlink()
        self.assert_rejected(mutated, "output file set mismatch")

    def test_cross_track_environment_drift_is_rejected(self) -> None:
        mutated = self.clone("environment-drift")
        matrix_path = mutated / "matrix-adversarial-sfexample.json"
        matrix = json.loads(matrix_path.read_text())
        matrix["environment"]["cpu_model"] = "Different CPU"
        write_json(matrix_path, matrix)
        for path in (mutated / "components").glob("adversarial-*.json"):
            component = json.loads(path.read_text())
            component["environment"]["cpu_model"] = "Different CPU"
            write_json(path, component)
        self.assert_rejected(mutated, "cross-track environment differs")

    def test_cross_track_backend_identity_drift_is_rejected(self) -> None:
        mutated = self.clone("backend-identity-drift")
        matrix_path = mutated / "matrix-adversarial-sfexample.json"
        matrix = json.loads(matrix_path.read_text())
        matrix["backends"][0]["backend"]["worker_threads"] = 64
        write_json(matrix_path, matrix)
        component_path = mutated / "components" / "adversarial-memory-sfexample.json"
        component = json.loads(component_path.read_text())
        component["backends"][0]["backend"]["worker_threads"] = 64
        write_json(component_path, component)
        self.assert_rejected(mutated, "cross-track backend identity differs")

    def test_failed_outcomes_receive_a_truthful_completion_receipt(self) -> None:
        case_root = self.root / "failed-outcomes"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        matrix_path = output / "matrix-baseline-sfexample.json"
        matrix = json.loads(matrix_path.read_text())
        matrix["valid"] = False
        write_json(matrix_path, matrix)
        component_path = output / "components" / "baseline-memory-sfexample.json"
        component = json.loads(component_path.read_text())
        component["valid"] = False
        write_json(component_path, component)
        watchdog_path = output / "watchdogs/baseline-memory.json"
        watchdog = read_jsonl(watchdog_path)[0]
        watchdog["child_exit_status"] = 1
        write_jsonl(watchdog_path, [watchdog])
        arguments = argparse.Namespace(
            revision=self.revision,
            repository=self.repository,
            output_dir=output,
            scale="example",
        )
        with mock.patch.object(PUBLICATION, "run_semantic_validators"):
            PUBLICATION.issue_receipt(arguments, SCRIPT_DIRECTORY)
            PUBLICATION.verify_receipt(output, SCRIPT_DIRECTORY)
        receipt = json.loads((output / PUBLICATION.RECEIPT_NAME).read_text())
        self.assertEqual(receipt["suite_valid"], {"baseline": False, "adversarial": True})
        self.assertFalse(receipt["all_required_outcomes_valid"])

    def reuse_cell(self, output: Path, cell: str, project: str) -> None:
        """Make CELL's watchdog record look copied from a prior run at PROJECT."""
        suite, backend = cell.split("-", 1)
        watchdog_path = output / "watchdogs" / f"{cell}.json"
        record = read_jsonl(watchdog_path)[0]
        record["project"] = project
        record["container_name"] = f"{project}-{suite}-{backend}-cell"
        write_jsonl(watchdog_path, [record])

    def issue_resumed(self, output: Path, reused: list[dict], scale: str = "example") -> dict:
        listing = output.parent / f"{output.name}-reused.json"
        listing.write_text(json.dumps(reused))
        arguments = argparse.Namespace(
            revision=self.revision,
            repository=self.repository,
            output_dir=output,
            scale=scale,
            reused_cells=listing,
        )
        with mock.patch.object(PUBLICATION, "run_semantic_validators"):
            PUBLICATION.issue_receipt(arguments, SCRIPT_DIRECTORY)
        return json.loads((output / PUBLICATION.RECEIPT_NAME).read_text())

    def test_fresh_receipt_lists_no_reused_cells_and_legacy_receipts_still_verify(self) -> None:
        receipt = json.loads((self.output / PUBLICATION.RECEIPT_NAME).read_text())
        self.assertEqual(receipt["reused_cells"], [])
        legacy = self.clone("legacy")
        del receipt["reused_cells"]
        (legacy / PUBLICATION.RECEIPT_NAME).write_bytes(PUBLICATION.canonical_json(receipt))
        self.verify(legacy)

    def test_resumed_run_names_its_reused_cells_and_their_prior_run(self) -> None:
        case_root = self.root / "resumed"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        prior_project = "grust-lsqb-matrix-11-22"
        for cell in ("adversarial-turso", "baseline-turso"):
            self.reuse_cell(output, cell, prior_project)
        entry = {
            "cell": "baseline-turso",
            "source_output_root": "/prior/matrix-sfexample",
            "source_receipt_sha256": "f" * 64,
            "watchdog_project": prior_project,
        }
        reused = [dict(entry, cell="adversarial-turso"), entry]
        receipt = self.issue_resumed(output, reused)
        self.assertEqual(receipt["reused_cells"], reused)
        self.assertNotEqual(receipt["watchdog"]["project"], prior_project)
        self.verify(output)

        # The list is part of the canonical receipt: dropping it makes the
        # foreign records unexplained.
        tampered = json.loads((output / PUBLICATION.RECEIPT_NAME).read_text())
        del tampered["reused_cells"]
        (output / PUBLICATION.RECEIPT_NAME).write_bytes(PUBLICATION.canonical_json(tampered))
        self.assert_rejected(output, "do not share one Compose project")

    def test_reused_cells_must_match_their_watchdog_records(self) -> None:
        prior_project = "grust-lsqb-matrix-11-22"
        entry = {
            "cell": "baseline-turso",
            "source_output_root": "/prior/matrix-sfexample",
            "source_receipt_sha256": "f" * 64,
            "watchdog_project": prior_project,
        }
        # A record from an unlisted foreign project is not a reused cell.
        unlisted = self.root / "unlisted"
        unlisted.mkdir()
        output = make_bundle(unlisted, self.revision)
        self.reuse_cell(output, "baseline-turso", prior_project)
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "do not share one Compose project"):
            self.issue_resumed(output, [])
        # A listed cell whose record carries another project was not copied
        # from the run the receipt names.
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "not produced by the prior run"):
            self.issue_resumed(output, [dict(entry, watchdog_project="grust-lsqb-matrix-33-44")])
        # A listed cell whose record carries this run's own project is a lie.
        own = self.root / "own"
        own.mkdir()
        output = make_bundle(own, self.revision)
        record = read_jsonl(output / "watchdogs/baseline-turso.json")[0]
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "own Compose project"):
            self.issue_resumed(output, [dict(entry, watchdog_project=record["project"])])
        # Reused cells are named once, sorted, with every field well formed.
        for broken, phrase in (
            ([entry, entry], "listed twice"),
            ([dict(entry, cell="baseline-nowhere")], "not a matrix cell"),
            ([dict(entry, source_receipt_sha256="nope")], "invalid prior receipt digest"),
            ([dict(entry, source_output_root="relative")], "absolute prior output"),
            ([dict(entry, watchdog_project="other")], "invalid prior Compose project"),
            ([dict(entry, extra=1)], "unexpected fields"),
        ):
            with self.assertRaisesRegex(PUBLICATION.PublicationError, phrase):
                self.issue_resumed(output, broken)
        # A failed cell is never reused: its record must be a clean completion.
        failed = self.root / "failed"
        failed.mkdir()
        output = make_bundle(failed, self.revision)
        self.reuse_cell(output, "baseline-turso", prior_project)
        component_path = output / "components/baseline-turso-sfexample.json"
        component = json.loads(component_path.read_text())
        component["valid"] = False
        write_json(component_path, component)
        matrix_path = output / "matrix-baseline-sfexample.json"
        matrix = json.loads(matrix_path.read_text())
        matrix["valid"] = False
        write_json(matrix_path, matrix)
        record = read_jsonl(output / "watchdogs/baseline-turso.json")[0]
        record["child_exit_status"] = 1
        write_jsonl(output / "watchdogs/baseline-turso.json", [record])
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "reuses a failed cell"):
            self.issue_resumed(output, [entry])
        # A run that reused every cell executed nothing. (At sfexample the
        # policy cell always runs, so this can only happen at a downloaded
        # scale.)
        everything = self.root / "everything"
        everything.mkdir()
        output = make_bundle(everything, self.revision, "0.1")
        reused = []
        for suite in PUBLICATION.SUITES:
            for backend_entry in json.loads(
                (SCRIPT_DIRECTORY / "evidence-manifest-v2.json").read_text()
            )["backends"]:
                cell = f"{suite}-{backend_entry['id']}"
                self.reuse_cell(output, cell, prior_project)
                reused.append(dict(entry, cell=cell))
        reused.sort(key=lambda item: item["cell"])
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "every cell was reused"):
            self.issue_resumed(output, reused, "0.1")

    def test_bundled_manifest_mutation_is_rejected(self) -> None:
        mutated = self.clone("manifest-mutation")
        manifest_path = mutated / PUBLICATION.MANIFEST_NAME
        manifest = json.loads(manifest_path.read_text())
        manifest["fixture_mutation"] = True
        write_json(manifest_path, manifest)
        self.assert_rejected(mutated, "receipt does not match")

    def test_verification_uses_the_sealed_manifest_not_the_current_catalog(self) -> None:
        different_source = self.root / "different-validator-source"
        different_source.mkdir()
        manifest = json.loads((SCRIPT_DIRECTORY / PUBLICATION.MANIFEST_NAME).read_text())
        manifest["future_catalog_change"] = True
        write_json(different_source / PUBLICATION.MANIFEST_NAME, manifest)
        PUBLICATION.verify_receipt(self.output, different_source)

    def test_failed_policy_outcome_receives_a_truthful_completion_receipt(self) -> None:
        case_root = self.root / "failed-policy"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        policy_path = output / "policy-portable-sfexample.json"
        policy = json.loads(policy_path.read_text())
        policy["runs"][0]["attacks"][0]["actual_rejection"] = "accepted"
        policy["runs"][0]["attacks"][0]["status"] = "fail"
        policy["runs"][0]["attacks"][0]["error"] = None
        policy["valid"] = False
        write_json(policy_path, policy)
        watchdog_path = output / "watchdogs/policy-portable.json"
        watchdog = read_jsonl(watchdog_path)[0]
        watchdog["child_exit_status"] = 1
        write_jsonl(watchdog_path, [watchdog])
        arguments = argparse.Namespace(
            revision=self.revision,
            repository=self.repository,
            output_dir=output,
            scale="example",
        )
        with mock.patch.object(PUBLICATION, "run_semantic_validators"):
            PUBLICATION.issue_receipt(arguments, SCRIPT_DIRECTORY)
            PUBLICATION.verify_receipt(output, SCRIPT_DIRECTORY)
        receipt = json.loads((output / PUBLICATION.RECEIPT_NAME).read_text())
        self.assertFalse(receipt["policy_valid"])
        self.assertFalse(receipt["all_required_outcomes_valid"])

    def test_post_write_failure_removes_all_attestation_files(self) -> None:
        case_root = self.root / "post-write-failure"
        case_root.mkdir()
        output = make_bundle(case_root, self.revision)
        arguments = argparse.Namespace(
            revision=self.revision,
            repository=self.repository,
            output_dir=output,
            scale="example",
        )
        with mock.patch.object(PUBLICATION, "run_semantic_validators"), mock.patch.object(
            PUBLICATION,
            "verify_receipt",
            side_effect=PUBLICATION.PublicationError("induced post-write failure"),
        ):
            with self.assertRaisesRegex(PUBLICATION.PublicationError, "induced"):
                PUBLICATION.issue_receipt(arguments, SCRIPT_DIRECTORY)
        self.assertFalse((output / PUBLICATION.RECEIPT_NAME).exists())
        self.assertFalse((output / PUBLICATION.MANIFEST_NAME).exists())

    def test_receipt_is_not_overwritten(self) -> None:
        arguments = argparse.Namespace(
            revision=self.revision,
            repository=self.repository,
            output_dir=self.output,
            scale="example",
        )
        with mock.patch.object(PUBLICATION, "run_semantic_validators"):
            with self.assertRaisesRegex(PUBLICATION.PublicationError, "bundled evidence manifest"):
                PUBLICATION.issue_receipt(arguments, SCRIPT_DIRECTORY)


if __name__ == "__main__":
    unittest.main()
