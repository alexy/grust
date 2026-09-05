#!/usr/bin/env python3
"""Build the pinned public Sail recipe with retained local provenance."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

REVISION = '4995115ad95e7e12215e86bcc13e60a78ddcea00'


def build(checkout, output):
    def git(*args):
        return subprocess.check_output(['git', '-C', str(checkout), *args], text=True).strip()
    def check_source():
        if git('rev-parse', 'HEAD') != REVISION or git('status', '--porcelain'):
            raise ValueError('requires a clean checkout of the pinned recipe revision')
    check_source()
    recipe = (checkout / 'benchmarks/lsqb/Dockerfile.sail').read_bytes()
    output.mkdir(exist_ok=False)
    tag = 'grust-sail-source:0.7.1-' + REVISION[:7]
    command = ['docker', 'build', '--platform', 'linux/arm64', '--progress=plain',
               '-f', 'benchmarks/lsqb/Dockerfile.sail', '--build-arg', 'GRUST_SOURCE_REVISION=' + REVISION,
               '--iidfile', str(output / 'image-id'), '-t', tag, '.']
    started = time.monotonic()
    with (output / 'build.log').open('xb', buffering=0) as log, (output / 'progress.jsonl').open('x') as journal:
        def event(kind, **fields):
            line = json.dumps(dict(event=kind, elapsed_s=round(time.monotonic() - started, 3), **fields)) + '\n'
            journal.write(line)
            journal.flush()
            os.fsync(journal.fileno())
            print(line, end='', flush=True)
        process = subprocess.Popen(command, cwd=checkout, stdout=log, stderr=subprocess.STDOUT)
        event('source-build-start', pid=process.pid, source_revision=REVISION, command=command)
        while True:
            try:
                status = process.wait(timeout=30)
                break
            except subprocess.TimeoutExpired:
                os.fsync(log.fileno())
                with (output / 'build.log').open('rb') as reader:
                    reader.seek(max(0, reader.seek(0, 2) - 1024))
                    tail = reader.read().decode(errors='replace').splitlines()
                event('source-build-progress', log_bytes=log.tell(), latest_output=tail[-1] if tail else '')
        os.fsync(log.fileno())
        event('source-build-finish', exit=status, log_bytes=log.tell())
        if status:
            raise subprocess.CalledProcessError(status, command)
        check_source()
        image_id = (output / 'image-id').read_text().strip()
        image = json.loads(subprocess.check_output(['docker', 'image', 'inspect', image_id], timeout=30))[0]
        labels = image['Config']['Labels']
        if image['Id'] != image_id or image['Architecture'] != 'arm64' or image['Os'] != 'linux':
            raise ValueError('built image platform/identity differs')
        if labels.get('org.opencontainers.image.revision') != REVISION or labels.get('org.opencontainers.image.version') != '0.7.1':
            raise ValueError('built image source/version labels differ')
        version = subprocess.check_output(['docker', 'run', '--rm', '--network', 'none', image_id, '--version'],
                                          text=True, timeout=30).strip()
        if version != 'sail 0.7.1':
            raise ValueError('built runtime version differs')
        receipt = dict(schema='grust-sail-source-build-v1', source_revision=REVISION,
                       recipe_path='benchmarks/lsqb/Dockerfile.sail', recipe_sha256=hashlib.sha256(recipe).hexdigest(),
                       image_id=image_id, image_tag=tag, image_labels=labels, platform='linux/arm64',
                       runtime_version=version, publication_qualified=False,
                       build_log_sha256=hashlib.sha256((output / 'build.log').read_bytes()).hexdigest(),
                       distribution='Grust-built pinned upstream wheel; not a public registry image')
        with (output / 'Dockerfile.sail').open('xb') as stream:
            stream.write(recipe)
        with (output / 'build-receipt.json').open('x') as stream:
            json.dump(receipt, stream, indent=2, sort_keys=True)
            stream.write('\n')
            stream.flush()
            os.fsync(stream.fileno())
        event('source-build-verified', image_id=image_id, runtime_version=version)
        return receipt


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--checkout', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    build(args.checkout.resolve(), args.output.resolve())
