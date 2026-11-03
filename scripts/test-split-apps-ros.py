"""Actual application-root acceptance with localhost synthetic ROS publishers.
Use scripts/dev .pixi/envs/default/bin/python; no physical robot peers or commands.
"""
import argparse
import hashlib
import json
import math
import os
import platform
import re
import signal
import struct
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RMWS = ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']
REMOTE = ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI',
          'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']


def publisher(out):
    if os.environ.get('ROS_DOMAIN_ID') != '187' or os.environ.get('ROS_LOCALHOST_ONLY') != '1' or os.environ.get('ROS_AUTOMATIC_DISCOVERY_RANGE') != 'LOCALHOST' or os.environ.get('RMW_IMPLEMENTATION') not in RMWS or any(os.environ.get(key) for key in REMOTE):
        raise SystemExit('Publisher requires explicit matching middleware and isolated localhost domain 187')
    import rclpy
    from rclpy.executors import ExternalShutdownException
    from rclpy.qos import QoSProfile, ReliabilityPolicy, DurabilityPolicy
    from std_msgs.msg import String, UInt64
    from sensor_msgs.msg import Image, LaserScan, PointCloud2, PointField
    from tf2_msgs.msg import TFMessage
    from geometry_msgs.msg import TransformStamped
    running = True

    def stop(*_):
        nonlocal running
        running = False
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    rclpy.init()
    node = rclpy.create_node('split_applications_fixture')
    reliable = node.create_publisher(String, '/studio_fixture/split/reliable', 10)
    best = node.create_publisher(String, '/studio_fixture/split/best', QoSProfile(depth=10, reliability=ReliabilityPolicy.BEST_EFFORT))
    retained = node.create_publisher(String, '/studio_fixture/split/retained', QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL))
    numeric = node.create_publisher(UInt64, '/studio_fixture/split/numeric', 10)
    image = node.create_publisher(Image, '/studio_fixture/split/image', 10)
    scan = node.create_publisher(LaserScan, '/studio_fixture/split/scan', 10)
    scan_best = node.create_publisher(LaserScan, '/studio_fixture/split/scan_best', QoSProfile(depth=10, reliability=ReliabilityPolicy.BEST_EFFORT))
    cloud = node.create_publisher(PointCloud2, '/studio_fixture/split/cloud', 10)
    tf = node.create_publisher(TFMessage, '/tf', 10)
    static = node.create_publisher(TFMessage, '/tf_static', QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL))
    transform = TransformStamped()
    transform.header.frame_id = 'split_world'
    transform.child_frame_id = 'split_laser'
    transform.transform.rotation.w = 1.0
    static.publish(TFMessage(transforms=[transform]))
    retained.publish(String(data='retained split fixture'))
    out.write_text(json.dumps({'pid': os.getpid(), 'rmw': os.environ['RMW_IMPLEMENTATION'], 'domain': 187, 'localhost_only': True}) + '\n')
    count = 0
    try:
        while running:
            stamp = node.get_clock().now().to_msg()
            reliable.publish(String(data=f'split fixture {count}'))
            best.publish(String(data=f'best effort {count}'))
            numeric.publish(UInt64(data=count))
            camera = Image(height=2, width=2, encoding='rgb8', step=6, data=[255, 50, 40, 40, 200, 60, 40, 60, 240, 240, 220, 50])
            camera.header.stamp = stamp
            image.publish(camera)
            laser = LaserScan(angle_min=-math.pi/2, angle_max=math.pi/2, angle_increment=math.pi/180,
                              time_increment=0.0, scan_time=0.2, range_min=0.1, range_max=10.0,
                              ranges=[2.0 + 0.1*math.sin(count*0.1)]*181)
            laser.header.frame_id = 'split_laser'
            laser.header.stamp = stamp
            laser.intensities = [float(i) for i in range(180)] + [float('nan')]
            scan.publish(laser)
            scan_best.publish(laser)
            points = [(0.,0.,0.),(1.,0.,0.),(0.,1.,0.),(0.,0.,1.),(1.,1.,2.)]
            colors = [0xfff04632,0xff3cc85a,0x804664e6,0xffe6be3c,0xffaa5adc]
            raw = b''.join(struct.pack('<fffIf',*p,colors[i],float(i)*25 if i<4 else float('nan')) for i,p in enumerate(points))
            frame = PointCloud2(height=1,width=5,point_step=20,row_step=100,is_bigendian=False,is_dense=True,
                fields=[PointField(name=name,offset=offset,datatype=datatype,count=1) for name,offset,datatype in [('x',0,7),('y',4,7),('z',8,7),('rgba',12,6),('intensity',16,7)]],data=raw)
            frame.header.frame_id,frame.header.stamp = 'split_laser',stamp
            cloud.publish(frame)
            dynamic = TransformStamped()
            dynamic.header.frame_id = 'split_world'
            dynamic.header.stamp = stamp
            dynamic.child_frame_id = 'split_base'
            dynamic.transform.rotation.w = 1.0
            tf.publish(TFMessage(transforms=[dynamic]))
            count += 1
            rclpy.spin_once(node, timeout_sec=0.2)
    except (KeyboardInterrupt, ExternalShutdownException):
        pass
    finally:
        node.destroy_node()
        if rclpy.ok():
            rclpy.shutdown()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', default='split-apps-ros-20261022')
    parser.add_argument('--publisher', type=Path)
    parser.add_argument('--app', choices=['all', 'tools', 'visualizer'], default='all')
    args = parser.parse_args()
    if Path(sys.prefix).resolve() != (ROOT / '.pixi/envs/default').resolve():
        parser.error('Use the project Pixi interpreter')
    if args.publisher:
        if not args.publisher.resolve().is_relative_to(ROOT / 'artifacts/feasibility/evidence'):
            parser.error('Publisher evidence must stay under ignored artifacts')
        publisher(args.publisher)
        return
    if not args.label or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_' for c in args.label):
        parser.error('Use an alphanumeric evidence label')
    out = ROOT / 'artifacts/feasibility/evidence' / args.label
    out.mkdir(parents=True, exist_ok=False)
    report = ROOT / 'artifacts/feasibility' / (args.label + '.json')
    versions = {'rust': subprocess.check_output(['rustc', '--version'], text=True).strip(),
                'eframe': '0.31.1', 'egui': '0.31.1', 'rclrs': '0.8.0',
                'backend_sha256': hashlib.sha256((ROOT / 'target/debug/studio-backend').read_bytes()).hexdigest(),
                'Cargo.lock': hashlib.sha256((ROOT / 'Cargo.lock').read_bytes()).hexdigest(),
                'pixi.lock': hashlib.sha256((ROOT / 'pixi.lock').read_bytes()).hexdigest()}
    rows = []
    for rmw in RMWS:
        env = {key: value for key, value in os.environ.items() if key not in REMOTE}
        env.update(ROS_DISTRO='jazzy', ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1',
                   ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST', RMW_IMPLEMENTATION=rmw,
                   STUDIO_BACKEND=str(ROOT / 'scripts/ui-ros-backend'), STUDIO_UI_ROS_EVIDENCE=str(out))
        ready = out / (rmw + '-publisher.json')
        publisher_command = [sys.executable, str(Path(__file__).resolve()), '--publisher', str(ready)]
        with (out / (rmw + '-publisher.log')).open('w') as stream:
            peer = subprocess.Popen(publisher_command, cwd=ROOT, env=env, stdout=stream, stderr=subprocess.STDOUT)
            try:
                deadline = time.monotonic() + 30
                while not ready.exists() and peer.poll() is None and time.monotonic() < deadline:
                    time.sleep(0.1)
                if not ready.exists():
                    raise RuntimeError('Synthetic publisher did not become ready')
                for app in (['tools', 'visualizer'] if args.app == 'all' else [args.app]):
                    command = ['cargo', 'test', '--locked', '-p', f'studio-{app}', '--bin', f'studio-{app}',
                               'split_real_ros', '--', '--ignored', '--nocapture', '--test-threads=1']
                    log = out / f'{rmw}-{app}.log'
                    with log.open('w') as output:
                        result = subprocess.run(command, cwd=ROOT, env=env, stdout=output, stderr=subprocess.STDOUT, timeout=180)
                    test_binary = re.search(r'Running unittests .*?\((target/debug/deps/[^)]+)\)', log.read_text())
                    test_versions = versions.copy()
                    if test_binary:
                        test_versions['app_test_binary'] = test_binary.group(1)
                        test_versions['app_test_sha256'] = hashlib.sha256((ROOT / test_binary.group(1)).read_bytes()).hexdigest()
                    rows.append({'test': f'{app}-actual-root-workflow', 'outcome': 'passed' if result.returncode == 0 else 'failed',
                                 'os': platform.platform(), 'cpu': platform.machine(), 'distro': 'jazzy',
                                 'middleware_pair': [rmw, rmw], 'versions': test_versions, 'command': command,
                                 'environment': {k: env[k] for k in ['ROS_DOMAIN_ID', 'ROS_LOCALHOST_ONLY', 'ROS_AUTOMATIC_DISCOVERY_RANGE', 'RMW_IMPLEMENTATION', 'STUDIO_BACKEND']},
                                 'exit_code': result.returncode, 'publisher_command': publisher_command, 'publisher_pid': peer.pid,
                                 'evidence': [str(log.relative_to(ROOT)), str(ready.relative_to(ROOT))],
                                 'scope': 'Actual app state and standard Rust/ROS backend with local synthetic publishers; no native window or hardware claim.'})
                    report.write_text(json.dumps(rows, indent=2) + '\n')
                    print(rmw, app, rows[-1]['outcome'], flush=True)
            finally:
                if peer.poll() is None:
                    peer.terminate()
                    try:
                        peer.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        peer.kill()
                        peer.wait(timeout=5)
                for row in rows:
                    if row['publisher_pid'] == peer.pid:
                        row['publisher_exit_code'] = peer.returncode
                rows.append({'test': 'publisher-cleanup', 'outcome': 'passed' if peer.returncode == 0 else 'failed', 'os': platform.platform(), 'cpu': platform.machine(), 'distro': 'jazzy', 'middleware_pair': [rmw, rmw], 'versions': versions.copy(), 'command': publisher_command, 'exit_code': peer.returncode, 'publisher_pid': peer.pid, 'evidence': [str((out / (rmw + '-publisher.log')).relative_to(ROOT))]})
                report.write_text(json.dumps(rows, indent=2) + '\n')
    raise SystemExit(int(any(row['outcome'] != 'passed' for row in rows)))


if __name__ == '__main__':
    main()
