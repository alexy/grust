#!/usr/bin/env python3
"""Freeze source-built Sail evidence for independent site admission.

Exports only structured observations from runtime logs. Includes the public
recipe and build logs, whose hashes must match the retained build receipts.
This manifest is transport integrity, not publication qualification.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('sail_audit', ROOT / 'validate-sail-source.py')
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)
FILES = ('component.json', 'invocation.json', 'watchdog.json', 'client-before.json',
         'client-after.json', 'server-before.json', 'server-after.json',
         'server-stopped.json', 'source-build.json', 'supervisor.json', 'cell.log')


def canonical(value):
    return (json.dumps(value, indent=2, sort_keys=True) + '\n').encode()


def read(path):
    return audit.matrix.read_regular_file(path, str(path))


def export(directory, output, source_build, client_build):
    captured = {name: read(directory / name) for name in FILES}
    build = json.loads(read(source_build / 'build-receipt.json'))
    client = json.loads(read(client_build / 'build-receipt.json'))
    audit.require(build == json.loads(captured['source-build.json']), 'source-build receipts differ')
    audit.require(build['image_id'] == audit.SERVER and client['image_id'] == audit.CLIENT and
                  client['source_revision'] == audit.REVISION, 'build identity differs')
    provenance = {}
    for prefix, root, receipt in [('source-build', source_build, build), ('client-build', client_build, client)]:
        log = read(root / 'build.log')
        audit.require(hashlib.sha256(log).hexdigest() == receipt['build_log_sha256'], 'build log differs')
        provenance[prefix + '/build.log'] = log
        provenance[prefix + '/build-receipt.json'] = canonical(receipt)
    recipe = read(source_build / 'Dockerfile.sail')
    audit.require(hashlib.sha256(recipe).hexdigest() == build['recipe_sha256'] ==
                  'b6ea1e6f903fd5b11bc0d277547e3fdb482b87ea12220d6e6acc568695d18ab4', 'public recipe differs')
    provenance['source-build/Dockerfile.sail'] = recipe
    # Validate captured bytes, never a changing live directory.
    with tempfile.TemporaryDirectory(prefix='grust-sail-frozen-') as temporary:
        frozen = Path(temporary)
        for name, data in captured.items():
            (frozen / name).write_bytes(data)
        checked = audit.validate(frozen)
    events = []
    for line in captured.pop('cell.log').decode().splitlines():
        if line.startswith('{'):
            item = json.loads(line)
            if item.get('event') == 'observation-recorded':
                events.append(item)
    captured['observations.jsonl'] = b''.join((json.dumps(x, sort_keys=True) + '\n').encode() for x in events)
    captured['audit.json'] = canonical(checked)
    captured.update(provenance)
    manifest = dict(schema='grust-sail-source-evidence-bundle-v1', track='sail-source',
                    source_revision=audit.REVISION, scale=checked['scale'], suite=checked['suite'],
                    publication_qualified=False,
                    files=[dict(path=name, bytes=len(data), sha256=hashlib.sha256(data).hexdigest())
                           for name, data in sorted(captured.items())])
    output.mkdir(exist_ok=False)
    for name, data in captured.items():
        target = output / name
        target.parent.mkdir(parents=True, exist_ok=True)
        with target.open('xb') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
    with (output / 'bundle.json').open('xb') as stream:
        stream.write(canonical(manifest))
        stream.flush()
        os.fsync(stream.fileno())
    return manifest


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--source-build', required=True, type=Path)
    parser.add_argument('--client-build', required=True, type=Path)
    args = parser.parse_args()
    print(json.dumps(export(args.directory, args.output, args.source_build, args.client_build)))
