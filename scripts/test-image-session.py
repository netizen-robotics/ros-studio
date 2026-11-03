"""Guarded backend-only session7 Image acceptance. No native window or robot.

Run serial matching RMW pairs with project Python and the existing localhost /
domain187 guard. Queues, selected raw captures, deadlines and owned lifetimes
are bounded. A source publication count is never interpreted as DDS delivery.
"""
import argparse
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import queue
import shlex
import signal
import subprocess
import sys
import threading
import time
import traceback

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('studio_image_workflow', ROOT / 'scripts/ui-image-workflow.py')
WORKFLOW = importlib.util.module_from_spec(spec)
spec.loader.exec_module(WORKFLOW)
MAX_LINE = 1024 * 1024
CAPTURE_BYTES = 8 * 1024 * 1024


class Backend:
    def __init__(self, folder, index, generation):
        self.generation = generation
        self.request = 0
        self.events = queue.Queue(maxsize=16)
        self.error = None
        self.finished = threading.Event()
        self.captured = 0
        self.omitted = 0
        self.seen = {}  # at most32 explicit test IDs, two captured samples each
        self.sequence = {}
        self.closed = False
        self.commands = (folder / f'backend-{index}-commands.jsonl').open('ab')
        self.capture = (folder / f'backend-{index}-selected.jsonl').open('wb')
        self.stderr = (folder / f'backend-{index}.stderr').open('wb')
        self.process = subprocess.Popen([str(ROOT / 'target/debug/studio-backend'), '--observe-session'],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr)
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        try:
            while True:
                line = self.process.stdout.readline(MAX_LINE + 1)
                if not line:
                    return
                if len(line) > MAX_LINE or not line.endswith(b'\n'):
                    raise AssertionError('Invalid bounded Image event frame')
                event = json.loads(line)
                sample = event.get('event') in ('observation', 'image_observation')
                sub = event.get('subscription', 0)
                if sample:
                    assert type(sub) is int and 1 <= sub <= 32
                retain = not sample or self.seen.get(sub, 0) < 2
                if retain and self.captured + len(line) <= CAPTURE_BYTES:
                    self.capture.write(line)
                    self.capture.flush()
                    self.captured += len(line)
                    if sample:
                        self.seen[sub] = self.seen.get(sub, 0) + 1
                else:
                    self.omitted += 1
                self.events.put(event, timeout=3)
        except Exception as error:
            self.error = repr(error)
        finally:
            self.capture.close()
            self.finished.set()

    def wait(self, predicate, seconds=12):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if self.error:
                raise AssertionError(self.error)
            try:
                event = self.events.get(timeout=min(.1, max(.001, deadline - time.monotonic())))
            except queue.Empty:
                if self.finished.is_set() or self.process.poll() is not None:
                    raise AssertionError(f'Image backend closed unexpectedly: {self.process.poll()}')
                continue
            if event.get('event') == 'error':
                raise AssertionError(event)
            if event.get('event') in ('observation', 'image_observation'):
                sub = event['subscription']
                assert event['generation'] == self.generation
                assert event['sequence'] > self.sequence.get(sub, 0), 'nonincreasing identified sequence'
                self.sequence[sub] = event['sequence']
            if predicate(event):
                return event
        raise AssertionError('Timed out waiting for Image acceptance event')

    def send(self, command):
        frame = json.dumps(command).encode() + b'\n'
        assert len(frame) <= 4096
        self.commands.write(frame)
        self.commands.flush()
        self.process.stdin.write(frame)
        self.process.stdin.flush()

    def start(self, sub, suffix, output='compact_image', retained=False, message_type='sensor_msgs/msg/Image', ack=True):
        self.request += 1
        self.send({'command': 'start', 'generation': self.generation, 'request': self.request, 'subscription': sub,
                   'configuration': {'topic': WORKFLOW.PREFIX + suffix, 'message_type': message_type,
                       'best_effort': False, 'transient_local': retained, 'output': output}})
        if ack:
            self.wait(lambda e: e.get('event') == 'started' and e['subscription'] == sub and e['request'] == self.request)
        return self.request

    def stop(self, sub):
        self.request += 1
        self.send({'command': 'stop', 'generation': self.generation, 'request': self.request, 'subscription': sub})
        first = self.wait(lambda e: e.get('subscription') == sub and e.get('event') in ('drops', 'stopped'))
        assert first['event'] == 'drops', 'Final Image loss snapshot must precede cleanup acknowledgment'
        assert first['generation'] == self.generation and type(first['backend_drops']) is int
        self.wait(lambda e: e.get('event') == 'stopped' and e['subscription'] == sub and e['request'] == self.request)
        return first['backend_drops']

    def sample(self, sub, output='image_observation', seconds=12):
        event = self.wait(lambda e: e.get('subscription') == sub and e.get('event') in (output, 'stream_error', 'request_error'), seconds)
        assert event['event'] == output, event
        return event

    def close(self):
        if self.closed:
            return
        if not self.process.stdin.closed:
            self.process.stdin.close()
        deadline = time.monotonic() + 5
        while self.process.poll() is None and time.monotonic() < deadline:
            for _ in range(16):
                try:
                    self.events.get_nowait()
                except queue.Empty:
                    break
            time.sleep(.02)
        forced = self.process.poll() is None
        if forced:
            self.process.kill()
        self.process.wait(timeout=3)
        deadline = time.monotonic() + 3
        while self.reader.is_alive() and time.monotonic() < deadline:
            for _ in range(16):
                try:
                    self.events.get_nowait()
                except queue.Empty:
                    break
            self.reader.join(timeout=.02)
        self.process.stdout.close()
        self.stderr.close()
        self.commands.close()
        self.closed = True
        assert not forced, 'Image backend failed graceful EOF cleanup'
        assert self.process.returncode == 0 and not self.reader.is_alive(), (self.process.returncode, self.error)
        assert self.error is None, self.error


