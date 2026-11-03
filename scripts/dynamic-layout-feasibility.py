"""Record offline dynamic-field inspection without creating ROS network entities."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--distro', required=True, choices=['humble', 'jazzy', 'lyrical'])
parser.add_argument('--label', required=True)
args = parser.parse_args()
if not args.label.replace('-', '').isalnum():
    parser.error('Use an alphanumeric label')
workspace = ROOT if args.distro == 'jazzy' else ROOT/'environments'/args.distro
runtime = workspace/'.pixi/envs/default'
if Path(sys.prefix).resolve() != runtime.resolve() or os.environ.get('ROS_DISTRO') != args.distro:
    parser.error('Use the selected distro project Python and ROS_DISTRO')
target = ROOT/'target' if args.distro == 'jazzy' else ROOT/'target'/args.distro
probe = target/'debug/studio-dynamic-layout-probe'
raw = ROOT/'artifacts/feasibility'
out = raw/'evidence'/('dynamic-layout-'+args.label)
out.mkdir(parents=True, exist_ok=False)
env = dict(os.environ, AMENT_PREFIX_PATH=str(runtime), DYLD_LIBRARY_PATH=str(runtime/'lib'), RUST_BACKTRACE='1')
command = [str(probe)]
result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=60)
(out/'stdout').write_text(result.stdout)
(out/'stderr').write_text(result.stderr)
data = json.loads(result.stdout)
versions = {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in [probe, ROOT/'Cargo.lock', workspace/'pixi.lock']}
rows = [dict(test=check.get('path', 'rclrs')+'/'+check['type']+'/'+check['field'], outcome='passed' if check['passed'] else 'failed',
             os=platform.platform(), cpu=platform.machine(), distro=args.distro,
             middleware_pair=['not started', 'not started'], versions=versions,
             command=command, scope=data['scope'], actual=check,
             evidence=[str((out/name).relative_to(raw)) for name in ['stdout', 'stderr']])
        for check in data['checks']]
(raw/('dynamic-layout-'+args.label+'.json')).write_text(json.dumps(rows, indent=2)+'\n')
print(json.dumps(data))
raise SystemExit(result.returncode)
