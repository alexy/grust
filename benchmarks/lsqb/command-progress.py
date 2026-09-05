#!/usr/bin/env python3
"""Retain build/test output with durable progress, without a guessed job timeout.

Use for non-measured setup work, not benchmark query supervision. Output and
progress files are exclusive, so retries cannot overwrite earlier evidence.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import time


def run(command, output, heartbeat=30):
    if not command or not 0 < heartbeat <= 60:
        raise ValueError('command and heartbeat in (0, 60] seconds required')
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

        process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT,
                                   start_new_session=True)
        try:
            event('command-start', pid=process.pid)
            while True:
                try:
                    status = process.wait(timeout=heartbeat)
                    break
                except subprocess.TimeoutExpired:
                    event('command-progress', **progress())
        except BaseException:
            # Cancel only the process group created by this wrapper. Reap the
            # child before returning; interruption must not leave a build live.
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
            event('command-interrupted', exit=process.returncode, **progress())
            raise
        event('command-finish', exit=status, **progress())
        return status


if __name__ == '__main__':
    def interrupted(_signal, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--heartbeat-seconds', type=float, default=30)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    raise SystemExit(run(command, args.output, args.heartbeat_seconds))
