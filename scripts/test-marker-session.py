"""Serialize both matching localhost ROS/backend/client Marker qualification runs."""
import argparse
import hashlib
import json
from session_wire import VERSION as SESSION_VERSION
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
REMOTE = ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI',
          'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', required=True)
    parser.add_argument('--application', action='store_true', help='Exercise the actual Visualizer root and Marker workers')
    args = parser.parse_args()
    if Path(sys.prefix).resolve() != (ROOT / '.pixi/envs/default').resolve():
        parser.error('Use the project Pixi Python')
    if not re.fullmatch(r'[A-Za-z0-9_-]+', args.label):
        parser.error('Use an alphanumeric label')
    out = ROOT / 'artifacts/feasibility/evidence' / args.label
    out.mkdir(parents=True, exist_ok=False)
    report = ROOT / 'artifacts/feasibility' / (args.label + '.json')
    sources = ['crates/client-core/examples/marker-session.rs', 'crates/client-core/src/map_fixture.rs',
               'crates/backend/tests/fixtures/marker_publisher.py', 'scripts/test-marker-session.py',
               'crates/backend/src/scene_worker/marker_display.rs', 'crates/backend/src/marker_codec.rs',
               'crates/backend/src/marker_producer.rs', 'crates/protocol/src/marker_snapshot.rs',
               'crates/client-core/src/backend_pipe/marker_pipe.rs',
               'crates/client-core/src/backend_pipe/marker_pipe/visibility.rs', 'Cargo.toml',
               'crates/protocol/Cargo.toml', 'Cargo.lock', 'pixi.lock']
    if args.application:
        sources += ['crates/visualizer/src/marker_ros_tests.rs', 'crates/visualizer/src/marker_display.rs',
                    'crates/visualizer/src/main.rs', 'crates/visualizer/src/ui.rs',
                    'crates/visualization/src/spatial/marker_worker.rs',
                    'crates/visualization/src/spatial/marker_worker/preparation.rs',
                    'crates/visualization/src/spatial/marker_worker/executor.rs',
                    'crates/visualization/src/spatial/marker_worker/bounds.rs',
                    'crates/workspace/src/application_v16.rs']
    versions = {'rust': subprocess.check_output(['rustc', '--version'], text=True).strip(),
                'python': platform.python_version(), 'rclrs': '0.8.0', 'session': SESSION_VERSION, 'marker_snapshot': 1,
                'visualizer_workspace': 16,
                'source_sha256': {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in sources},
                'binary_sha256': {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in
                                  (['target/debug/studio-backend'] if args.application else
                                   ['target/debug/studio-backend', 'target/debug/examples/marker-session'])}}
    rows = []
    for rmw in ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']:
        folder = out / rmw
        folder.mkdir()
        env = {k: v for k, v in os.environ.items() if k not in REMOTE}
        env.pop('DYLD_LIBRARY_PATH', None)
        env.update(ROS_DISTRO='jazzy', ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1',
                   ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST', RMW_IMPLEMENTATION=rmw,
                   STUDIO_MARKER_EVIDENCE=str(folder))
        command = [str(ROOT / 'target/debug/examples/marker-session')]
        if args.application:
            env.update(STUDIO_BACKEND=str(ROOT / 'scripts/ui-ros-backend'), STUDIO_UI_ROS_EVIDENCE=str(folder))
            command = ['cargo', 'test', '--locked', '-p', 'studio-visualizer', '--bin', 'studio-visualizer',
                       'marker_ros_tests::marker_real_ros_visualizer_workflow', '--', '--exact', '--ignored',
                       '--nocapture', '--test-threads=1']
        timed_out = False
        with (folder / 'session.log').open('w') as log:
            process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT,
                                       start_new_session=True)
            try:
                process.wait(timeout=240)
            except subprocess.TimeoutExpired:
                timed_out = True
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
        result_path = folder / 'result.json'
        result = json.loads(result_path.read_text()) if result_path.exists() else None
        pids = [process.pid]
        if (folder / 'publisher-ready.json').exists():
            pids.append(json.loads((folder / 'publisher-ready.json').read_text())['pid'])
        pids.extend(int(p.read_text()) for p in folder.glob('backend-*.pid'))
        if (folder / 'backend-launches.jsonl').exists():
            pids.extend(json.loads(line)['pid'] for line in (folder / 'backend-launches.jsonl').read_text().splitlines())
        if result:
            pids.append(result['test_pid'])
        pids = sorted(set(pids))
        cleanup = subprocess.run(['ps', '-p', ','.join(map(str, pids)), '-o', 'pid=,ppid=,comm='], capture_output=True, text=True)
        cleaned = cleanup.returncode == 1 and not cleanup.stdout.strip()
        evidence = [str(p.relative_to(ROOT)) for p in folder.iterdir() if p.is_file()]
        current_versions = versions.copy()
        binary = re.search(r'Running unittests .*?\((target/debug/deps/[^)]+)\)', (folder / 'session.log').read_text())
        if binary:
            current_versions['app_test_binary'] = binary.group(1)
            current_versions['app_test_binary_sha256'] = hashlib.sha256((ROOT / binary.group(1)).read_bytes()).hexdigest()
        row = {'test': 'actual-marker-visualizer-root-workers' if args.application else 'actual-marker-backend-default-client',
               'outcome': 'passed' if process.returncode == 0 and not timed_out and result and len(result['cases']) == 12 else 'failed',
               'os': platform.platform(), 'cpu': platform.machine(), 'distro': 'jazzy',
               'middleware_pair': [rmw, rmw], 'versions': current_versions, 'command': command,
               'environment': {k: env[k] for k in ['ROS_DOMAIN_ID', 'ROS_LOCALHOST_ONLY', 'ROS_AUTOMATIC_DISCOVERY_RANGE', 'RMW_IMPLEMENTATION']},
               'exit_code': process.returncode, 'timed_out': timed_out, 'result': result, 'evidence': evidence,
               'scope': ('Actual Visualizer root/workers/local synthetic ROS and Rust backend; ' if args.application else 'Actual local synthetic ROS and Rust backend/default-client; ') + 'no native-window, GPU, sustained performance, LAN, packaging or hardware acceptance'}
        rows.append(row)
        rows.append({'test': 'recorded-process-cleanup', 'outcome': 'passed' if cleaned else 'failed',
                     'os': platform.platform(), 'cpu': platform.machine(), 'distro': 'jazzy',
                     'middleware_pair': [rmw, rmw], 'versions': versions, 'command': cleanup.args,
                     'pids': pids, 'actual': {'returncode': cleanup.returncode, 'output': cleanup.stdout}, 'evidence': evidence})
        report.write_text(json.dumps(rows, indent=2) + '\n')
        print(rmw, row['outcome'], 'cleanup', cleaned, flush=True)
        if not cleaned:
            break
    raise SystemExit(int(any(r['outcome'] != 'passed' for r in rows)))


if __name__ == '__main__':
    main()
