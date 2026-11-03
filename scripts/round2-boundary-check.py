"""Check actual Rust observation backend against a separately running round2 publisher.

Does not start a publisher. Every backend child is recorded and reaped, including failures.
Run once per matching explicit middleware pair; domain187/localhost guards apply.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import selectors
import shlex
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('workflow', ROOT / 'scripts/ui-ros-workflow.py')
workflow = importlib.util.module_from_spec(spec)
spec.loader.exec_module(workflow)


def member(value, *path):
    for segment in path:
        value = (value['items'][segment] if isinstance(segment, int) else
                 next(field['value'] for field in value['fields'] if field['name'] == segment))
    return value


def inspect_backend(binary, arguments, evidence, seconds=6):
    command = [str(binary), *arguments]
    stdout = evidence.with_suffix('.stdout.jsonl')
    stderr = evidence.with_suffix('.stderr.log')
    events = []
    with stderr.open('wb') as errors, stdout.open('wb') as raw:
        child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=errors)
        workflow.append(evidence.parent / 'children.jsonl', {'pid': child.pid, 'argv': command, 'time': time.time()})
        selector = selectors.DefaultSelector()
        selector.register(child.stdout, selectors.EVENT_READ)
        # Unbuffered os.read avoids readline blocking on partial or buffered events.
        pending = b''
        try:
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                for key, _ in selector.select(min(0.2, max(0, deadline - time.monotonic()))):
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if not chunk:
                        return events
                    raw.write(chunk)
                    raw.flush()
                    pending += chunk
                    if len(pending) > 1024 * 1024:
                        raise AssertionError('backend line exceeds 1MiB')
                    while b'\n' in pending:
                        line, pending = pending.split(b'\n', 1)
                        event = json.loads(line)
                        events.append(event)
                        if event['event'] == 'typed_sample':
                            assert len(json.dumps(event['sample'], ensure_ascii=False, separators=(',', ':')).encode()) <= 512 * 1024
            return events
        finally:
            selector.close()
            if child.poll() is None:
                child.terminate()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=5)
            child.stdout.close()
            workflow.append(evidence.parent / 'children.jsonl', {'pid': child.pid, 'event': 'reaped', 'returncode': child.returncode})


def check_events(events):
    hello = next(e for e in events if e['event'] == 'hello')
    assert hello['version'] == 2, hello
    assert hello['rmw'] == os.environ['RMW_IMPLEMENTATION'], hello
    errors = [e for e in events if e['event'] == 'error']
    assert not errors, errors
    samples = [e['sample'] for e in events if e['event'] == 'typed_sample']
    for sample in samples:
        assert sample['contract_version'] == 2
        assert sample['source_time'] is None, 'header time must not silently become qualified source time'
    return samples


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', required=True)
    parser.add_argument('--backend', type=Path, default=ROOT / 'target/debug/studio-backend')
    parser.add_argument('--interface-bundle', type=Path)
    parser.add_argument('--timeout', type=float, default=6)
    args = parser.parse_args()
    if not 0 < args.timeout <= 60:
        parser.error('timeout must be in (0,60]')
    base = workflow.checked_environment()
    if not args.label or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_' for c in args.label):
        parser.error('invalid unused evidence label')
    evidence = base / args.label
    evidence.mkdir(exist_ok=False)
    binary = args.backend.resolve(strict=True)
    versions = {'backend_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'python': platform.python_version()}
    for name in ['Cargo.lock', 'pixi.lock']:
        versions[name + '_sha256'] = hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
    unsafe = ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI',
              'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']
    environment = {key: os.environ.get(key) for key in ['ROS_DISTRO', 'ROS_DOMAIN_ID', 'ROS_LOCALHOST_ONLY',
                   'ROS_AUTOMATIC_DISCOVERY_RANGE', 'RMW_IMPLEMENTATION', 'STUDIO_UI_ROS_EVIDENCE',
                   'AMENT_PREFIX_PATH', 'CMAKE_PREFIX_PATH', 'DYLD_LIBRARY_PATH', 'LD_LIBRARY_PATH']}
    isolation_prefix = ['env', *[part for key in unsafe for part in ['-u', key]],
              *[key + '=' + value for key, value in environment.items() if value is not None],
              'scripts/dev']
    (evidence / 'context.json').write_text(json.dumps({'environment': environment, 'argv': os.sys.argv,
        'publisher_launches': [json.loads(line) for line in (base / 'round2-publisher.jsonl').read_text().splitlines()]
            if (base / 'round2-publisher.jsonl').exists() else [],
        'versions': versions}, indent=2) + '\n')
    rows = []

    def run(name, arguments, assertion, seconds=None):
        events = []
        outcome, detail = 'passed', ''
        try:
            events = inspect_backend(binary, arguments, evidence / name, seconds or args.timeout)
            samples = check_events(events)
            assertion(samples, events)
        except Exception as error:
            outcome, detail = 'failed', repr(error)
        rows.append({'test': name, 'outcome': outcome, 'detail': detail, 'os': platform.platform(), 'cpu': platform.machine(),
                     'distro': 'jazzy', 'middleware_pair': [os.environ['RMW_IMPLEMENTATION']] * 2, 'versions': versions,
                     'command': shlex.join([*isolation_prefix, str(binary), *arguments]), 'evidence': [str(p.relative_to(ROOT)) for p in evidence.glob(name + '.*')]})
        print(name + ': ' + outcome + (' ' + detail if detail else ''), flush=True)

    def exact(path, kind, expected):
        def assertion(samples, _):
            assert samples, 'no typed samples'
            value = member(samples[0]['value'], *path)
            assert value['kind'] == kind and value['value'] == expected, value
        return assertion

    topic_prefix = '/studio_fixture/round2/'
    run('graph-only', [], lambda samples, events: (require(not samples, 'discovery emitted samples'),
        require(any(any(t[0] == topic_prefix + 'odometry' for t in e['topics']) for e in events if e['event'] == 'graph'), 'odometry not discovered')))
    run('uint64-exact', [topic_prefix + 'uint64', 'std_msgs/msg/UInt64'], exact(['data'], 'unsigned', '18446744073709551615'))
    run('int64-exact', [topic_prefix + 'int64', 'std_msgs/msg/Int64'], exact(['data'], 'signed', '-9223372036854775808'))
    run('odometry-nested', [topic_prefix + 'odometry', 'nav_msgs/msg/Odometry'], lambda samples, _: (
        require(len(samples) > 1, 'need changing data'),
        require(any(member(s['value'], 'pose', 'pose', 'position', 'x') != member(samples[0]['value'], 'pose', 'pose', 'position', 'x') for s in samples[1:]), 'odometry x did not change'),
        require(member(samples[0]['value'], 'pose', 'pose', 'position', 'x')['kind'] == 'float', 'x not typed float'),
        require(member(samples[0]['value'], 'pose', 'covariance')['original_items'] == 36, 'array length lost'),
        require(member(samples[0]['value'], 'header', 'frame_id')['value'] == 'odom', 'frame lost')))
    run('nonfinite', [topic_prefix + 'float64', 'std_msgs/msg/Float64'], lambda samples, _: require(
        {'finite', 'nan', 'positive_infinity', 'negative_infinity'}.issubset({member(s['value'], 'data')['value']['kind'] for s in samples}), 'missing numeric cases'))
    run('bounded-unicode', [topic_prefix + 'large_text', 'std_msgs/msg/String'], lambda samples, _: (
        require(bool(samples), 'no string data'),
        require(member(samples[0]['value'], 'data')['original_bytes'] == len(('café 🤖\n' * 5000).encode()), 'original bytes lost'),
        require(len(member(samples[0]['value'], 'data')['value'].encode()) <= 32768, 'text cap exceeded'),
        require(member(samples[0]['value'], 'data')['value'].startswith('café 🤖\n'), 'Unicode altered')))
    run('qos-mismatch', [topic_prefix + 'best_effort', 'std_msgs/msg/Float64'], lambda samples, _: require(not samples, 'reliable unexpectedly received best effort'))
    run('qos-corrected', [topic_prefix + 'best_effort', 'std_msgs/msg/Float64', '--best-effort'], exact(['data'], 'float', {'kind': 'finite', 'value': '12.5'}))
    run('retained-late-join', [topic_prefix + 'retained', 'std_msgs/msg/String', '--transient-local'], lambda samples, _: (
        require(bool(samples), 'retained sample absent'),
        require(member(samples[0]['value'], 'data')['value'] == 'Round2-retained-once café 🤖', 'retained value changed')))
    run('restart-discovery-no-replay', [], lambda samples, _: require(not samples, 'new discovery backend replayed subscription'))
    if args.interface_bundle:
        run('imported-custom', [topic_prefix + 'custom', 'feasibility_interfaces/msg/Sample', '--interface-bundle', str(args.interface_bundle.resolve())], lambda samples, _: (
            exact(['sequence'], 'unsigned', '18446744073709551615')(samples, []),
            require(member(samples[0]['value'], 'nested', 'label')['value'] == 'independently imported café 🤖', 'nested custom lost'),
            require(member(samples[0]['value'], 'fixed')['original_items'] == 3, 'fixed array lost'),
            require(member(samples[0]['value'], 'bounded')['original_items'] == 2, 'bounded array lost')))
    else:
        rows.append({'test': 'imported-custom', 'outcome': 'not tested', 'detail': 'No --interface-bundle supplied',
                     'os': platform.platform(), 'cpu': platform.machine(), 'distro': 'jazzy', 'middleware_pair': [os.environ['RMW_IMPLEMENTATION']] * 2,
                     'versions': versions, 'command': shlex.join(os.sys.argv), 'evidence': []})
    report = ROOT / 'artifacts/feasibility' / (args.label + '.json')
    if report.exists():
        raise SystemExit('report label exists; evidence preserved')
    report.write_text(json.dumps(rows, indent=2) + '\n')
    return int(any(row['outcome'] == 'failed' for row in rows))


def require(condition, detail):
    if not condition:
        raise AssertionError(detail)


if __name__ == '__main__':
    raise SystemExit(main())
