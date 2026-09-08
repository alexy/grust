#!/usr/bin/env python3
"""A memory-exceeded cell is declared only from a container's own retained exit."""

import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    'declare', Path(__file__).with_name('declare-cell-termination.py'))
declare = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(declare)

WATCHDOG = {
    'schema': 'grust-lsqb-cell-watchdog-completion-v1',
    'status': 'complete',
    'child_exit_status': 137,
    'container_termination': {'exit_code': 137, 'oom_killed': True},
    'container_id': 'a' * 64,
    'container_name': 'grust-lsqb-matrix-1-2-baseline-turso-cell',
    'project': 'grust-lsqb-matrix-1-2',
    'service': 'benchmark',
    'elapsed_wall_ms': 266726,
    'timeout_ms': 17799000,
}
ARGUMENTS = [
    '--suite', 'baseline', '--backend', 'turso', '--scale', '0.3',
    '--component', 'baseline-turso-sf0.3.json',
    '--runner-image', 'grust-lsqb-matrix-turso:0.13',
    '--runner-image-id', 'sha256:' + 'b' * 64,
    '--memory-limit-bytes', '6442450944', '--cell-timeout-ms', '17799000',
]


class DeclarationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='grust-cell-declaration.')
        self.root = Path(self.temporary.name)
        self.addCleanup(self.temporary.cleanup)

    def run_declaration(self, record):
        watchdog = self.root / 'watchdog.json'
        watchdog.write_text(json.dumps(record) if record is not None else 'not json')
        output = self.root / 'declaration.json'
        status = declare.main([*ARGUMENTS, '--watchdog', str(watchdog), '--output', str(output)])
        return status, output

    def test_oom_killed_container_is_declared_with_its_evidence(self):
        status, output = self.run_declaration(WATCHDOG)
        self.assertEqual(status, 0)
        value = json.loads(output.read_text())
        self.assertEqual(value['schema'], 'grust-lsqb-cell-memory-exceeded-v1')
        self.assertEqual(value['backend'], 'turso')
        self.assertEqual(value['suite'], 'baseline')
        self.assertEqual(value['scale'], '0.3')
        self.assertEqual(value['memory_limit_bytes'], 6442450944)
        self.assertIs(value['publication_qualified'], False)
        self.assertEqual(value['watchdog'], WATCHDOG)

    def test_a_worker_oom_leaves_the_main_process_to_exit_nonzero(self):
        # A cgroup OOM that takes a worker rather than the container's main
        # process: Docker still reports OOMKilled, the exit status is 1, and
        # the cell is as gone as it would be at 137.
        record = copy.deepcopy(WATCHDOG)
        record['child_exit_status'] = 1
        record['container_termination'] = {'exit_code': 1, 'oom_killed': True}
        status, output = self.run_declaration(record)
        self.assertEqual(status, 0)
        self.assertEqual(json.loads(output.read_text())['watchdog']['child_exit_status'], 1)

    def test_a_cell_that_only_exited_137_is_not_declared(self):
        record = copy.deepcopy(WATCHDOG)
        record.pop('container_termination')
        self.assertEqual(self.run_declaration(record)[0], declare.UNPROVEN_EXIT)

    def test_mutations_that_do_not_prove_a_memory_termination_fail_closed(self):
        for name, mutate in [
            ('not oom killed', lambda r: r['container_termination'].update(oom_killed=False)),
            ('clean child exit', lambda r: r.update(child_exit_status=0)),
            ('unread container state', lambda r: r.update(container_termination=None)),
            ('extra termination field', lambda r: r['container_termination'].update(signal='KILL')),
            ('incomplete watchdog', lambda r: r.update(status='error')),
            ('wrong schema', lambda r: r.update(schema='other')),
        ]:
            record = copy.deepcopy(WATCHDOG)
            mutate(record)
            with self.subTest(name=name):
                self.assertEqual(self.run_declaration(record)[0], declare.UNPROVEN_EXIT)

    def test_unreadable_record_is_an_error_not_a_declaration(self):
        self.assertEqual(self.run_declaration(None)[0], 2)

    def test_an_existing_declaration_is_never_overwritten(self):
        status, output = self.run_declaration(WATCHDOG)
        self.assertEqual(status, 0)
        watchdog = self.root / 'watchdog.json'
        self.assertEqual(
            declare.main([*ARGUMENTS, '--watchdog', str(watchdog), '--output', str(output)]), 2)


if __name__ == '__main__':
    unittest.main()
