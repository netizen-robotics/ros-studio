"""Offline mapping recipe checks; neither ROS nor native windows are launched."""
import argparse
import builtins
import contextlib
import importlib.util
import io
import math
import os
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('mapping_fixture', ROOT / 'scripts/mapping-scene-workflow.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class MappingFixtureTests(unittest.TestCase):
    def test_json_integer_pose_path_origin_and_tf_fields_are_assigned_as_floats(self):
        # These are the actual message-assignment adapters used by the publisher,
        # exercised without importing generated ROS messages.
        for position, yaw in [(fixture.LAYOUT['pose_source'], 90),
                              (fixture.LAYOUT['path_source'][1], 0),
                              (fixture.LAYOUT['map']['origin_translation'], 90)]:
            pose = SimpleNamespace(position=SimpleNamespace(), orientation=SimpleNamespace())
            fixture.set_pose_fields(pose, position, yaw)
            self.assertEqual([pose.position.x, pose.position.y, pose.position.z], position)
            self.assertTrue(all(type(value) is float for value in vars(pose.position).values()))
            self.assertTrue(all(type(value) is float for value in vars(pose.orientation).values()))
            self.assertAlmostEqual(sum(value * value for value in vars(pose.orientation).values()), 1.)
        transform = json_values = fixture.LAYOUT['tf']['translation']
        target = SimpleNamespace()
        fixture.set_float_fields(target, ('x', 'y', 'z'), transform)
        self.assertEqual([target.x, target.y, target.z], json_values)
        self.assertTrue(all(type(value) is float for value in vars(target).values()))
        fixture.set_float_fields(target, ('x', 'y', 'z', 'w'), [0, 0, 0, 1])
        self.assertTrue(all(type(value) is float for value in vars(target).values()))
        fixture.set_pose_fields(pose, [1, 2, 3], 0, invalid=True)
        self.assertEqual(vars(pose.orientation), dict(x=0., y=0., z=0., w=0.))
        self.assertTrue(all(type(value) is float for value in vars(pose.orientation).values()))
        with self.assertRaises(ValueError):
            fixture.set_float_fields(target, ('x',), [float('nan')])
        with self.assertRaises(ValueError):
            fixture.set_float_fields(target, ('x', 'y'), [1])

    def test_duplicate_changed_and_historical_map_stamps_do_not_use_current_time(self):
        original, current = (100, 123), (105, 456)
        self.assertEqual(fixture.map_stamp('map-duplicate', current, original), original)
        self.assertEqual(fixture.map_stamp('map-changed-stamp', current, original), original)
        self.assertEqual(fixture.map_stamp('map-historical', current, original), (85, 456))
        self.assertEqual(fixture.map_stamp('nominal', current, original), current)
        with self.assertRaises(ValueError):
            fixture.map_stamp('map-historical', (20, 0), original)

    def test_original_rotated_origin_then_tf_aligns_map_pose_path_with_sensor_point(self):
        data = fixture.geometry()
        grid = data['map']
        self.assertEqual(grid['cells'], [-1, 0, 25, 50, 75, 100])
        def world(x, y):
            # Independent exact 90-degree source-origin and source-to-fixed rotations.
            sx, sy = 1.25 - y, -.25 + x
            return [1 - sy, 2 + sx, 3.]
        centers = [world((i + .5) * .5, (j + .5) * .5) for j in range(2) for i in range(3)]
        self.assertEqual(centers[:3], [[1., 3., 3.], [.5, 3., 3.], [0., 3., 3.]])
        self.assertEqual(centers[3], [1., 2.5, 3.])
        self.assertEqual(fixture.LAYOUT['expected_pose_fixed'], centers[0])
        self.assertEqual(fixture.LAYOUT['expected_pose_fixed_yaw_degrees'], 180)
        transformed = [[1 - y, 2 + x, 3 + z] for x, y, z in fixture.LAYOUT['path_source']]
        self.assertEqual(transformed, fixture.LAYOUT['expected_path_fixed'])
        self.assertEqual(transformed[0], fixture.SENSOR.LAYOUT['expected_fixed_points'][0])

    def test_maximum_empty_malformed_and_changing_modes_are_explicit(self):
        large = fixture.geometry('large')
        self.assertEqual((large['map']['width'], large['map']['height'], len(large['map']['cells'])), (256, 256, 65536))
        self.assertEqual(len(large['path']), 1024)
        self.assertTrue(all(-1 <= v <= 100 for v in large['map']['cells']))
        empty = fixture.geometry('empty')
        self.assertEqual((empty['map']['width'], empty['map']['height'], empty['map']['cells'], empty['path']), (0, 0, [], []))
        self.assertEqual(fixture.geometry('map-malformed')['map']['cells'][0], 101)
        self.assertEqual(fixture.geometry('map-over-limit')['map']['width'], 257)
        self.assertEqual(len(fixture.geometry('path-over-limit')['path']), 1025)
        self.assertTrue(fixture.geometry('pose-malformed')['pose']['invalid_quaternion'])
        self.assertEqual(fixture.geometry('path-malformed')['path'][1]['frame_id'], 'contradictory_frame')
        self.assertNotEqual(fixture.geometry('path-updates', 0)['path'], fixture.geometry('path-updates', 1)['path'])
        self.assertEqual(fixture.geometry()['map']['cells'][0], -1, 'mutating a returned recipe cannot alter original input')
        for mode, tick in [('wrong', 0), ('nominal', -1), ('nominal', True)]:
            with self.assertRaises(ValueError):
                fixture.geometry(mode, tick)

    def test_mapping_tf_recovery_does_not_modify_robot_or_sensor_transforms(self):
        before, after = fixture.static_transforms(False), fixture.static_transforms(True)
        self.assertEqual(before, after[:-1])
        self.assertNotIn('mapping_source', [v['child'] for v in before])
        self.assertEqual(after[-1]['translation'], [1., 2., 3.])
        self.assertEqual(after[-1]['child'], 'mapping_source')
        for value in after:
            self.assertAlmostEqual(sum(q * q for q in value['rotation_xyzw']), 1.)
        self.assertEqual(len({v['child'] for v in after}), len(after))

    def test_import_invalid_arguments_and_rejected_guard_never_import_ros(self):
        original = builtins.__import__
        def no_ros(name, *args, **kwargs):
            if name.split('.')[0] in {'rclpy', 'geometry_msgs', 'nav_msgs', 'sensor_msgs', 'std_msgs', 'tf2_msgs'}:
                self.fail('ROS imported before successful guard')
            return original(name, *args, **kwargs)
        with patch('builtins.__import__', side_effect=no_ros):
            loaded = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(loaded)
            with patch.object(loaded, 'checked_environment') as guard:
                for mode, seconds in [('invalid', 1), ('nominal', 0), ('nominal', 3601)]:
                    with self.assertRaises((ValueError, argparse.ArgumentTypeError)):
                        loaded.publisher('rmw_fastrtps_cpp', Path('unused'), mode, seconds)
                guard.assert_not_called()
            with patch.object(loaded, 'checked_environment', side_effect=SystemExit('guard')):
                with self.assertRaises(SystemExit):
                    loaded.publisher('rmw_fastrtps_cpp', Path('unused'), 'nominal', 1)

    def test_isolation_guard_rejects_remote_mismatched_runtime_and_reused_evidence(self):
        base = dict(ROS_DISTRO='jazzy', ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1', ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST', RMW_IMPLEMENTATION='rmw_fastrtps_cpp')
        evidence = ROOT / 'artifacts/feasibility/evidence/offline-mapping-unused'
        changes = [dict(ROS_DOMAIN_ID='0'), dict(ROS_LOCALHOST_ONLY='0'), dict(ROS_AUTOMATIC_DISCOVERY_RANGE='SUBNET'), dict(ROS_DISTRO='humble'), dict(RMW_IMPLEMENTATION='rmw_cyclonedds_cpp')]
        changes += [{key: 'remote'} for key in ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI', 'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']]
        for change in changes:
            with patch.dict(os.environ, {**base, **change}, clear=True), patch.object(fixture.ROBOT.sys, 'prefix', str(fixture.HELPERS['RUNTIME'])), patch.object(Path, 'exists', return_value=False), patch.object(Path, 'mkdir') as mkdir:
                with self.assertRaises(SystemExit):
                    fixture.checked_environment('rmw_fastrtps_cpp', evidence)
                mkdir.assert_not_called()
        with patch.dict(os.environ, base, clear=True), patch.object(Path, 'exists', return_value=True), self.assertRaises(SystemExit):
            fixture.checked_environment('rmw_fastrtps_cpp', evidence)
        with patch.dict(os.environ, base, clear=True), self.assertRaises(SystemExit):
            fixture.checked_environment('rmw_fastrtps_cpp', Path('/tmp/mapping-outside'))

    def test_cooperative_signal_handlers_latch_first_cause_and_restore(self):
        state = fixture.SignalStop()
        signals = fixture.SENSOR.signal
        old = {signals.SIGINT: object(), signals.SIGTERM: object()}
        with patch.object(signals, 'getsignal', side_effect=lambda n: old[n]), patch.object(signals, 'signal') as register:
            state.install()
            state.handle(signals.SIGTERM, None)
            state.handle(signals.SIGINT, None)
            self.assertTrue(state.requested)
            self.assertEqual(state.signum, signals.SIGTERM)
            state.restore()
            self.assertEqual({c.args[0]: c.args[1] for c in register.call_args_list[2:]}, old)

    def test_cli_requires_explicit_pair_fresh_evidence_and_bounded_time(self):
        parser = fixture.argument_parser()
        args = parser.parse_args(['--rmw', 'rmw_cyclonedds_cpp', '--evidence', 'new', '--mode', 'map-retained'])
        self.assertEqual(args.seconds, 600)
        for flags in [[], ['--rmw', 'bad', '--evidence', 'new'], ['--rmw', 'rmw_fastrtps_cpp', '--evidence', 'new', '--seconds', 'NaN']]:
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                parser.parse_args(flags)


if __name__ == '__main__':
    unittest.main()
