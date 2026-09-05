#!/usr/bin/env python3
"""Run one named Compose cell with a hard, narrowly scoped wall-clock limit."""

from __future__ import annotations

import argparse
import json
import os
import queue
import re
import signal
import subprocess
import sys
import threading
import time
from typing import Any, Callable


CONTAINER_ID = re.compile(r"^[0-9a-f]{64}$")
CONTAINER_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]*$")
WATCHDOG_TIMEOUT_EXIT = 124
WATCHDOG_ERROR_EXIT = 125
COMPLETION_SCHEMA = "grust-lsqb-cell-watchdog-completion-v1"
DEFAULT_HEARTBEAT_MS = 30_000
ATOMIC_PROGRESS_BYTES = 512


class HeartbeatEmitter:
    """Drop-on-backpressure heartbeat delivery on a disposable daemon thread."""

    _STOP = object()

    def __init__(
        self,
        descriptor: int,
        *,
        write_once: Callable[[int, bytes], int] = os.write,
        owns_descriptor: bool = False,
    ) -> None:
        self._descriptor = descriptor
        self._write_once = write_once
        self._owns_descriptor = owns_descriptor
        self._messages: queue.Queue[bytes | object] = queue.Queue(maxsize=1)
        self._outstanding = threading.Semaphore(1)
        self._stopping = threading.Event()
        self._worker = threading.Thread(
            target=self._write_messages,
            name="grust-lsqb-heartbeat",
            daemon=True,
        )
        self._worker.start()

    @classmethod
    def for_stderr(cls) -> HeartbeatEmitter | None:
        """Duplicate stderr without changing flags shared with the child process."""
        try:
            descriptor = os.dup(sys.stderr.fileno())
        except (AttributeError, OSError, ValueError):
            return None
        try:
            return cls(descriptor, owns_descriptor=True)
        except Exception:
            try:
                os.close(descriptor)
            except OSError:
                pass
            return None

    def submit(self, content: bytes) -> bool:
        """Accept one queued-or-writing line without waiting for the output sink."""
        if (
            self._stopping.is_set()
            or not content.endswith(b"\n")
            or len(content) > ATOMIC_PROGRESS_BYTES
        ):
            return False
        if not self._outstanding.acquire(blocking=False):
            return False
        if self._stopping.is_set():
            self._outstanding.release()
            return False
        try:
            self._messages.put_nowait(content)
        except queue.Full:
            self._outstanding.release()
            return False
        except Exception:
            self._outstanding.release()
            self._stopping.set()
            return False
        return True

    def close(self) -> None:
        """Request daemon shutdown without joining or closing a possibly blocked fd."""
        self._stopping.set()
        try:
            self._messages.put_nowait(self._STOP)
        except Exception:
            pass

    def _write_messages(self) -> None:
        try:
            while True:
                content = self._messages.get()
                if content is self._STOP:
                    return
                try:
                    if self._stopping.is_set():
                        return
                    # At this producer fd, POSIX guarantees non-interleaving
                    # blocking pipe writes through PIPE_BUF; 512 bytes is its
                    # portable minimum. Never retry an exotic sink's short
                    # write because a second write could interleave.
                    written = self._write_once(self._descriptor, content)
                    if written != len(content):
                        self._stopping.set()
                        return
                except OSError:
                    self._stopping.set()
                    return
                finally:
                    self._outstanding.release()
        finally:
            if self._owns_descriptor:
                try:
                    os.close(self._descriptor)
                except OSError:
                    pass


class WatchdogError(RuntimeError):
    """The watchdog could not prove that a Docker action was narrowly scoped."""


def docker_command(arguments: list[str], timeout: float = 5.0) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            ["docker", *arguments],
            check=False,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise WatchdogError(f"Docker command failed: {error}") from error


def inspect_container(name_or_id: str, name: str, project: str, service: str) -> str | None:
    result = docker_command(["container", "inspect", name_or_id])
    if result.returncode != 0:
        detail = f"{result.stdout}\n{result.stderr}".lower()
        if "no such object" in detail or "no such container" in detail:
            return None
        raise WatchdogError(f"Docker inspect failed: {result.stderr.strip()}")
    try:
        records: Any = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise WatchdogError("Docker inspect returned invalid JSON") from error
    if not isinstance(records, list) or len(records) != 1 or not isinstance(records[0], dict):
        raise WatchdogError("Docker inspect did not return exactly one container")
    record = records[0]
    container_id = record.get("Id")
    labels = record.get("Config", {}).get("Labels", {})
    if (
        not isinstance(container_id, str)
        or CONTAINER_ID.fullmatch(container_id) is None
        or record.get("Name") != f"/{name}"
        or not isinstance(labels, dict)
        or labels.get("com.docker.compose.project") != project
        or labels.get("com.docker.compose.service") != service
    ):
        raise WatchdogError(
            "refusing to act on a container without the exact expected name and Compose labels"
        )
    return container_id


def stop_process_group(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=2.0)
        return
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        return
    process.wait(timeout=2.0)


