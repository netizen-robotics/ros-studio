"""Offline subprocess acceptance for explicit persistent UI session fixtures.

Run with scripts/dev .pixi/envs/default/bin/python scripts/test-ui-session-fixture.py.
No ROS/native-window/network use. Every subprocess is closed and reaped.
"""
import json
from session_wire import VERSION as SESSION_VERSION, CAMERA_CAPABILITIES
from marker_wire import CAPABILITIES as MARKER_CAPABILITIES
from pose_wire import CAPABILITIES as POSE_CAPABILITIES
from cloud_wire import decode_points
import base64
import hashlib
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / 'scripts/ui-workspace-fixture.py'


class Fixture:
    def __init__(self, mode='normal', arguments=None):
        self.directory = tempfile.TemporaryDirectory(prefix='studio-session-fixture-')
        environment = {**os.environ, 'STUDIO_UI_FIXTURE_MODE': mode,
                       'STUDIO_UI_FIXTURE_EVIDENCE': self.directory.name}
        self.process = subprocess.Popen([sys.executable, str(FIXTURE), *(arguments if arguments is not None else ['--observe-session'])],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=environment)
        self.events = queue.Queue(maxsize=4096)
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        for line in self.process.stdout:
            assert len(line) <= 1024 * 1024
            self.events.put(json.loads(line))

    def send(self, value):
        self.process.stdin.write(json.dumps(value).encode() + b'\n')
        self.process.stdin.flush()

    def next(self, kind=None, subscription=None, timeout=3):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            event = self.events.get(timeout=max(0.001, deadline - time.monotonic()))
            if (kind is None or event['event'] == kind) and (subscription is None or event.get('subscription') == subscription):
                return event
        raise AssertionError('Missing fixture event')

    def ready(self):
        ready = self.next('ready')
        assert ready == {'event': 'ready', 'version': SESSION_VERSION, 'camera_snapshot': CAMERA_CAPABILITIES, 'pose_snapshot': POSE_CAPABILITIES, 'marker_snapshot': MARKER_CAPABILITIES, 'distro': 'jazzy', 'rmw': 'synthetic-fixture-no-ROS',
                         'max_subscriptions': 32, 'max_sample_bytes': 512 * 1024, 'max_command_bytes': 128 * 1024,
                         'image_contract_version': 2, 'max_image_raw_bytes': 16777216, 'max_image_sample_bytes': 25165824,
                         'spatial_contract_version': 1, 'max_native_handles': 64, 'max_spatial_scan_rays': 2048,
                         'max_tf_frames': 256, 'max_tf_edges': 128, 'max_tf_dynamic_inserts': 256,
                         'tf_cache_window_ns': 10000000000, 'spatial_pending_timeout_ms': 1000, 'spatial_retry_interval_ms': 50,
                         'robot_contract_version': 1, 'max_robot_links': 128, 'max_robot_hz': 10, 'max_robot_sample_bytes': 512 * 1024,
                         'shared_scene_contract_version': 1, 'max_sensor_layers': 4, 'point_cloud_contract_version': 2,
                         'max_cloud_source_points': 8192, 'max_cloud_raw_bytes': 262144, 'max_cloud_sample_bytes': 524288,
          'map_snapshot': {'contract_version': 2, 'max_dimension': 2048, 'max_cells': 4194304, 'max_chunk_bytes': 196608, 'max_chunks': 22, 'max_sample_bytes': 524288, 'max_layers': 3, 'max_update_readers_per_map': 1},
          'robot_description': {'contract_version': 1, 'max_document_bytes': 1048576, 'max_chunk_bytes': 196608, 'max_chunks': 6, 'max_owners': 1},
          'tf_graph': {'contract_version': 1, 'max_hz': 5, 'max_sample_bytes': 524288, 'max_owners': 1},
          'mapping_contract_version': 1, 'max_map_dimension': 256, 'max_map_cells': 65536,
          'max_path_poses': 1024, 'max_mapping_sample_bytes': 524288,
          'max_map_layers': 3, 'max_pose_layers': 1, 'max_path_layers': 1}
        return self.next('graph')

    def close(self):
        if not self.process.stdin.closed:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        # Unblock a reader waiting to enqueue flood events before closing its pipe.
        while self.reader.is_alive():
            try:
                self.events.get(timeout=0.05)
            except queue.Empty:
                pass
            self.reader.join(timeout=0.01)
        self.process.stdout.close()
        self.process.stderr.close()
        self.directory.cleanup()


def start(request, subscription, topic='/demo/status', message_type='std_msgs/msg/String', **qos):
    return {'command': 'start', 'generation': 7, 'request': request, 'subscription': subscription,
            'configuration': {'topic': topic, 'message_type': message_type, 'best_effort': qos.get('best_effort', True),
                              'transient_local': qos.get('transient_local', False),
                              'output': qos.get('output', 'typed'), 'spatial': qos.get('spatial'), 'robot': qos.get('robot')}}


def spatial_start(request, subscription, **settings):
    return start(request, subscription, settings.get('topic', '/demo/scan'), 'sensor_msgs/msg/LaserScan', output='spatial_scan',
                 spatial={'fixed_frame': settings.get('fixed_frame', 'map'), 'clock_policy': settings.get('clock_policy', 'verified_system'),
                          'dynamic_tf_topic': settings.get('dynamic_tf_topic', '/tf'), 'dynamic_tf_best_effort': settings.get('dynamic_tf_best_effort', False),
                          'static_tf_topic': settings.get('static_tf_topic', '/tf_static')})


def cloud_start(request, subscription, **settings):
    command = spatial_start(request, subscription, **settings)
    command['configuration'].update(topic=settings.get('topic', '/demo/cloud'),
                                    message_type='sensor_msgs/msg/PointCloud2', output='point_cloud')
    return command


def tf_graph_start(request, subscription, **settings):
    command = spatial_start(request, subscription, **settings)
    scene = command['configuration']['spatial']
    command['configuration'].update(output='tf_graph', topic=scene['dynamic_tf_topic'],
                                    message_type='tf2_msgs/msg/TFMessage', best_effort=scene['dynamic_tf_best_effort'])
    return command


def stop(request, subscription):
    return {'command': 'stop', 'generation': 7, 'request': request, 'subscription': subscription}


def robot_start(request, subscription, **settings):
    robot = dict(fixed_frame='map', frames=['base', 'arm', 'tip', 'ascii_mesh', 'binary_mesh'],
                 clock_policy='verified_system', dynamic_tf_topic='/tf', dynamic_tf_best_effort=True,
                 static_tf_topic='/tf_static')
    robot.update(settings)
    return start(request, subscription, robot['dynamic_tf_topic'], 'tf2_msgs/msg/TFMessage',
                 output='robot', best_effort=robot['dynamic_tf_best_effort'], robot=robot)


def mapping_start(request, subscription, kind='occupancy_grid', **settings):
    command = spatial_start(request, subscription, **settings)
    types = {'occupancy_grid':'nav_msgs/msg/OccupancyGrid', 'pose':'geometry_msgs/msg/PoseStamped', 'path':'nav_msgs/msg/Path'}
    command['configuration'].update(output=kind, message_type=types[kind],
                                    topic=settings.get('topic', '/demo/' + ('map' if kind == 'occupancy_grid' else kind)),
                                    best_effort=False, transient_local=kind == 'occupancy_grid')
    return command


def snapshot_start(request, subscription):
    command = spatial_start(request, subscription)
    command['configuration'].update(topic='/demo/map', message_type='nav_msgs/msg/OccupancyGrid',
                                    output='map_snapshot', best_effort=False, transient_local=True)
    return command


