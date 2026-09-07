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
import statistics

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
SOURCE_REVISION = 'aaf0999706fa8cfdb7eeb10e8349b9a471229857'
CLIENT_IMAGE_ID = 'sha256:ff4f6c185f1d10a6ee63a815f823c0dfa48d917bcb25a0e2998d20c6291717b6'
CLIENT_PROFILES = {
    SOURCE_REVISION: CLIENT_IMAGE_ID,
    'bb4f7c161fdae90cc9fc2b35aaf0870e9da91164':
        'sha256:b68b801a134fc85af5e9a44486a1309b45315b2ff7e67eab8489e24dab51c9ed',
    '4c385e26135547f1771577f20a90234f830488b6':
        'sha256:91e1cec7607127a56f26859bcc7d3750b41d687a40b542f310e8047863117e4c',
    '242b6b842836e64fb76e667f8ad5609e7cb2c115':
        'sha256:7e20f8504f9ad0a04aa72226d4493c501cdaa2ebc62cd8041824fbcd2333f27e',
    '4995115ad95e7e12215e86bcc13e60a78ddcea00':
        'sha256:4934c9343e3a5edb03e4f13f0b8ccea48ec7a16142e3497bdb4ab649aded69b4',
}
SAMPLED_SOURCES = {'4c385e26135547f1771577f20a90234f830488b6', '242b6b842836e64fb76e667f8ad5609e7cb2c115',
                   '4995115ad95e7e12215e86bcc13e60a78ddcea00'}
ROTATING_SOURCES = {'4995115ad95e7e12215e86bcc13e60a78ddcea00'}
# The pinned tag's platform images (one multi-platform index, sha256:dbc377fb9cd8fe8dabc19d3041b197d5ca0ef8bae514cea175b8df265e5b7a76):
# a run's server must be one of them, and its retained image ID must match the
# one its invocation names.
SERVER_IMAGES = {
    'neo4j:2026.07.1-community@sha256:31697c776d8c255152be39430d4b306a414c1409c91dccd093ac5e6baf2cae9d',  # linux/arm64
    'neo4j:2026.07.1-community@sha256:a9d46c947a02de4fbaecc9adcca17d197661e32d31df8a944b4294259816a7a9',  # linux/amd64
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value):
    return type(value) is int and value >= 0


def require_matched_sampling(report):
    """Configuration gate, called only after full diagnostic/runtime validation."""
    require(report.get('schema') == 'grust-neo4j-native-diagnostic-v2', 'matched sampling requires schema v2')
    sampling = report.get('sampling', {})
    require(sampling.get('order') == 'suite-major-phase-major-rotating' and
            sampling.get('warmups_per_query') == 2 and sampling.get('measurements_per_query') == 10,
            'not the rotating W2/R10 comparison sampling cohort')
    require(all(item.get('query_timeout_ms') == 60000 for item in report['observations']),
            'comparison query deadline differs')


def summarize_measurements(report):
    """Derive statistics only when every warm-up and measured count passed."""
    grouped = {}
    sampled = report['schema'] == 'grust-neo4j-native-diagnostic-v2'
    for item in report['observations']:
        grouped.setdefault((item['suite'], item['id']), []).append(item)
    summaries = []
    for (suite, query_id), items in grouped.items():
        measured = [item for item in items if not sampled or item['phase'] == 'measurement']
        durations = [item['elapsed_ns'] for item in measured]
        eligible = bool(durations) and all(item['outcome'] == 'pass' for item in items)
        summaries.append(dict(suite=suite, id=query_id, measurement_samples=len(measured),
                              measurement_elapsed_ns=durations,
                              timing_summary=dict(min_ns=min(durations), median_ns=statistics.median(durations),
                                                  max_ns=max(durations)) if eligible else None))
    return summaries


def validate_network_record(record):
    """Validate a retained pre-run network inspection, not historical isolation."""
    require(set(record) == {'Name', 'Id', 'Internal'}, 'network record fields differ')
    require(record['Name'] == 'grust-lsqb-neo4j-qualification'
            and record['Internal'] is True, 'network was not internal')
    require(isinstance(record['Id'], str) and re.fullmatch(r'[0-9a-f]{64}', record['Id']),
            'invalid network identity')


