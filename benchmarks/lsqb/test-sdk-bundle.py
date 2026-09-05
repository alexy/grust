"""Frozen SDK export integrity tests against retained runtime evidence."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('bundle', Path(__file__).with_name('bundle-sdk.py'))
bundle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bundle)
FIXTURE = os.environ.get('GRUST_SDK_AUDIT_FIXTURE')
BUILD = os.environ.get('GRUST_SDK_BUILD_FIXTURE')


@unittest.skipUnless(FIXTURE and BUILD, 'set SDK runtime and build fixture paths')
class BundleTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='grust-sdk-bundle-test-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.output = self.root / 'bundle'

    def test_export_hashes_and_structured_journal(self):
        manifest = bundle.export(Path(FIXTURE), self.output, Path(BUILD))
        self.assertFalse(manifest['publication_qualified'])
        self.assertEqual(len(manifest['files']), 14)
        for entry in manifest['files']:
            data = (self.output / entry['path']).read_bytes()
            self.assertEqual(len(data), entry['bytes'])
            self.assertEqual(hashlib.sha256(data).hexdigest(), entry['sha256'])
        self.assertFalse((self.output / 'cell.log').exists())
        events = (self.output / 'observations.jsonl').read_text().splitlines()
        checked = json.loads((self.output / 'audit.json').read_text())
        self.assertEqual(len(events), checked['observations'])
        with self.assertRaises(FileExistsError):
            bundle.export(Path(FIXTURE), self.output, Path(BUILD))

    def test_build_mutations_rejected_before_output(self):
        for relative in ('Dockerfile', 'build/command.log', 'build-receipt.json'):
            with self.subTest(file=relative):
                build = self.root / relative.replace('/', '-')
                shutil.copytree(BUILD, build)
                target = build / relative
                if relative.endswith('.json'):
                    value = json.loads(target.read_text())
                    value['source_revision'] = 'f' * 40
                    target.write_text(json.dumps(value))
                else:
                    target.write_bytes(target.read_bytes() + b' altered')
                with self.assertRaises(bundle.audit.matrix.PublicationError):
                    bundle.export(Path(FIXTURE), self.output, build)
                self.assertFalse(self.output.exists())


if __name__ == '__main__':
    unittest.main()
