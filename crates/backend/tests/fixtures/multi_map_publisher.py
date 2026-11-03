"""Three independent local-only occupancy grids and ordered patches for acceptance."""
import json
import faulthandler
import os
import sys

REMOTE = ('ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT',
          'CYCLONEDDS_URI', 'FASTDDS_DEFAULT_PROFILES_FILE',
          'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE')
assert os.environ.get('ROS_DOMAIN_ID') == '187'
assert os.environ.get('ROS_LOCALHOST_ONLY') == '1'
assert os.environ.get('ROS_AUTOMATIC_DISCOVERY_RANGE') == 'LOCALHOST'
assert os.environ.get('RMW_IMPLEMENTATION') in ('rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp')
assert not any(os.environ.get(key) for key in REMOTE)
wire = os.fdopen(os.dup(sys.stdout.fileno()), 'w', buffering=1)
os.dup2(sys.stderr.fileno(), sys.stdout.fileno())

import rclpy
from rclpy.qos import QoSProfile, DurabilityPolicy
from nav_msgs.msg import OccupancyGrid
from map_msgs.msg import OccupancyGridUpdate
from tf2_msgs.msg import TFMessage
from geometry_msgs.msg import TransformStamped


def emit(value):
    print(json.dumps(value), file=wire, flush=True)


faulthandler.dump_traceback_later(8, file=sys.stderr)
rclpy.init()
node = rclpy.create_node('studio_multi_map_fixture')
dynamic = node.create_publisher(TFMessage, '/tf', 2)
static = node.create_publisher(TFMessage, '/tf_static',
    QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL))
topics = ('/map', '/global_costmap/costmap', '/local_costmap/costmap')
full = [node.create_publisher(OccupancyGrid, topic,
        QoSProfile(depth=2, durability=DurabilityPolicy.TRANSIENT_LOCAL)) for topic in topics]
patches = [node.create_publisher(OccupancyGridUpdate, topic + '_updates', 2) for topic in topics]
tf = TransformStamped()
tf.header.frame_id = 'world'
tf.child_frame_id = 'map_source'
tf.transform.rotation.w = 1.0
static.publish(TFMessage(transforms=[tf]))


def publish(index, value):
    grid = OccupancyGrid()
    grid.header.frame_id = 'map_source'
    grid.header.stamp = node.get_clock().now().to_msg()
    grid.info.width = grid.info.height = 8
    grid.info.resolution = 0.5
    grid.info.origin.position.x = float(index * 5)
    grid.info.origin.orientation.w = 1.0
    grid.data = [value] * 64
    full[index].publish(grid)


for i in range(3):
    publish(i, i * 40)
faulthandler.cancel_dump_traceback_later()
emit({'ready': True, 'pid': os.getpid()})
try:
    for line in sys.stdin:
        parts = line.split()
        if not parts:
            continue
        if parts[0] == 'stop':
            break
        if parts[0] == 'counts':
            emit({'full': [p.get_subscription_count() for p in full],
                  'updates': [p.get_subscription_count() for p in patches],
                  'dynamic_tf': dynamic.get_subscription_count(),
                  'static_tf': static.get_subscription_count()})
            continue
        index = int(parts[1])
        assert 0 <= index < 3
        value = int(parts[2])
        assert 0 <= value <= 100
        if parts[0] == 'full':
            publish(index, value)
        else:
            assert parts[0] in ('patch', 'invalid')
            patch = OccupancyGridUpdate()
            patch.header.frame_id = 'map_source'
            patch.header.stamp = node.get_clock().now().to_msg()
            patch.x = 100 if parts[0] == 'invalid' else 0
            patch.y = 0
            patch.width = patch.height = 1
            patch.data = [value]
            patches[index].publish(patch)
        emit({'sent': parts[0], 'index': index})
finally:
    node.destroy_node()
    rclpy.shutdown()
