"""Bounded v7 Image pressure/control acceptance on guarded localhost ROS only.

One maximum-size Image stream and one 5Hz typed peer share a real backend. A
short acknowledged stdout pause induces application queue pressure. Receiver
mailbox replacements, backend drops and publisher calls remain separate counts.
No DDS loss, sustained latency, native rendering or RSS result is inferred.
"""
import argparse
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
spec = importlib.util.spec_from_file_location('image_acceptance', ROOT / 'scripts/test-image-session.py')
NORMAL = importlib.util.module_from_spec(spec)
spec.loader.exec_module(NORMAL)
WORKFLOW = NORMAL.WORKFLOW
MAX_LINE = 1024 * 1024
CONTROL_ITEMS = 16
CONTROL_BYTES = 4096
RAW_CAPTURE_BYTES = 1024 * 1024
CONTROL_CAPTURE_BYTES = 256 * 1024


class Reader:
    """Four latest sample slots + latest graph + reserved16 control FIFO.

    One bounded frame may be in flight. Control pressure blocks stdout reading;
    samples replace only the same identity's pending sample. Test coalescing
    never changes the backend's reported drop count. Pausing interrupts a full
    control FIFO retry; an acknowledged pause starts no further stdout reads.
    """
    def __init__(self, stream, folder):
        self.stream = stream
        self.control = queue.Queue(maxsize=CONTROL_ITEMS)
        self.samples = {}
        self.graph = None
        self.lock = threading.Lock()
        self.coalesced = 0
        self.sequences = {}
        self.drops = {}
        self.frames = 0
        self.capture_bytes = 0
        self.control_bytes = 0
        self.omitted = 0
        self.captured_ids = set()
        self.pause_requested = threading.Event()
        self.paused = threading.Event()
        self.finished = threading.Event()
        self.error = None
        self.capture = (folder / 'backend-selected.jsonl').open('wb')
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def gate(self):
        while self.pause_requested.is_set():
            self.paused.set()
            time.sleep(.005)
        self.paused.clear()

    def accept(self, event):
        kind = event.get('event')
        if kind in ('observation', 'image_observation', 'drops'):
            sub = event['subscription']
            assert type(sub) is int and 1 <= sub <= 4, 'Unbounded test identity'
            assert event['generation'] == 81, 'Wrong pressure session generation'
            count = event['backend_drops']
            assert type(count) is int and count >= self.drops.get(sub, 0), 'Backend drop count regressed'
            self.drops[sub] = count
            if kind != 'drops':
                assert event['sequence'] > self.sequences.get(sub, 0), 'Sequence regressed'
                self.sequences[sub] = event['sequence']
                with self.lock:
                    self.coalesced += int(sub in self.samples)
                    self.samples[sub] = event
                return
        if kind == 'graph':
            with self.lock:
                self.graph = event
            return
        while True:
            self.gate()
            try:
                self.control.put(event, timeout=.02)
                return
            except queue.Full:
                pass

    def run(self):
        try:
            while True:
                self.gate()
                line = self.stream.readline(MAX_LINE + 1)
                if not line:
                    return
                assert len(line) <= MAX_LINE and line.endswith(b'\n'), 'Invalid bounded pressure frame'
                event = json.loads(line)
                sample = event.get('event') in ('observation', 'image_observation')
                if sample:
                    assert len(line) <= 524288, 'Oversized v7 sample wire frame'
                retain = (self.capture_bytes + len(line) <= RAW_CAPTURE_BYTES and
                          event.get('subscription') not in self.captured_ids) if sample else (
                          self.control_bytes + len(line) <= CONTROL_CAPTURE_BYTES)
                if retain:
                    self.capture.write(line)
                    self.capture.flush()
                    if sample:
                        self.capture_bytes += len(line)
                        self.captured_ids.add(event['subscription'])
                    else:
                        self.control_bytes += len(line)
                else:
                    self.omitted += 1
                self.frames += 1
                if not sample and event.get('event') != 'graph':
                    assert len(line) <= CONTROL_BYTES, 'Oversized reserved control frame'
                self.accept(event)
        except Exception as error:
            self.error = repr(error)
        finally:
            self.capture.close()
            self.finished.set()

    def take(self):
        try:
            return self.control.get_nowait()
        except queue.Empty:
            with self.lock:
                if self.graph is not None:
                    event, self.graph = self.graph, None
                    return event
                if self.samples:
                    # Insertion order plus pop/reinsert on new frames cannot
                    # starve the typed peer during the bounded wait polling.
                    return self.samples.pop(next(iter(self.samples)))
        return None

    def wait(self, process, predicate, seconds=8):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if self.error:
                raise AssertionError(self.error)
            event = self.take()
            if event is None:
                if self.finished.is_set() or process.poll() is not None:
                    raise AssertionError(f'Pressure backend unexpectedly closed: {process.poll()}')
                time.sleep(.005)
                continue
            assert event.get('event') not in ('error', 'stream_error', 'request_error'), event
            if predicate(event):
                return event
        raise AssertionError('Pressure event exceeded bounded8s control/sample deadline')

    def pause(self):
        self.pause_requested.set()
        assert self.paused.wait(timeout=2), 'Reader did not acknowledge stdout pause'

    def resume(self):
        self.pause_requested.clear()

    def drain(self):
        for _ in range(CONTROL_ITEMS + 5):
            if self.take() is None:
                break

    def join_after_exit(self):
        self.resume()
        deadline = time.monotonic() + 3
        while self.thread.is_alive() and time.monotonic() < deadline:
            self.drain()
            self.thread.join(timeout=.02)
        assert not self.thread.is_alive() and not self.error, self.error


