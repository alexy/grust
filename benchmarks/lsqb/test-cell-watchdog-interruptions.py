#!/usr/bin/env python3
"""Offline interruption and cleanup tests for the LSQB cell watchdog."""

from __future__ import annotations

import argparse
import importlib.util
import os
from pathlib import Path
import signal
import subprocess
import sys
import unittest
from unittest import mock


HERE = Path(__file__).resolve().parent
WATCHDOG = HERE / "cell-watchdog.py"
WATCHDOG_SPEC = importlib.util.spec_from_file_location(
    "cell_watchdog_interruptions", WATCHDOG
)
assert WATCHDOG_SPEC is not None and WATCHDOG_SPEC.loader is not None
WATCHDOG_MODULE = importlib.util.module_from_spec(WATCHDOG_SPEC)
WATCHDOG_SPEC.loader.exec_module(WATCHDOG_MODULE)

TARGET_ID = "a" * 64
REPLACEMENT_ID = "b" * 64
TARGET_NAME = "grust-lsqb-interruption-cell"
PROJECT = "grust-lsqb-interruption"
SERVICE = "benchmark"


def arguments(*, timeout_ms: int = 10_000) -> argparse.Namespace:
    return argparse.Namespace(
        timeout_ms=timeout_ms,
        heartbeat_ms=1_000,
        container=TARGET_NAME,
        project=PROJECT,
        service=SERVICE,
        command=["fixture-cell-command"],
    )


class FakeProcess:
    def __init__(self, *, returncode: int | None = None) -> None:
        self.pid = 47_111
        self.returncode = returncode

    def poll(self) -> int | None:
        return self.returncode


