#!/usr/bin/env python3
"""Real nested cancellation wiring with finite children and fake Docker only."""

from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
from typing import Any
import unittest


HERE = Path(__file__).resolve().parent
SELF = Path(__file__).resolve()
PROGRESS = HERE / "command-progress.py"
WATCHDOG = HERE / "cell-watchdog.py"
FIXTURE_SPEC = importlib.util.spec_from_file_location(
    "cell_watchdog_nested_fixtures", HERE / "test-cell-watchdog.py"
)
assert FIXTURE_SPEC is not None and FIXTURE_SPEC.loader is not None
FIXTURE = importlib.util.module_from_spec(FIXTURE_SPEC)
FIXTURE_SPEC.loader.exec_module(FIXTURE)


def read_records(path: Path) -> list[Any]:
    """Ignore only an unfinished final line while the logger is appending."""
    try:
        lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    except FileNotFoundError:
        return []
    return [json.loads(line) for line in lines if line.endswith("\n")]


def kill_owned_group(group: int) -> None:
    try:
        os.killpg(group, signal.SIGKILL)
    except ProcessLookupError:
        pass


class NestedCancellationTests(unittest.TestCase):
    def wait_ready(
        self, process: subprocess.Popen[bytes], output: Path, cell_path: Path
    ) -> tuple[int, int]:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            self.assertIsNone(process.poll(), "outer logger exited before readiness")
            records = read_records(output / "progress.jsonl")
            try:
                cell = json.loads(cell_path.read_text(encoding="utf-8"))
                log = (output / "command.log").read_text(encoding="utf-8")
            except (FileNotFoundError, json.JSONDecodeError):
                time.sleep(0.02)
                continue
            starts = [record for record in records if record["event"] == "command-start"]
            # A heartbeat is emitted only after name/label/ID discovery completes.
            if starts and "cell-watchdog.py: heartbeat " in log:
                watchdog_pid = starts[0]["pid"]
                self.assertIsInstance(watchdog_pid, int)
                self.assertEqual(cell["parent"], watchdog_pid)
                self.assertEqual(cell["pid"], cell["group"])
                self.assertEqual(os.getpgid(watchdog_pid), watchdog_pid)
                return watchdog_pid, cell["pid"]
            time.sleep(0.02)
        self.fail("nested fixture did not become ready within ten seconds")

    def cleanup(
        self, process: subprocess.Popen[bytes], output: Path, cell_path: Path
    ) -> None:
        # Try the production cancellation path first, including after a failed
        # assertion. Fallback groups come only from this run's own PID records.
        if process.poll() is None:
            process.send_signal(signal.SIGTERM)
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                pass
        groups = {process.pid}
        for record in read_records(output / "progress.jsonl"):
            if record["event"] == "command-start":
                groups.add(record["pid"])
        try:
            groups.add(json.loads(cell_path.read_text(encoding="utf-8"))["pid"])
        except (FileNotFoundError, json.JSONDecodeError):
            pass
        for group in groups:
            kill_owned_group(group)
        process.wait(timeout=5)

    def test_outer_sigterm_and_sigint_reap_both_layers_and_remove_only_owned_container(self) -> None:
        for kind in (signal.SIGTERM, signal.SIGINT):
            with self.subTest(signal=kind.name), tempfile.TemporaryDirectory(
                prefix="grust-watchdog-nested-test."
            ) as temporary:
                root = Path(temporary)
                fake_bin = root / "bin"
                fake_bin.mkdir()
                docker = fake_bin / "docker"
                docker.write_text(FIXTURE.FAKE_DOCKER, encoding="utf-8")
                docker.chmod(0o755)
                state = root / "state.json"
                docker_log = root / "docker.jsonl"
                docker_log.touch()
                unrelated = FIXTURE.container(
                    FIXTURE.UNRELATED_ID, "unrelated-container", "other-project"
                )
                state.write_text(
                    json.dumps({
                        FIXTURE.TARGET_ID: FIXTURE.container(
                            FIXTURE.TARGET_ID, FIXTURE.TARGET_NAME, FIXTURE.PROJECT
                        ),
                        FIXTURE.UNRELATED_ID: unrelated,
                    }),
                    encoding="utf-8",
                )
                environment = dict(os.environ)
                environment.update({
                    "PATH": f"{fake_bin}{os.pathsep}{os.environ.get('PATH', '')}",
                    "FAKE_DOCKER_STATE": str(state),
                    "FAKE_DOCKER_LOG": str(docker_log),
                })
                output = root / "progress"
                completion = root / "completion.json"
                cell_path = root / "cell.json"
                command = [
                    sys.executable, str(PROGRESS), "--output", str(output),
                    "--heartbeat-seconds", "0.05", "--termination-grace-seconds", "60",
                    "--", sys.executable, str(SELF), "--watchdog-launcher", str(completion),
                    "--timeout-ms", "30000", "--heartbeat-ms", "50",
                    "--container", FIXTURE.TARGET_NAME, "--project", FIXTURE.PROJECT,
                    "--service", FIXTURE.SERVICE,
                    "--", sys.executable, str(SELF), "--fixture-cell", str(cell_path),
                ]
                with (root / "outer.log").open("wb") as outer_log:
                    process = subprocess.Popen(
                        command, env=environment, stdout=outer_log,
                        stderr=subprocess.STDOUT, start_new_session=True,
                    )
                    try:
                        watchdog_pid, cell_pid = self.wait_ready(process, output, cell_path)
                        self.assertEqual(len({process.pid, watchdog_pid, cell_pid}), 3)
                        # Send to the outer PID only: forwarding must be performed
                        # by the real logger and watchdog, not by the test harness.
                        process.send_signal(kind)
                        process.wait(timeout=15)
                        self.assertNotEqual(process.returncode, 0)
                        for group in (process.pid, watchdog_pid, cell_pid):
                            with self.assertRaises(ProcessLookupError, msg=f"group {group} survived"):
                                os.killpg(group, 0)

                        marker = json.loads(completion.read_text(encoding="utf-8"))
                        self.assertEqual(marker["schema"], FIXTURE.WATCHDOG_MODULE.COMPLETION_SCHEMA)
                        self.assertEqual(marker["status"], "error")
                        self.assertEqual(marker["container_id"], FIXTURE.TARGET_ID)
                        self.assertEqual(marker["child_exit_status"], -signal.SIGTERM)
                        journal = read_records(output / "progress.jsonl")
                        self.assertEqual(journal[-1]["event"], "command-interrupted")
                        self.assertEqual(journal[-1]["pid"], watchdog_pid)
                        self.assertEqual(journal[-1]["exit"], 125)
                        self.assertEqual(journal[-1]["termination_grace_seconds"], 60)
                        self.assertIn("interrupted by SIGTERM", (output / "command.log").read_text())
                        self.assertEqual(
                            json.loads(state.read_text(encoding="utf-8")),
                            {FIXTURE.UNRELATED_ID: unrelated},
                        )
                        actions = read_records(docker_log)
                        mutations = [action for action in actions if action[1] != "inspect"]
                        self.assertEqual(mutations, [
                            ["container", "kill", FIXTURE.TARGET_ID],
                            ["container", "rm", "--force", FIXTURE.TARGET_ID],
                        ])
                    finally:
                        self.cleanup(process, output, cell_path)


if __name__ == "__main__":
    if sys.argv[1:2] == ["--watchdog-launcher"]:
        # command-progress intentionally does not inherit caller descriptors.
        # Open the completion descriptor inside its child, then replace that
        # child with the real watchdog main without adding another supervisor.
        descriptor = os.open(sys.argv[2], os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        os.set_inheritable(descriptor, True)
        os.execv(sys.executable, [
            sys.executable, str(WATCHDOG), "--record-fd", str(descriptor), *sys.argv[3:],
        ])
    elif sys.argv[1:2] == ["--fixture-cell"]:
        Path(sys.argv[2]).write_text(json.dumps({
            "pid": os.getpid(), "parent": os.getppid(), "group": os.getpgrp(),
        }), encoding="utf-8")
        time.sleep(30)
    else:
        unittest.main()
