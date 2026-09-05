#!/usr/bin/env python3
"""Offline tests for the pre-created Docker diagnostic command supervisor."""

from __future__ import annotations

import argparse
import importlib.util
import json
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location(
    "grust_lsqb_docker_diagnostic_command",
    HERE / "docker-diagnostic-command.py",
)
assert SPEC is not None and SPEC.loader is not None
COMMAND = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = COMMAND
SPEC.loader.exec_module(COMMAND)

CONTAINER_ID = "a" * 64
IMAGE_ID = "sha256:" + "b" * 64
NAME = "grust-lsqb-diagnostic-fixture"
PROJECT = "grust-lsqb-diagnostic"
SERVICE = "benchmark"


def record(status: str = "created", *, exit_code: int = 0, oom: bool = False):
    running = status == "running"
    return {
        "Id": CONTAINER_ID,
        "Name": f"/{NAME}",
        "Image": IMAGE_ID,
        "Config": {
            "Env": ["SECRET_MUST_NOT_BE_RETAINED=value"],
            "Labels": {
                "com.docker.compose.project": PROJECT,
                "com.docker.compose.service": SERVICE,
                "secret-label": "must-not-be-retained",
            },
        },
        "HostConfig": {
            "AutoRemove": False,
            "CpusetCpus": "",
            "Init": True,
            "Memory": COMMAND.MEMORY_BYTES,
            "MemorySwap": COMMAND.MEMORY_BYTES,
            "NanoCpus": COMMAND.NANO_CPUS,
            "NetworkMode": "none",
            "ReadonlyRootfs": True,
            "RestartPolicy": {"Name": "no", "MaximumRetryCount": 0},
        },
        "State": {
            "Dead": False,
            "Error": "",
            "ExitCode": exit_code,
            "FinishedAt": "2026-09-05T00:00:02Z" if status == "exited" else "",
            "OOMKilled": oom,
            "Paused": False,
            "Pid": 41 if running else 0,
            "Restarting": False,
            "Running": running,
            "StartedAt": "2026-09-05T00:00:01Z" if status != "created" else "",
            "Status": status,
        },
    }


class NoInterruption:
    def __init__(self):
        self.checkpoints = 0

    def checkpoint(self):
        self.checkpoints += 1


class InterruptAt(NoInterruption):
    def __init__(self, checkpoint: int, kind: int):
        super().__init__()
        self.at = checkpoint
        self.kind = kind

    def checkpoint(self):
        super().checkpoint()
        if self.checkpoints == self.at:
            raise COMMAND.WATCHDOG.WatchdogInterrupted(self.kind)


class FakeProcess:
    def __init__(self, status: int = 0, timeouts: int = 0):
        self.pid = 4242
        self.returncode = None
        self.status = status
        self.timeouts = timeouts
        self.wait_calls = []

    def wait(self, timeout=None):
        self.wait_calls.append(timeout)
        if self.timeouts:
            self.timeouts -= 1
            raise subprocess.TimeoutExpired("docker", timeout)
        self.returncode = self.status
        return self.status


class DockerDiagnosticCommandTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(
            prefix="grust-docker-diagnostic-command-test."
        )
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def arguments(self, name="case"):
        return argparse.Namespace(
            container_id=CONTAINER_ID,
            container=NAME,
            project=PROJECT,
            service=SERVICE,
            image_id=IMAGE_ID,
            output=self.root / name,
        )

    def run_with(
        self,
        inspections,
        *,
        process=None,
        interruption=None,
        remove_error=None,
        name="case",
    ):
        process = process or FakeProcess()
        interruption = interruption or NoInterruption()
        removal = mock.Mock(side_effect=remove_error)
        with (
            mock.patch.object(
                COMMAND.WATCHDOG,
                "inspect_container_record",
                side_effect=inspections,
            ) as inspected,
            mock.patch.object(COMMAND.subprocess, "Popen", return_value=process) as spawned,
            mock.patch.object(COMMAND.WATCHDOG, "stop_process_group") as stopped,
            mock.patch.object(COMMAND.WATCHDOG, "kill_exact_container") as killed,
            mock.patch.object(
                COMMAND.WATCHDOG, "remove_exact_container", removal
            ),
        ):
            result = COMMAND.run(self.arguments(name), interruption)
        return result, process, interruption, inspected, spawned, stopped, killed, removal

    def read(self, case, name):
        return json.loads((self.root / case / name).read_text())

    def test_success_attests_records_and_removes_only_the_pinned_id(self):
        created = record()
        exited = record("exited")
        result, process, _, inspected, spawned, stopped, killed, removal = self.run_with(
            [created, exited, exited]
        )
        self.assertEqual(result, 0)
        spawned.assert_called_once_with(
            ["docker", "container", "start", "--attach", CONTAINER_ID],
            start_new_session=True,
        )
        stopped.assert_called_once_with(process)
        killed.assert_not_called()
        removal.assert_called_once_with(CONTAINER_ID, NAME, PROJECT, SERVICE)
        self.assertEqual(inspected.call_count, 3)
        completion = self.read("case", "completion.json")
        self.assertEqual(completion["status"], "complete")
        self.assertFalse(completion["publication_eligible"])
        before = self.read("case", "container-before.json")
        after = self.read("case", "container-after.json")
        self.assertEqual(before["state"]["status"], "created")
        self.assertEqual(after["state"]["status"], "exited")
        retained = json.dumps([before, after])
        self.assertNotIn("SECRET_MUST_NOT_BE_RETAINED", retained)
        self.assertNotIn("secret-label", retained)

    def test_wait_uses_one_second_checkpoints_without_a_runtime_guess(self):
        process = FakeProcess(timeouts=2)
        result, process, interruption, *_ = self.run_with(
            [record(), record("exited"), record("exited")],
            process=process,
        )
        self.assertEqual(result, 0)
        self.assertEqual(process.wait_calls, [1.0, 1.0, 1.0])
        self.assertGreaterEqual(interruption.checkpoints, 6)

    def test_pre_start_contract_failure_never_starts_or_removes(self):
        wrong = record()
        wrong["HostConfig"]["MemorySwap"] = COMMAND.MEMORY_BYTES * 2
        result, _, _, _, spawned, stopped, killed, removal = self.run_with([wrong])
        self.assertEqual(result, COMMAND.ERROR_EXIT)
        spawned.assert_not_called()
        stopped.assert_not_called()
        killed.assert_not_called()
        removal.assert_not_called()
        self.assertIn("memory_swap differs", self.read("case", "completion.json")["error"])

    def test_image_mismatch_is_not_treated_as_owned_for_cleanup(self):
        wrong = record()
        wrong["Image"] = "sha256:" + "c" * 64
        result, _, _, _, spawned, _, killed, removal = self.run_with([wrong])
        self.assertEqual(result, COMMAND.ERROR_EXIT)
        spawned.assert_not_called()
        killed.assert_not_called()
        removal.assert_not_called()

    def test_interruption_after_attestation_removes_stopped_container(self):
        result, _, _, _, spawned, stopped, killed, removal = self.run_with(
            [record(), record()],
            interruption=InterruptAt(2, signal.SIGTERM),
        )
        self.assertEqual(result, 128 + signal.SIGTERM)
        spawned.assert_not_called()
        stopped.assert_not_called()
        killed.assert_not_called()
        removal.assert_called_once_with(CONTAINER_ID, NAME, PROJECT, SERVICE)
        completion = self.read("case", "completion.json")
        self.assertEqual(completion["status"], "interrupted")
        self.assertIn("SIGTERM", completion["error"])

    def test_interruption_after_spawn_reaps_group_then_kills_running_container(self):
        interruption = InterruptAt(3, signal.SIGINT)
        process = FakeProcess()
        result, _, _, _, spawned, stopped, killed, removal = self.run_with(
            [record(), record("running")],
            process=process,
            interruption=interruption,
        )
        self.assertEqual(result, 128 + signal.SIGINT)
        spawned.assert_called_once()
        stopped.assert_called_once_with(process)
        killed.assert_called_once_with(CONTAINER_ID, NAME, PROJECT, SERVICE)
        removal.assert_called_once_with(CONTAINER_ID, NAME, PROJECT, SERVICE)

    def test_oom_state_is_retained_and_cannot_be_success(self):
        oom = record("exited", exit_code=137, oom=True)
        result, *_ = self.run_with(
            [record(), oom, oom], process=FakeProcess(status=137)
        )
        self.assertEqual(result, COMMAND.ERROR_EXIT)
        after = self.read("case", "container-after.json")
        self.assertTrue(after["state"]["oom_killed"])
        completion = self.read("case", "completion.json")
        self.assertEqual(completion["status"], "error")
        self.assertIn("OOM-killed", completion["error"])

    def test_exit_status_mismatch_is_an_error(self):
        exited = record("exited", exit_code=7)
        result, *_ = self.run_with(
            [record(), exited, exited], process=FakeProcess(status=6)
        )
        self.assertEqual(result, COMMAND.ERROR_EXIT)
        self.assertIn("differs", self.read("case", "completion.json")["error"])

    def test_cleanup_failure_overrides_an_otherwise_successful_result(self):
        failure = COMMAND.WATCHDOG.WatchdogError("fixture remove failed")
        exited = record("exited")
        result, *_ = self.run_with(
            [record(), exited, exited], remove_error=failure
        )
        self.assertEqual(result, COMMAND.ERROR_EXIT)
        completion = self.read("case", "completion.json")
        self.assertEqual(completion["status"], "error")
        self.assertIn("cleanup failed", completion["error"])
        self.assertIn("fixture remove failed", completion["error"])

    def test_identity_mismatch_after_start_never_adopts_or_acts_by_name(self):
        mismatch = COMMAND.WATCHDOG.WatchdogError("fixture identity mismatch")
        result, _, _, inspected, _, _, killed, removal = self.run_with(
            [record(), mismatch, mismatch]
        )
        self.assertEqual(result, COMMAND.ERROR_EXIT)
        self.assertEqual(inspected.call_count, 3)
        for inspected_call in inspected.call_args_list:
            self.assertEqual(inspected_call.args[0], CONTAINER_ID)
        killed.assert_not_called()
        removal.assert_not_called()

    def test_extracted_inspect_parser_returns_record_and_wrapper_returns_id(self):
        raw = record()
        completed = subprocess.CompletedProcess(
            ["docker"], 0, stdout=json.dumps([raw]), stderr=""
        )
        with mock.patch.object(
            COMMAND.WATCHDOG, "docker_command", return_value=completed
        ):
            self.assertEqual(
                COMMAND.WATCHDOG.inspect_container_record(
                    CONTAINER_ID, NAME, PROJECT, SERVICE
                ),
                raw,
            )
            self.assertEqual(
                COMMAND.WATCHDOG.inspect_container(
                    CONTAINER_ID, NAME, PROJECT, SERVICE
                ),
                CONTAINER_ID,
            )

    def test_argument_parser_rejects_nonimmutable_identifiers(self):
        base = [
            "--container-id",
            CONTAINER_ID,
            "--container",
            NAME,
            "--project",
            PROJECT,
            "--service",
            SERVICE,
            "--image-id",
            IMAGE_ID,
            "--output",
            str(self.root / "arguments"),
        ]
        self.assertEqual(COMMAND.parse_arguments(base).container_id, CONTAINER_ID)
        with self.assertRaises(SystemExit):
            COMMAND.parse_arguments(["--container-id", "short", *base[2:]])
        bad_image = base.copy()
        bad_image[bad_image.index(IMAGE_ID)] = "rust:latest"
        with self.assertRaises(SystemExit):
            COMMAND.parse_arguments(bad_image)


if __name__ == "__main__":
    unittest.main()
