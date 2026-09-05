#!/usr/bin/env python3
"""Offline schema-v3 observation-plan contract tests."""

import copy
import importlib.util
import json
from pathlib import Path
import unittest


SPEC = importlib.util.spec_from_file_location(
    'publication', Path(__file__).with_name('validate-matrix-publication.py'))
publication = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(publication)
MANIFEST_PATH = Path(__file__).with_name('evidence-manifest-v2.json')


def report():
    observation = dict(elapsed_ns=10, iteration=1, outcome='pass', query_position=1,
                       recovery_ns=0, setup_ns=0, termination='normal-exit')
    return dict(schema_version=3, timing=dict(
        boundary='coordinator-go-to-result-consumed', cell_timeout_ms=1000,
        measurement_iterations=1, query_kill_reap_timeout_ms=100,
        query_order='rotating', query_reap_grace_ms=10, query_recovery_timeout_ms=100,
        query_timeout_ms=100, timeout_enforcement='coordinator-process-group',
        warmup_iterations=1, worker_ready_timeout_ms=100),
        backends=[dict(backend={'name': 'memory'}, setup_outcome='pass',
                       lifecycle=dict(load_strategy='per-observation-worker-reload',
                                      recovery_contract='process-group-absent'),
                       queries=[dict(execution={'class': 'in-process-reference'},
                                     warmups=[copy.deepcopy(observation)],
                                     measurements=[copy.deepcopy(observation)])])])


def manifest_with_registry(backend='memory', query_id='q1'):
    manifest = json.loads(MANIFEST_PATH.read_text())
    canonical = next(
        track['queries'][query_id]
        for track in manifest['tracks'].values()
        if query_id in track['queries'])
    if backend == 'memory':
        entry = dict(
            plan='count-factorized', execution_class='in-process-reference',
            source_sha256=canonical['source_sha256'],
            adapter_sha256=canonical['adapter_sha256'],
            rust_rows={'kind': 'not-materialized', 'rows': 0},
            backend_query_sha256=None)
    else:
        entry = dict(
            plan='sql-count', execution_class='backend-native-aggregate',
            source_sha256=canonical['source_sha256'],
            adapter_sha256=canonical['adapter_sha256'], rust_rows=None,
            backend_query_sha256='b' * 64)
    manifest['execution_plans'] = dict(
        schema=publication.EXECUTION_PLAN_REGISTRY_SCHEMA,
        entries={backend: {query_id: entry}})
    return manifest, entry


def optimized_report(backend='memory', query_id='q1', *, observations=True):
    manifest, entry = manifest_with_registry(backend, query_id)
    value = report()
    value['backends'][0]['backend']['name'] = backend
    query = value['backends'][0]['queries'][0]
    query.update(
        id=query_id, source_sha256=entry['source_sha256'],
        adapter_sha256=entry['adapter_sha256'], rust_rows=entry['rust_rows'],
        execution={
            'class': entry['execution_class'],
            'backend_query_sha256': entry['backend_query_sha256'],
        })
    if observations:
        for sample in query['warmups'] + query['measurements']:
            sample['plan'] = entry['plan']
    else:
        query['warmups'] = []
        query['measurements'] = []
    return manifest, value


class ObservationPlanTests(unittest.TestCase):
    def validate(self, value):
        publication.validate_v3_timeout_contract(value, 'fixture.json')

    def test_legacy_absence_is_accepted_without_inference_or_mutation(self):
        value = report()
        original = copy.deepcopy(value)
        self.validate(value)
        self.assertEqual(value, original)
        self.assertNotIn('plan', value['backends'][0]['queries'][0]['measurements'][0])

    def test_each_advertised_plan_matches_its_execution_classes(self):
        for plan, classes in publication.OBSERVATION_PLAN_CLASSES.items():
            for execution_class in classes:
                with self.subTest(plan=plan, execution_class=execution_class):
                    value = report()
                    if plan == 'sql-count':
                        value['backends'][0]['backend']['name'] = 'turso'
                    query = value['backends'][0]['queries'][0]
                    query['execution']['class'] = execution_class
                    for sample in query['warmups'] + query['measurements']:
                        sample['plan'] = plan
                    self.validate(value)

    def test_invalid_values_are_rejected_in_warmups_and_measurements(self):
        for phase in ('warmups', 'measurements'):
            for plan in (None, '', 'unknown', 'count-tree', 'legacy', 1, True, [], {}):
                with self.subTest(phase=phase, plan=plan):
                    value = report()
                    value['backends'][0]['queries'][0][phase][0]['plan'] = plan
                    with self.assertRaisesRegex(publication.PublicationError, 'invalid observation plan'):
                        self.validate(value)

    def test_wrong_or_missing_execution_class_is_rejected(self):
        for execution in ({'class': 'backend-native-aggregate'}, {'class': 'unknown'},
                          {'class': []}, {}, None, 'in-process-reference'):
            value = report()
            query = value['backends'][0]['queries'][0]
            query['execution'] = execution
            query['measurements'][0]['plan'] = 'clause-pipeline'
            with self.subTest(execution=execution), self.assertRaisesRegex(
                    publication.PublicationError, 'does not match execution class'):
                self.validate(value)

    def test_native_and_sql_plans_cannot_claim_the_reference_class(self):
        for plan in ('backend-native', 'sql-row-source', 'sql-count'):
            value = report()
            value['backends'][0]['queries'][0]['measurements'][0]['plan'] = plan
            with self.subTest(plan=plan), self.assertRaisesRegex(
                    publication.PublicationError, 'does not match execution class'):
                self.validate(value)

    def test_optimized_plans_are_restricted_to_their_backends(self):
        for plan, backend, execution_class in (
                ('count-factorized', 'ladybug', 'in-process-reference'),
                ('sql-count', 'memory', 'backend-native-aggregate')):
            value = report()
            value['backends'][0]['backend']['name'] = backend
            query = value['backends'][0]['queries'][0]
            query['execution']['class'] = execution_class
            query['measurements'][0]['plan'] = plan
            with self.subTest(plan=plan, backend=backend), self.assertRaisesRegex(
                    publication.PublicationError, 'does not match backend'):
                self.validate(value)

    def test_legacy_and_tagged_samples_remain_valid_evidence(self):
        value = report()
        query = value['backends'][0]['queries'][0]
        query['measurements'][0]['plan'] = 'clause-pipeline'
        self.validate(value)
        self.assertNotIn('plan', query['warmups'][0])


