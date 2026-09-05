#!/usr/bin/env python3
"""Audit source-built Sail observations; not a publication receipt issuer."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import statistics

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('matrix_contract', ROOT / 'validate-matrix-publication.py')
matrix = importlib.util.module_from_spec(spec)
spec.loader.exec_module(matrix)
require = matrix.require
REVISION = '79ee978411f4803ed5ae2dbbbb7d1507e8507490'
CLIENT = 'sha256:4a61a35c1c2a84389a4fc74366dd3d29f6a0fe1b9dc506b1dda85c1f22137341'
SERVER = 'sha256:344977c0f59f87e776822858f5b9f9e9b48373035db6e1becb7ed63e5823e14a'


def validate(directory):
    def read(name):
        return matrix.load_json(directory / name, name)[0]
    report = read('component.json')
    invocation = read('invocation.json')
    manifest = matrix.load_json(ROOT / 'evidence-manifest-v2.json', 'canonical manifest')[0]
    require(report['schema_version'] == 3, 'schema differs')
    matrix.validate_v3_timeout_contract(report, 'component.json')
    timing = report['timing']
    require((timing['warmup_iterations'], timing['measurement_iterations'], timing['query_timeout_ms'],
             timing['worker_ready_timeout_ms'], timing['query_recovery_timeout_ms']) == (2, 10, 60000, 30000, 15000),
            'comparison sampling differs')
    require(report['environment']['grust_revision'] == invocation['source_revision'] == REVISION,
            'client source differs')
    require(invocation['client_image_id'] == CLIENT, 'client image differs')
    scale, suite = report['dataset']['scale_factor'], report['suite']['track']
    require((scale, suite) == (invocation['scale'], invocation['suite']), 'invocation differs')
    for field in ('extracted_manifest_sha256', 'csv_files', 'csv_bytes', 'nodes', 'edges', 'person_nodes'):
        require(report['dataset'][field] == manifest['datasets'][scale][field], 'dataset identity differs')
    environment = report['environment']
    require((environment['container_os'], environment['container_arch'], environment['cpu_limit'],
             environment['memory_limit_bytes'], environment['resource_limit_scope']) ==
            ('linux', 'aarch64', '8', 6 * 1024**3, 'per-container'), 'declared environment differs')
    require(len(report['backends']) == 1 and report['backends'][0]['backend']['name'] == 'sail', 'backend differs')
    cell = report['backends'][0]
    require(cell['setup_outcome'] == 'pass', 'setup did not succeed')
    canonical = manifest['tracks'][suite]
    require([q['id'] for q in cell['queries']] == canonical['query_order'], 'query catalog differs')
    events = []
    for line in (directory / 'cell.log').read_text().splitlines():
        if line.startswith('{'):
            item = json.loads(line)
            if item.get('event') == 'observation-recorded':
                events.append(item)
    expected_events, summaries = [], []
    for query in cell['queries']:
        reference = canonical['queries'][query['id']]
        for field in ('source_sha256', 'adapter_sha256'):
            require(query[field] == reference[field], 'query digest differs')
        require(query['expected_count'] == reference['expected_count'][scale], 'oracle differs')
        if query['outcome'] == 'unsupported':
            require(scale != 'example' and not query['warmups'] and not query['measurements'], 'invalid unsupported sample')
            continue
        require(len(query['warmups']) == 2 and len(query['measurements']) == 10, 'incomplete samples')
        for phase, key in [('warmup', 'warmups'), ('measurement', 'measurements')]:
            for index, sample in enumerate(query[key], 1):
                require(sample['iteration'] == index, 'iteration differs')
                correct = sample.get('actual_count') == query['expected_count']
                require((sample['outcome'] == 'pass') == correct, 'sample count/outcome differs')
                expected_events.append(dict(backend='sail', complete=False, event='observation-recorded',
                                            journal_schema_version=1, observation=sample, phase=phase,
                                            query_id=query['id'], report_schema_version=3, scale=scale, suite=suite))
        passed = all(x['outcome'] == 'pass' for x in query['warmups'] + query['measurements'])
        require((query['outcome'] == 'pass') == passed, 'query aggregate differs')
        if passed:
            summaries.append(dict(query=query['id'], measured_samples=10,
                                  **{key: dict(min=min(values), median=statistics.median(values), max=max(values))
                                     for key in ('elapsed_ns', 'setup_ns', 'recovery_ns')
                                     for values in [[x[key] for x in query['measurements']]]}))
    expected_events.sort(key=lambda x: (x['phase'] == 'measurement', x['observation']['iteration'], x['observation']['query_position']))
    require(events == expected_events, 'incremental journal differs from final samples')
    for event in expected_events:
        sample = event['observation']
        position = (canonical['query_order'].index(event['query_id']) - sample['iteration'] + 1) % len(canonical['query_order']) + 1
        require(sample['query_position'] == position, 'rotating schedule differs')
    for component, image_id in [('client', CLIENT), ('server', SERVER)]:
        before, after = read(component + '-before.json'), read(component + '-after.json')
        require(before['container_id'] == after['container_id'] and before['image_id'] == after['image_id'] == image_id,
                'runtime identity differs')
        for snapshot in (before, after):
            resources = snapshot['resources']
            require((resources['Memory'], resources['MemorySwap'], resources['NanoCpus']) == (6 * 1024**3, 6 * 1024**3, 8_000_000_000), 'runtime resources differ')
            require(not snapshot['state']['OOMKilled'], 'runtime OOM')
        if component == 'client':
            require(not after['state']['Running'] and after['state']['ExitCode'] == 0, 'client did not exit successfully')
    watchdog = read('watchdog.json')
    require(watchdog['status'] == 'complete' and watchdog['child_exit_status'] == 0 and
            watchdog['container_id'] == read('client-before.json')['container_id'], 'watchdog did not complete')
    stopped = read('server-stopped.json')
    require(stopped['container_id'] == read('server-before.json')['container_id'] and
            not stopped['state']['Running'], 'owned service was not stopped')
    return dict(publication_qualified=False, scale=scale, suite=suite, observations=len(events),
                passed=sum(x['observation']['outcome'] == 'pass' for x in events),
                load_ns=cell['load_ns'], measured_summaries=summaries,
                component_sha256=hashlib.sha256((directory / 'component.json').read_bytes()).hexdigest())


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    print(json.dumps(validate(args.directory), indent=2, sort_keys=True))
