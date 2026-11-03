"""Session11 description capture through the actual Rust backend and local DDS.

Use scripts/dev .pixi/envs/default/bin/python; domain187 and matching RMW only.
No native-window, model-loading, packaging or physical robot acceptance claim.
"""
import argparse
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import queue
import signal
import subprocess
import sys
import time
import traceback

ROOT = Path(__file__).resolve().parents[1]
MAX_DOCUMENT = 1048576
CHUNK = 196608
PREFIX = '/studio_fixture/description/'
RMWS = ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']
REMOTE = ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI',
          'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']


def document():
    start = '<robot name="topic_fixture"><!--'
    end = '--><link name="base"/></robot>'
    return start + 'x' * (MAX_DOCUMENT - len(start) - len(end)) + end


def publisher(out):
    if (os.environ.get('ROS_DOMAIN_ID') != '187' or os.environ.get('ROS_LOCALHOST_ONLY') != '1'
            or os.environ.get('ROS_AUTOMATIC_DISCOVERY_RANGE') != 'LOCALHOST'
            or os.environ.get('RMW_IMPLEMENTATION') not in RMWS or any(os.environ.get(k) for k in REMOTE)):
        raise SystemExit('Requires explicit matching RMW, localhost domain187, no remote discovery settings')
    import rclpy
    from std_msgs.msg import String
    from rclpy.qos import QoSProfile, DurabilityPolicy, ReliabilityPolicy
    running = True

    def stop(*_):
        nonlocal running
        running = False
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    rclpy.init()
    node = rclpy.create_node('description_capture_fixture')
    retained = QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL)
    full = node.create_publisher(String, PREFIX + 'retained', retained)
    oversize = node.create_publisher(String, PREFIX + 'oversize', retained)
    best = node.create_publisher(String, PREFIX + 'best', QoSProfile(depth=1, reliability=ReliabilityPolicy.BEST_EFFORT))
    peer = node.create_publisher(String, PREFIX + 'peer', 1)
    full.publish(String(data=document()))
    oversize.publish(String(data=document() + 'x'))
    out.write_text(json.dumps(dict(pid=os.getpid(), domain=187, rmw=os.environ['RMW_IMPLEMENTATION'], retained_bytes=MAX_DOCUMENT)) + '\n')
    try:
        while running:
            peer.publish(String(data='peer remains usable'))
            best.publish(String(data='<robot name="best"><link name="base"/></robot>'))
            rclpy.spin_once(node, timeout_sec=.05)
    finally:
        node.destroy_node()
        rclpy.shutdown()


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def configuration(topic='retained', best=False, retained=True, output='robot_description'):
    return dict(topic=PREFIX + topic, message_type='std_msgs/msg/String', best_effort=best,
                transient_local=retained, output=output, robot=None, spatial=None)


def capture(backend, sub, expected):
    data = bytearray()
    count = (len(expected.encode()) + CHUNK - 1) // CHUNK
    for index in range(count):
        event = backend.wait(lambda e: e.get('event') == 'robot_description_observation' and e.get('subscription') == sub)
        sample = event['sample']
        assert event['generation'] == backend.generation and event['sequence'] == index + 1
        assert event['backend_drops'] == 0
        assert sample == {**sample, 'contract_version': 1, 'message_type': 'std_msgs/msg/String',
                          'index': index, 'count': count, 'document_bytes': len(expected.encode()),
                          'sha256': hashlib.sha256(expected.encode()).hexdigest()}
        raw = base64.b64decode(sample['data_base64'], validate=True)
        assert base64.b64encode(raw).decode() == sample['data_base64']
        assert len(raw) == min(CHUNK, len(expected.encode()) - index * CHUNK)
        data.extend(raw)
    assert data.decode() == expected
    return dict(bytes=len(data), chunks=count, sha256=hashlib.sha256(data).hexdigest())


