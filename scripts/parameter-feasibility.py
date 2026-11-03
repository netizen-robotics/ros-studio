"""Fixture-only ROS parameter API proof through the native dynamic service probe."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import threading
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import time

ROOT=Path(__file__).resolve().parents[1]
RAW=ROOT/'artifacts/feasibility'
RUNTIME=ROOT/'.pixi/envs/default'
PROBE=ROOT/'target/debug/studio-service-probe'
DISTRO='jazzy'
WORKSPACE=ROOT
OBSERVE_EVENTS=False
EVENT_STARTUP_BURST=False
PACKAGED_RUNTIME=None


def serve():
    if os.environ.get('ROS_DOMAIN_ID')!='187' or os.environ.get('ROS_LOCALHOST_ONLY')!='1':
        raise SystemExit('Fixture requires localhost/domain 187')
    import rclpy
    from rcl_interfaces.msg import ParameterDescriptor, SetParametersResult
    rclpy.init()
    node=rclpy.create_node('parameters',namespace='/studio_fixture')
    if os.environ.get('STUDIO_EXPECT_PARAMETER_OBSERVER')=='1':
        deadline=time.monotonic()+15
        while node.count_subscribers('/parameter_events')==0:
            if time.monotonic()>deadline:raise RuntimeError('Parameter observer did not match')
            rclpy.spin_once(node,timeout_sec=.05)
        if os.environ.get('STUDIO_EVENT_STARTUP_BURST')=='1':
            node.declare_parameter('ephemeral',123,ParameterDescriptor(dynamic_typing=True))
    for name,value in [('enabled',True),('speed',2),('scale',1.25),('label','fixture'),('axes',[1,2,3]),('tags',['one','two']),('switches',[True,False]),('gains',[1.5,2.5]),('bytes',[b'\x01',b'\xff'])]:
        node.declare_parameter(name,value)
    node.declare_parameter('serial','fixture-001',ParameterDescriptor(read_only=True))
    def validate(parameters):
        print(json.dumps({'validation':[{'name':p.name,'value':p.value} for p in parameters]}),flush=True)
        if any(p.name=='speed' and p.value<0 for p in parameters):
            return SetParametersResult(successful=False,reason='speed must be nonnegative')
        return SetParametersResult(successful=True)
    node.add_on_set_parameters_callback(validate)
    if os.environ.get('STUDIO_EXPECT_PARAMETER_OBSERVER')=='1' and os.environ.get('STUDIO_EVENT_STARTUP_BURST')!='1':
        # Separate the lifecycle marker from the depth-2 startup declaration burst.
        until=time.monotonic()+.5
        while time.monotonic()<until:rclpy.spin_once(node,timeout_sec=.05)
        node.declare_parameter('ephemeral',123,ParameterDescriptor(dynamic_typing=True))
    print('READY',flush=True)
    from rclpy.executors import ExternalShutdownException
    try:rclpy.spin(node)
    except (ExternalShutdownException,KeyboardInterrupt):pass
    finally:node.destroy_node();rclpy.try_shutdown()


def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()


def value(kind,field=None,data=None):
    v=dict(type=kind,bool_value=False,integer_value=0,double_value=0.0,string_value='',byte_array_value=[],bool_array_value=[],integer_array_value=[],double_array_value=[],string_array_value=[])
    if field:v[field]=data
    return v


def run(label):
    if platform.system()!='Darwin':raise SystemExit('This native fixture bundle path is macOS-only; other targets not tested')
    out=RAW/'evidence'/('parameters-'+label);out.mkdir(parents=True,exist_ok=False)
    bundle=out/'standard interface bundle with spaces';(bundle/'lib').mkdir(parents=True)
    index=bundle/'share/ament_index/resource_index/packages';index.mkdir(parents=True)
    interface_runtime=PACKAGED_RUNTIME or RUNTIME
    shutil.copy2(interface_runtime/'share/ament_index/resource_index/packages/rcl_interfaces',index/'rcl_interfaces')
    for lib in (interface_runtime/'lib').glob('librcl_interfaces__*.dylib'):
        if 'generator_py' not in lib.name:shutil.copy2(lib,bundle/'lib'/lib.name)
    manifest=dict(schema_version=1,ros_distro=DISTRO,os='macos',arch='aarch64' if platform.machine()=='arm64' else 'x86_64',packages=['rcl_interfaces'],files={str(p.relative_to(bundle)):sha(p) for p in bundle.rglob('*') if p.is_file()})
    (bundle/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    base=dict(os.environ,ROS_DOMAIN_ID='187',ROS_LOCALHOST_ONLY='1',ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST',RCUTILS_LOGGING_USE_STDOUT='0',SKIP_DEFAULT_XML='1',ROS_LOG_DIR=str(out/'ros-logs'))
    for key in ['ROS_STATIC_PEERS','ROS_DISCOVERY_SERVER','ROS_SUPER_CLIENT','CYCLONEDDS_URI','FASTRTPS_DEFAULT_PROFILES_FILE','FASTDDS_DEFAULT_PROFILES_FILE','FASTDDS_ENVIRONMENT_FILE']:base.pop(key,None)
    base['AMENT_PREFIX_PATH']=str(RUNTIME)
    base['DYLD_LIBRARY_PATH']=str(RUNTIME/'lib')
    (out/'dependencies.json').write_text(json.dumps([json.loads(p.read_text()) for p in sorted((RUNTIME/'conda-meta').glob('*.json'))],indent=2))
    rows=[];before=sha(PROBE);record_lock=threading.Lock()
    for rmw in ['rmw_fastrtps_cpp','rmw_cyclonedds_cpp']:
        env=dict(base,RMW_IMPLEMENTATION=rmw)
        client_env=env
        if PACKAGED_RUNTIME:
            home=out/'client-home';home.mkdir(exist_ok=True)
            client_env=dict(PATH='/usr/bin:/bin',HOME=str(home),ROS_DISTRO=DISTRO,ROS_DOMAIN_ID='187',
                            ROS_LOCALHOST_ONLY='1',ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST',
                            RMW_IMPLEMENTATION=rmw,AMENT_PREFIX_PATH=str(PACKAGED_RUNTIME),
                            ROS_LOG_DIR=str(home/'logs'),RCUTILS_LOGGING_USE_STDOUT='0')
        observer=None;observer_files=[]
        if OBSERVE_EVENTS:
            env['STUDIO_EXPECT_PARAMETER_OBSERVER']='1'
            if EVENT_STARTUP_BURST:env['STUDIO_EVENT_STARTUP_BURST']='1'
            observer_paths=[out/(rmw+'-events.stdout'),out/(rmw+'-events.stderr')]
            observer_files=[p.open('w') for p in observer_paths]
            observer_cmd=[str(PROBE.parent/'studio-backend'),'/parameter_events','rcl_interfaces/msg/ParameterEvent']
            observer=subprocess.Popen(observer_cmd,env=client_env,stdout=observer_files[0],stderr=observer_files[1])
        server_cmd=[sys.executable,str(Path(__file__).resolve()),'--server']
        server_log=out/(rmw+'-server.log')
        with server_log.open('w') as log:
            server=subprocess.Popen(server_cmd,env=env,stdout=log,stderr=log)
            try:
                deadline=time.monotonic()+10
                while 'READY' not in server_log.read_text():
                    if server.poll() is not None or time.monotonic()>deadline:raise RuntimeError(f'Server startup: {server_log}')
                    time.sleep(.05)
                def call(name,method,typ,request,expected=None,check=None,barrier=None):
                    stem=out/(rmw+'-'+name);request_path=stem.with_suffix('.request.json');request_path.write_text(json.dumps(request))
                    cmd=[str(PROBE),'--bundle',str(bundle),'--service','/studio_fixture/parameters/'+method,'--type','rcl_interfaces/srv/'+typ,'--request-file',str(request_path),'--timeout-ms','5000']
                    if barrier is not None:barrier.wait(timeout=10)
                    p=subprocess.run(cmd,env=client_env,capture_output=True,text=True,timeout=10)
                    stem.with_suffix('.stdout').write_text(p.stdout);stem.with_suffix('.stderr').write_text(p.stderr)
                    try:result=json.loads(p.stdout)
                    except json.JSONDecodeError:result={'raw':p.stdout}
                    response=result.get('response')
                    ok=p.returncode==0 and result.get('outcome')=='passed' and (response==expected if check is None else check(response))
                    with record_lock:
                        rows.append(dict(test=name,outcome='passed' if ok else 'failed',os=platform.platform(),cpu=platform.machine(),distro=DISTRO,middleware_pair=[rmw,rmw],command=cmd,server_command=server_cmd,environment={'ROS_DOMAIN_ID':'187','ROS_LOCALHOST_ONLY':'1','RMW_IMPLEMENTATION':rmw,'STUDIO_EXPECT_PARAMETER_OBSERVER':env.get('STUDIO_EXPECT_PARAMETER_OBSERVER'),'STUDIO_EVENT_STARTUP_BURST':env.get('STUDIO_EVENT_STARTUP_BURST')},versions={'probe_sha256':sha(PROBE),'probe_before_sha256':before,'cargo_lock_sha256':sha(ROOT/'Cargo.lock'),'pixi_lock_sha256':sha(WORKSPACE/'pixi.lock'),'native_packages':str((out/'dependencies.json').relative_to(RAW)),'harness_sha256':sha(Path(__file__))},actual=result,expected=expected,evidence=[str(p.relative_to(RAW)) for p in [request_path,stem.with_suffix('.stdout'),stem.with_suffix('.stderr'),server_log,bundle/'manifest.json']]))
                        (RAW/('parameters-'+label+'.json')).write_text(json.dumps(rows,indent=2)+'\n')
                        print(name,rmw,rows[-1]['outcome'],flush=True)
                call('read-types','get_parameters','GetParameters',{'names':['enabled','speed','scale','label','axes','tags','switches','gains','bytes']},{'values':[value(1,'bool_value',True),value(2,'integer_value',2),value(3,'double_value',1.25),value(4,'string_value','fixture'),value(7,'integer_array_value',[1,2,3]),value(9,'string_array_value',['one','two']),value(6,'bool_array_value',[True,False]),value(8,'double_array_value',[1.5,2.5]),value(5,'byte_array_value',[1,255])]})
                # Installed rclpy returns an empty list for an undeclared name;
                # do not fabricate a PARAMETER_NOT_SET value or silently use defaults.
                call('undeclared-read','get_parameters','GetParameters',{'names':['speed','absent']},{'values':[]})
                call('get-types','get_parameter_types','GetParameterTypes',{'names':['enabled','speed','scale','label','bytes','switches','axes','gains','tags']},{'types':[1,2,3,4,5,6,7,8,9]})
                call('write-accepted','set_parameters','SetParameters',{'parameters':[{'name':'speed','value':{'type':2,'integer_value':7}}]},{'results':[{'successful':True,'reason':''}]})
                call('write-readback','get_parameters','GetParameters',{'names':['speed']},{'values':[value(2,'integer_value',7)]})
                call('write-rejected','set_parameters','SetParameters',{'parameters':[{'name':'speed','value':{'type':2,'integer_value':-1}}]},{'results':[{'successful':False,'reason':'speed must be nonnegative'}]})
                call('rejected-readback','get_parameters','GetParameters',{'names':['speed']},{'values':[value(2,'integer_value',7)]})
                call('read-only-rejection','set_parameters','SetParameters',{'parameters':[{'name':'serial','value':{'type':4,'string_value':'changed'}}]},check=lambda r:len(r['results'])==1 and r['results'][0]['successful'] is False and bool(r['results'][0]['reason']))
                call('read-only-readback','get_parameters','GetParameters',{'names':['serial']},{'values':[value(4,'string_value','fixture-001')]})
                mixed={'parameters':[{'name':'label','value':{'type':4,'string_value':'partial'}},{'name':'speed','value':{'type':2,'integer_value':-5}}]}
                call('non-atomic-partial-write','set_parameters','SetParameters',mixed,{'results':[{'successful':True,'reason':''},{'successful':False,'reason':'speed must be nonnegative'}]})
                call('partial-readback','get_parameters','GetParameters',{'names':['label','speed']},{'values':[value(4,'string_value','partial'),value(2,'integer_value',7)]})
                mixed['parameters'][0]['value']['string_value']='must-not-persist'
                call('atomic-rejection','set_parameters_atomically','SetParametersAtomically',mixed,{'result':{'successful':False,'reason':'speed must be nonnegative'}})
                call('atomic-readback','get_parameters','GetParameters',{'names':['label','speed']},{'values':[value(4,'string_value','partial'),value(2,'integer_value',7)]})
                # Concurrent processes exercise the actual native service clients.
                # Atomicity requires a coherent final pair, not a predetermined winner.
                def atomic_request(speed,label):
                    return {'parameters':[{'name':'speed','value':{'type':2,'integer_value':speed}}, {'name':'label','value':{'type':4,'string_value':label}}]}
                for scenario,writes in [
                    ('concurrent-accepted',[(101,'writer-a',True),(202,'writer-b',True)]),
                    ('concurrent-rejected',[(-9,'must-not-persist',False),(303,'writer-c',True)])]:
                    barrier=threading.Barrier(2)
                    with ThreadPoolExecutor(max_workers=2) as pool:
                        futures=[pool.submit(call,scenario+'-'+str(i),'set_parameters_atomically','SetParametersAtomically',atomic_request(speed,label),{'result':{'successful':accepted,'reason':'' if accepted else 'speed must be nonnegative'}},None,barrier) for i,(speed,label,accepted) in enumerate(writes)]
                        for future in futures:future.result()
                    allowed=[{'values':[value(2,'integer_value',speed),value(4,'string_value',label)]} for speed,label,accepted in writes if accepted]
                    call(scenario+'-coherent-readback','get_parameters','GetParameters',{'names':['speed','label']},check=lambda response:response in allowed)
                call('describe-read-only','describe_parameters','DescribeParameters',{'names':['serial']},check=lambda r:len(r['descriptors'])==1 and r['descriptors'][0]['name']=='serial' and r['descriptors'][0]['read_only'] is True and r['descriptors'][0]['type']==4)
                call('list-parameters','list_parameters','ListParameters',{'prefixes':[],'depth':0},check=lambda r:set(['enabled','speed','scale','label','axes','tags','serial']).issubset(r['result']['names']))
                if OBSERVE_EVENTS:
                    call('delete-dynamic-parameter','set_parameters','SetParameters',{'parameters':[{'name':'ephemeral','value':{'type':0}}]},{'results':[{'successful':True,'reason':''}]})
                    deadline=time.monotonic()+5
                    decoded=[]
                    while time.monotonic()<deadline:
                        decoded=[]
                        for line in observer_paths[0].read_text().splitlines():
                            try:
                                event=json.loads(line)
                                if event.get('event')!='sample':continue
                                fields=dict(event['fields'])
                                if fields.get('node')!='Simple(String("/studio_fixture/parameters"))':continue
                                decoded.append({key:json.loads(fields[key]) for key in ['new_parameters','changed_parameters','deleted_parameters']})
                            except (ValueError,KeyError):continue
                        if any(e['deleted_parameters']['length'] for e in decoded):break
                        if observer.poll() is not None:break
                        time.sleep(.05)
                    def pairs(event,category):
                        result={}
                        for item in event[category]['items']:
                            f=item['fields'];v=f['value']['fields'];kind=v['type']
                            result[f['name']['text']]=v['integer_value'] if kind==2 else v['string_value']['text'] if kind==4 else None
                        return result
                    changed=[pairs(e,'changed_parameters') for e in decoded if e['changed_parameters']['length']]
                    new=[pairs(e,'new_parameters') for e in decoded]
                    deleted=[pairs(e,'deleted_parameters') for e in decoded]
                    expected=[{'speed':7},{'label':'partial'},{'speed':101,'label':'writer-a'},{'speed':202,'label':'writer-b'},{'speed':303,'label':'writer-c'}]
                    checks={
                        'parameter-events-new':any(e.get('ephemeral')==123 for e in new),
                        'parameter-events-exact-changes':len(changed)==len(expected) and all(e in changed for e in expected),
                        'parameter-events-delete':any('ephemeral' in e for e in deleted),
                    }
                    if EVENT_STARTUP_BURST:
                        expected_names={'ephemeral','enabled','speed','scale','label','axes','tags','switches','gains','bytes','serial'}
                        checks['parameter-events-startup-burst']=expected_names.issubset({name for event in new for name in event})
                    for name,ok in checks.items():
                        rows.append(dict(test=name,outcome='passed' if ok and observer.poll() is None else 'failed',os=platform.platform(),cpu=platform.machine(),distro=DISTRO,middleware_pair=[rmw,rmw],command=observer_cmd,server_command=server_cmd,environment={key:env.get(key) for key in ['ROS_DOMAIN_ID','ROS_LOCALHOST_ONLY','RMW_IMPLEMENTATION','STUDIO_EXPECT_PARAMETER_OBSERVER','STUDIO_EVENT_STARTUP_BURST']},versions={'backend_sha256':sha(Path(observer_cmd[0])),'probe_sha256':sha(PROBE),'pixi_lock_sha256':sha(WORKSPACE/'pixi.lock'),'harness_sha256':sha(Path(__file__))},actual={'changed':changed,'new':new,'deleted':deleted},expected_changes=expected,evidence=[str(p.relative_to(RAW)) for p in [*observer_paths,server_log]],scope='Observed native backend parameter events; exact accepted atomic groups and absence of rejected changes in this bounded run'))
                        print(name,rmw,rows[-1]['outcome'],flush=True)
                    (RAW/('parameters-'+label+'.json')).write_text(json.dumps(rows,indent=2)+'\n')
            finally:
                if observer is not None:
                    observer.terminate()
                    try:observer.wait(timeout=3)
                    except subprocess.TimeoutExpired:observer.kill();observer.wait()
                    for output in observer_files:output.close()
                server.terminate()
                try:server.wait(timeout=2)
                except subprocess.TimeoutExpired:server.kill();server.wait(timeout=2)
    if PACKAGED_RUNTIME:
        for row in rows:
            row['runtime_manifest_sha256']=sha(PACKAGED_RUNTIME/'manifest.json')
            row['client_environment']=dict(client_env,RMW_IMPLEMENTATION=row['middleware_pair'][0])
            row['scope']='Packaged clients and interface libraries; development Python fixture server. Not clean-host acceptance.'
        (RAW/('parameters-'+label+'.json')).write_text(json.dumps(rows,indent=2)+'\n')
    assert sha(PROBE)==before,'Probe changed during parameter bundle import'
    return 0 if all(r['outcome']=='passed' for r in rows) else 1


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--distro',choices=['humble','jazzy','lyrical'],default='jazzy');parser.add_argument('--server',action='store_true');parser.add_argument('--observe-events',action='store_true');parser.add_argument('--event-startup-burst',action='store_true');parser.add_argument('--label',default=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ'));parser.add_argument('--runtime-bundle',type=Path);args=parser.parse_args()
    if args.server:serve()
    else:
        if not args.label.replace('-','').isalnum():parser.error('Use alphanumeric label')
        if args.event_startup_burst and not args.observe_events:parser.error('Startup burst requires --observe-events')
        EVENT_STARTUP_BURST=args.event_startup_burst
        OBSERVE_EVENTS=args.observe_events
        DISTRO=args.distro
        WORKSPACE=ROOT if DISTRO=='jazzy' else ROOT/'environments'/DISTRO
        RUNTIME=WORKSPACE/'.pixi/envs/default'
        if Path(sys.prefix).resolve()!=RUNTIME.resolve() or os.environ.get('ROS_DISTRO')!=DISTRO:parser.error('Use the selected distro project Python and ROS_DISTRO')
        if DISTRO!='jazzy':PROBE=ROOT/'target'/DISTRO/'debug/studio-service-probe'
        if args.runtime_bundle:
            PACKAGED_RUNTIME=args.runtime_bundle.resolve()
            if json.loads((PACKAGED_RUNTIME/'manifest.json').read_text()).get('distro')!=DISTRO:
                parser.error('Runtime/fixture distro mismatch')
            PROBE=PACKAGED_RUNTIME/'bin/studio-service-probe'
        raise SystemExit(run(args.label))
