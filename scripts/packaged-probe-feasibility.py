"""Run packaged service/action probes with sanitized client environments."""
import argparse
import json
import os
import sys
import importlib.util
from pathlib import Path
import subprocess


def load(name):
    spec=importlib.util.spec_from_file_location(name,Path(__file__).with_name(name+'.py'))
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    return module


parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--runtime',type=Path,required=True)
parser.add_argument('--label',required=True)
args=parser.parse_args()
if not args.label.replace('-','').isalnum(): parser.error('Use alphanumeric label')
runtime=args.runtime.resolve()
service=load('service-feasibility'); action=load('action-feasibility')
manifest=json.loads((runtime/'manifest.json').read_text());distro=manifest.get('distro')
if distro not in ['humble','jazzy','lyrical']:parser.error('Unsupported runtime distro')
root=Path(__file__).resolve().parents[1]
workspace=root if distro=='jazzy' else root/'environments'/distro
development=workspace/'.pixi/envs/default'
if Path(sys.prefix).resolve()!=development.resolve() or os.environ.get('ROS_DISTRO')!=distro:parser.error('Use matching isolated development Python for fixture servers')
for module in [service,action]:
    module.DISTRO=distro;module.WORKSPACE=workspace;module.RUNTIME=development
    module.INSTALL_SUFFIX='' if distro=='jazzy' else '-'+distro
service.PROBE=runtime/'bin/studio-service-probe';service.DESKTOP=runtime/'bin/studio-desktop'
action.PROBE=runtime/'bin/studio-action-probe';action.BIN=runtime/'bin'
ss=service.Suite('packaged-'+args.label); aa=action.Suite('packaged-'+args.label)
inspect=ss.build_bundle('feasibility_interfaces'); reflect=ss.build_bundle('service_extra_interfaces')
home=ss.out/'clean-home';home.mkdir()
clean={'PATH':'/usr/bin:/bin','HOME':str(home),'ROS_DISTRO':distro,'ROS_DOMAIN_ID':'187',
       'ROS_LOCALHOST_ONLY':'1','ROS_AUTOMATIC_DISCOVERY_RANGE':'LOCALHOST','AMENT_PREFIX_PATH':str(runtime),'RCUTILS_LOGGING_USE_STDOUT':'0','ROS_LOG_DIR':str(home/'logs')}
def timed_out(suite,name,error,rmw):
    log=suite.out/(name+'-'+rmw+'-timeout.log')
    log.write_text(str(error)+'\n'+repr(error.stdout)+'\n'+repr(error.stderr))
    suite.record(name,False,error.cmd,rmw,rmw,reason='Packaged client exceeded harness process deadline',evidence=[str(log.relative_to(service.RAW))])
for rmw in service.RMWS:
    for kind,bundle,request,expected in [('inspect',inspect,service.INSPECT,{'accepted':True,'reason':'validated'}),
                                         ('reflect',reflect,service.REFLECT,service.REFLECT_RESPONSE)]:
        with ss.server(kind,rmw) as server:
            dev=ss.base;ss.base=clean
            try: ss.call('packaged-'+kind,bundle,kind,request,rmw,rmw,expected=expected,server=server)
            except subprocess.TimeoutExpired as error: timed_out(ss,'packaged-'+kind,error,rmw)
            finally: ss.base=dev
    for kind,bundle,goal,expected in [('count',inspect,{'target':5},{'status':4,'result':{'completed':5}}),
                                      ('accumulate',reflect,{'values':[-2,4,8]},{'status':4,'result':{'sum':10}})]:
        with aa.server(kind,rmw,'normal') as server:
            dev=aa.env;aa.env=clean
            try: aa.call('packaged-'+kind,bundle,kind,goal,rmw,rmw,server=server,state='succeeded',check=lambda r,e:r['result_response']==expected and r['feedback_count']>0)
            except subprocess.TimeoutExpired as error: timed_out(aa,'packaged-'+kind,error,rmw)
            finally: aa.env=dev
    with ss.server('inspect',rmw) as server:
        dev=ss.base;ss.base=clean
        try:
            ss.call('packaged-invalid-request',inspect,'inspect',{'unexpected':1},rmw,rmw,error='validation',server=server)
        finally: ss.base=dev
    dev=ss.base;ss.base=clean
    try:
        ss.call('packaged-service-unavailable',inspect,'inspect',service.INSPECT,rmw,error='server_unavailable',timeout=500,service='/studio_fixture/absent')
    finally: ss.base=dev
    with aa.server('count',rmw,'normal') as server:
        dev=aa.env;aa.env=clean
        try:
            aa.call('packaged-cancel-own-goal',inspect,'count',{'target':30},rmw,rmw,server=server,state='canceled',cancel=3,
                    check=lambda r,e:r['cancel_accepted'] is True and 1<=r['result_response']['result']['completed']<30
                    and len([x for x in e if x['event']=='cancel' and x['uuid']==r['goal_id']['uuid']])==1)
        finally: aa.env=dev
    with aa.server('count',rmw,'hang') as server:
        dev=aa.env;aa.env=clean
        try:
            aa.call('packaged-action-timeout-no-retry',inspect,'count',{'target':5},rmw,rmw,server=server,error='action_timeout',timeout=3000)
        finally: aa.env=dev
# Full sanitized environment is separate from the development fixture environment.
import json
for suite,records,path in [(ss,ss.records,ss.result),(aa,aa.records,aa.result)]:
    for row in records:
        row['runtime_manifest_sha256']=service.digest(runtime/'manifest.json')
        row['sanitized_client_environment']=dict(clean,RMW_IMPLEMENTATION=row['middleware_pair'][0])
        row['limitation']='Development host; Python fixture server only. Packaged probe does not invoke Python. Not a clean-machine qualification.'
    path.write_text(json.dumps(records,indent=2)+'\n')
raise SystemExit(0 if all(r['outcome']=='passed' for r in ss.records+aa.records) else 1)
