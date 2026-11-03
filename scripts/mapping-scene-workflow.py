"""Original bounded map/pose/path + Robot/scan/Cloud publisher, localhost187 only.

Run with scripts/dev project Python, explicit --rmw and fresh ignored --evidence.
No navigation commands. Publisher calls do not prove DDS/UI acceptance.
"""
import argparse
import copy
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('mapping_sensor_recipe', ROOT / 'scripts/sensor-scene-workflow.py')
SENSOR = importlib.util.module_from_spec(spec)
spec.loader.exec_module(SENSOR)
ROBOT, HELPERS, RMWS = SENSOR.ROBOT, SENSOR.HELPERS, SENSOR.RMWS
SignalStop = SENSOR.SignalStop
LAYOUT = json.loads((ROOT / 'fixtures/mapping_scene/alignment.json').read_text())
MODES = ('nominal', 'recover', 'missing', 'map-retained', 'map-malformed', 'map-over-limit',
         'pose-malformed', 'path-malformed', 'path-over-limit', 'large', 'empty', 'path-updates',
         'map-duplicate', 'map-changed-stamp', 'map-historical')
TOPICS = dict(dynamic='/mapping_scene/tf', static='/mapping_scene/tf_static',
              scan='/mapping_scene/scan', cloud='/mapping_scene/cloud', map='/mapping_scene/map',
              pose='/mapping_scene/pose', path='/mapping_scene/path', status='/mapping_scene/status')


def argument_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rmw', choices=RMWS, required=True)
    parser.add_argument('--evidence', type=Path, required=True)
    parser.add_argument('--seconds', type=ROBOT.bounded_seconds, default=600)
    parser.add_argument('--mode', choices=MODES, default='nominal')
    return parser


def checked_environment(rmw, evidence):
    return ROBOT.checked_environment(rmw, evidence)


def set_float_fields(target, names, values):
    """Normalize JSON numbers before generated ROS floating-field conversion."""
    values = tuple(values)
    if len(names) != len(values):
        raise ValueError('Floating field names and values must match')
    normalized = tuple(float(value) for value in values)
    if not all(math.isfinite(value) for value in normalized):
        raise ValueError('Fixture floating fields must be finite')
    for name, value in zip(names, normalized):
        setattr(target, name, value)


def set_pose_fields(pose, position, yaw, invalid=False):
    set_float_fields(pose.position, ('x', 'y', 'z'), position)
    radians = math.radians(float(yaw)) / 2
    quaternion = (0., 0., 0., 0.) if invalid else (0., 0., math.sin(radians), math.cos(radians))
    set_float_fields(pose.orientation, ('x', 'y', 'z', 'w'), quaternion)


def geometry(mode='nominal', tick=0):
    """Pure original source geometry, with deliberately invalid modes labeled."""
    if mode not in MODES or type(tick) is not int or tick < 0:
        raise ValueError('Invalid mapping fixture mode/tick')
    grid = copy.deepcopy(LAYOUT['map'])
    pose = dict(position=list(LAYOUT['pose_source']), yaw_degrees=90.)
    path = [dict(position=list(p), frame_id='', stamp_ns=0) for p in LAYOUT['path_source']]
    if mode == 'large':
        grid.update(width=256, height=256, cells=[-1 if i % 17 == 0 else i % 101 for i in range(65536)])
        path = [dict(position=[i / 1024, (i % 16) / 16, 0.], frame_id='', stamp_ns=0) for i in range(1024)]
    elif mode == 'empty':
        grid.update(width=0, height=0, cells=[])
        path = []
    elif mode == 'map-malformed':
        grid['cells'][0] = 101
    elif mode == 'map-over-limit':
        grid.update(width=257, height=1, cells=[0] * 257)
    elif mode == 'pose-malformed':
        pose['invalid_quaternion'] = True
    elif mode == 'path-malformed':
        path[1]['frame_id'] = 'contradictory_frame'
    elif mode == 'path-over-limit':
        path = [dict(position=[0., 0., 0.], frame_id='', stamp_ns=0) for _ in range(1025)]
    elif mode == 'path-updates':
        path[-1]['position'][2] = (tick % 10) / 10
    return dict(map=grid, pose=pose, path=path)


def static_transforms(mapping_ready):
    values = SENSOR.static_transforms(True)
    if mapping_ready:
        q = math.sqrt(.5)
        values.append(ROBOT.transform('map', 'mapping_source', (1., 2., 3.), (0., 0., q, q)))
    return values


