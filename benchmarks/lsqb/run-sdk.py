#!/usr/bin/env python3
"""Qualify network SDK lanes on owned Docker services; no publication receipt.

Loading clears the selected benchmark graph. Only explicitly disposable,
isolated services are accepted. The selected server is stopped after the cell.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('runtime', ROOT / 'run-native-neo4j.py')
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)
NETWORK = 'grust-lsqb-sdk-qualification'
MEMORY = 6 * 1024**3
LANES = {
    'helix-sdk': ('helix', 'HELIX_SDK_BASE_URL', 'http', 8080),
    'surreal-sdk': ('surreal', 'SURREAL_SDK_URL', 'ws', 8000),
}


def validate_server(server, lane, pinned_image):
    if not re.fullmatch(r'(?:[^\s@]+@)?sha256:[0-9a-f]{64}', pinned_image):
        raise ValueError('server image must be digest-pinned')
    if pinned_image.startswith('sha256:') and server['Image'] != pinned_image:
        raise ValueError('source-built server content identity differs')
    state, host = server['State'], server['HostConfig']
    if server['Config']['Image'] != pinned_image or not state['Running'] or state['OOMKilled']:
        raise ValueError('server image or running state differs')
    if server['Config'].get('Labels', {}).get('io.adversarial.disposable') != lane:
        raise ValueError('independent SDK disposable ownership label required')
    if host['PortBindings'] or host['Memory'] != MEMORY or host['MemorySwap'] != MEMORY or host['NanoCpus'] != 8_000_000_000:
        raise ValueError('server requires no host ports and 8 CPU / 6 GiB without swap')
    networks = server['NetworkSettings']['Networks']
    if set(networks) != {NETWORK} or not networks[NETWORK]['IPAddress']:
        raise ValueError('server must use only the SDK qualification network')
    return networks[NETWORK]['IPAddress']


def validate_source_image(image, pinned_image, revision):
    """Local images need an explicit source pin, checked on the image itself."""
    if not pinned_image.startswith('sha256:'):
        if revision is not None:
            raise ValueError('source revision is only accepted for a local image ID')
        return
    if not revision or not re.fullmatch(r'[0-9a-f]{40}', revision):
        raise ValueError('source-built server requires a full source revision')
    if image['Id'] != pinned_image or image['Architecture'] != 'arm64' or image['Os'] != 'linux':
        raise ValueError('source-built image identity or platform differs')
    if image['Config'].get('Labels', {}).get('org.opencontainers.image.revision') != revision:
        raise ValueError('source-built image revision differs')


def endpoint(lane, address):
    _, variable, scheme, port = LANES[lane]
    return f'{variable}={scheme}://{address}:{port}'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--backend', choices=LANES, required=True)
    parser.add_argument('--server', required=True)
    parser.add_argument('--server-image', required=True)
    parser.add_argument('--server-version', required=True)
    parser.add_argument('--server-source-revision')
    parser.add_argument('--image', required=True)
    parser.add_argument('--source-revision', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scale', choices=('example', '0.1', '0.3'), default='example')
    parser.add_argument('--suite', choices=('baseline', 'adversarial'), required=True)
    args = parser.parse_args()
    if not re.fullmatch(r'grust-lsqb-' + args.backend + r'-[a-zA-Z0-9-]+', args.server):
        parser.error('select a lane-specific owned grust-lsqb server')
    if not re.fullmatch(r'[0-9a-f]{40}', args.source_revision):
        parser.error('source revision must be a full commit SHA')
    if not re.fullmatch(r'[0-9][a-zA-Z0-9.+-]*', args.server_version):
        parser.error('concrete server version required')
    server = runtime.inspect(args.server)
    address = validate_server(server, args.backend, args.server_image)
    validate_source_image(runtime.inspect(server['Image'], image=True),
                          args.server_image, args.server_source_revision)
    network = json.loads(subprocess.check_output(['docker', 'network', 'inspect', NETWORK], timeout=20))[0]
    if not network['Internal']:
        parser.error('SDK network must be internal')
    image = runtime.inspect(args.image, image=True)
    labels = image['Config'].get('Labels', {})
    if labels.get('org.opencontainers.image.revision') != args.source_revision or labels.get('io.adversarial.grust.benchmark-feature') != LANES[args.backend][0]:
        parser.error('client source/feature labels differ')
    if image['Architecture'] != 'arm64' or image['Os'] != 'linux':
        parser.error('client requires Linux ARM64')
    engine = subprocess.check_output(['docker', 'version', '--format', '{{.Server.Version}}'], text=True, timeout=20).strip()
    cpu = subprocess.check_output(['sysctl', '-n', 'machdep.cpu.brand_string'], text=True, timeout=20).strip()
    output = args.output.resolve()
    output.mkdir(exist_ok=False)

    def save(name, value):
        with (output / name).open('x') as stream:
            json.dump(value, stream, sort_keys=True)
            stream.flush()
            os.fsync(stream.fileno())

    # Emergency suite ceiling; not an expected duration. Each query is 60s.
    timeout = 17_799_000
    project = f'grust-lsqb-{args.backend}-{os.getpid()}'
    container = project + '-cell'
    datasets = ROOT / ('upstream/lsqb/data' if args.scale == 'example' else 'data')
    prefix = args.backend.replace('-', '_').upper()
    command = ['docker', 'create', '--name', container, '--init',
               '--label', f'com.docker.compose.project={project}', '--label', 'com.docker.compose.service=benchmark',
               '--network', NETWORK, '--cpus', '8', '--memory', str(MEMORY), '--memory-swap', str(MEMORY),
               '--read-only', '--tmpfs', '/tmp:rw,nosuid,nodev,size=1g', '--user', f'{os.getuid()}:{os.getgid()}',
               '--mount', f'type=bind,src={output},dst=/out',
               '--mount', f'type=bind,src={datasets},dst=/datasets,readonly']
    for value in [endpoint(args.backend, address), 'DOCKER_CPUS=8', f'DOCKER_MEMORY_BYTES={MEMORY}',
                  f'DOCKER_ENGINE_VERSION={engine}', f'HOST_CPU_MODEL={cpu}',
                  'BENCHMARK_RESOURCE_COMPONENTS=2', 'BENCHMARK_RESOURCE_LIMIT_SCOPE=per-container',
                  f'BENCHMARK_IMAGE={image["Id"]}', f'BENCHMARK_IMAGE_ID={image["Id"]}',
                  f'{prefix}_IMAGE={args.server_image}', f'{prefix}_IMAGE_ID={server["Image"]}',
                  f'{prefix}_VERSION={args.server_version}']:
        command += ['--env', value]
    command += [image['Id'], '--backend', args.backend, '--suite', args.suite, '--scale', args.scale,
                '--warmups', '2', '--runs', '10', '--query-timeout-ms', '60000',
                '--worker-ready-timeout-ms', '30000', '--query-reap-grace-ms', '250',
                '--query-kill-reap-timeout-ms', '5000', '--query-recovery-timeout-ms', '15000',
                '--cell-timeout-ms', str(timeout), '--lsqb-root', '/opt/lsqb-mounted',
                '--attacks-dir', '/opt/grust-attacks', '--output', '/out/component.json']
    save('invocation.json', dict(publication_qualified=False, command=command,
                               source_revision=args.source_revision, backend=args.backend,
                               server_source_revision=args.server_source_revision,
                               scale=args.scale, suite=args.suite, client_image_id=image['Id']))
    save('supervisor.json', dict(files={name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
                                      for name in ('run-sdk.py', 'run-native-neo4j.py', 'cell-watchdog.py')},
                                network_id=network['Id'], network_internal=True,
                                docker_engine_version=engine, cpu_model=cpu))
    save('server-before.json', runtime.snapshot(server))
    client_id = None
    try:
        client_id = subprocess.check_output(command, text=True, timeout=30).strip()
        if not re.fullmatch(r'[0-9a-f]{64}', client_id):
            raise ValueError('invalid client identity')
        save('client-before.json', runtime.snapshot(runtime.inspect(client_id)))
        with (output / 'watchdog.json').open('xb', buffering=0) as watchdog, (output / 'cell.log').open('xb', buffering=0) as log:
            supervised = [sys.executable, str(ROOT / 'cell-watchdog.py'), '--timeout-ms', str(timeout),
                          '--heartbeat-ms', '30000', '--container', container, '--project', project,
                          '--service', 'benchmark', '--record-fd', str(watchdog.fileno()), '--',
                          sys.executable, str(ROOT / 'run-native-neo4j.py'), '_start-recorded', client_id, str(output)]
            process = subprocess.Popen(supervised, pass_fds=(watchdog.fileno(),), stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
            for line in process.stdout:
                log.write(line)
                os.fsync(log.fileno())
                sys.stdout.buffer.write(line)
                sys.stdout.buffer.flush()
            status = process.wait()
        state = json.loads((output / 'client-after.json').read_text())['state']
        if state['Running'] or state['OOMKilled'] or state['ExitCode'] != 0:
            status = 125
    finally:
        try:
            if client_id and not (output / 'client-after.json').exists():
                subprocess.run(['docker', 'stop', client_id], timeout=30, check=False)
        finally:
            try:
                save('server-after.json', runtime.snapshot(runtime.inspect(server['Id'])))
            finally:
                subprocess.run(['docker', 'stop', server['Id']], timeout=30, check=True)
            save('server-stopped.json', runtime.snapshot(runtime.inspect(server['Id'])))
    print(json.dumps(dict(event='sdk-cell-finished', exit=status, output=str(output))), flush=True)
    return status


if __name__ == '__main__':
    raise SystemExit(main())
