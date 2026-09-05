#!/usr/bin/env python3
"""Offline schema-v3 observation-plan contract tests."""

import copy
import importlib.util
from pathlib import Path
import unittest


SPEC = importlib.util.spec_from_file_location(
    'publication', Path(__file__).with_name('validate-matrix-publication.py'))
publication = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(publication)


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
        for plan in ('backend-native', 'sql-row-source'):
            value = report()
            value['backends'][0]['queries'][0]['measurements'][0]['plan'] = plan
            with self.subTest(plan=plan), self.assertRaisesRegex(
                    publication.PublicationError, 'does not match execution class'):
                self.validate(value)

    def test_legacy_and_tagged_samples_remain_valid_evidence(self):
        value = report()
        query = value['backends'][0]['queries'][0]
        query['measurements'][0]['plan'] = 'clause-pipeline'
        self.validate(value)
        self.assertNotIn('plan', query['warmups'][0])


if __name__ == '__main__':
    unittest.main()
