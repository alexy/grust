#!/usr/bin/env python3
"""Declare a matrix cell whose container exceeded its memory limit.

A cell whose container the kernel takes away under its per-container memory
limit leaves no runner inside it to write a component report. The launcher
calls this to record that outcome from the evidence it has — the cell
watchdog's retained completion record, which carries the container's own exit
code and OOM flag — rather than inferring one from a missing file.

This declares a host/budget outcome for one cell. It is not a component
report, asserts nothing about the backend beyond the cell's identity, and
never makes a run publishable: exit code 3 means the watchdog record does not
prove a memory termination, and the caller must treat the missing component as
fatal.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

SCHEMA = "grust-lsqb-cell-memory-exceeded-v1"
WATCHDOG_SCHEMA = "grust-lsqb-cell-watchdog-completion-v1"
# 128 + SIGKILL: the exit status a container gets when the kernel kills it.
OOM_EXIT_STATUS = 137
UNPROVEN_EXIT = 3
LIMITATION = (
    "The cell container was terminated by its memory limit before the runner "
    "wrote a component report; no query in this cell was observed."
)


def memory_termination_proof(record: object) -> dict[str, object] | None:
    """The container's retained exit, when it proves a memory termination."""
    if not isinstance(record, dict):
        return None
    termination = record.get("container_termination")
    if (
        record.get("schema") != WATCHDOG_SCHEMA
        or record.get("status") != "complete"
        or record.get("child_exit_status") != OOM_EXIT_STATUS
        or not isinstance(termination, dict)
        or set(termination) != {"exit_code", "oom_killed"}
        or termination.get("oom_killed") is not True
        or termination.get("exit_code") != OOM_EXIT_STATUS
    ):
        return None
    return record


def declaration(record: dict[str, object], arguments: argparse.Namespace) -> dict[str, object]:
    return {
        "schema": SCHEMA,
        "suite": arguments.suite,
        "backend": arguments.backend,
        "scale": arguments.scale,
        "component": arguments.component,
        "runner_image": arguments.runner_image,
        "runner_image_id": arguments.runner_image_id,
        "memory_limit_bytes": arguments.memory_limit_bytes,
        "cell_timeout_ms": arguments.cell_timeout_ms,
        "watchdog": record,
        "declared_by": "run-grust.sh",
        "publication_qualified": False,
        "limitation": LIMITATION,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--watchdog", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--suite", required=True)
    parser.add_argument("--backend", required=True)
    parser.add_argument("--scale", required=True)
    parser.add_argument("--component", required=True)
    parser.add_argument("--runner-image", required=True)
    parser.add_argument("--runner-image-id", required=True)
    parser.add_argument("--memory-limit-bytes", required=True, type=int)
    parser.add_argument("--cell-timeout-ms", required=True, type=int)
    arguments = parser.parse_args(argv)

    watchdog = Path(arguments.watchdog)
    output = Path(arguments.output)
    if watchdog.is_symlink() or not watchdog.is_file():
        print("watchdog completion record is not a regular file", file=sys.stderr)
        return 2
    try:
        record = json.loads(watchdog.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        print(f"cannot read the watchdog completion record: {error}", file=sys.stderr)
        return 2
    if memory_termination_proof(record) is None:
        print(
            "the cell watchdog record does not prove a container memory termination",
            file=sys.stderr,
        )
        return UNPROVEN_EXIT
    if output.exists() or output.is_symlink():
        print(f"refusing to overwrite an existing declaration: {output}", file=sys.stderr)
        return 2
    content = json.dumps(declaration(record, arguments), sort_keys=True, indent=2) + "\n"
    try:
        with open(output, "x", encoding="utf-8") as handle:
            handle.write(content)
    except OSError as error:
        print(f"cannot write the declaration: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
