"""Thirty-three small Images, synthetic localhost domain187 capacity acceptance."""
import json
import os
import sys

REMOTE = ('ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI',
          'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE')
assert os.environ.get('ROS_DOMAIN_ID') == '187'
assert os.environ.get('ROS_LOCALHOST_ONLY') == '1'
assert os.environ.get('ROS_AUTOMATIC_DISCOVERY_RANGE') == 'LOCALHOST'
assert os.environ.get('RMW_IMPLEMENTATION') in ('rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp')
assert not any(os.environ.get(key) for key in REMOTE)
wire = os.fdopen(os.dup(sys.stdout.fileno()), 'w', buffering=1)
os.dup2(sys.stderr.fileno(), sys.stdout.fileno())
import rclpy
from rclpy.qos import QoSProfile, DurabilityPolicy
from sensor_msgs.msg import Image


def emit(value):
    print(json.dumps(value), file=wire, flush=True)


rclpy.init()
node = rclpy.create_node('studio_capacity32_fixture')
publishers = [node.create_publisher(Image, f'/images/image{i}',
    QoSProfile(depth=2, durability=DurabilityPolicy.TRANSIENT_LOCAL)) for i in range(33)]


def publish(sequence):
    for index, publisher in enumerate(publishers):
        image = Image()
        image.header.frame_id = f'camera{index}'
        image.header.stamp.sec = sequence
        image.width, image.height, image.step = 64, 48, 192
        image.encoding = 'rgb8'
        image.data = bytes([index, sequence % 256, 100]) * (64 * 48)
        publisher.publish(image)


publish(1)
emit({'ready': True, 'pid': os.getpid()})
try:
    for line in sys.stdin:
        words = line.split()
        if not words:
            continue
        if words[0] == 'stop':
            break
        if words[0] == 'counts':
            emit({'readers': [p.get_subscription_count() for p in publishers]})
        else:
            assert words[0] == 'publish'
            sequence = int(words[1])
            assert 1 <= sequence <= 10000
            publish(sequence)
            emit({'published': sequence})
finally:
    node.destroy_node()
    rclpy.shutdown()
