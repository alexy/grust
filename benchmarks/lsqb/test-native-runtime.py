"""Runtime evidence excludes connection credentials and pins resource metadata."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
import subprocess
import tempfile
import json

spec = importlib.util.spec_from_file_location('native_runtime', Path(__file__).with_name('run-native-neo4j.py'))
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


class SnapshotTests(unittest.TestCase):
    def test_exit_snapshot_is_written_before_wrapper_returns(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(runtime.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)), \
                    patch.object(runtime, 'inspect', return_value={}), \
                    patch.object(runtime, 'snapshot', return_value={'container_id': 'a' * 64}):
                self.assertEqual(runtime.start_recorded('a' * 64, directory), 0)
            self.assertEqual(json.loads((Path(directory) / 'client-after.json').read_text()),
                             {'container_id': 'a' * 64})

    def test_server_stop_is_attempted_even_when_client_stop_fails(self):
        with patch.object(runtime.subprocess, 'run', side_effect=[subprocess.TimeoutExpired('docker', 30), None]) as run:
            with self.assertRaises(subprocess.TimeoutExpired):
                runtime.stop_owned('client-id', 'server-id')
            self.assertEqual([call.args[0] for call in run.call_args_list],
                             [['docker', 'stop', 'client-id'], ['docker', 'stop', 'server-id']])

    def test_allowlist_preserves_runtime_identity_without_environment(self):
        raw = dict(Id='a' * 64, Image='sha256:' + 'b' * 64, Name='/owned',
                   State=dict(Status='exited', Running=False, OOMKilled=False, ExitCode=0,
                              StartedAt='start', FinishedAt='end', Error='sensitive transport detail'),
                   Config=dict(Env=['NEO4J_PASSWORD=do-not-export'], Labels={'owner': 'benchmark'}),
                   HostConfig=dict(Memory=runtime.MEMORY, MemorySwap=runtime.MEMORY,
                                   NanoCpus=runtime.NANO_CPUS, CpusetCpus='', ReadonlyRootfs=True,
                                   NetworkMode='private', ArbitrarySecret='do-not-export'))
        result = runtime.snapshot(raw)
        self.assertEqual(result['container_id'], raw['Id'])
        self.assertEqual(result['image_id'], raw['Image'])
        self.assertEqual(result['resources']['Memory'], runtime.MEMORY)
        self.assertFalse(result['state']['OOMKilled'])
        self.assertNotIn('do-not-export', str(result))
        self.assertNotIn('sensitive transport detail', str(result))


if __name__ == '__main__':
    unittest.main()
