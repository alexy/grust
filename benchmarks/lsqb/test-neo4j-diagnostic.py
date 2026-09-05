"""Mutation coverage for native diagnostic observation admission."""
import copy
import importlib.util
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('native_check', Path(__file__).with_name('validate-neo4j-diagnostic.py'))
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)


class ObservationTests(unittest.TestCase):
    def setUp(self):
        self.reference = ('baseline', 'q1', 8, 'a' * 64)
        self.item = dict(event='observation-recorded', complete=False, suite='baseline', id='q1',
                         expected_count=8, actual_count=8, outcome='pass', termination='normal-exit',
                         source_sha256='a' * 64, query_sha256='a' * 64, setup_ns=1, elapsed_ns=2,
                         process_recovery_ns=1, query_timeout_ms=60000,
                         timing_boundary='coordinator-go-through-scalar-consumption-and-rollback-result',
                         server_recovery=dict(owned_transactions_remaining=0, subsequent_scalar=42,
                                              server_recovery_ns=1, transaction_tag='neo4j-7-123',
                                              targeted_termination_count=0, terminated_transaction_ids=[]))

    def test_pass_and_honest_mismatch(self):
        check.check_observation(self.item, self.reference, set())
        self.item.update(outcome='mismatch', actual_count=7)
        check.check_observation(self.item, self.reference, set())

    def test_timeout_requires_deadline_and_process_termination(self):
        self.item.update(outcome='timeout', actual_count=None, termination='deadline-sigterm', elapsed_ns=60_000_000_000)
        check.check_observation(self.item, self.reference, set())
        for key, value in [('elapsed_ns', 1), ('termination', 'normal-exit'), ('actual_count', 8)]:
            broken = copy.deepcopy(self.item)
            broken[key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                check.check_observation(broken, self.reference, set())

    def test_reject_count_source_timing_and_identity_mutations(self):
        for key, value in [('actual_count', 7), ('expected_count', 7), ('id', 'q2'),
                           ('source_sha256', 'b' * 64), ('complete', True), ('setup_ns', True),
                           ('query_timeout_ms', 1), ('outcome', 'unsupported'), ('elapsed_ns', 60_000_000_001)]:
            broken = copy.deepcopy(self.item)
            broken[key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                check.check_observation(broken, self.reference, set())

    def test_recovery_is_mandatory(self):
        for key, value in [('owned_transactions_remaining', 1), ('subsequent_scalar', 0),
                           ('targeted_termination_count', 1), ('transaction_tag', 'unknown'),
                           ('server_recovery_ns', -1), ('terminated_transaction_ids', ['a', 'b'])]:
            broken = copy.deepcopy(self.item)
            broken['server_recovery'][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                check.check_observation(broken, self.reference, set())
        with self.assertRaises(ValueError):
            check.check_observation(self.item, self.reference, {'neo4j-7-123'})


class JournalTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.upstream, self.attacks = self.root / 'upstream', self.root / 'attacks'
        for folder in (self.upstream / 'cypher', self.upstream / 'expected-output', self.attacks, self.root / 'result'):
            folder.mkdir(parents=True)
        counts = [8, 3, 6, 8, 3, 8, 11, 2, 4]
        oracle = ''.join(f'expected\t\texample\t{i}\t0\t{count}\n' for i, count in enumerate(counts, 1)).encode()
        (self.upstream / 'expected-output/expected-output.csv').write_bytes(oracle)
        # A small synthetic pinned oracle keeps tests independent of downloaded
        # repositories; production always uses the fixed upstream digest.
        self.oracle_sha = hashlib.sha256(oracle).hexdigest()
        base = ObservationTests()
        base.setUp()
        observations = []
        attack_counts = [8, 3, 8, 11, 2, 10000, 125, 5, 5, 5, 5, 28, 72]
        entries = [('baseline', f'q{i}', count) for i, count in enumerate(counts, 1)]
        entries += [('adversarial', f'a{i}-{name}', count)
                    for i, (name, count) in enumerate(zip(check.ATTACKS, attack_counts), 1)]
        for index, (suite, query_id, count) in enumerate(entries):
            source = f'// {query_id}\nRETURN {count} AS count\n'.encode()
            directory = self.upstream / 'cypher' if suite == 'baseline' else self.attacks
            (directory / f'{query_id}.cypher').write_bytes(source)
            item = copy.deepcopy(base.item)
            item.update(suite=suite, id=query_id, expected_count=count, actual_count=count,
                        source_sha256=hashlib.sha256(source).hexdigest(), query_sha256=hashlib.sha256(source).hexdigest())
            item['server_recovery']['transaction_tag'] = f'neo4j-7-{index}'
            observations.append(item)
        self.report = dict(schema='grust-neo4j-native-diagnostic-v1', complete=False,
                           publication_receipt=None, driver='neo4rs', driver_version='0.9.0-rc.10',
                           scale='example', load_ns=2_000_000, observations=observations,
                           dataset=dict(scale_factor='example', nodes=28, edges=72, person_nodes=5,
                                        extracted_manifest_sha256=check.DATASETS['example'][3]))
        self.events = [dict(event='load-progress', complete=False, nodes=28, edges=72, elapsed_ms=1)]
        for item in observations:
            self.events += [dict(event='query-start', complete=False, suite=item['suite'], id=item['id']), copy.deepcopy(item)]
        self.watchdog = dict(schema='grust-lsqb-cell-watchdog-completion-v1', status='complete',
                             child_exit_status=0, elapsed_wall_ms=10, timeout_ms=600000)

    def audit(self):
        (self.root / 'result/diagnostic.json').write_text(json.dumps(self.report))
        (self.root / 'result/observations.jsonl').write_text(''.join(json.dumps(x) + '\n' for x in self.events))
        (self.root / 'watchdog.json').write_text(json.dumps(self.watchdog))
        with patch.object(check, 'ORACLE_SHA', self.oracle_sha):
            return check.validate(self.root, self.upstream, self.attacks)

    def test_complete_fixture_is_still_not_publication(self):
        self.assertEqual(self.audit()['outcomes']['pass'], 22)
        self.assertIs(self.audit()['publication_qualified'], False)

    def sampled_fixture(self):
        originals = self.report['observations']
        self.report.update(schema='grust-neo4j-native-diagnostic-v2',
                           sampling=dict(warmups_per_query=1, measurements_per_query=2,
                                         order='query-major-warmups-then-measurements',
                                         worker_lifecycle='fresh-process-per-sample'))
        self.report['observations'] = []
        self.events = self.events[:1]
        for original in originals:
            for phase, index in [('warmup', 0), ('measurement', 0), ('measurement', 1)]:
                item = copy.deepcopy(original)
                item.update(phase=phase, sample_index=index)
                item['server_recovery']['transaction_tag'] = f'neo4j-7-{len(self.report["observations"])}'
                self.report['observations'].append(item)
                self.events += [dict(event='query-start', complete=False, suite=item['suite'], id=item['id'],
                                     phase=phase, sample_index=index), item]

    def test_warmup_mismatch_does_not_enter_measurement_totals(self):
        self.sampled_fixture()
        self.report['observations'][0].update(actual_count=7, outcome='mismatch')
        result = self.audit()
        self.assertEqual(result['outcomes']['mismatch'], 1)
        self.assertEqual(result['measurement_outcomes']['pass'], 44)
        self.assertEqual(result['measurement_outcomes']['mismatch'], 0)

    def test_mislabeled_and_omitted_samples_are_rejected(self):
        self.sampled_fixture()
        self.report['observations'][0]['phase'] = 'measurement'
        self.events[1]['phase'] = 'measurement'
        with self.assertRaises(ValueError):
            self.audit()
        self.report['observations'][0]['phase'] = 'warmup'
        self.events[1]['phase'] = 'warmup'
        self.report['observations'].pop()
        self.events = self.events[:-2]
        with self.assertRaises(ValueError):
            self.audit()

    def test_truncated_or_reordered_journal_is_rejected(self):
        original = copy.deepcopy(self.events)
        for events in (original[:-1], original[1:], [original[0], *original[2:]],
                       [original[0], original[2], original[1], *original[3:]]):
            self.events = events
            with self.assertRaises(ValueError):
                self.audit()

    def test_report_only_count_forgery_is_rejected(self):
        self.report['observations'][0]['actual_count'] = 99
        with self.assertRaises(ValueError):
            self.audit()


    def test_watchdog_failure_and_false_completion_are_rejected(self):
        self.watchdog['child_exit_status'] = 124
        with self.assertRaises(ValueError):
            self.audit()
        self.watchdog['child_exit_status'] = 0
        self.report['complete'] = True
        with self.assertRaises(ValueError):
            self.audit()


class RuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        resources = dict(Memory=6 * 1024**3, MemorySwap=6 * 1024**3, NanoCpus=8_000_000_000,
                         ReadonlyRootfs=True, NetworkMode='grust-lsqb-neo4j-qualification')
        labels = {'com.docker.compose.project': 'owned', 'com.docker.compose.service': 'benchmark'}
        client = dict(container_id='a' * 64, image_id=check.CLIENT_IMAGE_ID, name='/client',
                      resources=resources, labels=labels,
                      state=dict(Status='created', Running=False, OOMKilled=False, ExitCode=0, StartedAt='zero'))
        server = copy.deepcopy(client)
        server.update(container_id='b' * 64, image_id=check.SERVER_IMAGE.rsplit('@', 1)[1], name='/server')
        server['state'].update(Status='running', Running=True, StartedAt='start')
        self.records = {'client-before': client, 'client-after': copy.deepcopy(client),
                        'server-before': server, 'server-after': copy.deepcopy(server),
                        'watchdog': dict(container_id='a' * 64, container_name='client', project='owned', service='benchmark'),
                        'invocation': dict(diagnostic_only=True, source_revision=check.SOURCE_REVISION,
                                           client_image_id=check.CLIENT_IMAGE_ID, server_image=check.SERVER_IMAGE,
                                           client_labels={'org.opencontainers.image.revision': check.SOURCE_REVISION,
                                                          'io.adversarial.grust.benchmark-feature': 'neo4j-native'})}
        self.records['client-after']['state'].update(Status='exited', StartedAt='start')

    def audit(self):
        for name, value in self.records.items():
            (self.root / f'{name}.json').write_text(json.dumps(value))
        return check.validate_runtime(self.root)

    def test_consistent_records_pass(self):
        self.assertTrue(self.audit())

    def test_race_fix_profile_is_bound_to_its_own_image(self):
        revision = 'bb4f7c161fdae90cc9fc2b35aaf0870e9da91164'
        invocation = self.records['invocation']
        invocation['source_revision'] = revision
        invocation['client_labels']['org.opencontainers.image.revision'] = revision
        with self.assertRaises(ValueError):
            self.audit()
        image = check.CLIENT_PROFILES[revision]
        invocation['client_image_id'] = image
        for phase in ('before', 'after'):
            self.records[f'client-{phase}']['image_id'] = image
        self.assertTrue(self.audit())

    def test_sampling_requires_capable_source_and_matching_invocation(self):
        folder = self.root / 'result'
        folder.mkdir()
        (folder / 'diagnostic.json').write_text(json.dumps(dict(
            schema='grust-neo4j-native-diagnostic-v2', scale='example',
            sampling=dict(warmups_per_query=2, measurements_per_query=10))))
        self.records['invocation']['command'] = ['qualify', '/opt/lsqb-mounted', '/opt/grust-attacks',
                                                 'example', '/out/result', '2', '10']
        with self.assertRaises(ValueError):
            self.audit()
        with patch.object(check, 'SAMPLED_SOURCES', {check.SOURCE_REVISION}):
            self.assertTrue(self.audit())
            self.records['invocation']['command'][-1] = '1'
            with self.assertRaises(ValueError):
                self.audit()

    def test_restart_oom_changed_resources_and_wrong_watchdog_fail(self):
        original = copy.deepcopy(self.records)
        mutations = [('server-after', 'state', 'StartedAt', 'new-start'),
                     ('client-after', 'state', 'OOMKilled', True),
                     ('server-after', 'resources', 'Memory', 1),
                     ('client-after', 'state', 'Running', True)]
        for record, group, key, value in mutations:
            self.records = copy.deepcopy(original)
            self.records[record][group][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.audit()
        self.records = copy.deepcopy(original)
        self.records['watchdog']['container_id'] = 'c' * 64
        with self.assertRaises(ValueError):
            self.audit()

if __name__ == '__main__':
    unittest.main()