def kill_exact_container(container_id: str, name: str, project: str, service: str) -> None:
    current = inspect_container(container_id, name, project, service)
    if current is None:
        return
    if current != container_id:
        raise WatchdogError("container identity changed before watchdog kill")
    killed = docker_command(["container", "kill", container_id], timeout=10.0)
    if killed.returncode != 0 and inspect_container(container_id, name, project, service) is not None:
        raise WatchdogError(f"failed to kill timed-out container: {killed.stderr.strip()}")


def remove_exact_container(container_id: str, name: str, project: str, service: str) -> None:
    if inspect_container(container_id, name, project, service) is None:
        return
    removed = docker_command(["container", "rm", "--force", container_id], timeout=10.0)
    if removed.returncode != 0 and inspect_container(container_id, name, project, service) is not None:
        raise WatchdogError(f"failed to remove timed-out container: {removed.stderr.strip()}")


def elapsed_milliseconds(started_ns: int, observed_ns: int | None = None) -> int:
    if observed_ns is None:
        observed_ns = time.monotonic_ns()
    elapsed_ns = max(0, observed_ns - started_ns)
    return (elapsed_ns + 999_999) // 1_000_000


def completion_is_timely(
    child_exit_status: int | None, observed_ns: int, deadline_ns: int
) -> bool:
    return child_exit_status is not None and observed_ns < deadline_ns


def next_heartbeat_deadline(
    scheduled_ns: int, observed_ns: int, interval_ns: int
) -> int:
    """Advance to the first heartbeat deadline strictly after an observation."""
    if observed_ns < scheduled_ns:
        return scheduled_ns
    missed_intervals = (observed_ns - scheduled_ns) // interval_ns + 1
    return scheduled_ns + missed_intervals * interval_ns


def heartbeat_line(
    container: str, started_ns: int, deadline_ns: int, observed_ns: int
) -> str:
    remaining_ns = max(0, deadline_ns - observed_ns)
    remaining_ms = remaining_ns // 1_000_000
    return (
        f"cell-watchdog.py: heartbeat container={container} "
        f"elapsed_ms={elapsed_milliseconds(started_ns, observed_ns)} "
        f"remaining_ms={remaining_ms}"
    )


def heartbeat_content(
    container: str, started_ns: int, deadline_ns: int, observed_ns: int
) -> bytes:
    return (
        heartbeat_line(container, started_ns, deadline_ns, observed_ns) + "\n"
    ).encode("utf-8")


def completion_record(
    arguments: argparse.Namespace,
    *,
    child_exit_status: int | None,
    container_id: str | None,
    elapsed_wall_ms: int,
    status: str,
) -> dict[str, Any]:
    return {
        "child_exit_status": child_exit_status,
        "container_id": container_id,
        "container_name": arguments.container,
        "elapsed_wall_ms": elapsed_wall_ms,
        "project": arguments.project,
        "schema": COMPLETION_SCHEMA,
        "service": arguments.service,
        "status": status,
        "timeout_ms": arguments.timeout_ms,
    }


def normalized_record(record: dict[str, Any]) -> bytes:
    return (
        json.dumps(
            record,
            allow_nan=False,
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        )
        + "\n"
    ).encode("utf-8")


def write_completion_record(descriptor: int, record: dict[str, Any]) -> None:
    content = normalized_record(record)
    duplicate: int | None = None
    try:
        duplicate = os.dup(descriptor)
        written = 0
        while written < len(content):
            count = os.write(duplicate, content[written:])
            if count <= 0:
                raise WatchdogError("could not write the completion record")
            written += count
        os.fsync(duplicate)
    except OSError as error:
        raise WatchdogError(f"could not write the completion record: {error}") from error
    finally:
        if duplicate is not None:
            os.close(duplicate)