def validate_compact(event, suffix):
    sample = event['sample']
    metadata = sample['metadata']
    assert sample['contract_version'] == 1 and sample['message_type'] == 'sensor_msgs/msg/Image'
    assert sample['topic'] == WORKFLOW.PREFIX + suffix and sample['complete'] is True
    raw = base64.b64decode(sample['data_base64'], validate=True)
    assert len(raw) == sample['original_bytes'] == metadata['step'] * metadata['height'] <= 262144
    assert metadata['header_stamp']['seconds'] > 0 and 0 <= metadata['header_stamp']['nanoseconds'] < 1000000000
    count = WORKFLOW.frame_number(metadata, raw)
    expected = WORKFLOW.camera(suffix, count)
    for key in ('width', 'height', 'encoding', 'step', 'frame_id'):
        assert metadata[key] == getattr(expected, key), (key, metadata[key], getattr(expected, key))
    assert metadata['is_bigendian'] == bool(expected.is_bigendian)
    assert raw == expected.data, 'Native Image bytes differ from deterministic colors/alpha/padding/orientation'
    return count, metadata


def member(value, name):
    assert value['kind'] == 'object'
    return next(field['value'] for field in value['fields'] if field['name'] == name)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', required=True)
    args = parser.parse_args()
    if not args.label.startswith(('round4-', 'round5-')) or not args.label.replace('-', '').isalnum():
        parser.error('Use a new descriptive round4-/round5- label')
    folder = WORKFLOW.HELPERS['checked_environment']()
    report = ROOT / 'artifacts/feasibility' / f'{args.label}.json'
    if report.exists() or any(folder.iterdir()):
        raise SystemExit('Choose unused paths; preserve all earlier outcomes')
    WORKFLOW.HELPERS['inventory']()
    context = json.loads((folder / 'context.json').read_text())
    context['source_sha256'] = {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in [
        'scripts/ui-image-workflow.py', 'scripts/test-image-session.py', 'fixtures/simple_sim/model.py',
        'crates/protocol/src/session.rs', 'crates/backend/src/observation.rs']}
    context['middleware_pair'] = f"{os.environ['RMW_IMPLEMENTATION']}/{os.environ['RMW_IMPLEMENTATION']}"
    rows = []
    def record(case, outcome='passed', details=None):
        rows.append({**context, 'case': case, 'outcome': outcome, 'details': details,
                     'command': 'scripts/dev .pixi/envs/default/bin/python ' + shlex.join(sys.argv),
                     'environment': {key: os.environ.get(key) for key in ['ROS_DOMAIN_ID', 'ROS_LOCALHOST_ONLY', 'ROS_AUTOMATIC_DISCOVERY_RANGE', 'RMW_IMPLEMENTATION']},
                     'evidence': str(folder.relative_to(ROOT)), 'local_only': True,
                     'limits': 'Backend-only local publishers; no native Image rendering, DDS loss or sustained delivered rate/performance result inferred'})
        report.write_text(json.dumps(rows, indent=2) + '\n')
        print(outcome, case, details if details else '', flush=True)
    publisher = None
    backends = []
    owned = []
    cleanup_errors = []
    log = (folder / 'publisher.stderr').open('wb')
    try:
        publisher_args = [sys.executable, str(ROOT / 'scripts/ui-image-workflow.py'), '--publisher', '--seconds', '90', '--hz', '10', '--max-hz', '2']
        publisher = subprocess.Popen(publisher_args, stdout=log, stderr=log)
        owned.append({'pid': publisher.pid, 'argv': publisher_args})
        backend = Backend(folder, 1, 71)
        backends.append(backend)
        owned.append({'pid': backend.process.pid, 'argv': [str(ROOT / 'target/debug/studio-backend'), '--observe-session']})
        (folder / 'owned-processes.json').write_text(json.dumps(owned, indent=2) + '\n')
        ready = backend.wait(lambda e: e.get('event') == 'ready')
        assert ready['version'] == 7 and ready['image_contract_version'] == 1
        expected = {'max_command_bytes': 131072, 'robot_contract_version': 1,
                    'max_robot_links': 128, 'max_robot_hz': 10, 'max_robot_sample_bytes': 524288}
        assert all(ready.get(key) == value for key, value in expected.items()), ready
        assert ready['max_image_raw_bytes'] == 262144 and ready['max_image_sample_bytes'] == 524288
        backend.wait(lambda e: e.get('event') == 'graph' and all(any(t[0] == WORKFLOW.PREFIX + name for t in e['topics']) for name in WORKFLOW.TOPICS))
        backend.start(1, 'status', 'typed', message_type='std_msgs/msg/String')
        backend.sample(1, 'observation')
        pid = backend.process.pid
        record('session7 exact Image negotiation and discovery-only start', details={'backend_pid': pid, 'publisher_pid': publisher.pid})
        for sub, suffix in [(2, 'rgb8'), (3, 'mono8'), (4, 'bgr8')]:
            backend.start(sub, suffix)
            validate_compact(backend.sample(sub), suffix)
        req = backend.start(5, 'rgba8', ack=False)
        rejection = backend.wait(lambda e: e.get('event') == 'request_error' and e['request'] == req)
        assert rejection['no_handle'] is True
        before = backend.sequence[1]
        for sub in [2, 3, 4]:
            backend.stop(sub)
        assert backend.sample(1, 'observation')['sequence'] > before and backend.process.pid == pid
        record('three compact images plus typed peer; fifth rejected; independent Stops preserve peer/PID')
        for sub, suffix in [(6, 'rgba8'), (7, 'bgra8')]:
            backend.start(sub, suffix)
            validate_compact(backend.sample(sub), suffix)
            backend.stop(sub)
        record('all five encodings preserve exact raw colors/padding/orientation/alpha and metadata')
        backend.start(8, 'rgb8', 'typed')
        typed = backend.sample(8, 'observation')['sample']
        assert typed['contract_version'] == 2 and typed['message_type'] == 'sensor_msgs/msg/Image'
        assert int(member(typed['value'], 'width')['value']) == 64
        assert int(member(typed['value'], 'height')['value']) == 48
        data = member(typed['value'], 'data')
        assert data['kind'] == 'array' and data['original_items'] > len(data['items'])
        backend.stop(8)
        record('Image selected as Messages retains typed2 metadata and explicit truncated byte array')
        backend.start(9, 'unsupported')
        validate_compact(backend.sample(9), 'unsupported')
        validate_compact(backend.sample(9), 'unsupported')
        backend.stop(9)
        record('unsupported16UC1 remains complete compact metadata without false native cleanup')
        backend.start(10, 'malformed')
        snapshot = backend.wait(lambda e: e.get('subscription') == 10 and e.get('event') in ('drops', 'stream_error'))
        assert snapshot['event'] == 'drops' and snapshot['backend_drops'] == 0
        failure = backend.wait(lambda e: e.get('event') == 'stream_error' and e['subscription'] == 10)
        assert 0 < len(failure['message'].encode()) <= 512
        backend.sample(1, 'observation')
        record('malformed Image native failure is scoped after finalDrops and peer remains receiving', details={'message': failure['message']})
        backend.start(11, 'retained', retained=True)
        retained = backend.sample(11)
        count, metadata = validate_compact(retained, 'retained')
        assert count == 0
        pub_rows = [json.loads(line) for line in (folder / 'image-publisher.jsonl').read_text().splitlines()]
        published = next(row for row in pub_rows if row['event'] == 'retained-once')
        assert metadata['header_stamp'] == published['stamp']
        backend.stop(11)
        record('ReliableTransientLocal late join receives once-published Image with exact header stamp')
        backend.start(12, 'max_rgba')
        maximum = backend.sample(12, seconds=15)
        validate_compact(maximum, 'max_rgba')
        assert maximum['sample']['original_bytes'] == 262144
        backend.stop(12)
        record('maximum256x256RGBA262144raw bytes preserved exactly through actual Rust backend')
        backend.start(13, 'format_changes')
        formats = set()
        for _ in range(10):
            _, metadata = validate_compact(backend.sample(13), 'format_changes')
            formats.add((metadata['encoding'], metadata['width'], metadata['height']))
        assert len({kind[0] for kind in formats}) == 5
        assert len({kind[1:] for kind in formats}) == 2
        backend.sample(1, 'observation')
        record('live format/resolution changes retain exact native bytes and peer progress', details={'formats': sorted(formats)})
        backend.close()  # status + format_changes are still active.
        record('EOF cleans two active native owners and bounded reader exits')
        backend = Backend(folder, 2, 72)
        backends.append(backend)
        owned.append({'pid': backend.process.pid, 'argv': [str(ROOT / 'target/debug/studio-backend'), '--observe-session']})
        (folder / 'owned-processes.json').write_text(json.dumps(owned, indent=2) + '\n')
        backend.wait(lambda e: e.get('event') == 'ready' and e['version'] == 7)
        until = time.monotonic() + 2.2
        graphs = 0
        while time.monotonic() < until:
            event = backend.wait(lambda e: True, seconds=3)
            assert event['event'] == 'graph', 'New session activated without explicit Start'
            graphs += 1
        assert graphs >= 2
        backend.start(1, 'status', 'typed', message_type='std_msgs/msg/String')
        backend.sample(1, 'observation')
        backend.close()
        record('new backend discovers without replay and only explicit Start activates')
    except Exception as error:
        (folder / 'failure.txt').write_text(traceback.format_exc())
        record('Image backend acceptance interrupted', 'failed', {'error': repr(error)})
    finally:
        for backend in backends:
            try:
                backend.close()
            except Exception as error:
                cleanup_errors.append(repr(error))
        if publisher is not None:
            if publisher.poll() is None:
                publisher.send_signal(signal.SIGINT)
            try:
                publisher.wait(timeout=5)
            except subprocess.TimeoutExpired:
                cleanup_errors.append('Image publisher did not exit after SIGINT')
                publisher.kill()
                publisher.wait(timeout=3)
            if publisher.returncode != 0:
                cleanup_errors.append(f'publisher exit{publisher.returncode}')
        for process in owned:
            try:
                os.kill(process['pid'], 0)
            except ProcessLookupError:
                pass
            else:
                cleanup_errors.append(f'owned PID remains:{process["pid"]}')
        log.close()
        record('all owned Image publisher/backend processes reaped', 'failed' if cleanup_errors else 'passed', {'owned': owned, 'errors': cleanup_errors})
    if any(row['outcome'] != 'passed' for row in rows):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
