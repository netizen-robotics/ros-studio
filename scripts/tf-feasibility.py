"""Build and record the offline native tf2 proof. Does not test /tf or /clock delivery."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import platform
import subprocess

ROOT=Path(__file__).resolve().parents[1]
RAW=ROOT/'artifacts/feasibility'
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--label',default=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ'))
args=parser.parse_args()
if not args.label.replace('-','').isalnum():parser.error('Use alphanumeric label')
out=RAW/'evidence'/('tf-'+args.label);out.mkdir(parents=True,exist_ok=False)
commands=[['cargo','build','--workspace','--locked'],[str(ROOT/'target/debug/studio-tf-probe')]]
results=[]
for i,cmd in enumerate(commands):
    p=subprocess.run(cmd,cwd=ROOT,capture_output=True,text=True,timeout=120)
    (out/f'{i}.stdout').write_text(p.stdout);(out/f'{i}.stderr').write_text(p.stderr)
    results.append(p)
    if p.returncode:break
versions={name:hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in ['Cargo.lock','pixi.lock']}
probe=ROOT/'target/debug/studio-tf-probe'
if probe.exists():versions['probe_sha256']=hashlib.sha256(probe.read_bytes()).hexdigest()
versions['tf2_packages']=[{k:d.get(k) for k in ['name','version','build','sha256']} for path in (ROOT/'.pixi/envs/default/conda-meta').glob('*tf2*.json') for d in [json.loads(path.read_text())]]
base=dict(os=platform.platform(),cpu=platform.machine(),distro='jazzy',middleware_pair=[None,None],versions=versions,command=commands,evidence=[str(p.relative_to(RAW)) for p in sorted(out.glob('*'))])
rows=[]
if len(results)==2 and results[-1].returncode==0:
    data=json.loads(results[-1].stdout)
    for check in data['checks']:
        rows.append(dict(base,test=check['test'],outcome='passed' if check['passed'] else 'failed',scope=data['scope'],actual=check))
else:
    rows.append(dict(base,test='tf-build-or-runtime',outcome='failed',exit_code=results[-1].returncode))
(RAW/('tf-'+args.label+'.json')).write_text(json.dumps(rows,indent=2)+'\n')
print(f'{sum(r["outcome"]=="passed" for r in rows)}/{len(rows)} native tf2 checks passed')
raise SystemExit(0 if all(r['outcome']=='passed' for r in rows) else 1)
