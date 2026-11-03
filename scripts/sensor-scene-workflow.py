"""Guarded synthetic Robot+two scans+PointCloud2 publisher; never commands a robot.

Run through scripts/dev project Python, explicit matching --rmw, localhost187,
and a fresh ignored --evidence directory. Recover initially omits only cloud TF;
write ready to mode.txt to release its retained transform once. Publishers are
fixture calls, not proof of DDS delivery, UI FPS, native cleanup or hardware.
"""
import argparse
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import signal
import struct
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('sensor_scene_robot_recipe', ROOT / 'scripts/robot-ros-workflow.py')
ROBOT = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ROBOT)
HELPERS = ROBOT.HELPERS
RMWS = ROBOT.RMWS
MODES = ('nominal', 'recover', 'missing', 'cloud-malformed', 'cloud-pressure', 'cloud-empty', 'cloud-skipped',
         'cloud-rgb', 'cloud-rgba', 'cloud-attributes-skipped', 'cloud-unsupported')
LAYOUT = json.loads((ROOT / 'fixtures/sensor_scene/cloud-layout.json').read_text())
TOPICS = dict(dynamic='/sensor_scene/tf', static='/sensor_scene/tf_static',
              scan_a='/sensor_scene/scan_a', scan_b='/sensor_scene/scan_b',
              cloud='/sensor_scene/cloud', status='/sensor_scene/status')


class SignalStop:
    """Python handlers only request exit; ROS destruction stays in normal flow."""
    def __init__(self):
        self.requested = False
        self.signum = None
        self.previous = {}

    def handle(self, signum, _frame):
        if not self.requested:
            self.signum = signum
        self.requested = True

    def install(self):
        try:
            for signum in (signal.SIGINT, signal.SIGTERM):
                self.previous[signum] = signal.getsignal(signum)
                signal.signal(signum, self.handle)
        except BaseException:
            self.restore()
            raise

    def restore(self):
        for signum, handler in self.previous.items():
            signal.signal(signum, handler)
        self.previous.clear()


def argument_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rmw', choices=RMWS, required=True)
    parser.add_argument('--evidence', type=Path, required=True)
    parser.add_argument('--mode', choices=MODES, default='nominal')
    parser.add_argument('--seconds', type=ROBOT.bounded_seconds, default=600)
    parser.add_argument('--big-endian', action='store_true')
    return parser


def checked_environment(rmw, evidence):
    return ROBOT.checked_environment(rmw, evidence)


