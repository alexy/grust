"""Mutation checks against retained SDK Docker evidence."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('audit', Path(__file__).with_name('validate-sdk.py'))
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)
FIXTURE = os.environ.get('GRUST_SDK_AUDIT_FIXTURE')


@unittest.skipUnless(FIXTURE, 'set GRUST_SDK_AUDIT_FIXTURE to retained Docker evidence')
class AuditTests(unittest.TestCase):
    def test_real_docker_cohort(self):
        result = audit.validate(Path(FIXTURE))
        self.assertTrue(result['diagnostic_verified'])
        self.assertFalse(result['publication_qualified'])
        self.assertIn(result['observations'], (108, 156))

    def test_rejects_mutated_records(self):
        mutations = [
            ('component.json', lambda r: r['environment'].update(grust_revision='0' * 40)),
            ('component.json', lambda r: r['backends'][0]['queries'][0]['measurements'][0].update(actual_count=999)),
            ('component.json', lambda r: r['backends'][0]['queries'][0]['measurements'][0].update(query_position=99)),
            ('component.json', lambda r: r['backends'][0]['queries'][0]['measurements'].pop()),
            ('component.json', lambda r: r['backends'][0]['queries'][0]['measurements'][0].update(termination='deadline-sigkill')),
            ('component.json', lambda r: r['backends'][0]['queries'][0]['execution'].update(transport='HTTP')),
            ('component.json', lambda r: r['backends'][0]['backend'].update(name='surreal')),
            ('client-after.json', lambda r: r['resources'].update(Memory=1)),
            ('server-after.json', lambda r: r['state'].update(StartedAt='later')),
            ('server-after.json', lambda r: r['state'].update(OOMKilled=True)),
            ('server-before.json', lambda r: r['labels'].update({'io.adversarial.disposable': 'surreal'})),
            ('server-stopped.json', lambda r: r['state'].update(Running=True)),
            ('watchdog.json', lambda r: r.update(status='timeout')),
            ('invocation.json', lambda r: r.update(client_image_id='sha256:' + '0' * 64)),
        ]
        for index, (name, mutate) in enumerate(mutations):
            with self.subTest(index=index, file=name), tempfile.TemporaryDirectory() as temporary:
                target = Path(temporary) / 'evidence'
                shutil.copytree(FIXTURE, target)
                value = json.loads((target / name).read_text())
                mutate(value)
                (target / name).write_text(json.dumps(value))
                with self.assertRaises((audit.matrix.PublicationError, KeyError, ValueError)):
                    audit.validate(target)

    def test_missing_journal_observation_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / 'evidence'
            shutil.copytree(FIXTURE, target)
            lines = (target / 'cell.log').read_text().splitlines()
            for index, line in enumerate(lines):
                if line.startswith('{') and json.loads(line).get('event') == 'observation-recorded':
                    del lines[index]
                    break
            (target / 'cell.log').write_text('\n'.join(lines))
            with self.assertRaises(audit.matrix.PublicationError):
                audit.validate(target)


if __name__ == '__main__':
    unittest.main()
