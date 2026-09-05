"""Mock Docker build receipts while checking exact source/image admission."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('builder', Path(__file__).with_name('build-sdk.py'))
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


class BuildTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.checkout = Path(self.temp.name) / 'checkout'
        recipe = self.checkout / 'benchmarks/lsqb/Dockerfile'
        recipe.parent.mkdir(parents=True)
        recipe.write_text('FROM pinned-test-input\n')
        self.output = Path(self.temp.name) / 'output'
        self.revision = 'a' * 40
        self.image_id = 'sha256:' + 'b' * 64
        self.image = dict(Id=self.image_id, Architecture='arm64', Os='linux', Config={'Labels': {
            'org.opencontainers.image.revision': self.revision,
            'io.adversarial.grust.benchmark-feature': 'surreal'}})

    def fake_build(self, command, directory):
        self.assertIn('BENCHMARK_FEATURE=surreal', command)
        directory.mkdir()
        (directory / 'command.log').write_text('verified build output\n')
        (self.output / 'image-id').write_text(self.image_id)
        return 0

    def execute(self, sources=None):
        with patch.object(builder, 'source', side_effect=sources or [self.revision, self.revision]), \
                patch.object(builder.progress, 'run', side_effect=self.fake_build), \
                patch.object(builder.subprocess, 'check_output', return_value=json.dumps([self.image]).encode()), \
                patch('builtins.print'):
            builder.build(self.checkout, 'surreal-sdk', self.output)

    def test_receipt_pins_source_platform_recipe_and_build_output(self):
        self.execute()
        receipt = json.loads((self.output / 'build-receipt.json').read_text())
        self.assertEqual(receipt['source_revision'], self.revision)
        self.assertEqual(receipt['image_id'], self.image_id)
        self.assertFalse(receipt['publication_qualified'])
        self.assertEqual(receipt['build_log_sha256'], hashlib.sha256(b'verified build output\n').hexdigest())
        self.assertEqual(receipt['recipe_sha256'], hashlib.sha256((self.output / 'Dockerfile').read_bytes()).hexdigest())

    def test_source_change_prevents_receipt(self):
        with self.assertRaisesRegex(ValueError, 'source changed'):
            self.execute([self.revision, 'c' * 40])
        self.assertFalse((self.output / 'build-receipt.json').exists())

    def test_wrong_feature_prevents_receipt(self):
        self.image['Config']['Labels']['io.adversarial.grust.benchmark-feature'] = 'helix'
        with self.assertRaisesRegex(ValueError, 'source/feature'):
            self.execute()
        self.assertFalse((self.output / 'build-receipt.json').exists())

    def test_dirty_source_is_refused(self):
        with patch.object(builder.subprocess, 'check_output', side_effect=[self.revision, ' M file\n']):
            with self.assertRaisesRegex(ValueError, 'clean source'):
                builder.source(self.checkout)


if __name__ == '__main__':
    unittest.main()
