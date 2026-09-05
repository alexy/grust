#!/usr/bin/env python3
"""Freeze pinned SDK observations and client provenance for site admission.

This transport manifest is not a publication receipt. Only the qualified
Surreal SDK example source is currently accepted by the diagnostic auditor.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('sdk_audit', ROOT / 'validate-sdk.py')
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)
FILES = ('component.json', 'invocation.json', 'watchdog.json', 'client-before.json',
         'client-after.json', 'server-before.json', 'server-after.json',
         'server-stopped.json', 'supervisor.json', 'cell.log')
RECIPE = '879e613ea5de54e5a8b3ecd5d494758d1ef3cfd506728d2b7724be1bb466ff1b'


def canonical(value):
    return (json.dumps(value, indent=2, sort_keys=True) + '\n').encode()


def read(path):
    return audit.matrix.read_regular_file(path, str(path))


def export(directory, output, client_build):
    captured = {name: read(directory / name) for name in FILES}
    receipt_bytes = read(client_build / 'build-receipt.json')
    receipt = json.loads(receipt_bytes)
    audit.require(receipt['schema'] == 'grust-sdk-client-build-v1'
                  and receipt['publication_qualified'] is False
                  and receipt['backend'] == 'surreal-sdk' and receipt['feature'] == 'surreal'
                  and receipt['source_revision'] == audit.SOURCE
                  and receipt['image_id'] == audit.CLIENT
                  and receipt['platform'] == 'linux/arm64', 'client build identity differs')
    labels = receipt['image_labels']
    audit.require(labels['org.opencontainers.image.revision'] == audit.SOURCE
                  and labels['io.adversarial.grust.benchmark-feature'] == 'surreal',
                  'client build labels differ')
    recipe, log = read(client_build / 'Dockerfile'), read(client_build / 'build/command.log')
    audit.require(hashlib.sha256(recipe).hexdigest() == receipt['recipe_sha256'] == RECIPE,
                  'client recipe differs')
    audit.require(hashlib.sha256(log).hexdigest() == receipt['build_log_sha256'],
                  'client build log differs')
    # Audit a frozen byte snapshot so the exported payload is what was checked.
    with tempfile.TemporaryDirectory(prefix='grust-sdk-frozen-') as temporary:
        frozen = Path(temporary)
        for name, data in captured.items():
            (frozen / name).write_bytes(data)
        checked = audit.validate(frozen)
    events = [json.loads(line) for line in captured.pop('cell.log').decode().splitlines()
              if line.startswith('{')]
    captured['observations.jsonl'] = b''.join(
        (json.dumps(event, sort_keys=True) + '\n').encode()
        for event in events if event.get('event') == 'observation-recorded')
    captured.update({'audit.json': canonical(checked),
                     'client-build/build-receipt.json': receipt_bytes,
                     'client-build/Dockerfile': recipe, 'client-build/build.log': log})
    manifest = dict(schema='grust-sdk-evidence-bundle-v1', track='surreal-sdk',
                    source_revision=audit.SOURCE, scale=checked['scale'], suite=checked['suite'],
                    publication_qualified=False,
                    files=[dict(path=name, bytes=len(data), sha256=hashlib.sha256(data).hexdigest())
                           for name, data in sorted(captured.items())])
    output.mkdir(exist_ok=False)
    for name, data in {**captured, 'bundle.json': canonical(manifest)}.items():
        target = output / name
        target.parent.mkdir(parents=True, exist_ok=True)
        with target.open('xb') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
    return manifest


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--client-build', required=True, type=Path)
    args = parser.parse_args()
    print(json.dumps(export(args.directory, args.output, args.client_build)))
