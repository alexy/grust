#!/usr/bin/env python3
"""Audit native Neo4j diagnostics; never issue a publication receipt.

Checks counts against the pinned upstream oracle and local attack sources,
ordered incremental observations, process termination and server recovery.
Does NOT authenticate container/source provenance or qualify performance claims.
"""
import argparse
import csv
import hashlib
import io
import json
from pathlib import Path
import re

ORACLE_SHA = 'f2467b14cd6a060e8513d5357471ae6cff486c2f5e38074febe08a4cf4db0d3a'
DATASETS = {
    'example': (28, 72, 5, 'e47d935e186ccda58147fc2609d3db1a6f0e218b92384cf63a7161e2c2974def'),
    '0.1': (432235, 2080404, 1700, 'c0d76ea897df030f901c7436d2d7ee0cd31591db54c3c6c311d79a68fa138085'),
    '0.3': (1179535, 6183839, 3900, 'aeb94da1177ca732b127574116d7624b131113ffc7f6f8e612b0bb2dab31d5f3'),
}
ATTACKS = ('reversed-chain', 'reordered-join', 'split-match', 'optional-fanout',
           'negated-pattern', 'range-expansion', 'cartesian-count', 'union-dedup',
           'path-zero-hop', 'unicode-literal', 'schema-null-probe',
           'parser-comment-trivia', 'resource-edge-scan')


def require(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value):
    return type(value) is int and value >= 0


def check_observation(item, expected, seen_tags):
    suite, query_id, count, digest = expected
    require(item.get('event') == 'observation-recorded' and item.get('complete') is False,
            'invalid diagnostic observation marker')
    require((item.get('suite'), item.get('id')) == (suite, query_id), 'query order/identity differs')
    require(type(item.get('expected_count')) is int and item['expected_count'] == count, 'oracle differs')
    require(item.get('source_sha256') == digest == item.get('query_sha256'), 'query source differs')
    require(item.get('timing_boundary') == 'coordinator-go-through-scalar-consumption-and-rollback-result',
            'unsupported timing boundary')
    for key in ('setup_ns', 'elapsed_ns', 'process_recovery_ns'):
        require(integer(item.get(key)), 'invalid timing: ' + key)
    require(type(item.get('query_timeout_ms')) is int and item['query_timeout_ms'] == 60000,
            'unexpected query deadline')
    outcome, actual, termination = item.get('outcome'), item.get('actual_count'), item.get('termination')
    if outcome in ('pass', 'mismatch'):
        require(integer(actual) and (actual == count) == (outcome == 'pass'), 'incorrect count classification')
        require(termination == 'normal-exit', 'completed count lacks normal exit')
    elif outcome == 'timeout':
        require(actual is None and termination in ('deadline-sigterm', 'deadline-sigkill'), 'unproved timeout')
        require(item['elapsed_ns'] >= 60000 * 1000000, 'timeout precedes deadline')
    else:
        require(outcome == 'error' and actual is None and termination == 'normal-exit', 'invalid error classification')
    if outcome != 'timeout':
        require(item['elapsed_ns'] <= 60000 * 1000000, 'late non-timeout result')
    recovery = item.get('server_recovery', {})
    require(type(recovery.get('owned_transactions_remaining')) is int and
            recovery['owned_transactions_remaining'] == 0, 'remote work remains')
    require(type(recovery.get('subsequent_scalar')) is int and recovery['subsequent_scalar'] == 42,
            'next-query recovery missing')
    require(integer(recovery.get('server_recovery_ns')), 'missing recovery timing')
    tag = recovery.get('transaction_tag')
    require(isinstance(tag, str) and re.fullmatch(r'neo4j-[0-9]+-[0-9]+', tag) and tag not in seen_tags,
            'invalid or reused transaction tag')
    seen_tags.add(tag)
    ids = recovery.get('terminated_transaction_ids')
    require(isinstance(ids, list) and len(ids) <= 1 and all(isinstance(x, str) and x for x in ids),
            'invalid targeted termination identity')
    require(type(recovery.get('targeted_termination_count')) is int and
            recovery['targeted_termination_count'] == len(ids), 'termination count differs')


