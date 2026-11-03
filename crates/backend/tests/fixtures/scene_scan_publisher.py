"""Guarded localhost fixture: 100 Hz TF + 10 Hz best-effort scans. No robot actions."""
import json
import os
import queue
import sys
import threading

assert os.environ.get('ROS_DOMAIN_ID') == '187'
assert os.environ.get('ROS_LOCALHOST_ONLY') == '1'
assert os.environ.get('ROS_AUTOMATIC_DISCOVERY_RANGE') == 'LOCALHOST'
assert os.environ.get('RMW_IMPLEMENTATION') in ('rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp')
for key in ('ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI',
            'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE'):
    assert not os.environ.get(key), key
assert not sys.argv[1:]
wire = os.fdopen(os.dup(sys.stdout.fileno()), 'w', buffering=1)
os.dup2(sys.stderr.fileno(), sys.stdout.fileno())


def emit(value):
    print(json.dumps(value), file=wire, flush=True)


import rclpy
from rclpy.qos import QoSProfile, DurabilityPolicy, ReliabilityPolicy
from geometry_msgs.msg import TransformStamped
from sensor_msgs.msg import LaserScan
from tf2_msgs.msg import TFMessage

rclpy.init()
node = rclpy.create_node('studio_scene_scan_fixture')
scan = node.create_publisher(LaserScan, '/studio_fixture/scan', QoSProfile(depth=5, reliability=ReliabilityPolicy.BEST_EFFORT))
dynamic = node.create_publisher(TFMessage, '/tf', QoSProfile(depth=20))
static = node.create_publisher(TFMessage, '/tf_static', QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL))
counts = {'scans_sent': 0, 'tf_sent': 0}


def publish_tf():
    transform = TransformStamped()
    transform.header.stamp = node.get_clock().now().to_msg()
    transform.header.frame_id = 'odom'
    transform.child_frame_id = 'laser'
    transform.transform.rotation.w = 1.0
    dynamic.publish(TFMessage(transforms=[transform]))
    counts['tf_sent'] += 1


def publish_scan():
    message = LaserScan()
    message.header.stamp = node.get_clock().now().to_msg()
    message.header.frame_id = 'laser'
    message.angle_min = -0.5
    message.angle_max = 0.5
    message.angle_increment = 0.1
    message.scan_time = 0.1
    message.range_min = 0.1
    message.range_max = 10.0
    message.ranges = [1.0] * 11
    scan.publish(message)
    counts['scans_sent'] += 1


node.create_timer(0.01, publish_tf)
node.create_timer(0.1, publish_scan)
commands = queue.Queue(maxsize=16)


def read_commands():
    for line in sys.stdin:
        commands.put(line.strip())
    commands.put(None)


threading.Thread(target=read_commands, daemon=True).start()
emit({'ready': True, 'pid': os.getpid(), 'tf_hz': 100, 'scan_hz': 10, 'map_initially_missing': True})
try:
    while rclpy.ok():
        rclpy.spin_once(node, timeout_sec=0.005)
        try:
            command = commands.get_nowait()
        except queue.Empty:
            continue
        if command is None or command == 'stop':
            break
        if command == 'counts':
            emit(dict(scan=scan.get_subscription_count(), dynamic_tf=dynamic.get_subscription_count(), static_tf=static.get_subscription_count(), **counts))
        elif command == 'map':
            transform = TransformStamped()
            transform.header.frame_id = 'map'
            transform.child_frame_id = 'odom'
            transform.transform.rotation.w = 1.0
            static.publish(TFMessage(transforms=[transform]))
            emit({'sent': 'map'})
        else:
            raise AssertionError(command)
finally:
    node.destroy_node()
    rclpy.shutdown()
