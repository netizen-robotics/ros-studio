"""Offline rejection checks for the native UI/ROS fixture's isolation boundary."""
import importlib.util
import contextlib
import io
import os
from pathlib import Path
import unittest
import tempfile
from unittest.mock import patch
import map_dds_profiles

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('ui_ros_workflow', ROOT / 'scripts/ui-ros-workflow.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class FixtureIsolationTests(unittest.TestCase):
    def setUp(self):
        (ROOT / 'artifacts/feasibility/evidence').mkdir(parents=True, exist_ok=True)

    def test_diagnostic_map_profiles_are_explicit_exact_and_do_not_overwrite(self):
        base = {'STUDIO_MAP_DIAGNOSTIC_PROFILES': '1', 'ROS_DISTRO': 'jazzy', 'ROS_DOMAIN_ID': '187',
                'ROS_LOCALHOST_ONLY': '1', 'ROS_AUTOMATIC_DISCOVERY_RANGE': 'LOCALHOST'}
        for rmw, (key, name) in map_dds_profiles.PROFILES.items():
            with self.subTest(rmw=rmw), tempfile.TemporaryDirectory(dir=ROOT / 'artifacts/feasibility/evidence') as folder:
                env = {**base, 'RMW_IMPLEMENTATION': rmw}
                record = map_dds_profiles.configure(folder, env)
                target = Path(record['path'])
                self.assertEqual(target.read_bytes(), (ROOT / 'fixtures/dds' / name).read_bytes())
                self.assertEqual(env[key], str(target))
                # The backend and publisher independently see clean incoming
                # environments; an identical evidence copy can be reused.
                map_dds_profiles.configure(folder, {**base, 'RMW_IMPLEMENTATION': rmw})
                target.write_text('unreviewed profile')
                with self.assertRaisesRegex(ValueError, 'differs'):
                    map_dds_profiles.configure(folder, {**base, 'RMW_IMPLEMENTATION': rmw})
                self.assertEqual(target.read_text(), 'unreviewed profile')

    def test_diagnostic_profiles_reject_incoming_overrides_and_missing_optin(self):
        base = {'STUDIO_MAP_DIAGNOSTIC_PROFILES': '1', 'ROS_DISTRO': 'jazzy', 'ROS_DOMAIN_ID': '187',
                'ROS_LOCALHOST_ONLY': '1', 'ROS_AUTOMATIC_DISCOVERY_RANGE': 'LOCALHOST', 'RMW_IMPLEMENTATION': 'rmw_fastrtps_cpp'}
        changes = [{'STUDIO_MAP_DIAGNOSTIC_PROFILES': '0'}, {'ROS_DOMAIN_ID': '42'},
                   {'ROS_AUTOMATIC_DISCOVERY_RANGE': 'SUBNET'}, {'RMW_IMPLEMENTATION': 'unknown'}]
        changes += [{key: '/tmp/unreviewed'} for key in map_dds_profiles.REMOTE]
        with tempfile.TemporaryDirectory(dir=ROOT / 'artifacts/feasibility/evidence') as folder:
            for change in changes:
                with self.subTest(change=change), self.assertRaises(ValueError):
                    map_dds_profiles.configure(folder, {**base, **change})
            self.assertEqual(list(Path(folder).iterdir()), [])
            with self.assertRaisesRegex(ValueError, 'inside ignored'):
                map_dds_profiles.configure('/tmp/unreviewed', dict(base))

    def test_optional_flood_flags_have_bounds_and_preserve_default_publishers(self):
        parser = fixture.argument_parser()
        defaults = parser.parse_args(['--publisher'])
        self.assertIsNone(defaults.flood_rate)
        self.assertEqual(defaults.flood_bytes, 1024)
        for rate, size in [('1', '1'), ('2000', '8192'), ('127.5', '2048')]:
            flags = parser.parse_args(['--publisher', '--flood-rate', rate, '--flood-bytes', size])
            self.assertEqual(flags.flood_rate, float(rate))
            self.assertEqual(flags.flood_bytes, int(size))
        for flag, value in [('--flood-rate', '0'), ('--flood-rate', '2000.1'),
                            ('--flood-rate', 'nan'), ('--flood-rate', 'inf'),
                            ('--flood-bytes', '0'), ('--flood-bytes', '8193'),
                            ('--flood-bytes', '1.5'), ('--flood-bytes', '9' * 1000)]:
            with self.subTest(flag=flag, value=value), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):
                    parser.parse_args(['--publisher', flag, value])

    def test_flood_validation_precedes_ros_peer_creation(self):
        for rate, size in [(0, 1), (2001, 1), (float('nan'), 1), (1, 8193), (1, 0)]:
            with self.subTest(rate=rate, size=size), patch.object(fixture, 'checked_environment') as guard:
                with self.assertRaises(ValueError):
                    fixture.publisher(1, flood_rate=rate, flood_bytes=size)
                guard.assert_not_called()

    def test_flood_and_regular_deadlines_are_independent_without_catchup_history(self):
        schedule = fixture.PublishSchedule(2000, 10.0)
        self.assertEqual(schedule.due(10.0), (True, True))
        self.assertEqual(schedule.due(10.0), (False, False))
        self.assertEqual(schedule.due(10.001), (False, True))
        self.assertGreater(schedule.wait(10.001), 0)
        self.assertLessEqual(schedule.wait(10.001), .001)
        # A delayed worker emits at most one message of each kind rather than
        # allocating missed-history events or starving the regular publishers.
        self.assertEqual(schedule.due(1000.0), (True, True))
        self.assertEqual(schedule.due(1000.0), (False, False))
        self.assertEqual(set(vars(schedule)), {'rate', 'regular_at', 'flood_at'})
        default = fixture.PublishSchedule(None, 0)
        self.assertEqual(default.due(0), (True, False))
        self.assertEqual(default.due(.1), (False, False))
        self.assertEqual(default.due(.2), (True, False))

    def test_rejects_unsafe_environment_before_creating_ros_peers(self):
        base = {'ROS_DISTRO': 'jazzy', 'ROS_DOMAIN_ID': '187', 'ROS_LOCALHOST_ONLY': '1',
                'ROS_AUTOMATIC_DISCOVERY_RANGE': 'LOCALHOST', 'RMW_IMPLEMENTATION': 'rmw_fastrtps_cpp',
                'STUDIO_UI_ROS_EVIDENCE': str(ROOT / 'artifacts/feasibility/evidence/guard-test')}
        for name, changes in [
            ('domain', {'ROS_DOMAIN_ID': '57'}),
            ('localhost', {'ROS_LOCALHOST_ONLY': '0'}),
            ('range', {'ROS_AUTOMATIC_DISCOVERY_RANGE': 'SUBNET'}),
            ('remote-peer', {'ROS_STATIC_PEERS': '192.0.2.1'}),
            ('custom-fast-profile', {'FASTRTPS_DEFAULT_PROFILES_FILE': '/tmp/unreviewed', 'STUDIO_MAP_DIAGNOSTIC_PROFILES': '1'}),
            ('custom-cyclone-profile', {'CYCLONEDDS_URI': '/tmp/unreviewed', 'STUDIO_MAP_DIAGNOSTIC_PROFILES': '1'}),
            ('rmw', {'RMW_IMPLEMENTATION': 'invalid'}),
            ('evidence-path', {'STUDIO_UI_ROS_EVIDENCE': '/tmp/invalid-ui-evidence'}),
        ]:
            with self.subTest(case=name), patch.dict(os.environ, {**base, **changes}, clear=True):
                with self.assertRaises(SystemExit):
                    fixture.checked_environment()


if __name__ == '__main__':
    unittest.main()
