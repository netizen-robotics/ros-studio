"""Focused actual-App three-map acceptance, sequential matching localhost DDS pairs."""
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
from session_wire import VERSION as SESSION_VERSION

ROOT = Path(__file__).resolve().parents[1]
assert Path(sys.prefix).resolve() == (ROOT / '.pixi/envs/default').resolve()
label = sys.argv[1]
assert re.fullmatch(r'[a-zA-Z0-9_-]+', label)
out = ROOT / 'artifacts/feasibility/evidence' / label
out.mkdir(exist_ok=False, parents=True)
remote = ('ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI',
          'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE')
versions = {'rust': subprocess.check_output(['rustc', '--version'], text=True).strip(),
            'python': platform.python_version(), 'session': SESSION_VERSION, 'map_snapshot': 2,
            'visualizer_workspace': 19, 'rclrs': '0.8.0', 'wgpu': '24.0.5'}
sources = ['Cargo.lock', 'pixi.lock', 'crates/visualizer/src/multi_map_ros_tests.rs',
           'crates/backend/tests/fixtures/multi_map_publisher.py', 'scripts/test-multi-map-ros.py',
           'crates/client-core/src/backend_pipe/map_pipe.rs', 'crates/protocol/src/subscriptions/registry.rs']
versions['sha256'] = {p: hashlib.sha256((ROOT/p).read_bytes()).hexdigest() for p in sources}
versions['backend_sha256'] = hashlib.sha256((ROOT/'target/debug/studio-backend').read_bytes()).hexdigest()
rows = []
for rmw in ('rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp'):
    folder = out/rmw
    folder.mkdir()
    env = {k:v for k,v in os.environ.items() if k not in remote}
    env.update(ROS_DISTRO='jazzy', ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1',
               ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST', SKIP_DEFAULT_XML='1',
               RMW_IMPLEMENTATION=rmw, STUDIO_BACKEND=str(ROOT/'scripts/ui-ros-backend'),
               STUDIO_UI_ROS_EVIDENCE=str(folder))
    command = ['cargo','test','--locked','-p','studio-visualizer',
               'multi_map_ros_tests::three_maps_real_ros_isolated_updates_and_lifecycle',
               '--','--exact','--ignored','--nocapture','--test-threads=1']
    with (folder/'app-test.log').open('w') as log:
        result = subprocess.run(command,cwd=ROOT,env=env,stdout=log,stderr=subprocess.STDOUT,timeout=180)
    actual = json.loads((folder/'result.json').read_text()) if (folder/'result.json').exists() else None
    text = (folder/'app-test.log').read_text()
    binary = re.search(r'Running unittests .*?\((target/debug/deps/[^)]+)\)',text)
    current_versions = dict(versions)
    if binary:
        current_versions['app_test_sha256'] = hashlib.sha256((ROOT/binary.group(1)).read_bytes()).hexdigest()
    pids=[]
    if (folder/'publisher-ready.json').exists(): pids.append(json.loads((folder/'publisher-ready.json').read_text())['pid'])
    if (folder/'backend-launches.jsonl').exists(): pids.extend(json.loads(line)['pid'] for line in (folder/'backend-launches.jsonl').read_text().splitlines())
    if actual: pids.append(actual['test_pid'])
    remaining=[]
    for pid in pids:
        try: os.kill(pid,0)
        except ProcessLookupError: continue
        remaining.append(pid)
    rows.append(dict(test='three-map-actual-app-ros',outcome='passed' if result.returncode==0 and actual and not remaining else 'failed',
                     os=platform.platform(),cpu=platform.machine(),distro='jazzy',middleware_pair=[rmw,rmw],
                     command=command,versions=current_versions,exit_code=result.returncode,actual=actual,
                     owned_pids=pids,remaining_pids=remaining,evidence=[str(p.relative_to(ROOT)) for p in folder.iterdir()],
                     scope='Actual Rust App/backend with local synthetic ROS on domain187; no native window, LAN or hardware qualification'))
    print(rmw,rows[-1]['outcome'],flush=True)
(ROOT/'artifacts/feasibility'/f'{label}.json').write_text(json.dumps(rows,indent=2)+'\n')
raise SystemExit(0 if all(r['outcome']=='passed' for r in rows) else 1)
