"""Offline original sensor fixture contracts and isolation guards; no ROS imports."""
import argparse
import builtins
import contextlib
import importlib.util
import io
import math
import os
from pathlib import Path
import struct
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('sensor_scene_fixture', ROOT / 'scripts/sensor-scene-workflow.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class SensorFixtureTests(unittest.TestCase):
    def test_packed_color_nan_bits_and_unsupported_layouts_preserve_xyz_bytes(self):
        for big in [False, True]:
            order = '>' if big else '<'
            rgba = fixture.cloud_frame('cloud-rgba', big)
            self.assertEqual(rgba['fields'][-1], dict(name='rgba', offset=16, datatype=7, count=1))
            self.assertTrue(math.isnan(struct.unpack_from(order+'f', rgba['data'], 16)[0]))
            self.assertEqual(struct.unpack_from(order+'I', rgba['data'], 16)[0], 0xffc10203)
            unsupported = fixture.cloud_frame('cloud-unsupported', big)
            self.assertEqual((unsupported['fields'][-1]['datatype'], unsupported['fields'][-1]['count']), (2,4))
            for at in [0,32,72,104]:
                self.assertEqual(rgba['data'][at:at+12],unsupported['data'][at:at+12])
        skipped = fixture.cloud_frame('cloud-attributes-skipped')
        self.assertTrue(math.isnan(struct.unpack_from('<f',skipped['data'],32+12)[0]))
        self.assertTrue(math.isinf(struct.unpack_from('<f',skipped['data'],72+12)[0]))

    def test_signal_handlers_request_stop_without_raising_and_restore_prior_handlers(self):
        signals = fixture.signal
        old = {signals.SIGINT: object(), signals.SIGTERM: object()}
        state = fixture.SignalStop()
        with patch.object(signals, 'getsignal', side_effect=lambda signum: old[signum]), \
                patch.object(signals, 'signal') as register:
            state.install()
            self.assertFalse(state.requested)
            self.assertEqual(register.call_count, 2)
            installed = {call.args[0]: call.args[1] for call in register.call_args_list}
            installed[signals.SIGINT](signals.SIGINT, None)
            self.assertTrue(state.requested)
            self.assertEqual(state.signum, signals.SIGINT)
            # A repeated signal during destruction must not throw KeyboardInterrupt
            # or replace the first recorded cause with a misleading terminal result.
            installed[signals.SIGTERM](signals.SIGTERM, None)
            self.assertEqual(state.signum, signals.SIGINT)
            state.restore()
            self.assertEqual(register.call_count, 4)
            restored = {call.args[0]: call.args[1] for call in register.call_args_list[2:]}
            self.assertEqual(restored, old)
            self.assertFalse(state.previous)

    def test_reordered_mixed_width_endian_cloud_and_padding_have_known_points(self):
        for big in [False, True]:
            frame = fixture.cloud_frame(big_endian=big)
            self.assertEqual((frame['width'], frame['height'], frame['point_step'], frame['row_step']), (2, 2, 32, 72))
            self.assertEqual(len(frame['data']), 144)
            self.assertEqual([f['name'] for f in frame['fields']], ['y', 'intensity', 'z', 'x'])
            points = []
            order = '>' if big else '<'
            for i in range(4):
                at = (i // 2) * 72 + (i % 2) * 32
                points.append([struct.unpack_from(order + 'f', frame['data'], at + 8)[0],
                               struct.unpack_from(order + 'd', frame['data'], at + 20)[0],
                               struct.unpack_from(order + 'd', frame['data'], at)[0]])
                self.assertEqual(frame['data'][at + 16:at + 20], b'\xa5' * 4)
                self.assertEqual(frame['data'][at + 28:at + 32], b'\xa5' * 4)
            self.assertEqual(points, [[1, 0, 0], [0, 1, 0], [0, 0, 1], [1, 1, 1]])
            for at in [64, 136]:
                self.assertEqual(frame['data'][at:at + 8], b'\xa5' * 8)
            transformed = [[1 - y, 2 + x, 3 + z] for x, y, z in points]
            self.assertEqual(transformed, [[1, 3, 3], [0, 2, 3], [1, 2, 4], [0, 3, 4]])

    def test_empty_skipped_malformed_and_pressure_are_distinct_bounded_inputs(self):
        empty = fixture.cloud_frame('cloud-empty')
        self.assertEqual((empty['width'], empty['height'], empty['row_step'], empty['data']), (0, 1, 0, b''))
        skipped = fixture.cloud_frame('cloud-skipped')
        self.assertFalse(skipped['is_dense'])
        self.assertTrue(math.isnan(struct.unpack_from('<f', skipped['data'], 8)[0]))
        self.assertEqual(struct.unpack_from('<d', skipped['data'], 72)[0], 1_000_001)
        malformed = fixture.cloud_frame('cloud-malformed')
        self.assertLess(malformed['row_step'], malformed['width'] * malformed['point_step'])
        self.assertEqual(len(malformed['data']), 144)
        pressure = fixture.cloud_frame('cloud-pressure')
        self.assertEqual(pressure['width'] * pressure['height'], 8192)
        self.assertEqual(len(pressure['data']), 256 * 1024)
        self.assertEqual(pressure['row_step'] * pressure['height'], len(pressure['data']))
        with self.assertRaises(ValueError):
            fixture.cloud_frame('unknown')

    def test_common_tf_recipe_omits_only_cloud_until_recovery(self):
        before, after = fixture.static_transforms(False), fixture.static_transforms(True)
        self.assertNotIn('sensor_cloud', [t['child'] for t in before])
        self.assertEqual(after[:-1], before)
        self.assertEqual(after[-1]['child'], 'sensor_cloud')
        self.assertEqual(after[-1]['translation'], [1., 2., 3.])
        self.assertIn('tip', [t['child'] for t in before])
        for transform in after:
            self.assertAlmostEqual(sum(v * v for v in transform['rotation_xyzw']), 1)
            self.assertEqual(len([t for t in after if t['child'] == transform['child']]), 1)
        scan_a = next(t for t in after if t['child'] == 'sensor_scan_a')
        self.assertEqual(scan_a['translation'], after[-1]['translation'])
        self.assertEqual(scan_a['rotation_xyzw'], after[-1]['rotation_xyzw'])
        scan_b = next(t for t in after if t['child'] == 'sensor_scan_b')
        self.assertEqual(scan_b['translation'], [-1., 0., .5])
        self.assertLess(scan_b['rotation_xyzw'][2], 0)

    def test_invalid_arguments_and_import_never_import_ros_or_open_publishers(self):
        original = builtins.__import__
        def no_ros(name, *args, **kwargs):
            if name.split('.')[0] in {'rclpy', 'sensor_msgs', 'geometry_msgs', 'std_msgs', 'tf2_msgs'}:
                self.fail('ROS import before guarded publisher entry')
            return original(name, *args, **kwargs)
        with patch('builtins.__import__', side_effect=no_ros):
            loaded = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(loaded)
            with patch.object(loaded, 'checked_environment') as guard:
                for mode, seconds in [('invalid', 1), ('nominal', 0), ('nominal', 3601)]:
                    with self.assertRaises((ValueError, SystemExit, argparse.ArgumentTypeError)):
                        loaded.publisher('rmw_fastrtps_cpp', Path('unused'), mode, seconds)
                guard.assert_not_called()
            with patch.object(loaded, 'checked_environment', side_effect=SystemExit('unsafe environment')):
                with self.assertRaises(SystemExit):
                    loaded.publisher('rmw_fastrtps_cpp', Path('unused'), 'nominal', 1)

    def test_guard_rejects_remote_domain_runtime_pair_and_reused_evidence(self):
        base = dict(ROS_DISTRO='jazzy', ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1', ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST', RMW_IMPLEMENTATION='rmw_fastrtps_cpp')
        evidence = ROOT / 'artifacts/feasibility/evidence/offline-sensor-unused'
        cases = [dict(ROS_DOMAIN_ID='0'), dict(ROS_LOCALHOST_ONLY='0'), dict(ROS_AUTOMATIC_DISCOVERY_RANGE='SUBNET'), dict(ROS_DISTRO='humble'), dict(RMW_IMPLEMENTATION='rmw_cyclonedds_cpp')]
        cases += [{key: 'remote/custom'} for key in ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI', 'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']]
        for change in cases:
            with patch.dict(os.environ, {**base, **change}, clear=True), patch.object(fixture.ROBOT.sys, 'prefix', str(fixture.HELPERS['RUNTIME'])), patch.object(Path, 'exists', return_value=False), patch.object(Path, 'mkdir') as mkdir:
                with self.assertRaises(SystemExit):
                    fixture.checked_environment('rmw_fastrtps_cpp', evidence)
                mkdir.assert_not_called()
        with patch.dict(os.environ, base, clear=True), patch.object(Path, 'exists', return_value=True), self.assertRaises(SystemExit):
            fixture.checked_environment('rmw_fastrtps_cpp', evidence)
        with patch.dict(os.environ, base, clear=True), self.assertRaises(SystemExit):
            fixture.checked_environment('rmw_fastrtps_cpp', Path('/tmp/outside-sensor-evidence'))

    def test_cli_requires_pair_evidence_and_bounded_seconds(self):
        parser = fixture.argument_parser()
        selected = parser.parse_args(['--rmw', 'rmw_cyclonedds_cpp', '--evidence', 'new', '--big-endian'])
        self.assertEqual(selected.mode, 'nominal')
        self.assertTrue(selected.big_endian)
        self.assertEqual(selected.seconds, 600)
        for flags in [[], ['--rmw', 'bad', '--evidence', 'new'], ['--rmw', 'rmw_fastrtps_cpp', '--evidence', 'new', '--seconds', 'NaN']]:
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                parser.parse_args(flags)


if __name__ == '__main__':
    unittest.main()
