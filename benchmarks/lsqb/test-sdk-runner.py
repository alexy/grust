"""SDK Docker ownership, resource and transport gate mutation tests."""
import copy
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('sdk', Path(__file__).with_name('run-sdk.py'))
sdk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sdk)


class SdkRunnerTests(unittest.TestCase):
    def setUp(self):
        self.image = 'example/server@sha256:' + 'a' * 64
        self.server = {
            'Config': {'Image': self.image, 'Labels': {'io.adversarial.disposable': 'helix-sdk'}},
            'State': {'Running': True, 'OOMKilled': False},
            'HostConfig': {'PortBindings': {}, 'Memory': sdk.MEMORY,
                           'MemorySwap': sdk.MEMORY, 'NanoCpus': 8_000_000_000},
            'NetworkSettings': {'Networks': {sdk.NETWORK: {'IPAddress': '172.30.0.2'}}},
        }

    def test_owned_isolated_server_is_admitted(self):
        self.assertEqual(sdk.validate_server(self.server, 'helix-sdk', self.image), '172.30.0.2')

    def test_refuses_mutable_image_and_http_ownership(self):
        with self.assertRaises(ValueError):
            sdk.validate_server(self.server, 'helix-sdk', 'example/server:latest')
        self.server['Config']['Labels']['io.adversarial.disposable'] = 'helix'
        with self.assertRaises(ValueError):
            sdk.validate_server(self.server, 'helix-sdk', self.image)

    def test_refuses_unhealthy_unbounded_or_public_server(self):
        mutations = [
            ('State', 'Running', False), ('State', 'OOMKilled', True),
            ('HostConfig', 'Memory', 0), ('HostConfig', 'MemorySwap', -1),
            ('HostConfig', 'NanoCpus', 0), ('HostConfig', 'PortBindings', {'8080/tcp': []}),
            ('Config', 'Image', 'another@sha256:' + 'b' * 64),
            ('NetworkSettings', 'Networks', {'bridge': {'IPAddress': '172.30.0.2'}}),
        ]
        for section, key, value in mutations:
            broken = copy.deepcopy(self.server)
            broken[section][key] = value
            with self.subTest(section=section, key=key), self.assertRaises(ValueError):
                sdk.validate_server(broken, 'helix-sdk', self.image)

    def test_sdk_endpoints_do_not_use_http_lane_variables(self):
        self.assertEqual(sdk.endpoint('helix-sdk', '172.30.0.2'),
                         'HELIX_SDK_BASE_URL=http://172.30.0.2:8080')
        self.assertEqual(sdk.endpoint('surreal-sdk', '172.30.0.3'),
                         'SURREAL_SDK_URL=ws://172.30.0.3:8000')

    def test_local_source_image_requires_matching_content_and_revision(self):
        pinned = 'sha256:' + 'b' * 64
        revision = 'c' * 40
        image = {'Id': pinned, 'Architecture': 'arm64', 'Os': 'linux',
                 'Config': {'Labels': {'org.opencontainers.image.revision': revision}}}
        sdk.validate_source_image(image, pinned, revision)
        self.server['Image'] = pinned
        self.server['Config']['Image'] = pinned
        sdk.validate_server(self.server, 'helix-sdk', pinned)
        for value in (None, 'short', 'd' * 40):
            with self.subTest(revision=value), self.assertRaises(ValueError):
                sdk.validate_source_image(image, pinned, value)
        for key, value in [('Id', 'sha256:' + 'e' * 64),
                           ('Architecture', 'amd64'), ('Os', 'windows')]:
            broken = copy.deepcopy(image)
            broken[key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                sdk.validate_source_image(broken, pinned, revision)
        self.server['Image'] = 'sha256:' + 'f' * 64
        with self.assertRaises(ValueError):
            sdk.validate_server(self.server, 'helix-sdk', pinned)

    def test_registry_image_does_not_accept_source_override(self):
        sdk.validate_source_image({}, self.image, None)
        with self.assertRaises(ValueError):
            sdk.validate_source_image({}, self.image, 'c' * 40)


if __name__ == '__main__':
    unittest.main()
