"""Development-only protocol fixture; never imports ROS or opens network sockets.

Only --observe-session selects persistent current session; legacy [TOPIC TYPE QoS...] is v2.
STUDIO_UI_FIXTURE_MODE: normal, delayed, start-delay, empty, waiting, error,
start-error, scoped-error, failure, incompatible, flood, retained, long, graph-delay,
spatial-missing, spatial-delay, spatial-timeout, spatial-error, robot-missing,
robot-delay (one leaf missing in the first four Robot batches, then recovery).
STUDIO_UI_FIXTURE_EVIDENCE retains bounded launch/command records on disk and
accepts exit-request. No launch or discovery event activates a subscription.
Development-only pause-map withholds Map samples; fail-map terminates only Map
streams. Removing a fault never starts a stopped stream; explicit Start is needed.
pause-map-start withholds Map Start acknowledgement while commands/peers progress.
"""
import json
from session_wire import VERSION as SESSION_VERSION, CAMERA_CAPABILITIES, camera_waiting
import camera_wire
from cloud_wire import encode_points
import marker_wire
import pose_wire
from pose_wire import CAPABILITIES as POSE_CAPABILITIES
import base64
import hashlib
import math
import struct
from types import SimpleNamespace
import os
import queue
import re
import runpy
import threading
from pathlib import Path
import sys
import time
import unicodedata

VERSION = 2
IMAGE_TYPE = 'sensor_msgs/msg/Image'
SCAN_TYPE = 'sensor_msgs/msg/LaserScan'
CLOUD_TYPE = 'sensor_msgs/msg/PointCloud2'
TF_TYPE = 'tf2_msgs/msg/TFMessage'
MAPPING_TYPES = {'occupancy_grid': 'nav_msgs/msg/OccupancyGrid',
                 'pose': 'geometry_msgs/msg/PoseStamped', 'path': 'nav_msgs/msg/Path'}
MAPPING_LAYOUT = json.loads((Path(__file__).resolve().parents[1] / 'fixtures/mapping_scene/alignment.json').read_text())
CAMERA_ENCODINGS = ('mono8', 'rgb8', 'bgr8', 'rgba8', 'bgra8')
CAMERA_MODEL = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'fixtures/simple_sim/model.py'))


def emit(event):
    print(json.dumps(event, ensure_ascii=False, allow_nan=False), flush=True)


def number(value, signed=False):
    if isinstance(value, int):
        return {'kind': 'signed' if signed or value < 0 else 'unsigned', 'value': str(value)}
    spell = 'nan' if math.isnan(value) else ('positive_infinity' if value > 0 else 'negative_infinity')
    return {'kind': 'float', 'value': {'kind': 'finite', 'value': repr(value)} if math.isfinite(value) else {'kind': spell}}


def text(value, original_bytes=None):
    return {'kind': 'text', 'value': value, 'original_bytes': len(value.encode()) if original_bytes is None else original_bytes}


def array(items, original_items=None):
    return {'kind': 'array', 'items': items, 'original_items': len(items) if original_items is None else original_items}


def obj(**fields):
    return {'kind': 'object', 'fields': [{'name': key, 'value': value} for key, value in fields.items()], 'original_fields': len(fields)}