class CellWatchdogInterruptionTests(unittest.TestCase):
    def test_signal_handlers_latch_first_signal_and_restore_exact_handlers(self) -> None:
        previous_int = object()
        previous_term = object()

        def previous(signal_number: int) -> object:
            return {
                signal.SIGINT: previous_int,
                signal.SIGTERM: previous_term,
            }[signal_number]

        with (
            mock.patch.object(WATCHDOG_MODULE.signal, "getsignal", side_effect=previous),
            mock.patch.object(WATCHDOG_MODULE.signal, "signal") as install,
        ):
            with WATCHDOG_MODULE.controlled_interruption_signals() as controller:
                self.assertEqual(len(install.call_args_list), 2)
                self.assertIs(install.call_args_list[0].args[1].__self__, controller)
                self.assertIs(install.call_args_list[1].args[1].__self__, controller)
                controller.handle(signal.SIGTERM, None)
                controller.handle(signal.SIGINT, None)
                self.assertEqual(controller.signal_number, signal.SIGTERM)
                with self.assertRaisesRegex(
                    WATCHDOG_MODULE.WatchdogInterrupted, "SIGTERM"
                ):
                    controller.checkpoint()

        self.assertEqual(
            install.call_args_list[-2:],
            [
                mock.call(signal.SIGTERM, previous_term),
                mock.call(signal.SIGINT, previous_int),
            ],
        )

    def test_partial_signal_installation_restores_the_installed_handler(self) -> None:
        previous_int = object()
        previous_term = object()
        with (
            mock.patch.object(
                WATCHDOG_MODULE.signal,
                "getsignal",
                side_effect=[previous_int, previous_term],
            ),
            mock.patch.object(
                WATCHDOG_MODULE.signal,
                "signal",
                side_effect=[None, RuntimeError("fixture install"), None],
            ) as install,
        ):
            with self.assertRaisesRegex(
                WATCHDOG_MODULE.WatchdogError,
                "could not install watchdog handler for SIGTERM: fixture install",
            ):
                with WATCHDOG_MODULE.controlled_interruption_signals():
                    self.fail("the context must not be entered")

        self.assertEqual(
            install.call_args_list[-1], mock.call(signal.SIGINT, previous_int)
        )

    def test_stop_process_group_reaps_even_if_its_leader_already_exited(self) -> None:
        process = mock.Mock(pid=47_112, returncode=0)
        process.wait.side_effect = [0, 0]
        with mock.patch.object(WATCHDOG_MODULE.os, "killpg") as killpg:
            WATCHDOG_MODULE.stop_process_group(process)

        self.assertEqual(
            killpg.call_args_list,
            [
                mock.call(process.pid, signal.SIGTERM),
                mock.call(process.pid, signal.SIGKILL),
            ],
        )
        self.assertEqual(
            process.wait.call_args_list,
            [mock.call(timeout=2.0), mock.call(timeout=2.0)],
        )

    def test_stop_process_group_terminates_and_reaps_a_finite_child(self) -> None:
        process = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(30)"],
            start_new_session=True,
        )
        try:
            WATCHDOG_MODULE.stop_process_group(process)
            self.assertIsNotNone(process.returncode)
        finally:
            if process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=2)

    def test_interruption_after_identity_stops_child_then_mutates_exact_id(self) -> None:
        controller = WATCHDOG_MODULE.InterruptionController()
        process = FakeProcess()
        events: list[str] = []

        def discover(*_args: object) -> str:
            events.append("discover")
            controller.handle(signal.SIGTERM, None)
            return TARGET_ID

        def stop(_process: FakeProcess) -> None:
            events.append("stop")

        def kill(*_args: object) -> None:
            events.append("kill")

        def remove(*_args: object) -> None:
            events.append("remove")

        with (
            mock.patch.object(
                WATCHDOG_MODULE.subprocess, "Popen", return_value=process
            ),
            mock.patch.object(
                WATCHDOG_MODULE, "inspect_container", side_effect=discover
            ),
            mock.patch.object(WATCHDOG_MODULE, "stop_process_group", side_effect=stop),
            mock.patch.object(
                WATCHDOG_MODULE, "kill_exact_container", side_effect=kill
            ) as killed,
            mock.patch.object(
                WATCHDOG_MODULE, "remove_exact_container", side_effect=remove
            ) as removed,
        ):
            status, record, error = WATCHDOG_MODULE.run(
                arguments(), interruption=controller
            )

        self.assertEqual(status, WATCHDOG_MODULE.WATCHDOG_ERROR_EXIT)
        self.assertEqual(record["status"], "error")
        self.assertEqual(record["schema"], WATCHDOG_MODULE.COMPLETION_SCHEMA)
        self.assertEqual(record["container_id"], TARGET_ID)
        self.assertIn("SIGTERM", error)
        self.assertEqual(events, ["discover", "stop", "kill", "remove"])
        killed.assert_called_once_with(TARGET_ID, TARGET_NAME, PROJECT, SERVICE)
        removed.assert_called_once_with(TARGET_ID, TARGET_NAME, PROJECT, SERVICE)

    def test_interruption_before_identity_discovers_only_after_child_stops(self) -> None:
        controller = WATCHDOG_MODULE.InterruptionController()
        process = FakeProcess()
        events: list[str] = []

        def discover(*_args: object) -> str | None:
            if not events:
                events.append("discover-absent")
                controller.handle(signal.SIGINT, None)
                return None
            events.append("discover-late")
            return TARGET_ID

        with (
            mock.patch.object(
                WATCHDOG_MODULE.subprocess, "Popen", return_value=process
            ),
            mock.patch.object(
                WATCHDOG_MODULE, "inspect_container", side_effect=discover
            ),
            mock.patch.object(
                WATCHDOG_MODULE,
                "stop_process_group",
                side_effect=lambda _process: events.append("stop"),
            ),
            mock.patch.object(WATCHDOG_MODULE, "kill_exact_container") as killed,
            mock.patch.object(WATCHDOG_MODULE, "remove_exact_container") as removed,
        ):
            status, record, error = WATCHDOG_MODULE.run(
                arguments(), interruption=controller
            )

        self.assertEqual(status, WATCHDOG_MODULE.WATCHDOG_ERROR_EXIT)
        self.assertEqual(record["status"], "error")
        self.assertEqual(record["container_id"], TARGET_ID)
        self.assertIn("SIGINT", error)
        self.assertEqual(events, ["discover-absent", "stop", "discover-late"])
        killed.assert_called_once_with(TARGET_ID, TARGET_NAME, PROJECT, SERVICE)
        removed.assert_called_once_with(TARGET_ID, TARGET_NAME, PROJECT, SERVICE)

    def test_tainted_name_discovery_is_never_retried_or_mutated(self) -> None:
        process = FakeProcess()
        mismatch = WATCHDOG_MODULE.WatchdogError("fixture label mismatch")
        with (
            mock.patch.object(
                WATCHDOG_MODULE.subprocess, "Popen", return_value=process
            ),
            mock.patch.object(
                WATCHDOG_MODULE, "inspect_container", side_effect=mismatch
            ) as inspected,
            mock.patch.object(WATCHDOG_MODULE, "stop_process_group"),
            mock.patch.object(WATCHDOG_MODULE, "kill_exact_container") as killed,
            mock.patch.object(WATCHDOG_MODULE, "remove_exact_container") as removed,
        ):
            status, record, error = WATCHDOG_MODULE.run(arguments())

        self.assertEqual(status, WATCHDOG_MODULE.WATCHDOG_ERROR_EXIT)
        self.assertEqual(record["status"], "error")
        self.assertIsNone(record["container_id"])
        self.assertIn("label mismatch", error)
        inspected.assert_called_once_with(TARGET_NAME, TARGET_NAME, PROJECT, SERVICE)
        killed.assert_not_called()
        removed.assert_not_called()

    def test_replaced_pinned_identity_is_rejected_for_kill_and_removal(self) -> None:
        process = FakeProcess()
        identity = WATCHDOG_MODULE.OwnedContainerIdentity()
        identity.state = identity.PINNED
        identity.container_id = TARGET_ID
        with (
            mock.patch.object(WATCHDOG_MODULE, "stop_process_group"),
            mock.patch.object(
                WATCHDOG_MODULE, "inspect_container", return_value=REPLACEMENT_ID
            ) as inspected,
            mock.patch.object(WATCHDOG_MODULE, "docker_command") as docker,
        ):
            container_id, error = WATCHDOG_MODULE.cleanup_owned_cell(
                arguments(), process, identity, kill_container=True
            )

        self.assertEqual(container_id, TARGET_ID)
        self.assertIn("identity changed before watchdog kill", error)
        self.assertIn("identity changed before watchdog removal", error)
        self.assertEqual(
            inspected.call_args_list,
            [
                mock.call(TARGET_ID, TARGET_NAME, PROJECT, SERVICE),
                mock.call(TARGET_ID, TARGET_NAME, PROJECT, SERVICE),
            ],
        )
        docker.assert_not_called()

    def test_cleanup_aggregates_failures_and_attempts_each_safe_action(self) -> None:
        process = FakeProcess()
        identity = WATCHDOG_MODULE.OwnedContainerIdentity()
        identity.state = identity.PINNED
        identity.container_id = TARGET_ID
        with (
            mock.patch.object(
                WATCHDOG_MODULE,
                "stop_process_group",
                side_effect=WATCHDOG_MODULE.WatchdogError("stop failed"),
            ) as stopped,
            mock.patch.object(
                WATCHDOG_MODULE,
                "kill_exact_container",
                side_effect=WATCHDOG_MODULE.WatchdogError("kill failed"),
            ) as killed,
            mock.patch.object(
                WATCHDOG_MODULE,
                "remove_exact_container",
                side_effect=WATCHDOG_MODULE.WatchdogError("remove failed"),
            ) as removed,
        ):
            container_id, error = WATCHDOG_MODULE.cleanup_owned_cell(
                arguments(), process, identity, kill_container=True
            )

        self.assertEqual(container_id, TARGET_ID)
        self.assertEqual(error, "stop failed; kill failed; remove failed")
        stopped.assert_called_once_with(process)
        killed.assert_called_once_with(TARGET_ID, TARGET_NAME, PROJECT, SERVICE)
        removed.assert_called_once_with(TARGET_ID, TARGET_NAME, PROJECT, SERVICE)

    def test_failed_process_stop_prevents_unsafe_late_name_discovery(self) -> None:
        process = FakeProcess()
        identity = WATCHDOG_MODULE.OwnedContainerIdentity()
        with (
            mock.patch.object(
                WATCHDOG_MODULE,
                "stop_process_group",
                side_effect=WATCHDOG_MODULE.WatchdogError("group still active"),
            ),
            mock.patch.object(WATCHDOG_MODULE, "inspect_container") as inspected,
            mock.patch.object(WATCHDOG_MODULE, "kill_exact_container") as killed,
            mock.patch.object(WATCHDOG_MODULE, "remove_exact_container") as removed,
        ):
            container_id, error = WATCHDOG_MODULE.cleanup_owned_cell(
                arguments(), process, identity, kill_container=True
            )

        self.assertIsNone(container_id)
        self.assertEqual(error, "group still active")
        inspected.assert_not_called()
        killed.assert_not_called()
        removed.assert_not_called()

    def test_completed_cell_cleanup_failure_cannot_report_success(self) -> None:
        process = FakeProcess(returncode=0)
        with (
            mock.patch.object(
                WATCHDOG_MODULE.subprocess, "Popen", return_value=process
            ),
            mock.patch.object(
                WATCHDOG_MODULE, "inspect_container", return_value=TARGET_ID
            ),
            mock.patch.object(
                WATCHDOG_MODULE,
                "cleanup_owned_cell",
                return_value=(TARGET_ID, "fixture removal failure"),
            ),
        ):
            status, record, error = WATCHDOG_MODULE.run(arguments())

        self.assertEqual(status, WATCHDOG_MODULE.WATCHDOG_ERROR_EXIT)
        self.assertEqual(record["status"], "error")
        self.assertEqual(record["child_exit_status"], 0)
        self.assertIn("fixture removal failure", error)

    def test_unexpected_base_exception_still_runs_exact_cleanup(self) -> None:
        process = FakeProcess()
        with (
            mock.patch.object(
                WATCHDOG_MODULE.subprocess, "Popen", return_value=process
            ),
            mock.patch.object(
                WATCHDOG_MODULE, "inspect_container", return_value=TARGET_ID
            ),
            mock.patch.object(
                WATCHDOG_MODULE.time, "sleep", side_effect=KeyboardInterrupt
            ),
            mock.patch.object(WATCHDOG_MODULE, "stop_process_group") as stopped,
            mock.patch.object(WATCHDOG_MODULE, "kill_exact_container") as killed,
            mock.patch.object(WATCHDOG_MODULE, "remove_exact_container") as removed,
        ):
            with self.assertRaises(KeyboardInterrupt):
                WATCHDOG_MODULE.run(arguments())

        stopped.assert_called_once_with(process)
        killed.assert_called_once_with(TARGET_ID, TARGET_NAME, PROJECT, SERVICE)
        removed.assert_called_once_with(TARGET_ID, TARGET_NAME, PROJECT, SERVICE)


if __name__ == "__main__":
    unittest.main()
