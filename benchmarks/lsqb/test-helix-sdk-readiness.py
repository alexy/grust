"""Readiness must prove both probes on the selected isolated SDK service."""
import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('ready', Path(__file__).with_name('check-helix-sdk-ready.py'))
ready = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ready)


class ReadinessTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.output = Path(self.temp.name) / 'ready'
        self.image = 'server@sha256:' + 'a' * 64
        self.server = dict(Id='b' * 64, Image=self.image, Config={
            'Image': self.image, 'Labels': {'io.adversarial.disposable': 'helix-sdk'}},
            State={'Running': True, 'OOMKilled': False, 'StartedAt': 'start'},
            HostConfig={'PortBindings': {}, 'Memory': ready.sdk.MEMORY,
                        'MemorySwap': ready.sdk.MEMORY, 'NanoCpus': 8_000_000_000},
            NetworkSettings={'Networks': {ready.sdk.NETWORK: {'IPAddress': '172.30.0.2'}}})

    def run_check(self, codes, current=None):
        def execute(command, **_kwargs):
            code = codes.pop(0) if command[1] == 'run' else 0
            return subprocess.CompletedProcess(command, code, '{}', '')
        with patch.object(ready.sdk.runtime, 'inspect', side_effect=[self.server, {},
                *[current or self.server for _ in codes]]), \
             patch.object(ready.subprocess, 'check_output', return_value=json.dumps([
                 {'Id': 'network', 'Internal': True}]).encode()), \
             patch.object(ready.subprocess, 'run', side_effect=execute), \
             patch.object(ready.time, 'sleep'), patch('builtins.print'):
            return ready.check('grust-lsqb-helix-sdk-test', self.image, None, self.output)

    def test_both_probes_and_fail_early_are_required(self):
        command = ready.probe_command('owned-probe', '172.30.0.2')
        self.assertIn('--fail-early', command)
        self.assertIn('http://172.30.0.2:8080/healthz', command)
        self.assertIn('http://172.30.0.2:8080/readyz', command)
        self.assertIn(ready.CURL, command)
        self.assertNotIn('--publish', command)
        self.assertTrue(self.run_check([0]))
        records = [json.loads(line) for line in (self.output / 'readiness.jsonl').read_text().splitlines()]
        self.assertTrue(records[-1]['ready'])
        self.assertFalse(records[-1]['benchmark_result'])

    def test_failed_probe_is_recorded_before_retry(self):
        self.assertTrue(self.run_check([22, 0]))
        records = [json.loads(line) for line in (self.output / 'readiness.jsonl').read_text().splitlines()]
        self.assertEqual([r['exit'] for r in records if r['event'] == 'readiness-probe'], [22, 0])

    def test_server_restart_prevents_success(self):
        changed = copy.deepcopy(self.server)
        changed['State']['StartedAt'] = 'later'
        with self.assertRaisesRegex(ValueError, 'restarted'):
            self.run_check([0], changed)


if __name__ == '__main__':
    unittest.main()
