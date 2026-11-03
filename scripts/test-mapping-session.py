"""Bounded session10/mapping1 real-backend localhost synthetic ROS acceptance.

Run matching --rmw pairs separately through scripts/dev project Python. No UI,
map-rendering, FPS, LAN, packaging, navigation commands or hardware acceptance.
"""
import argparse
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys
import time
import traceback

ROOT = Path(__file__).resolve().parents[1]


def load(name,path):
    spec=importlib.util.spec_from_file_location(name,ROOT/path)
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    return module


SENSOR=load('mapping_sensor_acceptance','scripts/test-sensor-session.py')
FIXTURE=load('mapping_fixture_acceptance','scripts/mapping-scene-workflow.py')
# This helper is privately imported for this harness: extend its bounded reader
# classification for the paired session10 event, without changing source files.
SENSOR.SAMPLES.add('mapping_observation')
EXPECTED={**SENSOR.EXPECTED,'mapping_contract_version':1,
          'max_map_dimension':256,'max_map_cells':65536,'max_path_poses':1024,
          'max_mapping_sample_bytes':524288,'max_map_layers':3,'max_pose_layers':1,'max_path_layers':1}
TYPES={'occupancy_grid':'nav_msgs/msg/OccupancyGrid','pose':'geometry_msgs/msg/PoseStamped','path':'nav_msgs/msg/Path'}


def configuration(kind,retained=True,fixed='map',absent_tf=False):
    tf=dict(fixed_frame=fixed,clock_policy='verified_system',dynamic_tf_topic=FIXTURE.TOPICS['dynamic'],
            dynamic_tf_best_effort=False,static_tf_topic=FIXTURE.TOPICS['static'])
    if absent_tf:
        tf.update(dynamic_tf_topic='/mapping_scene/absent_tf',static_tf_topic='/mapping_scene/absent_static')
    if kind=='robot':
        return dict(topic=FIXTURE.TOPICS['dynamic'],message_type='tf2_msgs/msg/TFMessage',output=kind,
                    best_effort=False,transient_local=False,spatial=None,robot={**tf,'frames':list(FIXTURE.ROBOT.LINKS)})
    mapping=kind in TYPES
    topic_key='map' if kind=='occupancy_grid' else 'scan' if kind=='spatial_scan' else 'cloud' if kind=='point_cloud' else kind
    return dict(topic=FIXTURE.TOPICS[topic_key],message_type=TYPES[kind] if mapping else 'sensor_msgs/msg/LaserScan' if kind=='spatial_scan' else 'sensor_msgs/msg/PointCloud2',
                output=kind,best_effort=not mapping,transient_local=kind=='occupancy_grid' and retained,spatial=tf,robot=None)


class Backend(SENSOR.Backend):
    def scene(self,sub,kind,ack=True,**options):
        return self.start_configuration(sub,configuration(kind,**options),ack)

    def mapping(self,sub,status='ready'):
        return self.wait(lambda e:e.get('subscription')==sub and e.get('event')=='mapping_observation' and e['sample']['tf']['status']==status)


def rotate(q,p):
    x,y,z,w=q
    return [(1-2*(y*y+z*z))*p[0]+2*(x*y-z*w)*p[1]+2*(x*z+y*w)*p[2],
            2*(x*y+z*w)*p[0]+(1-2*(x*x+z*z))*p[1]+2*(y*z-x*w)*p[2],
            2*(x*z-y*w)*p[0]+2*(y*z+x*w)*p[1]+(1-2*(x*x+y*y))*p[2]]


def apply(pose,p):
    return [a+b for a,b in zip(pose[:3],rotate(pose[3:],p))]


def near(actual,expected,tolerance=1e-6):
    assert len(actual)==len(expected) and all(math.isfinite(v) for v in actual)
    assert max((abs(a-b) for a,b in zip(actual,expected)),default=0)<tolerance,(actual,expected)


