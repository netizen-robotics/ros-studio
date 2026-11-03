"""Guarded original Robot TF fixture; no motion or robot command publisher.

Publisher terminal (set the README localhost/domain187 environment first):
  scripts/dev .pixi/envs/default/bin/python scripts/robot-ros-workflow.py \
    --rmw rmw_fastrtps_cpp --evidence artifacts/feasibility/evidence/UNUSED-LABEL \
    --mode recover --seconds 600
Desktop terminal uses the same RMW/isolation settings, STUDIO_UI_ROS_EVIDENCE
pointing to that directory, and STUDIO_BACKEND=$PWD/scripts/ui-ros-backend.
Load fixtures/robot_visualizer/articulated.urdf; map fixture_robot to that
fixture directory. Select map, /robot_demo/tf and /robot_demo/tf_static.
Explicitly verify System clock, then enable Robot and Float64 status peers.

recover starts with only tip missing. Write `ready` to EVIDENCE/mode.txt to
release its retained static edge once. Latest TF cannot be unpublished: after
release, `waiting` does not remove tip; use a fresh publisher/backend session
for another initial-missing case. nominal includes tip from startup; missing
never releases it. No mode executes commands or changes a physical robot.
Write the control atomically (write a temporary file, then rename to mode.txt)
so the publisher never observes a truncated intermediate value.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import runpy
import stat
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
HELPERS = runpy.run_path(str(ROOT / 'scripts/ui-ros-workflow.py'))
RMWS = tuple(HELPERS['RMWS'])
MODES = ('nominal', 'missing', 'recover')
LINKS = ('base', 'arm', 'tip', 'ascii_mesh', 'binary_mesh')
TF_TOPIC = '/robot_demo/tf'
STATIC_TOPIC = '/robot_demo/tf_static'
STATUS_TOPICS = ('/robot_demo/status', '/robot_demo/status_two')
RATE_HZ = 5


def bounded_seconds(value):
    if type(value) not in (str, int):
        raise argparse.ArgumentTypeError('seconds must be an integer in 1..3600')
    try:
        seconds = int(value)
    except (ValueError, TypeError):
        raise argparse.ArgumentTypeError('seconds must be an integer in 1..3600') from None
    if not 1 <= seconds <= 3600:
        raise argparse.ArgumentTypeError('seconds must be an integer in 1..3600')
    return seconds


def argument_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rmw', choices=RMWS, required=True)
    parser.add_argument('--evidence', type=Path, required=True)
    parser.add_argument('--mode', choices=MODES, default='nominal')
    parser.add_argument('--seconds', type=bounded_seconds, default=600)
    return parser


def checked_environment(rmw, evidence):
    """Reuse the existing no-remote-peer/runtime guard before any ROS import."""
    if rmw not in RMWS or os.environ.get('RMW_IMPLEMENTATION') != rmw:
        raise SystemExit('Set RMW_IMPLEMENTATION to the same explicitly selected --rmw')
    out = Path(evidence).resolve()
    if not out.is_relative_to(ROOT / 'artifacts/feasibility/evidence'):
        raise SystemExit('Evidence must stay in ignored artifacts/feasibility/evidence')
    # A caller selects this path explicitly; do not silently reuse old captures.
    if out.exists():
        raise SystemExit('Use a new unused Robot evidence directory')
    os.environ['STUDIO_UI_ROS_EVIDENCE'] = str(out)
    return HELPERS['checked_environment']()


def transform(parent, child, xyz, quaternion=(0.0, 0.0, 0.0, 1.0)):
    return {'parent': parent, 'child': child, 'translation': list(xyz),
            'rotation_xyzw': list(quaternion)}


def static_transforms(include_tip):
    transforms = [transform('map', 'base', (0.0, 0.0, 0.0)),
                  transform('base', 'ascii_mesh', (0.0, 0.0, 0.0)),
                  transform('base', 'binary_mesh', (0.0, 0.0, 0.0))]
    if include_tip:
        transforms.append(transform('arm', 'tip', (0.0, 0.0, 0.6)))
    return transforms


def dynamic_transform(elapsed):
    if not math.isfinite(elapsed) or elapsed < 0:
        raise ValueError('elapsed must be finite and nonnegative')
    angle = 0.8 * math.sin(elapsed * 0.5)
    return transform('base', 'arm', (0.0, 0.0, 0.2),
                     (0.0, math.sin(angle / 2), 0.0, math.cos(angle / 2)))


def expected_tip_center(angle):
    """Analytic fixed-frame center, including the tip visual's .05m origin."""
    return [0.65 * math.sin(angle), 0.0, 0.2 + 0.65 * math.cos(angle)]


class ModeState:
    """Initial missing -> released is one way within a native TF buffer epoch."""
    def __init__(self, mode):
        if mode not in MODES:
            raise ValueError('unsupported Robot fixture mode')
        self.mode = mode
        self.tip_released = mode == 'nominal'

    def advance(self, control):
        if control not in ('waiting', 'ready'):
            raise ValueError('mode.txt must contain waiting or ready')
        release = self.mode == 'recover' and control == 'ready' and not self.tip_released
        self.tip_released |= release
        return release


def read_control(path):
    """Bounded regular-file control only; FIFOs/symlinks never block the loop."""
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return 'waiting'
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > 64:
        raise ValueError('mode.txt must be a regular nonsymlink file at most 64 bytes')
    flags = os.O_RDONLY | getattr(os, 'O_NONBLOCK', 0) | getattr(os, 'O_NOFOLLOW', 0)
    with os.fdopen(os.open(path, flags), 'rb') as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ValueError('mode.txt changed to a nonregular file')
        data = stream.read(65)
    if len(data) > 64:
        raise ValueError('mode.txt exceeds 64 bytes')
    value = data.decode('ascii').strip()
    if value not in ('waiting', 'ready'):
        raise ValueError('mode.txt must contain waiting or ready')
    return value


