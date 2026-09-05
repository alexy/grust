#!/usr/bin/env python3
"""Run one pre-created Docker diagnostic and retain its bounded state evidence.

This helper never creates, discovers by name, or pulls a container.  Its caller
supplies one immutable stopped-container ID.  The helper attests that container,
runs only ``docker container start --attach``, records the stopped result, and
then removes exactly that ID.  Its records are diagnostics, never publication
evidence.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
from types import ModuleType
from typing import Any


SCHEMA = "grust-lsqb-docker-diagnostic-command-v1"
ERROR_EXIT = 125
MEMORY_BYTES = 6 * 1024**3
NANO_CPUS = 8_000_000_000
IMAGE_ID = re.compile(r"^sha256:[0-9a-f]{64}$")


class DiagnosticError(RuntimeError):
    """The requested command could not be proved to satisfy its contract."""


def load_watchdog() -> ModuleType:
    path = Path(__file__).with_name("cell-watchdog.py")
    spec = importlib.util.spec_from_file_location("grust_lsqb_cell_watchdog", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load watchdog helpers from {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules.setdefault(spec.name, module)
    spec.loader.exec_module(module)
    return module


WATCHDOG = load_watchdog()


def exact_int(value: Any, label: str) -> int:
    if type(value) is not int:
        raise DiagnosticError(f"container {label} is not an integer")
    return value


def exact_bool(value: Any, label: str) -> bool:
    if type(value) is not bool:
        raise DiagnosticError(f"container {label} is not a boolean")
    return value


def mapping(record: dict[str, Any], key: str) -> dict[str, Any]:
    value = record.get(key)
    if not isinstance(value, dict):
        raise DiagnosticError(f"container inspect has no object {key}")
    return value


def resource_snapshot(record: dict[str, Any]) -> dict[str, Any]:
    host = mapping(record, "HostConfig")
    restart = host.get("RestartPolicy")
    if not isinstance(restart, dict):
        raise DiagnosticError("container restart policy is not an object")
    resources = {
        "auto_remove": exact_bool(host.get("AutoRemove"), "AutoRemove"),
        "cpuset_cpus": host.get("CpusetCpus"),
        "init": exact_bool(host.get("Init"), "Init"),
        "memory": exact_int(host.get("Memory"), "Memory"),
        "memory_swap": exact_int(host.get("MemorySwap"), "MemorySwap"),
        "nano_cpus": exact_int(host.get("NanoCpus"), "NanoCpus"),
        "network_mode": host.get("NetworkMode"),
        "readonly_rootfs": exact_bool(host.get("ReadonlyRootfs"), "ReadonlyRootfs"),
        "restart_maximum_retry_count": exact_int(
            restart.get("MaximumRetryCount"), "RestartPolicy.MaximumRetryCount"
        ),
        "restart_name": restart.get("Name"),
    }
    if not isinstance(resources["cpuset_cpus"], str):
        raise DiagnosticError("container CpusetCpus is not a string")
    if not isinstance(resources["network_mode"], str):
        raise DiagnosticError("container NetworkMode is not a string")
    if not isinstance(resources["restart_name"], str):
        raise DiagnosticError("container restart policy name is not a string")
    return resources


def state_snapshot(record: dict[str, Any]) -> dict[str, Any]:
    state = mapping(record, "State")
    snapshot = {
        "dead": exact_bool(state.get("Dead"), "State.Dead"),
        "error": state.get("Error"),
        "exit_code": exact_int(state.get("ExitCode"), "State.ExitCode"),
        "finished_at": state.get("FinishedAt"),
        "oom_killed": exact_bool(state.get("OOMKilled"), "State.OOMKilled"),
        "paused": exact_bool(state.get("Paused"), "State.Paused"),
        "pid": exact_int(state.get("Pid"), "State.Pid"),
        "restarting": exact_bool(state.get("Restarting"), "State.Restarting"),
        "running": exact_bool(state.get("Running"), "State.Running"),
        "started_at": state.get("StartedAt"),
        "status": state.get("Status"),
    }
    for key in ("error", "finished_at", "started_at", "status"):
        if not isinstance(snapshot[key], str):
            raise DiagnosticError(f"container State.{key} is not a string")
    return snapshot


def snapshot(record: dict[str, Any], arguments: argparse.Namespace) -> dict[str, Any]:
    """Return an allowlisted view; never retain environment or raw inspect data."""
    return {
        "container_id": record["Id"],
        "container_name": arguments.container,
        "image_id": record.get("Image"),
        "labels": {
            "com.docker.compose.project": arguments.project,
            "com.docker.compose.service": arguments.service,
        },
        "resources": resource_snapshot(record),
        "state": state_snapshot(record),
    }


def attest_common(record: dict[str, Any], arguments: argparse.Namespace) -> None:
    if record.get("Id") != arguments.container_id:
        raise DiagnosticError("container immutable ID differs")
    if record.get("Image") != arguments.image_id:
        raise DiagnosticError("container image ID differs")
    resources = resource_snapshot(record)
    expected = {
        "auto_remove": False,
        "cpuset_cpus": "",
        "init": True,
        "memory": MEMORY_BYTES,
        "memory_swap": MEMORY_BYTES,
        "nano_cpus": NANO_CPUS,
        "readonly_rootfs": True,
        "restart_maximum_retry_count": 0,
        "restart_name": "no",
    }
    for key, value in expected.items():
        if resources[key] != value:
            raise DiagnosticError(
                f"container resource {key} differs: expected {value!r}, "
                f"got {resources[key]!r}"
            )


def attest_created(record: dict[str, Any], arguments: argparse.Namespace) -> None:
    attest_common(record, arguments)
    state = state_snapshot(record)
    expected = {
        "dead": False,
        "exit_code": 0,
        "oom_killed": False,
        "paused": False,
        "pid": 0,
        "restarting": False,
        "running": False,
        "status": "created",
    }
    for key, value in expected.items():
        if state[key] != value:
            raise DiagnosticError(
                f"container pre-start state {key} differs: expected {value!r}, "
                f"got {state[key]!r}"
            )


def attest_exited(
    record: dict[str, Any], arguments: argparse.Namespace, child_exit_status: int
) -> None:
    attest_common(record, arguments)
    state = state_snapshot(record)
    expected = {
        "dead": False,
        "paused": False,
        "pid": 0,
        "restarting": False,
        "running": False,
        "status": "exited",
    }
    for key, value in expected.items():
        if state[key] != value:
            raise DiagnosticError(
                f"container post-run state {key} differs: expected {value!r}, "
                f"got {state[key]!r}"
            )
    if state["oom_killed"]:
        raise DiagnosticError("container was OOM-killed")
    if state["exit_code"] != child_exit_status:
        raise DiagnosticError(
            "docker start exit status differs from the container exit code"
        )
    if child_exit_status != 0:
        raise DiagnosticError(f"container command exited with status {child_exit_status}")


def normalized_json(value: dict[str, Any]) -> bytes:
    return (
        json.dumps(
            value,
            allow_nan=False,
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        )
        + "\n"
    ).encode("utf-8")


def save_record(directory: Path, name: str, value: dict[str, Any]) -> None:
    try:
        with (directory / name).open("xb", buffering=0) as stream:
            content = normalized_json(value)
            if stream.write(content) != len(content):
                raise DiagnosticError(f"short write for {name}")
            os.fsync(stream.fileno())
        descriptor = os.open(directory, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    except OSError as error:
        raise DiagnosticError(f"cannot retain {name}: {error}") from error


def inspect_exact(arguments: argparse.Namespace) -> dict[str, Any]:
    record = WATCHDOG.inspect_container_record(
        arguments.container_id,
        arguments.container,
        arguments.project,
        arguments.service,
    )
    if record is None:
        raise DiagnosticError("pinned container is absent")
    return record


def cleanup_exact(
    arguments: argparse.Namespace, process: subprocess.Popen[bytes] | None
) -> tuple[dict[str, Any] | None, str | None]:
    """Stop the attach client, snapshot the exact ID, then remove only that ID."""
    failures: list[str] = []
    if process is not None:
        try:
            WATCHDOG.stop_process_group(process)
        except WATCHDOG.WatchdogError as error:
            failures.append(str(error))
    record: dict[str, Any] | None = None
    try:
        record = WATCHDOG.inspect_container_record(
            arguments.container_id,
            arguments.container,
            arguments.project,
            arguments.service,
        )
    except WATCHDOG.WatchdogError as error:
        failures.append(f"cleanup identity check failed: {error}")
        return None, "; ".join(failures)
    if record is None:
        failures.append("pinned container disappeared before cleanup")
        return None, "; ".join(failures)
    retained: dict[str, Any] | None = None
    try:
        retained = snapshot(record, arguments)
    except DiagnosticError as error:
        failures.append(str(error))
    try:
        state = state_snapshot(record)
    except DiagnosticError as error:
        failures.append(str(error))
        state = None
    if state is not None and state["running"]:
        try:
            WATCHDOG.kill_exact_container(
                arguments.container_id,
                arguments.container,
                arguments.project,
                arguments.service,
            )
        except WATCHDOG.WatchdogError as error:
            failures.append(str(error))
    try:
        WATCHDOG.remove_exact_container(
            arguments.container_id,
            arguments.container,
            arguments.project,
            arguments.service,
        )
    except WATCHDOG.WatchdogError as error:
        failures.append(str(error))
    return retained, "; ".join(failures) or None


def terminal_record(
    arguments: argparse.Namespace,
    started_ns: int,
    *,
    child_exit_status: int | None,
    error: str | None,
    status: str,
) -> dict[str, Any]:
    return {
        "child_exit_status": child_exit_status,
        "container_id": arguments.container_id,
        "container_name": arguments.container,
        "elapsed_wall_ms": (time.monotonic_ns() - started_ns + 999_999) // 1_000_000,
        "error": error,
        "event": "terminal",
        "project": arguments.project,
        "publication_eligible": False,
        "schema": SCHEMA,
        "service": arguments.service,
        "status": status,
    }


def run(
    arguments: argparse.Namespace,
    interruption: Any,
) -> int:
    started_ns = time.monotonic_ns()
    try:
        arguments.output.mkdir(exist_ok=False)
    except OSError as error:
        raise DiagnosticError(f"cannot create fresh records directory: {error}") from error
    save_record(
        arguments.output,
        "invocation.json",
        {
            "container_id": arguments.container_id,
            "container_name": arguments.container,
            "expected_image_id": arguments.image_id,
            "project": arguments.project,
            "publication_eligible": False,
            "schema": SCHEMA,
            "service": arguments.service,
        },
    )
    process: subprocess.Popen[bytes] | None = None
    child_exit_status: int | None = None
    error: str | None = None
    status = "error"
    result = ERROR_EXIT
    pinned = False
    unexpected: BaseException | None = None
    try:
        interruption.checkpoint()
        before = inspect_exact(arguments)
        attest_created(before, arguments)
        pinned = True
        save_record(arguments.output, "container-before.json", snapshot(before, arguments))
        interruption.checkpoint()
        process = subprocess.Popen(
            ["docker", "container", "start", "--attach", arguments.container_id],
            start_new_session=True,
        )
        interruption.checkpoint()
        while True:
            interruption.checkpoint()
            try:
                child_exit_status = process.wait(timeout=1.0)
                break
            except subprocess.TimeoutExpired:
                continue
        interruption.checkpoint()
        after = inspect_exact(arguments)
        save_record(arguments.output, "container-after.json", snapshot(after, arguments))
        attest_exited(after, arguments, child_exit_status)
        status = "complete"
        result = 0
    except WATCHDOG.WatchdogInterrupted as caught:
        status = "interrupted"
        error = str(caught)
        result = min(255, 128 + caught.signal_number)
    except (DiagnosticError, WATCHDOG.WatchdogError, OSError) as caught:
        error = str(caught)
    except BaseException as caught:
        error = f"unexpected {type(caught).__name__}: {caught}"
        unexpected = caught
    cleanup_snapshot: dict[str, Any] | None = None
    cleanup_error: str | None = None
    if pinned:
        cleanup_snapshot, cleanup_error = cleanup_exact(arguments, process)
        after_path = arguments.output / "container-after.json"
        if cleanup_snapshot is not None and not after_path.exists():
            save_record(arguments.output, after_path.name, cleanup_snapshot)
    if cleanup_error is not None:
        status = "error"
        result = ERROR_EXIT
        error = f"{error}; cleanup failed: {cleanup_error}" if error else (
            f"cleanup failed: {cleanup_error}"
        )
    try:
        interruption.checkpoint()
    except WATCHDOG.WatchdogInterrupted as caught:
        if status == "complete":
            status = "interrupted"
            result = min(255, 128 + caught.signal_number)
            error = str(caught)
    terminal = terminal_record(
        arguments,
        started_ns,
        child_exit_status=child_exit_status,
        error=error,
        status=status,
    )
    save_record(arguments.output, "completion.json", terminal)
    print(normalized_json(terminal).decode("utf-8"), end="", flush=True)
    if unexpected is not None:
        raise unexpected
    return result


def parse_arguments(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--container-id", required=True)
    parser.add_argument("--container", required=True)
    parser.add_argument("--project", required=True)
    parser.add_argument("--service", required=True)
    parser.add_argument("--image-id", required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args(argv)
    if WATCHDOG.CONTAINER_ID.fullmatch(arguments.container_id) is None:
        parser.error("--container-id must be a full lowercase Docker ID")
    if WATCHDOG.CONTAINER_NAME.fullmatch(arguments.container) is None:
        parser.error("--container is not a safe Docker container name")
    if WATCHDOG.CONTAINER_NAME.fullmatch(arguments.project) is None:
        parser.error("--project is not a safe Compose project label")
    if WATCHDOG.CONTAINER_NAME.fullmatch(arguments.service) is None:
        parser.error("--service is not a safe Compose service label")
    if IMAGE_ID.fullmatch(arguments.image_id) is None:
        parser.error("--image-id must be an immutable sha256 Docker image ID")
    return arguments


def main(argv: list[str] | None = None) -> int:
    try:
        arguments = parse_arguments(argv)
        with WATCHDOG.controlled_interruption_signals() as interruption:
            return run(arguments, interruption)
    except (DiagnosticError, WATCHDOG.WatchdogError) as error:
        print(f"docker-diagnostic-command.py: {error}", file=sys.stderr)
        return ERROR_EXIT


if __name__ == "__main__":
    raise SystemExit(main())
