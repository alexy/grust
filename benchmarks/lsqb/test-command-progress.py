"""Live small-process checks for build/test progress recording."""
import importlib.util
import json
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, call, patch

spec = importlib.util.spec_from_file_location('progress', Path(__file__).with_name('command-progress.py'))
progress = importlib.util.module_from_spec(spec)
spec.loader.exec_module(progress)


def interrupt(kind=signal.SIGTERM):
    """Deliver synchronously to the installed handler without real OS signals."""
    signal.getsignal(kind)(kind, None)


def records(output):
    return [json.loads(line) for line in (output / 'progress.jsonl').read_text().splitlines()]


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


class CancellationTests(unittest.TestCase):
    def setUp(self):
        self.previous = {kind: signal.getsignal(kind) for kind in (signal.SIGINT, signal.SIGTERM)}

    def tearDown(self):
        for kind, handler in self.previous.items():
            self.assertEqual(signal.getsignal(kind), handler)

    def test_default_and_configured_grace_own_the_child_group_and_retain_terminal_log(self):
        for grace in (None, .25, 60):
            with self.subTest(grace=grace), tempfile.TemporaryDirectory() as root, \
                    patch('builtins.print'), patch.object(progress.os, 'killpg') as kill:
                child = Mock(pid=12345, returncode=None)

                def wait(*, timeout):
                    if child.wait.call_count == 1:
                        interrupt()
                    child.returncode = -signal.SIGTERM
                    return child.returncode

                child.wait.side_effect = wait
                output = Path(root) / 'run'
                options = {} if grace is None else {'termination_grace': grace}
                with patch.object(progress.subprocess, 'Popen', return_value=child) as spawn:
                    with self.assertRaises(KeyboardInterrupt):
                        progress.run(['owned-command'], output, .02, **options)
                expected = 5 if grace is None else grace
                self.assertEqual(child.wait.call_args_list,
                                 [call(timeout=.02), call(timeout=expected), call(timeout=5)])
                self.assertEqual(kill.call_args_list,
                                 [call(child.pid, signal.SIGTERM), call(child.pid, signal.SIGKILL)])
                self.assertTrue(spawn.call_args.kwargs['start_new_session'])
                journal = records(output)
                self.assertEqual([r['event'] for r in journal], ['command-start', 'command-interrupted'])
                self.assertEqual(journal[-1]['pid'], child.pid)
                self.assertEqual(journal[-1]['exit'], -signal.SIGTERM)
                self.assertEqual(journal[-1]['termination_grace_seconds'], expected)
                self.assertEqual(journal[-1]['log_bytes'], 0)

    def test_grace_expiry_escalates_only_owned_group_then_reaps(self):
        with tempfile.TemporaryDirectory() as root, patch('builtins.print'), \
                patch.object(progress.os, 'killpg') as kill:
            child = Mock(pid=12346, returncode=None)

            def wait(*, timeout):
                if child.wait.call_count == 1:
                    # Library callers may also interrupt without an OS signal.
                    raise KeyboardInterrupt
                if child.wait.call_count == 2:
                    raise subprocess.TimeoutExpired('owned-command', timeout)
                child.returncode = -signal.SIGKILL
                return child.returncode

            child.wait.side_effect = wait
            output = Path(root) / 'run'
            with patch.object(progress.subprocess, 'Popen', return_value=child):
                with self.assertRaises(KeyboardInterrupt):
                    progress.run(['owned-command'], output, .02, termination_grace=.75)
            self.assertEqual(child.wait.call_args_list,
                             [call(timeout=.02), call(timeout=.75), call(timeout=5)])
            self.assertEqual(kill.call_args_list,
                             [call(child.pid, signal.SIGTERM), call(child.pid, signal.SIGKILL)])
            self.assertEqual(records(output)[-1]['exit'], -signal.SIGKILL)

    def test_repeated_sigint_and_sigterm_cannot_interrupt_cleanup_or_journal(self):
        with tempfile.TemporaryDirectory() as root, patch('builtins.print'):
            child = Mock(pid=12347, returncode=None)

            def repeat(*_args):
                for kind in (signal.SIGINT, signal.SIGTERM) * 3:
                    interrupt(kind)

            def wait(*, timeout):
                if child.wait.call_count == 1:
                    interrupt(signal.SIGINT)
                repeat()
                child.returncode = 0
                return 0

            child.wait.side_effect = wait
            output = Path(root) / 'run'
            original_fsync = progress.os.fsync

            def fsync(descriptor):
                if child.returncode == 0:
                    repeat()
                original_fsync(descriptor)

            with patch.object(progress.subprocess, 'Popen', return_value=child), \
                    patch.object(progress.os, 'killpg', side_effect=repeat) as kill, \
                    patch.object(progress.os, 'fsync', side_effect=fsync):
                with self.assertRaises(KeyboardInterrupt):
                    progress.run(['owned-command'], output, .02, termination_grace=60)
            self.assertEqual(kill.call_args_list,
                             [call(child.pid, signal.SIGTERM), call(child.pid, signal.SIGKILL)])
            self.assertEqual(child.wait.call_count, 3)
            self.assertEqual(records(output)[-1]['event'], 'command-interrupted')
            self.assertEqual(records(output)[-1]['exit'], 0)

    def test_spawn_window_signals_are_latched_until_the_handle_is_assigned(self):
        with tempfile.TemporaryDirectory() as root, patch('builtins.print'), \
                patch.object(progress.os, 'killpg') as kill:
            child = Mock(pid=12348, returncode=None)
            assigned = []

            def spawn(*_args, **kwargs):
                kwargs['stdout'].write(b'spawned child\n')
                interrupt(signal.SIGINT)
                interrupt(signal.SIGTERM)
                assigned.append(True)
                return child

            def wait(*, timeout):
                self.assertEqual(assigned, [True])
                child.returncode = -signal.SIGTERM
                return child.returncode

            child.wait.side_effect = wait
            output = Path(root) / 'run'
            with patch.object(progress.subprocess, 'Popen', side_effect=spawn):
                with self.assertRaises(KeyboardInterrupt):
                    progress.run(['owned-command'], output, .02, termination_grace=60)
            self.assertEqual(child.wait.call_args_list, [call(timeout=60), call(timeout=5)])
            self.assertEqual(kill.call_args_list,
                             [call(child.pid, signal.SIGTERM), call(child.pid, signal.SIGKILL)])
            final = records(output)[-1]
            self.assertEqual(final['event'], 'command-interrupted')
            self.assertEqual(final['pid'], child.pid)
            self.assertEqual(final['latest_output'], 'spawned child')
            self.assertEqual((output / 'command.log').read_bytes(), b'spawned child\n')

    def test_disappearing_group_during_escalation_still_reaps_the_child(self):
        with tempfile.TemporaryDirectory() as root, patch('builtins.print'):
            child = Mock(pid=12349, returncode=0)
            child.wait.side_effect = [KeyboardInterrupt(), subprocess.TimeoutExpired('owned', .5), 0]
            output = Path(root) / 'run'
            with patch.object(progress.subprocess, 'Popen', return_value=child), \
                    patch.object(progress.os, 'killpg', side_effect=[None, ProcessLookupError()]):
                with self.assertRaises(KeyboardInterrupt):
                    progress.run(['owned'], output, termination_grace=.5)
            self.assertEqual(child.wait.call_count, 3)
            self.assertEqual(records(output)[-1]['exit'], 0)

    def test_terminal_record_is_retained_even_if_the_final_reap_fails(self):
        with tempfile.TemporaryDirectory() as root, patch('builtins.print'), \
                patch.object(progress.os, 'killpg'):
            child = Mock(pid=12350, returncode=None)
            child.wait.side_effect = [KeyboardInterrupt(), subprocess.TimeoutExpired('owned', 5),
                                      subprocess.TimeoutExpired('owned', 5)]
            output = Path(root) / 'run'
            with patch.object(progress.subprocess, 'Popen', return_value=child):
                with self.assertRaises(subprocess.TimeoutExpired):
                    progress.run(['owned'], output)
            final = records(output)[-1]
            self.assertEqual(final['event'], 'command-interrupted')
            self.assertIsNone(final['exit'])

    def test_exited_leader_does_not_skip_residual_group_cleanup(self):
        child = Mock(pid=12351, returncode=0)
        child.wait.return_value = 0
        actions = Mock()
        actions.attach_mock(child.wait, 'wait')
        with patch.object(progress.os, 'killpg') as kill:
            actions.attach_mock(kill, 'kill')
            progress.stop_process_group(child, 60)
        self.assertEqual(actions.mock_calls,
                         [call.kill(child.pid, signal.SIGTERM), call.wait(timeout=60),
                          call.kill(child.pid, signal.SIGKILL), call.wait(timeout=5)])

    def test_partial_handler_installation_restores_the_first_handler(self):
        interruptions = progress.InterruptionSignals()
        previous = self.previous[signal.SIGINT]
        failure = RuntimeError('second handler could not be installed')
        with patch.object(progress.signal, 'signal', side_effect=[previous, failure, None]) as install:
            with self.assertRaisesRegex(RuntimeError, 'second handler'):
                with interruptions:
                    self.fail('failed context entry must not run its body')
        self.assertEqual(install.call_args_list,
                         [call(signal.SIGINT, interruptions.handle),
                          call(signal.SIGTERM, interruptions.handle), call(signal.SIGINT, previous)])

    def test_signal_during_completion_journal_still_cleans_up_and_records_interruption(self):
        child = Mock(pid=12352, returncode=0)
        child.wait.return_value = 0

        def printed(line, **_kwargs):
            if json.loads(line)['event'] == 'command-finish':
                interrupt()

        with tempfile.TemporaryDirectory() as root, patch('builtins.print', side_effect=printed), \
                patch.object(progress.subprocess, 'Popen', return_value=child), \
                patch.object(progress.os, 'killpg') as kill:
            output = Path(root) / 'run'
            with self.assertRaises(KeyboardInterrupt):
                progress.run(['owned'], output)
            self.assertEqual([record['event'] for record in records(output)],
                             ['command-start', 'command-finish', 'command-interrupted'])
            self.assertEqual(kill.call_args_list,
                             [call(child.pid, signal.SIGTERM), call(child.pid, signal.SIGKILL)])
            self.assertEqual(records(output)[-1]['exit'], 0)

    def test_pending_signal_at_context_exit_is_not_a_success(self):
        with self.assertRaises(KeyboardInterrupt):
            with progress.InterruptionSignals():
                interrupt()

    def test_invalid_graces_fail_before_output_creation_or_spawning(self):
        for grace in (0, -1, 60.001, float('nan'), float('inf'), float('-inf')):
            with self.subTest(grace=grace), tempfile.TemporaryDirectory() as root, \
                    patch.object(progress.subprocess, 'Popen') as spawn:
                output = Path(root) / 'run'
                with self.assertRaisesRegex(ValueError, 'termination grace'):
                    progress.run(['owned'], output, termination_grace=grace)
                self.assertFalse(output.exists())
                spawn.assert_not_called()

    def test_cli_passes_the_default_or_configured_grace(self):
        for options, grace in (([], 5), (['--termination-grace-seconds', '60'], 60)):
            with self.subTest(grace=grace), patch.object(progress, 'run', return_value=7) as run:
                status = progress.main(['--output', '/unused/diagnostic', *options, '--', 'owned'])
                self.assertEqual(status, 7)
                run.assert_called_once_with(['owned'], Path('/unused/diagnostic'), 30, grace)


if __name__ == '__main__':
    unittest.main()
