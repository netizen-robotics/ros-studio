"""Paired actual Visualizer Map app-state acceptance using local synthetic ROS.
Run through scripts/dev and the project Python. No native window or robot commands.
"""
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
    parser.add_argument('--diagnostic-profiles', action='store_true',
                        help='Explicit reviewed localhost DDS profiles at publisher/backend; never default acceptance')
    parser.add_argument('--performance', action='store_true',
                        help='Also measure the declared30map/1Hz App data-path workload; no native FPS claim')
    args = parser.parse_args()
    if Path(sys.prefix).resolve() != (ROOT / '.pixi/envs/default').resolve():
        parser.error('Use the project Pixi Python')
    if not re.fullmatch(r'[A-Za-z0-9_-]+', args.label):
        parser.error('Use an alphanumeric evidence label')
    out = ROOT / 'artifacts/feasibility/evidence' / args.label
    out.mkdir(parents=True, exist_ok=False)
    report = ROOT / 'artifacts/feasibility' / (args.label + '.json')
    sources = ['crates/visualizer/src/map_ros_tests.rs', 'crates/visualizer/src/map_display.rs',
               'crates/client-core/src/map_fixture.rs', 'crates/backend/tests/fixtures/map_publisher.py',
               'scripts/test-map-app-ros.py', 'scripts/map_dds_profiles.py', 'scripts/ui-ros-workflow.py',
               'fixtures/dds/localhost-fast-map.xml', 'fixtures/dds/localhost-cyclone-map.xml',
               'crates/backend/native/service_shim.c', 'Cargo.lock', 'pixi.lock']
    versions = {'rust': subprocess.check_output(['rustc', '--version'], text=True).strip(),
                'python': platform.python_version(), 'eframe': '0.31.1', 'rclrs': '0.8.0',
                'session': SESSION_VERSION, 'snapshot': 2, 'visualizer_workspace': 16,
                'source_sha256': {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in sources},
                'backend_sha256': hashlib.sha256((ROOT / 'target/debug/studio-backend').read_bytes()).hexdigest()}
    rows = []
    for rmw in ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']:
        folder = out / rmw
        folder.mkdir()
        env = {k: v for k, v in os.environ.items() if k not in REMOTE}
        env.pop('STUDIO_MAP_DIAGNOSTIC_PROFILES', None)
        env.pop('STUDIO_MAP_PERFORMANCE', None)
        env.update(ROS_DISTRO='jazzy', ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1',
                   ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST', RMW_IMPLEMENTATION=rmw,
                   STUDIO_BACKEND=str(ROOT / 'scripts/ui-ros-backend'), STUDIO_UI_ROS_EVIDENCE=str(folder))
        if args.diagnostic_profiles:
            env['STUDIO_MAP_DIAGNOSTIC_PROFILES'] = '1'
        if args.performance:
            env['STUDIO_MAP_PERFORMANCE'] = '1'
        command = ['cargo', 'test', '--locked', '-p', 'studio-visualizer', '--bin', 'studio-visualizer',
                   'map_ros_tests::map_real_ros_visualizer_workflow', '--', '--exact', '--ignored',
                   '--nocapture', '--test-threads=1']
        log = folder / 'app-test.log'
        timed_out = False
        with log.open('w') as stream:
            process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=stream,
                                       stderr=subprocess.STDOUT, start_new_session=True)
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
        text = log.read_text()
        result = json.loads((folder / 'result.json').read_text()) if (folder / 'result.json').exists() else None
        outcome = 'passed' if process.returncode == 0 and not timed_out and re.search(r'test result: ok\. 1 passed; 0 failed;', text) and result and len(result['cases']) == 15 else 'failed'
        binary = re.search(r'Running unittests .*?\((target/debug/deps/[^)]+)\)', text)
        current_versions = versions.copy()
        if binary:
            current_versions['app_test_binary'] = binary.group(1)
            current_versions['app_test_sha256'] = hashlib.sha256((ROOT / binary.group(1)).read_bytes()).hexdigest()
        pids = []
        if (folder / 'publisher-ready.json').exists():
            pids.append(json.loads((folder / 'publisher-ready.json').read_text())['pid'])
        if (folder / 'backend-launches.jsonl').exists():
            pids.extend(json.loads(line)['pid'] for line in (folder / 'backend-launches.jsonl').read_text().splitlines())
        if result:
            pids.append(result['test_pid'])
        cleanup = subprocess.run(['ps', '-p', ','.join(map(str, pids)), '-o', 'pid=,ppid=,comm='],
                                 capture_output=True, text=True) if pids else None
        cleaned = cleanup is not None and cleanup.returncode == 1 and not cleanup.stdout.strip()
        record = {'test': 'actual-visualizer-map-workflow', 'outcome': outcome,
                  'configuration_scope': 'explicit diagnostic profiles' if args.diagnostic_profiles else 'default no custom profiles',
                  'os': platform.platform(), 'cpu': platform.machine(), 'distro': 'jazzy',
                  'middleware_pair': [rmw, rmw], 'versions': current_versions,
                  'command': command, 'exit_code': process.returncode, 'timed_out': timed_out,
                  'environment': {k: env[k] for k in ['ROS_DOMAIN_ID', 'ROS_LOCALHOST_ONLY', 'ROS_AUTOMATIC_DISCOVERY_RANGE', 'RMW_IMPLEMENTATION', 'STUDIO_BACKEND']},
                  'result': result, 'evidence': [str(p.relative_to(ROOT)) for p in folder.iterdir() if p.is_file()],
                  'scope': 'Actual App state/standard backend/local synthetic ROS; no native-window/GPU/hardware or application performance claim'}
        rows.append(record)
        if args.performance:
            performance_path = folder / 'performance-result.json'
            measurement = json.loads(performance_path.read_text()) if performance_path.exists() else None
            rows.append({'test': 'maximum-map-repeated-data-path-performance',
                         'outcome': measurement['outcome'] if measurement else 'failed',
                         'os': platform.platform(), 'cpu': platform.machine(), 'distro': 'jazzy',
                         'middleware_pair': [rmw, rmw], 'versions': current_versions,
                         'command': command, 'actual': measurement, 'evidence': record['evidence'],
                         'configuration_scope': record['configuration_scope'],
                         'scope': '30 maximum Reliable full maps; completed-delivery latency/RSS samples, no native FPS or transient peak-RSS claim'})
        if result:
            probes = {key: result[key] for key in ['maximum_best_effort_update_observed', 'maximum_best_effort_full_observed']}
            rows.append({'test': 'maximum-best-effort-delivery-probe', 'outcome': 'passed' if all(probes.values()) else 'failed',
                         'os': platform.platform(), 'cpu': platform.machine(), 'distro': 'jazzy',
                         'middleware_pair': [rmw, rmw], 'versions': current_versions,
                         'command': command, 'actual': probes, 'evidence': record['evidence'],
                         'scope': '4MiB Best effort delivery observation remains distinct from small QoS diagnostics; failure cannot be counted as large-data acceptance.'})
            rows[-1]['configuration_scope'] = record['configuration_scope']
        rows.append({'test': 'recorded-process-cleanup', 'outcome': 'passed' if cleaned else 'failed',
                     'os': platform.platform(), 'cpu': platform.machine(), 'distro': 'jazzy',
                     'middleware_pair': [rmw, rmw], 'versions': current_versions,
                     'command': cleanup.args if cleanup else 'No process record available',
                     'pids': pids, 'actual': {'returncode': cleanup.returncode, 'output': cleanup.stdout} if cleanup else None,
                     'evidence': record['evidence']})
        report.write_text(json.dumps(rows, indent=2) + '\n')
        print(rmw, outcome, 'cleanup', cleaned, flush=True)
    raise SystemExit(int(any(row['outcome'] != 'passed' for row in rows)))


if __name__ == '__main__':
    main()
