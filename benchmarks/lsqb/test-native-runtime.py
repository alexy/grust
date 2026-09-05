"""Runtime evidence excludes connection credentials and pins resource metadata."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('native_runtime', Path(__file__).with_name('run-native-neo4j.py'))
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


class SnapshotTests(unittest.TestCase):
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
