"""Serialize isolated real ROS pose or calibrated-Camera qualification."""
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
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
REMOTE = ('ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT',
          'CYCLONEDDS_URI', 'FASTDDS_DEFAULT_PROFILES_FILE',
          'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', required=True)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--session', action='store_true', help='Exercise paired backend/default-client/ROS instead of standalone reader')
    mode.add_argument('--application', action='store_true', help='Exercise actual Visualizer root and prepared geometry')
    parser.add_argument('--camera', action='store_true', help='Use Camera fixture and actual-root matrix; requires --application')
    args = parser.parse_args()
    if args.camera and not args.application:
        parser.error('--camera requires --application')
    if Path(sys.prefix).resolve() != (ROOT / '.pixi/envs/default').resolve():
        parser.error('Use project Pixi Python')
    if not re.fullmatch(r'[A-Za-z0-9_-]+', args.label):
        parser.error('Use an alphanumeric label')
    folder = ROOT / 'artifacts/feasibility/evidence' / args.label
    folder.mkdir(exist_ok=False)
    if args.session or args.application:
        with (folder / 'build.log').open('w') as output:
            commands = [['cargo', 'build', '--locked', '-p', 'studio-backend', '--bin', 'studio-backend']]
            if args.session:
                commands.append(['cargo', 'build', '--locked', '-p', 'studio-client-core', '--features', 'test-support', '--example', 'pose-session'])
            for command in commands:
                subprocess.run(command, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT, check=True)
    sources = ['crates/protocol/src/pose_snapshot.rs',
               'crates/protocol/src/pose_snapshot/transport.rs',
               'crates/protocol/src/pose_snapshot/store.rs',
               'crates/backend/src/pose_codec.rs', 'crates/backend/src/pose_producer.rs',
               'crates/backend/src/native_topic.rs', 'crates/backend/native/service_shim.c',
               'crates/backend/tests/native_pose_ros.rs',
               'crates/backend/tests/fixtures/pose_publisher.py',
               'scripts/test-pose-source.py', 'Cargo.lock', 'pixi.lock']
    if args.session:
        sources += ['crates/protocol/src/session.rs', 'crates/protocol/src/subscriptions/registry.rs',
                    'crates/backend/src/scene_worker.rs', 'crates/backend/src/scene_worker/pose_display.rs',
                    'crates/backend/src/observation.rs', 'crates/client-core/src/backend_pipe.rs',
                    'crates/client-core/src/backend_pipe/pose_pipe.rs', 'crates/client-core/examples/pose-session.rs',
                    'crates/client-core/Cargo.toml']
    if args.application:
        sources += ['crates/visualizer/src/uncertainty_ros_tests.rs', 'crates/visualizer/src/uncertainty_display.rs',
                    'crates/visualization/src/spatial/uncertainty.rs', 'crates/protocol/src/session.rs',
                    'crates/workspace/src/application_v16.rs', 'crates/client-core/src/backend_pipe/pose_pipe.rs']
    if args.camera:
        sources = ['crates/protocol/src/camera_info.rs', 'crates/protocol/src/camera_snapshot.rs',
                   'crates/protocol/src/camera_snapshot/store.rs', 'crates/protocol/src/camera_snapshot/transport.rs',
                   'crates/protocol/src/session.rs', 'crates/backend/src/camera_info_codec.rs',
                   'crates/backend/src/camera_producer.rs', 'crates/backend/src/scene_worker/camera_display.rs',
                   'crates/backend/src/native_topic.rs', 'crates/backend/native/service_shim.c',
                   'crates/backend/tests/fixtures/camera_publisher.py', 'crates/client-core/src/camera_stream.rs',
                   'crates/client-core/src/backend_pipe/camera_pipe.rs', 'crates/visualizer/src/camera_display.rs',
                   'crates/visualizer/src/camera_ros_tests.rs', 'crates/visualizer/src/main.rs',
                   'crates/visualization/src/spatial/calibrated_camera.rs', 'crates/workspace/src/application_v17.rs',
                   'scripts/test-pose-source.py', 'Cargo.lock', 'pixi.lock']
    versions = {'rust': subprocess.check_output(['rustc', '--version'], text=True).strip(),
                'python': platform.python_version(), 'session': SESSION_VERSION,
                'pose_snapshot': '1 (paired session)' if args.session else '1 (standalone reader check)',
                'native_packages': {name: ET.parse(ROOT / '.pixi/envs/default/share' / name / 'package.xml').getroot().findtext('version')
                                    for name in ('geometry_msgs', 'nav_msgs', 'builtin_interfaces', 'rosidl_typesupport_introspection_c')},
                'native_introspection_sha256': {name: hashlib.sha256((ROOT / '.pixi/envs/default/lib' / name).read_bytes()).hexdigest()
                                               for name in ('libgeometry_msgs__rosidl_typesupport_introspection_c.dylib',
                                                            'libnav_msgs__rosidl_typesupport_introspection_c.dylib')},
                'source_sha256': {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest()
                                  for p in sources}}
    if args.camera:
        versions.pop('pose_snapshot')
        versions['camera_snapshot'] = 1
        versions['visualizer_workspace'] = 17
        versions['tools_workspace'] = 8
        versions['native_packages']['sensor_msgs'] = ET.parse(
            ROOT / '.pixi/envs/default/share/sensor_msgs/package.xml').getroot().findtext('version')
        name = 'libsensor_msgs__rosidl_typesupport_introspection_c.dylib'
        versions['native_introspection_sha256'][name] = hashlib.sha256(
            (ROOT / '.pixi/envs/default/lib' / name).read_bytes()).hexdigest()
    rows = []
    for rmw in ('rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp'):
        log = folder / (rmw + '.log')
        env = {k: v for k, v in os.environ.items() if k not in REMOTE}
        env.update(ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1',
                   ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST', RMW_IMPLEMENTATION=rmw,
                   DYLD_LIBRARY_PATH=str(ROOT / '.pixi/envs/default/lib'))
        command = ['cargo', 'test', '--locked', '-p', 'studio-backend', '--test',
                   'native_pose_ros', '--', '--ignored', '--nocapture', '--test-threads=1']
        if args.session:
            session_folder = folder / rmw
            session_folder.mkdir()
            env['STUDIO_POSE_EVIDENCE'] = str(session_folder)
            command = [str(ROOT / 'target/debug/examples/pose-session')]
        if args.application:
            session_folder = folder / rmw
            session_folder.mkdir()
            env.pop('DYLD_LIBRARY_PATH', None)
            env.update(STUDIO_BACKEND=str(ROOT / 'scripts/ui-ros-backend'), STUDIO_UI_ROS_EVIDENCE=str(session_folder))
            command = ['cargo', 'test', '--locked', '-p', 'studio-visualizer', '--bin', 'studio-visualizer',
                       'uncertainty_ros_tests::uncertainty_real_ros_visualizer_workflow', '--', '--exact',
                       '--ignored', '--nocapture', '--test-threads=1']
        if args.camera:
            command[command.index('uncertainty_ros_tests::uncertainty_real_ros_visualizer_workflow')] = (
                'camera_ros_tests::camera_real_ros_visualizer_workflow')
        timed_out = False
        with log.open('w') as output:
            process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=output,
                                       stderr=subprocess.STDOUT, start_new_session=True)
            try:
                process.wait(timeout=180)
            except subprocess.TimeoutExpired:
                timed_out = True
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
        content = log.read_text()
        pids = [process.pid] + [int(p) for p in re.findall(r'(?:test|publisher|backend)_pid=(\d+)', content)]
        app_result = None
        if args.application:
            result_path = session_folder / 'result.json'
            app_result = json.loads(result_path.read_text()) if result_path.exists() else None
            ready_path = session_folder / 'publisher-ready.json'
            if ready_path.exists():
                pids.append(json.loads(ready_path.read_text())['pid'])
            launches = session_folder / 'backend-launches.jsonl'
            if launches.exists():
                pids.extend(json.loads(line)['pid'] for line in launches.read_text().splitlines())
            if app_result:
                pids.append(app_result['test_pid'])
        alive = []
        for pid in sorted(set(pids)):
            try:
                os.kill(pid, 0)
                alive.append(pid)
            except ProcessLookupError:
                pass
        binaries = re.findall(r'Running tests/[^\n]+\((target/debug/deps/[^)]+)\)', content)
        if args.session:
            binaries = ['target/debug/studio-backend', 'target/debug/examples/pose-session']
        if args.application:
            binaries = ['target/debug/studio-backend'] + re.findall(r'Running unittests .*?\((target/debug/deps/[^)]+)\)', content)
        binary_hashes = {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in binaries}
        passed = (not timed_out and process.returncode == 0 and not alive and
                  (bool(app_result and app_result['outcome'] == 'passed') if args.application else
                   'paired pose session passed' in content if args.session else
                   '1 passed; 0 failed' in content and 'native readers dropped' in content))
        row = dict(os=platform.platform(), cpu=platform.machine(), distro='jazzy',
                   versions=versions, middleware_pair=[rmw, rmw],
                   test='paired-pose-session' if args.session else 'native-pose-source-cache-receiver', outcome='passed' if passed else 'failed',
                   command=command, environment={k: env[k] for k in ('ROS_DOMAIN_ID',
                       'ROS_LOCALHOST_ONLY', 'ROS_AUTOMATIC_DISCOVERY_RANGE',
                       'RMW_IMPLEMENTATION', 'DYLD_LIBRARY_PATH') if k in env},
                   evidence=[str(log.relative_to(ROOT))], exit_code=process.returncode,
                   timed_out=timed_out, recorded_pids=sorted(set(pids)), alive=alive,
                   binary_sha256=binary_hashes,
                   scope='Real ROS retained source readers; native TF cache inserted directly; '
                         'snapshot/receiver/Clear and malformed-source separation. '
                         'No TF ROS delivery, paired observation session, App, native window, '
                         'GPU, sustained performance, LAN, packaging or hardware acceptance.')
        rows.append(row)
        if args.session:
            row['scope'] = (f'Actual standard ROS source/TF publishers, Rust backend/session{SESSION_VERSION}/shared actor and default client pipe. '
                            'Discovery without readers, explicit retained sources, QoS mismatch/correction, Clear/epoch permissions, '
                            'peer-scoped malformed source, Stop, inactive reconnect and active EOF cleanup. '
                            'No App/native window, GPU, sustained performance, LAN, packaging or hardware acceptance.')
        if args.application:
            row.update(test='actual-pose-visualizer-root', scope='Actual App/backend/ROS with prepared geometry; no native-window, GPU, sustained performance, LAN, packaging or hardware acceptance.',
                       application_result=app_result)
        if args.camera:
            row['test'] = 'actual-camera-visualizer-root'
            row['scope'] = ('Actual App/backend/ROS and prepared calibrated frame; retained paired sources/shared TF, '
                            'draft/replacement, invalidation/fresh Image, invalid/mismatched recovery, independent Info QoS, '
                            'Image-peer isolation, Stop/cancel/inactive reconnect. No native-window, GPU, load, LAN, packaging or hardware acceptance.')
        print(rmw, row['outcome'], 'alive', alive, flush=True)
    (ROOT / 'artifacts/feasibility' / (args.label + '.json')).write_text(json.dumps(rows, indent=2) + '\n')
    return 0 if all(r['outcome'] == 'passed' for r in rows) else 1


if __name__ == '__main__':
    raise SystemExit(main())
