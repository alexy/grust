#!/usr/bin/env python3
"""Focused tests for the narrowly scoped LSQB cell watchdog."""

from __future__ import annotations

import json
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


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
        self, timeout_ms: int, command: list[str]
    ) -> tuple[subprocess.CompletedProcess[str], bytes]:
        record_path = self.root / "completion.json"
        with record_path.open("wb") as record:
            result = subprocess.run(
                [
                    sys.executable,
                    str(WATCHDOG),
                    "--timeout-ms",
                    str(timeout_ms),
                    "--container",
                    TARGET_NAME,
                    "--project",
                    PROJECT,
                    "--service",
                    SERVICE,
                    "--record-fd",
                    str(record.fileno()),
                    "--",
                    *command,
                ],
                check=False,
                capture_output=True,
                text=True,
                env=self.environment,
                pass_fds=(record.fileno(),),
                timeout=5,
            )
        return result, record_path.read_bytes()

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
