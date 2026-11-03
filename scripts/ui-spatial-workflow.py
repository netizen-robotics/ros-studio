"""Guarded known-room scan/TF publishers; no robot, commands or simulated clock.

Run with project Python, domain187/localhost and an explicit matching RMW.
Counts are actual publisher calls, never inferred DDS delivery or performance.
"""
import argparse
from collections import deque
import importlib.util
import json
import math
import os
from pathlib import Path
import runpy
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
HELPERS = runpy.run_path(str(ROOT / 'scripts/ui-ros-workflow.py'))
spec = importlib.util.spec_from_file_location('_studio_spatial_model', ROOT / 'fixtures/simple_sim/model.py')
MODEL = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = MODEL
spec.loader.exec_module(MODEL)
PREFIX = '/studio_fixture/spatial/'
FIXED = 'studio_sim/odom'
BASE = 'studio_sim/base_link'
LASER = 'studio_sim/laser'
SCAN_NAMES = ('scan', 'same_frame_scan', 'interpolation_scan', 'delayed_scan', 'missing_scan', 'malformed_scan', 'out_of_order_scan')
TF_NAMES = ('tf', 'delayed_tf', 'out_of_order_tf', 'malformed_tf', 'empty_tf', 'tf_static', 'cyclic_tf_static')
TOPICS = (*SCAN_NAMES, *TF_NAMES, 'status')


def scan_values(elapsed, same_frame=False):
    x, y, yaw = (0., 0., 0.) if same_frame else MODEL.pose(elapsed)
    return {'frame': FIXED if same_frame else LASER, 'angle_min': -math.pi,
            'angle_max': -math.pi + 179 * 2 * math.pi / 180, 'angle_increment': 2 * math.pi / 180,
            'time_increment': 0., 'scan_time': .1, 'range_min': .05, 'range_max': 10.,
            'ranges': MODEL.scan(x, y, yaw), 'intensities': []}


class Schedule:
    def __init__(self, now):
        self.periods = {'tf': .05, 'scan': .1, 'special': 2., 'bad_scan': .5, 'status': .2, 'stats': 1.}
        self.next = {name: now for name in self.periods}

    def due(self, now):
        result = []
        for name, deadline in self.next.items():
            if now >= deadline:
                result.append(name)
                period = self.periods[name]
                self.next[name] = deadline + (math.floor((now - deadline) / period) + 1) * period
        return result

    def wait(self, now):
        return max(0., min(.02, min(self.next.values()) - now))


class Delayed:
    def __init__(self):
        self.pending = deque(maxlen=8)
        self.evicted = 0

    def put(self, now, value):
        if len(self.pending) == self.pending.maxlen:
            self.evicted += 1
        self.pending.append((now + .2, value))

    def due(self, now):
        result = []
        while self.pending and self.pending[0][0] <= now:
            result.append(self.pending.popleft()[1])
        return result


