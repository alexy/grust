"""Exercise Helix export against retained real runtime and build evidence."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('bundle', ROOT / 'bundle-helix-sdk.py')
bundle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bundle)
FIXTURE = os.environ.get('GRUST_HELIX_SDK_AUDIT_FIXTURE')
CLIENT = os.environ.get('GRUST_HELIX_SDK_BUILD_FIXTURE')
SERVER = os.environ.get('GRUST_HELIX_SERVER_BUILD_FIXTURE')


@unittest.skipUnless(FIXTURE and CLIENT and SERVER, 'set Helix runtime/client/server fixtures')
class BundleTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='grust-helix-bundle-test-')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.output = self.root / 'bundle'

    def export(self, server=None, recipe=None):
        return bundle.export(Path(FIXTURE), self.output, Path(CLIENT),
                             server or Path(SERVER), recipe or ROOT / 'Dockerfile.helix-sdk-server')

    def test_inventory_and_collision(self):
        manifest = self.export()
        self.assertEqual(manifest['track'], 'helix-sdk')
        self.assertFalse(manifest['publication_qualified'])
        self.assertEqual(len(manifest['files']), 17)
        for entry in manifest['files']:
            data = (self.output / entry['path']).read_bytes()
            self.assertEqual(len(data), entry['bytes'])
            self.assertEqual(hashlib.sha256(data).hexdigest(), entry['sha256'])
        checked = json.loads((self.output / 'audit.json').read_text())
        self.assertEqual(len((self.output / 'observations.jsonl').read_text().splitlines()),
                         checked['observations'])
        with self.assertRaises(FileExistsError):
            self.export()

    def test_server_receipt_mutations(self):
        for field, value in [('source_revision', 'f' * 40), ('build_exit', 1),
                             ('build_exit', False), ('source_clean_before_and_after', False),
                             ('image_id', 'sha256:' + 'f' * 64), ('sdk_version_target', '2.0.0'),
                             ('publication_qualified', True), ('platform', 'linux/amd64')]:
            with self.subTest(field=field, value=value):
                server = self.root / (field + str(value).replace('/', '-'))
                shutil.copytree(SERVER, server)
                receipt = server / 'build-receipt.json'
                data = json.loads(receipt.read_text())
                data[field] = value
                receipt.write_text(json.dumps(data))
                with self.assertRaises(bundle.audit.matrix.PublicationError):
                    self.export(server=server)
                self.assertFalse(self.output.exists())

    def test_recipe_and_log_mutations(self):
        recipe = self.root / 'Dockerfile'
        recipe.write_bytes((ROOT / 'Dockerfile.helix-sdk-server').read_bytes() + b' altered')
        with self.assertRaises(bundle.audit.matrix.PublicationError):
            self.export(recipe=recipe)
        server = self.root / 'server'
        shutil.copytree(SERVER, server)
        log = server / 'command.log'
        log.write_bytes(log.read_bytes() + b' altered')
        with self.assertRaises(bundle.audit.matrix.PublicationError):
            self.export(server=server)
        self.assertFalse(self.output.exists())


if __name__ == '__main__':
    unittest.main()
