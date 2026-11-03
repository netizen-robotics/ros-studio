"""Build/test under an activated, locked Pixi environment on each native runner."""
import os
import argparse
from datetime import datetime, timezone
import hashlib
import json
import platform
from pathlib import Path
import subprocess
import sys
import time
ROOT=Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--label',default=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ'))
parser.add_argument('--distro',choices=['humble','jazzy','lyrical'],default='jazzy')
parser.add_argument('--profile',choices=['debug','release'],default='debug')
args=parser.parse_args()
if not args.label.replace('-','').isalnum():parser.error('Use alphanumeric label')
out=ROOT/'artifacts/feasibility/evidence'/('native-'+args.label)
out.mkdir(parents=True,exist_ok=False)
workspace=ROOT if args.distro=='jazzy' else ROOT/'environments'/args.distro
PREFIX=Path(os.environ.get('CONDA_PREFIX', workspace/'.pixi/envs/default'))
if args.distro!='jazzy' and PREFIX.resolve()!=(workspace/'.pixi/envs/default').resolve():
    raise SystemExit('Selected distro must run inside its isolated Pixi environment')
if (PREFIX/'Library/share/ament_index').exists(): PREFIX=PREFIX/'Library'
if not (PREFIX/'share/ament_index').exists(): raise SystemExit(f'ROS package index missing: {PREFIX}')
env=dict(os.environ, ROS_DISTRO=args.distro, AMENT_PREFIX_PATH=str(PREFIX), CMAKE_PREFIX_PATH=str(PREFIX), RMW_IMPLEMENTATION='rmw_fastrtps_cpp', ROS_LOG_DIR=str(ROOT/'artifacts/ros-logs'/args.distro), RCUTILS_LOGGING_USE_STDOUT='0', CARGO_HOME=str(ROOT/'.cache/cargo'))
if args.distro!='jazzy':env['CARGO_TARGET_DIR']=str(ROOT/'target'/args.distro)
env['LIBCLANG_PATH']=str(PREFIX/('bin' if os.name=='nt' else 'lib'))
versions={name:hashlib.sha256(path.read_bytes()).hexdigest() for name,path in [('Cargo.lock',ROOT/'Cargo.lock'),('pixi.lock',workspace/'pixi.lock')]}
for tool in ['rustc','cargo']:
    versions[tool]=subprocess.check_output([tool,'--version'],env=env,text=True).strip()
metadata=PREFIX/'conda-meta'
if not metadata.exists():metadata=PREFIX.parent/'conda-meta'
packages=[]
for file in sorted(metadata.glob('*.json')):
    entry=json.loads(file.read_text())
    packages.append({k:entry[k] for k in ['name','version','build','subdir','sha256'] if k in entry})
inventory=out/'dependencies.json';inventory.write_text(json.dumps(packages,indent=2)+'\n')
records=[];build_failed=False
profile_flags=['--release'] if args.profile=='release' else []
for name,command in [
    ('workspace-build',['cargo','build','--workspace','--all-targets','--locked',*profile_flags]),
    ('workspace-tests',['cargo','test','--workspace','--locked','--no-fail-fast',*profile_flags]),
    ('format',['cargo','fmt','--all','--','--check']),
    ('clippy',['cargo','clippy','--workspace','--all-targets','--locked',*profile_flags,'--','-D','warnings']),
]:
    log=out/(name+'.log');started=time.monotonic();code=None
    if build_failed and name in ['workspace-tests','clippy']:
        outcome='not tested';log.write_text('Skipped because workspace build failed.\n')
    else:
        with log.open('w') as output:
            code=subprocess.run(command,cwd=ROOT,env=env,stdout=output,stderr=subprocess.STDOUT).returncode
        outcome='passed' if code==0 else 'failed'
        if name=='workspace-build' and code:build_failed=True
    records.append(dict(test=name,outcome=outcome,os=platform.platform(),cpu=platform.machine(),distro=args.distro,
                        middleware_pair=['rmw_fastrtps_cpp',None],versions=versions,command=command,exit_code=code,
                        environment={k:env[k] for k in ['ROS_DISTRO','AMENT_PREFIX_PATH','CMAKE_PREFIX_PATH','LIBCLANG_PATH','CARGO_HOME','CARGO_TARGET_DIR'] if k in env},
                        seconds=round(time.monotonic()-started,3),evidence=[str(p.relative_to(ROOT)) for p in [log,inventory]],
                        scope='Native build/unit/static checks only; no clean-host, ROS interoperability, UI or target-OS acceptance inferred.'))
    (ROOT/'artifacts/feasibility'/('native-'+args.label+'.json')).write_text(json.dumps(records,indent=2)+'\n')
    print(name,outcome,flush=True)
raise SystemExit(0 if all(r['outcome']=='passed' for r in records) else 1)
