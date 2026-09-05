#!/usr/bin/env python3
"""Audit the pinned Helix SDK example cohort; never issue publication receipts.

Checks raw counts, query identities, timing, rotation, journal equality and
retained Docker lifecycle. Other SDK sources/scales require new qualification.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import statistics

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('matrix', ROOT / 'validate-matrix-publication.py')
matrix = importlib.util.module_from_spec(spec)
spec.loader.exec_module(matrix)
require = matrix.require
SOURCE = 'ed3febd88d35c5a6bd6c090787536dc0f33c85cd'
CLIENT = 'sha256:3ca6078cbd6cfa85fef576fff4fc075f90b8cb7b7f3cae7152201f1ab1f1c2b5'
SERVER = 'sha256:3b97997c25a09753884acda0e65982e180961bacc50d355f3cbc913d1dc6eb58'
NETWORK = 'grust-lsqb-sdk-qualification'
MANIFEST = '1dcae942840f216a83282f45f27e7fe228616e8f51af764689dc4f4fea0de849'


def validate(directory):
    def read(name):
        return matrix.load_json(directory / name, name)[0]
    raw = (ROOT / 'evidence-manifest-v2.json').read_bytes()
    require(hashlib.sha256(raw).hexdigest() == MANIFEST, 'canonical contract changed')
    manifest = json.loads(raw)
    report, invocation = read('component.json'), read('invocation.json')
    require(report['schema_version'] == 3 and report['valid'] is True and report['complete'] is False,
            'not a successful standalone component')
    require(report['warning'] == 'These are not LDBC Benchmark Results.', 'qualification warning differs')
    matrix.validate_v3_timeout_contract(report, 'component.json')
    require(report['timing'] == dict(warmup_iterations=2, measurement_iterations=10,
        query_timeout_ms=60000, worker_ready_timeout_ms=30000, query_reap_grace_ms=250,
        query_kill_reap_timeout_ms=5000, query_recovery_timeout_ms=15000,
        cell_timeout_ms=17799000, timeout_enforcement='coordinator-process-group',
        query_order='rotating', boundary='coordinator-go-to-result-consumed'), 'sampling differs')
    require(invocation['publication_qualified'] is False and invocation['source_revision'] == SOURCE
            and invocation['client_image_id'] == CLIENT and invocation['backend'] == 'helix-sdk', 'invocation identity differs')
    require(invocation['server_source_revision'] == '0ef3cee0faf28bb81072fb149b982dcdb166d60a',
            'server source revision differs')
    require(read('server-before.json')['labels']['org.opencontainers.image.revision']
            == invocation['server_source_revision'], 'server source label differs')
    scale, suite = report['dataset']['scale_factor'], report['suite']['track']
    require(scale == invocation['scale'] == 'example' and suite == invocation['suite']
            and suite in ('baseline', 'adversarial'), 'unqualified scale/suite')
    for field in ('extracted_manifest_sha256', 'csv_files', 'csv_bytes', 'nodes', 'edges', 'person_nodes'):
        require(report['dataset'][field] == manifest['datasets'][scale][field], 'dataset differs')
    environment = report['environment']
    require(environment['grust_revision'] == SOURCE and environment['container_os'] == 'linux'
            and environment['container_arch'] == 'aarch64' and environment['cpu_limit'] == '8'
            and environment['memory_limit_bytes'] == 6442450944
            and environment['resource_limit_scope'] == 'per-container', 'environment differs')
    require(len(report['backends']) == 1, 'wrong backend count')
    cell = report['backends'][0]
    require(cell['backend'] == dict(name='helix-sdk', adapter='grust-helix', adapter_version='0.13.0',
        runner_image=CLIENT, runner_image_id=CLIENT, resource_components=2,
        service_version='0.1.0', image=SERVER, image_id=SERVER),
        'backend identity/transport lane differs')
    require(cell['setup_outcome'] == 'pass' and matrix.nonnegative_integer(cell['load_ns']), 'setup failed')
    canonical = manifest['tracks'][suite]
    require([q['id'] for q in cell['queries']] == canonical['query_order'], 'query catalog differs')
    expected, summaries = [], []
    for query in cell['queries']:
        reference = canonical['queries'][query['id']]
        for field in ('source_sha256', 'adapter_sha256'):
            require(query[field] == reference[field], 'query source differs')
        require(matrix.nonnegative_integer(query['expected_count'])
                and query['expected_count'] == reference['expected_count'][scale], 'oracle differs')
        require(query['execution'] == {
            'class': 'backend-materialize-rust-reference', 'language': 'Grust portable Cypher',
            'transport': 'Helix Rust SDK / HTTP'}, 'execution plan differs')
        rows = reference['rust_rows']['in_process']
        require(query['rust_rows'] == dict(kind=rows['kind'], rows=rows['rows'][scale]), 'row estimate differs')
        require(query['outcome'] == 'pass', 'cohort includes a failed/refused query')
        for phase, key, count in [('warmup', 'warmups', 2), ('measurement', 'measurements', 10)]:
            require(len(query[key]) == count, 'incomplete phase')
            for iteration, sample in enumerate(query[key], 1):
                position = (canonical['query_order'].index(query['id']) - iteration + 1) % len(cell['queries']) + 1
                require(sample['iteration'] == iteration and sample['query_position'] == position, 'rotation differs')
                require(matrix.nonnegative_integer(sample['actual_count'])
                        and sample['actual_count'] == query['expected_count'] and sample['outcome'] == 'pass'
                        and sample['termination'] == 'normal-exit', 'sample failed or count differs')
                expected.append(dict(backend='helix-sdk', complete=False, event='observation-recorded',
                    journal_schema_version=1, observation=sample, phase=phase, query_id=query['id'],
                    report_schema_version=3, scale=scale, suite=suite))
        summaries.append(dict(query=query['id'], measured_samples=10,
            **{field: dict(min=min(values), median=statistics.median(values), max=max(values))
               for field in ('elapsed_ns', 'setup_ns', 'recovery_ns')
               for values in [[s[field] for s in query['measurements']]]}))
    expected.sort(key=lambda e: (e['phase'] == 'measurement', e['observation']['iteration'], e['observation']['query_position']))
    events = [json.loads(line) for line in (directory / 'cell.log').read_text().splitlines() if line.startswith('{')]
    events = [e for e in events if e.get('event') == 'observation-recorded']
    require(events == expected, 'journal differs from final samples')
    for role, image in [('client', CLIENT), ('server', SERVER)]:
        before, after = read(role + '-before.json'), read(role + '-after.json')
        for field in ('container_id', 'image_id', 'name', 'resources', 'labels'):
            require(before[field] == after[field], f'{role} changed {field}')
        require(before['image_id'] == image, 'runtime image differs')
        for snapshot in (before, after):
            resource, state = snapshot['resources'], snapshot['state']
            require((resource['Memory'], resource['MemorySwap'], resource['NanoCpus'], resource['NetworkMode'])
                    == (6442450944, 6442450944, 8000000000, NETWORK), 'runtime limits/network differ')
            require(state['OOMKilled'] is False and state['ExitCode'] == 0, 'runtime OOM/failed exit')
        if role == 'client':
            require(before['state']['Status'] == 'created' and after['state']['Status'] == 'exited'
                    and not after['state']['Running'] and before['resources']['ReadonlyRootfs'], 'client lifecycle differs')
            require(before['labels']['org.opencontainers.image.revision'] == SOURCE
                    and before['labels']['io.adversarial.grust.benchmark-feature'] == 'helix', 'client feature/source differs')
        else:
            require(before['state']['Running'] and after['state']['Running']
                    and before['state']['StartedAt'] == after['state']['StartedAt'], 'server restarted/stopped during run')
            require(before['labels']['io.adversarial.disposable'] == 'helix-sdk', 'server ownership differs')
    watchdog, client = read('watchdog.json'), read('client-before.json')
    require(watchdog['schema'] == 'grust-lsqb-cell-watchdog-completion-v1'
            and watchdog['status'] == 'complete' and watchdog['child_exit_status'] == 0
            and watchdog['service'] == 'benchmark' and watchdog['timeout_ms'] == 17799000
            and matrix.nonnegative_integer(watchdog['elapsed_wall_ms'])
            and watchdog['elapsed_wall_ms'] <= watchdog['timeout_ms']
            and watchdog['container_id'] == client['container_id']
            and watchdog['container_name'] == client['name'].removeprefix('/')
            and watchdog['project'] == client['labels']['com.docker.compose.project'], 'watchdog incomplete or ownership differs')
    stopped = read('server-stopped.json')
    require(stopped['container_id'] == read('server-before.json')['container_id']
            and not stopped['state']['Running'] and not stopped['state']['OOMKilled'], 'server cleanup differs')
    return dict(publication_qualified=False, diagnostic_verified=True, scale=scale, suite=suite,
                observations=len(events), passed=len(events), load_ns=cell['load_ns'], measured_summaries=summaries,
                component_sha256=hashlib.sha256((directory / 'component.json').read_bytes()).hexdigest())


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    print(json.dumps(validate(parser.parse_args().directory), indent=2, sort_keys=True))