class SessionFixtureTests(unittest.TestCase):
    def test_scene_frame_inventory_and_read_only_refresh_preserve_stream(self):
        with patch.dict(os.environ, {'STUDIO_UI_FIXTURE_SCENE_FRAMES': '1'}):
            fixture = Fixture()
        self.addCleanup(fixture.close)
        fixture.ready()
        fixture.send(cloud_start(1, 1))
        self.assertIn('cloud', fixture.next('scene_frames')['frames'])
        fixture.next('started', 1)
        fixture.next('point_cloud_observation', 1)
        fixture.send(dict(command='tool', generation=7, request=50, subscription=50,
                          operation=dict(kind='graph', section='Topics', node='')))
        response = fixture.next('tool', 50)
        self.assertEqual(response['update']['state'], 'graph')
        self.assertIsInstance(response['update']['value'], dict)
        fixture.next('point_cloud_observation', 1)
        fixture.send(stop(2, 1))
        fixture.next('stopped', 1)
        self.assertEqual(fixture.next('scene_frames')['frames'], [])

    def test_pose_sources_clear_and_kind_admission_preserve_peer(self):
        fixture = Fixture()
        self.addCleanup(fixture.close)
        fixture.ready()
        def pose(request, subscription, kind):
            ty = 'nav_msgs/msg/Odometry' if kind == 'odometry' else 'geometry_msgs/msg/PoseWithCovarianceStamped'
            command = start(request, subscription, '/demo/' + kind, ty)
            command['configuration'].update(output={'pose_snapshot': {'kind': kind, 'history':
                {'keep': 100, 'position_tolerance_um': 100000, 'angle_tolerance_urad': 100000} if kind == 'odometry' else None}},
                spatial={'fixed_frame': 'world', 'clock_policy': 'verified_system', 'dynamic_tf_topic': '/tf',
                         'dynamic_tf_best_effort': False, 'static_tf_topic': '/tf_static'})
            return command
        for request, kind in enumerate(('pose_with_covariance', 'odometry'), 1):
            fixture.send(pose(request, request, kind))
            fixture.next('started', request)
            sample = fixture.next('pose_snapshot_observation', request)['sample']['sample']
            self.assertEqual(sample['owner']['kind'], kind)
            self.assertEqual(len(sample['latest']['covariance']), 36)
        fixture.send(pose(3, 3, 'odometry'))
        self.assertTrue(fixture.next('request_error', 3)['no_handle'])
        pause = Path(fixture.directory.name) / 'pause-poses'
        pause.write_text('pause')
        fixture.send(dict(command='clear_pose', generation=7, request=4, subscription=2))
        ack = fixture.next('pose_cleared', 2)
        self.assertEqual(ack['barrier']['continuity_epoch'], 2)
        self.assertEqual(ack['barrier']['reason'], 'local_clear')
        pause.unlink()
        next_pose = fixture.next('pose_snapshot_observation', 2)['sample']['sample']
        self.assertEqual(next_pose['continuity_epoch'], 2)
        self.assertEqual(fixture.next('pose_snapshot_observation', 1)['sample']['sample']['continuity_epoch'], 1)
        fixture.send(dict(command='stop', generation=7, request=5, subscription=2))
        fixture.next('stopped', 2)
        fixture.send(pose(6, 4, 'odometry'))
        fixture.next('started', 4)
        self.assertEqual(fixture.next('pose_snapshot_observation', 4)['sample']['sample']['owner']['kind'], 'odometry')

    def test_map_cancel_before_start_ack_has_no_late_ack_or_sample(self):
        fixture = self.fixture()
        hold = Path(fixture.directory.name) / 'pause-map-start'
        hold.touch()
        fixture.ready()
        fixture.send(snapshot_start(1, 1))
        fixture.send(start(2, 2, '/demo/camera/rgb8', 'sensor_msgs/msg/Image', output='compact_image'))
        while True:
            event = fixture.next()
            self.assertNotEqual(event.get('subscription'), 1)
            if event['event'] == 'image_observation':
                break
        fixture.send(stop(3, 1))
        fixture.next('stopped', 1)
        hold.unlink()
        fixture.send(snapshot_start(4, 3))
        while True:
            event = fixture.next()
            self.assertNotEqual(event.get('subscription'), 1)
            if event['event'] == 'map_snapshot_observation':
                self.assertEqual(event['subscription'], 3)
                break

    def test_paused_map_stop_has_no_late_delivery_and_peer_survives(self):
        fixture = self.fixture('mapping-large')
        pause = Path(fixture.directory.name) / 'pause-map'
        pause.touch()
        fixture.ready()
        fixture.send(snapshot_start(1, 1))
        fixture.next('started', 1)
        fixture.send(start(2, 2, '/demo/camera/rgb8', 'sensor_msgs/msg/Image', output='compact_image'))
        fixture.next('image_observation', 2)
        fixture.send(stop(3, 1))
        fixture.next('stopped', 1)
        pause.unlink()
        fixture.send(snapshot_start(4, 3))
        while True:
            event = fixture.next()
            self.assertFalse(event.get('subscription') == 1 and event['event'] == 'map_snapshot_observation')
            if event['event'] == 'map_snapshot_observation':
                self.assertEqual((event['subscription'], event['sample']['index']), (3, 0))
                break
        fixture.next('image_observation', 2)

    def test_map_failure_is_scoped_and_removal_requires_explicit_recovery(self):
        fixture = self.fixture('mapping-large')
        pause = Path(fixture.directory.name) / 'pause-map'
        fault = Path(fixture.directory.name) / 'fail-map'
        pause.touch()
        fixture.ready()
        fixture.send(snapshot_start(1, 1))
        fixture.next('started', 1)
        fixture.send(start(2, 2, '/demo/camera/rgb8', 'sensor_msgs/msg/Image', output='compact_image'))
        fixture.next('image_observation', 2)
        fault.touch()
        self.assertIn('only this fixture owner', fixture.next('stream_error', 1)['message'])
        fault.unlink()
        pause.unlink()
        fixture.send(snapshot_start(3, 3))
        while True:
            event = fixture.next()
            self.assertFalse(event.get('subscription') == 1 and event['event'] == 'map_snapshot_observation')
            if event['event'] == 'map_snapshot_observation':
                self.assertEqual((event['subscription'], event['sample']['index']), (3, 0))
                break
        fixture.next('image_observation', 2)

    def test_map_snapshot_maximum_chunks_unknown_continuity_and_exact_stop(self):
        fixture = self.fixture('mapping-large')
        fixture.ready()
        command = spatial_start(1, 1)
        command['configuration'].update(topic='/demo/map', message_type='nav_msgs/msg/OccupancyGrid',
                                        output='map_snapshot', best_effort=False, transient_local=True,
                                        map_updates=dict(topic='/demo/map_updates', best_effort=False, transient_local=False))
        fixture.send(command)
        fixture.next('started', 1)
        raw = bytearray()
        header = None
        for index in range(22):
            event = fixture.next('map_snapshot_observation', 1)
            chunk = event['sample']
            self.assertEqual((chunk['kind'], chunk['index'], chunk['count']), ('chunk', index, 22))
            self.assertEqual(chunk['header']['source_status']['continuity'], 'unknown')
            if header is None:
                header = chunk['header']
            self.assertEqual(header, chunk['header'])
            raw.extend(base64.b64decode(chunk['data_base64'], validate=True))
        self.assertEqual(len(raw), 4194304)
        self.assertEqual(hashlib.sha256(raw).hexdigest(), header['sha256'])
        self.assertEqual(header['contract_version'], 2)
        # Large fixture landmarks must survive a fit-to-window view and retain
        # row-major lower-left origin: bottom occupied, top half occupied.
        for x, y, value in [(16, 1024, 100), (512, 16, 100), (512, 2030, 50),
                            (2030, 1024, 255), (1040, 1024, 99), (512, 1024, 0)]:
            self.assertEqual(raw[y * 2048 + x], value)
        fixture.send(stop(2, 1))
        self.assertEqual(fixture.next('stopped', 1)['subscription'], 1)

    def test_map_repeat_has_complete_distinct_revisions_and_stops(self):
        fixture = self.fixture('mapping-large')
        fixture.ready()
        Path(fixture.directory.name, 'map-repeat').touch()
        command = spatial_start(1, 1)
        command['configuration'].update(topic='/demo/map', message_type='nav_msgs/msg/OccupancyGrid',
                                        output='map_snapshot', best_effort=False, transient_local=True)
        fixture.send(command)
        fixture.next('started', 1)
        headers = []
        for frame in range(3):
            raw = bytearray()
            header = None
            for index in range(22):
                chunk = fixture.next('map_snapshot_observation', 1)['sample']
                self.assertEqual((chunk['index'], chunk['count']), (index, 22))
                header = header or chunk['header']
                self.assertEqual(header, chunk['header'])
                raw.extend(base64.b64decode(chunk['data_base64'], validate=True))
            self.assertEqual(len(raw), 4194304)
            self.assertEqual(hashlib.sha256(raw).hexdigest(), header['sha256'])
            self.assertEqual(header['assembly_counters']['full_maps'], frame + 1)
            headers.append(header)
        self.assertEqual([h['revision'] for h in headers], [1, 2, 3])
        self.assertTrue(all(a['metadata']['stamp_ns'] < b['metadata']['stamp_ns']
                            for a, b in zip(headers, headers[1:])))
        fixture.send(stop(2, 1))
        fixture.next('stopped', 1)
        fixture.send(spatial_start(3, 2))
        fixture.next('started', 2)
        # Another owner provides a progress barrier after Stop; no late Map data.
        while True:
            event = fixture.next()
            self.assertFalse(event.get('subscription') == 1 and event['event'] == 'map_snapshot_observation')
            if event.get('subscription') == 2 and event['event'] == 'spatial_observation':
                break

    def test_marker_snapshot_clear_is_correlated_and_does_not_resubscribe_or_stop_peer(self):
        fixture = self.fixture()
        fixture.ready()
        command = spatial_start(1, 1)
        command['configuration'].update(topic='/demo/markers', message_type='visualization_msgs/msg/Marker', output='marker_snapshot')
        fixture.send(command)
        fixture.next('started', 1)
        sample = fixture.next('marker_snapshot_observation', 1)['sample']
        raw = base64.b64decode(sample['data_base64'], validate=True)
        self.assertEqual(hashlib.sha256(raw).hexdigest(), sample['header']['sha256'])
        self.assertEqual(json.loads(raw)['entries'][0]['marker']['geometry'], {'kind': 'cube'})
        fixture.send(start(2, 2))
        fixture.next('started', 2)
        fixture.send(dict(command='clear_markers', generation=7, request=3, subscription=1))
        self.assertEqual(fixture.next('markers_cleared', 1)['request'], 3)
        clear = fixture.next('marker_snapshot_observation', 1)['sample']
        self.assertEqual((clear['kind'], clear['reason']), ('clear', 'local_clear'))
        self.assertGreater(clear['revision'], sample['header']['revision'])
        fixture.next('observation', 2)
        fixture.send(dict(command='clear_markers', generation=7, request=4, subscription=2))
        rejected = fixture.next('request_error', 2)
        self.assertFalse(rejected['no_handle'])
        fixture.next('observation', 2)
        fixture.send(stop(5, 1))
        fixture.next('stopped', 1)

    def test_maximum_marker_chunks_repeat_with_exact_owner_budgets_and_stop(self):
        fixture = self.fixture()
        fixture.ready()
        root = Path(fixture.directory.name)
        (root / 'marker-scene').write_text('maximum')
        (root / 'marker-repeat').touch()
        def decode(subscription):
            first = fixture.next('marker_snapshot_observation', subscription)['sample']
            chunks = [first]
            self.assertGreater(first['count'], 1)
            for index in range(1, first['count']):
                part = fixture.next('marker_snapshot_observation', subscription)['sample']
                self.assertEqual(part['index'], index)
                self.assertEqual(part['header'], first['header'])
                chunks.append(part)
            raw = b''.join(base64.b64decode(c['data_base64'], validate=True) for c in chunks)
            self.assertTrue(all(len(base64.b64decode(c['data_base64'])) <= 196608 for c in chunks))
            self.assertEqual(len(raw), first['header']['document_bytes'])
            self.assertEqual(hashlib.sha256(raw).hexdigest(), first['header']['sha256'])
            return first['header'], json.loads(raw)
        for subscription, topic, ty in [(1,'/demo/markers','visualization_msgs/msg/Marker'),
                                       (2,'/demo/marker_array','visualization_msgs/msg/MarkerArray')]:
            command = spatial_start(subscription*2-1, subscription)
            command['configuration'].update(topic=topic, message_type=ty, output='marker_snapshot')
            fixture.send(command)
            fixture.next('started', subscription)
            header, state = decode(subscription)
            entries = state['entries']
            self.assertEqual(len(entries),256)
            self.assertEqual(sum(len(e['marker']['geometry'].get('points',[])) for e in entries),65536)
            self.assertEqual(sum(len(e['marker']['geometry'].get('text','')) for e in entries),65536)
            self.assertEqual(sum(e['marker']['geometry']['kind']=='mesh' for e in entries),int(subscription==1))
            self.assertTrue(all(e['version']==1 for e in entries))
            newer, _ = decode(subscription)
            self.assertGreater(newer['revision'], header['revision'])
            fixture.send(stop(subscription*2, subscription))
            fixture.next('stopped',subscription)

    def test_marker_rich_controls_have_distinct_revisions_and_keep_peers_alive(self):
        fixture = self.fixture()
        fixture.ready()
        root = Path(fixture.directory.name)
        (root / 'marker-scene').write_text('rich')
        command = spatial_start(1, 1)
        command['configuration'].update(topic='/demo/markers', message_type='visualization_msgs/msg/Marker', output='marker_snapshot')
        fixture.send(command)
        fixture.next('started', 1)
        def decode():
            sample = fixture.next('marker_snapshot_observation', 1)['sample']
            raw = base64.b64decode(sample['data_base64'], validate=True)
            self.assertEqual(hashlib.sha256(raw).hexdigest(), sample['header']['sha256'])
            return sample['header'], json.loads(raw)
        header, state = decode()
        self.assertEqual(len(state['entries']), 14)
        keys=[(e['key']['namespace'],e['key']['id']) for e in header['manifest']]
        self.assertEqual(keys, sorted(set(keys)))
        self.assertEqual(len({e['marker']['geometry']['kind'] for e in state['entries']}), 12)
        fixture.send(start(2, 2))
        fixture.next('started', 2)
        for profile in ('missing', 'recovered', 'finite'):
            (root / 'marker-scene.tmp').write_text(profile)
            (root / 'marker-scene.tmp').replace(root / 'marker-scene')
            newer, state = decode()
            self.assertGreater(newer['revision'], header['revision'])
            sphere = next(e for e in state['entries'] if e['marker']['key']['id']==2)
            self.assertEqual(sphere['transform']['kind'], 'pending' if profile=='missing' else 'resolved')
            if profile=='finite':
                life=next(e['lifetime'] for e in newer['manifest'] if e['key']['id']==1)
                self.assertEqual(life['kind'], 'expires')
                self.assertEqual(life['at_unix_ns'],newer['captured_at_unix_ns']+life['remaining_ns'])
            header=newer
            fixture.next('observation',2)
        (root / 'fail-markers').touch()
        self.assertIn('Synthetic Marker',fixture.next('stream_error',1)['message'])
        fixture.next('observation',2)

    def test_map_snapshot_optional_reader_capacity_and_legacy_map_exclusion(self):
        fixture = self.fixture()
        fixture.ready()
        command = spatial_start(1, 1)
        command['configuration'].update(topic='/demo/map', message_type='nav_msgs/msg/OccupancyGrid', output='map_snapshot',
                                        map_updates=dict(topic='/demo/map_updates', best_effort=False, transient_local=False))
        fixture.send(command)
        fixture.next('started', 1)
        for index in range(2, 6):
            fixture.send(start(index, index))
            fixture.next('started', index)
        fixture.send(start(6, 6))
        fixture.next('started', 6)
        legacy = spatial_start(7, 7)
        legacy['configuration'].update(topic='/demo/map', message_type='nav_msgs/msg/OccupancyGrid', output='occupancy_grid')
        fixture.send(legacy)
        fixture.next('started', 7)

    def test_map_snapshot_tf_delay_uses_clear_then_new_revision(self):
        fixture = self.fixture('mapping-delay')
        fixture.ready()
        command = spatial_start(1, 1)
        command['configuration'].update(topic='/demo/map', message_type='nav_msgs/msg/OccupancyGrid', output='map_snapshot')
        fixture.send(command)
        fixture.next('started', 1)
        first = fixture.next('map_snapshot_observation', 1)['sample']
        self.assertEqual((first['kind'], first['reason']), ('clear', 'missing_transform'))
        self.assertIsNotNone(first['metadata'])
        ready = None
        for _ in range(4):
            sample = fixture.next('map_snapshot_observation', 1)['sample']
            if sample['kind'] == 'chunk':
                ready = sample
                break
        self.assertIsNotNone(ready)
        self.assertGreater(ready['header']['revision'], first['revision'])
        self.assertEqual(ready['header']['metadata'], first['metadata'])

    def test_description_capture_max_chunks_single_owner_and_exact_stop(self):
        fixture = self.fixture('description-max')
        fixture.ready()
        command = start(1, 1, '/demo/robot_description', output='robot_description', best_effort=False, transient_local=True)
        fixture.send(command)
        fixture.next('started', 1)
        data = bytearray()
        for index in range(6):
            event = fixture.next('robot_description_observation', 1)
            chunk = event['sample']
            self.assertEqual((chunk['index'], chunk['count'], chunk['document_bytes'], event['sequence']), (index, 6, 1048576, index + 1))
            data.extend(base64.b64decode(chunk['data_base64'], validate=True))
        self.assertEqual(len(data), 1048576)
        self.assertEqual(hashlib.sha256(data).hexdigest(), chunk['sha256'])
        self.assertTrue(data.startswith(b'<?xml'))
        duplicate = start(2, 2, '/demo/robot_description', output='robot_description', best_effort=False, transient_local=True)
        fixture.send(duplicate)
        self.assertTrue(fixture.next('request_error', 2)['no_handle'])
        fixture.send({'command':'stop','generation':7,'request':3,'subscription':1})
        fixture.next('drops', 1)
        self.assertEqual(fixture.next('stopped', 1)['request'], 3)

    def test_tf_graph_shared_admission_disabled_until_start_and_independent_stop(self):
        fixture = self.fixture()
        fixture.ready()
        fixture.send(tf_graph_start(1, 1))
        self.assertEqual(fixture.next('started', 1)['request'], 1)
        event = fixture.next('tf_graph_observation', 1)
        graph = event['sample']
        self.assertEqual([f['name'] for f in graph['frames']], ['base', 'laser', 'map', 'orphan', 'remote'])
        self.assertEqual(graph['frames'][0]['parent']['kind'], 'dynamic')
        self.assertEqual(graph['frames'][3]['pose']['status'], 'missing')
        fixture.send(robot_start(2, 2, dynamic_tf_best_effort=False))
        fixture.next('robot_observation', 2)
        fixture.send(tf_graph_start(3, 3))
        self.assertTrue(fixture.next('request_error', 3)['no_handle'])
        fixture.send(stop(4, 1))
        fixture.next('drops', 1)
        fixture.next('stopped', 1)
        self.assertGreater(fixture.next('robot_observation', 2)['sequence'], 1)
        fixture.send(tf_graph_start(5, 4))
        self.assertEqual(fixture.next('tf_graph_observation', 4)['sequence'], 1)

    def test_tf_graph_empty_stale_missing_and_clock_rejection(self):
        for mode, expected in [('tf-empty', None), ('tf-stale', 'stale'), ('tf-missing', 'missing')]:
            fixture = self.fixture(mode)
            fixture.ready()
            fixture.send(tf_graph_start(1, 1))
            frames = fixture.next('tf_graph_observation', 1)['sample']['frames']
            if expected is None:
                self.assertEqual(frames, [])
            else:
                self.assertEqual(frames[0]['pose']['status'], expected)
        fixture = self.fixture()
        fixture.ready()
        fixture.send(tf_graph_start(1, 1, clock_policy='unknown'))
        self.assertTrue(fixture.next('request_error', 1)['no_handle'])
        invalid = tf_graph_start(2, 2)
        invalid['configuration']['best_effort'] = True
        fixture.send(invalid)
        self.assertTrue(fixture.next('request_error', 2)['no_handle'])


    def test_mapping_discovery_explicit_activation_complete_analytic_geometry_and_identity(self):
        fixture = self.fixture()
        graph = fixture.ready()
        for topic, typ in [('/demo/map','nav_msgs/msg/OccupancyGrid'),('/demo/pose','geometry_msgs/msg/PoseStamped'),('/demo/path','nav_msgs/msg/Path')]:
            self.assertIn([topic,[typ]],graph['topics'])
        with self.assertRaises(queue.Empty):
            fixture.events.get(timeout=.08)
        for sub, kind in enumerate(('occupancy_grid','pose','path'),1):
            fixture.send(mapping_start(sub,sub,kind))
            event = fixture.next('mapping_observation',sub)
            sample = event['sample']
            self.assertEqual((event['generation'],event['subscription'],sample['contract_version']),(7,sub,1))
            self.assertEqual((sample['fixed_frame'],sample['source_frame']),('map','mapping_source'))
            self.assertGreater(sample['acquisition_stamp_ns'],0)
            self.assertEqual(sample['geometry']['kind'],kind)
            self.assertEqual(sample['tf']['status'],'ready')
            self.assertEqual(sample['tf']['components'][:3],[1.,2.,3.])
            self.assertEqual(len(sample['counters']),9)
            if kind == 'occupancy_grid':
                geometry = sample['geometry']
                self.assertEqual((geometry['width'],geometry['height'],geometry['resolution']),(3,2,.5))
                self.assertEqual(geometry['cells'],[-1,0,25,50,75,100])
                self.assertEqual(geometry['origin'][:3],[1.25,-.25,0])
            elif kind == 'pose':
                self.assertEqual(sample['geometry']['components'][:3],[1,0,0])
            else:
                self.assertEqual([p['components'][:3] for p in sample['geometry']['poses']],[[1,0,0],[1,.5,0],[.5,0,0]])
                self.assertTrue(all(p['stamp_ns']==0 and p['frame_id']=='' for p in sample['geometry']['poses']))
        # Changing fixed frame/TF sources requires retiring every owner of the
        # old shared scene, including Pose and Path, before explicit reactivation.
        for request, subscription in ((4,1),(5,2),(6,3)):
            fixture.send(stop(request,subscription));fixture.next('stopped',subscription)
        fixture.send(mapping_start(7,4,fixed_frame='mapping_source',dynamic_tf_topic='/absent',static_tf_topic='/absent_static'))
        identity = fixture.next('mapping_observation',4)['sample']
        self.assertEqual(identity['tf'],dict(status='ready',components=[0.,0.,0.,0.,0.,0.,1.]))
        self.assertEqual(identity['counters']['tf_accepted'],0)

    def test_mapping_type_clock_tf_and_owner_caps_hold_during_starting(self):
        fixture = self.fixture('start-delay');fixture.ready()
        for sub,kind in enumerate(('occupancy_grid','pose','path'),1):
            fixture.send(mapping_start(sub,sub,kind))
        fixture.send(mapping_start(4,4,'pose'))
        self.assertTrue(fixture.next('request_error',4)['no_handle'])
        for sub in (5,6,7):
            fixture.send(start(sub,sub))
        fixture.send(start(8,8))
        fixture.next('started',8)
        fixture.send(robot_start(9,9,dynamic_tf_best_effort=False))
        fixture.next('robot_observation',9)
        fixture.send(stop(10,1));fixture.next('stopped',1)
        fixture.send(mapping_start(11,10))
        fixture.next('mapping_observation',10)
        fixture.send(stop(12,10));fixture.next('stopped',10)
        for request, mutate in [(13,lambda c:c.update(message_type='std_msgs/msg/String')),
                                (14,lambda c:c['spatial'].update(clock_policy='unknown')),
                                (15,lambda c:c['spatial'].update(static_tf_topic='/tf'))]:
            command = mapping_start(request,request)
            mutate(command['configuration']);fixture.send(command)
            self.assertTrue(fixture.next('request_error',request)['no_handle'])

    def test_mapping_missing_recovery_empty_maximum_and_malformed_are_bounded_distinct_modes(self):
        fixture = self.fixture('mapping-delay');fixture.ready()
        fixture.send(mapping_start(1,1))
        missing=fixture.next('mapping_observation',1)
        self.assertEqual(missing['sample']['tf']['status'],'missing')
        while True:
            recovered=fixture.next('mapping_observation',1)
            if recovered['sample']['tf']['status']=='ready':
                break
        self.assertGreater(recovered['sequence'],missing['sequence'])
        for mode in ('mapping-empty','mapping-large','mapping-malformed'):
            fixture=self.fixture(mode);fixture.ready()
            for sub,kind in enumerate(('occupancy_grid','path','pose'),1):
                fixture.send(mapping_start(sub,sub,kind));packet=fixture.next('mapping_observation',sub)
                self.assertLess(len(json.dumps(packet['sample']).encode()),512*1024)
                geometry=packet['sample']['geometry']
                if mode=='mapping-empty' and kind=='occupancy_grid':
                    self.assertEqual((geometry['width'],geometry['height'],geometry['cells']),(0,0,[]))
                elif mode=='mapping-empty' and kind=='path':
                    self.assertEqual(geometry['poses'],[])
                elif mode=='mapping-large' and kind=='occupancy_grid':
                    self.assertEqual((geometry['width'],geometry['height'],len(geometry['cells'])),(256,256,65536))
                elif mode=='mapping-large' and kind=='path':
                    self.assertEqual(len(geometry['poses']),1024)
                elif mode=='mapping-malformed':
                    if kind=='occupancy_grid':self.assertEqual(geometry['cells'][0],101)
                    elif kind=='pose':self.assertEqual(geometry['components'][3:],[0.]*4)
                    else:self.assertEqual(geometry['poses'][0]['frame_id'],'contradictory_frame')

    def test_mapping_stop_peer_isolation_and_fresh_process_never_replays_activation(self):
        fixture=self.fixture();fixture.ready()
        fixture.send(start(1,1));peer=fixture.next('observation',1)
        fixture.send(mapping_start(2,2));fixture.next('mapping_observation',2)
        fixture.send(stop(3,2));self.assertIsNone(fixture.next('drops',2)['spatial_counters']);fixture.next('stopped',2)
        self.assertGreater(fixture.next('observation',1)['sequence'],peer['sequence'])
        fixture.send(mapping_start(4,3,'pose'));replacement=fixture.next('mapping_observation',3)
        self.assertEqual(replacement['sequence'],1)
        while not fixture.events.empty():
            packet=fixture.events.get_nowait()
            self.assertFalse(packet['event']=='mapping_observation' and packet.get('subscription')==2)
        fresh=self.fixture();fresh.ready()
        with self.assertRaises(queue.Empty):fresh.events.get(timeout=.1)

    def test_mapping_scoped_failure_keeps_ordinary_peer_and_releases_mapping_kind(self):
        fixture=self.fixture('mapping-error');fixture.ready()
        fixture.send(start(1,1));peer=fixture.next('observation',1)
        fixture.send(mapping_start(2,2));fixture.next('mapping_observation',2)
        self.assertEqual(fixture.next('drops',2)['backend_drops'],0)
        self.assertEqual(fixture.next('stream_error',2)['subscription'],2)
        self.assertGreater(fixture.next('observation',1)['sequence'],peer['sequence'])
        fixture.send(mapping_start(3,3));self.assertEqual(fixture.next('mapping_observation',3)['sequence'],1)

    def test_spatial_discovery_typed_scan_tf_inspection_and_explicit_group_output(self):
        fixture = self.fixture()
        graph = fixture.ready()
        for topic, typ in [('/demo/scan', 'sensor_msgs/msg/LaserScan'), ('/tf', 'tf2_msgs/msg/TFMessage'), ('/tf_static', 'tf2_msgs/msg/TFMessage')]:
            self.assertIn([topic, [typ]], graph['topics'])
        with self.assertRaises(queue.Empty):
            fixture.events.get(timeout=0.08)
        fixture.send(start(1, 1, '/demo/scan', 'sensor_msgs/msg/LaserScan'))
        packet = fixture.next('observation', 1)
        fields = {field['name']: field['value'] for field in packet['sample']['value']['fields']}
        self.assertEqual(fields['ranges']['original_items'], len(fields['ranges']['items']))
        self.assertEqual(len(fields['ranges']['items']), 180)
        fixture.send(stop(2, 1))
        self.assertIsNone(fixture.next('drops', 1)['spatial_counters'])
        fixture.next('stopped', 1)
        fixture.send(start(3, 2, '/tf_static', 'tf2_msgs/msg/TFMessage'))
        packet = fixture.next('observation', 2)
        fields = {field['name']: field['value'] for field in packet['sample']['value']['fields']}
        transform = {field['name']: field['value'] for field in fields['transforms']['items'][0]['fields']}
        self.assertEqual(transform['child_frame_id']['value'], 'laser')
        fixture.send(stop(4, 2))
        fixture.next('stopped', 2)
        fixture.send(spatial_start(5, 3))
        packet = fixture.next('spatial_observation', 3)
        sample = packet['sample']
        self.assertEqual((packet['generation'], packet['subscription'], packet['sequence']), (7, 3, 1))
        self.assertEqual((sample['contract_version'], sample['fixed_frame'], sample['source_frame']), (1, 'map', 'laser'))
        self.assertEqual(sample['tf']['status'], 'ready')
        self.assertEqual(sample['tf']['components'][:3], [1.0, 2.0, 0.25])
        self.assertAlmostEqual(sum(value * value for value in sample['tf']['components'][3:]), 1.0)
        self.assertEqual(sample['acquisition_stamp_ns'], 10200000000)
        scan = {field['name']: field['value'] for field in sample['scan']['value']['fields']}
        header = {field['name']: field['value'] for field in scan['header']['fields']}
        stamp = {field['name']: field['value'] for field in header['stamp']['fields']}
        self.assertEqual(sample['acquisition_stamp_ns'], int(stamp['sec']['value']) * 1000000000 + int(stamp['nanosec']['value']))
        self.assertEqual(header['frame_id']['value'], sample['source_frame'])
        self.assertEqual(len(sample['counters']), 9)
        self.assertLess(len(json.dumps(sample).encode()), 512 * 1024)

    def test_spatial_shared_tf_preserves_peers_beyond_eight_readers_and_reuses_capacity(self):
        fixture = self.fixture()
        fixture.ready()
        fixture.send(start(1, 1))
        peer = fixture.next('observation', 1)
        fixture.send(spatial_start(2, 2))
        sample = fixture.next('spatial_observation', 2)['sample']
        for identity in range(3, 7):
            fixture.send(start(identity, identity))
            fixture.next('started', identity)
        fixture.send(start(7, 7))
        fixture.next('started',7)
        fixture.send(spatial_start(8, 8))
        fixture.next('started',8)
        fixture.send(stop(9, 2))
        counters = fixture.next('drops', 2)['spatial_counters']
        self.assertEqual(set(counters), set(sample['counters']))
        self.assertGreaterEqual(counters['scans_received'], sample['counters']['scans_received'])
        fixture.next('stopped', 2)
        self.assertGreater(fixture.next('observation', 1)['sequence'], peer['sequence'])
        fixture.send(spatial_start(10, 9))
        fixture.next('started', 9)
        fixture.send(stop(11, 9))
        fixture.next('stopped', 9)
        for request, subscription in [(12, 10), (13, 11), (14, 12)]:
            fixture.send(start(request, subscription))
            fixture.next('started', subscription)
        fixture.send(start(15, 13))
        fixture.next('started',13)

    def test_spatial_missing_then_ready_keeps_exact_acquisition_and_group_health(self):
        fixture = self.fixture('spatial-delay')
        fixture.ready()
        fixture.send(spatial_start(1, 1))
        missing = fixture.next('spatial_observation', 1)
        ready = fixture.next('spatial_observation', 1)
        self.assertEqual(missing['sample']['tf']['status'], 'missing')
        self.assertEqual(ready['sample']['tf']['status'], 'ready')
        self.assertGreater(ready['sequence'], missing['sequence'])
        self.assertEqual(missing['sample']['scan'], ready['sample']['scan'])
        self.assertEqual(missing['sample']['acquisition_stamp_ns'], ready['sample']['acquisition_stamp_ns'])
        self.assertEqual(ready['sample']['counters']['scans_received'], 1)
        self.assertEqual(ready['sample']['counters']['lookup_timeouts'], 0)
        self.assertIsNone(fixture.process.poll())

    def test_unknown_same_frame_or_unavailable_tf_never_invents_identity_and_timeout_is_nonfatal(self):
        for settings in [dict(fixed_frame='unknown'), dict(fixed_frame='laser'), dict(dynamic_tf_topic='/missing_tf')]:
            fixture = self.fixture()
            fixture.ready()
            fixture.send(spatial_start(1, 1, **settings))
            packet = fixture.next('spatial_observation', 1)
            self.assertEqual(packet['sample']['tf'], {'status': 'missing'})
        fixture = self.fixture('spatial-timeout')
        fixture.ready()
        fixture.send(start(1, 1))
        peer = fixture.next('observation', 1)
        fixture.send(spatial_start(2, 2))
        missing = fixture.next('spatial_observation', 2)
        timeout = fixture.next('spatial_observation', 2)
        self.assertEqual(timeout['sample']['tf'], {'status': 'lookup_timeout'})
        self.assertEqual(timeout['sample']['acquisition_stamp_ns'], missing['sample']['acquisition_stamp_ns'])
        self.assertEqual(timeout['sample']['counters']['lookup_timeouts'], 1)
        self.assertEqual(timeout['sample']['counters']['scans_received'], 1)
        with self.assertRaises(queue.Empty):
            fixture.next('spatial_observation', 2, timeout=0.25)
        self.assertGreater(fixture.next('observation', 1)['sequence'], peer['sequence'])
        self.assertIsNone(fixture.process.poll())

    def test_spatial_fatal_group_failure_reports_final_counters_and_preserves_peer(self):
        fixture = self.fixture('spatial-error')
        fixture.ready()
        fixture.send(start(1, 1))
        peer = fixture.next('observation', 1)
        fixture.send(spatial_start(2, 2))
        fixture.next('spatial_observation', 2)
        drops = fixture.next('drops', 2)
        self.assertEqual(drops['spatial_counters']['tf_rejected'], 1)
        self.assertEqual(drops['spatial_counters']['native_errors'], 0)
        self.assertIn('all three', fixture.next('stream_error', 2)['message'])
        self.assertGreater(fixture.next('observation', 1)['sequence'], peer['sequence'])
        fixture.send(spatial_start(3, 3))
        fixture.next('started', 3)
        self.assertIsNone(fixture.process.poll())

    def test_spatial_configuration_policy_types_tf_endpoints_and_shape_are_validated(self):
        fixture = self.fixture()
        fixture.ready()
        fixture.send(start(1, 1))
        fixture.next('observation', 1)
        invalid = [spatial_start(2, 2, fixed_frame=''), spatial_start(3, 3, fixed_frame='/map'),
                   spatial_start(4, 4, clock_policy='unknown'), spatial_start(5, 5, clock_policy='simulated'),
                   spatial_start(6, 6, dynamic_tf_topic='relative'), spatial_start(7, 7, static_tf_topic='/tf')]
        no_config = spatial_start(8, 8)
        no_config['configuration']['spatial'] = None
        invalid.append(no_config)
        wrong_type = spatial_start(9, 9)
        wrong_type['configuration']['message_type'] = 'std_msgs/msg/String'
        invalid.append(wrong_type)
        wrong_output = spatial_start(10, 10)
        wrong_output['configuration']['output'] = 'typed'
        invalid.append(wrong_output)
        for command in invalid:
            fixture.send(command)
            self.assertTrue(fixture.next('request_error', command['subscription'])['no_handle'])
        fixture.send(spatial_start(11, 11))
        fixture.next('started', 11)
        for change in [dict(clock_policy='forged'), dict(dynamic_tf_best_effort='false'), dict(static_tf_best_effort=True)]:
            bad = self.fixture()
            bad.ready()
            command = spatial_start(1, 1)
            command['configuration']['spatial'].update(change)
            bad.send(command)
            bad.next('error')
            self.assertEqual(bad.process.wait(timeout=1), 0)

    def test_spatial_start_cancellation_holds_shared_readers_until_stop_and_no_old_samples_survive(self):
        fixture = self.fixture('start-delay')
        fixture.ready()
        fixture.send(spatial_start(1, 1))
        for identity in range(2, 7):
            fixture.send(start(identity, identity))
        fixture.send(start(7, 7))
        fixture.send(stop(8, 1))
        drops = fixture.next('drops', 1)
        self.assertEqual(drops['spatial_counters']['scans_received'], 0)
        fixture.next('stopped', 1)
        fixture.send(spatial_start(9, 8))
        fixture.next('started', 8)
        remaining = []
        while not fixture.events.empty():
            remaining.append(fixture.events.get_nowait())
        self.assertFalse(any(event.get('subscription') == 1 and event['event'] in ('started', 'spatial_observation') for event in remaining))
        failed = self.fixture('start-error')
        failed.ready()
        failed.send(spatial_start(1, 1))
        self.assertEqual(sum(failed.next('drops', 1)['spatial_counters'].values()), 0)
        self.assertTrue(failed.next('request_error', 1)['no_handle'])

    def test_image_output_is_explicit_required_and_wrong_message_type_is_scoped(self):
        fixture = self.fixture()
        graph = fixture.ready()
        self.assertIn(['/demo/camera/rgb8', ['sensor_msgs/msg/Image']], graph['topics'])
        with self.assertRaises(queue.Empty):
            fixture.events.get(timeout=0.08)
        fixture.send(start(1, 1, '/demo/camera/rgb8', 'sensor_msgs/msg/Image'))
        fixture.next('started', 1)
        event = fixture.next('observation', 1)
        fields = {field['name']: field['value'] for field in event['sample']['value']['fields']}
        self.assertEqual(fields['encoding'], {'kind': 'text', 'value': 'rgb8', 'original_bytes': 4})
        self.assertEqual(len(fields['data']['items']), 2048)
        self.assertEqual(fields['data']['original_items'], (64 * 3 + 2) * 48)
        self.assertEqual(fields['width']['value'], '64')
        fixture.send(start(2, 2, output='compact_image'))
        self.assertTrue(fixture.next('request_error', 2)['no_handle'])
        self.assertGreater(fixture.next('observation', 1)['sequence'], event['sequence'])
        for invalid_output in [None, 'auto', True]:
            bad = self.fixture()
            bad.ready()
            command = start(1, 1)
            if invalid_output is None:
                del command['configuration']['output']
            else:
                command['configuration']['output'] = invalid_output
            bad.send(command)
            bad.next('error')
            self.assertEqual(bad.process.wait(timeout=1), 0)

    def test_five_compact_encodings_have_known_colors_padding_orientation_and_metadata(self):
        fixture = self.fixture()
        fixture.ready()
        for index, encoding in enumerate(['mono8', 'rgb8', 'bgr8', 'rgba8', 'bgra8']):
            with self.subTest(encoding=encoding):
                subscription = index + 1
                fixture.send(start(index * 2 + 1, subscription, '/demo/camera/' + encoding,
                                   'sensor_msgs/msg/Image', output='compact_image'))
                fixture.next('started', subscription)
                event = fixture.next('image_observation', subscription)
                self.assertEqual((event['generation'], event['subscription'], event['sequence']), (7, subscription, 1))
                sample = event['sample']
                metadata = sample['metadata']
                self.assertEqual((sample['contract_version'], sample['message_type']), (2, 'sensor_msgs/msg/Image'))
                self.assertEqual(metadata['header_stamp'], {'seconds': 0, 'nanoseconds': 100000000})
                self.assertEqual(metadata['frame_id'], 'studio_sim/camera_optical')
                self.assertNotIn('source_time', sample)
                self.assertEqual(metadata['encoding'], encoding)
                raw = base64.b64decode(sample['data_base64'], validate=True)
                channels = {'mono8': 1, 'rgb8': 3, 'bgr8': 3, 'rgba8': 4, 'bgra8': 4}[encoding]
                self.assertEqual(metadata['step'], 64 * channels + 2)
                self.assertEqual(len(raw), metadata['step'] * metadata['height'])
                self.assertEqual(sample['original_bytes'], len(raw))
                self.assertTrue(sample['complete'])
                for row in range(48):
                    self.assertEqual(raw[(row + 1) * metadata['step'] - 2:(row + 1) * metadata['step']], b'\xa5\xa5')
                def pixel(x, y):
                    offset = y * metadata['step'] + x * channels
                    return tuple(raw[offset:offset + channels])
                for x, y, rgba in [(8, 0, (255, 0, 255, 255)), (0, 47, (0, 255, 255, 255)),
                                   (1, 1, (255, 0, 0, 255)), (63, 1, (0, 255, 0, 192)),
                                   (1, 47, (0, 0, 255, 128)), (63, 47, (255, 255, 255, 64))]:
                    r, g, b, a = rgba
                    expected = {'mono8': ((77 * r + 150 * g + 29 * b + 128) // 256,),
                                'rgb8': (r, g, b), 'bgr8': (b, g, r),
                                'rgba8': rgba, 'bgra8': (b, g, r, a)}[encoding]
                    self.assertEqual(pixel(x, y), expected)
                fixture.send(stop(index * 2 + 2, subscription))
                fixture.next('stopped', subscription)

    def test_unsupported_encoding_is_forwarded_until_client_stops_and_peer_continues(self):
        fixture = self.fixture()
        fixture.ready()
        fixture.send(start(1, 1))
        peer = fixture.next('observation', 1)
        fixture.send(start(2, 2, '/demo/camera/unsupported', 'sensor_msgs/msg/Image', output='compact_image'))
        packet = fixture.next('image_observation', 2)
        sample = packet['sample']
        self.assertEqual(sample['metadata']['encoding'], '16SC1')
        self.assertEqual(len(base64.b64decode(sample['data_base64'])), sample['original_bytes'])
        self.assertGreater(fixture.next('image_observation', 2)['sequence'], packet['sequence'])
        fixture.send(stop(3, 2))
        self.assertEqual(fixture.next('drops', 2)['backend_drops'], 0)
        fixture.next('stopped', 2)
        self.assertGreater(fixture.next('observation', 1)['sequence'], peer['sequence'])
        self.assertIsNone(fixture.process.poll())

    def test_format_changes_and_deliberately_malformed_image_wire_are_distinct(self):
        fixture = self.fixture()
        fixture.ready()
        fixture.send(start(1, 1, '/demo/camera/format_changes', 'sensor_msgs/msg/Image', output='compact_image'))
        formats = []
        dimensions = set()
        for expected_sequence in range(1, 6):
            packet = fixture.next('image_observation', 1)
            sample = packet['sample']
            metadata = sample['metadata']
            self.assertEqual(packet['sequence'], expected_sequence)
            self.assertEqual(len(base64.b64decode(sample['data_base64'])), metadata['step'] * metadata['height'])
            formats.append(metadata['encoding'])
            dimensions.add((metadata['width'], metadata['height']))
        self.assertEqual(formats, ['mono8', 'rgb8', 'bgr8', 'rgba8', 'bgra8'])
        self.assertEqual(dimensions, {(64, 48), (32, 24)})
        fixture.send(stop(2, 1))
        fixture.next('stopped', 1)
        fixture.send(start(3, 2, '/demo/camera/malformed', 'sensor_msgs/msg/Image', output='compact_image'))
        malformed = fixture.next('image_observation', 2)['sample']
        self.assertNotEqual(malformed['original_bytes'], len(base64.b64decode(malformed['data_base64'])))
        self.assertLess(len(json.dumps(malformed).encode()), 512 * 1024)

    def test_image_flood_preserves_low_rate_and_explicit_stop_progress(self):
        fixture = self.fixture('flood')
        fixture.ready()
        fixture.send(start(1, 1, '/demo/camera/rgb8', 'sensor_msgs/msg/Image', output='compact_image'))
        fixture.next('image_observation', 1)
        fixture.send(start(2, 2, '/demo/slow', 'std_msgs/msg/Int32'))
        first = fixture.next('observation', 2)
        self.assertGreater(fixture.next('observation', 2)['sequence'], first['sequence'])
        fixture.send(stop(3, 1))
        fixture.next('stopped', 1)
        self.assertIsNone(fixture.process.poll())
        self.assertGreater(fixture.next('observation', 2)['sequence'], first['sequence'])

    def test_ready_can_precede_first_graph_and_pending_discovery_cancels(self):
        fixture = Fixture('graph-delay')
        try:
            self.assertEqual(fixture.next('ready')['version'], SESSION_VERSION)
            with self.assertRaises(queue.Empty):
                fixture.events.get(timeout=0.1)
            graph = fixture.next('graph', timeout=6)
            self.assertTrue(graph['topics'])
            self.assertFalse((Path(fixture.directory.name) / 'commands.jsonl').exists())
        finally:
            fixture.close()
        fixture = Fixture('graph-delay')
        try:
            fixture.next('ready')
        finally:
            fixture.close()

    def fixture(self, *args, **kwargs):
        fixture = Fixture(*args, **kwargs)
        self.addCleanup(fixture.close)
        return fixture

    def test_discovery_does_not_activate_and_start_preserves_typed_exact_values(self):
        fixture = self.fixture()
        graph = fixture.ready()
        self.assertIn(['/demo/multiple_types', ['std_msgs/msg/String', 'std_msgs/msg/Int32']], graph['topics'])
        with self.assertRaises(queue.Empty):
            fixture.events.get(timeout=0.08)
        fixture.send(start(1, 1, '/demo/numeric_edges', 'fixture/msg/NumericEdges', transient_local=True))
        self.assertEqual(fixture.next()['event'], 'started')
        sample = fixture.next('observation')
        self.assertEqual((sample['generation'], sample['subscription'], sample['sequence']), (7, 1, 1))
        fields = {f['name']: f['value'] for f in sample['sample']['value']['fields']}
        self.assertEqual(fields['exact_u64']['value'], '18446744073709551615')
        self.assertEqual(fields['exact_i64']['value'], '-9223372036854775808')
        self.assertGreater(fields['bounded_text']['original_bytes'], len(fields['bounded_text']['value'].encode()))
        command = json.loads((Path(fixture.directory.name) / 'commands.jsonl').read_text().splitlines()[0])
        self.assertTrue(command['configuration']['transient_local'])

    def test_stop_retry_replay_replacement_and_peer_sequence(self):
        fixture = self.fixture()
        fixture.ready()
        fixture.send(start(1, 1))
        fixture.next('observation', 1)
        fixture.send(start(2, 2, '/demo/slow', 'std_msgs/msg/Int32'))
        peer = fixture.next('observation', 2)
        fixture.send(stop(3, 1))
        fixture.next('stopped', 1)
        fixture.send(stop(4, 1))
        fixture.next('stopped', 1)
        fixture.send(start(5, 1))
        self.assertTrue(fixture.next('request_error', 1)['no_handle'])
        fixture.send(start(6, 3))
        fixture.next('started', 3)
        later = fixture.next('observation', 2)
        self.assertGreater(later['sequence'], peer['sequence'])
        fixture.send(start(6, 3))
        self.assertFalse(fixture.next('request_error', 3)['no_handle'])
        fixture.send(stop(7, 3))
        fixture.next('stopped', 3)
        time.sleep(0.05)
        remaining = []
        while not fixture.events.empty():
            remaining.append(fixture.events.get_nowait())
        self.assertFalse(any(e['event'] == 'observation' and e['subscription'] in (1, 3) for e in remaining))

    def test_capacity_cancel_during_start_and_slot_reuse(self):
        fixture = self.fixture('start-delay')
        fixture.ready()
        for subscription in range(1, 34):
            fixture.send(start(subscription, subscription))
        self.assertTrue(fixture.next('request_error', 33)['no_handle'])
        fixture.send(stop(34, 1))
        fixture.next('stopped', 1)
        fixture.send(start(35, 34))
        fixture.next('started', 34)
        remaining = []
        while not fixture.events.empty():
            remaining.append(fixture.events.get_nowait())
        self.assertFalse(any(e.get('subscription') == 1 and e['event'] in ('started', 'observation') for e in remaining))

    def test_flood_low_rate_and_stop_have_progress(self):
        fixture = self.fixture('flood')
        fixture.ready()
        fixture.send(start(1, 1))
        fixture.next('observation', 1)
        fixture.send(start(2, 2, '/demo/slow', 'std_msgs/msg/Int32'))
        first = fixture.next('observation', 2)
        second = fixture.next('observation', 2)
        self.assertGreater(second['sequence'], first['sequence'])
        fixture.send(stop(3, 1))
        fixture.next('stopped', 1)
        self.assertIsNone(fixture.process.poll())
        self.assertGreater(fixture.next('observation', 2)['sequence'], second['sequence'])

    def test_waiting_empty_retained_and_incompatible_states(self):
        for mode in ('waiting', 'empty'):
            fixture = self.fixture(mode)
            graph = fixture.ready()
            self.assertEqual(not graph['topics'], mode == 'empty')
            fixture.send(start(1, 1))
            fixture.next('started')
            with self.assertRaises(queue.Empty):
                fixture.events.get(timeout=0.08)
        fixture = self.fixture('retained')
        fixture.ready()
        fixture.send(start(1, 1, transient_local=True))
        fixture.next('observation')
        with self.assertRaises(queue.Empty):
            fixture.events.get(timeout=0.25)
        bad = self.fixture('incompatible')
        self.assertEqual(bad.next('ready')['version'], SESSION_VERSION - 1)
        self.assertEqual(bad.process.wait(timeout=1), 0)

    def test_scoped_failure_leaves_peer_running_and_global_failure_exits(self):
        fixture = self.fixture()
        fixture.ready()
        fixture.send(start(1, 1, '/demo/stream_error'))
        fixture.send(start(2, 2))
        fixture.next('stream_error', 1)
        self.assertGreater(fixture.next('observation', 2)['sequence'], 1)
        fixture.send(start(3, 3, '/demo/start_error'))
        self.assertTrue(fixture.next('request_error', 3)['no_handle'])
        failed = self.fixture('failure')
        failed.ready()
        failed.send(start(1, 1))
        failed.next('error')
        self.assertEqual(failed.process.wait(timeout=1), 0)

    def test_discovered_error_topics_are_reachable_and_keep_healthy_peer_receiving(self):
        fixture = self.fixture()
        graph = fixture.ready()
        for topic in ['/demo/start_error', '/demo/stream_error']:
            self.assertIn([topic, ['std_msgs/msg/String']], graph['topics'])
        pid = fixture.process.pid
        fixture.send(start(1, 1, '/demo/status'))
        first = fixture.next('observation', 1)
        fixture.send(start(2, 2, '/demo/stream_error'))
        self.assertEqual(fixture.next('drops', 2)['backend_drops'], 0)
        self.assertIn('Synthetic scoped failure', fixture.next('stream_error', 2)['message'])
        second = fixture.next('observation', 1)
        self.assertGreater(second['sequence'], first['sequence'])
        fixture.send(start(3, 3, '/demo/start_error'))
        self.assertEqual(fixture.next('drops', 3)['backend_drops'], 0)
        error = fixture.next('request_error', 3)
        self.assertTrue(error['no_handle'])
        self.assertGreater(fixture.next('observation', 1)['sequence'], second['sequence'])
        self.assertEqual(fixture.process.pid, pid)
        self.assertIsNone(fixture.process.poll())

    def test_bounded_malformed_input_generation_and_eof_cancel(self):
        for frame in [b' ' * (128 * 1024 + 1), b'{"command":"publish"}\n',
                      b'{"command":"stop","generation":1,"request":1,"subscription":1,"request":2}\n']:
            fixture = self.fixture()
            fixture.ready()
            fixture.process.stdin.write(frame)
            fixture.process.stdin.flush()
            fixture.next('error')
            self.assertEqual(fixture.process.wait(timeout=1), 0)
        fixture = self.fixture()
        fixture.ready()
        fixture.send(start(1, 1))
        fixture.next('started')
        command = stop(2, 1)
        command['generation'] = 8
        fixture.send(command)
        self.assertIn('Generation', fixture.next('error')['message'])
        self.assertEqual(fixture.process.wait(timeout=1), 0)
        delayed = self.fixture('delayed')
        delayed.process.stdin.close()
        self.assertEqual(delayed.process.wait(timeout=1), 0)
        self.assertTrue(delayed.events.empty())
        active = self.fixture()
        active.ready()
        active.send(start(1, 1))
        active.next('started')
        active.process.stdin.close()
        self.assertEqual(active.process.wait(timeout=1), 0)

    def test_long_truncation_invalid_config_and_control_only_rejection(self):
        fixture = self.fixture('long')
        graph = fixture.ready()
        long_topic = max(graph['topics'], key=lambda t: len(t[0]))[0]
        self.assertLessEqual(len(long_topic.encode()), 256)
        self.assertGreater(len(long_topic), 160)
        invalid = start(1, 1, 'relative')
        fixture.send(invalid)
        self.assertTrue(fixture.next('request_error', 1)['no_handle'])
        fixture.send(start(2, 2, long_topic))
        sample = fixture.next('observation', 2)['sample']
        self.assertEqual(sample['topic'], long_topic)
        value = sample['value']['fields'][0]['value']
        self.assertGreater(value['original_bytes'], len(value['value'].encode()))
        self.assertLess(len(json.dumps(sample).encode()), 512 * 1024)
        # No aliases for commands with effects, malformed bools or oversized IDs.
        for command in [dict(stop(1, 1), command='replace'),
                        dict(stop(1, 1), request=1 << 64),
                        dict(stop(1, 1), subscription=True),
                        dict(start(1, 1), configuration={**start(1, 1)['configuration'], 'best_effort': 1})]:
            bad = self.fixture()
            bad.ready()
            bad.send(command)
            bad.next('error')
            self.assertEqual(bad.process.wait(timeout=1), 0)

    def test_legacy_v2_remains_explicit_and_argument_errors_do_not_fallback(self):
        fixture = self.fixture(arguments=['/demo/status', 'std_msgs/msg/String'])
        self.assertEqual(fixture.next('hello')['version'], 2)
        self.assertEqual(fixture.next('typed_sample')['sample']['contract_version'], 2)
        (Path(fixture.directory.name) / 'exit-request').touch()
        self.assertEqual(fixture.process.wait(timeout=1), 0)
        invalid = self.fixture(arguments=['--observe-session', '/extra'])
        self.assertNotEqual(invalid.process.wait(timeout=1), 0)
        self.assertTrue(invalid.events.empty())

    def test_robot_explicit_complete_order_latest_stamps_and_ten_hz_bound_even_under_flood(self):
        fixture = self.fixture('flood')
        fixture.ready()
        with self.assertRaises(queue.Empty):
            fixture.events.get(timeout=0.08)
        command = robot_start(1, 1)
        fixture.send(command)
        first = fixture.next('robot_observation', 1)
        sample = first['sample']
        self.assertEqual((first['generation'], first['subscription'], first['sequence']), (7, 1, 1))
        self.assertEqual(sample['policy'], 'latest_available')
        self.assertEqual(sample['contract_version'], 1)
        self.assertEqual(sample['fixed_frame'], 'map')
        self.assertEqual([link['frame'] for link in sample['links']], command['configuration']['robot']['frames'])
        self.assertEqual(sample['links'][0]['pose']['resolved_stamp_ns'], 0)
        self.assertEqual(sample['links'][0]['pose']['components'], [0., 0., 0., 0., 0., 0., 1.])
        self.assertNotEqual(sample['links'][1]['pose']['resolved_stamp_ns'], sample['links'][2]['pose']['resolved_stamp_ns'])
        self.assertLess(len(json.dumps(sample).encode()), 512 * 1024)
        before = time.monotonic()
        for expected in range(2, 7):
            self.assertEqual(fixture.next('robot_observation', 1)['sequence'], expected)
        self.assertGreaterEqual(time.monotonic() - before, 0.4)

    def test_robot_partial_missing_and_delayed_leaf_recovery_preserve_other_link_poses(self):
        for mode in ['robot-missing', 'robot-delay']:
            fixture = self.fixture(mode)
            fixture.ready()
            fixture.send(robot_start(1, 1))
            first = fixture.next('robot_observation', 1)['sample']
            self.assertEqual(first['links'][2], {'frame': 'tip', 'pose': {'status': 'missing'}})
            self.assertTrue(all(link['pose']['status'] == 'ready' for i, link in enumerate(first['links']) if i != 2))
            for _ in range(4):
                later = fixture.next('robot_observation', 1)['sample']
            self.assertEqual(later['links'][2]['pose']['status'], 'ready' if mode == 'robot-delay' else 'missing')
            self.assertEqual([link['frame'] for link in first['links']], [link['frame'] for link in later['links']])
            self.assertGreater(later['links'][1]['pose']['resolved_stamp_ns'], first['links'][1]['pose']['resolved_stamp_ns'])

    def test_robot_invalid_source_qos_frames_clock_and_output_are_correlated_failures(self):
        fixture = self.fixture()
        fixture.ready()
        fixture.send(start(1, 1))
        peer = fixture.next('observation', 1)
        invalid = []
        for key, value in [('topic', '/other_tf'), ('message_type', 'std_msgs/msg/String'),
                           ('best_effort', False), ('transient_local', True), ('output', 'typed')]:
            config = robot_start(1, 1)['configuration']
            config[key] = value
            invalid.append(config)
        for settings in [dict(frames=[]), dict(frames=['base', 'base']), dict(frames=['/base']),
                         dict(frames=['x' * 257]), dict(frames=[f'link{i}' for i in range(129)]),
                         dict(clock_policy='unknown'), dict(static_tf_topic='/tf')]:
            invalid.append(robot_start(1, 1, **settings)['configuration'])
        config = spatial_start(1, 1)['configuration']
        config['robot'] = robot_start(1, 1)['configuration']['robot']
        invalid.append(config)
        for identity, config in enumerate(invalid, 2):
            fixture.send(dict(command='start', generation=7, request=identity, subscription=identity, configuration=config))
            error = fixture.next('request_error', identity)
            self.assertEqual(error['request'], identity)
            self.assertTrue(error['no_handle'])
        self.assertIsNone(fixture.process.poll())
        self.assertGreater(fixture.next('observation', 1)['sequence'], peer['sequence'])

    def test_camera_pair_reserves_two_readers_and_shares_tf_until_stop(self):
        fixture = self.fixture()
        fixture.ready()
        def camera(request, subscription):
            command = spatial_start(request, subscription)
            config = command['configuration']
            config.update(topic='/camera/image_rect', message_type='sensor_msgs/msg/Image',
                output={'camera_snapshot': dict(topic='/camera/camera_info', best_effort=True, transient_local=False)})
            return command
        fixture.send(camera(1, 1))
        sample = fixture.next('camera_snapshot_observation', 1)['sample']['sample']
        self.assertEqual((sample['status'], sample['frame']), ('waiting_info', None))
        self.assertEqual(sample['owner']['info']['topic'], '/camera/camera_info')
        fixture.send(camera(2, 2))
        self.assertTrue(fixture.next('request_error', 2)['no_handle'])
        for identity in range(3, 7):
            fixture.send(start(identity, identity))
            fixture.next('observation', identity)
        fixture.send(start(7, 7))
        fixture.next('started', 7)
        fixture.send(stop(8, 1))
        fixture.next('stopped', 1)
        fixture.send(camera(9, 9))
        self.assertEqual(fixture.next('camera_snapshot_observation', 9)['sequence'], 1)

    def test_camera_pending_cancel_and_invalid_info_source_require_explicit_restart(self):
        fixture = self.fixture('start-delay')
        fixture.ready()
        command = spatial_start(1, 1)
        command['configuration'].update(topic='/camera/image_rect', message_type='sensor_msgs/msg/Image',
            output={'camera_snapshot': dict(topic='/camera/camera_info', best_effort=True, transient_local=False)})
        fixture.send(command)
        fixture.send(stop(2, 1))
        fixture.next('stopped', 1)
        with self.assertRaises(queue.Empty):
            fixture.events.get(timeout=0.08)
        bad = json.loads(json.dumps(command))
        bad.update(request=3, subscription=3)
        bad['configuration']['output']['camera_snapshot']['topic'] = '/camera/image_rect'
        fixture.send(bad)
        self.assertTrue(fixture.next('request_error', 3)['no_handle'])
        command.update(request=4, subscription=4)
        fixture.send(command)
        self.assertEqual(fixture.next('camera_snapshot_observation', 4)['sequence'], 1)

    def test_calibrated_camera_fixture_captures_pixels_calibration_tf_and_stops_without_replay(self):
        fixture = self.fixture()
        graph = fixture.ready()
        self.assertIn(['/demo/calibrated/image_rect', ['sensor_msgs/msg/Image']], graph['topics'])
        fixture.send(start(1, 1))
        peer = fixture.next('observation', 1)
        command = spatial_start(2, 2)
        command['configuration'].update(topic='/demo/calibrated/image_rect', message_type='sensor_msgs/msg/Image',
            output={'camera_snapshot': dict(topic='/demo/calibrated/camera_info', best_effort=False, transient_local=False)})
        fixture.send(command)
        first = fixture.next('camera_snapshot_observation', 2)
        value = first['sample']['sample']
        self.assertEqual(value['status'], 'receiving')
        frame = value['frame']
        metadata = frame['image']['metadata']
        self.assertEqual((metadata['width'], metadata['height']), (64, 48))
        self.assertEqual(frame['calibration']['frame_id'], metadata['frame_id'])
        self.assertEqual(len(base64.b64decode(frame['image']['data_base64'])), metadata['step'] * metadata['height'])
        self.assertEqual(frame['transform']['resolved_stamp_ns'], metadata['header_stamp']['seconds'] * 1000000000 + metadata['header_stamp']['nanoseconds'])
        second = fixture.next('camera_snapshot_observation', 2)
        self.assertGreater(second['sequence'], first['sequence'])
        self.assertEqual(second['sample']['sample']['frame']['calibration_revision'], frame['calibration_revision'])
        for key in ('width', 'height', 'frame_id', 'k', 'r', 'p', 'binning_x', 'binning_y', 'roi'):
            self.assertEqual(second['sample']['sample']['frame']['calibration'][key], frame['calibration'][key])
        fixture.send(stop(3, 2))
        fixture.next('stopped', 2)
        self.assertGreater(fixture.next('observation', 1)['sequence'], peer['sequence'])
        fixture.send(command | dict(request=4, subscription=4))
        self.assertEqual(fixture.next('camera_snapshot_observation', 4)['sequence'], 1)

    def test_robot_shared_tf_readers_allow_six_peers_and_stop_preserves_them(self):
        fixture = self.fixture()
        fixture.ready()
        fixture.send(robot_start(1, 1))
        fixture.next('robot_observation', 1)
        for identity in range(2, 8):
            fixture.send(start(identity, identity))
            fixture.next('observation', identity)
        fixture.send(start(8, 8))
        fixture.next('started', 8)
        fixture.send(stop(9, 1))
        fixture.next('stopped', 1)
        for identity in range(2, 8):
            fixture.next('observation', identity)
        fixture.send(robot_start(10, 9))
        self.assertEqual(fixture.next('robot_observation', 9)['sequence'], 1)

    def test_robot_pending_group_rejects_second_robot_and_conflicting_scene_key_then_cancels_start(self):
        fixture = self.fixture('start-delay')
        fixture.ready()
        fixture.send(robot_start(1, 1))
        fixture.send(robot_start(2, 2))
        self.assertTrue(fixture.next('request_error', 2)['no_handle'])
        fixture.send(spatial_start(3, 3))
        self.assertTrue(fixture.next('request_error', 3)['no_handle'])
        fixture.send(stop(4, 1))
        fixture.next('stopped', 1)
        with self.assertRaises(queue.Empty):
            fixture.events.get(timeout=0.08)
        fixture.send(robot_start(5, 4))
        self.assertEqual(fixture.next('robot_observation', 4)['sequence'], 1)

    def test_robot_maximum_escaped_frames_and_reconnect_stay_explicit(self):
        fixture = self.fixture()
        fixture.ready()
        frames = [f'f{i:03}_' + '\\"' * 125 + 'q' for i in range(128)]
        command = robot_start(1, 1, frames=frames)
        self.assertTrue(all(len(frame.encode()) == 256 for frame in frames))
        self.assertGreater(len(json.dumps(command).encode()), 4096)
        self.assertLess(len(json.dumps(command).encode()), 128 * 1024)
        fixture.send(command)
        first = fixture.next('robot_observation', 1)
        self.assertEqual([link['frame'] for link in first['sample']['links']], frames)
        fixture.send(stop(2, 1))
        fixture.next('stopped', 1)
        with self.assertRaises(queue.Empty):
            fixture.events.get(timeout=0.15)
        fixture.process.stdin.close()
        self.assertEqual(fixture.process.wait(timeout=1), 0)
        replacement = self.fixture()
        replacement.ready()
        with self.assertRaises(queue.Empty):
            replacement.events.get(timeout=0.15)
        self.assertFalse((Path(replacement.directory.name) / 'commands.jsonl').exists())

    def test_shared_robot_two_scans_cloud_identity_peer_stop_and_replacement(self):
        fixture = self.fixture()
        graph = fixture.ready()
        self.assertIn(['/demo/cloud', ['sensor_msgs/msg/PointCloud2']], graph['topics'])
        with self.assertRaises(queue.Empty):
            fixture.events.get(timeout=0.08)
        fixture.send(robot_start(1, 1, dynamic_tf_best_effort=False))
        robot = fixture.next('robot_observation', 1)
        fixture.send(spatial_start(2, 2))
        scan_a = fixture.next('spatial_observation', 2)
        fixture.send(spatial_start(3, 3, topic='/demo/scan_secondary'))
        scan_b = fixture.next('spatial_observation', 3)
        fixture.send(cloud_start(4, 4))
        cloud = fixture.next('point_cloud_observation', 4)
        for packet, subscription, topic in [(scan_a, 2, '/demo/scan'),
                                             (scan_b, 3, '/demo/scan_secondary')]:
            self.assertEqual((packet['generation'], packet['subscription']), (7, subscription))
            self.assertEqual(packet['sample']['scan']['topic'], topic)
            self.assertEqual(packet['sample']['fixed_frame'], 'map')
            self.assertEqual(packet['sample']['scan']['message_type'], 'sensor_msgs/msg/LaserScan')
        sample = cloud['sample']
        self.assertEqual((cloud['generation'], cloud['subscription']), (7, 4))
        self.assertEqual((sample['contract_version'], sample['topic'], sample['message_type']),
                         (2, '/demo/cloud', 'sensor_msgs/msg/PointCloud2'))
        self.assertEqual((sample['fixed_frame'], sample['source_frame']), ('map', 'cloud'))
        self.assertGreater(sample['acquisition_stamp_ns'], 0)
        self.assertEqual((sample['source_points'], sample['skipped_points']), (5, 0))
        self.assertEqual(sample['channels']['color']['status'], 'values')
        self.assertEqual(len(sample['channels']['color']['values']), 5)
        self.assertEqual(sample['channels']['intensity'], dict(status='values', values=[0.,25.,50.,75.,None]))
        self.assertEqual(decode_points(sample['points']), [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.],
                                            [0., 0., 1.], [1., 1., 2.]])
        self.assertEqual(sample['tf'], {'status': 'ready', 'components': [0., 0., 0., 0., 0., 0., 1.]})
        self.assertEqual(sample['counters']['clouds_received'], cloud['sequence'])
        self.assertEqual(len(sample['counters']), 9)
        self.assertLess(len(json.dumps(sample).encode()), 512 * 1024)
        fixture.send(stop(5, 4))
        fixture.next('stopped', 4)
        for kind, subscription, previous in [('robot_observation', 1, robot),
                                               ('spatial_observation', 2, scan_a),
                                               ('spatial_observation', 3, scan_b)]:
            self.assertGreater(fixture.next(kind, subscription)['sequence'], previous['sequence'])
        fixture.send(cloud_start(6, 5, topic='/demo/cloud_replacement'))
        replacement = fixture.next('point_cloud_observation', 5)
        self.assertEqual(replacement['sequence'], 1)
        self.assertEqual(replacement['sample']['topic'], '/demo/cloud_replacement')
        fixture.send(stop(7, 2))
        fixture.next('stopped', 2)
        self.assertGreater(fixture.next('point_cloud_observation', 5)['sequence'], replacement['sequence'])
        self.assertGreater(fixture.next('spatial_observation', 3)['sequence'], scan_b['sequence'])
        remaining = []
        while not fixture.events.empty():
            remaining.append(fixture.events.get_nowait())
        self.assertFalse(any(packet.get('subscription') in (2, 4)
                             and packet['event'] in ('started', 'spatial_observation', 'point_cloud_observation')
                             for packet in remaining))

    def test_shared_scene_key_single_robot_four_sensors_and_independent_sensor_limits(self):
        fixture = self.fixture()
        fixture.ready()
        fixture.send(robot_start(1, 1, dynamic_tf_best_effort=False))
        first = fixture.next('robot_observation', 1)
        for identity, settings in enumerate([dict(fixed_frame='odom'), dict(dynamic_tf_topic='/other_tf'),
                                              dict(dynamic_tf_best_effort=True), dict(static_tf_topic='/other_static')], 2):
            fixture.send(cloud_start(identity, identity, **settings))
            self.assertTrue(fixture.next('request_error', identity)['no_handle'])
        fixture.send(robot_start(6, 6, dynamic_tf_best_effort=False))
        self.assertTrue(fixture.next('request_error', 6)['no_handle'])
        for identity, command, kind in [(7, spatial_start, 'spatial_observation'),
                                          (8, spatial_start, 'spatial_observation'),
                                          (9, cloud_start, 'point_cloud_observation'),
                                          (10, cloud_start, 'point_cloud_observation')]:
            fixture.send(command(identity, identity))
            fixture.next(kind, identity)
        # Four sensors and shared TF leave room for ordinary readers; the sensor cap is independent of those remaining native slots.
        fixture.send(cloud_start(11, 11))
        self.assertTrue(fixture.next('request_error', 11)['no_handle'])
        for identity in [12, 13]:
            fixture.send(start(identity, identity))
            fixture.next('observation', identity)
        fixture.send(start(14, 14))
        fixture.next('observation',14)
        self.assertGreater(fixture.next('robot_observation', 1)['sequence'], first['sequence'])
        fixture.send(stop(15, 9))
        fixture.next('stopped', 9)
        fixture.send(cloud_start(16, 15))
        self.assertEqual(fixture.next('point_cloud_observation', 15)['sequence'], 1)
        self.assertIsNone(fixture.process.poll())

    def test_pending_shared_scene_members_cancel_independently_and_last_stop_releases_key(self):
        fixture = self.fixture('start-delay')
        fixture.ready()
        fixture.send(robot_start(1, 1, dynamic_tf_best_effort=False))
        fixture.send(spatial_start(2, 2))
        fixture.send(cloud_start(3, 3))
        fixture.send(cloud_start(4, 4, fixed_frame='odom'))
        self.assertTrue(fixture.next('request_error', 4)['no_handle'])
        for request, subscription in [(5, 1), (6, 2), (7, 3)]:
            fixture.send(stop(request, subscription))
            fixture.next('stopped', subscription)
        with self.assertRaises(queue.Empty):
            fixture.events.get(timeout=0.08)
        fixture.send(cloud_start(8, 5, fixed_frame='odom'))
        packet = fixture.next('point_cloud_observation', 5)
        self.assertEqual((packet['sequence'], packet['sample']['fixed_frame']), (1, 'odom'))
        self.assertIsNone(fixture.process.poll())

    def test_terminal_drop_snapshots_precede_cleanup_without_reactivating_stopped_ids(self):
        fixture = self.fixture()
        fixture.ready()
        fixture.send(start(1, 1))
        fixture.next('observation', 1)
        fixture.send(stop(2, 1))
        self.assertEqual(fixture.next('drops', 1)['backend_drops'], 0)
        self.assertEqual(fixture.next()['event'], 'stopped')
        fixture.send(stop(3, 1))
        self.assertEqual(fixture.next()['event'], 'stopped')
        fixture.send(start(4, 2, '/demo/start_error'))
        self.assertEqual(fixture.next('drops', 2)['backend_drops'], 0)
        self.assertEqual(fixture.next()['event'], 'request_error')
        fixture.send(start(5, 3, '/demo/stream_error'))
        self.assertEqual(fixture.next('drops', 3)['backend_drops'], 0)
        self.assertEqual(fixture.next()['event'], 'stream_error')


if __name__ == '__main__':
    unittest.main()