def validate(directory, upstream, attacks):
    report = json.loads((directory / 'result/diagnostic.json').read_text())
    require(report.get('schema') == 'grust-neo4j-native-diagnostic-v1' and
            report.get('complete') is False and report.get('publication_receipt') is None,
            'not a non-publishable diagnostic')
    require((report.get('driver'), report.get('driver_version')) == ('neo4rs', '0.9.0-rc.10'), 'driver differs')
    scale = report.get('scale')
    require(scale in DATASETS, 'unknown dataset')
    nodes, edges, people, manifest = DATASETS[scale]
    dataset = report['dataset']
    require(dataset.get('scale_factor') == scale and integer(report.get('load_ns')), 'invalid dataset scale/load timing')
    require((dataset.get('nodes'), dataset.get('edges'), dataset.get('person_nodes'),
             dataset.get('extracted_manifest_sha256')) == (nodes, edges, people, manifest), 'dataset differs')
    oracle = (upstream / 'expected-output/expected-output.csv').read_bytes()
    require(hashlib.sha256(oracle).hexdigest() == ORACLE_SHA, 'upstream oracle is not pinned')
    counts = {int(row[3]): int(row[5]) for row in csv.reader(io.StringIO(oracle.decode()), delimiter='\t')
              if row[2] == scale}
    require(set(counts) == set(range(1, 10)), 'incomplete upstream oracle')
    expected = []
    for number in range(1, 10):
        digest = hashlib.sha256((upstream / f'cypher/q{number}.cypher').read_bytes()).hexdigest()
        expected.append(('baseline', f'q{number}', counts[number], digest))
    attack_counts = [counts[1], counts[2], counts[4], counts[7], counts[8], 10000,
                     people**3, people, people, people, people, nodes, edges]
    for number, (suffix, count) in enumerate(zip(ATTACKS, attack_counts), 1):
        query_id = f'a{number}-{suffix}'
        digest = hashlib.sha256((attacks / f'{query_id}.cypher').read_bytes()).hexdigest()
        expected.append(('adversarial', query_id, count, digest))
    observations = report['observations']
    require(len(observations) == 22, 'incomplete observation set')
    tags = set()
    for item, reference in zip(observations, expected):
        check_observation(item, reference, tags)
    events = [json.loads(line) for line in (directory / 'result/observations.jsonl').read_text().splitlines()]
    require([x for x in events if x.get('event') == 'observation-recorded'] == observations, 'journal differs')
    pending, index, totals = None, 0, (0, 0, 0)
    for event in events:
        kind = event.get('event')
        require(event.get('complete') is False, 'journal claims publication completion')
        if kind == 'load-progress':
            new = tuple(event.get(k) for k in ('nodes', 'edges', 'elapsed_ms'))
            require(index == 0 and pending is None and all(integer(x) for x in new) and
                    all(a <= b for a, b in zip(totals, new)), 'invalid load progress')
            totals = new
        elif kind == 'query-start':
            require(index < 22 and pending is None and totals[:2] == (nodes, edges), 'invalid query start')
            pending = (event.get('suite'), event.get('id'))
            require(pending == expected[index][:2], 'query start order differs')
        else:
            require(kind == 'observation-recorded' and pending == (event.get('suite'), event.get('id')),
                    'unmatched observation')
            pending, index = None, index + 1
    require(index == 22 and pending is None, 'incomplete journal')
    require(report['load_ns'] >= totals[2] * 1000000, 'load time precedes final load progress')
    watchdog = json.loads((directory / 'watchdog.json').read_text())
    require(watchdog.get('schema') == 'grust-lsqb-cell-watchdog-completion-v1' and
            watchdog.get('status') == 'complete' and type(watchdog.get('child_exit_status')) is int and
            watchdog['child_exit_status'] == 0, 'watchdog did not complete successfully')
    require(integer(watchdog.get('elapsed_wall_ms')) and integer(watchdog.get('timeout_ms')) and
            0 < watchdog['elapsed_wall_ms'] <= watchdog['timeout_ms'], 'invalid watchdog timing')
    return {'diagnostic_verified': True, 'publication_qualified': False, 'scale': scale,
            'outcomes': {key: sum(x['outcome'] == key for x in observations)
                         for key in ('pass', 'mismatch', 'timeout', 'error')}}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('--upstream', type=Path, default=Path(__file__).parent / 'upstream/lsqb')
    parser.add_argument('--attacks', type=Path, default=Path(__file__).parent / 'attacks')
    args = parser.parse_args()
    print(json.dumps(validate(args.directory, args.upstream, args.attacks)))
