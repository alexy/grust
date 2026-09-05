#!/usr/bin/env python3
"""Build a source-pinned SDK benchmark client with retained build progress.

The build receipt records provenance, not benchmark or publication success.
Use a clean immutable checkout so unrelated edits cannot alter build inputs.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('progress', ROOT / 'command-progress.py')
progress = importlib.util.module_from_spec(spec)
spec.loader.exec_module(progress)


def source(checkout):
    revision = subprocess.check_output(['git', '-C', str(checkout), 'rev-parse', 'HEAD'], text=True).strip()
    dirty = subprocess.check_output(['git', '-C', str(checkout), 'status', '--porcelain'], text=True)
    if dirty:
        raise ValueError('SDK client build requires a clean source checkout')
    return revision


def build(checkout, backend, output):
    if backend not in ('helix-sdk', 'surreal-sdk'):
        raise ValueError('unknown SDK backend')
    revision = source(checkout)
    recipe = (checkout / 'benchmarks/lsqb/Dockerfile').read_bytes()
    output.mkdir(exist_ok=False)
    feature = backend.removesuffix('-sdk')
    tag = f'grust-lsqb-{backend}:{revision[:12]}'
    command = ['docker', 'build', '--platform', 'linux/arm64', '--progress=plain',
               '--file', str(checkout / 'benchmarks/lsqb/Dockerfile'),
               '--build-arg', f'BENCHMARK_FEATURE={feature}',
               '--build-arg', f'GRUST_SOURCE_REVISION={revision}',
               '--iidfile', str(output / 'image-id'), '--tag', tag, str(checkout)]
    status = progress.run(command, output / 'build')
    if status:
        raise subprocess.CalledProcessError(status, command)
    if source(checkout) != revision:
        raise ValueError('source changed during SDK client build')
    image_id = (output / 'image-id').read_text().strip()
    image = json.loads(subprocess.check_output(['docker', 'image', 'inspect', image_id], timeout=30))[0]
    labels = image['Config'].get('Labels', {})
    if image['Id'] != image_id or image['Architecture'] != 'arm64' or image['Os'] != 'linux':
        raise ValueError('SDK client image identity/platform differs')
    if labels.get('org.opencontainers.image.revision') != revision or labels.get('io.adversarial.grust.benchmark-feature') != feature:
        raise ValueError('SDK client image source/feature differs')
    receipt = dict(schema='grust-sdk-client-build-v1', publication_qualified=False,
                   source_revision=revision, backend=backend, feature=feature,
                   image_id=image_id, image_tag=tag, image_labels=labels,
                   platform='linux/arm64', command=command,
                   recipe_sha256=hashlib.sha256(recipe).hexdigest(),
                   build_log_sha256=hashlib.sha256((output / 'build/command.log').read_bytes()).hexdigest())
    with (output / 'Dockerfile').open('xb') as stream:
        stream.write(recipe)
        stream.flush()
        os.fsync(stream.fileno())
    with (output / 'build-receipt.json').open('x') as stream:
        json.dump(receipt, stream, indent=2, sort_keys=True)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())
    print(json.dumps(dict(event='sdk-client-build-verified', image_id=image_id,
                         source_revision=revision, backend=backend)), flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--checkout', type=Path, required=True)
    parser.add_argument('--backend', choices=('helix-sdk', 'surreal-sdk'), required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    build(args.checkout.resolve(), args.backend, args.output.resolve())
