#!/usr/bin/env python3
"""Run Sail on a private Docker network, retaining source-built runtime evidence.

Uses a fresh coordinator-owned session. No public-registry availability claim or
publication receipt is issued. The selected disposable server is always stopped.
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
spec = importlib.util.spec_from_file_location('native_runtime', ROOT / 'run-native-neo4j.py')
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)
NETWORK = 'grust-lsqb-sail-source-qualification'
MEMORY = 6 * 1024**3


def read_build(directory):
    receipt = json.loads((directory / 'build-receipt.json').read_text())
    for name, field in [('build.log', 'build_log_sha256'), ('Dockerfile.sail', 'recipe_sha256')]:
        if hashlib.sha256((directory / name).read_bytes()).hexdigest() != receipt[field]:
            raise ValueError('source-build file hash mismatch: ' + name)
    if receipt['schema'] != 'grust-sail-source-build-v1' or receipt['runtime_version'] != 'sail 0.7.1':
        raise ValueError('unsupported source-build receipt')
    if receipt['source_revision'] != '4995115ad95e7e12215e86bcc13e60a78ddcea00':
        raise ValueError('source recipe revision differs')
    if receipt['recipe_sha256'] != 'b6ea1e6f903fd5b11bc0d277547e3fdb482b87ea12220d6e6acc568695d18ab4':
        raise ValueError('source recipe differs from public pinned recipe')
    return receipt


def validate_server(server, build):
    host = server['HostConfig']
    if server['Image'] != build['image_id'] or not server['State']['Running'] or server['State']['OOMKilled']:
        raise ValueError('server identity or running state differs')
    if host['PortBindings'] or host['Memory'] != MEMORY or host['MemorySwap'] != MEMORY or host['NanoCpus'] != 8_000_000_000:
        raise ValueError('server requires no host ports and 8 CPU / 6 GiB without swap')
    networks = server['NetworkSettings']['Networks']
    if set(networks) != {NETWORK} or not networks[NETWORK]['IPAddress']:
        raise ValueError('server must use only the private qualification network')
    if server['Config'].get('Labels', {}).get('io.adversarial.disposable') != 'sail-source':
        raise ValueError('server lacks explicit disposable ownership label')
    return networks[NETWORK]['IPAddress']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', required=True)
    parser.add_argument('--image', required=True)
    parser.add_argument('--source-revision', required=True)
    parser.add_argument('--source-build', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scale', choices=('example', '0.1', '0.3'), default='example')
    parser.add_argument('--suite', choices=('baseline', 'adversarial'), required=True)
    args = parser.parse_args()
    if not re.fullmatch(r'grust-lsqb-sail-source-[a-zA-Z0-9-]+', args.server):
        parser.error('select an explicitly owned grust-lsqb-sail-source-* server')
    if not re.fullmatch(r'[0-9a-f]{40}', args.source_revision):
        parser.error('source revision must be a full commit SHA')
    build = read_build(args.source_build)
    server = runtime.inspect(args.server)
    address = validate_server(server, build)
    network = json.loads(subprocess.check_output(['docker', 'network', 'inspect', NETWORK], timeout=20))[0]
    if not network['Internal']:
        parser.error('qualification network must be internal')
    engine = subprocess.check_output(['docker', 'version', '--format', '{{.Server.Version}}'], text=True, timeout=20).strip()
    cpu = subprocess.check_output(['sysctl', '-n', 'machdep.cpu.brand_string'], text=True, timeout=20).strip()
    image = runtime.inspect(args.image, image=True)
    labels = image['Config'].get('Labels', {})
    if labels.get('org.opencontainers.image.revision') != args.source_revision or labels.get('io.adversarial.grust.benchmark-feature') != 'sail':
        parser.error('client source/feature labels differ')
    if image['Architecture'] != 'arm64' or image['Os'] != 'linux':
        parser.error('client platform differs')
    output = args.output.resolve()
    output.mkdir(exist_ok=False)
    runtime.require_host_preflight(output)

    def save(name, value):
        with (output / name).open('x') as stream:
            json.dump(value, stream, sort_keys=True)
            stream.flush()
            os.fsync(stream.fileno())

    # Same worst-case suite ceiling as the matched example matrix. Per-query
    # 60s remains enforced independently; this is not an expected running time.
    timeout = 17_799_000
    project = f'grust-lsqb-sail-source-{os.getpid()}'
    container = project + '-cell'
    datasets = ROOT / ('upstream/lsqb/data' if args.scale == 'example' else 'data')
    command = ['docker', 'create', '--name', container, '--init',
               '--label', f'com.docker.compose.project={project}',
               '--label', 'com.docker.compose.service=benchmark',
               '--network', NETWORK, '--cpus', '8', '--memory', str(MEMORY), '--memory-swap', str(MEMORY),
               '--read-only', '--tmpfs', '/tmp:rw,nosuid,nodev,size=1g', '--user', f'{os.getuid()}:{os.getgid()}',
               '--mount', f'type=bind,src={output},dst=/out',
               '--mount', f'type=bind,src={datasets},dst=/datasets,readonly',
               '--env', f'SAIL_ENDPOINT=http://{address}:50051',
               '--env', 'DOCKER_CPUS=8', '--env', f'DOCKER_MEMORY_BYTES={MEMORY}',
               '--env', f'DOCKER_ENGINE_VERSION={engine}', '--env', f'HOST_CPU_MODEL={cpu}',
               '--env', 'BENCHMARK_RESOURCE_LIMIT_SCOPE=per-container',
               '--env', 'BENCHMARK_IMAGE=' + image['Id'], image['Id'],
               '--backend', 'sail', '--suite', args.suite, '--scale', args.scale,
               '--warmups', '2', '--runs', '10', '--query-timeout-ms', '60000',
               '--worker-ready-timeout-ms', '30000', '--query-reap-grace-ms', '250',
               '--query-kill-reap-timeout-ms', '5000', '--query-recovery-timeout-ms', '15000',
               '--cell-timeout-ms', str(timeout), '--lsqb-root', '/opt/lsqb-mounted',
               '--attacks-dir', '/opt/grust-attacks', '--output', '/out/component.json']
    save('invocation.json', dict(publication_qualified=False, command=command,
                                 source_revision=args.source_revision, scale=args.scale, suite=args.suite,
                                 client_image_id=image['Id'], client_labels=labels))
    save('source-build.json', build)
    save('supervisor.json', dict(files={name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
                                       for name in ('run-sail-source.py', 'run-native-neo4j.py', 'cell-watchdog.py')},
                                 docker_engine_version=engine, cpu_model=cpu,
                                 network_id=network['Id'], network_internal=network['Internal']))
    save('server-before.json', runtime.snapshot(server))
    client_id = None
    status = 125
    try:
        client_id = subprocess.check_output(command, text=True, timeout=30).strip()
        if not re.fullmatch(r'[0-9a-f]{64}', client_id):
            raise RuntimeError('invalid created client identity')
        save('client-before.json', runtime.snapshot(runtime.inspect(client_id)))
        with (output / 'watchdog.json').open('xb', buffering=0) as watchdog, (output / 'cell.log').open('xb', buffering=0) as log:
            supervised = [sys.executable, str(ROOT / 'cell-watchdog.py'), '--timeout-ms', str(timeout),
                          '--heartbeat-ms', '30000', '--container', container, '--project', project,
                          '--service', 'benchmark', '--record-fd', str(watchdog.fileno()),
                          '--', sys.executable, str(ROOT / 'run-native-neo4j.py'),
                          '_start-recorded', client_id, str(output)]
            process = subprocess.Popen(supervised, pass_fds=(watchdog.fileno(),),
                                       stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
            for line in process.stdout:
                log.write(line)
                os.fsync(log.fileno())
                sys.stdout.buffer.write(line)
                sys.stdout.buffer.flush()
            status = process.wait()
        after = json.loads((output / 'client-after.json').read_text())
        state = after['state']
        if state['Running'] or state['OOMKilled'] or state['ExitCode'] != 0:
            status = 125
    finally:
        # An absent post-run snapshot remains missing evidence, never success.
        # The watchdog owns client cleanup; stop the exact remote service too.
        try:
            if client_id and not (output / 'client-after.json').exists():
                subprocess.run(['docker', 'stop', client_id], timeout=30, check=False)
        finally:
            try:
                save('server-after.json', runtime.snapshot(runtime.inspect(server['Id'])))
            finally:
                subprocess.run(['docker', 'stop', server['Id']], timeout=30, check=True)
            save('server-stopped.json', runtime.snapshot(runtime.inspect(server['Id'])))
    print(json.dumps(dict(event='sail-source-cell-finished', exit=status, output=str(output))), flush=True)
    return status


if __name__ == '__main__':
    raise SystemExit(main())
