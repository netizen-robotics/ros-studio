"""Local-only DiagnosticArray acceptance fixture; never contacts physical peers."""
import importlib.util
import json
import os
from pathlib import Path
import platform
import signal
import sys
import time
import faulthandler
faulthandler.dump_traceback_later(12, repeat=True)
print("fixture: imports", flush=True)

ROOT = Path(__file__).resolve().parents[4]
spec = importlib.util.spec_from_file_location('workflow', ROOT / 'scripts/ui-ros-workflow.py')
workflow = importlib.util.module_from_spec(spec)
spec.loader.exec_module(workflow)
OUT = workflow.checked_environment()

import rclpy
from diagnostic_msgs.msg import DiagnosticArray, DiagnosticStatus, KeyValue
from rclpy.qos import QoSProfile, ReliabilityPolicy, DurabilityPolicy

print("fixture: rclpy init", flush=True)
rclpy.init()
print("fixture: create node", flush=True)
node = rclpy.create_node('diagnostic_acceptance_fixture', namespace='/studio_fixture')
print("fixture: create publishers", flush=True)
publishers = [
    node.create_publisher(DiagnosticArray, '/studio_fixture/diagnostics', QoSProfile(depth=10)),
    node.create_publisher(DiagnosticArray, '/studio_fixture/diagnostics_best', QoSProfile(depth=10, reliability=ReliabilityPolicy.BEST_EFFORT)),
    node.create_publisher(DiagnosticArray, '/studio_fixture/diagnostics_retained', QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL)),
]
print('fixture: ready', flush=True)
faulthandler.cancel_dump_traceback_later()
(OUT / 'diagnostic-publisher.json').write_text(json.dumps({
    'pid': os.getpid(), 'os': platform.platform(), 'cpu': platform.machine(),
    'distro': os.environ['ROS_DISTRO'], 'rmw': os.environ['RMW_IMPLEMENTATION'],
    'python': sys.version, 'rclpy': rclpy.__version__ if hasattr(rclpy, '__version__') else 'package inventory',
    'command': sys.argv, 'domain': 187, 'localhost': True,
}))
alive = True

def stop(*_):
    global alive
    alive = False

signal.signal(signal.SIGTERM, stop)
signal.signal(signal.SIGINT, stop)
started = time.monotonic()
retained_sent = False
late_probe = None

def retained_control(message):
    (OUT / 'python-late-retained.json').write_text(json.dumps({'received': True, 'status': message.status[0].name}))
try:
    while alive and time.monotonic() - started < 600:
        message = DiagnosticArray()
        message.header.stamp = node.get_clock().now().to_msg()
        status = DiagnosticStatus(level=DiagnosticStatus.WARN, name='fixture temperature', hardware_id='synthetic board', message='PRIVATE_DIAGNOSTIC_MESSAGE')
        status.values = [KeyValue(key='temperature', value='PRIVATE_DIAGNOSTIC_VALUE')]
        message.status = [status]
        if not (OUT / 'pause-diagnostics').exists():
            for publisher in publishers[:2]:
                publisher.publish(message)
        if not retained_sent and time.monotonic() - started > 1:
            publishers[2].publish(message)
            retained_sent = True
        if late_probe is None and time.monotonic() - started > 3:
            late_probe = node.create_subscription(DiagnosticArray, '/studio_fixture/diagnostics_retained', retained_control,
                QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL))
        rclpy.spin_once(node, timeout_sec=0.1)
finally:
    node.destroy_node()
    rclpy.shutdown()
