#!/usr/bin/env python3
"""Freeze audited native evidence for independent site admission.

This transport manifest is not a publication receipt or a performance ranking.
Only allowlisted structured records are exported, never raw container logs.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('neo4j_audit', ROOT / 'validate-neo4j-diagnostic.py')
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)
FILES = ('invocation.json', 'watchdog.json', 'client-before.json', 'client-after.json',
         'server-before.json', 'server-after.json', 'result/diagnostic.json',
         'result/observations.jsonl')


def canonical(value):
    return (json.dumps(value, sort_keys=True, indent=2) + '\n').encode()


def payloads(directory):
    exclusion = directory / 'performance-exclusion.json'
    if exclusion.exists() or exclusion.is_symlink():
        raise ValueError('run is excluded from performance export; retain raw diagnostic evidence')
    result = {}
    names = list(FILES)
    network = directory / 'network-before.json'
    if network.exists() or network.is_symlink():
        names.append('network-before.json')
    for name in names:
        path = directory / name
        if path.is_symlink() or not path.is_file() or path.parent.is_symlink():
            raise ValueError('not a regular evidence file: ' + name)
        result[name] = path.read_bytes()
    return result


def export(directory, output, upstream=ROOT / 'upstream/lsqb', attacks=ROOT / 'attacks'):
    # Audit precisely the captured bytes, not a live directory that may change
    # between validation and copying. The output is exclusive and never reused.
    captured = payloads(directory)
    with tempfile.TemporaryDirectory(prefix='grust-native-evidence-') as temporary:
        frozen = Path(temporary)
        for name, data in captured.items():
            target = frozen / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        checked = audit.validate(frozen, upstream, attacks)
        checked['runtime_verified'] = audit.validate_runtime(frozen)
        report = json.loads(captured['result/diagnostic.json'])
        if report['scale'] == '0.3':
            audit.require('network-before.json' in captured,
                          'SF0.3 publication requires retained internal-network provenance')
        if 'network-before.json' in captured:
            audit.validate_network_record(json.loads(captured['network-before.json']))
            checked['internal_network_verified'] = True
        audit.require_matched_sampling(report)
        checked['matched_sampling_verified'] = True
        checked['query_summaries'] = audit.summarize_measurements(report)
    invocation = json.loads(captured['invocation.json'])
    captured['audit.json'] = canonical(checked)
    manifest = dict(schema='grust-native-neo4j-evidence-bundle-v1',
                    track='native-neo4j', publication_qualified=False,
                    source_revision=invocation['source_revision'], scale=report['scale'],
                    files=[dict(path=name, bytes=len(data), sha256=hashlib.sha256(data).hexdigest())
                           for name, data in sorted(captured.items())])
    output.mkdir(parents=False, exist_ok=False)
    for name, data in captured.items():
        target = output / name
        target.parent.mkdir(parents=True, exist_ok=True)
        with target.open('xb') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
    # Written last: an interrupted export never has a completed manifest.
    with (output / 'bundle.json').open('xb') as stream:
        stream.write(canonical(manifest))
        stream.flush()
        os.fsync(stream.fileno())
    return manifest


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--upstream', type=Path, default=ROOT / 'upstream/lsqb')
    parser.add_argument('--attacks', type=Path, default=ROOT / 'attacks')
    args = parser.parse_args()
    print(json.dumps(export(args.directory, args.output, args.upstream, args.attacks)))