def absent(backend, sub, duration=.5):
    deadline = time.monotonic() + duration
    while time.monotonic() < deadline:
        assert backend.error is None and backend.process.poll() is None
        try:
            event = backend.events.get(timeout=.02)
        except queue.Empty:
            continue
        assert not (event.get('subscription') == sub and event.get('event') in
                    ('robot_description_observation', 'stream_error', 'request_error')), event


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--publisher', type=Path)
    parser.add_argument('--label')
    args = parser.parse_args()
    if Path(sys.prefix).resolve() != (ROOT / '.pixi/envs/default').resolve():
        parser.error('Use the project Pixi interpreter')
    if args.publisher:
        publisher(args.publisher)
        return
    if not args.label or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_' for c in args.label):
        parser.error('Use an unused alphanumeric evidence label')
    folder = ROOT / 'artifacts/feasibility/evidence' / args.label
    report = ROOT / 'artifacts/feasibility' / (args.label + '.json')
    if folder.exists() or report.exists():
        parser.error('Preserve previous evidence; use a fresh label')
    folder.mkdir(parents=True)
    robot = load('description_backend_base', 'scripts/test-robot-session.py')
    sensor = load('description_ready_contract', 'scripts/test-sensor-session.py')
    source_paths = ['scripts/test-description-session.py', 'crates/protocol/src/robot_description.rs',
                    'crates/protocol/src/session.rs', 'crates/protocol/src/subscriptions/registry.rs',
                    'crates/backend/src/description_codec.rs', 'crates/backend/src/description_worker.rs',
                    'crates/backend/src/native_topic.rs', 'crates/backend/src/observation.rs']
    digest = lambda path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest()
    versions = dict(rust=subprocess.check_output(['rustc', '--version'], text=True).strip(), python=platform.python_version(),
                    backend_sha256=digest('target/debug/studio-backend'), cargo_lock_sha256=digest('Cargo.lock'), pixi_lock_sha256=digest('pixi.lock'))
    sources = {path: digest(path) for path in source_paths}
    original, rows, failed = dict(os.environ), [], False
    for rmw in RMWS:
        os.environ.clear()
        os.environ.update({k: v for k, v in original.items() if k not in REMOTE})
        os.environ.update(ROS_DISTRO='jazzy', ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1',
                          ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST', RMW_IMPLEMENTATION=rmw)
        out = folder / rmw
        out.mkdir()
        ready = out / 'publisher.json'
        command = [sys.executable, str(ROOT / 'scripts/test-description-session.py'), '--publisher', str(ready)]
        publishers, backends, pids, cases, measurements = [], [], [], [], []
        outcome, details = 'passed', None
        try:
            with (out / 'publisher.log').open('w') as output:
                pub = subprocess.Popen(command, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT)
            publishers.append(pub)
            pids.append(pub.pid)
            deadline = time.monotonic() + 100
            while not ready.exists():
                assert pub.poll() is None and time.monotonic() < deadline
                time.sleep(.02)

            def connect(index):
                backend = robot.Backend(out, index, 1100 + index, deadline)
                backends.append(backend)
                pids.append(backend.process.pid)
                event = backend.wait(lambda e: e.get('event') == 'ready')
                assert set(event) == {'event', 'distro', 'rmw', *sensor.EXPECTED}
                assert all(event[k] == v for k, v in sensor.EXPECTED.items())
                assert event['rmw'] == rmw
                backend.wait(lambda e: e.get('event') == 'graph' and any(t == PREFIX + 'retained' and 'std_msgs/msg/String' in ty for t, ty in e['topics']))
                return backend

            backend = connect(1)
            backend.start_configuration(1, configuration('peer', retained=False, output='typed'))
            backend.start_configuration(2, configuration(retained=False))
            absent(backend, 2)
            backend.wait(lambda e: e.get('event') == 'observation' and e.get('subscription') == 1)
            backend.stop_robot(2)
            cases.append('Volatile late join waits without retained data; peer continues; explicit Stop')
            began = time.monotonic()
            backend.start_configuration(3, configuration())
            measurement = capture(backend, 3, document())
            measurement['start_to_complete_seconds'] = time.monotonic() - began
            measurements.append(measurement)
            absent(backend, 3)
            cases.append('Reliable TransientLocal late join captures exact 1 MiB in six chunks once')
            request = backend.start_configuration(4, configuration(), ack=False)
            event = backend.wait(lambda e: e.get('event') == 'request_error' and e.get('request') == request)
            assert event['no_handle'] is True
            backend.stop_robot(3)
            backend.wait(lambda e: e.get('event') == 'observation' and e.get('subscription') == 1)
            cases.append('single capture reservation persists until exact Drop/Stop while peer survives')
            backend.start_configuration(5, configuration('oversize'))
            error = backend.wait(lambda e: e.get('event') == 'stream_error' and e.get('subscription') == 5)
            assert '1 MiB' in error['message']
            backend.wait(lambda e: e.get('event') == 'observation' and e.get('subscription') == 1)
            cases.append('oversized native String rejected without partial data or peer failure')
            backend.start_configuration(6, configuration('best', retained=False))
            absent(backend, 6)
            backend.stop_robot(6)
            backend.start_configuration(7, configuration('best', best=True, retained=False))
            capture(backend, 7, '<robot name="best"><link name="base"/></robot>')
            backend.stop_robot(7)
            cases.append('Reliable/BestEffort mismatch waits; explicit QoS correction captures')
            backend.start_configuration(8, configuration())
            # EOF with a live capture owns the cleanup path; do not request Stop.
            backend.close()
            backend = connect(2)
            absent(backend, 8)
            cases.append('EOF cleans up capture/peer; new handshake/discovery never replays')
        except Exception:
            failed, outcome, details = True, 'failed', traceback.format_exc()
        finally:
            for backend in backends:
                try:
                    backend.close()
                except Exception:
                    failed, outcome = True, 'failed'
                    details = (details or '') + traceback.format_exc()
            for pub in publishers:
                if pub.poll() is None:
                    pub.send_signal(signal.SIGTERM)
                try:
                    pub.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    pub.kill()
                    pub.wait()
                    failed, outcome = True, 'failed'
                    details = (details or '') + 'Publisher forced cleanup'
            cleanup = []
            for pid in pids:
                try:
                    os.kill(pid, 0)
                    gone = False
                except ProcessLookupError:
                    gone = True
                cleanup.append(dict(pid=pid, absent=gone))
            if not all(p['absent'] for p in cleanup):
                failed, outcome = True, 'failed'
            (out / 'cleanup.json').write_text(json.dumps(cleanup, indent=2) + '\n')
            rows.append(dict(test='description-actual-backend', outcome=outcome, os=platform.platform(), cpu=platform.machine(), distro='jazzy',
                             middleware_pair=[rmw, rmw], versions=versions, source_sha256=sources, completed_cases=cases,
                             measurements=measurements, details=details,
                             command=['scripts/dev', '.pixi/envs/default/bin/python', 'scripts/test-description-session.py', '--label', args.label],
                             publisher_command=command, backend_command=['target/debug/studio-backend', '--observe-session'],
                             environment={k: os.environ[k] for k in ['ROS_DOMAIN_ID', 'ROS_LOCALHOST_ONLY', 'ROS_AUTOMATIC_DISCOVERY_RANGE', 'RMW_IMPLEMENTATION']},
                             evidence=[str(p.relative_to(ROOT)) for p in sorted(out.iterdir())],
                             scope='Local synthetic ROS and actual Rust/native reader only; not UI, model-loading, RSS or release acceptance.'))
            report.write_text(json.dumps(rows, indent=2) + '\n')
            print(rmw, outcome, len(cases), 'cases', flush=True)
    os.environ.clear()
    os.environ.update(original)
    if failed:
        raise SystemExit('Description boundary failed; preserve evidence')


if __name__ == '__main__':
    main()