def run(
    arguments: argparse.Namespace, heartbeat_emitter: HeartbeatEmitter | None = None
) -> tuple[int, dict[str, Any], str | None]:
    started_ns = time.monotonic_ns()
    command = list(arguments.command)
    if command and command[0] == "--":
        command.pop(0)
    if not command:
        record = completion_record(
            arguments,
            child_exit_status=None,
            container_id=None,
            elapsed_wall_ms=elapsed_milliseconds(started_ns),
            status="error",
        )
        return WATCHDOG_ERROR_EXIT, record, "no cell command was supplied"

    process: subprocess.Popen[bytes] | None = None
    container_id: str | None = None
    try:
        process = subprocess.Popen(command, start_new_session=True)
    except OSError as error:
        record = completion_record(
            arguments,
            child_exit_status=None,
            container_id=None,
            elapsed_wall_ms=elapsed_milliseconds(started_ns),
            status="error",
        )
        return WATCHDOG_ERROR_EXIT, record, f"cannot start cell command: {error}"
    deadline_ns = started_ns + arguments.timeout_ms * 1_000_000
    heartbeat_interval_ns = arguments.heartbeat_ms * 1_000_000
    next_heartbeat_ns = started_ns + heartbeat_interval_ns
    try:
        while True:
            if container_id is None:
                container_id = inspect_container(
                    arguments.container,
                    arguments.container,
                    arguments.project,
                    arguments.service,
                )
            remaining_ns = deadline_ns - time.monotonic_ns()
            if remaining_ns <= 0:
                break
            child_exit_status = process.poll()
            completion_observed_ns = time.monotonic_ns()
            if child_exit_status is not None and completion_observed_ns >= deadline_ns:
                break
            if completion_is_timely(
                child_exit_status, completion_observed_ns, deadline_ns
            ):
                if container_id is None:
                    raise WatchdogError(
                        "cell command exited before its immutable container identity was observed"
                    )
                remove_exact_container(
                    container_id,
                    arguments.container,
                    arguments.project,
                    arguments.service,
                )
                record = completion_record(
                    arguments,
                    child_exit_status=child_exit_status,
                    container_id=container_id,
                    elapsed_wall_ms=elapsed_milliseconds(
                        started_ns, completion_observed_ns
                    ),
                    status="complete",
                )
                return child_exit_status, record, None
            if completion_observed_ns >= next_heartbeat_ns:
                if heartbeat_emitter is not None:
                    heartbeat_emitter.submit(
                        heartbeat_content(
                            arguments.container,
                            started_ns,
                            deadline_ns,
                            completion_observed_ns,
                        )
                    )
                next_heartbeat_ns = next_heartbeat_deadline(
                    next_heartbeat_ns,
                    completion_observed_ns,
                    heartbeat_interval_ns,
                )
            time.sleep(min(0.1, remaining_ns / 1_000_000_000))
    except WatchdogError as error:
        stop_process_group(process)
        record = completion_record(
            arguments,
            child_exit_status=process.returncode,
            container_id=container_id,
            elapsed_wall_ms=elapsed_milliseconds(started_ns),
            status="error",
        )
        return WATCHDOG_ERROR_EXIT, record, str(error)
    except BaseException:
        stop_process_group(process)
        raise

    try:
        try:
            if container_id is not None and process.poll() is None:
                kill_exact_container(
                    container_id, arguments.container, arguments.project, arguments.service
                )
            stop_process_group(process)
            if container_id is None:
                container_id = inspect_container(
                    arguments.container,
                    arguments.container,
                    arguments.project,
                    arguments.service,
                )
                if container_id is not None:
                    kill_exact_container(
                        container_id,
                        arguments.container,
                        arguments.project,
                        arguments.service,
                    )
            if container_id is not None:
                remove_exact_container(
                    container_id, arguments.container, arguments.project, arguments.service
                )
        finally:
            stop_process_group(process)
    except WatchdogError as error:
        record = completion_record(
            arguments,
            child_exit_status=process.returncode,
            container_id=container_id,
            elapsed_wall_ms=elapsed_milliseconds(started_ns),
            status="error",
        )
        return WATCHDOG_ERROR_EXIT, record, str(error)
    record = completion_record(
        arguments,
        child_exit_status=process.returncode,
        container_id=container_id,
        elapsed_wall_ms=elapsed_milliseconds(started_ns),
        status="timeout",
    )
    return WATCHDOG_TIMEOUT_EXIT, record, None


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout-ms", required=True, type=int)
    parser.add_argument("--heartbeat-ms", type=int, default=DEFAULT_HEARTBEAT_MS)
    parser.add_argument("--container", required=True)
    parser.add_argument("--project", required=True)
    parser.add_argument("--service", required=True)
    parser.add_argument("--record-fd", required=True, type=int)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    arguments = parser.parse_args()
    if arguments.timeout_ms <= 0:
        parser.error("--timeout-ms must be greater than zero")
    if arguments.heartbeat_ms <= 0:
        parser.error("--heartbeat-ms must be greater than zero")
    if CONTAINER_NAME.fullmatch(arguments.container) is None:
        parser.error("--container is not a safe Docker container name")
    if CONTAINER_NAME.fullmatch(arguments.project) is None:
        parser.error("--project is not a safe Docker label value")
    if CONTAINER_NAME.fullmatch(arguments.service) is None:
        parser.error("--service is not a safe Docker label value")
    if arguments.record_fd < 3:
        parser.error("--record-fd must identify an inherited non-standard descriptor")
    return arguments


def main() -> int:
    try:
        arguments = parse_arguments()
        heartbeat_emitter = HeartbeatEmitter.for_stderr()
        try:
            status, record, error = run(arguments, heartbeat_emitter)
        finally:
            if heartbeat_emitter is not None:
                heartbeat_emitter.close()
        write_completion_record(arguments.record_fd, record)
        if record["status"] == "timeout":
            print(normalized_record(record).decode("utf-8"), end="", file=sys.stderr, flush=True)
        if error is not None:
            print(f"cell-watchdog.py: {error}", file=sys.stderr)
        return status
    except WatchdogError as error:
        print(f"cell-watchdog.py: {error}", file=sys.stderr)
        return WATCHDOG_ERROR_EXIT


if __name__ == "__main__":
    raise SystemExit(main())
