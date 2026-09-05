#!/usr/bin/env python3
"""Derive performance tables only from verified, checksum-bound matrix evidence."""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import sys

from host_evidence import validate_record


PLAN_CLASSES = {
    'clause-pipeline': {'in-process-reference', 'backend-materialize-rust-reference'},
    'count-factorized': {'in-process-reference'},
    'sql-row-source': {'backend-row-source-rust-projection'},
    'sql-count': {'backend-native-aggregate'},
    'backend-native': {'backend-native-aggregate'},
}
PLAN_NAMES = tuple(PLAN_CLASSES)
PLAN_FILTERS = (*PLAN_NAMES, 'legacy')


def distribution(values):
    return dict(min_ns=min(values), median_ns=statistics.median(values), max_ns=max(values))


def observation_plan(observation, execution_class=None):
    if 'plan' not in observation:
        return None
    plan = observation['plan']
    if not isinstance(plan, str) or plan not in PLAN_NAMES:
        raise ValueError(f'invalid observation plan: {plan!r}')
    if execution_class not in PLAN_CLASSES[plan]:
        raise ValueError(
            f'observation plan {plan!r} does not match execution class {execution_class!r}')
    return plan


def validate_plan_filter(plan):
    if plan is not None and plan not in PLAN_FILTERS:
        raise ValueError(f'invalid plan filter: {plan!r}')


def summarize_samples(query, warmups, measured, declared_warmups, declared_measurements):
    samples = warmups + measured
    execution = query.get('execution')
    execution_class = execution.get('class') if isinstance(execution, dict) else None
    plans = sorted(
        {observation_plan(x, execution_class) for x in samples},
        key=lambda plan: plan or '')
    reasons = []
    if not measured:
        reasons.append('no-measurements')
    if query['outcome'] != 'pass':
        reasons.append('query-not-pass')
    if any(x['outcome'] != 'pass' for x in samples):
        reasons.append('failed-observations')
    if len(plans) > 1:
        reasons.append('mixed-plans')
    if len(warmups) != declared_warmups:
        reasons.append('incomplete-warmup-cohort')
    if len(measured) != declared_measurements:
        reasons.append('incomplete-measurement-cohort')
    eligible = not reasons
    raw = {key: [x[key] for x in measured] for key in ('elapsed_ns', 'setup_ns', 'recovery_ns')}
    raw['sample_boundary_total_ns'] = [x['elapsed_ns'] + x['setup_ns'] + x['recovery_ns'] for x in measured]
    return dict(id=query['id'], outcome=query['outcome'], execution=query['execution'],
                plan=plans[0] if len(plans) == 1 else None, plans=plans, mixed_plans=len(plans) > 1,
                declared_warmup_count=declared_warmups, declared_measurement_count=declared_measurements,
                warmup_count=len(warmups), measurement_count=len(measured),
                missing_warmup_count=max(0, declared_warmups - len(warmups)),
                missing_measurement_count=max(0, declared_measurements - len(measured)),
                warmup_outcomes={key: sum(x['outcome'] == key for x in warmups)
                                for key in ('pass', 'mismatch', 'timeout', 'error')},
                measurement_outcomes={key: sum(x['outcome'] == key for x in measured)
                                     for key in ('pass', 'mismatch', 'timeout', 'error')},
                performance_eligible=eligible, ineligibility_reasons=reasons, measured_raw_ns=raw,
                statistics={key: distribution(values) for key, values in raw.items()} if eligible else None)


def summarize_query(query, plan=None, *, warmup_iterations=None, measurement_iterations=None):
    validate_plan_filter(plan)
    warmups, measured = query['warmups'], query['measurements']
    execution = query.get('execution')
    execution_class = execution.get('class') if isinstance(execution, dict) else None
    source_plans = sorted(
        {observation_plan(x, execution_class) for x in warmups + measured},
        key=lambda value: value or '')
    declared_warmups = len(warmups) if warmup_iterations is None else warmup_iterations
    declared_measurements = len(measured) if measurement_iterations is None else measurement_iterations
    selected = None if plan == 'legacy' else plan
    selected_warmups = [
        x for x in warmups
        if plan is None or observation_plan(x, execution_class) == selected]
    selected_measured = [
        x for x in measured
        if plan is None or observation_plan(x, execution_class) == selected]
    result = summarize_samples(query, selected_warmups, selected_measured,
                               declared_warmups, declared_measurements)
    result.update(plan_filter=plan, source_plans=source_plans,
                  excluded_warmup_count=len(warmups) - len(selected_warmups),
                  excluded_measurement_count=len(measured) - len(selected_measured),
                  plan_subsets=[summarize_samples(
                      query,
                      [x for x in selected_warmups
                       if observation_plan(x, execution_class) == value],
                      [x for x in selected_measured
                       if observation_plan(x, execution_class) == value],
                      declared_warmups, declared_measurements) for value in result['plans']])
    return result