def map_stamp(mode, current, original):
    """Explicit fixed/repeated versus historical acquisition stamp recipe."""
    if mode not in MODES:
        raise ValueError('Invalid mapping stamp mode')
    result = original if mode in ('map-duplicate', 'map-changed-stamp') else (current[0] - 20, current[1]) if mode == 'map-historical' else current
    if result[0] <= 0 or not 0 <= result[1] < 1_000_000_000:
        raise ValueError('Mapping stamp must remain positive and normalized')
    return result


def publisher(rmw, evidence, mode, seconds):
    seconds = ROBOT.bounded_seconds(seconds)
    if mode not in MODES:
        raise ValueError('Invalid mapping mode')
    folder = checked_environment(rmw, evidence)
    HELPERS['inventory']()
    # No ROS imports before environment, explicit matching RMW and evidence guards.
    import rclpy
    from rclpy.signals import SignalHandlerOptions
    from rclpy.qos import QoSProfile, ReliabilityPolicy, DurabilityPolicy
    from geometry_msgs.msg import TransformStamped, PoseStamped
    from nav_msgs.msg import OccupancyGrid, Path as RosPath
    from sensor_msgs.msg import LaserScan, PointCloud2, PointField
    from std_msgs.msg import Float64
    from tf2_msgs.msg import TFMessage
    stopping = SignalStop()
    stopping.install()
    node = None
    counts = dict.fromkeys(TOPICS, 0)
    ready = mode not in ('recover', 'missing', 'map-historical')
    original_map_stamp = None
    started = time.monotonic()

    def snapshot(event, **extra):
        value = dict(event=event, pid=os.getpid(), rmw=rmw, mode=mode, mapping_tf_ready=ready,
                     publisher_calls=dict(counts), elapsed_seconds=time.monotonic() - started,
                     time=time.time(), **extra)
        HELPERS['append'](folder / 'mapping-publisher.jsonl', value)
        (folder / 'publisher-state.json').write_text(json.dumps(value, indent=2) + '\n')

    def tf_message(values, stamp=None):
        result = TFMessage()
        for value in values:
            transform = TransformStamped()
            transform.header.frame_id, transform.child_frame_id = value['parent'], value['child']
            if stamp is not None:
                transform.header.stamp = stamp
            set_float_fields(transform.transform.translation, ('x', 'y', 'z'), value['translation'])
            set_float_fields(transform.transform.rotation, ('x', 'y', 'z', 'w'), value['rotation_xyzw'])
            result.transforms.append(transform)
        return result

    def pose_message(position, yaw, stamp, frame='mapping_source', invalid=False):
        pose = PoseStamped()
        pose.header.stamp.sec, pose.header.stamp.nanosec = stamp.sec, stamp.nanosec
        pose.header.frame_id = frame
        set_pose_fields(pose.pose, position, yaw, invalid)
        return pose

    try:
        rclpy.init(signal_handler_options=SignalHandlerOptions.NO)
        node = rclpy.create_node('studio_mapping_fixture', start_parameter_services=False)
        assert node.get_parameter('use_sim_time').value is False
        reliable = QoSProfile(depth=10, reliability=ReliabilityPolicy.RELIABLE)
        retained = QoSProfile(depth=1, reliability=ReliabilityPolicy.RELIABLE, durability=DurabilityPolicy.TRANSIENT_LOCAL)
        sensor = QoSProfile(depth=10, reliability=ReliabilityPolicy.BEST_EFFORT)
        types = dict(dynamic=TFMessage, static=TFMessage, scan=LaserScan, cloud=PointCloud2,
                     map=OccupancyGrid, pose=PoseStamped, path=RosPath, status=Float64)
        pubs = {key: node.create_publisher(types[key], topic, retained if key in ('map', 'static') else sensor if key in ('scan', 'cloud') else reliable) for key, topic in TOPICS.items()}
        inputs = [Path(__file__).resolve(), ROOT / 'scripts/sensor-scene-workflow.py',
                  ROOT / 'scripts/robot-ros-workflow.py', ROOT / 'fixtures/mapping_scene/alignment.json',
                  ROOT / 'fixtures/sensor_scene/cloud-layout.json']
        snapshot('started', argv=sys.argv, domain=187, localhost_only=True, topics=TOPICS,
                 source_sha256={str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in inputs},
                 expected_geometry=LAYOUT)
        pubs['static'].publish(tf_message(static_transforms(ready)))
        counts['static'] += 1
        cloud_data = SENSOR.cloud_frame()
        next_at = started
        while not stopping.requested and rclpy.ok() and time.monotonic() - started < seconds:
            now = time.monotonic()
            if now >= next_at:
                next_at = now + .2
                if mode == 'recover' and not ready and ROBOT.read_control(folder / 'mode.txt') == 'ready':
                    ready = True
                    pubs['static'].publish(tf_message(static_transforms(True)))
                    counts['static'] += 1
                    snapshot('mapping-tf-released')
                stamp = node.get_clock().now().to_msg()
                assert stamp.sec > 0
                data = geometry(mode, counts['dynamic'])
                dynamic = [ROBOT.dynamic_transform(now - started)]
                if mode == 'map-historical':
                    q = math.sqrt(.5)
                    dynamic.append(ROBOT.transform('map', 'mapping_source', (1., 2., 3.), (0., 0., q, q)))
                pubs['dynamic'].publish(tf_message(dynamic, stamp))
                counts['dynamic'] += 1
                scan = LaserScan()
                scan.header.stamp, scan.header.frame_id = stamp, 'sensor_scan_a'
                set_float_fields(scan, ('angle_min', 'angle_max', 'angle_increment'), (0., math.pi / 2, math.pi / 2))
                set_float_fields(scan, ('range_min', 'range_max', 'scan_time', 'time_increment'), (.05, 10., .2, 0.))
                scan.ranges = [float(value) for value in (1, 1)]
                pubs['scan'].publish(scan)
                counts['scan'] += 1
                cloud = PointCloud2()
                cloud.header.stamp, cloud.header.frame_id = stamp, 'sensor_cloud'
                for key in ['width', 'height', 'point_step', 'row_step', 'is_bigendian', 'is_dense', 'data']:
                    setattr(cloud, key, cloud_data[key])
                cloud.fields = [PointField(**f) for f in cloud_data['fields']]
                pubs['cloud'].publish(cloud)
                counts['cloud'] += 1
                if mode != 'map-retained' or counts['map'] == 0:
                    grid = OccupancyGrid()
                    if original_map_stamp is None:
                        original_map_stamp = (stamp.sec, stamp.nanosec)
                    selected_stamp = map_stamp(mode, (stamp.sec, stamp.nanosec), original_map_stamp)
                    grid.header.stamp.sec, grid.header.stamp.nanosec = selected_stamp
                    grid.header.frame_id = 'mapping_source'
                    grid.info.width, grid.info.height = data['map']['width'], data['map']['height']
                    grid.info.resolution = float(data['map']['resolution'])
                    grid.info.origin = pose_message(data['map']['origin_translation'], data['map']['origin_yaw_degrees'], stamp).pose
                    grid.data = data['map']['cells']
                    if mode == 'map-changed-stamp' and ROBOT.read_control(folder / 'mode.txt') == 'ready':
                        grid.data[0] = 100
                    pubs['map'].publish(grid)
                    counts['map'] += 1
                pubs['pose'].publish(pose_message(data['pose']['position'], data['pose']['yaw_degrees'], stamp, invalid=data['pose'].get('invalid_quaternion', False)))
                counts['pose'] += 1
                path = RosPath()
                path.header.stamp, path.header.frame_id = stamp, 'mapping_source'
                for entry in data['path']:
                    child = pose_message(entry['position'], 0., stamp, frame=entry['frame_id'])
                    child.header.stamp.sec = child.header.stamp.nanosec = 0
                    path.poses.append(child)
                pubs['path'].publish(path)
                counts['path'] += 1
                pubs['status'].publish(Float64(data=float(counts['dynamic'])))
                counts['status'] += 1
                if counts['dynamic'] == 1 or counts['dynamic'] % 25 == 0:
                    snapshot('publishing', map_cells=len(data['map']['cells']), path_poses=len(data['path']))
            rclpy.spin_once(node, timeout_sec=.002)
    finally:
        try:
            snapshot('stopping', signal=stopping.signum)
        finally:
            try:
                try:
                    if node is not None:
                        node.destroy_node()
                finally:
                    if rclpy.ok():
                        rclpy.shutdown()
            except BaseException as error:
                snapshot('cleanup_failed', error=f'{type(error).__name__}: {error}')
                raise
            else:
                snapshot('stopped', signal=stopping.signum)
            finally:
                stopping.restore()


if __name__ == '__main__':
    args = argument_parser().parse_args()
    publisher(args.rmw, args.evidence, args.mode, args.seconds)
