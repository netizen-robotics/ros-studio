"""Bounded session7/robot1 backend acceptance on synthetic localhost ROS.

Run matching --rmw pairs separately through scripts/dev and project Python.
No native window, URDF loader, GPU, LAN, physical robot or motion acceptance.
"""
import argparse
import importlib.util
import json
import math
import os
from pathlib import Path
import queue
import shlex
import signal
import subprocess
import sys
import time
import traceback

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


FIXTURE = load('robot_fixture', 'scripts/robot-ros-workflow.py')
NORMAL = load('robot_capture_base', 'scripts/test-image-session.py')


class Backend(NORMAL.Backend):
    def __init__(self, folder, index, generation, deadline):
        self.deadline = deadline
        self.retired = set()
        self.drops = {}
        self.robot_counts = {}
        self.requested = set()
        super().__init__(folder, index, generation)

    def read(self):
        try:
            while True:
                line = self.process.stdout.readline(NORMAL.MAX_LINE + 1)
                if not line:
                    return
                assert len(line) <= NORMAL.MAX_LINE and line.endswith(b'\n'), 'Invalid bounded wire frame'
                event = json.loads(line)
                sample = event.get('event') in ('observation', 'image_observation', 'spatial_observation', 'robot_observation')
                sub = event.get('subscription', 0)
                if sample:
                    assert type(sub) is int and 1 <= sub <= 32
                if (not sample or self.seen.get(sub, 0) < 2) and self.captured + len(line) <= NORMAL.CAPTURE_BYTES:
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

    def wait(self, predicate, seconds=25):
        def observe(event):
            kind, sub = event.get('event'), event.get('subscription')
            if kind in ('started', 'observation', 'robot_observation', 'drops'):
                assert sub in self.requested, 'Backend activated an unrequested subscription'
            if kind in ('observation', 'robot_observation', 'drops'):
                assert event['generation'] == self.generation
                assert type(sub) is int and 1 <= sub <= 32
                assert type(event['backend_drops']) is int and event['backend_drops'] >= self.drops.get(sub, 0)
                self.drops[sub] = event['backend_drops']
                if kind == 'drops':
                    assert sub not in self.retired, 'Repeated final loss snapshot'
                    self.retired.add(sub)
                else:
                    assert sub not in self.retired, 'Sample followed final cleanup snapshot'
            if kind == 'robot_observation':
                assert event['sequence'] > self.sequence.get(sub, 0)
                self.sequence[sub] = event['sequence']
                counts = event['sample']['counters']
                assert set(counts) == {'tf_dynamic_batches','tf_static_batches','tf_accepted','tf_rejected',
                                       'duplicate_statics','native_errors','pose_batches'}
                for key, value in counts.items():
                    assert type(value) is int and value >= self.robot_counts.get(sub, {}).get(key, 0)
                self.robot_counts[sub] = counts
            return predicate(event)
        remaining = self.deadline - time.monotonic()
        assert remaining > 0, 'Whole Robot matrix deadline exceeded'
        return super().wait(observe, min(seconds, remaining))

    def start_configuration(self, sub, configuration, ack=True):
        self.request += 1
        request = self.request
        self.requested.add(sub)
        self.send({'command': 'start', 'generation': self.generation, 'request': request,
                   'subscription': sub, 'configuration': configuration})
        if ack:
            event = self.wait(lambda e: e.get('subscription') == sub and e.get('event') in ('started', 'request_error', 'stream_error'))
            assert event['event'] == 'started' and event['request'] == request, event
        return request

    def robot(self, sub, clock='verified_system', ack=True):
        return self.start_configuration(sub, robot_configuration(clock), ack)

    def peer(self, sub, topic):
        self.start_configuration(sub, {'topic': topic, 'message_type': 'std_msgs/msg/Float64',
                                      'best_effort': False, 'transient_local': False, 'output': 'typed',
                                      'spatial': None, 'robot': None})
        return self.sample(sub, 'observation')

    def robot_sample(self, sub, tip_ready):
        event = self.wait(lambda e: e.get('subscription') == sub and (
            e.get('event') in ('request_error', 'stream_error') or
            (e.get('event') == 'robot_observation' and robot_state(e['sample'], tip_ready))))
        assert event['event'] == 'robot_observation', event
        return event

    def stop_robot(self, sub):
        self.request += 1
        request = self.request
        self.send({'command': 'stop', 'generation': self.generation, 'request': request, 'subscription': sub})
        first = self.wait(lambda e: e.get('event') in ('drops', 'stopped') and e.get('subscription') == sub)
        assert first['event'] == 'drops', 'Cleanup acknowledgment preceded final loss snapshot'
        event = self.wait(lambda e: e.get('event') == 'stopped' and e.get('subscription') == sub)
        assert event['request'] == request and sub in self.retired
        return event