def typed_event(topic, message_type, count):
    if message_type == 'nav_msgs/msg/Odometry':
        value = obj(header=obj(stamp=obj(sec=number(count // 5, signed=True), nanosec=number((count % 5) * 200000000)), frame_id=text('odom')),
                    child_frame_id=text('base_link'),
                    pose=obj(pose=obj(position=obj(x=number(math.sin(count / 10)), y=number(math.cos(count / 10)), z=number(0.0)),
                                      orientation=obj(x=number(0.0), y=number(0.0), z=number(0.0), w=number(1.0))),
                             covariance=array([number(0.0) for _ in range(36)])),
                    twist=obj(twist=obj(linear=obj(x=number(0.1 + math.sin(count / 10) / 20), y=number(0.0), z=number(0.0)),
                                        angular=obj(x=number(0.0), y=number(0.0), z=number(0.2))),
                              covariance=array([number(0.0) for _ in range(36)])))
    elif topic == '/demo/numeric_edges':
        value = obj(sequence=number(count), exact_u64=number(18446744073709551615), exact_i64=number(-9223372036854775808),
                    finite=number(math.sin(count / 10)), nonfinite=number([float('nan'), float('inf'), -float('inf')][count % 3]),
                    samples=array([number(1.25), number(-2.5)], 4), empty=array([]),
                    numeric_looking_text=text('123.456'), unicode=text('café 🤖 工作空间'), bounded_text=text('retained', 40000),
                    unsupported={'kind': 'unsupported', 'type_name': 'fixture/Unknown', 'reason': 'Synthetic unsupported value'},
                    nested=obj(enabled={'kind': 'bool', 'value': True}, **{'literal.field[0]': number(42)}))
    elif message_type == 'std_msgs/msg/Int32':
        value = obj(data=number(count, signed=True))
    else:
        value = obj(data=text(f'Synthetic message {count}. Read-only fixture; no ROS traffic.\nSecond line: café 🤖 机器人'))
    return {'event': 'typed_sample', 'sample': {'contract_version': VERSION, 'topic': topic,
            'message_type': message_type, 'source_time': None, 'value': value}, 'dropped_samples': 0}


def legacy_main(root, mode):
    if mode == 'delayed':
        time.sleep(5)
    emit({'event': 'hello', 'version': 1 if mode == 'incompatible' else VERSION, 'distro': 'jazzy', 'rmw': 'synthetic-fixture-no-ROS'})
    if mode == 'error':
        emit({'event': 'error', 'message': 'Synthetic subscription failure'})
        return
    topics = [
        ['/demo/odometry', ['nav_msgs/msg/Odometry']],
        ['/demo/numeric_edges', ['fixture/msg/NumericEdges']],
        ['/demo/status', ['std_msgs/msg/String']],
        ['/demo/multiple_types', ['std_msgs/msg/String', 'std_msgs/msg/Int32']],
        ['/demo/' + 'long_namespace/' * 8 + 'status', ['std_msgs/msg/String']],
    ]
    emit({'event': 'graph', 'topics': [] if mode == 'empty' else topics})
    count = 0
    while not (root / 'exit-request').exists():
        if len(sys.argv) >= 3 and mode not in ('empty', 'incompatible'):
            count += 1
            emit(typed_event(sys.argv[1], sys.argv[2], count))
        time.sleep(0.2)


def camera_sample(topic, count, compact):
    """Pure protocol data; exact complete compact bytes or bounded typed tree."""
    leaf = topic.rsplit('/', 1)[-1]
    encoding = leaf if leaf in CAMERA_ENCODINGS else 'rgb8'
    width, height = 64, 48
    size = os.environ.get('STUDIO_UI_FIXTURE_IMAGE_SIZE')
    if size and (leaf == 'rgb8' or topic == camera_wire.IMAGE_TOPIC):
        width, height = map(int, size.split('x'))
        if not (1 <= width <= 2048 and 1 <= height <= 2048):
            raise ValueError('Synthetic image dimensions exceed the component bound')
    if leaf == 'format_changes':
        encoding = CAMERA_ENCODINGS[(count - 1) % len(CAMERA_ENCODINGS)]
        width, height = (64, 48) if count % 2 else (32, 24)
    if leaf in ('depth16', 'depth32'):
        width, height = 640, 480
        encoding = '16UC1' if leaf == 'depth16' else '32FC1'
        row = b''.join(struct.pack('<H', x * 10000 // width) if leaf == 'depth16' else struct.pack('<f', x * 10. / width) for x in range(width))
        frame = SimpleNamespace(data=row * height, step=len(row), frame_id='camera_optical')
    else:
        frame = CAMERA_MODEL['camera'](count, width=width, height=height, encoding=encoding, padding=2)
    raw, step = frame.data, frame.step
    if leaf == 'unsupported':
        encoding, step = '16SC1', width * 2 + 2
        raw = b'\0' * (step * height)
    stamp = {'seconds': count // 10, 'nanoseconds': (count % 10) * 100000000}
    metadata = dict(header_stamp=stamp, frame_id=frame.frame_id, width=width, height=height,
                    encoding=encoding, is_bigendian=False if leaf in ('depth16', 'depth32') else count % 2 == 0, step=step)
    if compact:
        sample = dict(contract_version=2, topic=topic, message_type=IMAGE_TYPE, metadata=metadata,
                      original_bytes=len(raw), complete=True, data_base64=base64.b64encode(raw).decode('ascii'))
        if leaf == 'malformed':
            sample['original_bytes'] += 1  # Deliberate invalid peer wire, not a local rendering error.
    else:
        value = obj(header=obj(stamp=obj(sec=number(stamp['seconds'], signed=True), nanosec=number(stamp['nanoseconds'])),
                               frame_id=text(frame.frame_id)), width=number(width), height=number(height),
                    encoding=text(encoding), is_bigendian=number(int(metadata['is_bigendian'])), step=number(step),
                    data=array([number(byte) for byte in raw[:2048]], len(raw)))
        sample = dict(contract_version=2, topic=topic, message_type=IMAGE_TYPE, source_time=None, value=value)
    if len(raw) > 16 * 1024 * 1024 or len(json.dumps(sample, ensure_ascii=False).encode()) > (24 * 1024 * 1024 if compact else MAX_SAMPLE):
        raise ValueError('Synthetic camera exceeds raw/sample budget')
    return sample


def scan_sample(topic, acquisition):
    seconds, nanos = 10 + acquisition // 5, (acquisition % 5) * 200000000
    increment = 2 * math.pi / 180
    value = obj(header=obj(stamp=obj(sec=number(seconds, signed=True), nanosec=number(nanos)), frame_id=text('laser')),
                angle_min=number(-math.pi), angle_max=number(-math.pi + 179 * increment), angle_increment=number(increment),
                time_increment=number(0.0), scan_time=number(0.2), range_min=number(0.05), range_max=number(10.0),
                ranges=array([number(value) for value in CAMERA_MODEL['scan'](0, 0, 0)]), intensities=array([]))
    return dict(contract_version=2, topic=topic, message_type=SCAN_TYPE, source_time=None, value=value)


def tf_sample(topic, count):
    static = topic == '/tf_static'
    seconds, nanos = (0, 0) if static else (10 + count // 5, (count % 5) * 200000000)
    value = obj(transforms=array([obj(
        header=obj(stamp=obj(sec=number(seconds, signed=True), nanosec=number(nanos)), frame_id=text('base' if static else 'map')),
        child_frame_id=text('laser' if static else 'base'),
        transform=obj(translation=obj(x=number(0.0 if static else 1.0), y=number(0.0 if static else 2.0), z=number(0.25 if static else 0.0)),
                      rotation=obj(x=number(0.0), y=number(0.0), z=number(0.0 if static else math.sqrt(0.5)), w=number(1.0 if static else math.sqrt(0.5)))))]))
    return dict(contract_version=2, topic=topic, message_type=TF_TYPE, source_time=None, value=value)


SPATIAL_COUNTER_NAMES = ('scans_received', 'pending_replaced', 'lookup_timeouts', 'tf_dynamic_batches',
                         'tf_static_batches', 'tf_accepted', 'tf_rejected', 'duplicate_statics', 'native_errors')


def robot_sample(stream, mode):
    """Complete latest-result batch; synthetic TF, never scan-time provenance."""
    config = stream['command']['configuration']['robot']
    count = stream['count'] + 1
    known = (config['fixed_frame'] == 'map' and config['dynamic_tf_topic'] == '/tf'
             and config['static_tf_topic'] == '/tf_static')
    absent = 'tip' if 'tip' in config['frames'] else config['frames'][-1]
    angle = 0.4 * math.sin(count / 10)
    rotation = [0.0, math.sin(angle / 2), 0.0, math.cos(angle / 2)]
    links = []
    for index, frame in enumerate(config['frames']):
        pose = {'status': 'missing'}
        partial = frame == absent and (mode == 'robot-missing' or (mode == 'robot-delay' and count <= 4))
        if frame == config['fixed_frame']:
            pose = dict(status='ready', resolved_stamp_ns=0, components=[0., 0., 0., 0., 0., 0., 1.])
        elif known and not partial and frame in {'base', 'arm', 'tip', 'ascii_mesh', 'binary_mesh'}:
            translation = [0., 0., 0.]
            quaternion, stamp = [0., 0., 0., 1.], 0
            if frame in {'arm', 'tip'}:
                quaternion = rotation
                translation = [0., 0., 0.2] if frame == 'arm' else [0.6 * math.sin(angle), 0., 0.2 + 0.6 * math.cos(angle)]
                stamp = 20_000_000_000 + count * 100_000_000 + index
            pose = dict(status='ready', resolved_stamp_ns=stamp, components=translation + quaternion)
        links.append(dict(frame=frame, pose=pose))
    sample = dict(contract_version=1, fixed_frame=config['fixed_frame'], policy='latest_available', links=links,
                  counters=dict(tf_dynamic_batches=count, tf_static_batches=1, tf_accepted=3 + count * 2,
                                tf_rejected=0, duplicate_statics=0, native_errors=0, pose_batches=count))
    if len(json.dumps(sample, allow_nan=False).encode()) > MAX_SAMPLE:
        raise ValueError('Synthetic Robot sample exceeds byte budget')
    return sample


def spatial_sample(stream, now, mode):
    config = stream['command']['configuration']
    settings = config['spatial']
    counters = stream['spatial_counters']
    if mode == 'spatial-timeout' and stream.get('timed_out'):
        return None
    if stream.get('pending_scan') is None:
        stream['acquisition'] = stream.get('acquisition', 0) + 1
        stream['pending_scan'] = scan_sample(config['topic'], stream['acquisition'])
        stream['pending_at'] = now
        stream['missing_sent'] = False
        counters['scans_received'] += 1
    known = settings['fixed_frame'] == 'map' and settings['dynamic_tf_topic'] == '/tf' and settings['static_tf_topic'] == '/tf_static'
    missing = not known or mode in ('spatial-missing', 'spatial-timeout')
    delayed = mode == 'spatial-delay' and stream['acquisition'] == 1 and now - stream['pending_at'] < 0.35
    if settings['static_tf_topic'] == '/tf_static' and not counters['tf_static_batches']:
        counters['tf_static_batches'] = 1
        counters['tf_accepted'] += 1
    if missing or delayed:
        if stream['missing_sent'] and now - stream['pending_at'] < 1.0:
            stream['next_at'] = now + 0.05
            return None
        status = {'status': 'missing'} if not stream['missing_sent'] else {'status': 'lookup_timeout'}
        if status['status'] == 'lookup_timeout':
            counters['lookup_timeouts'] += 1
            stream['timed_out'] = mode == 'spatial-timeout'
        stream['missing_sent'] = True
    else:
        status = {'status': 'ready', 'components': [1.0, 2.0, 0.25, 0.0, 0.0, math.sqrt(0.5), math.sqrt(0.5)]}
        counters['tf_dynamic_batches'] += 1
        counters['tf_accepted'] += 1
    scan = stream['pending_scan']
    acquisition = stream['acquisition']
    sample = dict(contract_version=1, scan=scan, fixed_frame=settings['fixed_frame'], source_frame='laser',
                  acquisition_stamp_ns=(10 + acquisition // 5) * 1000000000 + (acquisition % 5) * 200000000,
                  tf=status, counters=dict(counters))
    if status['status'] in ('ready', 'lookup_timeout'):
        stream['pending_scan'] = None
        stream['next_at'] = now + (0.0 if mode == 'flood' else 0.2)
    else:
        stream['next_at'] = now + 0.05
    if len(json.dumps(sample, allow_nan=False).encode()) > MAX_SAMPLE:
        raise ValueError('Synthetic Spatial sample exceeds byte budget')
    return sample


def emit_drops(generation, subscription, stream=None, configuration=None):
    config = configuration or (stream['command']['configuration'] if stream is not None else {})
    counters = dict(stream['spatial_counters']) if stream is not None and stream.get('spatial_counters') is not None else None
    if counters is None and config.get('output') == 'spatial_scan':
        counters = dict.fromkeys(SPATIAL_COUNTER_NAMES, 0)
    emit(dict(event='drops', generation=generation, subscription=subscription, backend_drops=0, spatial_counters=counters))


TOPICS = [
    ['/demo/odometry', ['nav_msgs/msg/Odometry']],
    ['/demo/numeric_edges', ['fixture/msg/NumericEdges']],
    ['/demo/status', ['std_msgs/msg/String']],
    ['/demo/robot_description', ['std_msgs/msg/String']],
    ['/demo/slow', ['std_msgs/msg/Int32']],
    ['/demo/retained', ['std_msgs/msg/String']],
    ['/demo/multiple_types', ['std_msgs/msg/String', 'std_msgs/msg/Int32']],
    ['/demo/' + 'long_namespace/' * 12 + 'status', ['std_msgs/msg/String']],
    ['/demo/start_error', ['std_msgs/msg/String']],
    ['/demo/stream_error', ['std_msgs/msg/String']],
]
TOPICS += [['/demo/camera/' + name, [IMAGE_TYPE]]
           for name in (*CAMERA_ENCODINGS, 'depth16', 'depth32', 'unsupported', 'format_changes', 'malformed')]
TOPICS += [['/demo/cloud', [CLOUD_TYPE]], ['/demo/scan', [SCAN_TYPE]], ['/tf', [TF_TYPE]], ['/tf_static', [TF_TYPE]]]
TOPICS += [['/demo/' + ('map' if output == 'occupancy_grid' else output), [typ]] for output, typ in MAPPING_TYPES.items()]
TOPICS += [['/demo/' + kind, [typ]] for kind, typ in pose_wire.TYPES.items()]
TOPICS += [[camera_wire.IMAGE_TOPIC, [IMAGE_TYPE]], [camera_wire.INFO_TOPIC, ['sensor_msgs/msg/CameraInfo']]]
TOPICS += [[f'/images/image{i}', [IMAGE_TYPE]] for i in range(33)]
TOPICS += [[topic, ['nav_msgs/msg/OccupancyGrid']] for topic in ('/global_costmap/costmap', '/local_costmap/costmap')]
TOPICS += [['/demo/map_updates', ['map_msgs/msg/OccupancyGridUpdate']]]
TOPICS += [['/demo/markers', ['visualization_msgs/msg/Marker']], ['/demo/marker_array', ['visualization_msgs/msg/MarkerArray']]]
TOPICS += [['/demo/' + 'costmap_updates_namespace/' * 8 + 'updates', ['map_msgs/msg/OccupancyGridUpdate']]]
MODES = {'normal', 'delayed', 'graph-delay', 'start-delay', 'empty', 'waiting', 'error',
         'start-error', 'scoped-error', 'failure', 'incompatible', 'flood', 'retained', 'long',
         'spatial-missing', 'spatial-delay', 'spatial-timeout', 'spatial-error', 'robot-missing', 'robot-delay',
         'tf-empty', 'tf-stale', 'tf-missing',
         'description-max', 'description-wait', 'description-bad-digest', 'description-gap', 'description-oversize',
         'mapping-missing', 'mapping-delay', 'mapping-empty', 'mapping-large', 'mapping-malformed', 'mapping-error'}
MAX_COMMAND = 128 * 1024
MAX_SAMPLE = 512 * 1024
MAX_U64 = (1 << 64) - 1


def strict_members(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('Duplicate command member')
        result[key] = value
    return result


def output_kind(config):
    output = config.get('output')
    if isinstance(output, dict):
        return next(iter(output)) if len(output) == 1 and next(iter(output)) in ('pose_snapshot', 'camera_snapshot') else None
    return output


def decode_command(line):
    if len(line) > MAX_COMMAND or not line.endswith(b'\n'):
        raise ValueError('Command exceeds 128 KiB or lacks newline')
    command = json.loads(line, object_pairs_hook=strict_members)
    if not isinstance(command, dict):
        raise ValueError('Command must be an object')
    expected = {'command', 'generation', 'request', 'subscription'}
    if command.get('command') == 'start':
        expected.add('configuration')
        config = command.get('configuration')
        if (not isinstance(config, dict) or not {'topic', 'message_type', 'best_effort', 'transient_local', 'output'} <= set(config)
                or set(config) - {'topic', 'message_type', 'best_effort', 'transient_local', 'output', 'spatial', 'robot', 'map_updates'}
                or any(type(config[k]) is not bool for k in ('best_effort', 'transient_local'))
                or any(not isinstance(config[k], str) for k in ('topic', 'message_type'))
                or output_kind(config) not in ('typed', 'compact_image', 'spatial_scan', 'point_cloud', 'robot', 'tf_graph', 'robot_description', 'map_snapshot', 'marker_snapshot', 'pose_snapshot', 'camera_snapshot', *MAPPING_TYPES)):
            raise ValueError('Invalid configuration grammar')
        if output_kind(config) == 'pose_snapshot':
            if pose_wire.configuration(config['output']) is None:
                raise ValueError('Pose output requires its explicit kind/history configuration')
        if output_kind(config) == 'camera_snapshot':
            info = config['output']['camera_snapshot']
            if (not isinstance(info, dict) or set(info) != {'topic', 'best_effort', 'transient_local'}
                    or not isinstance(info['topic'], str)
                    or any(type(info[k]) is not bool for k in ('best_effort', 'transient_local'))):
                raise ValueError('Invalid CameraInfo source grammar')
        updates = config.get('map_updates')
        if updates is not None and (not isinstance(updates, dict)
                or set(updates) != {'topic', 'best_effort', 'transient_local'}
                or not isinstance(updates['topic'], str)
                or any(type(updates[key]) is not bool for key in ('best_effort', 'transient_local'))):
            raise ValueError('Invalid map update source grammar')
        settings = config.get('spatial')
        if settings is not None and (not isinstance(settings, dict)
                or set(settings) != {'fixed_frame', 'clock_policy', 'dynamic_tf_topic', 'dynamic_tf_best_effort', 'static_tf_topic'}
                or any(not isinstance(settings[key], str) for key in ('fixed_frame', 'clock_policy', 'dynamic_tf_topic', 'static_tf_topic'))
                or settings['clock_policy'] not in ('unknown', 'verified_system', 'simulated')
                or type(settings['dynamic_tf_best_effort']) is not bool):
            raise ValueError('Invalid Spatial configuration grammar')
        robot = config.get('robot')
        if robot is not None and (not isinstance(robot, dict)
                or set(robot) != {'fixed_frame', 'frames', 'clock_policy', 'dynamic_tf_topic', 'dynamic_tf_best_effort', 'static_tf_topic'}
                or any(not isinstance(robot[key], str) for key in ('fixed_frame', 'clock_policy', 'dynamic_tf_topic', 'static_tf_topic'))
                or not isinstance(robot['frames'], list) or any(not isinstance(frame, str) for frame in robot['frames'])
                or robot['clock_policy'] not in ('unknown', 'verified_system', 'simulated')
                or type(robot['dynamic_tf_best_effort']) is not bool):
            raise ValueError('Invalid Robot configuration grammar')
    elif command.get('command') == 'tool':
        expected.add('operation')
        if command.get('operation') != {'kind': 'graph', 'section': 'Topics', 'node': ''} or command.get('request') != command.get('subscription'):
            raise ValueError('Only read-only Topics graph requests are supported by this fixture')
    elif command.get('command') not in ('stop', 'clear_markers', 'clear_pose'):
        raise ValueError('Only observe Start/Stop/ClearMarkers commands are supported')
    if set(command) != expected:
        raise ValueError('Unknown or missing command member')
    for key in ('generation', 'request', 'subscription'):
        if type(command[key]) is not int or not 0 < command[key] <= MAX_U64:
            raise ValueError('Observation IDs must be nonzero u64')
    return command


def config_valid(config):
    token = r'[A-Za-z_][A-Za-z_0-9]*'
    updates = config.get('map_updates')
    if updates is not None and (output_kind(config) != 'map_snapshot' or updates['topic'] == config['topic']
            or len(updates['topic'].encode()) > 256
            or re.fullmatch(r'/(?:' + token + r'/)*' + token, updates['topic']) is None):
        return False
    settings = config.get('spatial')
    if output_kind(config) in ('spatial_scan', 'point_cloud', 'tf_graph', 'map_snapshot', 'marker_snapshot', 'pose_snapshot', 'camera_snapshot', *MAPPING_TYPES):
        expected = MAPPING_TYPES.get(output_kind(config), 'nav_msgs/msg/OccupancyGrid' if output_kind(config) == 'map_snapshot' else TF_TYPE if output_kind(config) == 'tf_graph' else SCAN_TYPE if output_kind(config) == 'spatial_scan' else CLOUD_TYPE)
        if output_kind(config) == 'camera_snapshot':
            expected = IMAGE_TYPE
            info = config['output']['camera_snapshot']
            if (info['topic'] == config['topic'] or len(info['topic'].encode()) > 256
                    or re.fullmatch(r'/(?:' + token + r'/)*' + token, info['topic']) is None):
                return False
        if output_kind(config) == 'pose_snapshot':
            expected = pose_wire.TYPES[pose_wire.configuration(config['output'])['kind']]
        if output_kind(config) == 'marker_snapshot':
            expected = config['message_type'] if config['message_type'] in ('visualization_msgs/msg/Marker', 'visualization_msgs/msg/MarkerArray') else None
        if settings is None or config['message_type'] != expected:
            return False
        frame = settings['fixed_frame']
        def valid_topic(topic):
            return len(topic.encode()) <= 256 and re.fullmatch(r'/(?:' + token + r'/)*' + token, topic) is not None
        if (not frame or len(frame.encode()) > 256 or frame.startswith('/')
                or any(char.isspace() or ord(char) < 32 or 127 <= ord(char) <= 159 for char in frame)
                or settings['clock_policy'] != 'verified_system'
                or not valid_topic(settings['dynamic_tf_topic']) or not valid_topic(settings['static_tf_topic'])
                or settings['dynamic_tf_topic'] == settings['static_tf_topic']):
            return False
    elif settings is not None:
        return False
    if output_kind(config) == 'tf_graph' and (config['topic'] != settings['dynamic_tf_topic'] or config['best_effort'] != settings['dynamic_tf_best_effort'] or config['transient_local']):
        return False
    robot = config.get('robot')
    if output_kind(config) == 'robot':
        def valid_frame(frame):
            return (0 < len(frame.encode()) <= 256 and not frame.startswith('/')
                    and not any(char.isspace() or unicodedata.category(char) == 'Cc' for char in frame))
        def valid_topic(topic):
            return len(topic.encode()) <= 256 and re.fullmatch(r'/(?:' + token + r'/)*' + token, topic) is not None
        if (robot is None or not valid_frame(robot['fixed_frame'])
                or not 1 <= len(robot['frames']) <= 128 or len(set(robot['frames'])) != len(robot['frames'])
                or any(not valid_frame(frame) for frame in robot['frames'])
                or robot['clock_policy'] != 'verified_system'
                or not valid_topic(robot['dynamic_tf_topic']) or not valid_topic(robot['static_tf_topic'])
                or robot['dynamic_tf_topic'] == robot['static_tf_topic']
                or config['topic'] != robot['dynamic_tf_topic'] or config['message_type'] != TF_TYPE
                or config['best_effort'] != robot['dynamic_tf_best_effort'] or config['transient_local']):
            return False
    elif robot is not None:
        return False
    return (len(config['topic'].encode()) <= 256 and len(config['message_type'].encode()) <= 256
            and re.fullmatch(r'/(?:' + token + r'/)*' + token, config['topic']) is not None
            and re.fullmatch(token + '/msg/' + token, config['message_type']) is not None
            and (output_kind(config) != 'compact_image' or config['message_type'] == IMAGE_TYPE)
            and (output_kind(config) != 'robot_description' or config['message_type'] == 'std_msgs/msg/String'))


def read_commands(commands, terminal):
    """Bounded ingress; terminal faults/EOF have a reserved slot, never sensor traffic."""
    try:
        pending = bytearray()
        while True:
            # os.read avoids a daemon holding BufferedReader's lock during
            # interpreter shutdown after a deliberate session/global failure.
            chunk = os.read(sys.stdin.fileno(), MAX_COMMAND)
            if not chunk:
                if pending:
                    raise ValueError('Command lacks newline')
                terminal.put(None)
                return
            pending.extend(chunk)
            while b'\n' in pending:
                end = pending.index(b'\n') + 1
                line = bytes(pending[:end])
                del pending[:end]
                commands.put(decode_command(line))
            if len(pending) >= MAX_COMMAND:
                raise ValueError('Command exceeds 128 KiB or lacks newline')
    except (ValueError, UnicodeError, OSError, RecursionError) as error:
        terminal.put(str(error)[:400])


def request_error(command, message, no_handle):
    emit({'event': 'request_error', **{key: command[key] for key in ('generation', 'request', 'subscription')},
          'message': message, 'no_handle': no_handle})


def scene_key(config):
    settings = config.get('spatial') or config.get('robot')
    if settings is None:
        return None
    return tuple(settings[k] for k in ('fixed_frame', 'dynamic_tf_topic', 'dynamic_tf_best_effort', 'static_tf_topic'))


def resources_valid(configurations):
    scene = [c for c in configurations if scene_key(c) is not None]
    return (len(configurations) <= 32
            and sum(output_kind(c) not in ('robot', 'tf_graph') for c in configurations) + sum(c.get('map_updates') is not None for c in configurations) + sum(output_kind(c) == 'camera_snapshot' for c in configurations) + (2 if scene else 0) <= 64
            and sum(output_kind(c) == 'camera_snapshot' for c in configurations) <= 1
            and sum(output_kind(c) == 'robot' for c in configurations) <= 1
            and sum(output_kind(c) == 'tf_graph' for c in configurations) <= 1
            and sum(output_kind(c) == 'robot_description' for c in configurations) <= 1
            and sum(output_kind(c) == 'marker_snapshot' for c in configurations) <= 2
            and sum(output_kind(c) in ('spatial_scan', 'point_cloud') for c in configurations) <= 4
            and all(sum(output_kind(c) == kind for c in configurations) <= 1 for kind in MAPPING_TYPES if kind != 'occupancy_grid')
            and sum(output_kind(c) in ('occupancy_grid', 'map_snapshot') for c in configurations) <= 3
            and all(sum(output_kind(c) == 'pose_snapshot' and pose_wire.configuration(c['output'])['kind'] == kind for c in configurations) <= 1 for kind in pose_wire.TYPES)
            and len({scene_key(c) for c in scene}) <= 1)


def map_snapshot_sample(stream, mode, repeat=False):
    """Strict snapshot2 protocol fixture only; never native ROS/TF proof."""
    config = stream['command']['configuration']
    if 'map_data' not in stream:
        size = 2048 if mode == 'mapping-large' else 16
        if mode == 'mapping-large':
            # Visible at Fit scale: one-cell markings become subpixel at2048².
            # Asymmetric bottom/top values establish row orientation separately
            # from the unknown right edge and near-occupied centre stripe.
            band = size // 32
            raw = bytes(255 if x >= size - band else 100 if x < band or y < band
                        else 50 if y >= size - band else 99 if size // 2 <= x < size // 2 + band
                        else 0 for y in range(size) for x in range(size))
        else:
            raw = bytes(255 if x == size - 1 else 100 if x == 0 or y in (0, size - 1)
                        else 99 if x == size // 2 else 0 for y in range(size) for x in range(size))
        stream['map_data'] = (raw, hashlib.sha256(raw).hexdigest(), dict(frame_id='map', stamp_ns=time.time_ns(),
            width=size, height=size, resolution=0.5, origin=[-4. + {'/global_costmap/costmap': 10., '/local_costmap/costmap': 20.}.get(config['topic'], 0.), -4., 0., 0., 0., 0., 1.], map_load_time_ns=0))
        stream['map_cursor'] = 0
        stream['map_revision'] = 1
        stream['map_full_count'] = 1
        stream['map_next_full_at'] = time.monotonic() + 1.0
    raw, digest, metadata = stream['map_data']
    owner = {key: stream['command'][key] for key in ('generation', 'subscription')}
    owner.update(topic=config['topic'], fixed_frame=config['spatial']['fixed_frame'])
    updates = config.get('map_updates') is not None
    status = dict(continuity='unknown' if updates else 'disabled', full_loss_events=False,
                  update_loss_events=False if updates else None, recovery_floor_ns=None,
                  ingress_counters=dict.fromkeys(('stale_events', 'sequence_faults', 'publisher_changes',
                      'reported_lost', 'loss_counter_resets', 'rejected_recovery_full'), 0))
    reason = ('waiting_full' if mode == 'mapping-empty' else 'invalid_input' if mode == 'mapping-malformed'
              else 'missing_transform' if mode in ('mapping-missing', 'spatial-missing')
              or (mode in ('mapping-delay', 'spatial-delay') and stream['count'] < 4) else None)
    if reason:
        if stream['count'] and mode not in ('mapping-delay', 'spatial-delay'):
            return None
        revision = stream['map_revision']
        stream['map_revision'] += 1
        return dict(kind='clear', contract_version=2, owner=owner, revision=revision, reason=reason,
                    metadata=metadata if reason == 'missing_transform' else None, source_status=status)
    if mode == 'retained' and not config['transient_local']:
        return None
    index = stream['map_cursor']
    count = (len(raw) + 196607) // 196608
    if index >= count:
        if not repeat or time.monotonic() < stream['map_next_full_at']:
            return None
        metadata = dict(metadata, stamp_ns=max(time.time_ns(), metadata['stamp_ns'] + 1))
        stream['map_data'] = (raw, digest, metadata)
        stream['map_revision'] += 1
        stream['map_full_count'] += 1
        stream['map_cursor'] = index = 0
        stream['map_next_full_at'] = time.monotonic() + 1.0
    header = dict(contract_version=2, owner=owner, revision=stream['map_revision'], metadata=metadata,
                  transform=[0., 0., 0., 0., 0., 0., 1.], sha256=digest, source_status=status,
                  source_counters=dict.fromkeys(('messages_received', 'pending_replaced', 'lookup_timeouts',
                      'tf_dynamic_batches', 'tf_static_batches', 'tf_accepted', 'tf_rejected', 'duplicate_statics', 'native_errors'), 0),
                  assembly_counters=dict(full_maps=stream['map_full_count'], patches=0, stale_inputs=0, updates_without_base=0, invalidations=0))
    stream['map_cursor'] += 1
    return dict(kind='chunk', header=header, index=index, count=count,
                data_base64=base64.b64encode(raw[index * 196608:(index + 1) * 196608]).decode())


def tf_graph_sample(stream, mode):
    """Deterministic graph fixture; never native TF/ROS acceptance evidence."""
    scene = stream['command']['configuration']['spatial']
    count = stream['count'] + 1
    stamp = 20_000_000_000 + count * 200_000_000
    known = (scene['fixed_frame'] == 'map' and scene['dynamic_tf_topic'] == '/tf'
             and scene['static_tf_topic'] == '/tf_static' and mode != 'tf-missing')
    def pose(name):
        if name in ('orphan', 'remote') or not known:
            return dict(status='missing')
        if name in ('base', 'laser') and mode == 'tf-stale':
            return dict(status='stale')
        return dict(status='ready', resolved_stamp_ns=0 if name == 'map' else stamp,
                    components=[1. if name != 'map' else 0., 2. if name != 'map' else 0.,
                                .25 if name == 'laser' else 0., 0., 0., 0., 1.])
    frames = [dict(name='base', parent=dict(frame='map', kind='dynamic', stamp_ns=stamp), pose=pose('base')),
              dict(name='laser', parent=dict(frame='base', kind='static', stamp_ns=0), pose=pose('laser')),
              dict(name='map', parent=None, pose=pose('map')),
              dict(name='orphan', parent=None, pose=pose('orphan')),
              dict(name='remote', parent=dict(frame='orphan', kind='static', stamp_ns=0), pose=pose('remote'))]
    if mode == 'tf-empty':
        frames = []
    counters = dict.fromkeys(SPATIAL_COUNTER_NAMES, 0)
    counters.update(tf_accepted=3 + count, tf_static_batches=1, tf_dynamic_batches=count)
    sample = dict(contract_version=1, fixed_frame=scene['fixed_frame'], frames=frames, counters=counters)
    if len(json.dumps(sample, allow_nan=False).encode()) > MAX_SAMPLE:
        raise ValueError('Synthetic TF graph exceeds sample budget')
    return sample


def cloud_sample(stream):
    config = stream['command']['configuration']
    counters = dict.fromkeys(('clouds_received', 'pending_replaced', 'lookup_timeouts',
        'tf_dynamic_batches', 'tf_static_batches', 'tf_accepted', 'tf_rejected', 'duplicate_statics', 'native_errors'), 0)
    counters['clouds_received'] = stream['count']
    return dict(contract_version=2, topic=config['topic'], message_type=CLOUD_TYPE,
        fixed_frame=config['spatial']['fixed_frame'], source_frame='cloud', acquisition_stamp_ns=time.time_ns(),
        source_points=5, skipped_points=0, points=encode_points([[0.,0.,0.], [1.,0.,0.], [0.,1.,0.], [0.,0.,1.], [1.,1.,2.]]),
        channels=dict(color=dict(status="values", values=[[240,70,50,255],[60,200,90,255],[70,100,230,128],[230,190,60,255],[170,90,220,255]]), intensity=dict(status="values", values=[0.,25.,50.,75.,None])),
        tf={'status':'ready', 'components':[0.,0.,0.,0.,0.,0.,1.]}, counters=counters)


def mapping_sample(stream, mode):
    """Bounded original mapping geometry; synthetic acquisition TF only."""
    config = stream['command']['configuration']
    settings, count = config['spatial'], stream['count']
    q = math.sqrt(.5)
    output = output_kind(config)
    if output == 'occupancy_grid':
        grid = MAPPING_LAYOUT['map']
        large, empty = mode == 'mapping-large', mode == 'mapping-empty'
        geometry = dict(kind=output, width=0 if empty else 256 if large else grid['width'],
                        height=0 if empty else 256 if large else grid['height'],
                        resolution=float(grid['resolution']), origin=[*grid['origin_translation'], 0., 0., q, q],
                        map_load_time_ns=0, cells=[] if empty else [i % 101 for i in range(65536)] if large else list(grid['cells']))
        if mode == 'mapping-malformed':
            geometry['cells'][0] = 101
    elif output == 'pose':
        geometry = dict(kind=output, components=[*MAPPING_LAYOUT['pose_source'], 0., 0., q, q])
        if mode == 'mapping-malformed':
            geometry['components'][3:] = [0.] * 4
    else:
        positions = [] if mode == 'mapping-empty' else [[i / 1024, 0., 0.] for i in range(1024)] if mode == 'mapping-large' else MAPPING_LAYOUT['path_source']
        geometry = dict(kind=output, poses=[dict(frame_id='', stamp_ns=0, components=[*point, 0., 0., 0., 1.]) for point in positions])
        if mode == 'mapping-malformed':
            geometry['poses'][0]['frame_id'] = 'contradictory_frame'
    identity = settings['fixed_frame'] == 'mapping_source'
    known = (settings['fixed_frame'] == 'map' and settings['dynamic_tf_topic'] == '/tf' and settings['static_tf_topic'] == '/tf_static')
    missing = not known or mode in ('mapping-missing', 'spatial-missing') or (mode in ('mapping-delay', 'spatial-delay') and count <= 4)
    tf = dict(status='ready', components=[0., 0., 0., 0., 0., 0., 1.]) if identity else dict(status='missing') if missing else dict(status='ready', components=[1., 2., 3., 0., 0., q, q])
    sample = dict(contract_version=1, topic=config['topic'], message_type=config['message_type'],
                  fixed_frame=settings['fixed_frame'], source_frame='mapping_source',
                  acquisition_stamp_ns=20_000_000_000 + count * 200_000_000, geometry=geometry, tf=tf,
                  counters=dict(messages_received=count, pending_replaced=0, lookup_timeouts=0,
                                tf_dynamic_batches=0 if identity else count, tf_static_batches=0 if identity else 1,
                                tf_accepted=0 if identity else count + 1, tf_rejected=0, duplicate_statics=0, native_errors=0))
    if len(json.dumps(sample, allow_nan=False).encode()) > MAX_SAMPLE:
        raise ValueError('Synthetic mapping sample exceeds byte budget')
    return sample


def session_main(root, mode):
    commands, terminal = queue.Queue(maxsize=16), queue.Queue(maxsize=1)
    threading.Thread(target=read_commands, args=(commands, terminal), daemon=True).start()
    # Cancellable handshake delay: stdin EOF and the explicit exit marker terminate startup.
    ready_at = time.monotonic() + (5 if mode == 'delayed' else 0)
    while time.monotonic() < ready_at:
        if not terminal.empty() or (root / 'exit-request').exists():
            return
        time.sleep(0.01)
    emit({'event': 'ready', 'version': SESSION_VERSION - 1 if mode == 'incompatible' else SESSION_VERSION,
          'camera_snapshot': CAMERA_CAPABILITIES, 'marker_snapshot': marker_wire.CAPABILITIES, 'pose_snapshot': POSE_CAPABILITIES,
          'distro': 'jazzy', 'rmw': 'synthetic-fixture-no-ROS',
          'max_subscriptions': 32, 'max_sample_bytes': MAX_SAMPLE, 'max_command_bytes': MAX_COMMAND,
          'image_contract_version': 2, 'max_image_raw_bytes': 16777216, 'max_image_sample_bytes': 25165824,
          'spatial_contract_version': 1, 'max_native_handles': 64, 'max_spatial_scan_rays': 2048,
          'max_tf_frames': 256, 'max_tf_edges': 128, 'max_tf_dynamic_inserts': 256,
          'tf_cache_window_ns': 10000000000, 'spatial_pending_timeout_ms': 1000, 'spatial_retry_interval_ms': 50,
          'robot_contract_version': 1, 'max_robot_links': 128, 'max_robot_hz': 10, 'max_robot_sample_bytes': MAX_SAMPLE,
          'shared_scene_contract_version': 1, 'max_sensor_layers': 4, 'point_cloud_contract_version': 2,
          'max_cloud_source_points': 8192, 'max_cloud_raw_bytes': 262144, 'max_cloud_sample_bytes': 524288,
          'map_snapshot': {'contract_version': 2, 'max_dimension': 2048, 'max_cells': 4194304, 'max_chunk_bytes': 196608, 'max_chunks': 22, 'max_sample_bytes': 524288, 'max_layers': 3, 'max_update_readers_per_map': 1},
          'robot_description': {'contract_version': 1, 'max_document_bytes': 1048576, 'max_chunk_bytes': 196608, 'max_chunks': 6, 'max_owners': 1},
          'tf_graph': {'contract_version': 1, 'max_hz': 5, 'max_sample_bytes': 524288, 'max_owners': 1},
          'mapping_contract_version': 1, 'max_map_dimension': 256, 'max_map_cells': 65536,
          'max_path_poses': 1024, 'max_mapping_sample_bytes': 524288,
          'max_map_layers': 3, 'max_pose_layers': 1, 'max_path_layers': 1})
    if mode == 'incompatible':
        return
    if mode == 'error':
        emit({'event': 'error', 'message': 'Synthetic session failure; reconnect manually'})
        return
    graph_at = time.monotonic() + (5 if mode == 'graph-delay' else 0)
    while time.monotonic() < graph_at:
        if not terminal.empty() or (root / 'exit-request').exists():
            return
        time.sleep(0.01)
    emit({'event': 'graph', 'topics': [] if mode == 'empty' else TOPICS})
    generation, last_request, last_subscription = None, 0, 0
    streams = {}
    cursor = 0
    sent_frames = []
    while not (root / 'exit-request').exists():
        try:
            command = commands.get(timeout=0.005 if streams else 0.02)
        except queue.Empty:
            command = None
        if command is not None:
            with (root / 'commands.jsonl').open('a') as log:
                log.write(json.dumps({'pid': os.getpid(), **command}, ensure_ascii=False) + '\n')
            g, request, subscription = (command[k] for k in ('generation', 'request', 'subscription'))
            if generation is not None and g != generation:
                emit({'event': 'error', 'message': 'Generation changed inside observation process'})
                return
            generation = g
            if command['command'] == 'tool':
                emit(dict(event='tool', generation=g, request=request, subscription=subscription,
                          update=dict(state='graph', value={} if mode == 'empty' else dict(TOPICS), terminal=True)))
                continue
            if request <= last_request:
                request_error(command, 'Request ID is stale; retry with a fresh request ID', subscription not in streams)
            else:
                last_request = request
                if command['command'] == 'start':
                    if subscription <= last_subscription:
                        request_error(command, 'Subscription ID was already allocated', subscription not in streams)
                    else:
                        last_subscription = subscription
                        config = command['configuration']
                        if not config_valid(config):
                            request_error(command, 'Invalid topic or discovered message type', True)
                        elif not resources_valid([item['command']['configuration'] for item in streams.values()] + [config]):
                            request_error(command, 'Scene display or logical/native reader capacity exceeded', True)
                        elif mode == 'start-error' or config['topic'] == '/demo/start_error':
                            emit_drops(g, subscription, configuration=config)
                            request_error(command, 'Synthetic start failure; no handle remains', True)
                        else:
                            streams[subscription] = {'command': command, 'count': 0, 'started': False,
                                                     'ready_at': time.monotonic() + (0.5 if mode == 'start-delay' else 0),
                                                     'next_at': 0.0, 'spatial_counters': dict.fromkeys(SPATIAL_COUNTER_NAMES, 0) if output_kind(config) == 'spatial_scan' else None}
                elif command['command'] == 'clear_pose':
                    stream = streams.get(subscription)
                    if stream is None or not stream['started'] or output_kind(stream['command']['configuration']) != 'pose_snapshot':
                        request_error(command, 'Clear requires an active pose stream', stream is None)
                    else:
                        barrier = pose_wire.clear(stream['command'], stream)
                        emit(dict(event='pose_cleared', generation=g, request=request, subscription=subscription, barrier=barrier))
                elif command['command'] == 'clear_markers':
                    stream = streams.get(subscription)
                    if stream is None or not stream['started'] or stream['command']['configuration']['output'] != 'marker_snapshot':
                        request_error(command, 'Clear requires an active Marker stream', stream is None)
                    else:
                        stream['marker_revision'] = stream.get('marker_revision', 1) + 1
                        stream['marker_sent'] = True
                        emit({'event': 'markers_cleared', 'generation': g, 'request': request, 'subscription': subscription})
                        stream['count'] += 1
                        emit(dict(event='marker_snapshot_observation', generation=g, subscription=subscription,
                                  sequence=stream['count'], sample=marker_wire.clear(stream['command'], stream['marker_revision']), backend_drops=0))
                else:
                    if subscription in streams or subscription <= last_subscription:
                        # No native handles exist in this fixture. Remove before ack; no future sample survives.
                        removed = streams.pop(subscription, None)
                        if removed is not None:
                            emit_drops(g, subscription, removed)
                        emit({'event': 'stopped', 'generation': g, 'request': request, 'subscription': subscription})
                    else:
                        request_error(command, 'Unknown subscription ID', True)
        if commands.empty() and not terminal.empty():
            error = terminal.get_nowait()
            if error is not None:
                emit({'event': 'error', 'message': error})
            return  # stdin EOF cancels every remaining synthetic stream
        if os.environ.get('STUDIO_UI_FIXTURE_SCENE_FRAMES') == '1' and generation is not None:
            scenes = [stream['command']['configuration'].get('spatial') or stream['command']['configuration'].get('robot') for stream in streams.values()]
            scenes = [scene for scene in scenes if scene]
            frames = sorted({'map', 'laser', 'cloud', 'mapping_source', *(scene['fixed_frame'] for scene in scenes)}) if scenes else []
            if frames != sent_frames:
                emit(dict(event='scene_frames', generation=generation, frames=frames))
                sent_frames = frames
        now = time.monotonic()
        # One visit per stream per turn gives control and low-rate streams progress under flood.
        ids = list(streams)
        if ids:
            cursor %= len(ids)
            ids = ids[cursor:] + ids[:cursor]
            cursor += 1
        for subscription in ids:
            stream = streams.get(subscription)
            if stream is None:
                continue
            command = stream['command']
            if now < stream['ready_at']:
                continue
            config = command['configuration']
            if (not stream['started'] and output_kind(config) == 'map_snapshot'
                    and (root / 'pause-map-start').exists()):
                continue
            if not stream['started']:
                emit({'event': 'started', **{k: command[k] for k in ('generation', 'request', 'subscription')}})
                stream['started'] = True
            # Native UI idle-expiry fixture only. Commands and non-sensor peers
            # keep progressing; unpausing never emits a recovered sample backlog.
            if output_kind(config) in ('spatial_scan', 'point_cloud') and (root / 'pause-sensors').exists():
                continue
            if mode in ('empty', 'waiting') or now < stream['next_at']:
                continue
            if output_kind(config) == 'camera_snapshot':
                stream['count'] += 1
                emit(dict(event='camera_snapshot_observation', generation=generation, subscription=subscription,
                    sequence=stream['count'], sample=camera_wire.snapshot(command, stream['count'], camera_sample(config['topic'], stream['count'], True)), backend_drops=0))
                stream['next_at'] = now + .05
                continue
            if output_kind(config) == 'pose_snapshot':
                if (root / 'pause-poses').exists():
                    continue
                stream['count'] += 1
                emit(dict(event='pose_snapshot_observation', generation=generation, subscription=subscription,
                          sequence=stream['count'], sample=pose_wire.snapshot(command, stream), backend_drops=0))
                stream['next_at'] = now + .05
                continue
            if output_kind(config) == 'marker_snapshot':
                # Development-only control files, no ROS or product runtime behavior.
                profile_path = root / 'marker-scene'
                profile = profile_path.read_text()[:32].strip() if profile_path.exists() else 'cube'
                if profile not in ('cube', 'rich', 'missing', 'recovered', 'finite', 'maximum'):
                    profile = 'cube'
                if (root / 'fail-markers').exists():
                    del streams[subscription]
                    emit_drops(generation, subscription, stream)
                    emit(dict(event='stream_error', generation=generation, subscription=subscription,
                              message='Synthetic Marker source failure'))
                    continue
                repeat = (root / 'marker-repeat').exists()
                if not stream.get('marker_sent') or stream.get('marker_profile') != profile or repeat:
                    stream['marker_profile'] = profile
                    stream['marker_revision'] = stream.get('marker_revision', 0) + 1
                    stream['marker_sent'] = True
                    for sample in marker_wire.snapshots(command, stream['marker_revision'], profile):
                        stream['count'] += 1
                        emit(dict(event='marker_snapshot_observation', generation=generation, subscription=subscription,
                                  sequence=stream['count'], sample=sample, backend_drops=0))
                    stream['next_at'] = now + .5
                continue
            if output_kind(config) == 'robot_description':
                if mode == 'description-wait' or (config['topic'] == '/demo/robot_description' and not config['transient_local']):
                    continue
                if mode == 'description-oversize':
                    del streams[subscription]
                    emit_drops(generation, subscription, stream)
                    emit({'event': 'stream_error', 'generation': generation, 'subscription': subscription,
                          'message': 'Synthetic oversized description exceeds 1 MiB'})
                    continue
                raw = ((Path(__file__).resolve().parents[1] / 'fixtures/robot_visualizer/topic.urdf').read_bytes()
                       if config['topic'] == '/demo/robot_description' else b'Not an XML robot description')
                if mode == 'description-max':
                    padding = 1048576 - len(raw) - len(b'<!---->')
                    raw = raw.replace(b'</robot>', b'<!--' + b'x' * padding + b'--></robot>')
                count = (len(raw) + 196607) // 196608
                index = stream['count']
                if index >= count:
                    continue
                sample = dict(contract_version=1, topic=config['topic'], message_type='std_msgs/msg/String',
                              index=index, count=count, document_bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest(),
                              data_base64=base64.b64encode(raw[index * 196608:(index + 1) * 196608]).decode())
                if mode == 'description-bad-digest':
                    sample['sha256'] = '0' * 64
                if mode == 'description-gap':
                    sample['index'] += 1
                stream['count'] += 1
                stream['next_at'] = now + .2
                emit({'event': 'robot_description_observation', 'generation': generation, 'subscription': subscription,
                      'sequence': stream['count'], 'sample': sample, 'backend_drops': 0})
                continue
            if output_kind(config) == 'map_snapshot':
                if (root / 'fail-map').exists():
                    del streams[subscription]
                    emit_drops(generation, subscription, stream)
                    emit({'event': 'stream_error', 'generation': generation, 'subscription': subscription,
                          'message': 'Synthetic Map failure; only this fixture owner is released'})
                    continue
                if (root / 'pause-map').exists():
                    continue
                sample = map_snapshot_sample(stream, mode, repeat=(root / 'map-repeat').exists())
                if sample is not None:
                    stream['count'] += 1
                    stream['next_at'] = now + 0.02
                    emit(dict(event='map_snapshot_observation', generation=generation, subscription=subscription,
                              sequence=stream['count'], sample=sample, backend_drops=0))
                continue
            retained = mode == 'retained' or config['topic'] == '/demo/retained'
            if retained and (stream['count'] or not config['transient_local']):
                continue
            if (mode == 'scoped-error' or config['topic'] == '/demo/stream_error') and stream['count'] >= 2:
                del streams[subscription]
                emit_drops(generation, subscription, stream)
                emit({'event': 'stream_error', 'generation': generation, 'subscription': subscription,
                      'message': 'Synthetic scoped failure; this stream is stopped'})
                continue
            if output_kind(config) == 'spatial_scan':
                if mode == 'spatial-error' and stream['count'] >= 2:
                    del streams[subscription]
                    stream['spatial_counters']['tf_rejected'] += 1
                    emit_drops(generation, subscription, stream)
                    emit({'event': 'stream_error', 'generation': generation, 'subscription': subscription,
                          'message': 'Synthetic Spatial group failure; all three fixture resources released'})
                    continue
                sample = spatial_sample(stream, now, mode)
                if sample is not None:
                    stream['count'] += 1
                    emit({'event': 'spatial_observation', 'generation': generation, 'subscription': subscription,
                          'sequence': stream['count'], 'sample': sample, 'backend_drops': 0})
                    if mode == 'failure':
                        emit({'event': 'error', 'message': 'Synthetic backend failure after receipt'})
                        return
                continue
            if output_kind(config) == 'point_cloud':
                stream['count'] += 1
                emit({'event':'point_cloud_observation', 'generation':generation, 'subscription':subscription,
                      'sequence':stream['count'], 'sample':cloud_sample(stream), 'backend_drops':0})
                continue
            if output_kind(config) in MAPPING_TYPES:
                if mode == 'mapping-error' and stream['count'] >= 2:
                    del streams[subscription]
                    emit_drops(generation, subscription, stream)
                    emit({'event':'stream_error', 'generation':generation, 'subscription':subscription,
                          'message':'Synthetic mapping failure; only this fixture owner is released'})
                    continue
                stream['count'] += 1
                emit({'event':'mapping_observation', 'generation':generation, 'subscription':subscription,
                      'sequence':stream['count'], 'sample':mapping_sample(stream, mode), 'backend_drops':0})
                stream['next_at'] = now + 0.2
                continue
            if output_kind(config) == 'tf_graph':
                sample = tf_graph_sample(stream, mode)
                stream['count'] += 1
                emit({'event': 'tf_graph_observation', 'generation': generation, 'subscription': subscription,
                      'sequence': stream['count'], 'sample': sample, 'backend_drops': 0})
                stream['next_at'] = now + 0.2
                continue
            if output_kind(config) == 'robot':
                sample = robot_sample(stream, mode)
                stream['count'] += 1
                emit({'event': 'robot_observation', 'generation': generation, 'subscription': subscription,
                      'sequence': stream['count'], 'sample': sample, 'backend_drops': 0})
                stream['next_at'] = now + 0.1  # At most 10 Hz, including flood mode.
                if mode == 'failure':
                    emit({'event': 'error', 'message': 'Synthetic backend failure after receipt'})
                    return
                continue
            burst = 32 if mode == 'flood' and config['topic'] != '/demo/slow' else 1
            for _ in range(burst):
                stream['count'] += 1
                count = stream['count']
                compact = output_kind(config) == 'compact_image'
                if config['message_type'] == IMAGE_TYPE:
                    sample = camera_sample(config['topic'], count, compact)
                elif config['message_type'] == SCAN_TYPE:
                    sample = scan_sample(config['topic'], count)
                elif config['message_type'] == TF_TYPE:
                    sample = tf_sample(config['topic'], count)
                else:
                    sample = typed_event(config['topic'], config['message_type'], count)['sample']
                if mode == 'long' and config['message_type'] != IMAGE_TYPE:
                    sample['value'] = obj(data=text('Long value café 🤖 ' * 100, 200000))
                elif config['message_type'] in ('std_msgs/msg/Float32', 'std_msgs/msg/Float64'):
                    sample['value'] = obj(data=number(math.sin(count / 10)))
                emit({'event': 'image_observation' if compact else 'observation', 'generation': generation, 'subscription': subscription,
                      'sequence': count, 'sample': sample, 'backend_drops': 0})
                if mode == 'failure':
                    emit({'event': 'error', 'message': 'Synthetic backend failure after receipt'})
                    return
            stream['next_at'] = now + (0.0 if mode == 'flood' and config['topic'] != '/demo/slow' else 0.2)


def main():
    arguments = sys.argv[1:]
    if arguments != ['--observe-session'] and arguments and (len(arguments) < 2 or arguments[0].startswith('--')
            or any(flag not in ('--best-effort', '--transient-local', '--fixture-echo') for flag in arguments[2:])):
        raise SystemExit('Use --observe-session or legacy [TOPIC TYPE QoS flags]')
    root = Path(os.environ.get('STUDIO_UI_FIXTURE_EVIDENCE', 'artifacts/feasibility/evidence/ui-workspace'))
    root.mkdir(parents=True, exist_ok=True)
    mode = os.environ.get('STUDIO_UI_FIXTURE_MODE', 'normal')
    if mode not in MODES:
        raise SystemExit('Unknown STUDIO_UI_FIXTURE_MODE')
    with (root / 'launches.jsonl').open('a') as log:
        log.write(json.dumps({'pid': os.getpid(), 'arguments': arguments, 'domain': os.environ.get('ROS_DOMAIN_ID'),
                              'rmw': os.environ.get('RMW_IMPLEMENTATION'), 'time': time.time(), 'mode': mode}) + '\n')
    if arguments == ['--observe-session']:
        session_main(root, mode)
    else:
        legacy_main(root, mode)


if __name__ == '__main__':
    try:
        main()
    except BrokenPipeError:
        pass
