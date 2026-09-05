import unittest

from host_preflight import assess


class HostPreflightTests(unittest.TestCase):
    def test_quiet_inventory(self):
        self.assertTrue(assess('1 0 0.1 /sbin/launchd\n2 1 1.2 /usr/bin/ps')['startup_screen_passed'])

    def test_busy_job_blocks(self):
        result = assess('27468 27458 261.9 target/release/et')
        self.assertFalse(result['startup_screen_passed'])
        self.assertEqual(result['busy_processes'][0]['pid'], 27468)

    def test_aggregate_contention_blocks(self):
        self.assertFalse(assess('1 0 70 a\n2 0 70 b\n3 0 70 c')['startup_screen_passed'])

    def test_paths_with_spaces_are_not_arguments(self):
        result = assess('123 1 110 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome')
        self.assertEqual(result['busy_processes'][0]['executable'], 'Google Chrome')

    def test_invalid_inventory_fails_closed(self):
        for raw in ['', 'bad line', '1 0 nan x', '1 0 inf x', '1 0 -1 x']:
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                assess(raw)


if __name__ == '__main__':
    unittest.main()
