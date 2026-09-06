#!/usr/bin/env python3
"""Declared cell terminations: recorded, bounded, and never inferred."""

import copy
import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location(
    'publication', Path(__file__).with_name('validate-matrix-publication.py'))
publication = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(publication)
PLAN_SPEC = importlib.util.spec_from_file_location(
    'plan_tests', Path(__file__).with_name('test-observation-plan.py'))
plan_tests = importlib.util.module_from_spec(PLAN_SPEC)
PLAN_SPEC.loader.exec_module(plan_tests)

DETAIL = 'backend falkor could not prove quiescence after an unacknowledged query exit'


def terminated_report():
    value = plan_tests.report()
    cell = value['backends'][0]
    cell['backend']['name'] = 'falkor'
    cell['queries'][0]['execution'] = {'class': 'backend-native-aggregate'}
    cell['lifecycle'] = dict(load_strategy='once-worker-attach', recovery_contract='falkor-server-deadline',
                             terminated=dict(query_id='q1', phase='warmup', iteration=1,
                                             reason_code='backend.quiescence-unproven', detail=DETAIL))
    query = cell['queries'][0]
    query.update(id='q1', outcome='error', reason_code='backend.quiescence-unproven', detail=DETAIL)
    query['warmups'][0].update(outcome='error', detail=DETAIL, termination='deadline-sigterm', elapsed_ns=100_000_000)
    query['measurements'] = []
    return value


class CellTerminationTests(unittest.TestCase):
    def check(self, value):
        publication.validate_v3_timeout_contract(value, 'fixture.json')

    def test_declared_termination_is_accepted(self):
        self.check(terminated_report())

    def test_mutations_fail_closed(self):
        def mutate(path, apply):
            value = terminated_report()
            apply(value)
            with self.subTest(path=path), self.assertRaises(publication.PublicationError):
                self.check(value)
        mutate('undeclared', lambda v: v['backends'][0]['lifecycle'].pop('terminated'))
        mutate('unknown query', lambda v: v['backends'][0]['lifecycle']['terminated'].update(query_id='q9'))
        mutate('wrong reason', lambda v: v['backends'][0]['lifecycle']['terminated'].update(reason_code='query.execution'))
        mutate('wrong phase', lambda v: v['backends'][0]['lifecycle']['terminated'].update(phase='cleanup'))
        mutate('detail differs', lambda v: v['backends'][0]['queries'][0]['warmups'][0].update(detail='other'))
        mutate('terminal observation passed', lambda v: v['backends'][0]['queries'][0]['warmups'][0].update(outcome='pass', actual_count=1))
        mutate('query relabeled pass', lambda v: v['backends'][0]['queries'][0].update(outcome='pass'))
        mutate('extra field', lambda v: v['backends'][0]['lifecycle']['terminated'].update(extra=True))
        mutate('observation after termination', lambda v: v['backends'][0]['queries'][0]['measurements'].append(dict(v['backends'][0]['queries'][0]['warmups'][0], outcome='pass', actual_count=1, termination='normal-exit', elapsed_ns=1)))
        mutate('empty detail', lambda v: v['backends'][0]['lifecycle']['terminated'].update(detail=''))


if __name__ == '__main__':
    unittest.main()
