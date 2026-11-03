"""Offline Robot fixture isolation/geometry checks; no rclpy or network required."""
import argparse
import builtins
import contextlib
import importlib.util
import io
import math
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('robot_ros_workflow', ROOT / 'scripts/robot-ros-workflow.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class RobotFixtureTests(unittest.TestCase):
    def test_import_and_invalid_arguments_never_import_ros(self):
        original = builtins.__import__
        def no_ros(name, *args, **kwargs):
            if name.split('.')[0] in {'rclpy', 'geometry_msgs', 'tf2_msgs', 'std_msgs'}:
                self.fail('ROS imported before validated publisher entry')
            return original(name, *args, **kwargs)
        with patch('builtins.__import__', side_effect=no_ros):
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            with patch.object(module, 'checked_environment') as guard:
                for args in [('rmw_fastrtps_cpp', Path('unused'), 'bad', 1),
                             ('rmw_fastrtps_cpp', Path('unused'), 'nominal', 0),
                             ('rmw_fastrtps_cpp', Path('unused'), 'nominal', 3601)]:
                    with self.assertRaises((ValueError, argparse.ArgumentTypeError, SystemExit)):
                        module.publisher(*args)
                guard.assert_not_called()

    def test_cli_requires_explicit_pair_evidence_and_bounded_duration(self):
        parser = fixture.argument_parser()
        flags = parser.parse_args(['--rmw', 'rmw_cyclonedds_cpp', '--evidence', 'new', '--mode', 'recover'])
        self.assertEqual(flags.seconds, 600)
        self.assertEqual(flags.mode, 'recover')
        for args in [[], ['--evidence', 'new'], ['--rmw', 'rmw_fastrtps_cpp'],
                     ['--rmw', 'bad', '--evidence', 'new'],
                     ['--rmw', 'rmw_fastrtps_cpp', '--evidence', 'new', '--seconds', 'NaN'],
                     ['--rmw', 'rmw_fastrtps_cpp', '--evidence', 'new', '--seconds', '1.5']]:
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                parser.parse_args(args)
        for value in [0, 3601, float('nan'), True, 1.5]:
            with self.assertRaises(argparse.ArgumentTypeError):
                fixture.bounded_seconds(value)

    def test_existing_environment_guard_rejects_every_remote_setting(self):
        base = {'ROS_DISTRO': 'jazzy', 'ROS_DOMAIN_ID': '187', 'ROS_LOCALHOST_ONLY': '1',
                'ROS_AUTOMATIC_DISCOVERY_RANGE': 'LOCALHOST', 'RMW_IMPLEMENTATION': 'rmw_fastrtps_cpp'}
        evidence = ROOT / 'artifacts/feasibility/evidence/offline-robot-unused'
        cases = [('domain', {'ROS_DOMAIN_ID': '0'}), ('localhost', {'ROS_LOCALHOST_ONLY': '0'}),
                 ('range', {'ROS_AUTOMATIC_DISCOVERY_RANGE': 'SUBNET'}),
                 ('rmw', {'RMW_IMPLEMENTATION': 'rmw_cyclonedds_cpp'}), ('distro', {'ROS_DISTRO': 'humble'})]
        cases.extend((key, {key: 'remote-or-custom'}) for key in [
            'ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI',
            'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE'])
        for label, change in cases:
            with self.subTest(case=label), patch.dict(os.environ, {**base, **change}, clear=True), \
                    patch.object(fixture.sys, 'prefix', str(fixture.HELPERS['RUNTIME'])), \
                    patch.object(Path, 'exists', return_value=False), patch.object(Path, 'mkdir') as mkdir:
                with self.assertRaises(SystemExit):
                    fixture.checked_environment('rmw_fastrtps_cpp', evidence)
                mkdir.assert_not_called()
        with patch.dict(os.environ, base, clear=True), self.assertRaises(SystemExit):
            fixture.checked_environment('rmw_fastrtps_cpp', Path('/tmp/unsafe-robot-evidence'))
        with patch.dict(os.environ, base, clear=True), patch.object(Path, 'exists', return_value=True), self.assertRaises(SystemExit):
            fixture.checked_environment('rmw_fastrtps_cpp', evidence)
        with patch.dict(os.environ, base, clear=True), \
                patch.object(fixture.sys, 'prefix', str(fixture.HELPERS['RUNTIME'])), \
                patch.object(Path, 'exists', return_value=False), patch.object(Path, 'mkdir'):
            self.assertEqual(fixture.checked_environment('rmw_fastrtps_cpp', evidence), evidence.resolve())

    def test_modes_only_release_tip_once_and_never_pretend_to_remove_tf(self):
        for mode in fixture.MODES:
            state = fixture.ModeState(mode)
            self.assertEqual(state.tip_released, mode == 'nominal')
            self.assertFalse(state.advance('waiting'))
            self.assertEqual(state.advance('ready'), mode == 'recover')
            self.assertFalse(state.advance('ready'))
            self.assertFalse(state.advance('waiting'))
            self.assertEqual(state.tip_released, mode != 'missing')
            with self.assertRaises(ValueError):
                state.advance('malformed')

    def test_source_geometry_matches_original_description_and_tip_analytic_center(self):
        description = ET.parse(ROOT / 'fixtures/robot_visualizer/articulated.urdf').getroot()
        self.assertEqual(tuple(link.attrib['name'] for link in description.findall('link')), fixture.LINKS)
        nominal = fixture.static_transforms(True) + [fixture.dynamic_transform(0)]
        self.assertEqual({v['child'] for v in nominal}, set(fixture.LINKS))
        self.assertEqual(len(nominal), 5)
        self.assertEqual(len(fixture.static_transforms(False)), 3)
        self.assertNotIn('tip', {v['child'] for v in fixture.static_transforms(False)})
        for value in nominal:
            self.assertAlmostEqual(sum(v*v for v in value['rotation_xyzw']), 1)
            self.assertTrue(all(math.isfinite(v) for v in value['translation']))
        for actual, expected in zip(fixture.expected_tip_center(0), [0, 0, .85]):
            self.assertAlmostEqual(actual, expected)
        right_angle = fixture.expected_tip_center(math.pi/2)
        self.assertAlmostEqual(right_angle[0], .65)
        self.assertAlmostEqual(right_angle[2], .2)
        moving = fixture.dynamic_transform(math.pi)
        self.assertAlmostEqual(moving['rotation_xyzw'][1], math.sin(.4))
        self.assertAlmostEqual(moving['rotation_xyzw'][3], math.cos(.4))
        self.assertEqual(moving['translation'], [0, 0, .2])
        for elapsed in [float('nan'), float('inf'), -1]:
            with self.assertRaises(ValueError):
                fixture.dynamic_transform(elapsed)

    def test_control_is_bounded_regular_and_nonsymlink(self):
        with tempfile.TemporaryDirectory(prefix='studio-robot-control-') as temp:
            path = Path(temp) / 'mode.txt'
            self.assertEqual(fixture.read_control(path), 'waiting')
            path.write_text('ready\n')
            self.assertEqual(fixture.read_control(path), 'ready')
            for value in ['x' * 65, 'missing', '']:
                path.write_text(value)
                with self.assertRaises(ValueError):
                    fixture.read_control(path)
            path.unlink()
            if hasattr(os, 'mkfifo'):
                os.mkfifo(path)
                with self.assertRaises(ValueError):
                    fixture.read_control(path)
                path.unlink()
            target = Path(temp) / 'target'
            target.write_text('ready')
            path.symlink_to(target)
            with self.assertRaises(ValueError):
                fixture.read_control(path)

    def test_deadline_helper_never_catches_up_in_a_burst(self):
        schedule = fixture.HELPERS['PublishSchedule'](None, 0)
        self.assertEqual(schedule.due(0), (True, False))
        self.assertEqual(schedule.due(100), (True, False))
        self.assertEqual(schedule.due(100), (False, False))
        self.assertLessEqual(schedule.wait(100), .2 + 1e-12)


if __name__ == '__main__':
    unittest.main()
