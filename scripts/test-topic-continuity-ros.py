"""Serialized supported-display continuity audit with localhost synthetic ROS.

Run via scripts/dev and project Python. A fresh label preserves each attempt.
--kinds allows focused repairs without repeating unchanged broad checks.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import platform
import queue
import signal
import struct
import subprocess
import sys
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
RMWS = ('rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp')
REMOTE = ('ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT',
          'CYCLONEDDS_URI', 'FASTDDS_DEFAULT_PROFILES_FILE',
          'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE')
KINDS = ('scan', 'cloud', 'pose', 'path', 'odometry', 'covariance',
         'camera', 'marker', 'marker_array', 'map', 'image', 'messages', 'plot', 'tf', 'robot')


def image_payload(size, encoding):
    pixel = {'rgb8': b'\xff\x00\x00', '16UC1': struct.pack('<H', 1000), '32FC1': struct.pack('<f', 1.)}[encoding]
    return pixel * (size[0] * size[1])


def case_definition(node, kind, suffix, image_size=(2, 2), image_encoding='rgb8'):
    from nav_msgs.msg import OccupancyGrid, Odometry, Path as PathMessage
    from geometry_msgs.msg import PoseStamped, PoseWithCovarianceStamped, TransformStamped
    from sensor_msgs.msg import LaserScan, PointCloud2, PointField, Image, CameraInfo
    from std_msgs.msg import Int32
    from visualization_msgs.msg import Marker, MarkerArray
    from tf2_msgs.msg import TFMessage
    prefix = '/studio_fixture/continuity/case_' + suffix
    topic, frame = prefix + '/data', 'source_' + suffix
    tf_topic = prefix + '/tf'
    tf_pub = node.create_publisher(TFMessage, tf_topic, 10)
    message_type = {
        'scan': (LaserScan, 'sensor_msgs/msg/LaserScan', 'spatial_scan'),
        'cloud': (PointCloud2, 'sensor_msgs/msg/PointCloud2', 'point_cloud'),
        'pose': (PoseStamped, 'geometry_msgs/msg/PoseStamped', 'pose'),
        'path': (PathMessage, 'nav_msgs/msg/Path', 'path'),
        'odometry': (Odometry, 'nav_msgs/msg/Odometry', {'pose_snapshot': {'kind': 'odometry', 'history': {'keep': 100, 'position_tolerance_um': 100000, 'angle_tolerance_urad': 100000}}}),
        'covariance': (PoseWithCovarianceStamped, 'geometry_msgs/msg/PoseWithCovarianceStamped', {'pose_snapshot': {'kind': 'pose_with_covariance', 'history': None}}),
        'camera': (Image, 'sensor_msgs/msg/Image', {'camera_snapshot': {'topic': prefix + '/info', 'best_effort': False, 'transient_local': False}}),
        'marker': (Marker, 'visualization_msgs/msg/Marker', 'marker_snapshot'),
        'marker_array': (MarkerArray, 'visualization_msgs/msg/MarkerArray', 'marker_snapshot'),
        'map': (OccupancyGrid, 'nav_msgs/msg/OccupancyGrid', 'map_snapshot'),
        'image': (Image, 'sensor_msgs/msg/Image', 'compact_image'),
        'messages': (Int32, 'std_msgs/msg/Int32', 'typed'),
        'plot': (Int32, 'std_msgs/msg/Int32', 'typed'),
        'tf': (TFMessage, 'tf2_msgs/msg/TFMessage', 'tf_graph'),
        'robot': (TFMessage, 'tf2_msgs/msg/TFMessage', 'robot'),
    }[kind]
    cls, exact, output = message_type
    latest = kind in ('tf', 'robot')
    publisher = tf_pub if latest else node.create_publisher(cls, topic, 2)
    spatial = dict(fixed_frame='world', clock_policy='verified_system', dynamic_tf_topic=tf_topic,
                   dynamic_tf_best_effort=False, static_tf_topic=prefix + '/tf_static')
    config = dict(topic=tf_topic if latest else topic, message_type=exact,
                  best_effort=False, transient_local=False, output=output,
                  spatial=None if kind in ('image', 'messages', 'plot', 'robot') else spatial,
                  robot=dict(spatial, frames=[frame]) if kind == 'robot' else None)
    def stamp(message, ns):
        message.header.frame_id = frame
        message.header.stamp.sec, message.header.stamp.nanosec = divmod(ns, 1_000_000_000)
    def publish_tf(ns, base):
        message = TransformStamped()
        stamp(message, ns)
        message.header.frame_id = 'world'
        message.child_frame_id = frame
        message.transform.rotation.w = 1.
        message.transform.translation.x = (ns - base) / 1_000_000_000
        tf_pub.publish(TFMessage(transforms=[message]))
    info_pub = node.create_publisher(CameraInfo, prefix + '/info', 2) if kind == 'camera' else None
    def publish_info():
        info = CameraInfo(width=image_size[0], height=image_size[1], distortion_model='plumb_bob')
        info.header.frame_id = frame
        info.d = [0.] * 5
        info.k = [100.,0.,1.,0.,100.,1.,0.,0.,1.]
        info.r = [1.,0.,0.,0.,1.,0.,0.,0.,1.]
        info.p = [100.,0.,1.,0.,0.,100.,1.,0.,0.,0.,1.,0.]
        info_pub.publish(info)
    def publish(ns):
        if latest:
            return
        message = cls()
        if kind not in ('messages', 'plot', 'marker_array'):
            stamp(message, ns)
        if kind == 'scan':
            message.angle_min, message.angle_max, message.angle_increment = 0., .2, .1
            message.range_min, message.range_max, message.ranges = .1, 10., [1.,2.,3.]
        elif kind == 'cloud':
            message.width, message.height = 1, 1
            message.point_step, message.row_step, message.is_dense = 12, 12, True
            message.fields = [PointField(name=name, offset=i*4, datatype=7, count=1) for i,name in enumerate(('x','y','z'))]
            message.data = struct.pack('<fff', 1., 2., 0.)
        elif kind == 'pose':
            message.pose.orientation.w = 1.
        elif kind == 'path':
            pose = PoseStamped()
            stamp(pose, ns)
            pose.pose.orientation.w = 1.
            message.poses = [pose]
        elif kind in ('odometry', 'covariance'):
            message.pose.pose.orientation.w = 1.
            message.pose.covariance = [0.1 if i//6 == i%6 else 0. for i in range(36)]
            if kind == 'odometry':
                message.child_frame_id = 'body'
        elif kind in ('image', 'camera'):
            message.width, message.height = image_size
            message.encoding = image_encoding
            message.step = message.width * {'rgb8': 3, '16UC1': 2, '32FC1': 4}[image_encoding]
            message.data = image_payload(image_size, image_encoding)
        elif kind in ('marker', 'marker_array'):
            marker = message if kind == 'marker' else Marker()
            stamp(marker, ns)
            marker.ns, marker.id, marker.type, marker.action = 'fixture', 0, 1, 0
            marker.pose.orientation.w = 1.
            marker.scale.x = marker.scale.y = marker.scale.z = 1.
            marker.color.r = marker.color.a = 1.
            if kind == 'marker_array':
                message.markers = [marker]
        elif kind == 'map':
            message.info.width = message.info.height = 2
            message.info.resolution, message.info.origin.orientation.w = .5, 1.
            message.data = [0,20,40,100]
        else:
            message.data = int(ns % 1000)
        publisher.publish(message)
    return config, publisher, tf_pub, info_pub, publish, publish_tf, publish_info


def state(event, kind):
    sample = event.get('sample', {})
    if kind in ('scan', 'cloud', 'pose', 'path'):
        return sample.get('tf', {}).get('status') == 'ready', sample.get('acquisition_stamp_ns')
    if kind in ('odometry', 'covariance'):
        snapshot = sample.get('sample', {})
        entry = snapshot.get('latest') or {}
        return entry.get('transform', {}).get('status') == 'ready', entry.get('acquisition_stamp_ns')
    if kind == 'camera':
        snapshot = sample.get('sample', {})
        frame = snapshot.get('frame') or {}
        image = frame.get('image', {}).get('metadata', {}).get('header_stamp', {})
        ns = image.get('seconds', 0)*1_000_000_000 + image.get('nanoseconds', 0)
        return snapshot.get('status') == 'receiving' and bool(frame), ns
    if kind == 'map':
        return sample.get('kind') == 'chunk', sample.get('header', {}).get('metadata', {}).get('stamp_ns')
    if kind in ('marker', 'marker_array'):
        if sample.get('kind') != 'chunk':
            return False, None
        document = json.loads(base64.b64decode(sample['data_base64']))
        entries = document.get('entries', [])
        return any((entry.get('transform') or {}).get('pose', {}).get('status') == 'ready' for entry in entries), entries[0]['marker']['stamp_ns'] if entries else None
    if kind == 'robot':
        return any(p.get('pose', {}).get('status') == 'ready' for p in sample.get('links', [])), None
    if kind == 'tf':
        return any(p.get('pose', {}).get('status') == 'ready' for p in sample.get('frames', [])), None
    return True, None


def run_case(folder, kinds, image_size=(2, 2), reception_only=False, image_encoding='rgb8'):
    assert os.environ['ROS_DOMAIN_ID'] == '187' and os.environ['ROS_LOCALHOST_ONLY'] == '1'
    assert os.environ['ROS_AUTOMATIC_DISCOVERY_RANGE'] == 'LOCALHOST'
    assert os.environ['RMW_IMPLEMENTATION'] in RMWS
    expected = folder / ('cyclone-loopback.xml' if os.environ['RMW_IMPLEMENTATION'] == 'rmw_cyclonedds_cpp' else 'fast-loopback.xml')
    profile_key = 'CYCLONEDDS_URI' if os.environ['RMW_IMPLEMENTATION'] == 'rmw_cyclonedds_cpp' else 'FASTDDS_DEFAULT_PROFILES_FILE'
    assert os.environ[profile_key] == str(expected)
    assert not any(os.environ.get(key) for key in REMOTE if key != profile_key)
    import rclpy
    rclpy.init()
    node = rclpy.create_node('studio_topic_continuity_fixture')
    events = queue.Queue(maxsize=1024)
    wire = (folder/'backend-events.jsonl').open('w')
    errors = (folder/'backend-stderr.log').open('w')
    child = subprocess.Popen([str(ROOT/'target/debug/studio-backend'), '--observe-session'], stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, stderr=errors, text=True)
    def read():
        for line in child.stdout:
            wire.write(line)
            wire.flush()
            events.put_nowait(json.loads(line))
    reader = threading.Thread(target=read, daemon=True)
    reader.start()
    def send(command):
        child.stdin.write(json.dumps(command)+'\n')
        child.stdin.flush()
    def wait(predicate, timeout=12):
        deadline = time.monotonic()+timeout
        while time.monotonic() < deadline:
            try:
                event = events.get(timeout=.05)
            except queue.Empty:
                continue
            if predicate(event):
                return event
        raise AssertionError('Backend event deadline exceeded')
    rows = []
    try:
        wait(lambda event: event.get('event') == 'ready')
        for index, kind in enumerate(kinds, 1):
            row = {'kind':kind, 'outcome':'failed'}
            rows.append(row)
            try:
                config, pub, tf_pub, info, publish, tf, publish_info = case_definition(node, kind, str(index), image_size, image_encoding)
                if reception_only and kind == 'camera':
                    config['spatial']['fixed_frame'] = 'source_' + str(index)
                send(dict(command='start', generation=1, request=index*2-1, subscription=index, configuration=config))
                wait(lambda e: e.get('event') == 'started' and e.get('subscription') == index)
                transformed = kind not in ('image','messages','plot')
                latest = kind in ('tf','robot')
                deadline = time.monotonic()+10
                while pub.get_subscription_count() != 1 or (transformed and tf_pub.get_subscription_count() != 1) or (info and info.get_subscription_count() != 1):
                    assert time.monotonic() < deadline, 'DDS matching deadline'
                    rclpy.spin_once(node, timeout_sec=.02)
                base = time.time_ns()
                if info:
                    publish_info()
                    wait(lambda e: e.get('subscription') == index and
                         e.get('sample', {}).get('sample', {}).get('status') == 'waiting_image')
                tf(base-300_000_000, base)
                tf(base, base)
                publish(base-150_000_000)
                relevant = lambda e: e.get('subscription') == index and 'sample' in e
                first = wait(lambda e: relevant(e) and state(e,kind)[0])
                if kind in ('image', 'camera'):
                    sample = first['sample'] if kind == 'image' else first['sample']['sample']['frame']['image']
                    assert (sample['metadata']['width'], sample['metadata']['height']) == tuple(image_size)
                    assert sample['contract_version'] == 2
                    assert sample['metadata']['encoding'] == image_encoding
                    assert sample['original_bytes'] == len(image_payload(image_size, image_encoding))
                    assert base64.b64decode(sample['data_base64']) == image_payload(image_size, image_encoding)
                    row['image_size'] = image_size
                    row['image_encoding'] = image_encoding
                replacement = base+300_000_000
                if not transformed or latest or reception_only:
                    if reception_only:
                        tf(base+600_000_000,base)
                    if latest:
                        tf(time.time_ns(),base)
                    else:
                        publish(replacement)
                    wait(lambda e: relevant(e) and state(e,kind)[0])
                    row['checks'] = ['backend sample/pose reception', 'replacement reception']
                else:
                    publish(replacement)
                    end = time.monotonic()+.15
                    while time.monotonic() < end:
                        try:
                            event = events.get(timeout=.02)
                        except queue.Empty:
                            continue
                        if relevant(event):
                            ready, stamp = state(event, kind)
                            assert ready, ('transient replacement hid valid display',event)
                            assert stamp != replacement, 'new data borrowed old TF'
                    tf(base+600_000_000,base)
                    updated = wait(lambda e: relevant(e) and state(e,kind) == (True,replacement))
                    limit = 2.7 if kind == 'camera' else 1.7
                    end = time.monotonic()+limit
                    hidden = None
                    n = 0
                    while time.monotonic() < end and hidden is None:
                        n += 1
                        publish(base+10_000_000_000+n*1_000_000)
                        until = time.monotonic()+.1
                        while time.monotonic() < until:
                            try:
                                event = events.get(timeout=.01)
                            except queue.Empty:
                                continue
                            if relevant(event) and not state(event,kind)[0]:
                                hidden = event
                                break
                    assert hidden is not None, 'continuous input extended old display indefinitely'
                    row['checks'] = ['initial reception', 'no transient blank during 150ms TF gap',
                        'new source stamp waits for exact acquisition TF', 'continuous input reaches bounded invalidation']
                send(dict(command='stop', generation=1, request=index*2, subscription=index))
                wait(lambda e: e.get('event') == 'stopped' and e.get('subscription') == index)
                row['checks'].append('explicit Stop cleanup acknowledged')
                row['outcome'] = 'passed'
                print(kind, 'passed', flush=True)
            except Exception as error:
                row['error'] = repr(error)
                print(kind, 'failed', repr(error), flush=True)
                try:
                    send(dict(command='stop', generation=1, request=index*2, subscription=index))
                    wait(lambda e: e.get('event') == 'stopped' and e.get('subscription') == index,3)
                except Exception:
                    break
            finally:
                (folder/'cases.json').write_text(json.dumps(rows,indent=2)+'\n')
    finally:
        child.stdin.close()
        try:
            child.wait(timeout=8)
        except subprocess.TimeoutExpired:
            child.kill();child.wait()
        reader.join(timeout=2)
        wire.close();errors.close()
        node.destroy_node();rclpy.shutdown()
        (folder/'result.json').write_text(json.dumps(dict(cases=rows,backend_pid=child.pid,publisher_pid=os.getpid(),backend_exit_code=child.returncode),indent=2)+'\n')
    assert len(rows) == len(kinds) and all(row['outcome']=='passed' for row in rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', required=True)
    parser.add_argument('--case', choices=RMWS)
    parser.add_argument('--kinds', nargs='+', choices=KINDS, default=list(KINDS))
    parser.add_argument('--image-size', nargs=2, type=int, default=[2, 2])
    parser.add_argument('--reception-only', action='store_true')
    parser.add_argument('--image-encoding', choices=['rgb8', '16UC1', '32FC1'], default='rgb8')
    args = parser.parse_args()
    assert Path(sys.prefix).resolve() == (ROOT/'.pixi/envs/default').resolve()
    assert args.label.replace('-','').replace('_','').isalnum()
    folder = ROOT/'artifacts/feasibility/evidence'/args.label
    if args.case:
        run_case(folder/args.case,args.kinds,args.image_size,args.reception_only,args.image_encoding)
        return
    folder.mkdir(parents=True,exist_ok=False)
    rows=[]
    for rmw in RMWS:
        case=folder/rmw;case.mkdir()
        env={k:v for k,v in os.environ.items() if k not in REMOTE}
        env.update(ROS_DOMAIN_ID='187',ROS_LOCALHOST_ONLY='1',ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST',
            SKIP_DEFAULT_XML='1',RMW_IMPLEMENTATION=rmw,DYLD_LIBRARY_PATH=str(ROOT/'.pixi/envs/default/lib'))
        if rmw == 'rmw_cyclonedds_cpp':
            profile = case/'cyclone-loopback.xml'
            profile.write_text('<CycloneDDS><Domain><General><Interfaces><NetworkInterface address="127.0.0.1"/></Interfaces></General></Domain></CycloneDDS>')
            env['CYCLONEDDS_URI'] = str(profile)
        else:
            profile = case/'fast-loopback.xml'
            profile.write_text('''<?xml version="1.0"?><profiles xmlns="http://www.eprosima.com/XMLSchemas/fastRTPS_Profiles">
<transport_descriptors><transport_descriptor><transport_id>fixture_loopback</transport_id><type>UDPv4</type>
<interfaceWhiteList><address>127.0.0.1</address></interfaceWhiteList></transport_descriptor></transport_descriptors>
<participant profile_name="fixture_loopback" is_default_profile="true"><rtps><useBuiltinTransports>false</useBuiltinTransports>
<userTransports><transport_id>fixture_loopback</transport_id></userTransports></rtps></participant></profiles>''')
            env['FASTDDS_DEFAULT_PROFILES_FILE'] = str(profile)
            env.pop('SKIP_DEFAULT_XML', None)
        command=[sys.executable,__file__,'--label',args.label,'--case',rmw,'--kinds',*args.kinds]
        command += ['--image-size', *map(str,args.image_size)]
        command += ['--image-encoding', args.image_encoding]
        if args.reception_only:
            command.append('--reception-only')
        timed_out=False
        with (case/'fixture.log').open('w') as log:
            process=subprocess.Popen(command,env=env,stdout=log,stderr=log,start_new_session=os.name=='posix')
            try:process.wait(timeout=240)
            except subprocess.TimeoutExpired:
                timed_out=True
                if os.name=='posix':os.killpg(process.pid,signal.SIGTERM)
                else:process.kill()
                try:process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    if os.name=='posix':os.killpg(process.pid,signal.SIGKILL)
                    else:process.kill()
                    process.wait()
        actual=json.loads((case/'result.json').read_text()) if (case/'result.json').exists() else None
        remaining=[]
        if actual:
            for pid in [actual['backend_pid'],actual['publisher_pid']]:
                try:os.kill(pid,0);remaining.append(pid)
                except ProcessLookupError:pass
        rows.append(dict(outcome='passed' if process.returncode==0 and not timed_out and not remaining else 'failed',
            os=platform.platform(),cpu=platform.machine(),distro='jazzy',middleware_pair=[rmw,rmw],command=command,
            versions=dict(python=platform.python_version(),rust=subprocess.check_output(['rustc','--version'],text=True).strip(),rclrs='0.8.0',protocol=21),
            hashes={name:hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in ['Cargo.lock','pixi.lock','target/debug/studio-backend']},
            actual=actual,remaining_pids=remaining,evidence=str(case.relative_to(ROOT)),
            scope='Actual backend and localhost synthetic publishers; no native/LAN/physical robot qualification'))
        print(rmw,rows[-1]['outcome'],flush=True)
    (ROOT/'artifacts/feasibility'/(args.label+'.json')).write_text(json.dumps(rows,indent=2)+'\n')
    assert all(row['outcome']=='passed' for row in rows)


if __name__=='__main__':main()
