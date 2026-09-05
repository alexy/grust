import importlib.util
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

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
        self.assertIsNone(result['plan'])
        self.assertEqual(result['plans'], [None])
        self.assertFalse(result['mixed_plans'])

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
        self.assertIsNone(result['plan'])
        self.assertEqual(result['plans'], [])

    def tagged_query(self, plan='clause-pipeline'):
        query = self.query()
        for observation in query['warmups'] + query['measurements']:
            observation['plan'] = plan
        return query

    def test_uniform_plan_is_exposed_without_changing_timings_or_input(self):
        query = self.tagged_query()
        original = copy.deepcopy(query)
        result = summary.summarize_query(query)
        self.assertEqual(result['plan'], 'clause-pipeline')
        self.assertEqual(result['plans'], ['clause-pipeline'])
        self.assertTrue(result['performance_eligible'])
        self.assertEqual(result['statistics']['elapsed_ns']['median_ns'], 20)
        self.assertEqual(result['plan_subsets'][0]['statistics'], result['statistics'])
        self.assertEqual(query, original)

    def test_mixed_warmup_plan_suppresses_pooled_and_subset_statistics(self):
        query = self.tagged_query()
        del query['warmups'][0]['plan']
        result = summary.summarize_query(query)
        self.assertEqual(result['plans'], [None, 'clause-pipeline'])
        self.assertTrue(result['mixed_plans'])
        self.assertIsNone(result['statistics'])
        self.assertIn('mixed-plans', result['ineligibility_reasons'])
        subsets = {part['plan']: part for part in result['plan_subsets']}
        self.assertEqual(subsets['clause-pipeline']['missing_warmup_count'], 1)
        self.assertEqual(subsets[None]['missing_measurement_count'], 2)
        self.assertTrue(all(part['statistics'] is None for part in subsets.values()))

    def test_mixed_measurement_plans_are_never_pooled_into_statistics(self):
        query = self.tagged_query()
        del query['measurements'][1]['plan']
        result = summary.summarize_query(query)
        self.assertIsNone(result['statistics'])
        self.assertEqual(result['measured_raw_ns']['elapsed_ns'], [10, 30])
        self.assertEqual([part['measurement_count'] for part in result['plan_subsets']], [1, 1])
        self.assertTrue(all(part['statistics'] is None for part in result['plan_subsets']))

    def test_filter_does_not_invent_corresponding_warmups(self):
        query = self.tagged_query()
        del query['warmups'][0]['plan']
        result = summary.summarize_query(query, 'clause-pipeline')
        self.assertEqual(result['plan_filter'], 'clause-pipeline')
        self.assertEqual(result['source_plans'], [None, 'clause-pipeline'])
        self.assertEqual(result['excluded_warmup_count'], 1)
        self.assertEqual(result['declared_warmup_count'], 1)
        self.assertEqual(result['warmup_count'], 0)
        self.assertEqual(result['missing_warmup_count'], 1)
        self.assertIn('incomplete-warmup-cohort', result['ineligibility_reasons'])
        self.assertIsNone(result['statistics'])

    def test_explicit_legacy_filter_never_reclassifies_absent_plans(self):
        legacy = summary.summarize_query(self.query(), 'legacy')
        self.assertIsNone(legacy['plan'])
        self.assertEqual(legacy['plan_filter'], 'legacy')
        self.assertTrue(legacy['performance_eligible'])
        absent = summary.summarize_query(self.tagged_query(), 'legacy')
        self.assertIsNone(absent['statistics'])
        self.assertEqual(absent['measurement_count'], 0)
        self.assertEqual(absent['excluded_measurement_count'], 2)
        tagged = summary.summarize_query(self.query(), 'clause-pipeline')
        self.assertEqual(tagged['measurement_count'], 0)

    def test_filtered_warmup_failure_and_original_query_failure_remain_ineligible(self):
        query = self.tagged_query()
        query['warmups'][0]['outcome'] = 'error'
        self.assertIsNone(summary.summarize_query(query, 'clause-pipeline')['statistics'])
        query = self.tagged_query()
        query['outcome'] = 'mismatch'
        self.assertIsNone(summary.summarize_query(query, 'clause-pipeline')['statistics'])

    def test_declared_missing_samples_are_reported(self):
        result = summary.summarize_query(self.query(), warmup_iterations=2, measurement_iterations=10)
        self.assertEqual(result['declared_warmup_count'], 2)
        self.assertEqual(result['missing_warmup_count'], 1)
        self.assertEqual(result['declared_measurement_count'], 10)
        self.assertEqual(result['missing_measurement_count'], 8)
        self.assertIsNone(result['statistics'])

    def test_invalid_plan_values_cannot_be_hidden_by_filtering(self):
        for value in (None, '', 'legacy', 'count-tree', 1, [], {}):
            query = self.query()
            query['measurements'][0]['plan'] = value
            with self.subTest(value=value), self.assertRaisesRegex(ValueError, 'invalid observation plan'):
                summary.summarize_query(query, 'clause-pipeline')
        with self.assertRaisesRegex(ValueError, 'invalid plan filter'):
            summary.summarize_query(self.query(), 'count-tree')

    def test_native_plan_remains_an_opaque_execution_label(self):
        query = self.tagged_query('backend-native')
        query['execution']['class'] = 'backend-native-aggregate'
        result = summary.summarize_query(query, 'backend-native')
        self.assertEqual(result['plan'], 'backend-native')
        self.assertTrue(result['performance_eligible'])

    def test_optimized_count_plans_are_distinct_filters(self):
        cases = (
            ('count-factorized', 'in-process-reference'),
            ('sql-count', 'backend-native-aggregate'),
        )
        for plan, execution_class in cases:
            query = self.tagged_query(plan)
            query['execution']['class'] = execution_class
            with self.subTest(plan=plan):
                result = summary.summarize_query(query, plan)
                self.assertEqual(result['plan'], plan)
                self.assertTrue(result['performance_eligible'])

    def test_plan_class_mismatch_cannot_be_summarized(self):
        query = self.tagged_query('sql-count')
        with self.assertRaisesRegex(ValueError, 'does not match execution class'):
            summary.summarize_query(query)

    def test_directory_summary_passes_declared_cohort_counts_and_plan_filter(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            matrix = dict(schema_version=3, environment={},
                          timing=dict(warmup_iterations=2, measurement_iterations=2),
                          backends=[dict(backend={'name': 'memory'}, lifecycle={},
                                         setup_outcome='pass', queries=[self.tagged_query()])])
            name = 'matrix-baseline-sfexample.json'
            raw = json.dumps(matrix).encode()
            (directory / name).write_bytes(raw)
            receipt = dict(suite_order=['baseline'], scale_factor='example', source_revision='a' * 40,
                           output_inventory=[dict(path=name, bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest())])
            receipt_raw = json.dumps(receipt).encode()
            (directory / 'publication-receipt.json').write_bytes(receipt_raw)
            with mock.patch.object(summary.subprocess, 'run') as verify:
                result = summary.summarize(directory, 'clause-pipeline')
            verify.assert_called_once()
            self.assertEqual(result['plan_filter'], 'clause-pipeline')
            query = result['suites'][0]['backends'][0]['queries'][0]
            self.assertEqual(query['missing_warmup_count'], 1)
            self.assertIsNone(query['statistics'])
            self.assertEqual((directory / name).read_bytes(), raw)
            self.assertEqual((directory / 'publication-receipt.json').read_bytes(), receipt_raw)

    def host_record(self):
        return dict(schema='grust-host-preflight-v1', startup_screen_passed=True,
                    clean_host_performance_eligible=False,
                    limitation='startup screen only; ongoing contention monitoring required',
                    samples=[dict(total_cpu_percent=12.5, busy_processes=[],
                                  startup_screen_passed=True,
                                  observed_at=f'2026-09-05T12:00:0{index}.000000+00:00')
                             for index in range(3)])

    def write_summary_bundle(self, directory, host_raw=None):
        matrix = dict(schema_version=3, environment={},
                      timing=dict(warmup_iterations=1, measurement_iterations=2),
                      backends=[dict(backend={'name': 'memory'}, lifecycle={},
                                     setup_outcome='pass', queries=[self.tagged_query()])])
        files = {'matrix-baseline-sfexample.json': json.dumps(matrix).encode()}
        if host_raw is not None:
            files['host-preflight.json'] = host_raw
        receipt = dict(suite_order=['baseline'], scale_factor='example', source_revision='a' * 40,
                       output_inventory=[dict(path=name, bytes=len(raw),
                                              sha256=hashlib.sha256(raw).hexdigest())
                                         for name, raw in files.items()])
        files['publication-receipt.json'] = json.dumps(receipt).encode()
        for name, raw in files.items():
            (directory / name).write_bytes(raw)
        return files

    def test_legacy_host_screen_is_unknown_not_a_failed_or_qualified_screen(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            files = self.write_summary_bundle(directory)
            with mock.patch.object(summary.subprocess, 'run'):
                result = summary.summarize(directory)
            self.assertEqual(result['host_screen'], dict(
                status='unrecorded-legacy', startup_screen_passed=None,
                evidence_path=None, evidence_sha256=None, clean_host_performance_eligible=False,
                limitation='Startup screening does not establish ongoing host isolation.'))
            query = result['suites'][0]['backends'][0]['queries'][0]
            self.assertTrue(query['performance_eligible'])
            self.assertEqual(query['statistics']['elapsed_ns']['median_ns'], 20)
            self.assertTrue(any('not clean-host performance qualification' in note
                                for note in result['notes']))
            self.assertEqual({name: (directory / name).read_bytes() for name in files}, files)

    def test_recorded_host_screen_is_bound_but_never_ongoing_qualification(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            host_raw = json.dumps(self.host_record()).encode()
            files = self.write_summary_bundle(directory, host_raw)
            with mock.patch.object(summary.subprocess, 'run'):
                result = summary.summarize(directory)
            self.assertEqual(result['host_screen'], dict(
                status='recorded-startup-only', startup_screen_passed=True,
                evidence_path='host-preflight.json',
                evidence_sha256=hashlib.sha256(host_raw).hexdigest(),
                clean_host_performance_eligible=False,
                limitation='Startup screening does not establish ongoing host isolation.'))
            query = result['suites'][0]['backends'][0]['queries'][0]
            self.assertTrue(query['performance_eligible'])
            self.assertEqual(query['statistics']['elapsed_ns']['median_ns'], 20)
            self.assertEqual({name: (directory / name).read_bytes() for name in files}, files)

    def test_host_screen_tampering_after_receipt_verification_is_rejected(self):
        original = json.dumps(self.host_record()).encode()
        replacements = (original.replace(b'12.5', b'13.5'), original + b'\n')
        for replacement in replacements:
            with self.subTest(length=len(replacement)), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                self.write_summary_bundle(directory, original)

                def mutate(*args, **kwargs):
                    (directory / 'host-preflight.json').write_bytes(replacement)

                with mock.patch.object(summary.subprocess, 'run', side_effect=mutate):
                    with self.assertRaisesRegex(ValueError, 'host preflight changed after receipt verification'):
                        summary.summarize(directory)

    def test_bound_host_screen_is_revalidated_even_after_receipt_verification(self):
        failed = self.host_record()
        failed['startup_screen_passed'] = False
        for raw in (b'{', json.dumps(failed).encode()):
            with self.subTest(raw=raw), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                self.write_summary_bundle(directory, raw)
                with mock.patch.object(summary.subprocess, 'run'):
                    with self.assertRaises(ValueError):
                        summary.summarize(directory)

    def test_cli_documents_plan_choices_and_rejects_unknown_selection(self):
        script = str(Path(__file__).with_name('summarize-performance.py'))
        help_result = subprocess.run([sys.executable, script, '--help'], capture_output=True, text=True)
        self.assertEqual(help_result.returncode, 0)
        for plan in summary.PLAN_FILTERS:
            self.assertIn(plan, help_result.stdout)
        invalid = subprocess.run([sys.executable, script, 'unused', '--plan', 'count-tree'],
                                 capture_output=True, text=True)
        self.assertEqual(invalid.returncode, 2)
        self.assertIn('invalid choice', invalid.stderr)


if __name__ == '__main__':
    unittest.main()
