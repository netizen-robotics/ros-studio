"""Guarded local real-ROS stdout-stall / final-drop acceptance.

Use project Python through scripts/dev with localhost/domain187, explicit matching
RMW and an unused STUDIO_UI_ROS_EVIDENCE folder. No native window or robot access.
The test owns only its two Popen children and reaps them on every outcome.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import runpy
import shlex
import signal
import subprocess
import sys
import threading
import time
import traceback

ROOT = Path(__file__).resolve().parents[1]
HELPERS = runpy.run_path(str(ROOT / 'scripts/ui-ros-workflow.py'))
MAX_LINE = 1024 * 1024
READER_ITEMS = 32
CAPTURE_BYTES = 2 * 1024 * 1024
FLOOD_TOPIC = '/studio_fixture/ui/flood'
LOW_TOPIC = '/studio_fixture/ui/numeric'


class BoundedReader:
    """At most32 pending wire frames plus one in-flight frame, each<=1MiB.

    Pause acknowledges that no further stdout read will start until resume.
    A requested pause also interrupts a full test queue's bounded put retry.
    """
    def __init__(self, stream, folder):
        self.stream = stream
        self.events = queue.Queue(maxsize=READER_ITEMS)
        self.pause_requested = threading.Event()
        self.paused = threading.Event()
        self.finished = threading.Event()
        self.error = None
        self.frames = 0
        self.capture_bytes = 0
        self.omitted_frames = 0
        self.flood_captures = 0
        self.log = (folder / 'backend-events-selected.jsonl').open('wb')
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def gate(self):
        while self.pause_requested.is_set():
            self.paused.set()
            time.sleep(.005)
        self.paused.clear()

    def run(self):
        try:
            while True:
                self.gate()
                line = self.stream.readline(MAX_LINE + 1)
                if not line:
                    return
                if len(line) > MAX_LINE or not line.endswith(b'\n'):
                    raise AssertionError('Invalid bounded backend frame')
                event = json.loads(line)
                self.frames += 1
                flood = event.get('event') == 'observation' and event.get('subscription') == 1
                # Retain every control/low-rate event and the first8 flood samples.
                # The complete capture also has a2MiB hard bound.
                if self.capture_bytes + len(line) <= CAPTURE_BYTES and (not flood or self.flood_captures < 8):
                    self.log.write(line)
                    self.log.flush()
                    self.capture_bytes += len(line)
                    if flood:
                        self.flood_captures += 1
                else:
                    self.omitted_frames += 1
                while True:
                    self.gate()
                    try:
                        self.events.put(event, timeout=.02)
                        break
                    except queue.Full:
                        pass
        except Exception as error:
            self.error = repr(error)
        finally:
            self.log.close()
            self.finished.set()

    def pause(self):
        self.pause_requested.set()
        if not self.paused.wait(timeout=2):
            raise AssertionError('Reader did not acknowledge pause')

    def resume(self):
        self.pause_requested.clear()

    def wait(self, process, predicate, seconds=8):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if self.error:
                raise AssertionError(self.error)
            try:
                event = self.events.get(timeout=min(.1, max(.001, deadline - time.monotonic())))
            except queue.Empty:
                if self.finished.is_set() or process.poll() is not None:
                    raise AssertionError(f'Backend closed while waiting, status={process.poll()}')
                continue
            if event.get('event') == 'error':
                raise AssertionError(event)
            if predicate(event):
                return event
        raise AssertionError('Timed out waiting for bounded acceptance event')

    def drain(self):
        # A producer can still be running during exceptional cleanup. Bound
        # each drain pass even when new frames are concurrently enqueued.
        for _ in range(READER_ITEMS):
            try:
                self.events.get_nowait()
            except queue.Empty:
                break

    def join_after_exit(self):
        self.resume()
        deadline = time.monotonic() + 3
        while self.thread.is_alive() and time.monotonic() < deadline:
            self.drain()
            self.thread.join(timeout=.02)
        if self.thread.is_alive():
            raise AssertionError('Backend stdout reader did not terminate after process exit')
        if self.error:
            raise AssertionError(self.error)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', required=True)
    parser.add_argument('--stall-seconds', type=HELPERS['bounded_number'](.5, 2, float), default=1.2)
    parser.add_argument('--flood-rate', type=HELPERS['bounded_number'](1, 2000, float), default=2000)
    parser.add_argument('--flood-bytes', type=HELPERS['bounded_number'](1, 8192, int), default=8192)
    args = parser.parse_args()
    if not args.label.startswith(('round3-', 'round4-', 'round5-')) or not args.label.replace('-', '').isalnum():
        parser.error('Use an unused descriptive round3-/round4-/round5- label')
    folder = HELPERS['checked_environment']()
    report = ROOT / 'artifacts/feasibility' / f'{args.label}.json'
    if report.exists() or any(folder.iterdir()):
        raise SystemExit('Choose unused report and evidence paths; never overwrite a run')
    HELPERS['inventory']()
    context = json.loads((folder / 'context.json').read_text())
    context['source_sha256'] = {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in [
        'scripts/test-observe-session-flood.py', 'scripts/ui-ros-workflow.py',
        'crates/backend/src/observation.rs', 'crates/protocol/src/session.rs']}
    context['middleware_pair'] = f"{os.environ['RMW_IMPLEMENTATION']}/{os.environ['RMW_IMPLEMENTATION']}"
    command = 'scripts/dev .pixi/envs/default/bin/python ' + shlex.join(sys.argv)
    rows = []
    def record(case, outcome='passed', details=None):
        rows.append({**context, 'case': case, 'outcome': outcome, 'details': details,
                     'command': command, 'environment': {k: os.environ.get(k) for k in [
                         'ROS_DISTRO', 'ROS_DOMAIN_ID', 'ROS_LOCALHOST_ONLY', 'ROS_AUTOMATIC_DISCOVERY_RANGE', 'RMW_IMPLEMENTATION']},
                     'local_only': True, 'evidence': str(folder.relative_to(ROOT)),
                     'limits': 'Backend-only controlled stdout stall. Application queue drops only; no DDS loss, sustained RSS/latency or native UI proof.'})
        report.write_text(json.dumps(rows, indent=2) + '\n')
        print(outcome, case, details if details is not None else '', flush=True)

    publisher = backend = reader = None
    owned = []
    cleanup_errors = []
    backend_stderr = (folder / 'backend.stderr').open('wb')
    publisher_stderr = (folder / 'publisher.stderr').open('wb')
    request = 0
    generation = 61
    max_backend_drops = {1: 0, 2: 0}
    last_low_sequence = 0
    try:
        publisher_args = [sys.executable, str(ROOT / 'scripts/ui-ros-workflow.py'), '--publisher', '--numeric-session',
                          '--seconds', '45', '--flood-rate', str(args.flood_rate), '--flood-bytes', str(args.flood_bytes)]
        publisher = subprocess.Popen(publisher_args, stdout=publisher_stderr, stderr=publisher_stderr)
        owned.append(publisher.pid)
        backend_args = [str(ROOT / 'target/debug/studio-backend'), '--observe-session']
        backend = subprocess.Popen(backend_args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=backend_stderr)
        owned.append(backend.pid)
        (folder / 'owned-processes.json').write_text(json.dumps({'backend': {'pid': backend.pid, 'argv': backend_args},
            'publisher': {'pid': publisher.pid, 'argv': publisher_args}}, indent=2) + '\n')
        reader = BoundedReader(backend.stdout, folder)
        def wait(predicate, seconds=8):
            def observed(event):
                nonlocal last_low_sequence
                if event.get('event') in ('observation', 'drops'):
                    sub = event['subscription']
                    if sub in max_backend_drops:
                        count = event['backend_drops']
                        assert type(count) is int and count >= max_backend_drops[sub], ('drop counter regressed', event)
                        max_backend_drops[sub] = count
                    if event.get('event') == 'observation' and sub == 2:
                        assert event['sequence'] > last_low_sequence
                        last_low_sequence = event['sequence']
                return predicate(event)
            return reader.wait(backend, observed, seconds)
        def send(kind, sub, configuration=None):
            nonlocal request
            request += 1
            message = {'command': kind, 'generation': generation, 'request': request, 'subscription': sub}
            if configuration is not None:
                message['configuration'] = configuration
            frame = json.dumps(message).encode() + b'\n'
            assert len(frame) <= 4096
            backend.stdin.write(frame)
            backend.stdin.flush()
            with (folder / 'commands.jsonl').open('ab') as log:
                log.write(frame)
            return request
        ready = wait(lambda e: e.get('event') == 'ready')
        assert ready['version'] == 7 and ready['max_subscriptions'] == 4
        expected = {'max_command_bytes': 131072, 'robot_contract_version': 1,
                    'max_robot_links': 128, 'max_robot_hz': 10, 'max_robot_sample_bytes': 524288}
        assert all(ready.get(key) == value for key, value in expected.items()), ready
        wait(lambda e: e.get('event') == 'graph' and all(any(t[0] == topic for t in e['topics']) for topic in [FLOOD_TOPIC, LOW_TOPIC]))
        for sub, topic, ty in [(1, FLOOD_TOPIC, 'std_msgs/msg/String'), (2, LOW_TOPIC, 'std_msgs/msg/UInt64')]:
            req = send('start', sub, {'topic': topic, 'message_type': ty, 'best_effort': False, 'transient_local': False, 'output': 'typed'})
            wait(lambda e: e.get('event') == 'started' and e['subscription'] == sub and e['request'] == req)
            wait(lambda e: e.get('event') == 'observation' and e['subscription'] == sub)
        record('actual flood and low-rate subscriptions receive in one backend', details={'backend_pid': backend.pid, 'publisher_pid': publisher.pid,
               'requested_hz': args.flood_rate, 'flood_bytes': args.flood_bytes, 'reader_queue_items': READER_ITEMS, 'max_line_bytes': MAX_LINE})
        low_before_stall = last_low_sequence
        reader.pause()
        stalled_at = time.monotonic()
        time.sleep(args.stall_seconds)
        assert backend.poll() is None and publisher.poll() is None
        reader.resume()
        wait(lambda e: e.get('event') == 'observation' and e['subscription'] == 1 and e['backend_drops'] > 0)
        wait(lambda e: e.get('event') == 'observation' and e['subscription'] == 2 and e['sequence'] > low_before_stall)
        record('bounded stdout stall induces visible backend queue drops and low-rate recovers', details={
               'stall_seconds': args.stall_seconds, 'elapsed_until_low_recovery': time.monotonic() - stalled_at,
               'observed_backend_drops': max_backend_drops[1], 'low_sequence': last_low_sequence})
        low_before_stop = last_low_sequence
        for sub in [1, 2]:
            previously_reported = max_backend_drops[sub]
            stop_at = time.monotonic()
            req = send('stop', sub)
            terminal = wait(lambda e: e.get('subscription') == sub and e.get('event') in ('drops', 'stopped'))
            assert terminal['event'] == 'drops', 'Stopped preceded final Drops snapshot'
            assert terminal['generation'] == generation and terminal['backend_drops'] >= previously_reported
            if sub == 1:
                assert terminal['backend_drops'] > 0
            ack = wait(lambda e: e.get('subscription') == sub and e.get('event') == 'stopped')
            assert ack['request'] == req and backend.poll() is None
            record(f'final drop snapshot precedes stream{sub} stop acknowledgment', details={
                   'subscription': sub, 'last_observed_count': previously_reported, 'final_count': terminal['backend_drops'],
                   'control_seconds': time.monotonic() - stop_at})
            if sub == 1:
                wait(lambda e: e.get('event') == 'observation' and e['subscription'] == 2 and e['sequence'] > low_before_stop)
                record('stopping flood preserves low-rate progress and backend PID', details={'backend_pid': backend.pid, 'low_sequence': last_low_sequence})
        backend.stdin.close()
        backend.wait(timeout=5)
        assert backend.returncode == 0
        reader.join_after_exit()
        record('backend EOF exits cleanly and bounded reader terminates', details={'returncode': backend.returncode, 'frames_read': reader.frames,
               'selected_capture_bytes': reader.capture_bytes, 'omitted_flood_frames': reader.omitted_frames})
    except Exception as error:
        (folder / 'failure.txt').write_text(traceback.format_exc())
        record('controlled flood acceptance interrupted', 'failed', {'error': repr(error)})
    finally:
        if publisher is not None and publisher.poll() is None:
            publisher.send_signal(signal.SIGINT)
            try:
                publisher.wait(timeout=4)
            except subprocess.TimeoutExpired:
                cleanup_errors.append('publisher failed graceful SIGINT')
                publisher.kill()
                publisher.wait(timeout=3)
        if publisher is not None:
            publisher.wait(timeout=3)
            if publisher.returncode != 0:
                cleanup_errors.append(f'publisher exit status {publisher.returncode}')
        if reader is not None:
            reader.resume()
        if backend is not None:
            if not backend.stdin.closed:
                backend.stdin.close()
            until = time.monotonic() + 4
            while backend.poll() is None and time.monotonic() < until:
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
        for pid in owned:
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                pass
            else:
                cleanup_errors.append(f'owned PID still exists: {pid}')
        backend_stderr.close()
        publisher_stderr.close()
        record('all owned processes reaped', 'failed' if cleanup_errors else 'passed', {'pids': owned, 'errors': cleanup_errors,
               'publisher_returncode': publisher.returncode if publisher else None,
               'backend_returncode': backend.returncode if backend else None})
    if any(row['outcome'] != 'passed' for row in rows):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
