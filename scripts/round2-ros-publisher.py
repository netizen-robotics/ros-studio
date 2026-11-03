"""Local-only typed ROS publisher. Use scripts/dev project Python with domain187 guards.

Publishes observation data only, never subscribes to commands. Optional --custom-prefix
is an already-built feasibility_interfaces install; no build or remote peer is started.
"""
import argparse
import importlib.util
import math
import os
from pathlib import Path
import signal
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('workflow', ROOT / 'scripts/ui-ros-workflow.py')
workflow = importlib.util.module_from_spec(spec)
spec.loader.exec_module(workflow)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--seconds', type=float, default=600)
    parser.add_argument('--custom-prefix', type=Path)
    args = parser.parse_args()
    out = workflow.checked_environment()
    if not 0 < args.seconds <= 3600:
        parser.error('seconds must be in (0,3600]')
    if args.custom_prefix:
        prefix = args.custom_prefix.resolve()
        if not prefix.is_relative_to(ROOT / 'artifacts'):
            parser.error('custom prefix must be a local ignored artifacts install')
        library = str(prefix / 'lib')
        if os.environ.get('STUDIO_ROUND2_LIBRARY_PREFIX') != str(prefix):
            # macOS dyld reads this before process startup. Setting it after
            # importing generated Python bindings cannot fix dispatch dlopen.
            environment = os.environ.copy()
            for key in ['DYLD_LIBRARY_PATH', 'LD_LIBRARY_PATH']:
                environment[key] = library + os.pathsep + str(workflow.RUNTIME / 'lib')
            environment['STUDIO_ROUND2_LIBRARY_PREFIX'] = str(prefix)
            os.execve(sys.executable, [sys.executable, *sys.argv], environment)
    import rclpy
    from rclpy.qos import QoSProfile, ReliabilityPolicy, DurabilityPolicy
    from nav_msgs.msg import Odometry
    from std_msgs.msg import Float64, UInt64, Int64, String
    custom = None
    if args.custom_prefix:
        prefix = args.custom_prefix.resolve()
        if not prefix.is_relative_to(ROOT / 'artifacts'):
            parser.error('custom prefix must be a local ignored artifacts install')
        for path in prefix.glob('lib/python*/site-packages'):
            sys.path.insert(0, str(path))
        from feasibility_interfaces.msg import Sample
        custom = Sample
    rclpy.init()
    node = rclpy.create_node('round2_typed_fixture', enable_rosout=False, start_parameter_services=False)
    qos = QoSProfile(depth=10)
    definitions = [('odometry', Odometry, qos), ('uint64', UInt64, qos), ('int64', Int64, qos),
                   ('float64', Float64, qos), ('large_text', String, qos),
                   ('best_effort', Float64, QoSProfile(depth=10, reliability=ReliabilityPolicy.BEST_EFFORT)),
                   ('retained', String, QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL))]
    if custom:
        definitions.append(('custom', custom, qos))
    publishers = {name: node.create_publisher(typ, '/studio_fixture/round2/' + name, profile) for name, typ, profile in definitions}
    log = out / 'round2-publisher.jsonl'
    workflow.append(log, {'event': 'started', 'pid': os.getpid(), 'time': time.time(), 'argv': sys.argv,
                         'rmw': os.environ['RMW_IMPLEMENTATION'], 'domain': '187', 'localhost_only': True,
                         'topics': list(publishers), 'custom': bool(custom)})
    end = time.monotonic() + args.seconds
    count = 0
    try:
        publishers['retained'].publish(String(data='Round2-retained-once café 🤖'))
        workflow.append(log, {'event': 'retained-once', 'time': time.time()})
        while rclpy.ok() and time.monotonic() < end:
            sample = Odometry()
            sample.header.stamp = node.get_clock().now().to_msg()
            sample.header.frame_id = 'odom'
            sample.child_frame_id = 'base_link'
            sample.pose.pose.position.x = math.sin(count / 10)
            sample.pose.pose.position.y = math.cos(count / 10)
            sample.pose.pose.orientation.w = 1.0
            sample.twist.twist.linear.x = 0.1 + math.sin(count / 10) / 20
            publishers['odometry'].publish(sample)
            publishers['uint64'].publish(UInt64(data=18446744073709551615))
            publishers['int64'].publish(Int64(data=-9223372036854775808))
            publishers['float64'].publish(Float64(data=[1.25, float('nan'), float('inf'), -float('inf')][count % 4]))
            publishers['best_effort'].publish(Float64(data=12.5))
            publishers['large_text'].publish(String(data='café 🤖\n' * 5000))
            if custom:
                value = custom()
                value.sequence = 18446744073709551615
                value.nested.label = 'independently imported café 🤖'
                value.nested.value = 3.25
                value.fixed = [1.25, -2.5, 0.0]
                value.bounded = [-2147483648, 2147483647]
                value.labels = ['first', 'second']
                value.payload = [0, 127, 255]
                publishers['custom'].publish(value)
            count += 1
            rclpy.spin_once(node, timeout_sec=0.2)
    except KeyboardInterrupt:
        pass
    finally:
        previous = signal.signal(signal.SIGINT, signal.SIG_IGN)
        try:
            node.destroy_node()
            if rclpy.ok():
                rclpy.shutdown()
            workflow.append(log, {'event': 'stopped', 'time': time.time(), 'samples_per_stream': count})
        finally:
            signal.signal(signal.SIGINT, previous)


if __name__ == '__main__':
    main()
