"""Bounded synthetic Tools UI peers. Never controls physical devices."""
import argparse
import json
import os
from pathlib import Path
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument('--seconds', type=int, default=180)
parser.add_argument('--nav2', action='store_true', help='Use the synthetic Nav2 action endpoint')
parser.add_argument('--evidence', type=Path)
args = parser.parse_args()
if Path(sys.prefix).resolve() != (ROOT / '.pixi/envs/default').resolve():
    raise SystemExit('Use project Python through scripts/dev')
if not 1 <= args.seconds <= 600:
    raise SystemExit('Duration must be 1..600 seconds')
if args.evidence:
    args.evidence = args.evidence.resolve()
    if not args.evidence.is_relative_to(ROOT/'artifacts/feasibility/evidence'):
        raise SystemExit('Evidence must stay under ignored artifacts/feasibility/evidence')
    args.evidence.mkdir(parents=True, exist_ok=False)
if os.environ.get('ROS_DOMAIN_ID') != '187' or os.environ.get('ROS_LOCALHOST_ONLY') != '1' or os.environ.get('ROS_AUTOMATIC_DISCOVERY_RANGE') != 'LOCALHOST':
    raise SystemExit('Requires localhost fixture domain187')
if os.environ.get('RMW_IMPLEMENTATION') not in ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']:
    raise SystemExit('Choose matching middleware explicitly')
for key in ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI',
            'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']:
    if os.environ.get(key):
        raise SystemExit('Remove custom/remote discovery settings: ' + key)
import rclpy
from example_interfaces.srv import AddTwoInts
from std_msgs.msg import String, UInt64, Float64
from sensor_msgs.msg import Image
rclpy.init()
node = rclpy.create_node('tools_fixture', namespace='/studio_fixture')
def addition(request, response):
    response.sum = request.a + request.b
    return response
server = node.create_service(AddTwoInts, '/studio_fixture/tools_add', addition)
publisher = node.create_publisher(String, '/studio_fixture/tools_text', 10)
numeric = node.create_publisher(UInt64, '/studio_fixture/tools_numeric', 10)
image = node.create_publisher(Image, '/studio_fixture/tools_image', 10)
plot = node.create_publisher(Float64, '/studio_fixture/tools_plot', 10)
count = 0
def publish_samples():
    global count
    publisher.publish(String(data='Tools synthetic fixture'))
    numeric.publish(UInt64(data=18446744073709551000 + count % 100))
    image.publish(Image(height=2, width=2, encoding='rgb8', step=6,
                        data=[230,70,60,60,190,90,70,100,230,220,200,80]))
    plot.publish(Float64(data=23.4 + (count % 10) / 10))
    count += 1
timer = node.create_timer(0.2, publish_samples)
import importlib.util
spec = importlib.util.spec_from_file_location('tools_peers', ROOT / 'scripts/tools-peers.py')
peers = importlib.util.module_from_spec(spec)
spec.loader.exec_module(peers)
peers.create_parameters(node)
action_server = peers.create_nav2_action(node) if args.nav2 else peers.create_action(node)
from rclpy.executors import MultiThreadedExecutor, ExternalShutdownException
executor = MultiThreadedExecutor(num_threads=3)
executor.add_node(node)
if args.evidence:
    (args.evidence/'started.json').write_text(json.dumps({'pid':os.getpid(),'event':'started','nav2':args.nav2})+'\n')
try:
    until = time.monotonic() + args.seconds
    while time.monotonic() < until:
        executor.spin_once(timeout_sec=0.05)
except (KeyboardInterrupt, ExternalShutdownException):
    pass
finally:
    executor.shutdown(timeout_sec=5)
    if args.evidence:
        (args.evidence/'stopped.json').write_text(json.dumps({'pid':os.getpid(),'event':'stopped','goal_requests':node.tools_goal_requests,'cancel_requests':node.tools_cancel_requests})+'\n')
    action_server.destroy()
    node.destroy_node()
    if rclpy.ok():
        rclpy.shutdown()
