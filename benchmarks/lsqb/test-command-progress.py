"""Live small-process checks for build/test progress recording."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('progress', Path(__file__).with_name('command-progress.py'))
progress = importlib.util.module_from_spec(spec)
spec.loader.exec_module(progress)


class ProgressTests(unittest.TestCase):
    def test_retains_heartbeat_output_and_terminal_status(self):
        with tempfile.TemporaryDirectory() as root, patch('builtins.print'):
            output = Path(root) / 'run'
            status = progress.run([sys.executable, '-u', '-c',
                                   'import time; print("compiling adapter"); time.sleep(.15)'], output, .02)
            records = [json.loads(line) for line in (output / 'progress.jsonl').read_text().splitlines()]
            self.assertEqual(status, 0)
            self.assertEqual(records[0]['event'], 'command-start')
            self.assertEqual(records[-1]['event'], 'command-finish')
            self.assertEqual(records[-1]['exit'], 0)
            self.assertEqual(records[-1]['latest_output'], 'compiling adapter')
            self.assertTrue(any(r['event'] == 'command-progress' for r in records))
            self.assertEqual((output / 'command.log').read_text(), 'compiling adapter\n')
            with self.assertRaises(FileExistsError):
                progress.run([sys.executable, '-c', 'pass'], output)

    def test_nonzero_exit_is_not_reported_as_success(self):
        with tempfile.TemporaryDirectory() as root, patch('builtins.print'):
            output = Path(root) / 'run'
            self.assertEqual(progress.run([sys.executable, '-c', 'raise SystemExit(7)'], output), 7)
            final = json.loads((output / 'progress.jsonl').read_text().splitlines()[-1])
            self.assertEqual(final['exit'], 7)


if __name__ == '__main__':
    unittest.main()
