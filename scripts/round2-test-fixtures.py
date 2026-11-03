"""Offline protocol fixture contracts and ROS-isolation rejection checks; no peers."""
import contextlib
import io
import shlex
import importlib.util
import json
import os
from pathlib import Path
import selectors
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


fixture = load('fixture', 'ui-workspace-fixture.py')
workflow = load('workflow', 'ui-ros-workflow.py')
boundary = load('boundary', 'round2-boundary-check.py')


class Contracts(unittest.TestCase):
    def test_typed_nested_and_exact_edges(self):
        event = fixture.typed_event('/demo/odometry', 'nav_msgs/msg/Odometry', 10)
        self.assertEqual(event['sample']['contract_version'], 2)
        self.assertIsNone(event['sample']['source_time'])
        self.assertEqual(boundary.member(event['sample']['value'], 'pose', 'covariance')['original_items'], 36)
        edge = fixture.typed_event('/demo/numeric_edges', 'fixture/msg/NumericEdges', 1)['sample']['value']
        self.assertEqual(boundary.member(edge, 'exact_u64')['value'], '18446744073709551615')
        self.assertEqual(boundary.member(edge, 'exact_i64')['value'], '-9223372036854775808')
        self.assertEqual(boundary.member(edge, 'samples')['original_items'], 4)
        self.assertEqual(len(boundary.member(edge, 'samples')['items']), 2)
        self.assertEqual(boundary.member(edge, 'numeric_looking_text')['kind'], 'text')
        self.assertEqual(boundary.member(edge, 'nested', 'literal.field[0]')['value'], '42')
        json.dumps(edge, allow_nan=False)

    def launch(self, mode, arguments):
        with tempfile.TemporaryDirectory(dir=ROOT / 'artifacts') as temporary:
            environment = {**os.environ, 'STUDIO_UI_FIXTURE_MODE': mode, 'STUDIO_UI_FIXTURE_EVIDENCE': temporary}
            process = subprocess.Popen([sys.executable, str(ROOT / 'scripts/ui-workspace-fixture.py'), *arguments],
                                       env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0)
            selector = selectors.DefaultSelector()
            selector.register(process.stdout, selectors.EVENT_READ)
            try:
                events = []
                while len(events) < (3 if arguments and mode == 'normal' else 2):
                    self.assertTrue(selector.select(3), 'fixture output timeout')
                    events.append(json.loads(process.stdout.readline()))
                return events
            finally:
                selector.close()
                process.terminate()
                process.communicate(timeout=3)
                self.assertIsNotNone(process.returncode)

    def test_graph_only_never_activates_data(self):
        events = self.launch('normal', [])
        self.assertEqual([e['event'] for e in events], ['hello', 'graph'])
        self.assertEqual(events[0]['version'], 2)
        self.assertIn(['/demo/odometry', ['nav_msgs/msg/Odometry']], events[1]['topics'])

    def test_explicit_subscription_is_typed(self):
        events = self.launch('normal', ['/demo/odometry', 'nav_msgs/msg/Odometry'])
        self.assertEqual(events[2]['event'], 'typed_sample')
        self.assertEqual(events[2]['sample']['topic'], '/demo/odometry')

    def test_empty_and_incompatible_modes(self):
        self.assertEqual(self.launch('empty', [])[1]['topics'], [])
        self.assertEqual(self.launch('incompatible', [])[0]['version'], 1)
        self.assertEqual(self.launch('error', [])[1]['event'], 'error')

    def test_boundary_partial_line_timeout_reaps_child(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'artifacts') as temporary:
            path = Path(temporary)
            events = boundary.inspect_backend(Path(sys.executable),
                ['-u', '-c', 'import sys,time;sys.stdout.write("partial");sys.stdout.flush();time.sleep(10)'],
                path / 'partial', seconds=0.15)
            self.assertEqual(events, [])
            records = [json.loads(line) for line in (path / 'children.jsonl').read_text().splitlines()]
            self.assertEqual(records[-1]['event'], 'reaped')
            self.assertEqual(records[0]['pid'], records[-1]['pid'])
            self.assertEqual((path / 'partial.stdout.jsonl').read_bytes(), b'partial')

    def test_report_command_matches_actual_recorded_child_and_isolation(self):
        # Exercise main's reporting closure after topic setup. A deliberately
        # non-ROS backend may fail data assertions; metadata must stay exact.
        with tempfile.TemporaryDirectory(dir=ROOT / 'artifacts') as temporary:
            root = Path(temporary)
            base = root / 'artifacts/feasibility/evidence'
            base.mkdir(parents=True)
            for name in ['Cargo.lock', 'pixi.lock']:
                (root / name).write_text('offline fixture lock')
            environment = {'ROS_DISTRO': 'jazzy', 'ROS_DOMAIN_ID': '187',
                'ROS_LOCALHOST_ONLY': '1', 'ROS_AUTOMATIC_DISCOVERY_RANGE': 'LOCALHOST',
                'RMW_IMPLEMENTATION': 'rmw_cyclonedds_cpp', 'STUDIO_UI_ROS_EVIDENCE': str(base)}
            argv = ['round2-boundary-check.py', '--label', 'command-metadata',
                    '--backend', sys.executable, '--timeout', '0.01',
                    '--interface-bundle', str(root / 'bundle with spaces')]
            with patch.object(boundary, 'ROOT', root), patch.object(boundary.workflow, 'checked_environment', return_value=base), \
                 patch.dict(os.environ, environment, clear=True), patch.object(sys, 'argv', argv), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(boundary.main(), 1)
            records = [json.loads(line) for line in (base / 'command-metadata/children.jsonl').read_text().splitlines()]
            children = [record['argv'] for record in records if 'argv' in record]
            rows = json.loads((root / 'artifacts/feasibility/command-metadata.json').read_text())
            self.assertEqual(len(rows), len(children))
            unsafe = {'ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI',
                      'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE'}
            for row, child_argv in zip(rows, children):
                command = shlex.split(row['command'])
                self.assertEqual(command.pop(0), 'env')
                unset = set()
                while command[0] == '-u':
                    command.pop(0)
                    unset.add(command.pop(0))
                self.assertEqual(unset, unsafe)
                logged = {}
                while '=' in command[0]:
                    key, value = command.pop(0).split('=', 1)
                    logged[key] = value
                self.assertEqual(logged, environment)
                self.assertEqual(command.pop(0), 'scripts/dev')
                self.assertEqual(command, child_argv)

    def test_rejects_unsafe_environment_before_ros_imports(self):
        base = {'ROS_DISTRO': 'jazzy', 'ROS_DOMAIN_ID': '187', 'ROS_LOCALHOST_ONLY': '1',
                'ROS_AUTOMATIC_DISCOVERY_RANGE': 'LOCALHOST', 'RMW_IMPLEMENTATION': 'rmw_fastrtps_cpp',
                'STUDIO_UI_ROS_EVIDENCE': str(ROOT / 'artifacts/feasibility/evidence/round2-guard')}
        cases = [{'ROS_DOMAIN_ID': '0'}, {'ROS_LOCALHOST_ONLY': '0'}, {'ROS_AUTOMATIC_DISCOVERY_RANGE': 'SUBNET'},
                 {'RMW_IMPLEMENTATION': 'unknown'}, {'STUDIO_UI_ROS_EVIDENCE': '/tmp/elsewhere'}]
        cases += [{key: 'remote/custom'} for key in ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT',
                   'CYCLONEDDS_URI', 'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']]
        for changes in cases:
            with self.subTest(changes=changes), patch.dict(os.environ, {**base, **changes}, clear=True):
                with self.assertRaises(SystemExit):
                    workflow.checked_environment()


if __name__ == '__main__':
    (ROOT / 'artifacts').mkdir(exist_ok=True)
    unittest.main()
