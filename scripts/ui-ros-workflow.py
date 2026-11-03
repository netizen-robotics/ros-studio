"""Local synthetic ROS peers and an exec-only launch recorder for native UI QA.

Run with scripts/dev and its project Python. --backend is development-only:
it records exact launch arguments, then replaces itself with the real Rust
backend so the desktop still owns and terminates that backend directly.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
RUNTIME = ROOT / '.pixi/envs/default'
RMWS = ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']


def checked_environment():
    if Path(sys.prefix).resolve() != RUNTIME.resolve() or os.environ.get('ROS_DISTRO') != 'jazzy':
        raise SystemExit('Use the Jazzy project Python through scripts/dev')
    if os.environ.get('ROS_DOMAIN_ID') != '187' or os.environ.get('ROS_LOCALHOST_ONLY') != '1' or os.environ.get('ROS_AUTOMATIC_DISCOVERY_RANGE') != 'LOCALHOST':
        raise SystemExit('Only localhost fixture domain 187 is permitted')
    if os.environ.get('RMW_IMPLEMENTATION') not in RMWS:
        raise SystemExit('Choose an explicit supported RMW')
    for key in ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI',
                'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']:
        if os.environ.get(key):
            raise SystemExit(f'Remove remote/custom discovery setting {key}')
    out = Path(os.environ['STUDIO_UI_ROS_EVIDENCE']).resolve()
    if not out.is_relative_to(ROOT / 'artifacts/feasibility/evidence'):
        raise SystemExit('Evidence must stay in ignored artifacts/feasibility/evidence')
    out.mkdir(parents=True, exist_ok=True)
    return out


def append(path, value):
    with path.open('a') as stream:
        stream.write(json.dumps(value) + '\n')


def backend(arguments):
    out = checked_environment()
    diagnostic_profile = None
    if os.environ.get('STUDIO_MAP_DIAGNOSTIC_PROFILES') == '1':
        from map_dds_profiles import configure
        diagnostic_profile = configure(out)
    executable = ROOT / 'target/debug/studio-backend'
    append(out / 'backend-launches.jsonl', {
        'time': time.time(), 'pid': os.getpid(), 'argv': [str(executable), *arguments],
        'environment': {k: os.environ.get(k) for k in [
            'ROS_DISTRO', 'ROS_DOMAIN_ID', 'ROS_LOCALHOST_ONLY', 'ROS_AUTOMATIC_DISCOVERY_RANGE',
            'RMW_IMPLEMENTATION', 'AMENT_PREFIX_PATH', 'DYLD_LIBRARY_PATH', 'SKIP_DEFAULT_XML',
            'STUDIO_MAP_DIAGNOSTIC_PROFILES', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'CYCLONEDDS_URI']},
        'diagnostic_map_profile': diagnostic_profile,
        'backend_sha256': hashlib.sha256(executable.read_bytes()).hexdigest(),
    })
    os.execv(str(executable), [str(executable), *arguments])


def validate_flood_options(rate, size):
    if rate is not None and (not math.isfinite(rate) or not 1 <= rate <= 2000):
        raise ValueError('Flood rate must be finite and between 1 and 2000 Hz')
    if type(size) is not int or not 1 <= size <= 8192:
        raise ValueError('Flood bytes must be an integer between 1 and 8192')


class PublishSchedule:
    """Constant-memory monotonic deadlines; a late iteration never catches up in a burst."""
    def __init__(self, rate, now):
        validate_flood_options(rate, 1)
        self.rate = rate
        self.regular_at = now
        self.flood_at = now

    @staticmethod
    def next_deadline(deadline, period, now):
        return deadline + (math.floor((now - deadline) / period) + 1) * period

    def due(self, now):
        regular = now >= self.regular_at
        flood = self.rate is not None and now >= self.flood_at
        if regular:
            self.regular_at = self.next_deadline(self.regular_at, 0.2, now)
        if flood:
            self.flood_at = self.next_deadline(self.flood_at, 1 / self.rate, now)
        return regular, flood

    def wait(self, now):
        next_at = self.regular_at if self.rate is None else min(self.regular_at, self.flood_at)
        return max(0.0, min(0.2, next_at - now))


def publisher(seconds, numeric_session=False, flood_rate=None, flood_bytes=1024):
    validate_flood_options(flood_rate, flood_bytes)  # reject before importing/creating ROS peers
    out = checked_environment()
    import rclpy
    from rclpy.qos import QoSProfile, ReliabilityPolicy, DurabilityPolicy
    from std_msgs.msg import String
    rclpy.init()
    node = rclpy.create_node('ui_workflow_fixture', namespace='/studio_fixture/ui')
    rmw = os.environ['RMW_IMPLEMENTATION']
    name = 'fast' if rmw == RMWS[0] else 'cyclone'
    reliable = node.create_publisher(String, '/studio_fixture/ui/reliable', QoSProfile(depth=10))
    best = node.create_publisher(String, '/studio_fixture/ui/best_effort', QoSProfile(depth=10, reliability=ReliabilityPolicy.BEST_EFFORT))
    retained = node.create_publisher(String, '/studio_fixture/ui/retained', QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL))
    numeric = None
    if numeric_session:
        from std_msgs.msg import UInt64
        numeric = node.create_publisher(UInt64, '/studio_fixture/ui/numeric', QoSProfile(depth=10))
    flood = None
    if flood_rate is not None:
        flood = node.create_publisher(String, '/studio_fixture/ui/flood', QoSProfile(depth=10))
    flood_message = String(data='F' * flood_bytes) if flood is not None else None
    log = out / f'publisher-{name}.jsonl'
    append(log, {'event': 'started', 'pid': os.getpid(), 'time': time.time(), 'rmw': rmw,
                 'argv': sys.argv, 'domain': '187', 'localhost_only': True,
                 'configured_regular_hz': 5, 'configured_flood_hz': flood_rate,
                 'configured_flood_bytes': flood_bytes if flood is not None else 0})
    deadline = time.monotonic() + seconds
    count = 0
    flood_count = 0
    started_at = time.monotonic()
    schedule = PublishSchedule(flood_rate, started_at)
    report_at = started_at + 1
    try:
        retained.publish(String(data=f'UI-ROS-{name}-retained-once'))
        append(log, {'event': 'retained-once', 'value': f'UI-ROS-{name}-retained-once', 'time': time.time()})
        while rclpy.ok() and time.monotonic() < deadline:
            now = time.monotonic()
            regular_due, flood_due = schedule.due(now)
            if regular_due:
                if numeric is not None:
                    numeric.publish(UInt64(data=count))
                for suffix, pub in [('reliable', reliable), ('best-effort', best)]:
                    value = f'UI-ROS-{name}-{suffix}:{count}'
                    pub.publish(String(data=value))
                    append(log, {'event': 'published', 'value': value, 'time': time.time()})
                count += 1
            if flood_due:
                flood.publish(flood_message)
                flood_count += 1
            if flood is not None and now >= report_at:
                append(log, {'event': 'flood-published-counts', 'time': time.time(),
                             'elapsed_monotonic_seconds': now - started_at,
                             'regular_cycles': count, 'flood_published': flood_count,
                             'configured_flood_hz': flood_rate, 'configured_flood_bytes': flood_bytes,
                             'scope': 'Publisher calls only; no DDS delivery is inferred'})
                report_at = now + 1
            rclpy.spin_once(node, timeout_sec=schedule.wait(time.monotonic()))
    except KeyboardInterrupt:
        pass
    finally:
        if flood is not None:
            append(log, {'event': 'flood-final-counts', 'time': time.time(),
                         'elapsed_monotonic_seconds': time.monotonic() - started_at,
                         'regular_cycles': count, 'flood_published': flood_count,
                         'scope': 'Publisher calls only; no DDS delivery is inferred'})
        append(log, {'event': 'stopped', 'time': time.time()})
        node.destroy_node()
        if rclpy.ok():
            rclpy.shutdown()


def inventory():
    out = checked_environment()
    packages = []
    for path in sorted((RUNTIME / 'conda-meta').glob('*.json')):
        entry = json.loads(path.read_text())
        packages.append({k: entry[k] for k in ['name', 'version', 'build', 'subdir', 'sha256'] if k in entry})
    (out / 'dependencies.json').write_text(json.dumps(packages, indent=2) + '\n')
    versions = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in [ROOT / 'Cargo.lock', ROOT / 'pixi.lock', ROOT / 'target/debug/studio-desktop', ROOT / 'target/debug/studio-backend', Path(__file__).resolve()]}
    (out / 'context.json').write_text(json.dumps({'os': platform.platform(), 'cpu': platform.machine(), 'distro': 'jazzy',
        'versions': versions, 'python': platform.python_version(), 'eframe': '0.31.1', 'egui': '0.31.1',
        'dependency_inventory': str((out / 'dependencies.json').relative_to(ROOT))}, indent=2) + '\n')
    print(out)


def bounded_number(low, high, cast):
    def parse(value):
        try:
            number = cast(value)
            if not low <= number <= high or not math.isfinite(number):
                raise ValueError()
            return number
        except ValueError:
            raise argparse.ArgumentTypeError(f'Choose a value from {low} to {high}') from None
    return parse


def argument_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--publisher', action='store_true')
    parser.add_argument('--seconds', type=int, default=600)
    parser.add_argument('--numeric-session', action='store_true', help='Add UInt64 for concurrent backend acceptance')
    parser.add_argument('--flood-rate', type=bounded_number(1, 2000, float), help='Add bounded String flood publisher at requested 1..2000 Hz')
    parser.add_argument('--flood-bytes', type=bounded_number(1, 8192, int), default=1024, help='Flood String UTF-8 bytes (1..8192); no per-message flood log')
    return parser


if __name__ == '__main__':
    if sys.argv[1:2] == ['--backend']:
        backend(sys.argv[2:])
    else:
        parser = argument_parser()
        args = parser.parse_args()
        if args.publisher:
            publisher(args.seconds, args.numeric_session, args.flood_rate, args.flood_bytes)
        else:
            inventory()