def robot_configuration(clock='verified_system'):
    return {'topic': FIXTURE.TF_TOPIC, 'message_type': 'tf2_msgs/msg/TFMessage', 'output': 'robot',
            'best_effort': False, 'transient_local': False, 'spatial': None,
            'robot': {'fixed_frame': 'map', 'frames': list(FIXTURE.LINKS), 'clock_policy': clock,
                      'dynamic_tf_topic': FIXTURE.TF_TOPIC, 'dynamic_tf_best_effort': False,
                      'static_tf_topic': FIXTURE.STATIC_TOPIC}}


def peer_value(event):
    sample = event['sample']
    assert sample['contract_version'] == 2 and sample['message_type'] == 'std_msgs/msg/Float64'
    field = NORMAL.member(sample['value'], 'data')
    assert field['kind'] == 'float' and field['value']['kind'] == 'finite'
    value = float(field['value']['value'])
    assert math.isfinite(value)
    return value


def robot_state(sample, tip_ready):
    assert sample['contract_version'] == 1 and sample['fixed_frame'] == 'map'
    assert sample['policy'] == 'latest_available'
    assert [link['frame'] for link in sample['links']] == list(FIXTURE.LINKS)
    poses = {link['frame']: link['pose'] for link in sample['links']}
    for pose in poses.values():
        assert pose['status'] in ('ready', 'missing', 'stale', 'extrapolated', 'lookup_timeout')
        if pose['status'] == 'ready':
            stamp, components = pose['resolved_stamp_ns'], pose['components']
            assert type(stamp) is int and 0 <= stamp <= 2147483647999999999
            assert len(components) == 7 and all(math.isfinite(v) for v in components)
            assert max(abs(v) for v in components[:3]) <= 1_000_000
            assert abs(sum(v*v for v in components[3:]) - 1) < 1e-6
    if any(poses[name]['status'] != 'ready' for name in ('base', 'arm', 'ascii_mesh', 'binary_mesh')):
        return False
    for name in ('base', 'ascii_mesh', 'binary_mesh'):
        assert poses[name]['resolved_stamp_ns'] == 0, 'Static native result must not use receipt time'
        assert poses[name]['components'] == [0, 0, 0, 0, 0, 0, 1]
    assert poses['arm']['resolved_stamp_ns'] > 0
    assert max(abs(a-b) for a,b in zip(poses['arm']['components'][:3], [0, 0, .2])) < 1e-9
    return poses['tip']['status'] == ('ready' if tip_ready else 'missing')


