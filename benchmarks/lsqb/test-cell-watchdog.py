#!/usr/bin/env python3
"""Focused tests for the narrowly scoped LSQB cell watchdog."""

from __future__ import annotations

import argparse
from contextlib import redirect_stderr
import io
import json
import importlib.util
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock


HERE = Path(__file__).resolve().parent
WATCHDOG = HERE / "cell-watchdog.py"
WATCHDOG_SPEC = importlib.util.spec_from_file_location("cell_watchdog", WATCHDOG)
assert WATCHDOG_SPEC is not None and WATCHDOG_SPEC.loader is not None
WATCHDOG_MODULE = importlib.util.module_from_spec(WATCHDOG_SPEC)
WATCHDOG_SPEC.loader.exec_module(WATCHDOG_MODULE)
TARGET_ID = "a" * 64
UNRELATED_ID = "b" * 64
PROJECT = "grust-lsqb-test"
SERVICE = "benchmark"
TARGET_NAME = "grust-lsqb-test-cell"

FAKE_DOCKER = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

state_path = Path(os.environ["FAKE_DOCKER_STATE"])
log_path = Path(os.environ["FAKE_DOCKER_LOG"])
arguments = sys.argv[1:]
with log_path.open("a", encoding="utf-8") as log:
    log.write(json.dumps(arguments, separators=(",", ":")) + "\n")
state = json.loads(state_path.read_text(encoding="utf-8"))

if arguments[:2] == ["container", "inspect"]:
    identity = arguments[2]
    match = next(
        (item for item in state.values() if item["Id"] == identity or item["Name"] == "/" + identity),
        None,
    )
    if match is None:
        print("Error: No such object: " + identity, file=sys.stderr)
        raise SystemExit(1)
    print(json.dumps([match], separators=(",", ":")))
elif arguments[:2] == ["container", "kill"]:
    identity = arguments[2]
    if identity not in state:
        print("Error: No such container: " + identity, file=sys.stderr)
        raise SystemExit(1)
    state[identity]["killed"] = True
    state_path.write_text(json.dumps(state), encoding="utf-8")
    print(identity)
elif arguments[:3] == ["container", "rm", "--force"]:
    identity = arguments[3]
    if identity not in state:
        print("Error: No such container: " + identity, file=sys.stderr)
        raise SystemExit(1)
    del state[identity]
    state_path.write_text(json.dumps(state), encoding="utf-8")
    print(identity)
else:
    print("unexpected fake Docker arguments: " + repr(arguments), file=sys.stderr)
    raise SystemExit(2)
