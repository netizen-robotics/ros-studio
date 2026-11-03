"""Record Docker fixture checks and native Rust receipt via the localhost relay.

Start the dedicated simulation first. This leaves it running for UI exploration,
but always reaps the relay and native test backends that this check creates.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import selectors
import shlex
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]


def received_typed_sample(event, topic):
    sample = event.get('sample')
    return (event.get('event') == 'typed_sample' and isinstance(sample, dict)
            and sample.get('topic') == topic and sample.get('contract_version') == 2
            and isinstance(sample.get('value'), dict) and sample['value'].get('kind') == 'object'
            and bool(sample['value'].get('fields')))


def typed_member(sample, name):
    fields = sample.get('value', {}).get('fields', [])
    return next((field.get('value') for field in fields if field.get('name') == name), None)


def camera_metadata_received(events):
    """Legacy v2 proves metadata receipt only, never a complete camera payload."""
    for event in events:
        sample = event['sample']
        if sample.get('message_type') != 'sensor_msgs/msg/Image':
            continue
        if (typed_member(sample, 'width') == {'kind': 'unsigned', 'value': '64'}
                and typed_member(sample, 'height') == {'kind': 'unsigned', 'value': '48'}
                and typed_member(sample, 'step') == {'kind': 'unsigned', 'value': '192'}
                and typed_member(sample, 'encoding') == {'kind': 'text', 'value': 'rgb8', 'original_bytes': 4}):
            return True
    return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', required=True)
    args = parser.parse_args()
    if not args.label or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_' for c in args.label):
        parser.error('Use a descriptive alphanumeric label with hyphens/underscores')
    out = ROOT / 'artifacts/feasibility/evidence' / args.label
    report = ROOT / 'artifacts/feasibility' / f'{args.label}.json'
    if out.exists() or report.exists():
        parser.error('Choose an unused label to preserve evidence')
    out.mkdir(parents=True)
    rows = []
    metadata = json.loads(subprocess.check_output(['docker', 'image', 'inspect', 'ros-studio-simple-sim:jazzy']))[0]
    (out / 'image.json').write_text(json.dumps(metadata, indent=2) + '\n')
    packages = subprocess.check_output(['docker', 'exec', 'ros-studio-simple-sim-simulator-1', 'dpkg-query', '-W'], text=True)
    (out / 'container-packages.txt').write_text(packages)
    versions = {'image_id': metadata['Id'], 'python': platform.python_version(),
                'packages': str((out / 'container-packages.txt').relative_to(ROOT))}
    for path in ['Cargo.lock', 'pixi.lock', 'target/debug/studio-backend', 'target/debug/studio-desktop']:
        versions[path + '_sha256'] = hashlib.sha256((ROOT / path).read_bytes()).hexdigest()
    versions['fixture_sources'] = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                                   for path in sorted((ROOT / 'fixtures/simple_sim').glob('*.py'))}
    versions['relay_sha256'] = hashlib.sha256((ROOT / 'scripts/simple-sim-relay.py').read_bytes()).hexdigest()
    versions['verifier_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()

    def record(name, passed, command, evidence, native=False, rmw='rmw_cyclonedds_cpp', detail=None):
        rows.append({'test': name, 'outcome': 'passed' if passed else 'failed',
                     'os': platform.platform() if native else 'Linux Docker / Ubuntu 24.04 on macOS',
                     'cpu': platform.machine() if native else metadata['Architecture'], 'distro': 'jazzy',
                     'middleware_pair': [rmw, rmw], 'command': shlex.join(command), 'versions': versions,
                     'evidence': [str(path.relative_to(ROOT)) for path in evidence], 'detail': detail,
                     'scope': 'native Rust backend via development relay; no rendered window' if native else 'isolated synthetic container fixture; shared container network namespace'})
        print(name + ': ' + rows[-1]['outcome'], flush=True)
        report.write_text(json.dumps(rows, indent=2) + '\n')

    for name, command in [
        ('model-unit-tests', [sys.executable, 'fixtures/simple_sim/test_model.py']),
        ('relay-transport-tests', [sys.executable, 'scripts/test-simple-sim-relay.py']),
        ('camera-fixture-offline-tests', [sys.executable, 'fixtures/simple_sim/test_camera.py']),
        ('camera-transport-migration-tests', [sys.executable, 'scripts/test-simple-sim-camera.py']),
        ('container-ros-check', ['scripts/simple-sim', 'check']),
    ]:
        result = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=35)
        log = out / f'{name}.log'
        log.write_text(result.stdout)
        record(name, result.returncode == 0, command, [log], detail={'exit_code': result.returncode})
    for rmw in ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']:
        native_out = out / rmw
        native_out.mkdir()
        env = os.environ.copy()
        for key in ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI',
                    'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']:
            env.pop(key, None)
        env.update(ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1', ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST',
                   RMW_IMPLEMENTATION=rmw, STUDIO_UI_ROS_EVIDENCE=str(native_out), SKIP_DEFAULT_XML='1')
        inventory_command = [sys.executable, 'scripts/ui-ros-workflow.py']
        subprocess.run(inventory_command, cwd=ROOT, env=env, check=True, stdout=subprocess.DEVNULL)
        relay_command = [sys.executable, 'scripts/simple-sim-relay.py', '--seconds', '120']
        relay_log = native_out / 'relay.log'
        with relay_log.open('w') as stream:
            relay = subprocess.Popen(relay_command, cwd=ROOT, env=env, stdout=stream, stderr=subprocess.STDOUT)
            try:
                cases = [
                    ('status', '/studio_sim/status', 'std_msgs/msg/String', []),
                    ('odometry', '/studio_sim/odometry', 'nav_msgs/msg/Odometry', []),
                    ('joints', '/studio_sim/joint_states', 'sensor_msgs/msg/JointState', []),
                    ('scan', '/studio_sim/scan', 'sensor_msgs/msg/LaserScan', ['--best-effort']),
                    ('camera-metadata', '/studio_sim/camera', 'sensor_msgs/msg/Image', ['--best-effort']),
                    ('tf', '/tf', 'tf2_msgs/msg/TFMessage', []),
                    ('description', '/studio_sim/robot_description', 'std_msgs/msg/String', ['--transient-local']),
                    ('static-tf', '/tf_static', 'tf2_msgs/msg/TFMessage', ['--transient-local']),
                ]
                for name, topic, typ, flags in cases:
                    command = [str(ROOT / 'target/debug/studio-backend'), topic, typ, *flags]
                    stdout = native_out / f'{name}.jsonl'
                    stderr = native_out / f'{name}.stderr.log'
                    messages = []
                    with stderr.open('w') as err:
                        backend = subprocess.Popen(command, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=err)
                        selector = selectors.DefaultSelector()
                        selector.register(backend.stdout, selectors.EVENT_READ)
                        pending = b''
                        events = []
                        try:
                            deadline = time.monotonic() + 12
                            while time.monotonic() < deadline and not messages and backend.poll() is None and relay.poll() is None:
                                if not selector.select(timeout=0.2):
                                    continue
                                chunk = os.read(backend.stdout.fileno(), 65536)
                                if not chunk:
                                    break
                                pending += chunk
                                if len(pending) > 1048576:
                                    raise ValueError('Oversized backend protocol line')
                                while b'\n' in pending:
                                    line, pending = pending.split(b'\n', 1)
                                    events.append(json.loads(line))
                                    event = events[-1]
                                    if received_typed_sample(event, topic):
                                        messages.append(event)
                        finally:
                            selector.close()
                            backend.terminate()
                            try:
                                backend.wait(timeout=5)
                            except subprocess.TimeoutExpired:
                                backend.kill()
                                backend.wait(timeout=5)
                            backend.stdout.close()
                        stdout.write_text(''.join(json.dumps(event) + '\n' for event in events))
                        passed = bool(messages) and not any(e.get('event') == 'error' for e in events)
                        passed &= any(e.get('event') == 'hello' and e.get('version') == 2 for e in events)
                        if name == 'status':
                            passed &= 'Biggs-inspired' in json.dumps(messages) and 'simulation' in json.dumps(messages)
                        if name == 'camera-metadata':
                            passed &= camera_metadata_received(messages)
                        record('native-' + name, passed, command, [stdout, stderr, relay_log, native_out / 'context.json'],
                               native=True, rmw=rmw, detail={'backend_pid': backend.pid, 'reaped': backend.poll() is not None,
                                                          'relay_command': shlex.join(relay_command), 'samples': len(messages),
                                                          'camera_scope': 'legacy v2 metadata only; complete compact Image transport/rendering not tested' if name == 'camera-metadata' else None})
            finally:
                relay.send_signal(signal.SIGINT)
                try:
                    relay.wait(timeout=6)
                except subprocess.TimeoutExpired:
                    relay.kill()
                    relay.wait(timeout=5)
                record('relay-shutdown', relay.returncode == 0, relay_command, [relay_log, native_out / 'simple-sim-relay.jsonl'],
                       native=True, rmw=rmw, detail={'pid': relay.pid, 'exit_code': relay.returncode, 'reaped': True})
        interrupt_command = [sys.executable, 'scripts/test-simple-sim-interrupt.py']
        interrupt = subprocess.run(interrupt_command, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=12)
        interrupt_log = native_out / 'repeated-interrupt.log'
        interrupt_log.write_text(interrupt.stdout)
        record('repeated-interrupt-cleanup', interrupt.returncode == 0, interrupt_command, [interrupt_log],
               native=True, rmw=rmw, detail={'exit_code': interrupt.returncode, 'signal_target': 'test process itself'})
    compose = subprocess.check_output(['docker', 'compose', '-f', 'containers/compose.simple-sim.yaml', 'ps', '--format', 'json'], text=True)
    (out / 'compose-state.json').write_text(compose)
    (out / 'simulator.log').write_text(subprocess.check_output(['scripts/simple-sim', 'logs'], text=True))
    for options in [[], ['--check']]:
        subprocess.run([sys.executable, 'scripts/snapshot-evidence.py', *options], cwd=ROOT, check=True)
    return int(any(row['outcome'] != 'passed' for row in rows))


if __name__ == '__main__':
    raise SystemExit(main())
