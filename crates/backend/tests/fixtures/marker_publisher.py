"""Local synthetic Marker/MarkerArray RPC fixture; no robot commands."""
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
assert not sys.argv[1:]
wire = os.fdopen(os.dup(sys.stdout.fileno()), 'w', buffering=1)
os.dup2(sys.stderr.fileno(), sys.stdout.fileno())


def emit(value):
    print(json.dumps(value), file=wire, flush=True)


import rclpy
from rclpy.qos import QoSProfile, DurabilityPolicy, ReliabilityPolicy
from visualization_msgs.msg import Marker, MarkerArray
from geometry_msgs.msg import Point, TransformStamped
from tf2_msgs.msg import TFMessage
from sensor_msgs.msg import Image
from std_msgs.msg import ColorRGBA

rclpy.init()
node = rclpy.create_node('studio_marker_fixture')
retained = QoSProfile(depth=32, durability=DurabilityPolicy.TRANSIENT_LOCAL)
pubs = {
    'single': node.create_publisher(Marker, '/studio_fixture/marker', retained),
    'array': node.create_publisher(MarkerArray, '/studio_fixture/marker_array', retained),
    'sibling': node.create_publisher(MarkerArray, '/studio_fixture/marker_array_array', retained),
    'best': node.create_publisher(Marker, '/studio_fixture/marker_best', QoSProfile(depth=2, reliability=ReliabilityPolicy.BEST_EFFORT)),
    'image': node.create_publisher(Image, '/studio_fixture/marker_image', 2),
    'static_tf': node.create_publisher(TFMessage, '/tf_static', retained),
}
transform = TransformStamped()
transform.header.frame_id = 'world'
transform.child_frame_id = 'source'
transform.transform.translation.x = 2.0
transform.transform.rotation.w = 1.0
pubs['static_tf'].publish(TFMessage(transforms=[transform]))
emit({'ready': True, 'pid': os.getpid()})
try:
    for line in sys.stdin:
        command = json.loads(line)
        op = command['op']
        if op == 'counts':
            emit({key: pub.get_subscription_count() for key, pub in pubs.items()})
            continue
        if op == 'connect_frame':
            other = TransformStamped()
            other.header.frame_id = 'world'
            other.child_frame_id = 'missing'
            other.transform.rotation.w = 1.0
            pubs['static_tf'].publish(TFMessage(transforms=[transform, other]))
            emit({'sent': op})
            continue
        if op == 'image':
            pubs['image'].publish(Image(height=2, width=2, encoding='mono8', step=2, data=[0, 50, 100, 255]))
            emit({'sent': op})
            continue
        assert op == 'publish'
        target = command['target']
        assert target in ['single', 'array', 'best']
        markers = []
        for spec in command['markers']:
            m = Marker()
            m.header.frame_id = spec.get('frame', 'source')
            m.ns = spec.get('namespace', 'objects')
            m.id = spec['id']
            m.action = spec.get('action', 0)
            m.type = spec.get('type', 1)
            m.pose.orientation.w = 1.0
            m.pose.position.x = float(spec.get('x', 0.0))
            m.pose.position.y = float(spec.get('y', 0.0))
            m.pose.position.z = float(spec.get('z', 0.0))
            scale = spec.get('scale', [1.0, 1.0, 1.0])
            assert len(scale) == 3
            m.scale.x, m.scale.y, m.scale.z = map(float, scale)
            color = spec.get('color', [1.0, 0.0, 0.0, spec.get('alpha', 1.0)])
            assert len(color) == 4
            m.color.r, m.color.g, m.color.b, m.color.a = map(float, color)
            m.text = spec.get('text', '')
            m.mesh_resource = spec.get('mesh_resource', '')
            m.mesh_use_embedded_materials = spec.get('embedded', False)
            assert len(m.text.encode()) <= 4096 and len(m.mesh_resource.encode()) <= 4096
            m.frame_locked = spec.get('locked', False)
            lifetime = spec.get('lifetime_ns', 0)
            m.lifetime.sec, m.lifetime.nanosec = divmod(lifetime, 1000000000)
            points = spec.get('points', 0)
            if isinstance(points, int):
                assert 0 <= points <= 4096
                m.points = [Point(x=float(i), y=0.0, z=0.0) for i in range(points)]
            else:
                assert len(points) <= 4096 and all(len(p) == 3 for p in points)
                m.points = [Point(x=float(p[0]), y=float(p[1]), z=float(p[2])) for p in points]
            colors = spec.get('colors', [])
            assert len(colors) <= 4096 and all(len(c) == 4 for c in colors)
            m.colors = [ColorRGBA(r=float(c[0]), g=float(c[1]), b=float(c[2]), a=float(c[3])) for c in colors]
            markers.append(m)
        if target == 'array':
            pubs[target].publish(MarkerArray(markers=markers))
        else:
            assert len(markers) == 1
            pubs[target].publish(markers[0])
        emit({'sent': op, 'target': target, 'markers': len(markers)})
finally:
    node.destroy_node()
    rclpy.shutdown()
    emit({'stopped': True, 'pid': os.getpid()})
