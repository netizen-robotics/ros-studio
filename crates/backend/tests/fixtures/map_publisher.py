"""Synthetic native-grid reader fixture; no physical robot commands."""
import array
import json
import os
import sys
import time

REMOTE = ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT',
          'CYCLONEDDS_URI', 'FASTDDS_DEFAULT_PROFILES_FILE',
          'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']
assert os.environ.get('ROS_DOMAIN_ID') == '187'
assert os.environ.get('ROS_LOCALHOST_ONLY') == '1'
assert os.environ.get('ROS_AUTOMATIC_DISCOVERY_RANGE') == 'LOCALHOST'
assert os.environ.get('RMW_IMPLEMENTATION') in ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']
assert not any(os.environ.get(key) for key in REMOTE)
if sys.argv[1:] == ['--diagnostic-profile']:
    from pathlib import Path
    sys.path.insert(0, str(Path(__file__).resolve().parents[4] / 'scripts'))
    from map_dds_profiles import configure
    configure(os.environ['STUDIO_UI_ROS_EVIDENCE'])
else:
    assert not sys.argv[1:], 'Unknown synthetic publisher option'

# Preserve the control pipe before loading native ROS libraries. Vendor stdio
# diagnostics belong on stderr, as in the production backend wire boundary.
wire = os.fdopen(os.dup(sys.stdout.fileno()), 'w', buffering=1)
os.dup2(sys.stderr.fileno(), sys.stdout.fileno())

def emit(value):
    print(json.dumps(value), file=wire, flush=True)

import rclpy
from rclpy.qos import QoSProfile, DurabilityPolicy, ReliabilityPolicy
from nav_msgs.msg import OccupancyGrid
from map_msgs.msg import OccupancyGridUpdate
from std_msgs.msg import UInt64
from sensor_msgs.msg import Image
from tf2_msgs.msg import TFMessage
from geometry_msgs.msg import TransformStamped

rclpy.init()
node = rclpy.create_node('studio_native_grid_fixture')
full = node.create_publisher(OccupancyGrid, '/studio_fixture/raw_grid',
    QoSProfile(depth=2, durability=DurabilityPolicy.TRANSIENT_LOCAL))
