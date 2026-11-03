"""Serialized actual-App acceptance with local-only synthetic diagnostic publishers."""
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'artifacts/feasibility/evidence/shared-diagnostics-20261031'
assert Path(sys.prefix).resolve() == (ROOT / '.pixi/envs/default').resolve()
assert os.environ.get('ROS_DISTRO') == 'jazzy'
for rmw in (sys.argv[1:] or ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']):
    assert rmw in ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']
    folder = OUT / (rmw + os.environ.get('STUDIO_HEALTH_RUN_SUFFIX', ''))
    folder.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ)
    for key in ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI', 'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']:
        env.pop(key, None)
    env.update(ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1', ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST', RMW_IMPLEMENTATION=rmw,
               STUDIO_UI_ROS_EVIDENCE=str(folder), STUDIO_BACKEND=str(ROOT / 'scripts/ui-ros-backend'))
    publisher_env = dict(env, DYLD_LIBRARY_PATH=str(ROOT / '.pixi/envs/default/lib'))
    app = os.environ.get('STUDIO_HEALTH_TEST_APP')
    assert app in [None, 'tools', 'visualizer']
    packages = ['-p', f'studio-{app}'] if app else ['-p', 'studio-tools', '-p', 'studio-visualizer']
    command = ['scripts/dev', 'cargo', 'test', '--locked', *packages, 'health_real_ros', '--', '--ignored', '--test-threads=1', '--nocapture']
    with (folder / 'publisher.log').open('w') as log:
        publisher = subprocess.Popen([sys.executable, str(ROOT / 'crates/backend/tests/fixtures/diagnostic_publisher.py')], env=publisher_env, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 60
            while not (folder / 'diagnostic-publisher.json').exists():
                assert publisher.poll() is None, 'Publisher exited; inspect publisher.log'
                assert time.monotonic() < deadline, 'Publisher startup timed out'
                time.sleep(.1)
            with (folder / 'app-tests.log').open('w') as result:
                run = subprocess.run(command, env=env, stdout=result, stderr=subprocess.STDOUT, timeout=300)
            outcome = 'passed' if run.returncode == 0 else 'failed'
        finally:
            publisher.terminate()
            try:
                publisher.wait(timeout=10)
            except subprocess.TimeoutExpired:
                publisher.kill()
                publisher.wait()
    record = dict(outcome=outcome, os=platform.platform(), cpu=platform.machine(), ros='jazzy', middleware_pair=[rmw, rmw], command=command,
                  publisher_pid=publisher.pid, publisher_reaped=True, returncode=run.returncode,
                  binaries={name: hashlib.sha256((ROOT / f'target/debug/{name}').read_bytes()).hexdigest() for name in ['studio-tools', 'studio-visualizer', 'studio-backend']})
    (folder / 'run.json').write_text(json.dumps(record, indent=2))
    print(rmw, outcome, flush=True)
    if run.returncode:
        raise SystemExit(run.returncode)