def cloud_frame(mode='nominal', big_endian=False):
    """Pure bounded ROS-layout bytes; padding remains sentinel0xA5."""
    if mode not in MODES or type(big_endian) is not bool:
        raise ValueError('Invalid sensor cloud fixture mode or byte order')
    width, height, row_step = 2, 2, 72
    points = [list(p) for p in LAYOUT['points']]
    if mode == 'cloud-pressure':
        width, height, row_step = 4096, 2, 4096 * 32
        points = [[(i % 64) / 10, (i // 64) / 10, (i % 5) / 20] for i in range(8192)]
    elif mode == 'cloud-empty':
        width, height, row_step, points = 0, 1, 0, []
    elif mode in ('cloud-skipped', 'cloud-attributes-skipped'):
        points[0][0], points[2][2] = float('nan'), 1_000_001.0
    raw = bytearray(b'\xa5' * (row_step * height))
    order = '>' if big_endian else '<'
    for i, (x, y, z) in enumerate(points):
        at = (i // width) * row_step + (i % width) * 32
        struct.pack_into(order + 'd', raw, at, z)
        struct.pack_into(order + 'f', raw, at + 8, x)
        intensity = [1., float('nan'), float('inf'), -42.][i % 4] if mode == 'cloud-attributes-skipped' else float(i)
        struct.pack_into(order + 'f', raw, at + 12, intensity)
        struct.pack_into(order + 'd', raw, at + 20, y)
        if mode in ('cloud-rgb', 'cloud-rgba', 'cloud-attributes-skipped', 'cloud-pressure', 'cloud-unsupported'):
            struct.pack_into(order + 'I', raw, at + 16, [0xffc10203, 0x00112233, 0x80445566, 0x40778899][i % 4])
    assert len(raw) <= 256 * 1024 and len(points) <= 8192
    fields = [dict(f) for f in LAYOUT['fields']]
    if mode in ('cloud-rgb', 'cloud-rgba', 'cloud-attributes-skipped', 'cloud-pressure', 'cloud-unsupported'):
        fields.append(dict(name='rgb' if mode == 'cloud-rgb' else 'rgba', offset=16,
                           datatype=2 if mode == 'cloud-unsupported' else 7, count=4 if mode == 'cloud-unsupported' else 1))
    if mode == 'cloud-unsupported':
        fields[1].update(datatype=2, count=4)
    return dict(width=width, height=height, point_step=32, row_step=63 if mode == 'cloud-malformed' else row_step,
                fields=fields, is_bigendian=big_endian,
                is_dense=mode not in ('cloud-skipped', 'cloud-attributes-skipped'), data=bytes(raw))


def static_transforms(cloud_ready):
    q = math.sqrt(0.5)
    values = ROBOT.static_transforms(True)
    values += [ROBOT.transform('map', 'sensor_scan_a', (1., 2., 3.), (0., 0., q, q)),
               ROBOT.transform('map', 'sensor_scan_b', (-1., 0., 0.5), (0., 0., -q, q))]
    if cloud_ready:
        values.append(ROBOT.transform('map', 'sensor_cloud', (1., 2., 3.), (0., 0., q, q)))
    return values


def publisher(rmw, evidence, mode, seconds, big_endian=False):
    seconds = ROBOT.bounded_seconds(seconds)
    if mode not in MODES or type(big_endian) is not bool:
        raise ValueError('Invalid sensor fixture mode or byte order')
    folder = checked_environment(rmw, evidence)
    HELPERS['inventory']()
    # Every ROS import follows the project/runtime/domain/peer/evidence guards.
    import rclpy
    from rclpy.signals import SignalHandlerOptions
    from geometry_msgs.msg import TransformStamped
    from rclpy.qos import QoSProfile, ReliabilityPolicy, DurabilityPolicy
    from sensor_msgs.msg import LaserScan, PointCloud2, PointField
    from std_msgs.msg import Float64
    from tf2_msgs.msg import TFMessage
    stopping = SignalStop()
    stopping.install()
    node = None
    counts = dict.fromkeys(TOPICS, 0)
    cloud_ready = mode not in ('missing', 'recover')
    started = time.monotonic()
    log = folder / 'sensor-publisher.jsonl'

    def tf_message(values, stamp=None):
        result = TFMessage()
        for value in values:
            transform = TransformStamped()
            transform.header.frame_id, transform.child_frame_id = value['parent'], value['child']
            if stamp is not None:
                transform.header.stamp = stamp
            transform.transform.translation.x, transform.transform.translation.y, transform.transform.translation.z = value['translation']
            transform.transform.rotation.x, transform.transform.rotation.y, transform.transform.rotation.z, transform.transform.rotation.w = value['rotation_xyzw']
            result.transforms.append(transform)
        return result

    def snapshot(event, **extra):
        value = dict(event=event, pid=os.getpid(), rmw=rmw, mode=mode, cloud_ready=cloud_ready,
                     publisher_calls=dict(counts), elapsed_seconds=time.monotonic() - started, time=time.time(), **extra)
        HELPERS['append'](log, value)
        (folder / 'publisher-state.json').write_text(json.dumps(value, indent=2) + '\n')

    try:
        rclpy.init(signal_handler_options=SignalHandlerOptions.NO)
        node = rclpy.create_node('studio_combined_sensor_fixture', start_parameter_services=False)
        assert node.get_parameter('use_sim_time').value is False
        reliable = QoSProfile(depth=10, reliability=ReliabilityPolicy.RELIABLE)
        sensor = QoSProfile(depth=10, reliability=ReliabilityPolicy.BEST_EFFORT)
        pubs = {key: node.create_publisher(TFMessage if key in ('dynamic', 'static') else LaserScan if key.startswith('scan') else PointCloud2 if key == 'cloud' else Float64,
                                         topic, QoSProfile(depth=1, reliability=ReliabilityPolicy.RELIABLE, durability=DurabilityPolicy.TRANSIENT_LOCAL) if key == 'static' else sensor if key in ('scan_a', 'scan_b', 'cloud') else reliable)
                for key, topic in TOPICS.items()}
        inputs = [Path(__file__).resolve(), ROOT / 'scripts/robot-ros-workflow.py',
                  ROOT / 'fixtures/sensor_scene/cloud-layout.json', ROOT / 'fixtures/robot_visualizer/articulated.urdf',
                  ROOT / 'fixtures/robot_visualizer/meshes/wedge-ascii.stl', ROOT / 'fixtures/robot_visualizer/meshes/wedge-binary.stl']
        snapshot('started', argv=sys.argv, domain=187, localhost_only=True,
                 source_sha256={str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in inputs},
                 topics=TOPICS, expected_fixed_points=LAYOUT['expected_fixed_points'])
        pubs['static'].publish(tf_message(static_transforms(cloud_ready)))
        counts['static'] += 1
        data = cloud_frame(mode, big_endian)
        slow_at = cloud_at = time.monotonic()
        while not stopping.requested and rclpy.ok() and time.monotonic() - started < seconds:
            now = time.monotonic()
            if now >= slow_at:
                slow_at = now + 0.2
                if mode == 'recover' and not cloud_ready and ROBOT.read_control(folder / 'mode.txt') == 'ready':
                    cloud_ready = True
                    pubs['static'].publish(tf_message(static_transforms(True)))
                    counts['static'] += 1
                    snapshot('cloud-tf-released')
                stamp = node.get_clock().now().to_msg()
                assert stamp.sec > 0
                pubs['dynamic'].publish(tf_message([ROBOT.dynamic_transform(now - started)], stamp))
                counts['dynamic'] += 1
                for key, frame, distance in [('scan_a', 'sensor_scan_a', 1.), ('scan_b', 'sensor_scan_b', 2.)]:
                    scan = LaserScan()
                    scan.header.stamp, scan.header.frame_id = stamp, frame
                    scan.angle_min, scan.angle_max, scan.angle_increment = 0., math.pi / 2, math.pi / 2
                    scan.range_min, scan.range_max, scan.scan_time, scan.time_increment = 0.05, 10., 0.2, 0.
                    scan.ranges = [distance, distance]
                    pubs[key].publish(scan)
                    counts[key] += 1
                pubs['status'].publish(Float64(data=float(counts['dynamic'])))
                counts['status'] += 1
                if counts['dynamic'] == 1 or counts['dynamic'] % 25 == 0:
                    snapshot('publishing')
            if not stopping.requested and now >= cloud_at:
                cloud_at = now + (0.01 if mode == 'cloud-pressure' else 0.2)
                cloud = PointCloud2()
                cloud.header.stamp, cloud.header.frame_id = node.get_clock().now().to_msg(), 'sensor_cloud'
                for key in ['width', 'height', 'point_step', 'row_step', 'is_bigendian', 'is_dense', 'data']:
                    setattr(cloud, key, data[key])
                cloud.fields = [PointField(**field) for field in data['fields']]
                pubs['cloud'].publish(cloud)
                counts['cloud'] += 1
            rclpy.spin_once(node, timeout_sec=0.002)
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
    publisher(args.rmw, args.evidence, args.mode, args.seconds, args.big_endian)
