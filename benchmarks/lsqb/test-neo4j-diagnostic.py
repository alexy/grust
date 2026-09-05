"""Mutation coverage for native diagnostic observation admission."""
import copy
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('native_check', Path(__file__).with_name('validate-neo4j-diagnostic.py'))
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)


class ObservationTests(unittest.TestCase):
    def setUp(self):
        self.reference = ('baseline', 'q1', 8, 'a' * 64)
        self.item = dict(event='observation-recorded', complete=False, suite='baseline', id='q1',
                         expected_count=8, actual_count=8, outcome='pass', termination='normal-exit',
                         source_sha256='a' * 64, query_sha256='a' * 64, setup_ns=1, elapsed_ns=2,
                         process_recovery_ns=1, query_timeout_ms=60000,
                         timing_boundary='coordinator-go-through-scalar-consumption-and-rollback-result',
                         server_recovery=dict(owned_transactions_remaining=0, subsequent_scalar=42,
                                              server_recovery_ns=1, transaction_tag='neo4j-7-123',
                                              targeted_termination_count=0, terminated_transaction_ids=[]))

    def test_pass_and_honest_mismatch(self):
        check.check_observation(self.item, self.reference, set())
        self.item.update(outcome='mismatch', actual_count=7)
        check.check_observation(self.item, self.reference, set())

    def test_timeout_requires_deadline_and_process_termination(self):
        self.item.update(outcome='timeout', actual_count=None, termination='deadline-sigterm', elapsed_ns=60_000_000_000)
        check.check_observation(self.item, self.reference, set())
        for key, value in [('elapsed_ns', 1), ('termination', 'normal-exit'), ('actual_count', 8)]:
            broken = copy.deepcopy(self.item)
            broken[key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                check.check_observation(broken, self.reference, set())

    def test_reject_count_source_timing_and_identity_mutations(self):
        for key, value in [('actual_count', 7), ('expected_count', 7), ('id', 'q2'),
                           ('source_sha256', 'b' * 64), ('complete', True), ('setup_ns', True),
                           ('query_timeout_ms', 1), ('outcome', 'unsupported'), ('elapsed_ns', 60_000_000_001)]:
            broken = copy.deepcopy(self.item)
            broken[key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                check.check_observation(broken, self.reference, set())

    def test_recovery_is_mandatory(self):
        for key, value in [('owned_transactions_remaining', 1), ('subsequent_scalar', 0),
                           ('targeted_termination_count', 1), ('transaction_tag', 'unknown'),
                           ('server_recovery_ns', -1), ('terminated_transaction_ids', ['a', 'b'])]:
            broken = copy.deepcopy(self.item)
            broken['server_recovery'][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                check.check_observation(broken, self.reference, set())
        with self.assertRaises(ValueError):
            check.check_observation(self.item, self.reference, {'neo4j-7-123'})


if __name__ == '__main__':
    unittest.main()
