#!/usr/bin/env python3
"""Run an owned Neo4j diagnostic with retained client/server runtime evidence.

Does not create, clear, or delete a database. A failed cell stops its explicitly
selected disposable server. This runner does not issue publication receipts.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

SERVER_IMAGE = 'neo4j:2026.07.1-community@sha256:31697c776d8c255152be39430d4b306a414c1409c91dccd093ac5e6baf2cae9d'
MEMORY = 6 * 1024**3
NANO_CPUS = 8_000_000_000


def require_host_preflight(output):
    """Fail before client creation; retain the screen even on contention."""
    result = subprocess.run(
        [sys.executable, str(Path(__file__).with_name('host_preflight.py')),
         '--output', str(output / 'host-preflight.json')], check=False, timeout=45)
    if result.returncode != 0:
        raise ValueError('host CPU preflight failed; no benchmark client created')


def validate_network(server, network):
    name = 'grust-lsqb-neo4j-qualification'
    if network.get('Name') != name or network.get('Internal') is not True:
        raise ValueError('Neo4j qualification network must be internal')
    attached = server['NetworkSettings']['Networks']
    if set(attached) != {name} or server['HostConfig']['NetworkMode'] != name:
        raise ValueError('server must be attached only to the qualification network')
    if attached[name].get('NetworkID') != network.get('Id'):
        raise ValueError('server network identity differs')


def inspect(target, image=False):
    command = ['docker', 'image', 'inspect'] if image else ['docker', 'inspect']
    return json.loads(subprocess.check_output([*command, target], timeout=20))[0]


def snapshot(raw):
    """Allowlist runtime metadata; never export arbitrary environment variables."""
    host = raw['HostConfig']
    return dict(container_id=raw['Id'], image_id=raw['Image'], name=raw['Name'],
                state={key: raw['State'].get(key) for key in
                       ('Status', 'Running', 'OOMKilled', 'ExitCode', 'StartedAt', 'FinishedAt')},
                resources={key: host.get(key) for key in
                           ('Memory', 'MemorySwap', 'NanoCpus', 'CpusetCpus', 'ReadonlyRootfs', 'NetworkMode')},
                labels=raw['Config'].get('Labels', {}))


def stop_owned(client_id, server_id):
    """Always attempt remote shutdown, even if stopping the client fails."""
    try:
        subprocess.run(['docker', 'stop', client_id], check=True, timeout=30)
    finally:
        subprocess.run(['docker', 'stop', server_id], check=True, timeout=30)


def wait_ready(server_id, timeout=120):
    """Probe Bolt before measurement; a running container is not a ready DB.

    Retain only probe status, never unfiltered command output. Each attempt is
    bounded and emits progress, including when cypher-shell itself times out.
    """
    started = time.monotonic()
    attempts = 0
    while True:
        remaining = timeout - (time.monotonic() - started)
        if remaining <= 0:
            raise RuntimeError('Neo4j readiness deadline exceeded before benchmark start')
        attempts += 1
        try:
            result = subprocess.run(
                ['docker', 'exec', server_id, 'cypher-shell', '--format', 'plain',
                 'RETURN 42 AS readiness'], capture_output=True, text=True,
                timeout=min(10, remaining), check=False)
            ready = result.returncode == 0 and result.stdout.strip().splitlines() == ['readiness', '42']
        except subprocess.TimeoutExpired:
            ready = False
        record = dict(event='neo4j-readiness', attempt=attempts, ready=ready,
                      elapsed_ms=round((time.monotonic() - started) * 1000))
        print(json.dumps(record), flush=True)
        if ready:
            return record
        remaining = timeout - (time.monotonic() - started)
        if remaining > 0:
            time.sleep(min(2, remaining))


def start_recorded(client_id, output):
    """Capture exit metadata before the watchdog removes its owned container."""
    if not re.fullmatch(r'[0-9a-f]{64}', client_id):
        raise ValueError('invalid client identity')
    status = subprocess.run(['docker', 'start', '--attach', client_id]).returncode
    with (Path(output) / 'client-after.json').open('x') as stream:
        json.dump(snapshot(inspect(client_id)), stream, sort_keys=True)
        stream.flush()
        os.fsync(stream.fileno())
    return status


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', required=True)
    parser.add_argument('--image', required=True)
    parser.add_argument('--source-revision', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scale', choices=('example', '0.1', '0.3'), default='example')
    parser.add_argument('--mode', choices=('qualify', 'deadline-probe', 'recovery-probe'), default='qualify')
    parser.add_argument('--warmups', type=int)
    parser.add_argument('--runs', type=int)
    args = parser.parse_args()
    sampled = args.warmups is not None or args.runs is not None
    if sampled and (args.mode != 'qualify' or args.warmups is None or args.runs is None or
                    not 0 <= args.warmups <= 5 or not 1 <= args.runs <= 10):
        parser.error('sampling requires qualify mode, --warmups 0..5 and --runs 1..10 together')
    if not re.fullmatch(r'grust-lsqb-neo4j-[a-zA-Z0-9-]+', args.server):
        parser.error('server must be an explicitly owned grust-lsqb-neo4j-* container')
    if not re.fullmatch(r'[0-9a-f]{40}', args.source_revision):
        parser.error('source revision must be a full commit SHA')
    server, image = inspect(args.server), inspect(args.image, image=True)
    if not server['State']['Running'] or server['State']['OOMKilled']:
        parser.error('server is not healthy/running')
    if server['Config']['Image'] != SERVER_IMAGE or 'NEO4J_AUTH=none' not in server['Config']['Env']:
        parser.error('server must be the pinned disposable unauthenticated image on a private network')
    host = server['HostConfig']
    if host['PortBindings'] or host['Memory'] != MEMORY or host['MemorySwap'] != MEMORY or host['NanoCpus'] != NANO_CPUS:
        parser.error('server must have no host ports and 8 CPU / 6 GiB without swap')
    labels = image['Config'].get('Labels', {})
    if labels.get('org.opencontainers.image.revision') != args.source_revision or labels.get('io.adversarial.grust.benchmark-feature') != 'neo4j-native':
        parser.error('client image source/feature labels differ')
    network = 'grust-lsqb-neo4j-qualification'
    network_record = json.loads(subprocess.check_output(
        ['docker', 'network', 'inspect', network], timeout=20))[0]
    try:
        validate_network(server, network_record)
    except ValueError as error:
        parser.error(str(error))
    address = server['NetworkSettings']['Networks'].get(network, {}).get('IPAddress')
    if not address:
        parser.error('server must be on the private qualification network')
    root = Path(__file__).resolve().parent
    datasets = root / ('upstream/lsqb/data' if args.scale == 'example' else 'data')
    output = args.output.resolve()
    output.mkdir(exist_ok=False)

    def save(name, value):
        with (output / name).open('x') as stream:
            json.dump(value, stream, sort_keys=True)
            stream.flush()
            os.fsync(stream.fileno())

    if args.mode == 'qualify':
        require_host_preflight(output)

    project = f'grust-lsqb-native-{os.getpid()}'
    container = project + '-neo4j-cell'
    command = ['docker', 'create', '--name', container, '--init',
               '--label', f'com.docker.compose.project={project}', '--label', 'com.docker.compose.service=benchmark',
               '--network', network, '--cpus', '8', '--memory', '6g', '--memory-swap', '6g',
               '--read-only', '--tmpfs', '/tmp:rw,nosuid,nodev,size=1g', '--user', f'{os.getuid()}:{os.getgid()}',
               '--mount', f'type=bind,src={output},dst=/out',
               '--mount', f'type=bind,src={datasets},dst=/datasets,readonly',
               '--env', f'NEO4J_URI=bolt://{address}:7687', '--env', 'NEO4J_BENCHMARK_DISPOSABLE=1',
               '--entrypoint', '/usr/local/bin/grust-lsqb-neo4j', image['Id'], args.mode]
    if args.mode == 'qualify':
        command += ['/opt/lsqb-mounted', '/opt/grust-attacks', args.scale, '/out/result']
        if sampled:
            command += [str(args.warmups), str(args.runs)]
    save('invocation.json', dict(diagnostic_only=True, command=command, source_revision=args.source_revision,
                                 client_image_id=image['Id'], client_labels=labels, server_image=SERVER_IMAGE))
    save('server-before.json', snapshot(server))
    save('network-before.json', {key: network_record[key] for key in ('Name', 'Id', 'Internal')})
    # No client exists yet and no dataset has been touched. A startup failure
    # therefore cannot be mistaken for a query failure or a measured sample.
    try:
        wait_ready(server['Id'])
    except Exception:
        subprocess.run(['docker', 'stop', server['Id']], check=True, timeout=30)
        raise
    client_id = subprocess.check_output(command, text=True, timeout=30).strip()
    if not re.fullmatch(r'[0-9a-f]{64}', client_id):
        raise RuntimeError('invalid created client identity')
    save('client-before.json', snapshot(inspect(client_id)))
    # Import allowance plus all per-sample READY/query/reap/recovery bounds.
    # This is an emergency ceiling, not an estimate of expected wall time.
    timeout = (600000 + 22 * (args.warmups + args.runs) * 110250) if sampled else (
        600000 if args.scale == 'example' else 3100000)
    status = 125
    try:
        with (output / 'watchdog.json').open('xb', buffering=0) as watchdog, (output / 'cell.log').open('xb', buffering=0) as log:
            supervised = [sys.executable, str(root / 'cell-watchdog.py'), '--timeout-ms', str(timeout),
                          '--heartbeat-ms', '30000', '--container', container, '--project', project,
                          '--service', 'benchmark', '--record-fd', str(watchdog.fileno()),
                          '--', sys.executable, str(Path(__file__).resolve()),
                          '_start-recorded', client_id, str(output)]
            process = subprocess.Popen(supervised, pass_fds=(watchdog.fileno(),), stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
            for line in process.stdout:
                log.write(line)
                os.fsync(log.fileno())
                sys.stdout.buffer.write(line)
                sys.stdout.buffer.flush()
            status = process.wait()
    finally:
        # The child wrapper captures a normal exit before watchdog cleanup.
        # Missing evidence after an outer timeout is never forged into a snapshot.
        try:
            client_after = json.loads((output / 'client-after.json').read_text())
        except Exception:
            try:
                stop_owned(client_id, server['Id'])
            finally:
                save('client-after.json', dict(container_id=client_id,
                                               capture_error='container inspection unavailable'))
                save('server-after.json', snapshot(inspect(server['Id'])))
            raise
        state = client_after['state']
        if status == 0 and (state['Running'] or state['OOMKilled'] or state['ExitCode'] != 0):
            status = 125
        if status:
            subprocess.run(['docker', 'stop', server['Id']], check=True, timeout=30)
        save('server-after.json', snapshot(inspect(server['Id'])))
    print(json.dumps(dict(event='native-cell-finished', exit=status, output=str(output))), flush=True)
    return status


if __name__ == '__main__':
    if len(sys.argv) == 4 and sys.argv[1] == '_start-recorded':
        raise SystemExit(start_recorded(sys.argv[2], sys.argv[3]))
    raise SystemExit(main())