def validate_runtime(directory):
    """Validate retained records, not registry availability or measurement isolation."""
    records = {name: json.loads((directory / f'{name}.json').read_text()) for name in
               ('invocation', 'watchdog', 'client-before', 'client-after', 'server-before', 'server-after')}
    invocation, watchdog = records['invocation'], records['watchdog']
    revision = invocation.get('source_revision')
    require(isinstance(revision, str) and revision in CLIENT_PROFILES, 'unqualified source revision')
    client_image = CLIENT_PROFILES[revision]
    require(invocation.get('diagnostic_only') is True, 'runtime lane is not diagnostic')
    require(invocation.get('client_image_id') == client_image and
            invocation.get('server_image') in SERVER_IMAGES, 'runtime source/image identity differs')
    labels = invocation.get('client_labels', {})
    require(labels.get('org.opencontainers.image.revision') == revision and
            labels.get('io.adversarial.grust.benchmark-feature') == 'neo4j-native', 'client source labels differ')
    report_path = directory / 'result/diagnostic.json'
    if report_path.exists():
        report = json.loads(report_path.read_text())
        if report.get('schema') == 'grust-neo4j-native-diagnostic-v2':
            require(revision in SAMPLED_SOURCES, 'source does not qualify the sampling protocol')
            sampling = report['sampling']
            expected_order = ('suite-major-phase-major-rotating' if revision in ROTATING_SOURCES
                              else 'query-major-warmups-then-measurements')
            require(sampling.get('order') == expected_order, 'source does not implement the declared sampling order')
            command = invocation.get('command', [])
            require(isinstance(command, list) and len(command) >= 7 and command[-7] == 'qualify' and
                    command[-4] == report['scale'] and command[-3] == '/out/result' and
                    command[-2:] == [str(sampling['warmups_per_query']), str(sampling['measurements_per_query'])],
                    'sampling report differs from the container invocation')
    for role in ('client', 'server'):
        before, after = records[f'{role}-before'], records[f'{role}-after']
        require(isinstance(before.get('container_id'), str) and
                re.fullmatch(r'[0-9a-f]{64}', before['container_id']), 'invalid runtime container identity')
        for key in ('container_id', 'image_id', 'name', 'resources', 'labels'):
            require(before.get(key) == after.get(key), f'{role} runtime identity/resources changed')
        resource = before['resources']
        require(resource.get('Memory') == resource.get('MemorySwap') == 6 * 1024**3 and
                resource.get('NanoCpus') == 8_000_000_000 and
                resource.get('NetworkMode') == 'grust-lsqb-neo4j-qualification', 'runtime limits differ')
        for record in (before, after):
            require(record['state'].get('OOMKilled') is False and
                    type(record['state'].get('ExitCode')) is int and record['state']['ExitCode'] == 0,
                    'runtime OOM or nonzero exit')
        if role == 'client':
            require(before['image_id'] == client_image and resource.get('ReadonlyRootfs') is True,
                    'client runtime image/filesystem differs')
            require(before['state'].get('Status') == 'created' and before['state'].get('Running') is False and
                    after['state'].get('Status') == 'exited' and after['state'].get('Running') is False,
                    'client lifecycle incomplete')
            require(watchdog.get('container_id') == before['container_id'] and
                    watchdog.get('container_name') == before['name'].lstrip('/') and
                    watchdog.get('project') == before['labels'].get('com.docker.compose.project') and
                    watchdog.get('service') == before['labels'].get('com.docker.compose.service'),
                    'watchdog/client ownership differs')
        else:
            require(before['image_id'] == invocation['server_image'].rsplit('@', 1)[1], 'server runtime image differs')
            require(before['state'].get('Running') is True and after['state'].get('Running') is True and
                    before['state'].get('StartedAt') == after['state'].get('StartedAt'), 'server restarted or stopped')
    return True


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
    disappeared = recovery.get('disappeared_before_termination_ids', [])
    require(isinstance(disappeared, list) and len(disappeared) + len(ids) <= 1 and
            all(isinstance(x, str) and x and x not in ids for x in disappeared),
            'invalid disappeared transaction identity')