def publisher(rmw, evidence, mode, seconds):
    # Reject malformed arguments and unsafe environments before ROS is imported.
    seconds = bounded_seconds(seconds)
    state = ModeState(mode)
    folder = checked_environment(rmw, evidence)
    HELPERS['inventory']()
    import rclpy
    from geometry_msgs.msg import TransformStamped
    from rclpy.qos import DurabilityPolicy, QoSProfile, ReliabilityPolicy
    from std_msgs.msg import Float64
    from tf2_msgs.msg import TFMessage

    rclpy.init()
    node = rclpy.create_node('studio_robot_visualizer_fixture', start_parameter_services=False)
    log = folder / 'robot-publisher.jsonl'
    counts = {'dynamic': 0, 'static': 0, 'status': 0, 'status_two': 0}
    started = time.monotonic()
    ready = folder / 'publisher-state.json'

    def message(values, stamp):
        batch = TFMessage()
        for value in values:
            item = TransformStamped()
            item.header.frame_id = value['parent']
            item.child_frame_id = value['child']
            if stamp is not None:
                item.header.stamp = stamp
            xyz, q = value['translation'], value['rotation_xyzw']
            item.transform.translation.x, item.transform.translation.y, item.transform.translation.z = xyz
            item.transform.rotation.x, item.transform.rotation.y, item.transform.rotation.z, item.transform.rotation.w = q
            batch.transforms.append(item)
        return batch

    def snapshot(event, elapsed):
        value = {'event': event, 'pid': os.getpid(), 'rmw': rmw, 'mode': mode,
                 'tip_released': state.tip_released, 'elapsed_seconds': elapsed,
                 'publisher_calls': dict(counts), 'fixed_frame': 'map', 'requested_link_order': LINKS,
                 'configured_dynamic_hz': RATE_HZ, 'time': time.time(),
                 'limits': 'Publisher calls are not DDS delivery, UI rate or physical motion evidence.'}
        HELPERS['append'](log, value)
        ready.write_text(json.dumps(value, indent=2) + '\n')

    try:
        dynamic = node.create_publisher(TFMessage, TF_TOPIC, QoSProfile(depth=10, reliability=ReliabilityPolicy.RELIABLE))
        retained = node.create_publisher(TFMessage, STATIC_TOPIC,
                                        QoSProfile(depth=1, reliability=ReliabilityPolicy.RELIABLE,
                                                   durability=DurabilityPolicy.TRANSIENT_LOCAL))
        peers = [node.create_publisher(Float64, topic, QoSProfile(depth=10)) for topic in STATUS_TOPICS]
        assert node.get_parameter('use_sim_time').value is False, 'System-clock fixture only'
        inputs = [Path(__file__).resolve(), ROOT / 'fixtures/robot_visualizer/articulated.urdf',
                  ROOT / 'fixtures/robot_visualizer/meshes/wedge-ascii.stl',
                  ROOT / 'fixtures/robot_visualizer/meshes/wedge-binary.stl']
        HELPERS['append'](log, {'event': 'started', 'pid': os.getpid(), 'argv': sys.argv,
                               'time': time.time(), 'rmw': rmw, 'domain': 187, 'localhost_only': True,
                               'source_sha256': {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in inputs},
                               'topics': [TF_TOPIC, STATIC_TOPIC, *STATUS_TOPICS]})
        retained.publish(message(static_transforms(state.tip_released), None))
        counts['static'] += 1
        schedule = HELPERS['PublishSchedule'](None, started)
        while rclpy.ok() and time.monotonic() - started < seconds:
            now = time.monotonic()
            due, _ = schedule.due(now)
            if due:
                if state.advance(read_control(folder / 'mode.txt')):
                    # Publish the complete retained set, not just the new edge.
                    retained.publish(message(static_transforms(True), None))
                    counts['static'] += 1
                    snapshot('tip-released', now - started)
                elapsed = now - started
                value = dynamic_transform(elapsed)
                stamp = node.get_clock().now().to_msg()
                assert stamp.sec > 0, 'Positive system acquisition stamp required'
                dynamic.publish(message([value], stamp))
                counts['dynamic'] += 1
                for index, peer in enumerate(peers):
                    peer.publish(Float64(data=float(counts['dynamic']) + index * 0.5))
                    counts['status' if index == 0 else 'status_two'] += 1
                if counts['dynamic'] == 1 or counts['dynamic'] % 25 == 0:
                    angle = 0.8 * math.sin(elapsed * 0.5)
                    example = {'dynamic': value, 'stamp': {'sec': stamp.sec, 'nanosec': stamp.nanosec},
                               'static': static_transforms(state.tip_released),
                               'expected_tip_visual_center_map': expected_tip_center(angle)}
                    (folder / 'tf-source-example.json').write_text(json.dumps(example, indent=2) + '\n')
                    snapshot('publishing', elapsed)
            rclpy.spin_once(node, timeout_sec=min(0.02, schedule.wait(time.monotonic())))
    except KeyboardInterrupt:
        pass
    finally:
        try:
            snapshot('stopped', time.monotonic() - started)
        finally:
            node.destroy_node()
            if rclpy.ok():
                rclpy.shutdown()


def main():
    args = argument_parser().parse_args()
    publisher(args.rmw, args.evidence, args.mode, args.seconds)


if __name__ == '__main__':
    main()
