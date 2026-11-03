"""Bounded current-session/cloud2 acceptance with actual localhost synthetic ROS.

Run matching explicit RMW pairs separately through scripts/dev project Python.
This establishes backend/data behavior only, never native UI, FPS, LAN or robots.
"""
import argparse
from marker_wire import CAPABILITIES as MARKER_CAPABILITIES
from pose_wire import CAPABILITIES as POSE_CAPABILITIES
import hashlib
from cloud_wire import decode_points
import json
from session_wire import VERSION as SESSION_VERSION, CAMERA_CAPABILITIES
import math
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
import importlib.util

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


ROBOT = load('sensor_robot_acceptance', 'scripts/test-robot-session.py')
FIXTURE = load('sensor_fixture_acceptance', 'scripts/sensor-scene-workflow.py')
BASE = ROBOT.NORMAL
SAMPLES = {'observation', 'image_observation', 'spatial_observation',
           'robot_observation', 'point_cloud_observation', 'tf_graph_observation'}
EXPECTED = dict(version=SESSION_VERSION, camera_snapshot=CAMERA_CAPABILITIES, pose_snapshot=POSE_CAPABILITIES, marker_snapshot=MARKER_CAPABILITIES, max_subscriptions=32, max_native_handles=64,
                max_command_bytes=131072, max_sample_bytes=524288,
                image_contract_version=2, max_image_raw_bytes=16777216,
                max_image_sample_bytes=25165824, spatial_contract_version=1,
                max_spatial_scan_rays=2048, max_tf_frames=256, max_tf_edges=128,
                max_tf_dynamic_inserts=256, tf_cache_window_ns=10000000000,
                spatial_pending_timeout_ms=1000, spatial_retry_interval_ms=50,
                robot_contract_version=1, max_robot_links=128, max_robot_hz=10,
                max_robot_sample_bytes=524288, shared_scene_contract_version=1,
                max_sensor_layers=4, point_cloud_contract_version=2,
                max_cloud_source_points=8192, max_cloud_raw_bytes=262144,
                max_cloud_sample_bytes=524288, map_snapshot={'contract_version': 2, 'max_dimension': 2048, 'max_cells': 4194304, 'max_chunk_bytes': 196608, 'max_chunks': 22, 'max_sample_bytes': 524288, 'max_layers': 3, 'max_update_readers_per_map': 1}, robot_description=dict(contract_version=1, max_document_bytes=1048576, max_chunk_bytes=196608, max_chunks=6, max_owners=1), tf_graph=dict(contract_version=1, max_hz=5, max_sample_bytes=524288, max_owners=1), mapping_contract_version=1, max_map_dimension=256, max_map_cells=65536, max_path_poses=1024, max_mapping_sample_bytes=524288, max_map_layers=3, max_pose_layers=1, max_path_layers=1)


