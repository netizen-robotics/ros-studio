"""Run local checks and record evidence without upgrading their scope."""
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
ROOT=Path(__file__).resolve().parents[1]
(ROOT/'artifacts/feasibility').mkdir(parents=True, exist_ok=True)
OUT=ROOT/'artifacts/feasibility/evidence'
OUT.mkdir(parents=True,exist_ok=True)
base={'os':platform.platform(),'cpu':platform.machine(),'distro':os.environ.get('ROS_DISTRO'),'middleware_pair':[os.environ.get('RMW_IMPLEMENTATION'),None],'versions':{'rclrs':'0.8.0','rust':'1.92.0','locks':{name:hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in ['Cargo.lock','pixi.lock']}}}
checks=[('native-build',['cargo','build','--workspace','--locked'],0),('unit-tests',['cargo','test','--workspace','--locked'],0),('clippy',['cargo','clippy','--workspace','--locked','--','-D','warnings'],0),('format',['cargo','fmt','--all','--','--check'],0),('missing-custom-schema',[str(ROOT/'target/debug/studio-backend'),'/studio_fixture','missing_fixture/msg/Absent'],1)]
records=[]
for name,cmd,expected in checks:
    result=subprocess.run(cmd,cwd=ROOT,env=dict(os.environ,ROS_DOMAIN_ID='190',ROS_LOCALHOST_ONLY='1'),capture_output=True,text=True,timeout=60)
    evidence=OUT/(name+'.log')
    evidence.write_text(result.stdout+result.stderr)
    ok=result.returncode==expected
    if name=='missing-custom-schema': ok=ok and 'RequiredPrefixNotSourced' in result.stdout
    records.append(dict(base,test=name,command=cmd,outcome='passed' if ok else 'failed',exit_code=result.returncode,evidence=str(evidence.relative_to(ROOT))))
(ROOT/'artifacts/feasibility/local-checks.json').write_text(json.dumps(records,indent=2)+'\n')
metadata=[]
for path in (ROOT/'.pixi/envs/default/conda-meta').glob('*.json'):
    p=json.loads(path.read_text())
    if any(s in p['name'] for s in ['rmw','rcl','fastdds','fastrtps','cyclonedds','rosidl','rust']):
        metadata.append({k:p.get(k) for k in ['name','version','build','subdir','sha256','license']})
(ROOT/'artifacts/feasibility/native-dependencies.json').write_text(json.dumps(metadata,indent=2)+'\n')
print(json.dumps(records,indent=2))
raise SystemExit(0 if all(r['outcome']=='passed' for r in records) else 1)