class ExecutionPlanRegistryTests(unittest.TestCase):
    def validate(self, manifest, value):
        publication.validate_report_execution_plans(value, manifest, 'fixture.json')

    def test_exact_memory_and_sql_registry_contracts_are_accepted(self):
        for backend in ('memory', 'turso', 'postgres'):
            with self.subTest(backend=backend):
                manifest, value = optimized_report(backend)
                publication.manifest_execution_plans(manifest)
                self.validate(manifest, value)

    def test_planned_nonexecuted_shape_is_evidence_but_not_an_observation(self):
        for backend in ('memory', 'turso'):
            with self.subTest(backend=backend):
                manifest, value = optimized_report(backend, observations=False)
                self.validate(manifest, value)
                query = value['backends'][0]['queries'][0]
                self.assertEqual(query['warmups'] + query['measurements'], [])

    def test_every_optimized_observation_must_name_the_authorized_plan(self):
        for replacement in (None, 'clause-pipeline', 'backend-native'):
            manifest, value = optimized_report('memory')
            sample = value['backends'][0]['queries'][0]['warmups'][0]
            if replacement is None:
                del sample['plan']
            else:
                sample['plan'] = replacement
            with self.subTest(replacement=replacement), self.assertRaisesRegex(
                    publication.PublicationError, 'one plan for every observation'):
                self.validate(manifest, value)

    def test_unknown_query_cannot_claim_not_materialized_rows(self):
        manifest, value = optimized_report('memory')
        query = value['backends'][0]['queries'][0]
        query['id'] = 'q2'
        with self.assertRaisesRegex(
                publication.PublicationError, 'not authorized by the manifest'):
            self.validate(manifest, value)

    def test_sql_digest_and_adapter_hash_are_part_of_authorization(self):
        for field, value in (
                ('backend_query_sha256', 'c' * 64),
                ('adapter_sha256', 'd' * 64),
                ('source_sha256', 'e' * 64)):
            manifest, report_value = optimized_report('turso')
            query = report_value['backends'][0]['queries'][0]
            if field == 'backend_query_sha256':
                query['execution'][field] = value
            else:
                query[field] = value
            with self.subTest(field=field), self.assertRaisesRegex(
                    publication.PublicationError, 'not authorized by the manifest'):
                self.validate(manifest, report_value)

    def test_registry_schema_and_entries_are_strict(self):
        mutations = []
        manifest, _ = manifest_with_registry()
        manifest['execution_plans']['schema'] = 'future-schema'
        mutations.append(manifest)
        manifest, _ = manifest_with_registry()
        manifest['execution_plans']['entries'] = {}
        mutations.append(manifest)
        manifest, _ = manifest_with_registry()
        manifest['execution_plans'] = None
        mutations.append(manifest)
        manifest, _ = manifest_with_registry()
        manifest['execution_plans']['extra'] = True
        mutations.append(manifest)
        manifest, _ = manifest_with_registry()
        entry = manifest['execution_plans']['entries']['memory'].pop('q1')
        manifest['execution_plans']['entries']['memory']['unknown'] = entry
        mutations.append(manifest)
        manifest, _ = manifest_with_registry()
        manifest['execution_plans']['entries']['memory']['q1']['extra'] = True
        mutations.append(manifest)
        for index, manifest in enumerate(mutations):
            with self.subTest(index=index), self.assertRaises(publication.PublicationError):
                publication.manifest_execution_plans(manifest)

    def test_coherently_rehashed_registry_still_has_to_match_canonical_query(self):
        manifest, value = optimized_report('memory')
        entry = manifest['execution_plans']['entries']['memory']['q1']
        entry['source_sha256'] = value['backends'][0]['queries'][0]['source_sha256'] = 'f' * 64
        with self.assertRaisesRegex(publication.PublicationError, 'query hashes differ'):
            publication.manifest_execution_plans(manifest)


if __name__ == '__main__':
    unittest.main()