def argument_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', required=True)
    parser.add_argument('--stall-seconds', type=WORKFLOW.HELPERS['bounded_number'](.5, 2, float), default=1.2)
    parser.add_argument('--image-hz', type=WORKFLOW.HELPERS['bounded_number'](10, 30, float), default=30)
    return parser


def main():
    args = argument_parser().parse_args()
    if not args.label.startswith(('round4-', 'round5-')) or not args.label.replace('-', '').isalnum():
        raise SystemExit('Use an unused descriptive round4-/round5- label')
    folder = WORKFLOW.HELPERS['checked_environment']()
    report = ROOT / 'artifacts/feasibility' / f'{args.label}.json'
    if report.exists() or any(folder.iterdir()):
        raise SystemExit('Choose unused evidence/report paths; preserve earlier runs')
    WORKFLOW.HELPERS['inventory']()
    context = json.loads((folder / 'context.json').read_text())
    context['middleware_pair'] = f"{os.environ['RMW_IMPLEMENTATION']}/{os.environ['RMW_IMPLEMENTATION']}"
    context['source_sha256'] = {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in [
        'scripts/test-image-session-pressure.py', 'scripts/ui-image-workflow.py',
        'scripts/test-image-session.py', 'crates/backend/src/observation.rs', 'crates/protocol/src/session.rs']}
    rows = []
    def record(case, outcome='passed', details=None):
        rows.append({**context, 'case': case, 'outcome': outcome, 'details': details,
            'command': 'scripts/dev .pixi/envs/default/bin/python ' + shlex.join(sys.argv),
            'environment': {key: os.environ.get(key) for key in ['ROS_DOMAIN_ID', 'ROS_LOCALHOST_ONLY', 'ROS_AUTOMATIC_DISCOVERY_RANGE', 'RMW_IMPLEMENTATION']},
            'local_only': True, 'evidence': str(folder.relative_to(ROOT)),
            'limits': 'Bounded local Image stdout-stall test; receiver coalescing, backend queue drops and publisher calls are distinct. No DDS loss, sustained latency/RSS, native rendering or LAN proof.'})
        report.write_text(json.dumps(rows, indent=2) + '\n')
        print(outcome, case, details if details is not None else '', flush=True)
    backend = publisher = reader = None
    owned = []
    cleanup_errors = []
    publisher_log = (folder / 'publisher.stderr').open('wb')
    backend_log = (folder / 'backend.stderr').open('wb')
    request = 0
    try:
        publisher_args = [sys.executable, str(ROOT / 'scripts/ui-image-workflow.py'), '--publisher', '--seconds', '60', '--hz', '1', '--max-hz', str(args.image_hz)]
        publisher = subprocess.Popen(publisher_args, stdout=publisher_log, stderr=publisher_log)
        backend_args = [str(ROOT / 'target/debug/studio-backend'), '--observe-session']
        backend = subprocess.Popen(backend_args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=backend_log)
        owned = [{'pid': publisher.pid, 'argv': publisher_args}, {'pid': backend.pid, 'argv': backend_args}]
        (folder / 'owned-processes.json').write_text(json.dumps(owned, indent=2) + '\n')
        reader = Reader(backend.stdout, folder)
        def wait(predicate, seconds=8):
            return reader.wait(backend, predicate, seconds)
        def send(kind, sub, suffix=None, output='compact_image'):
            nonlocal request
            request += 1
            command = {'command': kind, 'generation': 81, 'request': request, 'subscription': sub}
            if suffix is not None:
                command['configuration'] = {'topic': WORKFLOW.PREFIX + suffix, 'message_type': 'std_msgs/msg/String' if output == 'typed' else 'sensor_msgs/msg/Image',
                    'output': output, 'best_effort': False, 'transient_local': False}
            frame = json.dumps(command).encode() + b'\n'
            assert len(frame) <= CONTROL_BYTES
            with (folder / 'commands.jsonl').open('ab') as log:
                log.write(frame)
            started_at = time.monotonic()
            backend.stdin.write(frame)
            backend.stdin.flush()
            return request, started_at
        def start(sub, suffix, output='compact_image'):
            req, started_at = send('start', sub, suffix, output)
            wait(lambda e: e.get('event') == 'started' and e['subscription'] == sub and e['request'] == req)
            return time.monotonic() - started_at
        def sample(sub):
            return wait(lambda e: e.get('event') in ('observation', 'image_observation') and e['subscription'] == sub)
        def stop(sub):
            before = reader.drops.get(sub, 0)
            req, started_at = send('stop', sub)
            event = wait(lambda e: e.get('subscription') == sub and e.get('event') in ('drops', 'stopped'))
            assert event['event'] == 'drops' and event['backend_drops'] >= before
            final = event['backend_drops']
            ack = wait(lambda e: e.get('event') == 'stopped' and e['subscription'] == sub)
            assert ack['request'] == req and ack['generation'] == 81
            return {'subscription': sub, 'previous_backend_drops': before, 'final_backend_drops': final,
                    'request': req, 'control_ack_seconds': time.monotonic() - started_at}
        ready = wait(lambda e: e.get('event') == 'ready')
        assert ready['version'] == 7 and ready['max_image_raw_bytes'] == 262144
        expected = {'max_command_bytes': 131072, 'robot_contract_version': 1,
                    'max_robot_links': 128, 'max_robot_hz': 10, 'max_robot_sample_bytes': 524288}
        assert all(ready.get(key) == value for key, value in expected.items()), ready
        wait(lambda e: e.get('event') == 'graph' and all(any(t[0] == WORKFLOW.PREFIX + name for t in e['topics']) for name in ['max_rgba', 'status']))
        start(1, 'max_rgba')
        NORMAL.validate_compact(sample(1), 'max_rgba')
        status_ack = start(2, 'status', 'typed')
        low_before = sample(2)['sequence']
        record('maximum Image and low-rate typed peer receive', details={'image_raw_bytes': 262144,
            'image_hz_requested': args.image_hz, 'typed_hz_requested': 5, 'status_start_ack_seconds': status_ack,
            'reader_samples_max': 4, 'reader_control_max': CONTROL_ITEMS, 'max_line_bytes': MAX_LINE})
        # Let the receiver consume transport but defer its consumer to exercise
        # latest-frame replacement independently of application transport loss.
        time.sleep(.35)
        assert reader.coalesced > 0
        NORMAL.validate_compact(sample(1), 'max_rgba')
        record('receiver latest-frame coalescing remains bounded', details={'coalesced': reader.coalesced,
            'sample_slots': len(reader.samples), 'backend_drops': dict(reader.drops)})
        reader.pause()
        low_before = reader.sequences.get(2, low_before)
        paused_at = time.monotonic()
        time.sleep(args.stall_seconds)
        assert backend.poll() is None and publisher.poll() is None
        reader.resume()
        event = wait(lambda e: e.get('event') == 'image_observation' and e['backend_drops'] > 0)
        NORMAL.validate_compact(event, 'max_rgba')
        low_recovered = wait(lambda e: e.get('event') == 'observation' and e['subscription'] == 2 and e['sequence'] > low_before)
        record('stdout pressure yields explicit backend drops and healthy peer recovery', details={
            'stall_seconds_requested': args.stall_seconds, 'pause_to_peer_seconds': time.monotonic() - paused_at,
            'image_backend_drops': event['backend_drops'], 'low_sequence': low_recovered['sequence'], 'receiver_coalesced': reader.coalesced})
        # A new independently owned typed stream is correlated while the large
        # Image remains live, then stopped before the pressure stream itself.
        elapsed = start(3, 'status', 'typed')
        sample(3)
        assert backend.poll() is None
        record('correlated Start acknowledgment under declared live Image traffic', details={'subscription': 3, 'request': request,
            'control_ack_seconds': elapsed, 'deadline_seconds': 8, 'backend_pid': backend.pid})
        sibling_stop = stop(3)
        record('correlated peer Stop preserves Image and original typed owner', details=sibling_stop)
        NORMAL.validate_compact(sample(1), 'max_rgba')
        low_before_stop = sample(2)['sequence']
        image_stop = stop(1)
        assert image_stop['final_backend_drops'] > 0
        record('final Image Drops precedes correlated Stop acknowledgment', details=image_stop)
        assert sample(2)['sequence'] > low_before_stop and backend.poll() is None
        record('stopping pressure Image preserves low-rate progress and backend PID', details={'backend_pid': backend.pid})
        record('low-rate final Drops precedes correlated Stop acknowledgment', details=stop(2))
        backend.stdin.close()
        backend.wait(timeout=5)
        assert backend.returncode == 0
        reader.join_after_exit()
        record('EOF and bounded reader terminate', details={'frames_read': reader.frames, 'receiver_coalesced': reader.coalesced,
            'raw_capture_bytes': reader.capture_bytes, 'control_capture_bytes': reader.control_bytes, 'omitted_frames': reader.omitted})
    except Exception as error:
        (folder / 'failure.txt').write_text(traceback.format_exc())
        record('Image pressure acceptance interrupted', 'failed', {'error': repr(error)})
    finally:
        if reader is not None:
            reader.resume()
        if backend is not None:
            if not backend.stdin.closed:
                backend.stdin.close()
            deadline = time.monotonic() + 5
            while backend.poll() is None and time.monotonic() < deadline:
                if reader is not None:
                    reader.drain()
                time.sleep(.02)
            if backend.poll() is None:
                cleanup_errors.append('backend failed graceful EOF')
                backend.kill()
            backend.wait(timeout=3)
            if reader is not None:
                try:
                    reader.join_after_exit()
                except Exception as error:
                    cleanup_errors.append(repr(error))
            backend.stdout.close()
            if backend.returncode != 0:
                cleanup_errors.append(f'backend exit{backend.returncode}')
        if publisher is not None:
            if publisher.poll() is None:
                publisher.send_signal(signal.SIGINT)
            try:
                publisher.wait(timeout=5)
            except subprocess.TimeoutExpired:
                cleanup_errors.append('publisher failed graceful SIGINT')
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
        publisher_log.close()
        backend_log.close()
        record('all owned pressure publisher/backend processes reaped', 'failed' if cleanup_errors else 'passed', {'owned': owned, 'errors': cleanup_errors})
    if any(row['outcome'] != 'passed' for row in rows):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