def validate(directory, upstream, attacks):
    report = json.loads((directory / 'result/diagnostic.json').read_text())
    require(report.get('schema') in ('grust-neo4j-native-diagnostic-v1', 'grust-neo4j-native-diagnostic-v2') and
            report.get('complete') is False and report.get('publication_receipt') is None,
            'not a non-publishable diagnostic')
    sampled = report['schema'] == 'grust-neo4j-native-diagnostic-v2'
    if sampled:
        sampling = report.get('sampling', {})
        warmups, runs = sampling.get('warmups_per_query'), sampling.get('measurements_per_query')
        require(type(warmups) is int and 0 <= warmups <= 5 and type(runs) is int and 1 <= runs <= 10,
                'invalid sampling counts')
        require(sampling.get('order') in ('query-major-warmups-then-measurements', 'suite-major-phase-major-rotating') and
                sampling.get('worker_lifecycle') == 'fresh-process-per-sample', 'sampling protocol differs')
        schedule = [('warmup', i) for i in range(warmups)] + [('measurement', i) for i in range(runs)]
    else:
        require('sampling' not in report, 'legacy diagnostic cannot declare another sampling protocol')
        warmups, runs, schedule = 0, 1, [(None, None)]
    require((report.get('driver'), report.get('driver_version')) == ('neo4rs', '0.9.0-rc.10'), 'driver differs')
    scale = report.get('scale')
    require(scale in DATASETS, 'unknown dataset')
    nodes, edges, people, manifest = DATASETS[scale]
    dataset = report['dataset']
    require(dataset.get('scale_factor') == scale and integer(report.get('load_ns')), 'invalid dataset scale/load timing')
    if 'load_timeout_ms' in report:
        require(type(report['load_timeout_ms']) is int and report['load_timeout_ms'] == 600000 and
                report['load_ns'] <= 600000 * 1000000, 'declared load deadline was not respected')
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
    if sampled and sampling['order'] == 'suite-major-phase-major-rotating':
        references, expected = expected, []
        for suite in ('baseline', 'adversarial'):
            group = [case for case in references if case[0] == suite]
            for phase, index in schedule:
                expected.extend((*group[(position + index) % len(group)], phase, index)
                                for position in range(len(group)))
    else:
        expected = [(*case, phase, index) for case in expected for phase, index in schedule]
    observations = report['observations']
    require(len(observations) == len(expected), 'incomplete observation set')
    tags = set()
    for item, reference in zip(observations, expected):
        check_observation(item, reference[:4], tags)
        if sampled:
            require(type(item.get('sample_index')) is int and
                    (item.get('phase'), item['sample_index']) == reference[4:], 'sample identity/order differs')
        else:
            require('phase' not in item and 'sample_index' not in item, 'legacy sample is ambiguously phased')
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
            require(index < len(expected) and pending is None and totals[:2] == (nodes, edges), 'invalid query start')
            pending = (event.get('suite'), event.get('id'), event.get('phase'), event.get('sample_index'))
            require(pending == (*expected[index][:2], *expected[index][4:]), 'query start order differs')
            require(not sampled or type(event.get('sample_index')) is int, 'invalid start sample index')
        else:
            require(kind == 'observation-recorded' and pending ==
                    (event.get('suite'), event.get('id'), event.get('phase'), event.get('sample_index')),
                    'unmatched observation')
            pending, index = None, index + 1
    require(index == len(expected) and pending is None, 'incomplete journal')
    require(report['load_ns'] >= totals[2] * 1000000, 'load time precedes final load progress')
    watchdog = json.loads((directory / 'watchdog.json').read_text())
    require(watchdog.get('schema') == 'grust-lsqb-cell-watchdog-completion-v1' and
            watchdog.get('status') == 'complete' and type(watchdog.get('child_exit_status')) is int and
            watchdog['child_exit_status'] == 0, 'watchdog did not complete successfully')
    require(integer(watchdog.get('elapsed_wall_ms')) and integer(watchdog.get('timeout_ms')) and
            0 < watchdog['elapsed_wall_ms'] <= watchdog['timeout_ms'], 'invalid watchdog timing')
    measurements = [x for x in observations if not sampled or x['phase'] == 'measurement']
    warmup_samples = [x for x in observations if sampled and x['phase'] == 'warmup']
    return {'diagnostic_verified': True, 'publication_qualified': False, 'scale': scale,
            'sampling_order': sampling['order'] if sampled else 'single-pass-canonical',
            'query_timeout_ms': 60000,
            'warmups_per_query': warmups, 'measurements_per_query': runs,
            'measurement_outcomes': {key: sum(x['outcome'] == key for x in measurements)
                                     for key in ('pass', 'mismatch', 'timeout', 'error')},
            'warmup_outcomes': {key: sum(x['outcome'] == key for x in warmup_samples)
                               for key in ('pass', 'mismatch', 'timeout', 'error')},
            'outcomes': {key: sum(x['outcome'] == key for x in observations)
                         for key in ('pass', 'mismatch', 'timeout', 'error')}}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('--upstream', type=Path, default=Path(__file__).parent / 'upstream/lsqb')
    parser.add_argument('--attacks', type=Path, default=Path(__file__).parent / 'attacks')
    parser.add_argument('--runtime', action='store_true', help='also require retained runtime evidence')
    parser.add_argument('--summaries', action='store_true', help='include measured-only timing series and summaries')
    parser.add_argument('--matched-sampling', action='store_true', help='require rotating W2/R10/60s sampling (also requires --runtime)')
    args = parser.parse_args()
    if args.matched_sampling and not args.runtime:
        parser.error('--matched-sampling requires --runtime')
    result = validate(args.directory, args.upstream, args.attacks)
    if args.runtime:
        result['runtime_verified'] = validate_runtime(args.directory)
    if args.matched_sampling:
        require_matched_sampling(json.loads((args.directory / 'result/diagnostic.json').read_text()))
        result['matched_sampling_verified'] = True
    if args.summaries:
        result['query_summaries'] = summarize_measurements(json.loads((args.directory / 'result/diagnostic.json').read_text()))
    print(json.dumps(result))
