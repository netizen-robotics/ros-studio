"""Isolated standard pose/covariance source fixture; no robot commands."""
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
from geometry_msgs.msg import PoseWithCovarianceStamped, TransformStamped
from nav_msgs.msg import Odometry
from tf2_msgs.msg import TFMessage

rclpy.init()
node = rclpy.create_node('studio_pose_fixture')
# Pose sources retain one current state. Retaining a prior intentionally invalid
# sample would correctly fault every new reader before its latest valid state.
qos = QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL)
types = {'pose': PoseWithCovarianceStamped, 'odom': Odometry, 'odom_best': Odometry}
pubs = {key: node.create_publisher(typ, '/studio_fixture/raw_' + key,
                                 QoSProfile(depth=2, reliability=ReliabilityPolicy.BEST_EFFORT) if key == 'odom_best' else qos)
        for key, typ in types.items()}
tf_pub = node.create_publisher(TFMessage, '/tf_static', qos)
transform = TransformStamped()
transform.header.frame_id = 'world'
transform.child_frame_id = 'odom'
transform.transform.translation.x = 10.0
transform.transform.rotation.w = 1.0
tf_pub.publish(TFMessage(transforms=[transform]))


def publish(target, stamp=2, invalid=False, child='base'):
    message = types[target]()
    message.header.frame_id = 'odom'
    message.header.stamp.sec = stamp
    message.header.stamp.nanosec = 3 if stamp else 0
    message.pose.pose.position.x = 1.0
    message.pose.pose.position.y = 2.0
    message.pose.pose.position.z = 3.0
    message.pose.pose.orientation.w = 1.0
    v = [1.0, 2.0, 3.0, 0.1, 0.2, 0.3]
    matrix = [a * b for a in v for b in v]
    if invalid:
        matrix[0] = -1.0
    message.pose.covariance = matrix
    if target.startswith('odom'):
        message.child_frame_id = child
        message.twist.twist.linear.x = 99.0
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
        publish(command['target'], command.get('stamp', 2), command.get('invalid', False), command.get('child', 'base'))
        emit({'sent': command['target']})
finally:
    node.destroy_node()
    rclpy.shutdown()
