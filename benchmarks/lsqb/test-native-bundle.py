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
    def test_internal_network_provenance_rejects_mutations(self):
        record = dict(Name='grust-lsqb-neo4j-qualification', Id='a' * 64, Internal=True)
        bundle.audit.validate_network_record(record)
        for changed in ({**record, 'Internal': False}, {**record, 'Internal': 1},
                        {**record, 'Name': 'foreign'}, {**record, 'Id': 'invalid'},
                        {**record, 'extra': True}):
            with self.subTest(record=changed), self.assertRaises(ValueError):
                bundle.audit.validate_network_record(changed)

    def test_network_payload_is_captured_and_symlink_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in bundle.FILES:
                target = root / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text('{}')
            network = root / 'network-before.json'
            network.write_text('{"Internal":true}')
            self.assertEqual(bundle.payloads(root)['network-before.json'], network.read_bytes())
            network.unlink()
            network.symlink_to(root / 'invocation.json')
            with self.assertRaises(ValueError):
                bundle.payloads(root)

    def test_missing_and_symlink_inputs_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaises(ValueError):
                bundle.payloads(root)
            (root / 'target').write_text('{}')
            (root / bundle.FILES[0]).symlink_to(root / 'target')
            with self.assertRaises(ValueError):
                bundle.payloads(root)

    def test_sf03_requires_network_provenance_before_export(self):
        for network in (None, {'Name': 'grust-lsqb-neo4j-qualification',
                               'Id': 'a' * 64, 'Internal': False},
                        {'Name': 'grust-lsqb-neo4j-qualification',
                         'Id': 'a' * 64, 'Internal': True}):
            captured = {name: b'{}' for name in bundle.FILES}
            captured['invocation.json'] = b'{"source_revision":"fixture"}'
            captured['result/diagnostic.json'] = b'{"scale":"0.3"}'
            if network is not None:
                captured['network-before.json'] = json.dumps(network).encode()
            with self.subTest(network=network), tempfile.TemporaryDirectory() as temporary, \
                    patch.object(bundle, 'payloads', return_value=captured), \
                    patch.object(bundle.audit, 'validate', return_value={'publication_qualified': False}), \
                    patch.object(bundle.audit, 'validate_runtime', return_value=True), \
                    patch.object(bundle.audit, 'require_matched_sampling'), \
                    patch.object(bundle.audit, 'summarize_measurements', return_value=[]):
                output = Path(temporary) / 'bundle'
                if network is None or network['Internal'] is False:
                    with self.assertRaises(ValueError):
                        bundle.export(Path(temporary), output)
                    self.assertFalse(output.exists())
                else:
                    manifest = bundle.export(Path(temporary), output)
                    self.assertEqual(len(manifest['files']), len(bundle.FILES) + 2)
                    checked = json.loads((output / 'audit.json').read_text())
                    self.assertTrue(checked['internal_network_verified'])

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
