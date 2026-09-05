#!/usr/bin/env python3
"""Retain a conservative host-CPU preflight; never stop or modify processes.

Passing is only a startup screen, not proof of isolation during a benchmark.
CPU percentages are ps estimates, with 100 percent representing one CPU core.
Command arguments and environment variables are deliberately not collected.
"""
import argparse
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import subprocess
import time


def assess(raw):
    processes = []
    for line in raw.splitlines():
        if not line.strip():
            continue
        pid, parent, cpu, executable = line.split(maxsplit=3)
        cpu = float(cpu)
        if not math.isfinite(cpu) or cpu < 0:
            raise ValueError('invalid process CPU estimate')
        processes.append(dict(pid=int(pid), parent_pid=int(parent), cpu_percent=cpu,
                              executable=Path(executable).name))
    if not processes:
        raise ValueError('empty process inventory')
    total = round(sum(p['cpu_percent'] for p in processes), 2)
    busy = [p for p in processes if p['cpu_percent'] >= 100]
    return dict(total_cpu_percent=total, busy_processes=busy,
                startup_screen_passed=not busy and total < 200)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    record = dict(schema='grust-host-preflight-v1', samples=[],
                  clean_host_performance_eligible=False,
                  limitation='startup screen only; ongoing contention monitoring required')
    # Refuse overwrite so earlier host evidence cannot be silently replaced.
    with args.output.open('x') as stream:
        try:
            for index in range(3):
                raw = subprocess.check_output(
                    ['ps', '-axo', 'pid=,ppid=,pcpu=,comm='], text=True, timeout=10)
                sample = assess(raw)
                sample['observed_at'] = datetime.now(timezone.utc).isoformat()
                record['samples'].append(sample)
                print(json.dumps(dict(event='host-preflight-progress', sample=index + 1,
                                      **sample)), flush=True)
                if index < 2:
                    time.sleep(1)
            record['startup_screen_passed'] = all(
                sample['startup_screen_passed'] for sample in record['samples'])
        except (ValueError, OSError, subprocess.SubprocessError):
            record['startup_screen_passed'] = False
            record['error'] = 'host process inventory unavailable or malformed'
        json.dump(record, stream, sort_keys=True)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())
    return 0 if record['startup_screen_passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
