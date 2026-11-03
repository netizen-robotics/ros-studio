"""Serialized opt-in actual App/egui/native GPU maximum load. No ROS/window claim.
Run through scripts/dev with the project Python; full qualification requires60s.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import sys
from session_wire import VERSION as SESSION_VERSION

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', required=True)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--uncertainty', action='store_true', help='Run maximum covariance/Odometry history instead of Markers')
    mode.add_argument('--camera', action='store_true', help='Run maximum calibrated Camera instead of Markers')
    args = parser.parse_args()
    if Path(sys.prefix).resolve() != (ROOT / '.pixi/envs/default').resolve():
        parser.error('Use the project Pixi Python')
    if not re.fullmatch(r'[A-Za-z0-9_-]+', args.label):
        parser.error('Use an alphanumeric evidence label')
    folder = ROOT / 'artifacts/feasibility/evidence' / args.label
    folder.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ)
    env.pop('DYLD_LIBRARY_PATH', None)
    for key in ['STUDIO_MARKER_PERFORMANCE_SOURCE_HZ', 'STUDIO_MARKER_PERFORMANCE_CAMERA_HZ', 'STUDIO_MARKER_PERFORMANCE_VMMAP']:
        env.pop(key, None)
    env.update(STUDIO_MARKER_PERFORMANCE_EVIDENCE=str(folder), STUDIO_MARKER_PERFORMANCE_SECONDS='60',
               STUDIO_MARKER_PERFORMANCE_SOURCE_HZ='2', STUDIO_MARKER_PERFORMANCE_CAMERA_HZ='30')
    test = 'marker_display::performance::maximum_app_metal_load_and_lifecycle'
    if args.uncertainty:
        env['STUDIO_UNCERTAINTY_PERFORMANCE_EVIDENCE'] = str(folder)
        test = 'uncertainty_performance_tests::maximum_uncertainty_app_metal_load_and_lifecycle'
    if args.camera:
        env['STUDIO_CAMERA_PERFORMANCE_EVIDENCE'] = str(folder)
        test = 'camera_performance_tests::maximum_camera_app_metal_load_and_lifecycle'
    command = ['cargo', 'test', '--locked', '--release', '-p', 'studio-visualizer', '--bin', 'studio-visualizer',
               test, '--', '--exact', '--ignored',
               '--nocapture', '--test-threads=1']
    with (folder / 'test.log').open('w') as log:
        process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT,
                                   start_new_session=True)
        timed_out = False
        try:
            process.wait(timeout=900)
        except subprocess.TimeoutExpired:
            timed_out = True
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
    log = (folder / 'test.log').read_text()
    result_path = folder / 'performance-result.json'
    result = json.loads(result_path.read_text()) if result_path.exists() else None
    binary = re.search(r'Running unittests .*?\((target/release/deps/[^)]+)\)', log)
    sources = ['crates/visualizer/src/marker_performance_tests.rs', 'crates/visualizer/src/frame_trace.rs', 'crates/visualizer/src/main.rs',
               'crates/visualization/src/spatial/marker_primitives/storage.rs',
               'crates/visualizer/src/marker_display.rs', 'crates/visualization/src/spatial/gpu.rs',
               'crates/visualization/src/spatial/marker_worker.rs', 'crates/visualization/src/spatial/marker_worker/preparation.rs',
               'crates/visualization/src/spatial/marker_primitives.rs', 'scripts/test-marker-app-load.py',
               'crates/visualizer/src/performance_support.rs',
               'Cargo.toml', 'crates/protocol/Cargo.toml', 'Cargo.lock', 'pixi.lock']
    if args.uncertainty:
        sources += ['crates/visualizer/src/uncertainty_performance_tests.rs',
                    'crates/visualizer/src/uncertainty_display.rs', 'crates/client-core/src/backend_pipe/pose_pipe.rs',
                    'crates/visualization/src/spatial/uncertainty.rs']
    if args.camera:
        sources += ['crates/visualizer/src/camera_performance_tests.rs',
                    'crates/visualizer/src/camera_display.rs', 'crates/visualizer/src/ui.rs',
                    'crates/protocol/src/camera_info.rs', 'crates/protocol/src/camera_snapshot.rs',
                    'crates/protocol/src/camera_snapshot/store.rs', 'crates/protocol/src/camera_snapshot/transport.rs',
                    'crates/client-core/src/camera_stream.rs', 'crates/client-core/src/backend_pipe/camera_pipe.rs',
                    'crates/visualization/src/spatial/calibrated_camera.rs',
                    'crates/visualization/src/spatial/calibrated_view.rs']
    versions = dict(rust=subprocess.check_output(['rustc', '--version'], text=True).strip(),
                    python=platform.python_version(), eframe='0.31.1', wgpu='24.0.5', session=SESSION_VERSION,
                    marker_snapshot=1, pose_snapshot=1, visualizer_workspace=17,
                    source_sha256={p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in sources})
    if args.camera:
        versions['camera_snapshot'] = 1
    if binary:
        versions['test_binary'] = binary.group(1)
        versions['test_binary_sha256'] = hashlib.sha256((ROOT / binary.group(1)).read_bytes()).hexdigest()
    context = dict(os=platform.platform(), cpu=platform.machine(), distro='jazzy',
                   middleware_pair=['protocol ingress fixture (no ROS)', 'native Metal App/egui'], versions=versions)
    passed = process.returncode == 0 and not timed_out and result and result['outcome'] == 'passed'
    pids = [process.pid] + ([result['test_pid']] if result else [])
    cleanup = subprocess.run(['ps', '-p', ','.join(map(str, pids)), '-o', 'pid=,ppid=,comm='],
                             capture_output=True, text=True)
    cleaned = cleanup.returncode == 1 and not cleanup.stdout.strip()
    rows = [dict(context, test='actual-App-egui-Metal-maximum-load', outcome='passed' if passed else 'failed',
                 command=command, exit_code=process.returncode, timed_out=timed_out, actual=result,
                 evidence=[str(p.relative_to(ROOT)) for p in folder.iterdir() if p.is_file()],
                 scope='Actual App frame/client assembly/workers/egui/Metal in one isolated test process; excludes OS window/input-to-present, ROS/network and other spatial arrangements.'),
            dict(context, test='recorded-performance-process-cleanup', outcome='passed' if cleaned else 'failed',
                 command=cleanup.args, pids=pids, actual=dict(returncode=cleanup.returncode, output=cleanup.stdout))]
    (ROOT / 'artifacts/feasibility' / (args.label + '.json')).write_text(json.dumps(rows, indent=2) + '\n')
    print(rows[0]['outcome'], 'cleanup', cleaned, flush=True)
    raise SystemExit(int(not passed or not cleaned))


if __name__ == '__main__':
    main()
