"""Independent live ROS subscriber checks for the isolated Docker fixture."""
import json
import math
import os
import platform
import time
import xml.etree.ElementTree as ET

import rclpy
from nav_msgs.msg import Odometry
from rclpy.qos import DurabilityPolicy, QoSProfile, qos_profile_sensor_data
from sensor_msgs.msg import Image, JointState, LaserScan
from std_msgs.msg import String
from tf2_msgs.msg import TFMessage
import model


def key(stamp):
    return stamp.sec, stamp.nanosec


def main():
    if os.environ.get('ROS_DOMAIN_ID') != '187':
        raise SystemExit('Checker requires isolated fixture domain 187')
    rclpy.init()
    node = rclpy.create_node('simple_sim_checker')
    samples = {name: [] for name in ['odometry', 'joint_states', 'scan', 'camera', 'status', 'tf', 'tf_static', 'robot_description']}
    subscriptions = []
    retained = QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL)
    for name, typ, topic, qos in [
        ('odometry', Odometry, '/studio_sim/odometry', 10),
        ('joint_states', JointState, '/studio_sim/joint_states', 10),
        ('scan', LaserScan, '/studio_sim/scan', qos_profile_sensor_data),
        ('camera', Image, '/studio_sim/camera', qos_profile_sensor_data),
        ('status', String, '/studio_sim/status', 10),
        ('tf', TFMessage, '/tf', 10), ('tf_static', TFMessage, '/tf_static', retained),
        ('robot_description', String, '/studio_sim/robot_description', retained),
    ]:
        def receive(message, name=name):
            if len(samples[name]) < 300:
                samples[name].append(message)
        subscriptions.append(node.create_subscription(typ, topic, receive, qos))
    started = time.monotonic()
    while time.monotonic() - started < 12:
        rclpy.spin_once(node, timeout_sec=0.1)
        if all(samples.values()) and len(samples['odometry']) >= 30 and len(samples['scan']) >= 10 and len(samples['camera']) >= 10 and len(samples['status']) >= 3:
            break
    results = []

    def record(name, passed, detail):
        results.append({'test': name, 'outcome': 'passed' if passed else 'failed', 'detail': detail})

    record('all-eight-streams', all(samples.values()), {name: len(items) for name, items in samples.items()})
    odoms = samples['odometry']
    if odoms:
        points = [(item.pose.pose.position.x, item.pose.pose.position.y) for item in odoms]
        moved = math.dist(points[0], points[-1])
        stamps = [key(item.header.stamp) for item in odoms]
        record('changing-pose-and-time', moved > 0.05 and all(a < b for a, b in zip(stamps, stamps[1:])),
               {'distance': moved, 'first': points[0], 'last': points[-1], 'first_stamp': stamps[0], 'last_stamp': stamps[-1]})
        record('odom-frames-and-unit-quaternion', all(
            item.header.frame_id == 'studio_sim/odom' and item.child_frame_id == 'studio_sim/base_footprint'
            and abs(item.pose.pose.orientation.z ** 2 + item.pose.pose.orientation.w ** 2 - 1) < 1e-6 for item in odoms), len(odoms))
    pairs = {key(item.header.stamp): item for item in odoms}
    scans_checked = 0
    scans_ok = True
    for laser in samples['scan']:
        odom = pairs.get(key(laser.header.stamp))
        if odom is None:
            continue
        scans_checked += 1
        q = odom.pose.pose.orientation
        yaw = 2 * math.atan2(q.z, q.w)
        for i, distance in enumerate(laser.ranges):
            angle = yaw + laser.angle_min + i * laser.angle_increment
            x = odom.pose.pose.position.x + distance * math.cos(angle)
            y = odom.pose.pose.position.y + distance * math.sin(angle)
            scans_ok &= math.isfinite(distance) and laser.range_min < distance < laser.range_max
            scans_ok &= abs(max(abs(x), abs(y)) - 3) < 1e-4
        scans_ok &= len(laser.ranges) == 180 and laser.header.frame_id == 'studio_sim/laser'
    record('scan-rays-hit-room-walls', scans_checked >= 5 and scans_ok, {'matched_scans': scans_checked})
    camera_ok = len(samples['camera']) >= 10
    camera_numbers = []
    for image in samples['camera']:
        raw = bytes(image.data)
        if image.encoding != model.CAMERA_ENCODING or image.width != model.CAMERA_WIDTH or image.height != model.CAMERA_HEIGHT or len(raw) < 24:
            camera_ok = False
            continue
        number = int.from_bytes(bytes(raw[index * 3] for index in range(8)), 'little')
        expected_frame = model.camera(number)
        camera_numbers.append(number)
        camera_ok &= image.header.frame_id == model.CAMERA_FRAME and image.is_bigendian == 0
        camera_ok &= image.step == expected_frame.step and raw == expected_frame.data
        camera_ok &= key(image.header.stamp) > (0, 0)
    camera_ok &= len(camera_numbers) >= 10 and all(a < b for a, b in zip(camera_numbers, camera_numbers[1:]))
    record('camera-colors-orientation-stride-and-frame-counter', camera_ok,
           {'frames': len(camera_numbers), 'first': camera_numbers[0] if camera_numbers else None,
            'last': camera_numbers[-1] if camera_numbers else None, 'raw_bytes': 64 * 48 * 3,
            'encoding': model.CAMERA_ENCODING, 'scope': 'CDR ROS Image data, before app rendering'})
    tf_checked = 0
    tf_ok = True
    for message in samples['tf']:
        for item in message.transforms:
            if item.child_frame_id != 'studio_sim/base_footprint':
                continue
            odom = pairs.get(key(item.header.stamp))
            if odom:
                tf_checked += 1
                tf_ok &= item.header.frame_id == odom.header.frame_id and item.transform.rotation == odom.pose.pose.orientation
                tf_ok &= item.transform.translation.x == odom.pose.pose.position.x and item.transform.translation.y == odom.pose.pose.position.y
    record('tf-agrees-with-odometry', tf_checked >= 10 and tf_ok, {'matched_transforms': tf_checked})
    joints = samples['joint_states']
    joint_ok = len(joints) > 1
    for item in joints:
        joint_ok &= len(item.name) == len(set(item.name)) == len(item.position) == len(item.velocity) == 4
        joint_ok &= all(math.isfinite(v) for v in [*item.position, *item.velocity])
        joint_ok &= item.velocity[0] == item.velocity[1] < item.velocity[2] == item.velocity[3]
        joint_ok &= abs((item.velocity[0] + item.velocity[2]) * 0.0363 / 2 - 0.144) < 1e-6
    if len(joints) > 1:
        joint_ok &= joints[-1].position != joints[0].position
    record('four-moving-differential-wheels', joint_ok, {'samples': len(joints)})
    retained_ok = False
    if samples['robot_description'] and samples['tf_static']:
        root = ET.fromstring(samples['robot_description'][0].data)
        names = {joint.attrib['name'] for joint in root.findall('joint') if joint.attrib['type'] == 'continuous'}
        children = {item.child_frame_id for item in samples['tf_static'][0].transforms}
        retained_ok = len(names) == 4 and children == {'studio_sim/base_link', 'studio_sim/laser'}
    record('late-join-retained-description-and-static-tf', retained_ok, 'Published only at simulator startup before this checker joined')
    statuses = [json.loads(item.data) for item in samples['status']]
    record('explicit-simulation-status', len(statuses) >= 3 and all(item.get('simulation') is True for item in statuses)
           and statuses[-1]['sample'] > statuses[0]['sample'], statuses[-1] if statuses else None)
    expected = {'/studio_sim/odometry': 'nav_msgs/msg/Odometry', '/studio_sim/scan': 'sensor_msgs/msg/LaserScan',
                '/studio_sim/joint_states': 'sensor_msgs/msg/JointState', '/studio_sim/status': 'std_msgs/msg/String',
                '/studio_sim/camera': 'sensor_msgs/msg/Image'}
    graph = dict(node.get_topic_names_and_types())
    record('discovered-topic-types', all(typ in graph.get(topic, []) for topic, typ in expected.items()), graph)
    try:
        endpoints = node.get_subscriber_names_and_types_by_node('biggs_data_fixture', '/studio_sim')
        record('simulator-has-no-command-subscriptions', not endpoints, endpoints)
        services = node.get_service_names_and_types_by_node('biggs_data_fixture', '/studio_sim')
        # Jazzy exposes a read-only type-description service independently of
        # parameter services. Permit that exact introspection endpoint only.
        read_only_type_service = ('/studio_sim/biggs_data_fixture/get_type_description',
                                  ['type_description_interfaces/srv/GetTypeDescription'])
        record('simulator-has-no-command-services', all(tuple(item) == read_only_type_service for item in services), services)
    except Exception as error:
        record('simulator-has-no-command-subscriptions', False, str(error))
    print(json.dumps({'os': platform.platform(), 'cpu': platform.machine(), 'ros_distro': os.environ.get('ROS_DISTRO'),
                      'python': platform.python_version(), 'middleware_pair': [os.environ.get('RMW_IMPLEMENTATION')] * 2,
                      'duration_seconds': round(time.monotonic() - started, 3), 'results': results}), flush=True)
    node.destroy_node()
    rclpy.shutdown()
    return int(any(row['outcome'] != 'passed' for row in results))


if __name__ == '__main__':
    raise SystemExit(main())