def publisher(seconds):
    if type(seconds) is not int or not 1 <= seconds <= 3600:
        raise ValueError('Fixture duration must be1..3600seconds')
    folder = HELPERS['checked_environment']()
    import rclpy
    from builtin_interfaces.msg import Time
    from geometry_msgs.msg import TransformStamped
    from rclpy.qos import QoSProfile, DurabilityPolicy, qos_profile_sensor_data
    from sensor_msgs.msg import LaserScan
    from std_msgs.msg import String
    from tf2_msgs.msg import TFMessage
    rclpy.init()
    node = rclpy.create_node('studio_spatial_workflow_fixture', start_parameter_services=False)
    assert node.get_parameter('use_sim_time').value is False, 'System-clock fixture cannot use simulated time'
    retained = QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL)
    pubs = {name: node.create_publisher(LaserScan, PREFIX + name, qos_profile_sensor_data) for name in SCAN_NAMES}
    pubs.update({name: node.create_publisher(TFMessage, PREFIX + name,
                 retained if name.endswith('static') else QoSProfile(depth=10)) for name in TF_NAMES})
    pubs['status'] = node.create_publisher(String, PREFIX + 'status', 10)
    counts = {name: 0 for name in TOPICS}
    started = time.monotonic()
    schedule = Schedule(started)
    delayed = Delayed()
    tf_brackets = deque(maxlen=2)
    interpolation_proofs = deque(maxlen=8)
    log = folder / 'spatial-publisher.jsonl'

    def emit(name, message):
        pubs[name].publish(message)
        counts[name] += 1

    def transform(stamp, parent, child, x=0., y=0., z=0., yaw=0., invalid=False):
        item = TransformStamped()
        item.header.stamp, item.header.frame_id, item.child_frame_id = stamp, parent, child
        item.transform.translation.x, item.transform.translation.y, item.transform.translation.z = x, y, z
        item.transform.rotation.z = math.sin(yaw / 2)
        item.transform.rotation.w = 0. if invalid else math.cos(yaw / 2)
        return item

    def scan(name, stamp, elapsed):
        values = scan_values(elapsed, name == 'same_frame_scan')
        message = LaserScan()
        message.header.stamp, message.header.frame_id = stamp, values.pop('frame')
        for key, value in values.items():
            setattr(message, key, value)
        if name == 'malformed_scan':
            message.angle_increment = 0.
        emit(name, message)

    HELPERS['append'](log, {'event': 'started', 'time': time.time(), 'pid': os.getpid(), 'argv': sys.argv,
        'use_sim_time': False, 'clock_policy': 'Explicit system clock; own node use_sim_time=false verified',
        'requested_hz': {'tf': 20, 'scan': 10, 'delayed_scan': .5, 'missing_scan': .5, 'status': 5},
        'delay_seconds': .2, 'topics': list(TOPICS), 'scope': 'Local synthetic publisher calls only'})
    try:
        static = TFMessage(transforms=[transform(Time(), BASE, LASER, z=.0529)])
        emit('tf_static', static)
        emit('cyclic_tf_static', TFMessage(transforms=[transform(Time(), FIXED, BASE), transform(Time(), BASE, FIXED)]))
        HELPERS['append'](log, {'event': 'retained-static-once', 'time': time.time(), 'stamp_ns': 0, 'parent': BASE, 'child': LASER})
        while rclpy.ok() and time.monotonic() - started < seconds:
            now = time.monotonic()
            elapsed = now - started
            due = schedule.due(now)
            stamp = node.get_clock().now().to_msg()
            for message in delayed.due(now):
                emit('delayed_tf', message)
            if 'tf' in due:
                x, y, yaw = MODEL.pose(elapsed)
                message = TFMessage(transforms=[transform(stamp, FIXED, BASE, x=x, y=y, yaw=yaw)])
                emit('tf', message)
                tf_brackets.append((stamp.sec * 1000000000 + stamp.nanosec, elapsed, x, y, yaw))
                delayed.put(now, message)
                wrong_stamp = Time(sec=stamp.sec - (counts['out_of_order_tf'] % 2), nanosec=stamp.nanosec)
                emit('out_of_order_tf', TFMessage(transforms=[transform(wrong_stamp, FIXED, BASE, x=x, y=y, yaw=yaw)]))
                emit('malformed_tf', TFMessage(transforms=[transform(stamp, FIXED, BASE, invalid=True)]))
            if 'scan' in due:
                scan('scan', stamp, elapsed)
                scan('same_frame_scan', stamp, elapsed)
            if 'special' in due:
                scan('delayed_scan', stamp, elapsed)
                scan('missing_scan', stamp, elapsed)
                if len(tf_brackets) == 2:
                    before, after = tf_brackets
                    midpoint = (before[0] + after[0]) // 2
                    fraction = (midpoint - before[0]) / (after[0] - before[0])
                    scan('interpolation_scan', Time(sec=midpoint // 1000000000, nanosec=midpoint % 1000000000),
                         before[1] + fraction * (after[1] - before[1]))
                    yaw = before[4] + fraction * (after[4] - before[4])
                    interpolation_proofs.append({'stamp_ns': midpoint, 'before_stamp_ns': before[0], 'after_stamp_ns': after[0],
                        'expected_components': [before[2] + fraction * (after[2] - before[2]), before[3] + fraction * (after[3] - before[3]),
                                                .0529, 0., 0., math.sin(yaw / 2), math.cos(yaw / 2)]})
                    (folder / 'interpolation-latest-eight.json').write_text(json.dumps(list(interpolation_proofs)) + '\n')
            if 'bad_scan' in due:
                scan('malformed_scan', stamp, elapsed)
                wrong_stamp = Time(sec=stamp.sec - (counts['out_of_order_scan'] % 2), nanosec=stamp.nanosec)
                scan('out_of_order_scan', wrong_stamp, elapsed)
            if 'status' in due:
                emit('status', String(data=f'SPATIAL-STATUS:{counts["status"]}'))
            if 'stats' in due:
                HELPERS['append'](log, {'event': 'published-counts', 'time': time.time(), 'elapsed_monotonic_seconds': elapsed,
                    'counts': dict(counts), 'delayed_pending': len(delayed.pending), 'delayed_evicted': delayed.evicted,
                    'scope': 'Publisher calls only; not DDS delivery'})
            rclpy.spin_once(node, timeout_sec=schedule.wait(time.monotonic()))
    except KeyboardInterrupt:
        pass
    finally:
        HELPERS['append'](log, {'event': 'stopped', 'time': time.time(), 'elapsed_monotonic_seconds': time.monotonic() - started,
            'counts': counts, 'delayed_pending': len(delayed.pending), 'delayed_evicted': delayed.evicted})
        node.destroy_node()
        if rclpy.ok():
            rclpy.shutdown()


def argument_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--publisher', action='store_true')
    parser.add_argument('--seconds', type=HELPERS['bounded_number'](1, 3600, int), default=120)
    return parser


if __name__ == '__main__':
    args = argument_parser().parse_args()
    if args.publisher:
        publisher(args.seconds)
    else:
        HELPERS['inventory']()
