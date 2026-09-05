#!/usr/bin/env python3
"""Fail-closed input tests for the source-built Sail runner."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('sail_source', Path(__file__).with_name('run-sail-source.py'))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)
audit_spec = importlib.util.spec_from_file_location('sail_audit', Path(__file__).with_name('validate-sail-source.py'))
audit = importlib.util.module_from_spec(audit_spec)
audit_spec.loader.exec_module(audit)


class ServerContract(unittest.TestCase):
    def setUp(self):
        self.build = {'image_id': 'sha256:' + 'a' * 64}
        self.server = dict(Image=self.build['image_id'], State=dict(Running=True, OOMKilled=False),
                           HostConfig=dict(PortBindings={}, Memory=runner.MEMORY,
                                           MemorySwap=runner.MEMORY, NanoCpus=8_000_000_000),
                           NetworkSettings=dict(Networks={runner.NETWORK: dict(IPAddress='172.30.0.2')}),
                           Config=dict(Labels={'io.adversarial.disposable': 'sail-source'}))

    def test_owned_isolated_server(self):
        self.assertEqual(runner.validate_server(self.server, self.build), '172.30.0.2')

    def test_rejects_resource_identity_and_ownership_drift(self):
        mutations = [
            ('Image', 'wrong'),
            ('State', dict(Running=False, OOMKilled=False)),
            ('State', dict(Running=True, OOMKilled=True)),
            ('Config', dict(Labels={})),
            ('NetworkSettings', dict(Networks={})),
            ('NetworkSettings', dict(Networks={runner.NETWORK: dict(IPAddress='172.30.0.2'), 'bridge': {}})),
        ]
        for field, value in mutations:
            with self.subTest(field=field, value=value):
                candidate = copy.deepcopy(self.server)
                candidate[field] = value
                with self.assertRaises(ValueError):
                    runner.validate_server(candidate, self.build)
        for field, value in [('Memory', 0), ('MemorySwap', 0), ('NanoCpus', 0), ('PortBindings', {'50051/tcp': [{}]})]:
            with self.subTest(field=field):
                candidate = copy.deepcopy(self.server)
                candidate['HostConfig'][field] = value
                with self.assertRaises(ValueError):
                    runner.validate_server(candidate, self.build)


@unittest.skipUnless(os.environ.get('GRUST_SAIL_AUDIT_FIXTURE'), 'set a retained example evidence directory')
class RetainedEvidenceMutations(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='grust-sail-audit-test-')
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name) / 'evidence'
        shutil.copytree(os.environ['GRUST_SAIL_AUDIT_FIXTURE'], self.directory)

    def mutate(self, name, change):
        path = self.directory / name
        value = json.loads(path.read_text())
        change(value)
        path.write_text(json.dumps(value))
        with self.assertRaises(audit.matrix.PublicationError):
            audit.validate(self.directory)

    def test_valid_recorded_evidence(self):
        result = audit.validate(self.directory)
        self.assertEqual(result['observations'], result['passed'])
        self.assertFalse(result['publication_qualified'])

    def test_count_tampering(self):
        self.mutate('component.json', lambda x: x['backends'][0]['queries'][0]['measurements'][0].update(actual_count=999))

    def test_missing_sample(self):
        self.mutate('component.json', lambda x: x['backends'][0]['queries'][0]['measurements'].pop())

    def test_dataset_tampering(self):
        self.mutate('component.json', lambda x: x['dataset'].update(nodes=0))

    def test_runtime_oom(self):
        self.mutate('server-after.json', lambda x: x['state'].update(OOMKilled=True))

    def test_incomplete_watchdog(self):
        self.mutate('watchdog.json', lambda x: x.update(status='timeout'))

    def test_missing_journal(self):
        (self.directory / 'cell.log').write_text('')
        with self.assertRaises(audit.matrix.PublicationError):
            audit.validate(self.directory)


if __name__ == '__main__':
    unittest.main()