def check_mapping(event,kind,mode='nominal',identity=False):
    sample=event['sample'];geometry=sample['geometry']
    assert sample['contract_version']==1 and sample['topic']==FIXTURE.TOPICS['map' if kind=='occupancy_grid' else kind]
    assert sample['message_type']==TYPES[kind] and sample['source_frame']=='mapping_source'
    assert type(sample['acquisition_stamp_ns']) is int and 0<sample['acquisition_stamp_ns']<=2147483647999999999
    assert sample['fixed_frame']==('mapping_source' if identity else 'map')
    tf=sample['tf'];assert tf['status']=='ready'
    expected_tf=[0,0,0,0,0,0,1] if identity else [1,2,3,0,0,math.sqrt(.5),math.sqrt(.5)]
    near(tf['components'],expected_tf)
    assert set(sample['counters'])=={'messages_received','pending_replaced','lookup_timeouts','tf_dynamic_batches','tf_static_batches','tf_accepted','tf_rejected','duplicate_statics','native_errors'}
    assert geometry['kind']==kind
    fixture=FIXTURE.geometry(mode)
    if kind=='occupancy_grid':
        grid=fixture['map']
        assert (geometry['width'],geometry['height'])==(grid['width'],grid['height'])
        assert geometry['cells']==grid['cells'] and geometry['map_load_time_ns']==0
        assert abs(geometry['resolution']-grid['resolution'])<1e-6
        yaw=math.radians(grid['origin_yaw_degrees'])
        near(geometry['origin'],[*grid['origin_translation'],0,0,math.sin(yaw/2),math.cos(yaw/2)])
        if mode=='nominal' and not identity:
            resolution=geometry['resolution'];origin=geometry['origin']
            centers=[apply(tf['components'],apply(origin,[(col+.5)*resolution,.5*resolution,0])) for col in range(3)]
            for actual,wanted in zip(centers,FIXTURE.LAYOUT['expected_first_row_cell_centers']):near(actual,wanted)
            near(apply(tf['components'],apply(origin,[.5*resolution,1.5*resolution,0])),FIXTURE.LAYOUT['expected_second_row_first_cell_center'])
    elif kind=='pose':
        near(geometry['components'][:3],fixture['pose']['position'])
        if not identity:
            near(apply(tf['components'],geometry['components'][:3]),FIXTURE.LAYOUT['expected_pose_fixed'])
            # Compose source and TF orientations, checking the transformed X axis.
            near(rotate(tf['components'][3:],rotate(geometry['components'][3:],[1,0,0])),[-1,0,0])
    else:
        assert len(geometry['poses'])==len(fixture['path'])
        for index,(actual,wanted) in enumerate(zip(geometry['poses'],fixture['path'])):
            assert actual['frame_id'] in ('','mapping_source') and actual['stamp_ns']==0
            near(actual['components'],[*wanted['position'],0,0,0,1])
            if mode=='nominal' and not identity:near(apply(tf['components'],actual['components'][:3]),FIXTURE.LAYOUT['expected_path_fixed'][index])
    return sample


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rmw',choices=FIXTURE.RMWS,required=True);parser.add_argument('--label',required=True)
    args=parser.parse_args()
    if not 1<=len(args.label)<=96 or not args.label.isascii() or not args.label.replace('-','').isalnum():parser.error('Use unused ASCII alphanumeric/hyphen label')
    folder=ROOT/'artifacts/feasibility/evidence'/args.label;report=ROOT/'artifacts/feasibility'/(args.label+'.json')
    if folder.exists() or report.exists():parser.error('Preserve earlier evidence: choose unused paths')
    folder=FIXTURE.checked_environment(args.rmw,folder);FIXTURE.HELPERS['inventory']()
    context=json.loads((folder/'context.json').read_text());context['middleware_pair']=[args.rmw,args.rmw]
    context['versions']['rust']=subprocess.run(['rustc','--version'],check=True,capture_output=True,text=True,timeout=10).stdout.strip()
    sources=['scripts/test-mapping-session.py','scripts/mapping-scene-workflow.py','scripts/test-sensor-session.py','scripts/test-robot-session.py','scripts/test-image-session.py',
             'scripts/sensor-scene-workflow.py','scripts/robot-ros-workflow.py','scripts/ui-ros-workflow.py','fixtures/mapping_scene/alignment.json',
             'crates/protocol/src/mapping.rs','crates/protocol/src/session.rs','crates/backend/src/mapping_codec.rs','crates/backend/src/mapping_worker.rs',
             'crates/backend/src/scene_worker.rs','crates/backend/src/observation.rs','crates/backend/src/native_topic.rs','crates/backend/native/tf_shim.cpp','target/debug/studio-backend']
    context['source_sha256']={p:hashlib.sha256((ROOT/p).read_bytes()).hexdigest() for p in sources}
    context['environment']={key:os.environ.get(key) for key in ('ROS_DISTRO','ROS_DOMAIN_ID','ROS_LOCALHOST_ONLY','ROS_AUTOMATIC_DISCOVERY_RANGE','RMW_IMPLEMENTATION',
        'ROS_STATIC_PEERS','ROS_DISCOVERY_SERVER','ROS_SUPER_CLIENT','CYCLONEDDS_URI','FASTDDS_DEFAULT_PROFILES_FILE','FASTRTPS_DEFAULT_PROFILES_FILE',
        'FASTDDS_ENVIRONMENT_FILE','SKIP_DEFAULT_XML','AMENT_PREFIX_PATH','DYLD_LIBRARY_PATH')}
    command='scripts/dev .pixi/envs/default/bin/python '+shlex.join(sys.argv)
    rows,owned,backends,logs=[],[],[],[];publisher=None;publisher_index=0;deadline=time.monotonic()+300

    def record(case,outcome='passed',details=None):
        rows.append({**context,'test':case,'outcome':outcome,'details':details,'command':command,'evidence':str(folder.relative_to(ROOT)),
                     'local_only':True,'scope':'Actual Rust backend with localhost synthetic ROS; no native window, renderer, FPS, LAN, packaging, hardware or command acceptance.'})
        report.write_text(json.dumps(rows,indent=2)+'\n');print(outcome,case,flush=True)

    def save_owned():(folder/'owned-processes.json').write_text(json.dumps(owned,indent=2)+'\n')

    def stop_publisher():
        nonlocal publisher
        if publisher is None:return
        child=publisher
        if child.poll() is None:child.send_signal(signal.SIGINT)
        try:child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill();child.wait(timeout=3);raise AssertionError('Owned fixture required forced kill')
        publisher=None
        assert child.returncode==0,f'Fixture exit{child.returncode}'
        state=json.loads((folder/f'publisher-{publisher_index}'/'publisher-state.json').read_text())
        assert state['event']=='stopped' and state['pid']==child.pid,state

    def publish(mode):
        nonlocal publisher,publisher_index
        stop_publisher();publisher_index+=1;path=folder/f'publisher-{publisher_index}'
        argv=[sys.executable,str(ROOT/'scripts/mapping-scene-workflow.py'),'--rmw',args.rmw,'--evidence',str(path),'--mode',mode,'--seconds','360']
        log=(folder/f'publisher-{publisher_index}.log').open('wb');logs.append(log)
        publisher=subprocess.Popen(argv,stdout=log,stderr=log);owned.append({'pid':publisher.pid,'argv':argv,'evidence':str(path.relative_to(ROOT))});save_owned()
        end=min(deadline,time.monotonic()+20)
        while time.monotonic()<end:
            assert publisher.poll() is None,f'Fixture exited{publisher.returncode}'
            state_path=path/'publisher-state.json'
            if state_path.exists():
                try:state=json.loads(state_path.read_text())
                except json.JSONDecodeError:
                    time.sleep(.02)
                    continue
                # A started snapshot precedes the first publication. In
                # particular a Volatile late-join test must start after the
                # one-shot retained map has actually been published.
                if state['event']=='publishing' and state['publisher_calls']['map']>=1:
                    return path
            time.sleep(.02)
        raise AssertionError('Fixture startup deadline exceeded')

    def publisher_state(path):
        end=min(deadline,time.monotonic()+2)
        while time.monotonic()<end:
            try:return json.loads((path/'publisher-state.json').read_text())
            except json.JSONDecodeError:time.sleep(.01)
        raise AssertionError('Fixture state remained incomplete')

    def connect(index):
        backend=Backend(folder,index,90+index,deadline);backends.append(backend)
        owned.append({'pid':backend.process.pid,'argv':[str(ROOT/'target/debug/studio-backend'),'--observe-session']});save_owned()
        ready=backend.wait(lambda e:e.get('event')=='ready')
        assert set(ready)=={'event','distro','rmw',*EXPECTED},ready
        assert all(ready[k]==v for k,v in EXPECTED.items()) and ready['rmw']==args.rmw and ready['distro']=='jazzy',ready
        return backend

    def reject(backend,sub,cfg):
        request=backend.start_configuration(sub,cfg,ack=False)
        result=backend.wait(lambda e:e.get('event')=='request_error' and e.get('request')==request)
        assert result['no_handle'] is True,result

    def activate(backend,sub,kind,**options):
        backend.scene(sub,kind,**options)
        return backend.mapping(sub)

    def peer_progress(backend,old=None):
        robot=backend.robot_ready();assert SENSOR.ROBOT.robot_state(robot['sample'],True)
        scan=backend.ready_sample(2,'spatial_observation');SENSOR.check_scan(scan,'sensor_scan_a',[1,2,3],math.pi/2)
        cloud=backend.ready_sample(4,'point_cloud_observation')['sample']
        SENSOR.check_tf(cloud,[1,2,3],math.pi/2);assert SENSOR.decode_points(cloud['points'])==FIXTURE.SENSOR.LAYOUT['points']
        peer=backend.sample(9,'observation')
        if old is not None:peer=backend.wait(lambda e:e.get('event')=='observation' and e.get('subscription')==9 and e['sequence']>old['sequence'])
        assert SENSOR.ROBOT.peer_value(peer)>=0
        return peer

    def atomic_ready(path):
        temporary=path/'mode.tmp';temporary.write_text('ready\n');temporary.replace(path/'mode.txt')

    try:
        path=publish('recover');backend=connect(1)
        backend.wait(lambda e:e.get('event')=='graph' and all(any(t[0]==name for t in e['topics']) for name in FIXTURE.TOPICS.values()))
        backend.wait(lambda e:e.get('event')=='graph');assert not backend.sequence
        record('session10 exact paired capabilities and discovery without activation')
        for sub,kind in [(1,'robot'),(2,'spatial_scan'),(3,'spatial_scan'),(4,'point_cloud'),(5,'occupancy_grid'),(6,'pose'),(7,'path')]:backend.scene(sub,kind)
        for sub in (5,6,7):backend.mapping(sub,'missing')
        ordinary=dict(topic=FIXTURE.TOPICS['status'],message_type='std_msgs/msg/Float64',output='typed',best_effort=False,transient_local=False,spatial=None,robot=None)
        reject(backend,8,ordinary)
        backend.stop_robot(3);backend.peer(9,FIXTURE.TOPICS['status']);peer_progress(backend)
        record('Robot two scan owners cloud map pose path fit exact eight handles; ordinary rejected until one scan stops')
        record('mapping TF absence isolates map pose path while Robot scan cloud ordinary progress')
        atomic_ready(path)
        for sub,kind in [(5,'occupancy_grid'),(6,'pose'),(7,'path')]:check_mapping(backend.mapping(sub),kind)
        peer_progress(backend)
        record('explicit TF recovery preserves map cell origin rotation pose orientation path order and header acquisition metadata')
        for sub in (5,6,7):backend.stop_robot(sub)
        peer_progress(backend)
        record('independent mapping Stops emit final Drops before cleanup while shared TF peers continue')
        sub=10
        for mode,kind in [('map-malformed','occupancy_grid'),('map-over-limit','occupancy_grid'),('pose-malformed','pose'),('path-malformed','path'),('path-over-limit','path')]:
            publish(mode);backend.scene(sub,kind,ack=False)
            failure=backend.wait(lambda e:e.get('subscription')==sub and e.get('event') in ('stream_error','request_error'))
            assert sub in backend.retired,'Malformed mapping failure omitted final loss snapshot'
            if failure['event']=='request_error':assert failure['no_handle'] is True
            peer_progress(backend);record(mode+' fails only its owner and releases its reservation',details=failure);sub+=1
        for mode in ('large','empty'):
            large_path=publish(mode)
            if mode=='large':
                map_sub,path_sub=sub,sub+1
                check_mapping(activate(backend,map_sub,'occupancy_grid'),'occupancy_grid',mode)
                check_mapping(activate(backend,path_sub,'path'),'path',mode)
                peer_before=backend.sample(9,'observation')
                backend.pause()
                try:
                    before_pressure=publisher_state(large_path)
                    pause_start=time.monotonic()
                    pressure_end=min(deadline,pause_start+18)
                    while True:
                        assert publisher.poll() is None,'Fixture stopped during pressure'
                        after_pressure=publisher_state(large_path)
                        elapsed=time.monotonic()-pause_start
                        # Snapshots appear every25 calls and can lag by24.
                        # A delta>=50 proves at least26 additional source
                        # publications during the pause, beyond16 queue slots.
                        published=min(after_pressure['publisher_calls'][key]-before_pressure['publisher_calls'][key] for key in ('map','path'))
                        if elapsed>=6 and published>=50:break
                        assert time.monotonic()<pressure_end,'Bounded pressure did not establish sufficient source publications'
                        time.sleep(.05)
                finally:backend.resume()
                loss=backend.wait(lambda e:e.get('event')=='mapping_observation' and e.get('subscription')==map_sub and e['backend_drops']>0,seconds=20)
                peer_progress(backend,peer_before)
                backend.stop_robot(map_sub);backend.stop_robot(path_sub)
                assert backend.drops[map_sub]>=loss['backend_drops']
                record('maximum map and path coexist with bounded queue loss and final Drops',details={'observed_backend_drops':loss['backend_drops'],'final_backend_drops':backend.drops[map_sub],
                    'pause_seconds':elapsed,'source_calls_before':before_pressure['publisher_calls'],'source_calls_after':after_pressure['publisher_calls'],
                    'minimum_additional_map_and_path_source_calls':published-24,
                    'scope':'Source publication count establishes pressure opportunity; actual backend_drops independently proves queue loss. No DDS-loss, memory, renderer or FPS claim'})
                sub+=2
                continue
            for kind in ('occupancy_grid','path'):
                check_mapping(activate(backend,sub,kind),kind,mode);backend.stop_robot(sub);sub+=1
            peer_progress(backend);record(mode+' map and path cross actual compact native boundary with complete geometry')
        pub_path=publish('map-duplicate')
        check_mapping(activate(backend,sub,'occupancy_grid'),'occupancy_grid')
        initial_sequence=backend.sequence[sub]
        for _ in range(3):backend.wait(lambda e:e.get('event')=='graph')
        assert backend.sequence[sub]==initial_sequence==1,'Identical retransmission replayed geometry'
        state=json.loads((pub_path/'publisher-state.json').read_text())
        # Raw publication count may update only in periodic snapshots; inspect
        # elapsed wall time plus source recipe, never call it DDS delivery count.
        assert state['mode']=='map-duplicate'
        backend.stop_robot(sub);sub+=1;peer_progress(backend)
        record('identical retained map stamp source geometry retransmissions do not replay observations')
        pub_path=publish('map-changed-stamp')
        check_mapping(activate(backend,sub,'occupancy_grid'),'occupancy_grid');atomic_ready(pub_path)
        failure=backend.wait(lambda e:e.get('subscription')==sub and e.get('event')=='stream_error')
        assert sub in backend.retired;peer_progress(backend);sub+=1
        record('changed equal-stamp map fails only its owner after explicit fixture control',details=failure)
        pub_path=publish('map-retained')
        backend.scene(sub,'occupancy_grid',retained=False)
        for _ in range(2):backend.wait(lambda e:e.get('event')=='graph')
        assert sub not in backend.sequence,'Volatile late join received one-shot retained map'
        backend.stop_robot(sub);sub+=1
        retained=activate(backend,sub,'occupancy_grid',retained=True);check_mapping(retained,'occupancy_grid')
        stamp=retained['sample']['acquisition_stamp_ns'];backend.stop_robot(sub);sub+=1
        again=activate(backend,sub,'occupancy_grid',retained=True);check_mapping(again,'occupancy_grid')
        assert again['sample']['acquisition_stamp_ns']==stamp,'Retained map was retimestamped at receipt'
        backend.stop_robot(sub);sub+=1;peer_progress(backend)
        record('explicit retained QoS correction and replacement preserve original positive header time')
        publish('path-updates');backend.scene(sub,'path')
        first=backend.mapping(sub)
        changed=backend.wait(lambda e:e.get('event')=='mapping_observation' and e.get('subscription')==sub and e['sample']['tf']['status']=='ready' and e['sample']['geometry']!=first['sample']['geometry'])
        assert changed['sample']['acquisition_stamp_ns']>first['sample']['acquisition_stamp_ns']
        backend.stop_robot(sub);peer_progress(backend)
        record('changing path remains ordered bounded geometry at each positive Path header time')
        backend.close();record('EOF joins remaining shared TF Robot sensor and ordinary owners')
        publish('map-historical');backend=connect(2)
        backend.wait(lambda e:e.get('event')=='graph');backend.wait(lambda e:e.get('event')=='graph');assert not backend.sequence
        backend.peer(1,FIXTURE.TOPICS['status']);backend.scene(2,'occupancy_grid')
        historical=backend.wait(lambda e:e.get('event')=='mapping_observation' and e.get('subscription')==2 and e['sample']['tf']['status']=='extrapolated')
        assert historical['sample']['acquisition_stamp_ns']>0
        backend.sample(1,'observation');backend.stop_robot(2);backend.close()
        record('fresh generation inactive and historical retained geometry remains extrapolated without latest fallback')
        publish('map-retained');backend=connect(3)
        backend.wait(lambda e:e.get('event')=='graph')
        identity=activate(backend,1,'occupancy_grid',fixed='mapping_source',absent_tf=True)
        check_mapping(identity,'occupancy_grid',identity=True)
        assert identity['sample']['counters']['tf_accepted']==0
        backend.stop_robot(1);backend.close()
        record('map-only identity works without discovered configured TF topics or admitted TF frames')
    except Exception as error:
        (folder/'failure.txt').write_text(traceback.format_exc());record('mapping session interrupted','failed',{'error':repr(error)})
        record('subsequent mapping cases','not tested',{'reason':'Earlier failed case interrupted bounded matrix; no subsequent pass inferred'})
    finally:
        errors=[]
        for backend in backends:
            try:backend.close()
            except Exception as error:errors.append(repr(error))
        try:stop_publisher()
        except Exception as error:errors.append(repr(error))
        for process in owned:
            try:os.kill(process['pid'],0)
            except ProcessLookupError:pass
            else:errors.append(f'Owned PID remains:{process["pid"]}')
        for log in logs:log.close()
        record('all recorded backend and publisher owners reaped','failed' if errors else 'passed',{'owned':owned,'errors':errors})
    if any(row['outcome']!='passed' for row in rows):raise SystemExit(1)


if __name__=='__main__':main()
