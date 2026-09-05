import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('summary', Path(__file__).with_name('summarize-performance.py'))
summary = importlib.util.module_from_spec(spec)
spec.loader.exec_module(summary)


class PerformanceTests(unittest.TestCase):
    def query(self):
        return dict(id='q1', outcome='pass', execution={'class': 'in-process-reference'},
                    warmups=[dict(outcome='pass', elapsed_ns=999, setup_ns=999, recovery_ns=999)],
                    measurements=[dict(outcome='pass', elapsed_ns=10, setup_ns=30, recovery_ns=5),
                                  dict(outcome='pass', elapsed_ns=30, setup_ns=10, recovery_ns=5)])

    def test_measured_only_and_per_sample_boundary_total(self):
        result = summary.summarize_query(self.query())
        self.assertEqual(result['statistics']['elapsed_ns']['median_ns'], 20)
        self.assertEqual(result['measured_raw_ns']['sample_boundary_total_ns'], [45, 45])
        self.assertEqual(result['measurement_count'], 2)

    def test_warmup_failure_suppresses_statistics(self):
        query = self.query()
        query['warmups'][0]['outcome'] = 'error'
        result = summary.summarize_query(query)
        self.assertFalse(result['performance_eligible'])
        self.assertIsNone(result['statistics'])
        self.assertEqual(result['warmup_outcomes']['error'], 1)

    def test_timeout_is_retained_not_a_successful_latency(self):
        query = self.query()
        query['outcome'] = query['measurements'][1]['outcome'] = 'timeout'
        result = summary.summarize_query(query)
        self.assertIsNone(result['statistics'])
        self.assertEqual(result['measurement_outcomes']['timeout'], 1)
        self.assertEqual(len(result['measured_raw_ns']['elapsed_ns']), 2)

    def test_unsupported_has_no_fabricated_timings(self):
        query = self.query()
        query.update(outcome='unsupported', warmups=[], measurements=[])
        result = summary.summarize_query(query)
        self.assertIsNone(result['statistics'])
        self.assertEqual(result['measurement_count'], 0)


if __name__ == '__main__':
    unittest.main()
