"""Guarded real-ROS backend-only session7 Spatial acceptance; no native UI.

Fixed bounded reader/captures and deadlines; explicit starts own three handles.
Synthetic data only, localhost/domain187, matching explicit middleware required.
"""
import argparse
import hashlib
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
NORMAL = load('image_normal', 'scripts/test-image-session.py')
FIXTURE = load('spatial_fixture', 'scripts/ui-spatial-workflow.py')
MEMBER = NORMAL.member


class Backend(NORMAL.Backend):
    def read(self):
        try:
            while True:
                line = self.process.stdout.readline(NORMAL.MAX_LINE + 1)
                if not line:
                    return
                assert len(line) <= NORMAL.MAX_LINE and line.endswith(b'\n')
                event = json.loads(line)
                sample = event.get('event') in ('observation', 'image_observation', 'spatial_observation')
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

    def __init__(self, folder, index, generation):
        self.reported_drops = {}
        self.spatial_counts = {}
        self.retired = set()
        super().__init__(folder, index, generation)

    def wait(self, predicate, seconds=12):
        def observe(event):
            kind, sub = event.get('event'), event.get('subscription')
            if kind in ('observation', 'image_observation', 'spatial_observation', 'drops'):
                assert event['generation'] == self.generation
                assert type(sub) is int and 1 <= sub <= 32
                assert event['backend_drops'] >= self.reported_drops.get(sub, 0)
                self.reported_drops[sub] = event['backend_drops']
                if kind != 'drops':
                    assert sub not in self.retired, 'Data followed final native-owner loss snapshot'
                else:
                    self.retired.add(sub)
                if kind == 'spatial_observation':
                    assert event['sequence'] > self.sequence.get(sub, 0)
                    self.sequence[sub] = event['sequence']
                    counts = event['sample']['counters']
                elif kind == 'drops':
                    counts = event.get('spatial_counters')
                else:
                    counts = None
                if counts is not None:
                    assert len(counts) == 9
                    for key, count in counts.items():
                        assert type(count) is int and count >= self.spatial_counts.get(sub, {}).get(key, 0)
                    self.spatial_counts[sub] = counts
            return predicate(event)
        return super().wait(observe, seconds)

    def peer(self, sub=1):
        self.request += 1
        self.send({'command': 'start', 'generation': self.generation, 'request': self.request, 'subscription': sub,
            'configuration': {'topic': FIXTURE.PREFIX + 'status', 'message_type': 'std_msgs/msg/String',
                              'best_effort': False, 'transient_local': False, 'output': 'typed', 'spatial': None}})
        self.wait(lambda e: e.get('event') == 'started' and e['subscription'] == sub and e['request'] == self.request)
        return self.sample(sub, 'observation')

    def scene(self, sub, scan='scan', dynamic='tf', static='tf_static', clock='verified_system', ack=True):
        self.request += 1
        self.send({'command': 'start', 'generation': self.generation, 'request': self.request, 'subscription': sub,
            'configuration': {'topic': FIXTURE.PREFIX + scan, 'message_type': 'sensor_msgs/msg/LaserScan',
                'best_effort': True, 'transient_local': False, 'output': 'spatial_scan',
                'spatial': {'fixed_frame': FIXTURE.FIXED, 'clock_policy': clock,
                    'dynamic_tf_topic': FIXTURE.PREFIX + dynamic, 'dynamic_tf_best_effort': False,
                    'static_tf_topic': FIXTURE.PREFIX + static}}})
        if ack:
            self.wait(lambda e: e.get('event') == 'started' and e['subscription'] == sub and e['request'] == self.request)
        return self.request

    def spatial(self, sub, status='ready', seconds=12):
        event = self.wait(lambda e: e.get('subscription') == sub and (
            e.get('event') in ('stream_error', 'request_error') or
            (e.get('event') == 'spatial_observation' and (status is None or e['sample']['tf']['status'] == status))), seconds)
        assert event['event'] == 'spatial_observation', event
        return event

    def stop_scene(self, sub):
        count = self.stop(sub)
        assert self.spatial_counts.get(sub) is not None
        return {'backend_drops': count, 'spatial_counters': self.spatial_counts[sub]}


