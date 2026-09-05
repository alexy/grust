#!/usr/bin/env python3
"""Derive performance tables only from verified, checksum-bound matrix evidence."""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import sys


def distribution(values):
    return dict(min_ns=min(values), median_ns=statistics.median(values), max_ns=max(values))


def summarize_query(query):
    measured, warmups = query['measurements'], query['warmups']
    samples = warmups + measured
    eligible = bool(measured) and query['outcome'] == 'pass' and all(x['outcome'] == 'pass' for x in samples)
    raw = {key: [x[key] for x in measured] for key in ('elapsed_ns', 'setup_ns', 'recovery_ns')}
    raw['sample_boundary_total_ns'] = [x['elapsed_ns'] + x['setup_ns'] + x['recovery_ns'] for x in measured]
    return dict(id=query['id'], outcome=query['outcome'], execution=query['execution'],
                warmup_count=len(warmups), measurement_count=len(measured),
                warmup_outcomes={key: sum(x['outcome'] == key for x in warmups)
                                for key in ('pass', 'mismatch', 'timeout', 'error')},
                measurement_outcomes={key: sum(x['outcome'] == key for x in measured)
                                     for key in ('pass', 'mismatch', 'timeout', 'error')},
                performance_eligible=eligible, measured_raw_ns=raw,
                statistics={key: distribution(values) for key, values in raw.items()} if eligible else None)


def summarize(directory):
    validator = Path(__file__).with_name('validate-matrix-publication.py')
    receipt_bytes = (directory / 'publication-receipt.json').read_bytes()
    subprocess.run([sys.executable, str(validator), 'verify', '--output-dir', str(directory)],
                   check=True, stdout=subprocess.DEVNULL)
    if (directory / 'publication-receipt.json').read_bytes() != receipt_bytes:
        raise ValueError('receipt changed during verification')
    receipt = json.loads(receipt_bytes)
    inventory = {entry['path']: entry for entry in receipt['output_inventory']}
    suites = []
    for suite in receipt['suite_order']:
        name = f"matrix-{suite}-sf{receipt['scale_factor']}.json"
        raw = (directory / name).read_bytes()
        expected = inventory[name]
        if len(raw) != expected['bytes'] or hashlib.sha256(raw).hexdigest() != expected['sha256']:
            raise ValueError('matrix changed after receipt verification')
        matrix = json.loads(raw)
        if matrix['schema_version'] != 3:
            raise ValueError('performance summary requires separated schema-v3 timing fields')
        suites.append(dict(suite=suite, environment=matrix['environment'], timing=matrix['timing'],
                           backends=[dict(identity=cell['backend'], lifecycle=cell['lifecycle'],
                                          setup_outcome=cell['setup_outcome'],
                                          coordinator_load_ns=cell.get('load_ns'),
                                          queries=[summarize_query(q) for q in cell['queries']])
                                     for cell in matrix['backends']]))
    if (directory / 'publication-receipt.json').read_bytes() != receipt_bytes:
        raise ValueError('receipt changed during summary generation')
    return dict(schema='grust-lsqb-performance-summary-v1',
                warning='These are not LDBC Benchmark Results.',
                source_revision=receipt['source_revision'], scale=receipt['scale_factor'],
                receipt_sha256=hashlib.sha256(receipt_bytes).hexdigest(),
                notes=['Warm-ups are excluded from timing statistics, but any failed warm-up suppresses statistics.',
                       'Coordinator loading is a single separately recorded duration, not a sampled distribution.',
                       'Sample boundary total sums setup, query and recovery per observation; it is not service throughput.',
                       'Execution class, lifecycle and resource limits must accompany cross-backend comparisons.'],
                suites=suites)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('--output', type=Path, help='write a new summary file; never overwrite')
    args = parser.parse_args()
    result = json.dumps(summarize(args.directory), indent=2) + '\n'
    if args.output:
        with args.output.open('x') as stream:
            stream.write(result)
    else:
        print(result, end='')