def validate_chain(sample):
    poses = {link['frame']: link['pose'] for link in sample['links']}
    arm, tip = poses['arm'], poses['tip']
    if arm['resolved_stamp_ns'] != tip['resolved_stamp_ns']:
        return {'common_stamp_comparison': 'not applicable: independent latest times differ'}
    a, t = arm['components'], tip['components']
    qx,qy,qz,qw = a[3:]
    # Native fixed<-tip must equal fixed<-arm applied to the declared .6m mount.
    expected = [a[0]+.6*2*(qx*qz+qw*qy), a[1]+.6*2*(qy*qz-qw*qx),
                a[2]+.6*(1-2*(qx*qx+qy*qy))]
    assert max(abs(x-y) for x,y in zip(t[:3],expected)) < 1e-9
    assert max(abs(x-y) for x,y in zip(t[3:],a[3:])) < 1e-9
    return {'common_stamp_comparison': 'passed', 'native_stamp_ns': arm['resolved_stamp_ns'],
            'expected_tip_translation': expected, 'actual_tip_translation': t[:3]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rmw', choices=FIXTURE.RMWS, required=True)
    parser.add_argument('--label', required=True)
    args = parser.parse_args()
    if not 1 <= len(args.label) <= 96 or not args.label.replace('-', '').isalnum() or not args.label.isascii():
        parser.error('Use an unused descriptive ASCII alphanumeric/hyphen label')
    folder = ROOT / 'artifacts/feasibility/evidence' / args.label
    report = ROOT / 'artifacts/feasibility' / (args.label + '.json')
    if report.exists() or folder.exists():
        parser.error('Preserve earlier evidence: use unused report and evidence paths')
    folder = FIXTURE.checked_environment(args.rmw, folder)
    FIXTURE.HELPERS['inventory']()
    context = json.loads((folder / 'context.json').read_text())
    context['middleware_pair'] = [args.rmw, args.rmw]
    context['versions']['rust'] = subprocess.run(['rustc', '--version'], check=True, capture_output=True, text=True, timeout=10).stdout.strip()
    import hashlib
    sources = ['scripts/test-robot-session.py', 'scripts/robot-ros-workflow.py', 'scripts/test-image-session.py',
               'crates/protocol/src/robot.rs', 'crates/protocol/src/session.rs', 'crates/backend/src/robot_worker.rs',
               'crates/backend/src/observation.rs', 'crates/backend/native/tf_shim.cpp']
    context['source_sha256'] = {path: hashlib.sha256((ROOT/path).read_bytes()).hexdigest() for path in sources}
    context['environment'] = {key: os.environ.get(key) for key in [
        'ROS_DISTRO','ROS_DOMAIN_ID','ROS_LOCALHOST_ONLY','ROS_AUTOMATIC_DISCOVERY_RANGE',
        'RMW_IMPLEMENTATION','SKIP_DEFAULT_XML','AMENT_PREFIX_PATH','DYLD_LIBRARY_PATH',
        'ROS_STATIC_PEERS','ROS_DISCOVERY_SERVER','ROS_SUPER_CLIENT','CYCLONEDDS_URI',
        'FASTDDS_DEFAULT_PROFILES_FILE','FASTRTPS_DEFAULT_PROFILES_FILE','FASTDDS_ENVIRONMENT_FILE']}
    rows, owned, backends = [], [], []
    publisher = None
    pub_log = (folder / 'publisher.stderr').open('wb')
    deadline = time.monotonic() + 90
    def record(case, outcome='passed', details=None):
        rows.append({**context, 'test': case, 'outcome': outcome, 'details': details,
                     'command': 'scripts/dev .pixi/envs/default/bin/python ' + shlex.join(sys.argv),
                     'evidence': str(folder.relative_to(ROOT)), 'local_only': True,
                     'scope': 'Real Rust backend + local synthetic ROS TF/Float64; no native UI, assets, GPU, LAN, hardware or motion claim.'})
        report.write_text(json.dumps(rows, indent=2) + '\n')
        print(outcome, case, flush=True)
    def save_owned():
        (folder / 'owned-processes.json').write_text(json.dumps(owned, indent=2) + '\n')
    def connect(index, generation):
        backend = Backend(folder, index, generation, deadline)
        backends.append(backend)
        owned.append({'pid': backend.process.pid, 'argv': [str(ROOT/'target/debug/studio-backend'), '--observe-session']})
        save_owned()
        ready = backend.wait(lambda e:e.get('event') == 'ready')
        expected = {'version':7,'robot_contract_version':1,'max_robot_links':128,'max_robot_hz':10,
                    'max_robot_sample_bytes':524288,'max_native_handles':4,'max_subscriptions':4,'max_command_bytes':131072}
        assert all(ready.get(k)==v for k,v in expected.items()), ready
        return backend
    try:
        publisher_args = [sys.executable,str(ROOT/'scripts/robot-ros-workflow.py'),'--rmw',args.rmw,
                          '--evidence',str(folder/'publisher'),'--mode','recover','--seconds','115']
        publisher = subprocess.Popen(publisher_args, stdout=pub_log, stderr=pub_log)
        owned.append({'pid':publisher.pid,'argv':publisher_args});save_owned()
        backend = connect(1,91)
        topics = [FIXTURE.TF_TOPIC,FIXTURE.STATIC_TOPIC,*FIXTURE.STATUS_TOPICS]
        backend.wait(lambda e:e.get('event') == 'graph' and all(any(t[0]==topic for t in e['topics']) for topic in topics))
        backend.wait(lambda e:e.get('event') == 'graph')
        assert not backend.sequence, 'Discovery activated a subscription'
        record('session7 exact negotiation and discovery without activation')
        backend.robot(1)
        initial = backend.robot_sample(1,False)
        (folder/'initial-partial.json').write_text(json.dumps(initial,indent=2)+'\n')
        assert initial['sample']['counters']['tf_static_batches'] > 0
        record('retained static native zero stamps; articulated arm positive native stamp; only tip missing')
        peers = [backend.peer(2,FIXTURE.STATUS_TOPICS[0]),backend.peer(3,FIXTURE.STATUS_TOPICS[1])]
        req = backend.start_configuration(4,{'topic':FIXTURE.STATUS_TOPICS[0],'message_type':'std_msgs/msg/Float64','best_effort':False,'transient_local':False,'output':'typed'},ack=False)
        error = backend.wait(lambda e:e.get('event') == 'request_error' and e.get('request') == req)
        assert error['no_handle'] is True
        req = backend.robot(5,ack=False)
        error = backend.wait(lambda e:e.get('event') == 'request_error' and e.get('request') == req)
        assert error['no_handle'] is True and 'Only one' in error['message']
        scan = {'topic':'/robot_demo/unused_scan','message_type':'sensor_msgs/msg/LaserScan','output':'spatial_scan','best_effort':False,'transient_local':False,'robot':None,
                'spatial':{'fixed_frame':'map','clock_policy':'verified_system','dynamic_tf_topic':FIXTURE.TF_TOPIC,'dynamic_tf_best_effort':False,'static_tf_topic':FIXTURE.STATIC_TOPIC}}
        req = backend.start_configuration(6,scan,ack=False)
        error = backend.wait(lambda e:e.get('event') == 'request_error' and e.get('request') == req)
        assert error['no_handle'] is True and 'Only one' in error['message']
        for sub,previous in zip((2,3),peers):
            advanced = backend.wait(lambda e: e.get('event') == 'observation' and e.get('subscription') == sub and peer_value(e) > peer_value(previous))
            assert advanced['sequence'] > previous['sequence']
        record('two ordinary peers fit two-resource Robot; fifth resource and second TF groups rejected')
        control = folder/'publisher/mode.tmp';control.write_text('ready\n');control.replace(folder/'publisher/mode.txt')
        recovered = backend.robot_sample(1,True)
        chain = validate_chain(recovered['sample'])
        (folder/'recovered-native-chain.json').write_text(json.dumps({'event':recovered,'chain':chain},indent=2)+'\n')
        record('explicit missing-tip recovery preserves other links and native per-link resolved times',details=chain)
        backend.stop_robot(1)
        for sub in (2,3):
            backend.sample(sub,'observation')
        record('Stop final loss snapshot precedes cleanup acknowledgment; both peers continue')
        for sub,clock in [(7,'unknown'),(8,'simulated')]:
            req = backend.robot(sub,clock=clock,ack=False)
            error = backend.wait(lambda e:e.get('event') == 'request_error' and e.get('request') == req)
            assert error['no_handle'] is True
            backend.sample(2,'observation')
        record('unverified and simulated clocks rejected before opening Robot handles')
        backend.robot(9)
        restarted = backend.robot_sample(9,True)
        validate_chain(restarted['sample'])
        backend.close();assert not backend.error, backend.error
        record('explicit Robot restart receives complete retained statics; EOF reaps Robot and peers')
        backend = connect(2,92)
        backend.wait(lambda e:e.get('event') == 'graph')
        backend.wait(lambda e:e.get('event') == 'graph')
        assert not backend.sequence, 'Reconnect replayed earlier configuration'
        backend.robot(1)
        backend.robot_sample(1,True)
        backend.close();assert not backend.error, backend.error
        record('fresh backend generation remains inactive until explicit Robot Start; retained recovery survives late join')
    except Exception as error:
        (folder/'failure.txt').write_text(traceback.format_exc())
        record('Robot session interrupted','failed',{'error':repr(error)})
        record('subsequent Robot scenarios','not tested',{'reason':'Earlier failed case interrupted the bounded matrix; no later pass is inferred.'})
    finally:
        errors=[]
        for backend in backends:
            try:
                backend.close()
            except Exception as error:
                errors.append(repr(error))
        if publisher is not None:
            if publisher.poll() is None:
                publisher.send_signal(signal.SIGINT)
            try:
                publisher.wait(timeout=5)
            except subprocess.TimeoutExpired:
                errors.append('Publisher did not exit on SIGINT');publisher.kill();publisher.wait(timeout=3)
            if publisher.returncode != 0:
                errors.append(f'Publisher exit {publisher.returncode}')
        for process in owned:
            try:
                os.kill(process['pid'],0)
            except ProcessLookupError:
                pass
            else:
                errors.append(f'Owned PID remains: {process["pid"]}')
        pub_log.close()
        record('all recorded backend and publisher owners reaped','failed' if errors else 'passed',{'owned':owned,'errors':errors})
    if any(row['outcome'] != 'passed' for row in rows):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
