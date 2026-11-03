"""Guarded local Image publishers for native/backend QA; no robot access.

Use scripts/dev project Python with the existing localhost/domain187/RMW guard.
Every Image comes from the deterministic simple_sim camera model. Publisher call
counts are recorded, never interpreted as DDS delivery or receiver throughput.
"""
import argparse
from dataclasses import replace
import importlib.util
import json
import math
from pathlib import Path
import runpy
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
HELPERS = runpy.run_path(str(ROOT / 'scripts/ui-ros-workflow.py'))
spec = importlib.util.spec_from_file_location('_studio_image_model', ROOT / 'fixtures/simple_sim/model.py')
MODEL = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = MODEL
spec.loader.exec_module(MODEL)
ENCODINGS = ('rgb8', 'mono8', 'bgr8', 'rgba8', 'bgra8')
TOPICS = (*ENCODINGS, 'format_changes', 'unsupported', 'malformed', 'retained', 'max_rgba', 'status')
PREFIX = '/studio_fixture/image/'
PADDING = {'rgb8': 2, 'mono8': 3, 'bgr8': 4, 'rgba8': 1, 'bgra8': 2}


def camera(suffix, counter):
    if suffix in ENCODINGS:
        return MODEL.camera(counter, encoding=suffix, padding=PADDING[suffix], big_endian=suffix in ('bgr8', 'bgra8'))
    if suffix == 'format_changes':
        encoding = ENCODINGS[counter % len(ENCODINGS)]
        width, height = (64, 48) if counter % 2 == 0 else (48, 32)
        return MODEL.camera(counter, width=width, height=height, encoding=encoding, padding=counter % 5, big_endian=bool(counter % 2))
    if suffix == 'max_rgba':
        return MODEL.camera(counter, width=256, height=256, encoding='rgba8')
    if suffix == 'unsupported':
        source = MODEL.camera(counter, encoding='mono8')
        raw = bytes(part for value in source.data for part in (value, 0))
        return replace(source, encoding='16UC1', step=source.width * 2, data=raw)
    if suffix == 'malformed':
        source = MODEL.camera(counter)
        return replace(source, data=source.data[:-1])
    if suffix == 'retained':
        return MODEL.camera(0)
    raise ValueError('Unknown Image fixture endpoint')


def frame_number(metadata, raw):
    channels = {'mono8': 1, 'rgb8': 3, 'bgr8': 3, 'rgba8': 4, 'bgra8': 4, '16UC1': 2}[metadata['encoding']]
    return int.from_bytes(bytes(raw[index * channels] for index in range(8)), 'little')


class Schedule:
    def __init__(self, now, hz, max_hz):
        if not all(math.isfinite(rate) and .2 <= rate <= 30 for rate in (hz, max_hz)):
            raise ValueError('Image rates must be finite and .2..30Hz')
        self.periods = {'images': 1 / hz, 'maximum': 1 / max_hz, 'status': .2, 'stats': 1.0}
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
        return max(0.0, min(.2, min(self.next.values()) - now))


def publisher(seconds, hz=10.0, max_hz=2.0):
    schedule = Schedule(time.monotonic(), hz, max_hz)  # validate before ROS import
    if not 1 <= seconds <= 3600:
        raise ValueError('Fixture duration must be1..3600seconds')
    folder = HELPERS['checked_environment']()
    import rclpy
    from rclpy.qos import QoSProfile, DurabilityPolicy
    from sensor_msgs.msg import Image
    from std_msgs.msg import String
    rclpy.init()
    node = rclpy.create_node('studio_image_workflow_fixture', start_parameter_services=False)
    publishers = {name: node.create_publisher(Image, PREFIX + name, QoSProfile(depth=1,
                  durability=DurabilityPolicy.TRANSIENT_LOCAL) if name == 'retained' else QoSProfile(depth=1))
                  for name in TOPICS if name != 'status'}
    status = node.create_publisher(String, PREFIX + 'status', QoSProfile(depth=10))
    log = folder / 'image-publisher.jsonl'
    counts = {name: 0 for name in TOPICS}
    started = time.monotonic()
    def emit(name, counter):
        frame = camera(name, counter)
        message = Image()
        message.header.stamp = node.get_clock().now().to_msg()
        message.header.frame_id = frame.frame_id
        message.width, message.height, message.encoding = frame.width, frame.height, frame.encoding
        message.is_bigendian, message.step, message.data = frame.is_bigendian, frame.step, frame.data
        publishers[name].publish(message)
        counts[name] += 1
        if name == 'retained':
            HELPERS['append'](log, {'event': 'retained-once', 'time': time.time(),
                'stamp': {'seconds': message.header.stamp.sec, 'nanoseconds': message.header.stamp.nanosec},
                'counter': 0, 'raw_bytes': len(frame.data)})
    HELPERS['append'](log, {'event': 'started', 'pid': __import__('os').getpid(), 'argv': sys.argv,
        'time': time.time(), 'image_hz_requested': hz, 'max_image_hz_requested': max_hz, 'status_hz_requested': 5,
        'topics': list(TOPICS), 'scope': 'Publisher calls only; no DDS delivery inferred'})
    try:
        emit('retained', 0)
        while rclpy.ok() and time.monotonic() - started < seconds:
            due = schedule.due(time.monotonic())
            if 'images' in due:
                for name in (*ENCODINGS, 'format_changes', 'unsupported', 'malformed'):
                    emit(name, counts[name])
            if 'maximum' in due:
                emit('max_rgba', counts['max_rgba'])
            if 'status' in due:
                status.publish(String(data=f'IMAGE-STATUS:{counts["status"]}'))
                counts['status'] += 1
            if 'stats' in due:
                HELPERS['append'](log, {'event': 'published-counts', 'time': time.time(),
                    'elapsed_monotonic_seconds': time.monotonic() - started, 'counts': dict(counts),
                    'scope': 'Actual publisher calls; no receiver delivery inferred'})
            rclpy.spin_once(node, timeout_sec=schedule.wait(time.monotonic()))
    except KeyboardInterrupt:
        pass
    finally:
        HELPERS['append'](log, {'event': 'stopped', 'time': time.time(),
            'elapsed_monotonic_seconds': time.monotonic() - started, 'counts': counts})
        node.destroy_node()
        if rclpy.ok():
            rclpy.shutdown()


def argument_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--publisher', action='store_true')
    parser.add_argument('--seconds', type=HELPERS['bounded_number'](1, 3600, int), default=120)
    parser.add_argument('--hz', type=HELPERS['bounded_number'](.2, 30, float), default=10.0)
    parser.add_argument('--max-hz', type=HELPERS['bounded_number'](.2, 30, float), default=2.0)
    return parser


if __name__ == '__main__':
    args = argument_parser().parse_args()
    if args.publisher:
        publisher(args.seconds, args.hz, args.max_hz)
    else:
        HELPERS['inventory']()
