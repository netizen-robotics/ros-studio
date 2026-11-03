import hashlib
from datetime import datetime, timezone
import json
import argparse
import os
from pathlib import Path
import platform
import shutil
import subprocess
import time
ROOT=Path(__file__).resolve().parents[1]
EVIDENCE=ROOT/'artifacts/feasibility/evidence'
EVIDENCE.mkdir(parents=True, exist_ok=True)
parser=argparse.ArgumentParser()
parser.add_argument('--app',action='store_true')
parser.add_argument('--bundle',type=Path)
parser.add_argument('--label',default=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ'))
args=parser.parse_args()
if not args.label.replace('-','').isalnum(): parser.error('Use alphanumeric label')
label=('app' if args.app else 'bundle')+'-'+args.label
EVIDENCE=EVIDENCE/label
EVIDENCE.mkdir(parents=True,exist_ok=False)
source=ROOT/'artifacts/macos-jazzy'
bundle=ROOT/'artifacts/relocated runtime with spaces'
if args.bundle: bundle=args.bundle.resolve()
elif args.app: bundle=ROOT/'artifacts/ROS Studio Feasibility.app/Contents'
elif source.exists() and not bundle.exists(): source.rename(bundle)
if not bundle.exists(): raise SystemExit('Run package-macos.py first')
manifest_path=bundle/'manifest.json'
if args.app and not manifest_path.exists():manifest_path=bundle/'Resources/manifest.json'
manifest=json.loads(manifest_path.read_text());distro=manifest['distro']
if distro not in ['humble','jazzy','lyrical']:raise SystemExit('Unsupported bundle distro')
workspace=ROOT if distro=='jazzy' else ROOT/'environments'/distro
# Only explicit system/app configuration; no inherited ROS, Pixi, Python, or loader paths.
env={'PATH':'/usr/bin:/bin','HOME':str(ROOT/'artifacts/clean-home'),'ROS_LOG_DIR':str(ROOT/'artifacts/clean-home/log'),'AMENT_PREFIX_PATH':str(bundle),'ROS_DISTRO':distro,'ROS_DOMAIN_ID':'188','ROS_LOCALHOST_ONLY':'1','RCUTILS_LOGGING_USE_STDOUT':'0'}
if args.app and (bundle/'Resources/share/ament_index').is_dir():
    env['AMENT_PREFIX_PATH']=str(bundle/'Resources')
Path(env['HOME']).mkdir(exist_ok=True)
results=[]
for rmw in ['rmw_fastrtps_cpp','rmw_cyclonedds_cpp']:
    out=EVIDENCE/f'{label}-{rmw}.jsonl'
    err=EVIDENCE/f'{label}-{rmw}.stderr'
    cmd=[str(bundle/('MacOS' if args.app else 'bin')/'studio-backend')]
    started=time.monotonic(); passed=False
    with out.open('w') as output, err.open('w') as errors:
        proc=subprocess.Popen(cmd,env=dict(env,RMW_IMPLEMENTATION=rmw),stdout=output,stderr=errors)
        try:
            deadline=time.monotonic()+20
            while time.monotonic()<deadline:
                time.sleep(.2)
                events=[]
                for line in out.read_text().splitlines():
                    try: events.append(json.loads(line))
                    except json.JSONDecodeError: pass
                if any(e.get('event')=='graph' for e in events) and any(e.get('event')=='hello' and e.get('distro')==distro and e.get('rmw')==rmw for e in events): passed=True; break
                if proc.poll() is not None: break
        finally:
            if proc.poll() is None: proc.terminate()
            try: proc.wait(timeout=3)
            except subprocess.TimeoutExpired: proc.kill(); proc.wait()
    results.append(dict(test='relocated-bundle-startup',outcome='passed' if passed else 'failed',os=platform.platform(),cpu=platform.machine(),distro=distro,versions={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in [workspace/'pixi.lock',ROOT/'Cargo.lock']} | {'backend_sha256':hashlib.sha256(Path(cmd[0]).read_bytes()).hexdigest()},middleware_pair=[rmw,None],command=cmd,seconds=round(time.monotonic()-started,3),evidence=[str(out.relative_to(ROOT)),str(err.relative_to(ROOT))],limitation='Sanitized environment on development host, NOT a clean-machine test'))
(ROOT/f'artifacts/feasibility/{label}-results.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps(results,indent=2))
raise SystemExit(0 if all(r['outcome']=='passed' for r in results) else 1)
