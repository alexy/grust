"""Deterministic tests for the pre-measurement Bolt readiness gate."""
import importlib.util
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    'native_runner', Path(__file__).with_name('run-native-neo4j.py'))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class ReadinessTests(unittest.TestCase):
    def test_ready_requires_success_and_exact_scalar(self):
        results = [subprocess.CompletedProcess([], 1, 'readiness\n42\n'),
                   subprocess.CompletedProcess([], 0, 'readiness\n7\n'),
                   subprocess.CompletedProcess([], 0, 'readiness\n42\n')]
        with patch.object(runner.subprocess, 'run', side_effect=results) as run, \
                patch.object(runner.time, 'sleep'), patch('builtins.print'):
            record = runner.wait_ready('a' * 64)
        self.assertTrue(record['ready'])
        self.assertEqual(record['attempt'], 3)
        self.assertEqual(run.call_args.args[0][-1], 'RETURN 42 AS readiness')
        self.assertLessEqual(run.call_args.kwargs['timeout'], 10)

    def test_transient_timeout_is_retried(self):
        with patch.object(runner.subprocess, 'run', side_effect=[
                subprocess.TimeoutExpired('probe', 10),
                subprocess.CompletedProcess([], 0, 'readiness\n42\n')]), \
                patch.object(runner.time, 'sleep'), patch('builtins.print'):
            self.assertEqual(runner.wait_ready('a' * 64)['attempt'], 2)

    def test_deadline_does_not_start_an_unbounded_probe(self):
        with patch.object(runner.time, 'monotonic', side_effect=[0, 121]), \
                patch.object(runner.subprocess, 'run') as run:
            with self.assertRaisesRegex(RuntimeError, 'readiness deadline'):
                runner.wait_ready('a' * 64)
        run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