def finite(value):
    assert value['kind'] == 'float' and value['value']['kind'] == 'finite'
    result = float(value['value']['value'])
    assert math.isfinite(result)
    return result


def validate_projection(event, same_frame=False):
    sample = event['sample']
    assert sample['contract_version'] == 1 and sample['fixed_frame'] == FIXTURE.FIXED
    scan = sample['scan']
    assert scan['contract_version'] == 2 and scan['message_type'] == 'sensor_msgs/msg/LaserScan'
    header = MEMBER(scan['value'], 'header')
    stamp = MEMBER(header, 'stamp')
    stamp_ns = int(MEMBER(stamp, 'sec')['value']) * 1000000000 + int(MEMBER(stamp, 'nanosec')['value'])
    assert stamp_ns == sample['acquisition_stamp_ns'] > 0
    assert MEMBER(header, 'frame_id')['value'] == sample['source_frame'] == (FIXTURE.FIXED if same_frame else FIXTURE.LASER)
    assert sample['tf']['status'] == 'ready'
    components = sample['tf']['components']
    assert len(components) == 7 and all(math.isfinite(v) for v in components)
    assert abs(sum(v*v for v in components[3:]) - 1) < 1e-6
    if same_frame:
        assert max(abs(a-b) for a,b in zip(components, [0,0,0,0,0,0,1])) < 1e-12
    ranges = MEMBER(scan['value'], 'ranges')
    assert ranges['original_items'] == len(ranges['items']) == 180
    angle_min = finite(MEMBER(scan['value'], 'angle_min'))
    increment = finite(MEMBER(scan['value'], 'angle_increment'))
    qx,qy,qz,qw = components[3:]
    worst = 0.
    for index, ray in enumerate(ranges['items']):
        r = finite(ray)
        angle = angle_min + index * increment
        px,py,pz = r*math.cos(angle),r*math.sin(angle),0.
        tx,ty,tz = 2*(qy*pz-qz*py),2*(qz*px-qx*pz),2*(qx*py-qy*px)
        x = px+qw*tx+qy*tz-qz*ty+components[0]
        y = py+qw*ty+qz*tx-qx*tz+components[1]
        worst = max(worst,abs(max(abs(x),abs(y))-FIXTURE.MODEL.ROOM_HALF_WIDTH))
    assert worst < .03, ('Room alignment error',worst)
    return {'acquisition_stamp_ns':stamp_ns,'maximum_wall_error_metres':worst,'components':components}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', required=True)
    args = parser.parse_args()
    if not args.label.startswith('round5-') or not args.label.replace('-', '').isalnum():
        parser.error('Use an unused descriptive round5- label')
    folder = FIXTURE.HELPERS['checked_environment']()
    report = ROOT/'artifacts/feasibility'/f'{args.label}.json'
    if report.exists() or any(folder.iterdir()):
        raise SystemExit('Use unused report/evidence paths; preserve previous outcomes')
    FIXTURE.HELPERS['inventory']()
    context = json.loads((folder/'context.json').read_text())
    context['source_sha256']={p:hashlib.sha256((ROOT/p).read_bytes()).hexdigest() for p in [
        'scripts/ui-spatial-workflow.py','scripts/test-spatial-session.py','scripts/test-image-session.py',
        'fixtures/simple_sim/model.py','crates/protocol/src/session.rs',
        'crates/protocol/src/spatial.rs','crates/backend/src/observation.rs','crates/backend/src/spatial_worker.rs']}
    context['middleware_pair']=f"{os.environ['RMW_IMPLEMENTATION']}/{os.environ['RMW_IMPLEMENTATION']}"
    rows=[]
    def record(case,outcome='passed',details=None):
        rows.append({**context,'case':case,'outcome':outcome,'details':details,
            'command':'scripts/dev .pixi/envs/default/bin/python '+shlex.join(sys.argv),
            'environment':{k:os.environ.get(k) for k in ['ROS_DOMAIN_ID','ROS_LOCALHOST_ONLY','ROS_AUTOMATIC_DISCOVERY_RANGE','RMW_IMPLEMENTATION']},
            'local_only':True,'evidence':str(folder.relative_to(ROOT)),
            'limits':'Backend-only localhost synthetic scan/TF. No native rendering, Docker, DDS loss, delivered rate/RSS, LAN or other-platform acceptance inferred.'})
        report.write_text(json.dumps(rows,indent=2)+'\n')
        print(outcome,case,details if details is not None else '',flush=True)
    publisher=None; backends=[]; owned=[]; cleanup_errors=[]
    log=(folder/'publisher.stderr').open('wb')
    try:
        publisher_args=[sys.executable,str(ROOT/'scripts/ui-spatial-workflow.py'),'--publisher','--seconds','120']
        publisher=subprocess.Popen(publisher_args,stdout=log,stderr=log)
        owned.append({'pid':publisher.pid,'argv':publisher_args})
        backend=Backend(folder,1,91);backends.append(backend)
        owned.append({'pid':backend.process.pid,'argv':[str(ROOT/'target/debug/studio-backend'),'--observe-session']})
        (folder/'owned-processes.json').write_text(json.dumps(owned,indent=2)+'\n')
        ready=backend.wait(lambda e:e.get('event')=='ready')
        expected={'version':7,'spatial_contract_version':1,'max_native_handles':4,'max_spatial_scan_rays':2048,
            'max_tf_frames':256,'max_tf_edges':128,'max_tf_dynamic_inserts':256,'tf_cache_window_ns':10000000000,
            'spatial_pending_timeout_ms':1000,'spatial_retry_interval_ms':50,
            'max_command_bytes':131072,'robot_contract_version':1,'max_robot_links':128,
            'max_robot_hz':10,'max_robot_sample_bytes':524288}
        assert all(ready[k]==v for k,v in expected.items()),ready
        backend.wait(lambda e:e.get('event')=='graph' and all(any(t[0]==FIXTURE.PREFIX+name for t in e['topics']) for name in FIXTURE.TOPICS))
        backend.peer();backend.scene(2)
        projections=[validate_projection(backend.spatial(2)) for _ in range(5)]
        assert max(p['components'][0] for p in projections)-min(p['components'][0] for p in projections)>.001
        record('v7 negotiation; 20Hz dynamic TF and retained static compose moving room scan at acquisition',details={'backend_pid':backend.process.pid,'publisher_pid':publisher.pid,'projections':projections})
        req=backend.scene(3,ack=False)
        rejection=backend.wait(lambda e:e.get('event')=='request_error' and e['request']==req)
        assert rejection['no_handle'] is True
        before=backend.sample(1,'observation')['sequence']
        final=backend.stop_scene(2)
        assert final['spatial_counters']['tf_accepted']>0 and final['spatial_counters']['tf_static_batches']>0
        assert backend.sample(1,'observation')['sequence']>before
        record('three-resource reservation rejects another group; Stop final counters preserve healthy peer/PID',details=final)
        backend.scene(4,'same_frame_scan')
        identity=validate_projection(backend.spatial(4),same_frame=True);backend.stop_scene(4)
        record('same-frame identity is an explicit native tf2 result',details=identity)
        backend.scene(5,'interpolation_scan')
        interpolated=validate_projection(backend.spatial(5))
        proofs=json.loads((folder/'interpolation-latest-eight.json').read_text())
        proof=next(p for p in proofs if p['stamp_ns']==interpolated['acquisition_stamp_ns'])
        assert proof['before_stamp_ns']<proof['stamp_ns']<proof['after_stamp_ns']
        assert max(abs(a-b) for a,b in zip(interpolated['components'],proof['expected_components']))<1e-9
        (folder/'matched-interpolation-proof.json').write_text(json.dumps(proof,indent=2)+'\n')
        backend.stop_scene(5)
        record('native tf2 interpolates between identified source-time brackets, no latest fallback',details={'projection':interpolated,'brackets':proof})
        backend.scene(6,'delayed_scan','delayed_tf')
        unavailable=backend.spatial(6,status=None)
        assert unavailable['sample']['tf']['status'] in ('missing','extrapolated')
        stamp=unavailable['sample']['acquisition_stamp_ns']
        resumed=backend.spatial(6)
        assert resumed['sample']['acquisition_stamp_ns']==stamp and resumed['sequence']>unavailable['sequence']
        delayed=validate_projection(resumed);backend.stop_scene(6)
        record('200ms delayed TF retries the same pending acquisition with new sequence before deadline',details=delayed)
        backend.scene(7,'missing_scan','empty_tf')
        missing=backend.spatial(7,status='missing')
        expired=backend.spatial(7,status='lookup_timeout')
        assert expired['sample']['acquisition_stamp_ns']==missing['sample']['acquisition_stamp_ns']
        assert expired['sample']['counters']['lookup_timeouts']>=1
        backend.stop_scene(7)
        record('missing transform expires without identity/latest fallback or inferred source staleness')
        for sub,scan,dynamic,static in [(8,'malformed_scan','tf','tf_static'),(9,'out_of_order_scan','tf','tf_static'),
                (10,'scan','empty_tf','cyclic_tf_static'),(11,'scan','out_of_order_tf','tf_static'),(12,'scan','malformed_tf','tf_static')]:
            backend.scene(sub,scan,dynamic,static)
            snapshot=backend.wait(lambda e:e.get('subscription')==sub and e.get('event') in ('drops','stream_error'))
            assert snapshot['event']=='drops' and snapshot.get('spatial_counters') is not None
            failure=backend.wait(lambda e:e.get('event')=='stream_error' and e['subscription']==sub)
            assert 0<len(failure['message'].encode())<=512
            backend.sample(1,'observation')
            record(f'scoped {scan}/{dynamic}/{static} error follows final counters and preserves peer',details={'message':failure['message'],'spatial_counters':snapshot['spatial_counters']})
        for sub,clock in [(13,'unknown'),(14,'simulated')]:
            req=backend.scene(sub,clock=clock,ack=False)
            failure=backend.wait(lambda e:e.get('event')=='request_error' and e['request']==req)
            assert failure['no_handle'] is True
            backend.sample(1,'observation')
        record('unknown and simulated clock policy fail before native group activation')
        backend.scene(15)
        validate_projection(backend.spatial(15))
        backend.close()
        record('EOF cleans active three-handle Spatial group plus typed peer')
        backend=Backend(folder,2,92);backends.append(backend)
        owned.append({'pid':backend.process.pid,'argv':[str(ROOT/'target/debug/studio-backend'),'--observe-session']})
        (folder/'owned-processes.json').write_text(json.dumps(owned,indent=2)+'\n')
        backend.wait(lambda e:e.get('event')=='ready' and e['version']==7)
        until=time.monotonic()+2.2;graphs=0
        while time.monotonic()<until:
            event=backend.wait(lambda e:True,seconds=3)
            assert event['event']=='graph','Reconnect replayed configuration'
            graphs+=1
        assert graphs>=2
        backend.peer();backend.close()
        record('new generation discovers without replay and only explicit Start activates')
    except Exception as error:
        (folder/'failure.txt').write_text(traceback.format_exc())
        record('Spatial session acceptance interrupted','failed',{'error':repr(error)})
    finally:
        for backend in backends:
            try: backend.close()
            except Exception as error: cleanup_errors.append(repr(error))
        if publisher is not None:
            if publisher.poll() is None: publisher.send_signal(signal.SIGINT)
            try: publisher.wait(timeout=5)
            except subprocess.TimeoutExpired:
                cleanup_errors.append('Spatial publisher failed graceful SIGINT');publisher.kill();publisher.wait(timeout=3)
            if publisher.returncode!=0:cleanup_errors.append(f'publisher exit{publisher.returncode}')
        for process in owned:
            try:os.kill(process['pid'],0)
            except ProcessLookupError:pass
            else:cleanup_errors.append(f'owned PID remains:{process["pid"]}')
        log.close()
        record('all owned Spatial publisher/backend processes reaped','failed' if cleanup_errors else 'passed',{'owned':owned,'errors':cleanup_errors})
    if any(row['outcome']!='passed' for row in rows):raise SystemExit(1)


if __name__=='__main__':main()
