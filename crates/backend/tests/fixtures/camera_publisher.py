"""Bounded localhost Camera/Image/CameraInfo fixture; no robot commands."""
import json
import os
import sys

assert os.environ.get('ROS_DOMAIN_ID') == '187'
assert os.environ.get('ROS_LOCALHOST_ONLY') == '1'
assert os.environ.get('ROS_AUTOMATIC_DISCOVERY_RANGE') == 'LOCALHOST'
assert os.environ.get('RMW_IMPLEMENTATION') in ('rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp')
for key in ('ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT',
            'CYCLONEDDS_URI', 'FASTDDS_DEFAULT_PROFILES_FILE',
            'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE'):
    assert not os.environ.get(key), key
assert not sys.argv[1:]
wire = os.fdopen(os.dup(sys.stdout.fileno()), 'w', buffering=1)
os.dup2(sys.stderr.fileno(), sys.stdout.fileno())


def emit(value):
    print(json.dumps(value), file=wire, flush=True)


import rclpy
from rclpy.qos import QoSProfile, DurabilityPolicy, ReliabilityPolicy
from sensor_msgs.msg import Image, CameraInfo
from geometry_msgs.msg import TransformStamped
from tf2_msgs.msg import TFMessage

rclpy.init()
node = rclpy.create_node('studio_calibrated_camera_fixture')
retained = QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL)
best = QoSProfile(depth=1, reliability=ReliabilityPolicy.BEST_EFFORT)
topics = {'image': ('image_rect', Image), 'peer': ('peer_image', Image),
          'info': ('camera_info', CameraInfo), 'other': ('other_info', CameraInfo),
          'best': ('best_info', CameraInfo)}
pubs = {key: node.create_publisher(typ, '/studio_fixture/' + topic,
                                  best if key == 'best' else retained)
        for key, (topic, typ) in topics.items()}
tf_pub = node.create_publisher(TFMessage, '/tf_static', retained)
transform = TransformStamped()
transform.header.frame_id = 'world'
transform.child_frame_id = 'camera_optical'
transform.transform.translation.z = 5.0
transform.transform.rotation.x = 1.0
transform.transform.rotation.w = 0.0
assert sum(getattr(transform.transform.rotation, axis) ** 2 for axis in "xyzw") == 1.0
tf_pub.publish(TFMessage(transforms=[transform]))


def publish(target, stamp=2, focal=60.0, invalid=False, width=64):
    if target in ('image', 'peer'):
        message = Image()
        message.width = width
        message.height = 48
        message.encoding = 'rgb8'
        message.step = width * 3
        message.data = bytes([255, 0, 0]) * (width * 48)
    else:
        message = CameraInfo()
        message.width = 64
        message.height = 48
        message.distortion_model = 'plumb_bob'
        message.d = [0.0] * 5
        focal = -1.0 if invalid else focal
        message.k = [focal, 0.0, 30.0, 0.0, 60.0, 23.0, 0.0, 0.0, 1.0]
        message.r = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
        message.p = [focal, 0.0, 30.0, 0.0, 0.0, 60.0, 23.0, 0.0,
                     0.0, 0.0, 1.0, 0.0]
    message.header.frame_id = 'other_optical' if target == 'other' else 'camera_optical'
    message.header.stamp.sec = stamp
    message.header.stamp.nanosec = 3 if stamp else 0
    pubs[target].publish(message)


try:
    for target in pubs:
        publish(target)
    emit({'ready': True, 'pid': os.getpid()})
    for line in sys.stdin:
        command = json.loads(line)
        if command['op'] == 'stop':
            break
        if command['op'] == 'counts':
            emit({**{key: pub.get_subscription_count() for key, pub in pubs.items()},
                  'static_tf': tf_pub.get_subscription_count()})
            continue
        assert command['op'] == 'publish'
        publish(command['target'], command.get('stamp', 2), command.get('focal', 60.0),
                command.get('invalid', False), command.get('width', 64))
        emit({'sent': command['target']})
finally:
    node.destroy_node()
    rclpy.shutdown()