def read_bound_artifact(directory, inventory, name, label):
    raw = (directory / name).read_bytes()
    expected = inventory[name]
    if len(raw) != expected['bytes'] or hashlib.sha256(raw).hexdigest() != expected['sha256']:
        raise ValueError(f'{label} changed after receipt verification')
    return raw


def summarize_host_screen(directory, inventory):
    name = 'host-preflight.json'
    result = dict(status='unrecorded-legacy', startup_screen_passed=None,
                  evidence_path=None, evidence_sha256=None,
                  clean_host_performance_eligible=False,
                  limitation='Startup screening does not establish ongoing host isolation.')
    if name in inventory:
        raw = read_bound_artifact(directory, inventory, name, 'host preflight')
        validate_record(raw)
        result.update(status='recorded-startup-only', startup_screen_passed=True,
                      evidence_path=name, evidence_sha256=hashlib.sha256(raw).hexdigest())
    return result


def summarize(directory, plan=None):
    validate_plan_filter(plan)
    validator = Path(__file__).with_name('validate-matrix-publication.py')
    receipt_bytes = (directory / 'publication-receipt.json').read_bytes()
    subprocess.run([sys.executable, str(validator), 'verify', '--output-dir', str(directory)],
                   check=True, stdout=subprocess.DEVNULL)
    if (directory / 'publication-receipt.json').read_bytes() != receipt_bytes:
        raise ValueError('receipt changed during verification')
    receipt = json.loads(receipt_bytes)
    inventory = {entry['path']: entry for entry in receipt['output_inventory']}
    host_screen = summarize_host_screen(directory, inventory)
    suites = []
    for suite in receipt['suite_order']:
        name = f"matrix-{suite}-sf{receipt['scale_factor']}.json"
        raw = read_bound_artifact(directory, inventory, name, 'matrix')
        matrix = json.loads(raw)
        if matrix['schema_version'] != 3:
            raise ValueError('performance summary requires separated schema-v3 timing fields')
        suites.append(dict(suite=suite, environment=matrix['environment'], timing=matrix['timing'],
                           backends=[dict(identity=cell['backend'], lifecycle=cell['lifecycle'],
                                          setup_outcome=cell['setup_outcome'],
                                          coordinator_load_ns=cell.get('load_ns'),
                                          queries=[summarize_query(
                                              q, plan, warmup_iterations=matrix['timing']['warmup_iterations'],
                                              measurement_iterations=matrix['timing']['measurement_iterations'])
                                              for q in cell['queries']])
                                     for cell in matrix['backends']]))
    if (directory / 'publication-receipt.json').read_bytes() != receipt_bytes:
        raise ValueError('receipt changed during summary generation')
    return dict(schema='grust-lsqb-performance-summary-v1',
                warning='These are not LDBC Benchmark Results.',
                source_revision=receipt['source_revision'], scale=receipt['scale_factor'],
                plan_filter=plan, host_screen=host_screen,
                receipt_sha256=hashlib.sha256(receipt_bytes).hexdigest(),
                notes=['Warm-ups are excluded from timing statistics, but any failed warm-up suppresses statistics.',
                       'Coordinator loading is a single separately recorded duration, not a sampled distribution.',
                       'Sample boundary total sums setup, query and recovery per observation; it is not service throughput.',
                       'Execution class, plan, lifecycle and resource limits must accompany cross-backend comparisons.',
                       'A missing observation plan remains null (legacy unknown); backend-native does not describe a server physical plan.',
                       'count-factorized and sql-count are admitted only for query shapes authenticated by the bundled execution-plan registry.',
                       'Per-query performance_eligible means a successful fixed-plan cohort (or uniformly legacy-unknown plans), not clean-host performance qualification; a startup screen does not verify ongoing isolation.',
                       'Mixed plans, including warm-ups, suppress pooled statistics. Plan subsets and filters require the full declared warm-up and measurement cohort.'],
                suites=suites)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('--output', type=Path, help='write a new summary file; never overwrite')
    parser.add_argument('--plan', choices=PLAN_FILTERS,
                        help='select one observed plan, or legacy for observations without a plan; incomplete cohorts have no statistics')
    args = parser.parse_args()
    result = json.dumps(summarize(args.directory, args.plan), indent=2) + '\n'
    if args.output:
        with args.output.open('x') as stream:
            stream.write(result)
    else:
        print(result, end='')
