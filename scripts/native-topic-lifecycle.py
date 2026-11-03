"""Native fixture topic startup/normal teardown and guard diagnostics; no peer."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import time
ROOT=Path(__file__).resolve().parents[1]
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--distro',required=True,choices=['humble','jazzy','lyrical'])
p.add_argument('--label',required=True)
a=p.parse_args()
if not a.label.replace('-','').isalnum():p.error('Use alphanumeric label')
workspace=ROOT if a.distro=='jazzy' else ROOT/'environments'/a.distro
runtime=workspace/'.pixi/envs/default'
if Path(sys.prefix).resolve()!=runtime.resolve() or os.environ.get('ROS_DISTRO')!=a.distro:p.error('Use selected distro Python and ROS_DISTRO')
target=ROOT/'target' if a.distro=='jazzy' else ROOT/'target'/a.distro
probe=target/'debug/studio-native-topic-probe'
raw=ROOT/'artifacts/feasibility';out=raw/'evidence'/('native-topic-lifecycle-'+a.label);out.mkdir(parents=True,exist_ok=False)
versions={str(f.relative_to(ROOT)):hashlib.sha256(f.read_bytes()).hexdigest() for f in [probe,workspace/'pixi.lock',ROOT/'Cargo.lock']}
rows=[]
for rmw in ['rmw_fastrtps_cpp','rmw_cyclonedds_cpp']:
 for name,domain,topic in [('normal-teardown','187','/studio_fixture'),('domain-guard','57','/studio_fixture'),('topic-guard','187','/not_a_fixture')]:
  env=dict(os.environ,ROS_DOMAIN_ID=domain,ROS_LOCALHOST_ONLY='1',ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST',AMENT_PREFIX_PATH=str(runtime),DYLD_LIBRARY_PATH=str(runtime/'lib'),RMW_IMPLEMENTATION=rmw,ROS_LOG_DIR=str(out/'ros-logs'))
  for key in ['ROS_STATIC_PEERS','ROS_DISCOVERY_SERVER','ROS_SUPER_CLIENT','CYCLONEDDS_URI','FASTDDS_DEFAULT_PROFILES_FILE','FASTRTPS_DEFAULT_PROFILES_FILE','FASTDDS_ENVIRONMENT_FILE']:env.pop(key,None)
  cmd=[str(probe),topic,'sensor_msgs/msg/Image','--fixture-echo','--duration-ms','500']
  start=time.monotonic();result=subprocess.run(cmd,env=env,capture_output=True,text=True,timeout=20)
  stem=out/(name+'-'+rmw);stem.with_suffix('.stdout').write_text(result.stdout);stem.with_suffix('.stderr').write_text(result.stderr)
  try:actual=json.loads(result.stdout)
  except json.JSONDecodeError:actual={'invalid_output':result.stdout}
  ok=result.returncode==0 and actual.get('result')=={'Ok':0} if name=='normal-teardown' else result.returncode==1 and isinstance(actual.get('result',{}).get('Err'),str)
  rows.append(dict(test=name,outcome='passed' if ok else 'failed',os=platform.platform(),cpu=platform.machine(),distro=a.distro,middleware_pair=[rmw,'no peer'],versions=versions,command=cmd,environment={'ROS_DOMAIN_ID':domain,'ROS_LOCALHOST_ONLY':'1','RMW_IMPLEMENTATION':rmw},seconds=time.monotonic()-start,actual=actual,evidence=[str(stem.with_suffix(s).relative_to(raw)) for s in ['.stdout','.stderr']],scope='Normal process exit after native context/subscription/publisher lifetime or pre-connect guard rejection; not a memory leak or peer-delivery test'))
  (raw/('native-topic-lifecycle-'+a.label+'.json')).write_text(json.dumps(rows,indent=2)+'\n')
  print(name,rmw,rows[-1]['outcome'],flush=True)
raise SystemExit(0 if all(r['outcome']=='passed' for r in rows) else 1)