updates = node.create_publisher(OccupancyGridUpdate, '/studio_fixture/raw_grid_updates', 2)
full_best = node.create_publisher(OccupancyGrid, '/studio_fixture/raw_grid_best', QoSProfile(depth=2, durability=DurabilityPolicy.TRANSIENT_LOCAL, reliability=ReliabilityPolicy.BEST_EFFORT))
updates_best = node.create_publisher(OccupancyGridUpdate, '/studio_fixture/raw_grid_updates_best', QoSProfile(depth=2, reliability=ReliabilityPolicy.BEST_EFFORT))
image = node.create_publisher(Image, '/studio_fixture/map_image', 2)
dynamic = node.create_publisher(TFMessage, '/tf', 2)
counter = node.create_publisher(UInt64, '/studio_fixture/map_counter', 2)
static = node.create_publisher(TFMessage, '/tf_static', QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL))
transform = TransformStamped()
transform.header.frame_id = 'world'
transform.child_frame_id = 'source'
transform.transform.translation.x = 2.0
transform.transform.rotation.w = 1.0
static.publish(TFMessage(transforms=[transform]))
emit({'ready': True, 'pid': os.getpid()})
counter_value = 0
try:
    for command in sys.stdin:
        command = command.strip()
        probe_dimension = None
        performance_stamp = None
        if command.startswith(('full_perf ', 'patch_perf ')):
            name, value = command.split()
            performance_stamp = int(value)
            assert 4 <= performance_stamp <= 34
            command = 'full' if name == 'full_perf' else 'patch'
        if command.startswith(('patch_best_size ', 'full_best_size ')):
            probe_name, raw_dimension = command.split()
            probe_dimension = int(raw_dimension)
            assert 1 <= probe_dimension <= 2048
            command = 'patch_best' if probe_name == 'patch_best_size' else 'retained_best'
        if command == 'stop':
            break
        if command == 'counts':
            emit({'full': full.get_subscription_count(), 'updates': updates.get_subscription_count(),
                  'counter': counter.get_subscription_count(), 'static_tf': static.get_subscription_count(), 'full_best': full_best.get_subscription_count(), 'updates_best': updates_best.get_subscription_count(), 'image': image.get_subscription_count()})
            continue
        if command == 'connect_frame':
            parent = TransformStamped()
            parent.header.frame_id = 'missing_world'
            parent.child_frame_id = 'world'
            parent.transform.translation.x = 10.0
            parent.transform.rotation.w = 1.0
            static.publish(TFMessage(transforms=[transform, parent]))
            emit({'sent': command})
            continue
        assert command in ['retained_best_small', 'patch_best_small', 'retained_best', 'patch_best', 'patch_best_nowait', 'image', 'full', 'retained', 'recovery', 'patch', 'burst', 'malformed_patch', 'counter']
        is_full = command in ['full', 'retained', 'recovery', 'retained_best', 'retained_best_small']
        publisher = full_best if command in ['retained_best', 'retained_best_small'] else updates_best if command in ['patch_best', 'patch_best_nowait', 'patch_best_small'] else image if command == 'image' else full if is_full else counter if command == 'counter' else updates
        deadline = time.monotonic() + 15
        while command not in ['retained', 'retained_best', 'patch_best_nowait'] and publisher.get_subscription_count() != 1:
            assert time.monotonic() < deadline, 'Expected exactly one matching subscriber'
            rclpy.spin_once(node, timeout_sec=0.05)
        if command == 'image':
            message = Image(height=2, width=2, encoding='mono8', step=2, data=[0, 50, 100, 255])
            publisher.publish(message)
            emit({'sent': command})
            continue
        if command == 'counter':
            counter_value += 1
            publisher.publish(UInt64(data=counter_value))
            emit({'sent': command, 'value': counter_value})
            continue
        if command == 'burst':
            for index in range(32):
                message = OccupancyGridUpdate(x=index, y=0, width=1, height=1, data=[index - 64])
                message.header.frame_id = 'source'
                message.header.stamp.sec = 3
                message.header.stamp.nanosec = index
                publisher.publish(message)
            emit({'sent': command, 'count': 32})
            continue
        message = OccupancyGrid() if is_full else OccupancyGridUpdate()
        message.header.frame_id = 'source'
        message.header.stamp.sec = 2
        message.header.stamp.nanosec = 3 if command in ['full', 'retained', 'retained_best'] else 4
        if performance_stamp is not None:
            message.header.stamp.nanosec = performance_stamp
        if command in ['patch_best', 'patch_best_nowait', 'patch_best_small']:
            message.header.stamp.sec = 5
            message.header.stamp.nanosec = 0
        if command == 'recovery':
            message.header.stamp.sec = 4
            message.header.stamp.nanosec = 0
        if is_full:
            message.info.width = message.info.height = 2048
            message.info.resolution = 0.5
            message.info.origin.orientation.w = 1.0
        else:
            message.width = message.height = 2048
        message.data = array.array('b', bytes(range(256)) * 16384)
        if probe_dimension is not None:
            if is_full:
                message.info.width = message.info.height = probe_dimension
                message.header.stamp.sec = 7
                message.header.stamp.nanosec = 0
            else:
                message.width = message.height = probe_dimension
            cell_count = probe_dimension * probe_dimension
            message.data = array.array('b', (bytes(range(256)) * ((cell_count + 255) // 256))[:cell_count])
        if command == 'retained_best_small':
            message.info.width = message.info.height = 2
            message.header.stamp.sec = 6
            message.header.stamp.nanosec = 0
            message.data = array.array('b', [0, 50, 100, -1])
        if command == 'patch_best_small':
            message.width = message.height = 1
            message.data = array.array('b', [42])
        if command == 'malformed_patch':
            message.width = message.height = 2
            message.data = array.array('b', [1])
        publisher.publish(message)
        emit({'sent': command, 'cells': len(message.data)})
finally:
    node.destroy_node()
    rclpy.shutdown()
