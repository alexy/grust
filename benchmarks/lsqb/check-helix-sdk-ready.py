#!/usr/bin/env python3
"""Record private-network Helix SDK server readiness, not benchmark success."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('sdk', ROOT / 'run-sdk.py')
sdk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sdk)
CURL = 'curlimages/curl@sha256:463eaf6072688fe96ac64fa623fe73e1dbe25d8ad6c34404a669ad3ce1f104b6'


def probe_command(name, address):
    return ['docker', 'run', '--rm', '--name', name, '--network', sdk.NETWORK,
            '--label', 'io.adversarial.disposable=helix-sdk-readiness',
            '--cpus', '1', '--memory', '128m', '--memory-swap', '128m',
            '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
            CURL, '--fail-early', '--fail', '--silent', '--show-error', '--max-time', '3',
            f'http://{address}:8080/healthz', '--next', '--fail', '--silent',
            '--show-error', '--max-time', '3', f'http://{address}:8080/readyz']


def check(server_name, image, revision, output, deadline_s=120):
    if not server_name.startswith('grust-lsqb-helix-sdk-'):
        raise ValueError('select the dedicated Helix SDK server')
    if not 1 <= deadline_s <= 120:
        raise ValueError('readiness deadline must be between 1 and 120 seconds')
    server = sdk.runtime.inspect(server_name)
    address = sdk.validate_server(server, 'helix-sdk', image)
    sdk.validate_source_image(sdk.runtime.inspect(server['Image'], image=True), image, revision)
    network = json.loads(subprocess.check_output(
        ['docker', 'network', 'inspect', sdk.NETWORK], timeout=20))[0]
    if not network['Internal']:
        raise ValueError('SDK network must be internal')
    output.mkdir(exist_ok=False)
    name = f'grust-lsqb-helix-sdk-readiness-{os.getpid()}'
    command = probe_command(name, address)
    started = time.monotonic()
    with (output / 'readiness.jsonl').open('x') as log:
        def emit(event, **fields):
            record = dict(event=event, elapsed_s=round(time.monotonic() - started, 3), **fields)
            line = json.dumps(record) + '\n'
            log.write(line)
            log.flush()
            os.fsync(log.fileno())
            print(line, end='', flush=True)

        emit('readiness-start', server_id=server['Id'], server_image=image,
             source_revision=revision, network_id=network['Id'], command=command,
             deadline_s=deadline_s, benchmark_result=False)
        attempt = 0
        try:
            while time.monotonic() - started < deadline_s:
                attempt += 1
                remaining = deadline_s - (time.monotonic() - started)
                try:
                    result = subprocess.run(command, capture_output=True, text=True,
                                            timeout=max(0.001, min(15, remaining)))
                except subprocess.TimeoutExpired:
                    emit('readiness-probe-timeout', attempt=attempt)
                    break
                emit('readiness-probe', attempt=attempt, exit=result.returncode,
                     response=result.stdout[-4096:], detail=result.stderr[-4096:])
                current = sdk.runtime.inspect(server['Id'])
                sdk.validate_server(current, 'helix-sdk', image)
                if current['State']['StartedAt'] != server['State']['StartedAt']:
                    raise ValueError('server restarted during readiness')
                if result.returncode == 0:
                    emit('readiness-complete', ready=True, benchmark_result=False)
                    return True
                time.sleep(min(2, max(0, deadline_s - (time.monotonic() - started))))
            emit('readiness-complete', ready=False, benchmark_result=False)
            return False
        finally:
            # Only this invocation's temporary probe can be removed. Never stop
            # the selected service here; the caller retains its failure state.
            subprocess.run(['docker', 'rm', '-f', name], capture_output=True, timeout=15)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', required=True)
    parser.add_argument('--server-image', required=True)
    parser.add_argument('--server-source-revision', required=True)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    raise SystemExit(0 if check(args.server, args.server_image,
                               args.server_source_revision, args.output) else 1)
