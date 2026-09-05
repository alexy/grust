import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('bundle', Path(__file__).with_name('bundle-native-neo4j.py'))
bundle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bundle)


class BundleTests(unittest.TestCase):
    def test_missing_and_symlink_inputs_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaises(ValueError):
                bundle.payloads(root)
            (root / 'target').write_text('{}')
            (root / bundle.FILES[0]).symlink_to(root / 'target')
            with self.assertRaises(ValueError):
                bundle.payloads(root)

    def test_export_hashes_exact_validated_snapshot_and_never_overwrites(self):
        captured = {name: b'{}' for name in bundle.FILES}
        captured['invocation.json'] = b'{"source_revision":"fixture"}'
        captured['result/diagnostic.json'] = b'{"scale":"example"}'
        with tempfile.TemporaryDirectory() as temporary, \
                patch.object(bundle, 'payloads', return_value=captured), \
                patch.object(bundle.audit, 'validate', return_value={'publication_qualified': False}) as validate, \
                patch.object(bundle.audit, 'validate_runtime', return_value=True), \
                patch.object(bundle.audit, 'require_matched_sampling'), \
                patch.object(bundle.audit, 'summarize_measurements', return_value=[]):
            output = Path(temporary) / 'bundle'
            manifest = bundle.export(Path(temporary), output)
            self.assertNotEqual(validate.call_args.args[0], Path(temporary))
            self.assertFalse(manifest['publication_qualified'])
            self.assertEqual(len(manifest['files']), len(bundle.FILES) + 1)
            for entry in manifest['files']:
                data = (output / entry['path']).read_bytes()
                self.assertEqual(len(data), entry['bytes'])
                self.assertEqual(hashlib.sha256(data).hexdigest(), entry['sha256'])
            self.assertEqual(json.loads((output / 'bundle.json').read_text()), manifest)
            with self.assertRaises(FileExistsError):
                bundle.export(Path(temporary), output)

    def test_failed_audit_does_not_create_output(self):
        with tempfile.TemporaryDirectory() as temporary, \
                patch.object(bundle, 'payloads', return_value={}), \
                patch.object(bundle.audit, 'validate', side_effect=ValueError('incomplete')):
            output = Path(temporary) / 'bundle'
            with self.assertRaises(ValueError):
                bundle.export(Path(temporary), output)
            self.assertFalse(output.exists())


if __name__ == '__main__':
    unittest.main()
