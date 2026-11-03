"""Runtime-loaded action acceptance on synthetic localhost/domain-187 servers."""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
import platform

ROOT=Path(__file__).resolve().parents[1]
RAW=ROOT/'artifacts/feasibility'
PROBE=ROOT/'target/debug/studio-action-probe'
DISTRO='jazzy'
WORKSPACE=ROOT
RUNTIME=ROOT/'.pixi/envs/default'
BIN=ROOT/'target/debug'
INSTALL_SUFFIX=''
SERVER_STARTUP_TIMEOUT=15
PACKAGED_RUNTIME=None
RMWS=['rmw_fastrtps_cpp','rmw_cyclonedds_cpp']


def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()

def stop(process):
    if process.poll() is None:process.terminate()
    try:process.wait(timeout=2)
    except subprocess.TimeoutExpired:process.kill();process.wait(timeout=2)


class Suite:
    def __init__(self,label):
        self.out=RAW/'evidence'/('actions-'+label);self.out.mkdir(parents=True,exist_ok=False)
        self.result=RAW/('actions-'+label+'.json');self.records=[];self.number=0
        self.env=dict(os.environ,ROS_DOMAIN_ID='187',ROS_LOCALHOST_ONLY='1',ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST',RCUTILS_LOGGING_USE_STDOUT='0',SKIP_DEFAULT_XML='1',AMENT_PREFIX_PATH=str(RUNTIME),DYLD_LIBRARY_PATH=str(RUNTIME/'lib'),ROS_LOG_DIR=str(self.out/'ros-logs'))
        for key in ['ROS_STATIC_PEERS','ROS_DISCOVERY_SERVER','ROS_SUPER_CLIENT','CYCLONEDDS_URI','FASTDDS_DEFAULT_PROFILES_FILE','FASTRTPS_DEFAULT_PROFILES_FILE','FASTDDS_ENVIRONMENT_FILE']:self.env.pop(key,None)
        self.env['CMAKE_PREFIX_PATH']=str(RUNTIME)
        self.env['CARGO_HOME']=str(ROOT/'.cache/cargo')
        self.env['LIBCLANG_PATH']=str(RUNTIME/'lib')
        if DISTRO!='jazzy':self.env['CARGO_TARGET_DIR']=str(ROOT/'target'/DISTRO)
        (self.out/'dependencies.json').write_text(json.dumps([json.loads(p.read_text()) for p in sorted((RUNTIME/'conda-meta').glob('*.json'))],indent=2))
        with (self.out/'build.log').open('w') as log:subprocess.run(['cargo','build','--workspace','--locked'],env=self.env,stdout=log,stderr=log,check=True)
        self.before={name:digest(BIN/name) for name in ['studio-action-probe','studio-service-probe','studio-desktop']}

    def client_environment(self):
        if PACKAGED_RUNTIME is None:return self.env
        home=self.out/'client-home';home.mkdir(exist_ok=True)
        return dict(PATH='/usr/bin:/bin',HOME=str(home),ROS_DISTRO=DISTRO,
                    ROS_DOMAIN_ID='187',ROS_LOCALHOST_ONLY='1',ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST',
                    AMENT_PREFIX_PATH=str(PACKAGED_RUNTIME),RCUTILS_LOGGING_USE_STDOUT='0',ROS_LOG_DIR=str(home/'logs'))

    def record(self,name,passed,command,rmw=None,peer=None,**data):
        if PACKAGED_RUNTIME is not None:
            data.update(runtime_manifest_sha256=digest(PACKAGED_RUNTIME/'manifest.json'),
                        client_base_environment=self.client_environment(),
                        scope='Packaged client on development host; project Python fixture server. Not clean-machine evidence.')
        self.records.append(dict(test=name,outcome='passed' if passed else 'failed',os=platform.platform(),cpu=platform.machine(),distro=DISTRO,middleware_pair=[rmw,peer],command=command,
                                 versions={'probe_sha256':digest(PROBE),'cargo_lock_sha256':digest(ROOT/'Cargo.lock'),'pixi_lock_sha256':digest(WORKSPACE/'pixi.lock'),'native_packages':str((self.out/'dependencies.json').relative_to(RAW))},**data))
        self.result.write_text(json.dumps(self.records,indent=2)+'\n');print(name,self.records[-1]['outcome'],flush=True)

    def build(self,package):
        target=self.out/package
        cmd=[sys.executable,'scripts/build-service-bundle.py','--distro',DISTRO,'--package',package,'--destination',str(target)]
        with (self.out/(package+'-build.log')).open('w') as log:subprocess.run(cmd,env=self.env,stdout=log,stderr=log,check=True)
        return target

    @contextmanager
    def server(self,kind,rmw,mode):
        self.number+=1;stem=self.out/f'server-{self.number}-{kind}-{rmw}-{mode}'
        events=stem.with_suffix('.jsonl');log=stem.with_suffix('.log')
        package='feasibility_interfaces' if kind=='count' else 'service_extra_interfaces'
        install=ROOT/'artifacts'/(package+'-service-install'+INSTALL_SUFFIX)
        env=dict(self.env,RMW_IMPLEMENTATION=rmw,PYTHONPATH=str(install/f'lib/python{sys.version_info.major}.{sys.version_info.minor}/site-packages')+os.pathsep+os.environ.get('PYTHONPATH',''),DYLD_LIBRARY_PATH=str(install/'lib')+os.pathsep+self.env['DYLD_LIBRARY_PATH'],AMENT_PREFIX_PATH=str(install)+os.pathsep+self.env['AMENT_PREFIX_PATH'])
        cmd=[sys.executable,'scripts/action-fixture-server.py','--kind',kind,'--mode',mode,'--log',str(events)]
        with log.open('w') as file:
            p=subprocess.Popen(cmd,env=env,stdout=file,stderr=file)
            try:
                deadline=time.monotonic()+SERVER_STARTUP_TIMEOUT
                while 'READY' not in log.read_text():
                    if p.poll() is not None or time.monotonic()>deadline:
                        self.record('fixture-server-startup',False,cmd,rmw,rmw,startup_timeout_seconds=SERVER_STARTUP_TIMEOUT,evidence=[str(log.relative_to(RAW))])
                        raise RuntimeError(f'Server not ready: {log}')
                    time.sleep(.05)
                yield events,cmd,log,p
            finally:stop(p)

    @staticmethod
    def events(server):
        path=server[0]
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def call(self,name,bundle,kind,goal,rmw,peer=None,server=None,state=None,error=None,check=None,timeout=10000,cancel=None,domain='187',action=None,typ=None):
        self.number+=1;stem=self.out/f'{self.number}-{name}-{rmw}'
        request=stem.with_suffix('.goal.json');request.write_text(goal if isinstance(goal,str) else json.dumps(goal))
        cmd=[str(PROBE),'--bundle',str(bundle),'--action',action or '/studio_fixture/'+kind,'--type',typ or ('feasibility_interfaces/action/Count' if kind=='count' else 'service_extra_interfaces/action/Accumulate'),'--goal-file',str(request),'--timeout-ms',str(timeout)]
        if cancel:cmd+=['--cancel-after-feedback',str(cancel)]
        start=time.monotonic();p=subprocess.run(cmd,env=dict(self.client_environment(),RMW_IMPLEMENTATION=rmw,ROS_DOMAIN_ID=domain),capture_output=True,text=True,timeout=timeout/1000+10)
        stdout=stem.with_suffix('.stdout');stderr=stem.with_suffix('.stderr');stdout.write_text(p.stdout);stderr.write_text(p.stderr)
        try:result=json.loads(p.stdout)
        except json.JSONDecodeError:result={'error':{'code':'invalid_output'},'raw':p.stdout}
        events=self.events(server) if server else []
        goals=[e for e in events if e['event']=='goal']
        passed=len(p.stdout.encode())<=1024*1024
        if error:
            passed=passed and p.returncode==1 and result.get('error',{}).get('code')==error
            if error in ['validation','fixture_guard','input','type_support','bundle','server_unavailable']:
                passed=passed and result.get('goal_state')=='not_sent' and not goals
            elif error=='action_timeout':passed=passed and len(goals)==1 and not [e for e in events if e['event']=='cancel']
        else:
            passed=passed and p.returncode==0 and result.get('outcome')=='passed' and result.get('goal_state')==state
            if server:passed=passed and len(goals)==1 and goals[0]['goal']==goal
            if check:passed=passed and check(result,events)
        self.record(name,passed,cmd,rmw,peer,actual=result,exit_code=p.returncode,seconds=round(time.monotonic()-start,3),server_events=events,server_command=server[1] if server else None,
                    environment={'ROS_DOMAIN_ID':domain,'ROS_LOCALHOST_ONLY':'1','RMW_IMPLEMENTATION':rmw},bundle_manifest_sha256=digest(bundle/'manifest.json'),
                    evidence=[str(p.relative_to(RAW)) for p in [request,stdout,stderr]]+([str(server[0].relative_to(RAW)),str(server[2].relative_to(RAW))] if server else []))
        return result

    def killed(self,bundle,rmw):
        with self.server('count',rmw,'hang') as server:
            req=self.out/f'kill-{rmw}.json';req.write_text('{"target":5}')
            cmd=[str(PROBE),'--bundle',str(bundle),'--action','/studio_fixture/count','--type','feasibility_interfaces/action/Count','--goal-file',str(req)]
            stdout=self.out/f'kill-{rmw}.stdout';stderr=self.out/f'kill-{rmw}.stderr'
            with stdout.open('w') as out,stderr.open('w') as err:
                p=subprocess.Popen(cmd,env=dict(self.client_environment(),RMW_IMPLEMENTATION=rmw),stdout=out,stderr=err)
                try:
                    deadline=time.monotonic()+8
                    while not any(e['event']=='execute' for e in self.events(server)) and p.poll() is None and time.monotonic()<deadline:time.sleep(.05)
                finally:stop(p)
            restarted=subprocess.run([str(PROBE)],env=dict(self.client_environment(),RMW_IMPLEMENTATION=rmw),capture_output=True,text=True,timeout=5)
            data=json.loads(restarted.stdout);restart=self.out/f'restart-{rmw}.stdout';restart.write_text(restarted.stdout)
            events=self.events(server)
            passed=p.returncode<0 and restarted.returncode==1 and data.get('goal_state')=='not_sent' and len([e for e in events if e['event']=='goal'])==1 and not any(e['event']=='cancel' for e in events)
            self.record('kill-restart-no-replay',passed,[cmd,[str(PROBE)]],rmw,rmw,server_events=events,actual=data,server_command=server[1],evidence=[str(p.relative_to(RAW)) for p in [req,stdout,stderr,restart,server[0],server[2]]])


def bundle_errors(suite,bundle):
    for rmw in RMWS:
        for name,key,value in [('distro','ros_distro','jazzy' if DISTRO!='jazzy' else 'humble'),('platform','os','windows'),('cpu','arch','x86_64')]:
            target=suite.out/(name+'-'+rmw);shutil.copytree(bundle,target)
            manifest=json.loads((target/'manifest.json').read_text());manifest[key]=value
            (target/'manifest.json').write_text(json.dumps(manifest))
            suite.call('bundle-'+name,target,'count',{'target':3},rmw,error='bundle')
        for name in ['checksum','missing-file','missing-type-support','unloadable-type-support']:
            target=suite.out/(name+'-'+rmw);shutil.copytree(bundle,target)
            lib=target/'lib/libfeasibility_interfaces__rosidl_typesupport_introspection_c.dylib'
            if name in ['checksum','unloadable-type-support']:lib.write_bytes(b'not a native library')
            else:lib.unlink()
            if name in ['missing-type-support','unloadable-type-support']:
                manifest=json.loads((target/'manifest.json').read_text())
                manifest['files']={str(p.relative_to(target)):digest(p) for p in sorted(target.rglob('*')) if p.is_file() and p.name!='manifest.json'}
                (target/'manifest.json').write_text(json.dumps(manifest))
            suite.call('bundle-'+name,target,'count',{'target':3},rmw,error='type_support' if name in ['missing-type-support','unloadable-type-support'] else 'bundle')


def run(label,bundle_errors_only=False):
    suite=Suite(label);bundle=suite.build('feasibility_interfaces')
    if bundle_errors_only:
        bundle_errors(suite,bundle)
        return 0 if all(r['outcome']=='passed' for r in suite.records) else 1
    for rmw in RMWS:
        with suite.server('count',rmw,'normal') as server:
            suite.call('count-success',bundle,'count',{'target':5},rmw,rmw,server=server,state='succeeded',check=lambda r,e:r['result_response']=={'status':4,'result':{'completed':5}} and r['feedback_count']>=1 and all(1<=f['current']<=5 for f in r['feedback']))
    extra=suite.build('service_extra_interfaces');moved=suite.out/'second action bundle with spaces';shutil.move(str(extra),moved);extra=moved
    after={name:digest(BIN/name) for name in suite.before}
    suite.record('second-action-no-rebuild',suite.before==after,[sys.executable,'scripts/build-service-bundle.py','--distro',DISTRO,'--package','service_extra_interfaces','--destination',str(extra)],before=suite.before,after=after,evidence=[str((suite.out/'service_extra_interfaces-build.log').relative_to(RAW))])
    for rmw in RMWS:
        with suite.server('accumulate',rmw,'normal') as server:
            suite.call('second-action-relocated',extra,'accumulate',{'values':[-2,4,8]},rmw,rmw,server=server,state='succeeded',check=lambda r,e:r['result_response']=={'status':4,'result':{'sum':10}} and r['feedback_count']>=1)
        with suite.server('count',rmw,'reject') as server:
            suite.call('goal-rejection',bundle,'count',{'target':5},rmw,rmw,server=server,state='rejected',check=lambda r,e:r['result_sequence'] is None and r['cancel_sequence'] is None and not any(x['event']=='execute' for x in e))
        with suite.server('count',rmw,'abort') as server:
            suite.call('goal-abort',bundle,'count',{'target':20},rmw,rmw,server=server,state='aborted',check=lambda r,e:r['result_response']=={'status':6,'result':{'completed':2}})
        with suite.server('count',rmw,'normal') as server:
            suite.call('cancel-own-goal',bundle,'count',{'target':20},rmw,rmw,server=server,state='canceled',cancel=3,
                       check=lambda r,e:r['cancel_accepted'] is True and r['cancel_response']['return_code']==0 and any(x['goal_id']==r['goal_id'] for x in r['cancel_response']['goals_canceling']) and 1<=r['result_response']['result']['completed']<20 and len([x for x in e if x['event']=='cancel' and x['uuid']==r['goal_id']['uuid']])==1)
        # Installed Jazzy rclpy removes callback-rejected goals without changing
        # return_code=0. Require no accepted goal, one callback, and normal completion.
        with suite.server('count',rmw,'cancel-reject') as server:
            suite.call('cancel-rejection',bundle,'count',{'target':12},rmw,rmw,server=server,state='succeeded',cancel=2,check=lambda r,e:r['cancel_response']['return_code'] in [0,1] and r['cancel_response']['goals_canceling']==[] and r['cancel_accepted'] is False and r['result_response']['result']['completed']==12 and len([x for x in e if x['event']=='cancel' and x['uuid']==r['goal_id']['uuid']])==1)
        with suite.server('count',rmw,'foreign') as server:
            suite.call('foreign-feedback-isolation',bundle,'count',{'target':8},rmw,rmw,server=server,state='succeeded',check=lambda r,e:r['foreign_feedback_ignored']>0 and 1<=r['feedback_count']<=8)
        with suite.server('count',rmw,'flood') as server:
            suite.call('feedback-bounds',bundle,'count',{'target':1000},rmw,rmw,server=server,state='succeeded',check=lambda r,e:r['feedback_count']>64 and len(r['feedback'])==64 and r['feedback_dropped']==r['feedback_count']-64)
        for mode in ['hang','delay-goal']:
            with suite.server('count',rmw,mode) as server:
                suite.call(mode+'-timeout',bundle,'count',{'target':5},rmw,rmw,server=server,error='action_timeout',timeout=3000)
        suite.call('unavailable',bundle,'count',{'target':5},rmw,error='server_unavailable',timeout=500,action='/studio_fixture/absent')
        for name,goal,error in [('invalid-goal',{'target':-1},'validation'),('unknown-field',{'unknown':5},'validation'),('invalid-json','{','input'),('input-bound',' '*1048577,'input')]:
            suite.call(name,bundle,'count',goal,rmw,error=error)
        suite.call('wrong-domain',bundle,'count',{'target':5},rmw,error='fixture_guard',domain='57')
        suite.call('wrong-namespace',bundle,'count',{'target':5},rmw,error='fixture_guard',action='/navigate_to_pose')
        suite.call('missing-action-type',bundle,'count',{},rmw,error='type_support',typ='feasibility_interfaces/action/Missing')
        suite.killed(bundle,rmw)
    bundle_errors(suite,bundle)
    for local,peer in [(RMWS[0],RMWS[1]),(RMWS[1],RMWS[0])]:
        with suite.server('count',peer,'normal') as server:
            suite.call('mixed-action-exploratory',bundle,'count',{'target':3},local,peer,server=server,state='succeeded',timeout=5000,check=lambda r,e:r['result_response']=={'status':4,'result':{'completed':3}})
    return 0 if all(r['outcome']=='passed' for r in suite.records if r['test']!='mixed-action-exploratory') else 1


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--distro',choices=['humble','jazzy','lyrical'],default='jazzy');parser.add_argument('--server-startup-timeout',type=float,default=15);parser.add_argument('--label',default=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ'));parser.add_argument('--runtime-bundle',type=Path);parser.add_argument('--bundle-errors-only',action='store_true');args=parser.parse_args()
    if not args.label.replace('-','').isalnum():parser.error('Use alphanumeric label')
    DISTRO=args.distro
    WORKSPACE=ROOT if DISTRO=='jazzy' else ROOT/'environments'/DISTRO
    RUNTIME=WORKSPACE/'.pixi/envs/default'
    if Path(sys.prefix).resolve()!=RUNTIME.resolve() or os.environ.get('ROS_DISTRO')!=DISTRO:parser.error('Use the selected distro project Python and ROS_DISTRO')
    if not 0<args.server_startup_timeout<=120:parser.error('Startup timeout must be between 0 and 120 seconds')
    SERVER_STARTUP_TIMEOUT=args.server_startup_timeout
    if DISTRO!='jazzy':
        INSTALL_SUFFIX='-'+DISTRO
        BIN=ROOT/'target'/DISTRO/'debug'
        PROBE=BIN/'studio-action-probe'
    if args.runtime_bundle:
        PACKAGED_RUNTIME=args.runtime_bundle.resolve()
        if json.loads((PACKAGED_RUNTIME/'manifest.json').read_text()).get('distro')!=DISTRO:
            parser.error('Packaged runtime distro must match the selected fixture distro')
        BIN=PACKAGED_RUNTIME/'bin';PROBE=BIN/'studio-action-probe'
    raise SystemExit(run(args.label,args.bundle_errors_only))
