#!/usr/bin/env python3
"""Retain build/test output with durable progress, without a guessed job timeout.

Use for non-measured setup work, not benchmark query supervision. Output and
progress files are exclusive, so retries cannot overwrite earlier evidence.
"""
import argparse
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import time


class InterruptionSignals:
    """Latch cancellation while spawning or cleaning up the owned child."""

    def __init__(self):
        self.pending = False
        self.interruptible = False
        self.previous = {}

    def __enter__(self):
        try:
            for kind in (signal.SIGINT, signal.SIGTERM):
                self.previous[kind] = signal.signal(kind, self.handle)
        except BaseException:
            self.restore()
            raise
        return self

    def __exit__(self, exception_type, *_exception):
        self.interruptible = False
        self.restore()
        if exception_type is None:
            # Cancellation during the final file closes must not become a
            # successful wrapper exit just because the child already finished.
            self.checkpoint()

    def restore(self):
        for kind, handler in self.previous.items():
            signal.signal(kind, handler)

    def handle(self, _kind, _frame):
        self.pending = True
        if self.interruptible:
            # Disarm before raising: another signal must not interrupt the
            # finally blocks that transfer control to child cleanup.
            self.interruptible = False
            raise KeyboardInterrupt

    def checkpoint(self):
        if self.pending:
            self.interruptible = False
            raise KeyboardInterrupt


def stop_process_group(process, termination_grace):
    """Stop only our new-session group, including children of an exited leader."""
    try:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=termination_grace)
        except subprocess.TimeoutExpired:
            pass
    finally:
        # A reaped leader does not imply its group is gone. Give a live leader
        # its cleanup allowance, then kill residual descendants even when that
        # leader returned early. Never discover or signal unrelated processes.
        try:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        finally:
            process.wait(timeout=5)


def run(command, output, heartbeat=30, termination_grace=5):
    if not command or not 0 < heartbeat <= 60:
        raise ValueError('command and heartbeat in (0, 60] seconds required')
    if not math.isfinite(termination_grace) or not 0 < termination_grace <= 60:
        raise ValueError('termination grace must be finite and in (0, 60] seconds')
    with InterruptionSignals() as interruptions:
        return _run(command, output, heartbeat, termination_grace, interruptions)


def _run(command, output, heartbeat, termination_grace, interruptions):
    output.mkdir(exist_ok=False)
    started = time.monotonic()
    with (output / 'command.log').open('xb', buffering=0) as log, \
            (output / 'progress.jsonl').open('x') as journal:
        def event(kind, **fields):
            record = dict(event=kind, elapsed_s=round(time.monotonic() - started, 3), **fields)
            line = json.dumps(record) + '\n'
            journal.write(line)
            journal.flush()
            os.fsync(journal.fileno())
            print(line, end='', flush=True)

        def progress():
            os.fsync(log.fileno())
            size = os.fstat(log.fileno()).st_size
            with (output / 'command.log').open('rb') as reader:
                reader.seek(max(0, size - 2048))
                lines = reader.read().decode(errors='replace').splitlines()
            return dict(log_bytes=size, latest_output=lines[-1] if lines else '')

        process = None
        try:
            interruptions.checkpoint()
            # Handlers only latch here. Popen must return its child handle
            # before a pending interruption can begin narrowly owned cleanup.
            process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT,
                                       start_new_session=True)
            try:
                interruptions.interruptible = True
                event('command-start', pid=process.pid)
                interruptions.checkpoint()
                while True:
                    try:
                        status = process.wait(timeout=heartbeat)
                        break
                    except subprocess.TimeoutExpired:
                        event('command-progress', **progress())
            finally:
                interruptions.interruptible = False
            interruptions.checkpoint()
            event('command-finish', exit=status, **progress())
            interruptions.checkpoint()
            return status
        except BaseException:
            # Cancel only the process group created by this wrapper. Reap the
            # child before returning; interruption must not leave a build live.
            try:
                if process is not None:
                    stop_process_group(process, termination_grace)
            finally:
                event('command-interrupted',
                      pid=process.pid if process is not None else None,
                      exit=process.returncode if process is not None else None,
                      termination_grace_seconds=termination_grace, **progress())
            raise


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--heartbeat-seconds', type=float, default=30)
    parser.add_argument('--termination-grace-seconds', type=float, default=5,
                        help='SIGTERM cleanup allowance before SIGKILL (default 5, max 60)')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    return run(command, args.output, args.heartbeat_seconds, args.termination_grace_seconds)


if __name__ == '__main__':
    raise SystemExit(main())