class Backend(ROBOT.Backend):
    """Validate every wire sample; keep only eight latest client preview slots.

    Client coalescing is separately counted and never called backend loss.
    Controls retain ordering and are never coalesced. Capture bytes are bounded.
    """
    def __init__(self, folder, index, generation, deadline):
        self.lock = threading.Lock()
        self.latest = {}
        self.graph = None
        self.client_coalesced = 0
        self.history = []
        self.counts = {}
        self.pause_requested = threading.Event()
        self.paused = threading.Event()
        super().__init__(folder, index, generation, deadline)

    def read(self):
        try:
            while True:
                if self.pause_requested.is_set():
                    self.paused.set()
                    while self.pause_requested.is_set():
                        time.sleep(.005)
                    self.paused.clear()
                line = self.process.stdout.readline(BASE.MAX_LINE + 1)
                if not line:
                    return
                assert len(line) <= BASE.MAX_LINE and line.endswith(b'\n'), 'Unbounded wire frame'
                event = json.loads(line)
                kind, sub = event.get('event'), event.get('subscription')
                sample = kind in SAMPLES
                if sample or kind in ('started', 'drops', 'stopped'):
                    assert sub in self.requested and event['generation'] == self.generation, event
                with self.lock:
                    if sample or kind == 'drops':
                        assert type(sub) is int and 1 <= sub <= 64
                        count = event['backend_drops']
                        assert type(count) is int and count >= self.drops.get(sub, 0)
                        self.drops[sub] = count
                        assert sub not in self.retired, 'Wire event followed final loss snapshot'
                        if sample:
                            assert type(event['sequence']) is int and event['sequence'] > self.sequence.get(sub, 0)
                            self.sequence[sub] = event['sequence']
                            counters = event['sample'].get('counters', {})
                            for key, value in counters.items():
                                assert type(value) is int and value >= self.counts.get(sub, {}).get(key, 0)
                            self.counts[sub] = counters
                        else:
                            self.retired.add(sub)
                            self.latest.pop(sub, None)
                    if kind in ('started', 'stopped', 'request_error', 'stream_error', 'drops'):
                        assert len(self.history) < 256, 'Control history budget exceeded'
                        self.history.append(event)
                    if sample:
                        if sub in self.latest:
                            self.client_coalesced += 1
                        self.latest[sub] = event
                        assert len(self.latest) <= 8
                    elif kind == 'graph':
                        self.graph = event
                if (not sample or self.seen.get(sub, 0) < 2) and self.captured + len(line) <= BASE.CAPTURE_BYTES:
                    self.capture.write(line)
                    self.capture.flush()
                    self.captured += len(line)
                    if sample:
                        self.seen[sub] = self.seen.get(sub, 0) + 1
                else:
                    self.omitted += 1
                if not sample and kind != 'graph':
                    self.events.put(event, timeout=3)
        except Exception as error:
            self.error = repr(error)
        finally:
            self.capture.close()
            self.finished.set()

    def wait(self, predicate, seconds=25):
        end = min(self.deadline, time.monotonic() + seconds)
        while time.monotonic() < end:
            assert self.error is None, self.error
            try:
                event = self.events.get_nowait()
            except queue.Empty:
                with self.lock:
                    if self.graph is not None:
                        event, self.graph = self.graph, None
                    elif self.latest:
                        sub = next(iter(self.latest))
                        event = self.latest.pop(sub)
                    else:
                        event = None
                if event is None:
                    assert not self.finished.is_set(), 'Backend ended while awaiting acceptance'
                    time.sleep(.005)
                    continue
            assert event.get('event') != 'error', event
            if predicate(event):
                return event
        raise AssertionError('Bounded sensor acceptance wait expired')

    def pause(self):
        self.pause_requested.set()
        assert self.paused.wait(timeout=2), 'Reader did not enter bounded pause'

    def resume(self):
        self.pause_requested.clear()

    def close(self):
        self.resume()
        super().close()

    def scene(self, sub, kind, topic=None, ack=True):
        return self.start_configuration(sub, configuration(kind, topic), ack)

    def ready_sample(self, sub, output):
        return self.wait(lambda e: e.get('subscription') == sub and e.get('event') == output
                         and e['sample']['tf']['status'] == 'ready')

    def robot_ready(self, sub=1):
        return self.wait(lambda e: e.get('subscription') == sub and e.get('event') == 'robot_observation'
                         and ROBOT.robot_state(e['sample'], True))


def configuration(kind, topic=None):
    tf = dict(fixed_frame='map', clock_policy='verified_system',
              dynamic_tf_topic=FIXTURE.TOPICS['dynamic'], dynamic_tf_best_effort=False,
              static_tf_topic=FIXTURE.TOPICS['static'])
    if kind == 'robot':
        return dict(topic=FIXTURE.TOPICS['dynamic'], message_type='tf2_msgs/msg/TFMessage',
                    best_effort=False, transient_local=False, output='robot', spatial=None,
                    robot={**tf, 'frames': list(FIXTURE.ROBOT.LINKS)})
    cloud = kind == 'point_cloud'
    return dict(topic=topic or FIXTURE.TOPICS['cloud' if cloud else 'scan_a'],
                message_type='sensor_msgs/msg/PointCloud2' if cloud else 'sensor_msgs/msg/LaserScan',
                best_effort=True, transient_local=False, output=kind, spatial=tf, robot=None)


def check_tf(sample, translation, yaw):
    assert sample['contract_version'] == (2 if sample.get('message_type') == 'sensor_msgs/msg/PointCloud2' else 1) and sample['fixed_frame'] == 'map'
    assert type(sample['acquisition_stamp_ns']) is int and 0 < sample['acquisition_stamp_ns'] <= 2147483647999999999
    tf = sample['tf']
    assert tf['status'] == 'ready'
    expected = [*translation, 0, 0, math.sin(yaw/2), math.cos(yaw/2)]
    assert max(abs(a-b) for a,b in zip(tf['components'], expected)) < 1e-8
    return tf['components']