'''


def container(identity: str, name: str, project: str) -> dict[str, object]:
    return {
        "Id": identity,
        "Name": f"/{name}",
        "Config": {
            "Labels": {
                "com.docker.compose.project": project,
                "com.docker.compose.service": SERVICE,
            }
        },
    }


class CellWatchdogTests(unittest.TestCase):
    def test_deadline_helpers_are_strict_and_round_elapsed_time_up(self) -> None:
        self.assertTrue(WATCHDOG_MODULE.completion_is_timely(0, 999_999, 1_000_000))
        self.assertFalse(WATCHDOG_MODULE.completion_is_timely(0, 1_000_000, 1_000_000))
        self.assertFalse(WATCHDOG_MODULE.completion_is_timely(0, 1_000_001, 1_000_000))
        self.assertFalse(WATCHDOG_MODULE.completion_is_timely(None, 999_999, 1_000_000))
        self.assertEqual(WATCHDOG_MODULE.elapsed_milliseconds(0, 1), 1)
        self.assertEqual(WATCHDOG_MODULE.elapsed_milliseconds(0, 1_000_000), 1)
        self.assertEqual(WATCHDOG_MODULE.elapsed_milliseconds(0, 1_000_001), 2)

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="grust-cell-watchdog-test.")
        self.root = Path(self.temporary.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        docker = self.bin / "docker"
        docker.write_text(FAKE_DOCKER, encoding="utf-8")
        docker.chmod(0o755)
        self.state = self.root / "state.json"
        self.log = self.root / "docker.log"
        self.log.write_text("", encoding="utf-8")
        self.environment = dict(os.environ)
        self.environment.update(
            {
                "PATH": f"{self.bin}{os.pathsep}{os.environ.get('PATH', '')}",
                "FAKE_DOCKER_STATE": str(self.state),
                "FAKE_DOCKER_LOG": str(self.log),
            }
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def invoke(
        self,
        timeout_ms: int,
        command: list[str],
        heartbeat_ms: int | None = None,
    ) -> tuple[subprocess.CompletedProcess[str], bytes]:
        record_path = self.root / "completion.json"
        arguments = self.watchdog_arguments(timeout_ms, heartbeat_ms)
        with record_path.open("wb") as record:
            result = subprocess.run(
                [*arguments, "--record-fd", str(record.fileno()), "--", *command],
                check=False,
                capture_output=True,
                text=True,
                env=self.environment,
                pass_fds=(record.fileno(),),
                timeout=5,
            )
        return result, record_path.read_bytes()

    @staticmethod
    def watchdog_arguments(
        timeout_ms: int, heartbeat_ms: int | None = None
    ) -> list[str]:
        arguments = [
            sys.executable,
            str(WATCHDOG),
            "--timeout-ms",
            str(timeout_ms),
        ]
        if heartbeat_ms is not None:
            arguments.extend(["--heartbeat-ms", str(heartbeat_ms)])
        arguments.extend(
            [
                "--container",
                TARGET_NAME,
                "--project",
                PROJECT,
                "--service",
                SERVICE,
            ]
        )
        return arguments

    def test_heartbeat_deadlines_coalesce_missed_intervals(self) -> None:
        next_deadline = WATCHDOG_MODULE.next_heartbeat_deadline
        self.assertEqual(next_deadline(100, 99, 10), 100)
        self.assertEqual(next_deadline(100, 100, 10), 110)
        self.assertEqual(next_deadline(100, 135, 10), 140)
        self.assertEqual(next_deadline(100, 1_000, 10), 1_010)

    def test_heartbeat_is_exact_and_omits_command_secrets(self) -> None:
        sentinel = "postgres://secret-user:secret-password@example.invalid/db"
        arguments = type(
            "Arguments",
            (),
            {"container": TARGET_NAME, "command": ["--endpoint", sentinel]},
        )()
        line = WATCHDOG_MODULE.heartbeat_line(
            arguments.container,
            1_000_000_000,
            15_000_000_000,
            6_000_000_000,
        )
        self.assertEqual(
            line,
            f"cell-watchdog.py: heartbeat container={TARGET_NAME} "
            "elapsed_ms=5000 remaining_ms=9000",
        )
        self.assertNotIn(sentinel, line)
        self.assertNotIn("endpoint", line)
        self.assertNotIn('"schema"', line)
        self.assertEqual(
            WATCHDOG_MODULE.heartbeat_content(
                arguments.container,
                1_000_000_000,
                15_000_000_000,
                6_000_000_000,
            ),
            (line + "\n").encode(),
        )

    def test_heartbeat_output_failure_is_best_effort(self) -> None:
        attempted = threading.Event()

        def broken_write(_descriptor: int, _content: bytes) -> int:
            attempted.set()
            raise OSError("fixture closed pipe")

        emitter = WATCHDOG_MODULE.HeartbeatEmitter(-1, write_once=broken_write)
        self.assertTrue(emitter.submit(b"fixture-heartbeat\n"))
        self.assertTrue(attempted.wait(1), "daemon writer never attempted output")
        emitter.close()

    def test_heartbeat_emitter_makes_one_bounded_atomic_write(self) -> None:
        calls: list[tuple[int, bytes]] = []
        written = threading.Event()

        def record_write(descriptor: int, content: bytes) -> int:
            calls.append((descriptor, content))
            written.set()
            return len(content)

        emitter = WATCHDOG_MODULE.HeartbeatEmitter(17, write_once=record_write)
        self.assertFalse(
            emitter.submit(b"x" * WATCHDOG_MODULE.ATOMIC_PROGRESS_BYTES + b"\n")
        )
        content = WATCHDOG_MODULE.heartbeat_content(TARGET_NAME, 0, 1_000, 500)
        self.assertTrue(emitter.submit(content))
        self.assertTrue(written.wait(1), "daemon writer never received output")
        emitter.close()

        self.assertEqual(calls, [(17, content)])
        self.assertLessEqual(len(content), WATCHDOG_MODULE.ATOMIC_PROGRESS_BYTES)

    def test_recovery_drops_stale_heartbeats_and_accepts_a_fresh_one(self) -> None:
        first = b"heartbeat-1\n"
        second = b"heartbeat-2-stale\n"
        third = b"heartbeat-3-stale\n"
        fourth = b"heartbeat-4-fresh\n"
        calls: list[bytes] = []
        first_entered = threading.Event()
        release_first = threading.Event()
        fourth_written = threading.Event()

        def controlled_write(_descriptor: int, content: bytes) -> int:
            calls.append(content)
            if content == first:
                first_entered.set()
                release_first.wait(5)
            elif content == fourth:
                fourth_written.set()
            return len(content)

        emitter = WATCHDOG_MODULE.HeartbeatEmitter(19, write_once=controlled_write)
        self.assertTrue(emitter.submit(first))
        self.assertTrue(first_entered.wait(1), "first heartbeat did not enter its sink")
        self.assertFalse(emitter.submit(second))
        self.assertFalse(emitter.submit(third))
        release_first.set()

        deadline = time.monotonic() + 1
        fourth_accepted = False
        while time.monotonic() < deadline:
            fourth_accepted = emitter.submit(fourth)
            if fourth_accepted:
                break
            time.sleep(0.001)
        self.assertTrue(fourth_accepted, "fresh heartbeat was not accepted")
        self.assertTrue(fourth_written.wait(1), "fresh heartbeat was not emitted")
        emitter.close()

        self.assertEqual(calls, [first, fourth])

    def test_short_atomic_write_is_never_retried(self) -> None:
        calls: list[bytes] = []
        attempted = threading.Event()

        def short_write(_descriptor: int, content: bytes) -> int:
            calls.append(content)
            attempted.set()
            return len(content) - 1

        emitter = WATCHDOG_MODULE.HeartbeatEmitter(23, write_once=short_write)
        content = b"fixture-heartbeat\n"
        self.assertTrue(emitter.submit(content))
        self.assertTrue(attempted.wait(1), "short write was not attempted")
        self.assertTrue(emitter._stopping.wait(1), "short write did not disable output")
        self.assertFalse(emitter.submit(b"must-not-retry\n"))
        emitter.close()

        self.assertEqual(calls, [content])

    def test_queue_failure_releases_the_outstanding_permit(self) -> None:
        emitter = WATCHDOG_MODULE.HeartbeatEmitter(
            29, write_once=lambda _, value: len(value)
        )
        original_put = emitter._messages.put_nowait
        emitter._messages.put_nowait = mock.Mock(side_effect=RuntimeError("fixture queue"))
        try:
            self.assertFalse(emitter.submit(b"fixture-heartbeat\n"))
            self.assertTrue(emitter._outstanding.acquire(blocking=False))
            emitter._outstanding.release()
        finally:
            emitter._messages.put_nowait = original_put
            emitter.close()

    def test_stop_before_write_releases_the_outstanding_permit(self) -> None:
        writes: list[bytes] = []
        emitter = WATCHDOG_MODULE.HeartbeatEmitter(
            31, write_once=lambda _, content: writes.append(content) or len(content)
        )
        with emitter._messages.not_empty:
            self.assertTrue(emitter._outstanding.acquire(blocking=False))
            emitter._messages.queue.append(b"must-not-write\n")
            emitter._stopping.set()
            emitter._messages.not_empty.notify()
        emitter._worker.join(1)

        self.assertFalse(emitter._worker.is_alive())
        self.assertEqual(writes, [])
        self.assertTrue(emitter._outstanding.acquire(blocking=False))
        emitter._outstanding.release()

    def test_parser_defaults_to_thirty_second_positive_heartbeats(self) -> None:
        base = [
            str(WATCHDOG),
            "--timeout-ms",
            "1000",
            "--container",
            TARGET_NAME,
            "--project",
            PROJECT,
            "--service",
            SERVICE,
            "--record-fd",
            "3",
            "--",
            "true",
        ]
        with mock.patch.object(sys, "argv", base):
            self.assertEqual(
                WATCHDOG_MODULE.parse_arguments().heartbeat_ms,
                WATCHDOG_MODULE.DEFAULT_HEARTBEAT_MS,
            )
        invalid = [*base[:3], "--heartbeat-ms", "0", *base[3:]]
        with mock.patch.object(sys, "argv", invalid), redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as raised:
                WATCHDOG_MODULE.parse_arguments()
        self.assertEqual(raised.exception.code, 2)

    def test_heartbeat_reaches_the_pipe_before_the_child_finishes(self) -> None:
        self.state.write_text(
            json.dumps({TARGET_ID: container(TARGET_ID, TARGET_NAME, PROJECT)}),
            encoding="utf-8",
        )
        record_path = self.root / "heartbeat-completion.json"
        release_path = self.root / "release-child"
        command = [
            sys.executable,
            "-c",
            (
                "from pathlib import Path\n"
                "import sys\n"
                "import time\n"
                "release = Path(sys.argv[1])\n"
                "while not release.exists():\n"
                "    time.sleep(0.01)\n"
            ),
            str(release_path),
        ]
        arguments = self.watchdog_arguments(2_000, heartbeat_ms=10)
        with record_path.open("wb") as record:
            process = subprocess.Popen(
                [*arguments, "--record-fd", str(record.fileno()), "--", *command],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                env=self.environment,
                pass_fds=(record.fileno(),),
            )
            try:
                assert process.stderr is not None
                readable, _, _ = select.select([process.stderr], [], [], 2.0)
                self.assertTrue(readable, "no heartbeat reached stderr within two seconds")
                first_line = process.stderr.readline()
                self.assertTrue(
                    first_line.startswith(
                        f"cell-watchdog.py: heartbeat container={TARGET_NAME} "
                    ),
                    first_line,
                )
                self.assertIsNone(process.poll(), "watchdog finished before its first heartbeat")
                release_path.write_text("release\n", encoding="utf-8")
                _stdout, remaining_stderr = process.communicate(timeout=5)
            finally:
                if process.poll() is None:
                    release_path.write_text("release\n", encoding="utf-8")
                    process.communicate(timeout=5)

        self.assertEqual(process.returncode, 0, first_line + remaining_stderr)
        marker = json.loads(record_path.read_bytes())
        self.assertEqual(
            set(marker),
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
            },
        )
        self.assertEqual(marker["schema"], WATCHDOG_MODULE.COMPLETION_SCHEMA)
        self.assertNotIn('"schema"', first_line + remaining_stderr)

    def test_blocked_heartbeat_sink_cannot_delay_hard_timeout(self) -> None:
        self.state.write_text(
            json.dumps({TARGET_ID: container(TARGET_ID, TARGET_NAME, PROJECT)}),
            encoding="utf-8",
        )
        sink_entered = threading.Event()
        release_sink = threading.Event()

        def blocked_write(_descriptor: int, content: bytes) -> int:
            sink_entered.set()
            release_sink.wait(10)
            return len(content)

        emitter = WATCHDOG_MODULE.HeartbeatEmitter(-1, write_once=blocked_write)
        self.assertTrue(emitter.submit(b"occupy-blocked-sink\n"))
        self.assertTrue(sink_entered.wait(1), "fixture sink did not block")
        arguments = argparse.Namespace(
            timeout_ms=50,
            heartbeat_ms=1,
            container=TARGET_NAME,
            project=PROJECT,
            service=SERVICE,
            command=[sys.executable, "-c", "import time; time.sleep(10)"],
        )
        results: list[tuple[int, dict[str, object], str | None]] = []
        errors: list[BaseException] = []

        def supervise() -> None:
            try:
                results.append(WATCHDOG_MODULE.run(arguments, emitter))
            except BaseException as error:  # Preserve a worker-thread test failure.
                errors.append(error)

        supervisor = threading.Thread(target=supervise)
        with mock.patch.dict(os.environ, self.environment, clear=True):
            supervisor.start()
            supervisor.join(3)
            finished_before_sink_release = not supervisor.is_alive()
            emitter.close()
            release_sink.set()
            supervisor.join(5)

        self.assertTrue(finished_before_sink_release, "blocked progress delayed supervision")
        self.assertFalse(supervisor.is_alive(), "watchdog supervision did not terminate")
        self.assertEqual(errors, [])
        self.assertEqual(len(results), 1)
        status, marker, error = results[0]
        self.assertEqual(status, WATCHDOG_MODULE.WATCHDOG_TIMEOUT_EXIT)
        self.assertIsNone(error)
        self.assertEqual(marker["status"], "timeout")
        self.assertEqual(marker["schema"], WATCHDOG_MODULE.COMPLETION_SCHEMA)

    def test_timeout_kills_only_the_attested_named_cell(self) -> None:
        self.state.write_text(
            json.dumps(
                {
                    TARGET_ID: container(TARGET_ID, TARGET_NAME, PROJECT),
                    UNRELATED_ID: container(UNRELATED_ID, "unrelated-container", "other-project"),
                }
            ),
            encoding="utf-8",
        )
        result, raw_record = self.invoke(
            50, [sys.executable, "-c", "import time; time.sleep(10)"]
        )

        self.assertEqual(result.returncode, 124, result.stderr)
        marker = json.loads(raw_record)
        self.assertEqual(marker["status"], "timeout")
        self.assertEqual(marker["timeout_ms"], 50)
        self.assertEqual(marker["container_id"], TARGET_ID)
        self.assertEqual(marker["container_name"], TARGET_NAME)
        self.assertEqual(marker["project"], PROJECT)
        self.assertEqual(marker["service"], SERVICE)
        self.assertGreaterEqual(marker["elapsed_wall_ms"], 50)
        self.assertEqual(
            raw_record,
            (json.dumps(marker, sort_keys=True, separators=(",", ":")) + "\n").encode(),
        )
        remaining = json.loads(self.state.read_text(encoding="utf-8"))
        self.assertNotIn(TARGET_ID, remaining)
        self.assertIn(UNRELATED_ID, remaining)
        actions = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertIn(["container", "kill", TARGET_ID], actions)
        self.assertNotIn(["container", "kill", UNRELATED_ID], actions)
        self.assertNotIn(["container", "rm", "--force", UNRELATED_ID], actions)

    def test_label_mismatch_is_never_killed(self) -> None:
        self.state.write_text(
            json.dumps({TARGET_ID: container(TARGET_ID, TARGET_NAME, "unrelated-project")}),
            encoding="utf-8",
        )
        result, raw_record = self.invoke(
            50, [sys.executable, "-c", "import time; time.sleep(10)"]
        )

        self.assertEqual(result.returncode, 125, result.stderr)
        marker = json.loads(raw_record)
        self.assertEqual(marker["status"], "error")
        self.assertIsNone(marker["container_id"])
        self.assertIn(TARGET_ID, json.loads(self.state.read_text(encoding="utf-8")))
        actions = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertFalse(any(action[:2] == ["container", "kill"] for action in actions))
        self.assertFalse(any(action[:2] == ["container", "rm"] for action in actions))

    def test_fast_command_returns_its_status_and_removes_only_its_container(self) -> None:
        self.state.write_text(
            json.dumps(
                {
                    TARGET_ID: container(TARGET_ID, TARGET_NAME, PROJECT),
                    UNRELATED_ID: container(
                        UNRELATED_ID, "unrelated-container", "other-project"
                    ),
                }
            ),
            encoding="utf-8",
        )
        result, raw_record = self.invoke(
            1_000, [sys.executable, "-c", "raise SystemExit(7)"]
        )

        self.assertEqual(result.returncode, 7, result.stderr)
        marker = json.loads(raw_record)
        self.assertEqual(marker["status"], "complete")
        self.assertEqual(marker["child_exit_status"], 7)
        self.assertEqual(marker["container_id"], TARGET_ID)
        self.assertLessEqual(marker["elapsed_wall_ms"], 1_000)
        remaining = json.loads(self.state.read_text(encoding="utf-8"))
        self.assertNotIn(TARGET_ID, remaining)
        self.assertIn(UNRELATED_ID, remaining)
        actions = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertFalse(any(action[:2] == ["container", "kill"] for action in actions))
        self.assertIn(["container", "rm", "--force", TARGET_ID], actions)
        self.assertNotIn(["container", "rm", "--force", UNRELATED_ID], actions)

    def test_completed_child_without_observed_identity_fails_closed(self) -> None:
        self.state.write_text("{}", encoding="utf-8")
        result, raw_record = self.invoke(
            1_000, [sys.executable, "-c", "raise SystemExit(0)"]
        )

        self.assertEqual(result.returncode, 125, result.stderr)
        marker = json.loads(raw_record)
        self.assertEqual(marker["status"], "error")
        self.assertIsNone(marker["container_id"])
        self.assertIn("before its immutable container identity", result.stderr)
        actions = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertFalse(any(action[:2] == ["container", "kill"] for action in actions))
        self.assertFalse(any(action[:2] == ["container", "rm"] for action in actions))


if __name__ == "__main__":
    unittest.main()
