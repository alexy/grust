import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('progress', Path(__file__).with_name('native-progress.py'))
progress = importlib.util.module_from_spec(spec)
spec.loader.exec_module(progress)


class ProgressTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        (self.root / 'result').mkdir()
        (self.root / 'invocation.json').write_text(json.dumps({'command': [
            'docker', 'image', 'qualify', '/lsqb', '/attacks', 'example', '/out/result', '2', '10']}))

    def test_missing_journal_is_not_a_completed_run(self):
        result = progress.summarize(self.root)
        self.assertEqual(result['samples_expected'], 264)
        self.assertEqual(result['samples_recorded'], 0)
        self.assertEqual(result['process_liveness'], 'not-checked')
        self.assertFalse(result['diagnostic_report_present'])
        self.assertFalse(result['publication_qualified'])

    def test_partial_line_and_outcomes_remain_explicit(self):
        events = [dict(event='load-progress', nodes=28, edges=36),
                  dict(event='query-start', id='q1', suite='baseline', phase='warmup', sample_index=0),
                  dict(event='observation-recorded', outcome='timeout', phase='warmup'),
                  dict(event='query-start', id='q2', suite='baseline', phase='warmup', sample_index=0)]
        (self.root / 'result/observations.jsonl').write_text(
            ''.join(json.dumps(x) + '\n' for x in events) + '{"event":')
        result = progress.summarize(self.root)
        self.assertEqual(result['load_percent'], dict(nodes=100.0, edges=50.0))
        self.assertEqual(result['outcomes'], {'timeout': 1})
        self.assertEqual(result['current_query']['id'], 'q2')
        self.assertTrue(result['ignored_partial_line'])

    def test_malformed_complete_line_fails_instead_of_hiding_it(self):
        (self.root / 'result/observations.jsonl').write_text('{bad}\n')
        with self.assertRaises(json.JSONDecodeError):
            progress.summarize(self.root)


if __name__ == '__main__':
    unittest.main()