def check_scan(event, source, translation, yaw):
    sample = event['sample']
    assert sample['source_frame'] == source
    check_tf(sample, translation, yaw)
    header = BASE.member(sample['scan']['value'], 'header')
    stamp = BASE.member(header, 'stamp')
    sec = BASE.member(stamp, 'sec')
    nsec = BASE.member(stamp, 'nanosec')
    # Integer values are tagged structured scalars; compare to acquisition time.
    def integer(value):
        assert value['kind'] in ('signed', 'unsigned'), value
        return int(value['value'])
    assert sample['acquisition_stamp_ns'] == integer(sec)*1000000000 + integer(nsec)


def check_cloud(event, mode='nominal'):
    sample = event['sample']
    points = decode_points(sample['points'])
    assert sample['topic'] == FIXTURE.TOPICS['cloud']
    assert sample['message_type'] == 'sensor_msgs/msg/PointCloud2' and sample['source_frame'] == 'sensor_cloud'
    tf = check_tf(sample, [1,2,3], math.pi/2)
    colors = [[193,2,3,255], [17,34,51,0], [68,85,102,128], [119,136,153,64]]
    attributed = mode in ('cloud-rgb', 'cloud-rgba', 'cloud-attributes-skipped', 'cloud-pressure')
    indices = list(range(8192 if mode == 'cloud-pressure' else 0 if mode == 'cloud-empty' else 4))
    if mode in ('cloud-skipped', 'cloud-attributes-skipped'):
        indices = [1,3]
    channels = sample['channels']
    if mode == 'cloud-unsupported':
        assert channels == dict(color=dict(status='unsupported'), intensity=dict(status='unsupported'))
    else:
        expected_colors = [colors[i % 4][:3] + [255 if mode == 'cloud-rgb' else colors[i % 4][3]] for i in indices]
        assert channels['color'] == (dict(status='values', values=expected_colors) if attributed else dict(status='missing'))
        intensities = [None, -42.] if mode == 'cloud-attributes-skipped' else [float(i) for i in indices]
        assert channels['intensity'] == dict(status='values', values=intensities)
    if mode == 'cloud-pressure':
        assert sample['source_points'] == 8192 and sample['skipped_points'] == 0 and len(points) == 8192
        return
    expected = FIXTURE.LAYOUT['points']
    count, skipped = 4, 0
    if mode == 'cloud-empty':
        expected, count = [], 0
    elif mode in ('cloud-skipped', 'cloud-attributes-skipped'):
        expected, skipped = [expected[1], expected[3]], 2
    assert sample['source_points'] == count and sample['skipped_points'] == skipped
    assert len(points) == len(expected)
    for actual, wanted in zip(points, expected):
        assert max(abs(a-b) for a,b in zip(actual,wanted)) < 1e-6
    # Apply the native map<-sensor_cloud transform to noncoplanar source XYZ.
    if mode == 'nominal':
        actual_fixed = [[tf[0]-p[1], tf[1]+p[0], tf[2]+p[2]] for p in points]
        for actual, wanted in zip(actual_fixed, FIXTURE.LAYOUT['expected_fixed_points']):
            assert max(abs(a-b) for a,b in zip(actual,wanted)) < 1e-6


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rmw', choices=FIXTURE.RMWS, required=True)
    parser.add_argument('--label', required=True)
    args = parser.parse_args()
    if not 1 <= len(args.label) <= 96 or not args.label.isascii() or not args.label.replace('-', '').isalnum():
        parser.error('Use an unused ASCII alphanumeric/hyphen label')
    folder = ROOT/'artifacts/feasibility/evidence'/args.label
    report = ROOT/'artifacts/feasibility'/(args.label+'.json')
    if folder.exists() or report.exists():
        parser.error('Preserve prior evidence: choose unused paths')
    folder = FIXTURE.checked_environment(args.rmw, folder)
    FIXTURE.HELPERS['inventory']()
    context = json.loads((folder/'context.json').read_text())
    context['middleware_pair'] = [args.rmw,args.rmw]
    context['versions']['rust'] = subprocess.run(['rustc','--version'], capture_output=True, text=True, check=True, timeout=10).stdout.strip()
    sources = ['scripts/test-sensor-session.py','scripts/sensor-scene-workflow.py',
               'scripts/test-robot-session.py','scripts/test-image-session.py',
               'scripts/robot-ros-workflow.py','scripts/ui-ros-workflow.py',
               'fixtures/sensor_scene/cloud-layout.json','crates/protocol/src/session.rs',
               'crates/protocol/src/cloud.rs','crates/protocol/src/cloud_attributes.rs','crates/protocol/src/cloud_points.rs',
               'scripts/cloud_wire.py','crates/backend/src/scene_worker.rs',
               'crates/backend/src/cloud_codec.rs','crates/backend/src/observation.rs',
               'crates/backend/src/cloud_worker.rs','crates/backend/src/spatial_worker.rs',
               'crates/backend/src/robot_worker.rs','crates/backend/src/spatial_session.rs',
               'crates/backend/native/tf_shim.cpp','target/debug/studio-backend']
    context['source_sha256'] = {p:hashlib.sha256((ROOT/p).read_bytes()).hexdigest() for p in sources}
    context['environment'] = {key:os.environ.get(key) for key in (
        'ROS_DISTRO','ROS_DOMAIN_ID','ROS_LOCALHOST_ONLY','ROS_AUTOMATIC_DISCOVERY_RANGE',
        'RMW_IMPLEMENTATION','ROS_STATIC_PEERS','ROS_DISCOVERY_SERVER','ROS_SUPER_CLIENT',
        'CYCLONEDDS_URI','FASTDDS_DEFAULT_PROFILES_FILE','FASTRTPS_DEFAULT_PROFILES_FILE',
        'FASTDDS_ENVIRONMENT_FILE','SKIP_DEFAULT_XML','AMENT_PREFIX_PATH','DYLD_LIBRARY_PATH')}
    command = 'scripts/dev .pixi/envs/default/bin/python '+shlex.join(sys.argv)
    rows, owned, backends, publisher_logs = [], [], [], []
    deadline = time.monotonic()+240
    publisher = None
    publisher_index = 0

    def record(case, outcome='passed', details=None):
        rows.append({**context,'test':case,'outcome':outcome,'details':details,'command':command,
                     'evidence':str(folder.relative_to(ROOT)),'local_only':True,
                     'scope':'Actual Rust backend + localhost synthetic ROS; no native UI, assets, FPS, LAN, packaging or hardware acceptance.'})
        report.write_text(json.dumps(rows,indent=2)+'\n')
        print(outcome,case,flush=True)

    def save_owned():
        (folder/'owned-processes.json').write_text(json.dumps(owned,indent=2)+'\n')

    def stop_publisher():
        nonlocal publisher
        if publisher is None:
            return
        child = publisher
        if child.poll() is None:
            child.send_signal(signal.SIGINT)
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill();child.wait(timeout=3)
            raise AssertionError('Owned fixture required forced kill')
        publisher = None
        assert child.returncode == 0, f'Publisher exit {child.returncode}'
        state = json.loads((folder/f'publisher-{publisher_index}'/'publisher-state.json').read_text())
        assert state['event'] == 'stopped' and state['pid'] == child.pid, state

    def publish(mode, big_endian=False):
        nonlocal publisher,publisher_index
        stop_publisher()
        publisher_index += 1
        path = folder/f'publisher-{publisher_index}'
        argv = [sys.executable,str(ROOT/'scripts/sensor-scene-workflow.py'),'--rmw',args.rmw,
                '--evidence',str(path),'--mode',mode,'--seconds','300']
        if big_endian:
            argv.append('--big-endian')
        log = (folder/f'publisher-{publisher_index}.log').open('wb');publisher_logs.append(log)
        publisher = subprocess.Popen(argv,stdout=log,stderr=log)
        owned.append({'pid':publisher.pid,'argv':argv,'evidence':str(path.relative_to(ROOT))});save_owned()
        # The graph wait below establishes discovery; state only confirms startup.
        end = min(deadline,time.monotonic()+20)
        while time.monotonic() < end:
            assert publisher.poll() is None, f'Publisher exited {publisher.returncode}'
            if (path/'publisher-state.json').exists():
                return path
            time.sleep(.02)
        raise AssertionError('Fixture startup deadline exceeded')

    def connect(index):
        backend = Backend(folder,index,90+index,deadline);backends.append(backend)
        owned.append({'pid':backend.process.pid,'argv':[str(ROOT/'target/debug/studio-backend'),'--observe-session']});save_owned()
        ready = backend.wait(lambda e:e.get('event') == 'ready')
        assert set(ready) == {'event','distro','rmw',*EXPECTED}, ready
        assert all(ready[k] == v for k,v in EXPECTED.items()) and ready['rmw'] == args.rmw and ready['distro'] == 'jazzy', ready
        return backend

    def reject(backend,sub,cfg):
        request = backend.start_configuration(sub,cfg,ack=False)
        error = backend.wait(lambda e:e.get('event') == 'request_error' and e.get('request') == request)
        assert error['no_handle'] is True, error
        return error

    def peers_progress(backend,old=None):
        peer = backend.sample(5,'observation')
        if old is not None:
            peer = backend.wait(lambda e:e.get('event') == 'observation' and e.get('subscription') == 5
                                and e['sequence'] > old['sequence'])
        assert ROBOT.peer_value(peer) >= 0
        robot = backend.robot_ready()
        assert ROBOT.robot_state(robot['sample'],True)
        for sub,source,translation,yaw in ((2,'sensor_scan_a',[1,2,3],math.pi/2),(3,'sensor_scan_b',[-1,0,.5],-math.pi/2)):
            check_scan(backend.ready_sample(sub,'spatial_observation'),source,translation,yaw)
        return peer

    try:
        pub_path = publish('recover')
        backend = connect(1)
        backend.wait(lambda e:e.get('event') == 'graph' and all(any(t[0] == name for t in e['topics']) for name in FIXTURE.TOPICS.values()))
        backend.wait(lambda e:e.get('event') == 'graph')
        assert not backend.sequence, 'Discovery activated subscriptions'
        record(f'strict session{SESSION_VERSION} cloud2 capabilities and discovery remain inactive')
        backend.scene(1,'robot')
        backend.scene(2,'spatial_scan',FIXTURE.TOPICS['scan_a'])
        backend.scene(3,'spatial_scan',FIXTURE.TOPICS['scan_b'])
        backend.scene(4,'point_cloud')
        backend.peer(5,FIXTURE.TOPICS['status'])
        missing = backend.wait(lambda e:e.get('event') == 'point_cloud_observation' and e.get('subscription') == 4 and e['sample']['tf']['status'] == 'missing')
        assert missing['sample']['source_points'] == 4
        before = peers_progress(backend)
        record('Robot plus two scans cloud ordinary share TF; missing cloud transform isolates only cloud')
        control = pub_path/'mode.tmp';control.write_text('ready\n');control.replace(pub_path/'mode.txt')
        ready = backend.ready_sample(4,'point_cloud_observation');check_cloud(ready)
        (folder/'cloud-recovered.json').write_text(json.dumps(ready,indent=2)+'\n')
        chain = ROBOT.validate_chain(backend.robot_ready()['sample'])
        peers_progress(backend,before)
        record('retained cloud transform recovery and analytic noncoplanar XYZ acquisition transforms',details=chain)
        backend.scene(6,'spatial_scan',FIXTURE.TOPICS['scan_a'])
        backend.ready_sample(6,'spatial_observation')
        reject(backend,7,configuration('point_cloud'))
        backend.peer(8,FIXTURE.TOPICS['status'])  # An additional ordinary reader fits the current resource limits.
        ordinary = dict(topic=FIXTURE.TOPICS['status'],message_type='std_msgs/msg/Float64',output='typed',best_effort=False,transient_local=False,spatial=None,robot=None)
        backend.start_configuration(9,ordinary)
        backend.sample(9,'observation')
        mismatch = configuration('point_cloud');mismatch['spatial']['fixed_frame'] = 'odom'
        reject(backend,10,mismatch)
        record('four-sensor ceiling rejects extra sensor while current handle capacity admits ordinary peers')
        backend.stop_robot(9)
        backend.stop_robot(8)
        # A real race is conditional: admission is valid only after exact old cleanup.
        backend.request += 1
        stop_request = backend.request
        backend.send(dict(command='stop',generation=backend.generation,request=stop_request,subscription=6))
        start_request = backend.scene(11,'point_cloud',ack=False)
        result = backend.wait(lambda e:(e.get('event') == 'started' and e.get('subscription') == 11)
                              or (e.get('event') == 'request_error' and e.get('request') == start_request))
        history = list(backend.history)
        stopped = [i for i,e in enumerate(history) if e.get('event') == 'stopped' and e.get('subscription') == 6]
        if result['event'] == 'started':
            started = next(i for i,e in enumerate(history) if e.get('event') == 'started' and e.get('subscription') == 11)
            assert stopped and stopped[0] < started, 'Replacement admitted while stopping reservation remained'
            backend.stop_robot(11)
            record('stopping reservation race','passed',{'observed':'old cleanup completed before new admission; live stopping rejection window not observed'})
        else:
            assert result['no_handle'] is True
            if not stopped:
                backend.wait(lambda e:e.get('event') == 'stopped' and e.get('subscription') == 6 and e.get('request') == stop_request)
            record('stopping reservation race','passed',{'observed':'replacement rejected until stopping owner released reservation'})
        peers_progress(backend)
        backend.stop_robot(4)
        reject(backend,19,mismatch)
        unverified = configuration('point_cloud')
        unverified['spatial']['clock_policy'] = 'unknown'
        reject(backend,20,unverified)
        peers_progress(backend)
        record('sensor stop final Drops precedes cleanup and shared Robot scan peers continue')
        record('incompatible fixed frame and unverified clock reject before opening an owner with capacity available')
        sub = 21
        for mode,big_endian in [('cloud-empty',False),('cloud-skipped',False),('nominal',True),
                               ('cloud-rgb',False),('cloud-rgba',True),('cloud-attributes-skipped',False),
                               ('cloud-unsupported',False),('cloud-malformed',False),('cloud-pressure',False)]:
            publish(mode,big_endian)
            backend.scene(sub,'point_cloud',ack=mode != 'cloud-malformed')
            if mode == 'cloud-malformed':
                failure = backend.wait(lambda e:e.get('subscription') == sub and e.get('event') in ('stream_error','request_error'))
                assert sub in backend.retired, 'Scoped failure lacked final loss snapshot'
                if failure['event'] == 'request_error':
                    assert failure['no_handle'] is True
                peers_progress(backend)
                record('malformed cloud fails its owner with final loss snapshot and healthy shared peers',details=failure)
            else:
                sample = backend.ready_sample(sub,'point_cloud_observation');check_cloud(sample,mode)
                if mode == 'cloud-pressure':
                    peer_before = backend.sample(5,'observation')
                    backend.pause()
                    try:
                        time.sleep(2)
                    finally:
                        backend.resume()
                    loss = backend.wait(lambda e:e.get('subscription') == sub and e.get('event') == 'point_cloud_observation' and e['backend_drops'] > 0,seconds=20)
                    observed = loss['backend_drops']
                    peers_progress(backend,peer_before)
                    backend.stop_robot(sub)
                    assert backend.drops[sub] >= observed
                    record('bounded pressure reports backend queue loss and final Drops; ordinary and TF peers progress',details={'observed_backend_drops':observed,'final_backend_drops':backend.drops[sub],'client_preview_coalesced':backend.client_coalesced,'scope':'Queue loss only; no DDS loss, RSS or FPS claim'})
                else:
                    backend.stop_robot(sub)
                    peers_progress(backend)
                    record('big endian reordered mixed XYZ and padded rows' if big_endian else mode+' explicit replacement preserves shared TF peers')
            sub += 1
        backend.close()
        record('EOF joins all active shared TF sensor and ordinary owners')
        backend = connect(2)
        backend.wait(lambda e:e.get('event') == 'graph');backend.wait(lambda e:e.get('event') == 'graph')
        assert not backend.sequence, 'Fresh backend replayed previous activation'
        backend.scene(1,'robot')
        backend.robot_ready()
        backend.close()
        record('fresh generation inactive until explicit Start and retained TF late join')
    except Exception as error:
        (folder/'failure.txt').write_text(traceback.format_exc())
        record('sensor session interrupted','failed',{'error':repr(error)})
        record('subsequent sensor cases','not tested',{'reason':'Earlier failure interrupted bounded matrix; no later pass inferred'})
    finally:
        errors = []
        for backend in backends:
            try:
                backend.close()
            except Exception as error:
                errors.append(repr(error))
        try:
            stop_publisher()
        except Exception as error:
            errors.append(repr(error))
        for process in owned:
            try:
                os.kill(process['pid'],0)
            except ProcessLookupError:
                pass
            else:
                errors.append(f'Owned PID remains: {process["pid"]}')
        for log in publisher_logs:
            log.close()
        record('every recorded backend and fixture PID reaped','failed' if errors else 'passed',{'owned':owned,'errors':errors})
    if any(row['outcome'] != 'passed' for row in rows):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
