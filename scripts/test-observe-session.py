"""Local real-ROS acceptance for the persistent Rust observation backend.

Run separately for matching Fast/Fast and Cyclone/Cyclone through scripts/dev
and project Python. The existing isolation guard enforces localhost/domain 187.
This is backend evidence; it does not establish desktop/native-window behavior.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import queue
import runpy
import shlex
import subprocess
import sys
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
HELPERS = runpy.run_path(str(ROOT / 'scripts/ui-ros-workflow.py'))


class Backend:
    def __init__(self, folder, index, arguments=('--observe-session',)):
        self.log = (folder / f'backend-{index}.jsonl').open('wb')
        self.stderr = (folder / f'backend-{index}.stderr').open('wb')
        self.events = queue.Queue(maxsize=128)
        self.process = subprocess.Popen([str(ROOT / 'target/debug/studio-backend'), *arguments],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr)
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        try:
            for line in iter(lambda: self.process.stdout.readline(1024 * 1024 + 1), b''):
                self.log.write(line)
                self.log.flush()
                if len(line) > 1024 * 1024 or not line.endswith(b'\n'):
                    raise AssertionError('invalid bounded frame')
                event = json.loads(line)
                self.events.put(event, timeout=2)
        except Exception as error:
            self.events.put({'reader_error': repr(error)}, timeout=2)

    def send(self, command):
        self.process.stdin.write(json.dumps(command).encode() + b'\n')
        self.process.stdin.flush()

    def wait(self, predicate, seconds=12):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                event = self.events.get(timeout=min(.2, max(.001, deadline - time.monotonic())))
            except queue.Empty:
                if self.process.poll() is not None:
                    raise AssertionError(f'backend exited {self.process.returncode}')
                continue
            if 'reader_error' in event or event.get('event') == 'error':
                raise AssertionError(event)
            if predicate(event):
                return event
        raise AssertionError('timed out waiting for expected event')

    def sample(self, subscription):
        return self.wait(lambda e: e.get('event') == 'observation' and e.get('subscription') == subscription)

    def close(self, expected_exit=0):
        if not self.process.stdin.closed:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=3)
            raise AssertionError('backend failed graceful EOF cleanup')
        finally:
            self.reader.join(timeout=2)
            self.process.stdout.close()
            self.log.close()
            self.stderr.close()
        assert self.process.returncode == expected_exit, self.process.returncode
        assert not self.reader.is_alive()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', required=True)
    args = parser.parse_args()
    if not args.label.startswith(('round3-', 'round4-', 'round5-')) or not args.label.replace('-', '').isalnum():
        raise SystemExit('Use a descriptive unused round3-/round4-/round5- label')
    folder = HELPERS['checked_environment']()
    report = ROOT / 'artifacts/feasibility' / f'{args.label}.json'
    if report.exists() or any(folder.glob('backend-*')):
        raise SystemExit('Choose unused evidence paths; never overwrite earlier runs')
    HELPERS['inventory']()
    rmw = os.environ['RMW_IMPLEMENTATION']
    context = json.loads((folder / 'context.json').read_text())
    context['source_sha256'] = {path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest() for path in [
        'crates/backend/src/observation.rs', 'crates/backend/src/main.rs',
        'crates/protocol/src/session.rs', 'scripts/test-observe-session.py', 'scripts/ui-ros-workflow.py']}
    rows = []
    backend = None
    publisher = None
    pub_log = (folder / 'publisher.stderr').open('wb')
    def result(case, outcome='passed', details=None):
        rows.append({**context, 'middleware_pair': f'{rmw}/{rmw}',
                     'command': 'scripts/dev .pixi/envs/default/bin/python ' + shlex.join(sys.argv),
                     'environment': {k: os.environ.get(k) for k in ['ROS_DOMAIN_ID', 'ROS_LOCALHOST_ONLY', 'ROS_AUTOMATIC_DISCOVERY_RANGE', 'RMW_IMPLEMENTATION']},
                     'case': case, 'outcome': outcome, 'details': details,
                     'evidence': str(folder.relative_to(ROOT)), 'local_only': True})
        report.write_text(json.dumps(rows, indent=2) + '\n')
        print(outcome, case, flush=True)
    request = 0
    def start(id, topic, *, best=False, retained=False, ty='std_msgs/msg/String'):
        nonlocal request
        request += 1
        backend.send({'command': 'start', 'generation': 31, 'request': request, 'subscription': id,
                      'configuration': {'topic': topic, 'message_type': ty, 'best_effort': best, 'transient_local': retained, 'output': 'typed'}})
        return request
    def stop(id, expect_snapshot=True):
        nonlocal request
        request += 1
        backend.send({'command': 'stop', 'generation': 31, 'request': request, 'subscription': id})
        if expect_snapshot:
            snapshot = backend.wait(lambda e: e.get('event') in ('drops', 'stopped') and e.get('subscription') == id)
            assert snapshot['event'] == 'drops', 'cleanup acknowledgment preceded final backend drop snapshot'
            assert snapshot['generation'] == 31 and type(snapshot['backend_drops']) is int and snapshot['backend_drops'] >= 0
        backend.wait(lambda e: e.get('event') == 'stopped' and e.get('request') == request and e.get('subscription') == id)
    def ack(id):
        backend.wait(lambda e: e.get('event') == 'started' and e.get('subscription') == id)
    reliable = '/studio_fixture/ui/reliable'
    best = '/studio_fixture/ui/best_effort'
    try:
        publisher = subprocess.Popen([sys.executable, str(ROOT / 'scripts/ui-ros-workflow.py'), '--publisher', '--numeric-session', '--seconds', '120'], stdout=pub_log, stderr=pub_log)
        backend = Backend(folder, 1)
        ready = backend.wait(lambda e: e.get('event') == 'ready')
        expected = {'version': 7, 'max_subscriptions': 4, 'max_command_bytes': 131072,
                    'robot_contract_version': 1, 'max_robot_links': 128,
                    'max_robot_hz': 10, 'max_robot_sample_bytes': 524288}
        assert all(ready.get(key) == value for key, value in expected.items()), ready
        backend.wait(lambda e: e.get('event') == 'graph' and all(any(t[0] == topic for t in e['topics']) for topic in [reliable, best]))
        pid = backend.process.pid
        result('v7 handshake and real graph discovery', details={'pid': pid})
        start(1, reliable)
        ack(1)
        one = backend.sample(1)
        start(2, best, best=True)
        ack(2)
        two = backend.sample(2)
        assert one['sample']['topic'] == reliable and two['sample']['topic'] == best
        assert backend.sample(1)['sequence'] > one['sequence']
        result('two independent live native subscriptions in one process')
        for id in [3, 4]:
            start(id, '/studio_fixture/ui/numeric' if id == 4 else reliable,
                  ty='std_msgs/msg/UInt64' if id == 4 else 'std_msgs/msg/String')
            ack(id)
            sample = backend.sample(id)
            if id == 4:
                assert sample['sample']['message_type'] == 'std_msgs/msg/UInt64'
                assert sample['sample']['value']['fields'][0]['value']['kind'] == 'unsigned'
        req = start(5, reliable)
        backend.wait(lambda e: e.get('event') == 'request_error' and e.get('request') == req and e.get('no_handle') is True)
        assert backend.process.pid == pid and backend.process.poll() is None
        result('four handles; fifth rejected without disturbing peers')
        result('String and UInt64 typed samples coexist without cross-routing')
        stop(1)
        before = backend.sample(2)['sequence']
        stop(1, expect_snapshot=False)  # Already-stopped ID: original snapshot was final.
        start(6, reliable)
        ack(6)
        backend.sample(6)
        assert backend.sample(2)['sequence'] > before
        assert backend.process.pid == pid and backend.process.poll() is None
        result('stop retry and replacement preserve peer progress and backend PID')
        stop(3)
        req = start(7, reliable, ty='std_msgs/msg/DoesNotExist')
        snapshot = backend.wait(lambda e: e.get('event') in ('drops', 'request_error') and e.get('subscription') == 7)
        assert snapshot['event'] == 'drops' and snapshot['backend_drops'] == 0
        backend.wait(lambda e: e.get('event') == 'request_error' and e.get('request') == req and e.get('no_handle') is True)
        backend.sample(2)
        result('scoped native open failure leaves peers active')
        start(8, '/studio_fixture/ui/retained', retained=True)
        ack(8)
        retained = backend.sample(8)
        assert 'retained-once' in json.dumps(retained['sample']['value'])
        result('reliable transient-local late join receives retained data')
        stop(4)
        stop(2)
        start(9, best)  # Reliable subscriber is incompatible with best-effort offer.
        ack(9)
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            event = backend.wait(lambda e: e.get('event') == 'observation', seconds=3)
            assert event['subscription'] != 9, 'unexpected delivery across QoS mismatch'
        stop(9)
        start(10, best, best=True)
        ack(10)
        backend.sample(10)
        backend.sample(6)
        result('QoS mismatch and explicit correction isolated from peers')
        backend.close()
        backend = None
        result('EOF closes three active native owners and process exits cleanly')
        backend = Backend(folder, 2)
        backend.wait(lambda e: e.get('event') == 'ready')
        graph_count = 0
        deadline = time.monotonic() + 2.2
        while time.monotonic() < deadline:
            event = backend.wait(lambda e: True, seconds=3)
            assert event.get('event') == 'graph', event
            graph_count += 1
        assert graph_count >= 2
        result('new session discovers without replaying subscriptions')
        # Cancel immediately during native startup; stopped is the only success.
        request = 0
        start(1, reliable)
        stop(1)
        backend.close()
        backend = None
        result('start immediately canceled and session exits cleanly')
        backend = Backend(folder, 3)
        backend.wait(lambda e: e.get('event') == 'ready')
        backend.send({'command': 'publish', 'topic': '/cmd_vel'})
        error = backend.events.get(timeout=5)
        assert error.get('event') == 'error' and len(error['message'].encode()) <= 512, error
        backend.close(expected_exit=1)
        backend = None
        result('unsupported command causes bounded session failure and clean process exit')
        for index, arguments in [(4, ()), (5, (reliable, 'std_msgs/msg/String'))]:
            backend = Backend(folder, index, arguments)
            backend.wait(lambda e: e.get('event') == 'hello' and e.get('version') == 2)
            backend.wait(lambda e: e.get('event') == ('typed_sample' if arguments else 'graph'))
            # Legacy modes deliberately do not interpret stdin. Terminate only
            # this recorded fixture process after confirming its v2 behavior.
            backend.process.terminate()
            backend.close(expected_exit=-15)
            backend = None
        result('legacy v2 graph-only and TOPIC TYPE modes remain operational')
    except Exception as error:
        result('acceptance interrupted', 'failed', repr(error))
        raise
    finally:
        if backend is not None:
            try:
                backend.close()
            except Exception as error:
                result('cleanup', 'failed', repr(error))
        if publisher is not None:
            publisher.terminate()
            try:
                publisher.wait(timeout=5)
            except subprocess.TimeoutExpired:
                publisher.kill()
                publisher.wait(timeout=3)
            assert publisher.poll() is not None
            result('owned publisher reaped', details={'pid': publisher.pid, 'returncode': publisher.returncode})
        pub_log.close()


if __name__ == '__main__':
    main()
