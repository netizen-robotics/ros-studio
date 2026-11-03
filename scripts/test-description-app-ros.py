"""Actual Visualizer-root Robot description onboarding with isolated ROS peers.

Use the project Pixi Python through scripts/dev. No native window/hardware claim.
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
import time
import traceback

ROOT = Path(__file__).resolve().parents[1]
RMWS = ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']
REMOTE = ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI',
          'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']
PREFIX = '/studio_fixture/description_app/'


def publisher(out):
    if (os.environ.get('ROS_DOMAIN_ID') != '187' or os.environ.get('ROS_LOCALHOST_ONLY') != '1'
            or os.environ.get('ROS_AUTOMATIC_DISCOVERY_RANGE') != 'LOCALHOST'
            or os.environ.get('RMW_IMPLEMENTATION') not in RMWS or any(os.environ.get(k) for k in REMOTE)):
        raise SystemExit('Requires matching RMW, localhost domain187 and no custom/remote discovery')
    import rclpy
    from rclpy.signals import SignalHandlerOptions
    from rclpy.qos import QoSProfile, DurabilityPolicy, ReliabilityPolicy
    from std_msgs.msg import String
    from sensor_msgs.msg import Image, LaserScan
    from nav_msgs.msg import OccupancyGrid
    from geometry_msgs.msg import TransformStamped
    from tf2_msgs.msg import TFMessage
    running = True

    def stop(*_):
        nonlocal running
        running = False
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    text = (ROOT / 'fixtures/robot_visualizer/topic.urdf').read_text()
    padded = text.replace('</robot>', '<!--' + 'x' * (1048576 - len(text.encode()) - 7) + '--></robot>')
    assert len(padded.encode()) == 1048576
    rclpy.init(signal_handler_options=SignalHandlerOptions.NO)
    node = rclpy.create_node('description_app_fixture')
    retained = QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL)
    writers = {key: node.create_publisher(String, PREFIX + key, retained)
               for key in ['retained', 'bad_xml', 'oversize', 'assets', 'obj', 'collada', 'rich', 'collision']}
    best = node.create_publisher(String, PREFIX + 'best', QoSProfile(depth=1, reliability=ReliabilityPolicy.BEST_EFFORT))
    image = node.create_publisher(Image, PREFIX + 'image', 1)
    scan = node.create_publisher(LaserScan, PREFIX + 'scan', retained)
    grid = node.create_publisher(OccupancyGrid, PREFIX + 'map', retained)
    dynamic = node.create_publisher(TFMessage, '/tf', 10)
    static = node.create_publisher(TFMessage, '/tf_static', retained)
    writers['retained'].publish(String(data=padded))
    writers['bad_xml'].publish(String(data='<robot name="broken"><link'))
    writers['oversize'].publish(String(data=padded + 'x'))
    writers['assets'].publish(String(data=(ROOT / 'fixtures/robot_visualizer/articulated.urdf').read_text()))

    for key in ['obj','collada','rich','collision']:
        writers[key].publish(String(data=(ROOT / f'fixtures/robot_visualizer/{key}.urdf').read_text()))

    def transform(parent, child, xyz):
        value = TransformStamped()
        value.header.frame_id = parent
        value.child_frame_id = child
        value.transform.translation.x, value.transform.translation.y, value.transform.translation.z = xyz
        value.transform.rotation.w = 1.0
        return value
    static.publish(TFMessage(transforms=[transform('base', 'arm', [0., 0., .2]),
                                        transform('arm', 'tip', [0., 0., .6]),
                                        transform('base', 'ascii_mesh', [0., 0., 0.]),
                                        transform('base', 'binary_mesh', [0., 0., 0.])]))
    base = transform('map', 'base', [1., 0., 0.])
    out.write_text(json.dumps(dict(pid=os.getpid(), domain=187, rmw=os.environ['RMW_IMPLEMENTATION'], retained_bytes=1048576)) + '\n')
    try:
        while running:
            base.header.stamp = node.get_clock().now().to_msg()
            dynamic.publish(TFMessage(transforms=[base]))
            best.publish(String(data=text))
            stamp=rclpy.time.Time(nanoseconds=node.get_clock().now().nanoseconds-100_000_000).to_msg()
            rays=LaserScan();rays.header.frame_id='base';rays.header.stamp=stamp
            rays.angle_min=0.;rays.angle_max=0.;rays.angle_increment=1.;rays.range_min=.1;rays.range_max=10.;rays.ranges=[1.]
            scan.publish(rays)
            cells=OccupancyGrid();cells.header.frame_id='base';cells.header.stamp=stamp
            cells.info.width=1;cells.info.height=1;cells.info.resolution=1.;cells.info.origin.orientation.w=1.;cells.data=[100]
            grid.publish(cells)
            image.publish(Image(height=2, width=2, encoding='rgb8', step=6,
                                data=[255, 40, 40, 40, 220, 40, 40, 40, 255, 230, 220, 40]))
            rclpy.spin_once(node, timeout_sec=.05)
    finally:
        node.destroy_node()
        if rclpy.ok():
            rclpy.shutdown()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label')
    parser.add_argument('--publisher', type=Path)
    args = parser.parse_args()
    if Path(sys.prefix).resolve() != (ROOT / '.pixi/envs/default').resolve():
        parser.error('Use the project Pixi interpreter')
    if args.publisher:
        if not args.publisher.resolve().is_relative_to(ROOT / 'artifacts/feasibility/evidence'):
            parser.error('Publisher evidence must stay in ignored artifacts')
        publisher(args.publisher)
        return
    if not args.label or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_' for c in args.label):
        parser.error('Choose an unused alphanumeric evidence label')
    out = ROOT / 'artifacts/feasibility/evidence' / args.label
    report = ROOT / 'artifacts/feasibility' / (args.label + '.json')
    if out.exists() or report.exists():
        parser.error('Preserve earlier evidence; choose an unused label')
    out.mkdir(parents=True)
    sha = lambda path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest()
    sources = ['scripts/test-description-app-ros.py', 'scripts/ui-ros-workflow.py', 'scripts/ui-ros-backend',
               'crates/visualizer/src/robot_setup.rs', 'crates/visualizer/src/robot_setup/ros_tests.rs',
               'crates/visualizer/src/main.rs', 'crates/visualizer/src/workspace_app.rs',
               'crates/client-core/src/backend_pipe/description_pipe.rs', 'crates/backend/src/description_worker.rs',
               'crates/protocol/src/robot_description.rs', 'crates/workspace/src/application_v11.rs',
               'fixtures/robot_visualizer/topic.urdf', 'fixtures/robot_visualizer/articulated.urdf']
    sources += [str(p.relative_to(ROOT)) for base in ['crates/visualization/src/robot','crates/visualization/src/spatial','crates/workspace/src','crates/visualizer/src','fixtures/robot_visualizer'] for p in (ROOT/base).rglob('*') if p.is_file()]
    source_hashes = {path: sha(path) for path in sources}
    versions = dict(rust=subprocess.check_output(['rustc', '--version'], text=True).strip(),
                    python=platform.python_version(), eframe='0.31.1', rclrs='0.8.0',
                    backend_sha256=sha('target/debug/studio-backend'),
                    cargo_lock_sha256=sha('Cargo.lock'), pixi_lock_sha256=sha('pixi.lock'))
    rows = []
    for rmw in RMWS:
        folder = out / rmw
        folder.mkdir()
        env = {key: value for key, value in os.environ.items() if key not in REMOTE}
        env.update(ROS_DISTRO='jazzy', ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1',
                   ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST', RMW_IMPLEMENTATION=rmw,
                   STUDIO_BACKEND=str(ROOT / 'scripts/ui-ros-backend'), STUDIO_UI_ROS_EVIDENCE=str(folder))
        ready = folder / 'publisher.json'
        publisher_command = [sys.executable, str(Path(__file__).resolve()), '--publisher', str(ready)]
        command = ['cargo', 'test', '--locked', '-p', 'studio-visualizer', '--bin', 'studio-visualizer',
                   'description_real_ros_visualizer_workflow', '--', '--ignored', '--nocapture', '--test-threads=1']
        peer = None
        outcome, details, code = 'passed', None, None
        log = folder / 'root.log'
        try:
            with (folder / 'publisher.log').open('w') as output:
                peer = subprocess.Popen(publisher_command, cwd=ROOT, env=env, stdout=output, stderr=subprocess.STDOUT)
            deadline = time.monotonic() + 30
            while not ready.exists():
                assert peer.poll() is None and time.monotonic() < deadline, 'Publisher startup failed'
                time.sleep(.05)
            with log.open('w') as output:
                test = subprocess.Popen(command, cwd=ROOT, env=env, stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
                try:
                    code = test.wait(timeout=240)
                except subprocess.TimeoutExpired:
                    # This isolated group contains only the recorded test job
                    # and its children, never an unrelated user ROS process.
                    os.killpg(test.pid, signal.SIGTERM)
                    try:
                        test.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        os.killpg(test.pid, signal.SIGKILL)
                        test.wait()
                    raise
            assert code == 0, 'Actual-root test failed; inspect root.log'
        except Exception:
            outcome, details = 'failed', traceback.format_exc()
        finally:
            if peer is not None:
                if peer.poll() is None:
                    peer.send_signal(signal.SIGTERM)
                try:
                    peer.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    peer.kill()
                    peer.wait()
                    outcome, details = 'failed', (details or '') + 'Publisher required forced cleanup'
            launches = folder / 'backend-launches.jsonl'
            records = [json.loads(line) for line in launches.read_text().splitlines()] if launches.exists() else []
            cleanup = []
            for pid in [*([peer.pid] if peer else []), *[row['pid'] for row in records]]:
                try:
                    os.kill(pid, 0)
                    gone = False
                except ProcessLookupError:
                    gone = True
                cleanup.append(dict(pid=pid, absent=gone))
            if not all(row['absent'] for row in cleanup):
                outcome = 'failed'
            (folder / 'cleanup.json').write_text(json.dumps(cleanup, indent=2) + '\n')
            content = log.read_text() if log.exists() else ''
            test_versions = dict(versions)
            binary = re.search(r'Running unittests .*?\((target/debug/deps/[^)]+)\)', content)
            if binary:
                test_versions['root_test_binary'] = binary.group(1)
                test_versions['root_test_sha256'] = sha(binary.group(1))
            rows.append(dict(test='description-actual-visualizer-root', outcome=outcome, os=platform.platform(),
                             cpu=platform.machine(), distro='jazzy', middleware_pair=[rmw, rmw], versions=test_versions,
                             source_sha256=source_hashes, command=['scripts/dev', '.pixi/envs/default/bin/python', 'scripts/test-description-app-ros.py', '--label', args.label],
                             root_command=command, publisher_command=publisher_command, exit_code=code,
                             environment={key: env[key] for key in ['ROS_DOMAIN_ID', 'ROS_LOCALHOST_ONLY', 'ROS_AUTOMATIC_DISCOVERY_RANGE', 'RMW_IMPLEMENTATION', 'STUDIO_BACKEND']},
                             backend_launches=records, cleanup=cleanup, publisher_exit_code=None if peer is None else peer.returncode,
                             completed_cases=re.findall(r'^CASE passed: (.+)$', content, flags=re.M),
                             measurements=re.findall(r'^MEASURE (.+)$', content, flags=re.M), details=details,
                             evidence=[str(p.relative_to(ROOT)) for p in sorted(folder.iterdir())],
                             scope='Actual Visualizer state/controller/asset worker and standard Rust ROS backend, local synthetic publishers; no native-window, LAN, other-platform or hardware acceptance.'))
            report.write_text(json.dumps(rows, indent=2) + '\n')
            print(rmw, outcome, len(rows[-1]['completed_cases']), 'cases', flush=True)
    if any(row['outcome'] != 'passed' for row in rows):
        raise SystemExit('Actual description root workflow failed; preserve evidence')


if __name__ == '__main__':
    main()
