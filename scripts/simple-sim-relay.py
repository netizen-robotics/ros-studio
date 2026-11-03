"""Development-only Docker simulation snapshots -> local standard ROS publishers.

The app still uses the real Rust backend. No robot connections, commands, automatic
reconnect or app-protocol adapter. Stops on transport failure; restart explicitly.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import runpy
import signal
import time
from urllib.request import build_opener, ProxyHandler

ROOT = Path(__file__).resolve().parents[1]
TRANSPORT = runpy.run_path(str(ROOT / 'fixtures/simple_sim/transport.py'))
MAX_SNAPSHOT_BYTES = TRANSPORT['MAX_SNAPSHOT_BYTES']
MAX_CDR_BYTES = TRANSPORT['MAX_CDR_BYTES']


def unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError('Duplicate fixture JSON field')
        value[key] = item
    return value


def decode_snapshot(payload, seen, types):
    if len(payload) > MAX_SNAPSHOT_BYTES:
        raise ValueError('Oversized fixture snapshot')
    snapshot = json.loads(payload, object_pairs_hook=unique_object)
    if not isinstance(snapshot, dict) or set(snapshot) != {'fixture_version', 'samples'} or not isinstance(snapshot.get('samples'), dict):
        raise ValueError('Unsupported fixture transport')
    allowed = TRANSPORT['topics_for_version'](snapshot.get('fixture_version'))
    if len(snapshot['samples']) > len(allowed):
        raise ValueError('Unexpected fixture endpoint set')
    decoded = []
    for topic, sample in snapshot['samples'].items():
        if topic not in allowed or topic not in types or not isinstance(sample, dict) or sample.get('type') != allowed[topic] or sample.get('type') != types[topic][0]:
            raise ValueError('Unexpected fixture topic/type')
        if set(sample) != {'type', 'sequence', 'cdr'}:
            raise ValueError('Invalid fixture sample metadata')
        sequence = sample.get('sequence')
        if type(sequence) is not int or sequence < 0 or sequence < seen[topic]:
            raise ValueError('Simulation restarted or invalid sequence; restart this relay explicitly')
        encoded = sample['cdr']
        if not isinstance(encoded, str) or len(encoded) > ((MAX_CDR_BYTES + 2) // 3) * 4:
            raise ValueError('Oversized fixture sample')
        cdr = base64.b64decode(encoded, validate=True)
        if len(cdr) > MAX_CDR_BYTES or base64.b64encode(cdr).decode('ascii') != encoded:
            raise ValueError('Oversized fixture sample')
        if sequence == seen[topic]:
            continue
        decoded.append((topic, sequence, cdr))
    return decoded


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--seconds', type=float, default=600)
    args = parser.parse_args()
    if not 0 < args.seconds <= 86400:
        parser.error('--seconds must be positive and at most 86400')
    # Reuse the existing project-Python, localhost/domain and peer-profile guard.
    workflow = runpy.run_path(str(ROOT / 'scripts/ui-ros-workflow.py'))
    out = workflow['checked_environment']()
    import rclpy
    from nav_msgs.msg import Odometry
    from rclpy.qos import DurabilityPolicy, QoSProfile, qos_profile_sensor_data
    from rclpy.serialization import deserialize_message
    from sensor_msgs.msg import Image, JointState, LaserScan
    from std_msgs.msg import String
    from tf2_msgs.msg import TFMessage
    rclpy.init()
    node = rclpy.create_node('docker_simulation_relay', namespace='/studio_sim', start_parameter_services=False)
    retained = QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL)
    types = {
        '/studio_sim/odometry': ('nav_msgs/msg/Odometry', Odometry, 10),
        '/studio_sim/joint_states': ('sensor_msgs/msg/JointState', JointState, 10),
        '/studio_sim/scan': ('sensor_msgs/msg/LaserScan', LaserScan, qos_profile_sensor_data),
        '/studio_sim/camera': ('sensor_msgs/msg/Image', Image, qos_profile_sensor_data),
        '/studio_sim/status': ('std_msgs/msg/String', String, 10),
        '/studio_sim/robot_description': ('std_msgs/msg/String', String, retained),
        '/tf': ('tf2_msgs/msg/TFMessage', TFMessage, 10),
        '/tf_static': ('tf2_msgs/msg/TFMessage', TFMessage, retained),
    }
    publishers = {topic: node.create_publisher(typ, topic, qos) for topic, (_, typ, qos) in types.items()}
    seen = {topic: -1 for topic in types}
    counts = {topic: 0 for topic in types}
    opener = build_opener(ProxyHandler({}))
    deadline = time.monotonic() + args.seconds
    log = out / 'simple-sim-relay.jsonl'
    workflow['append'](log, {'event': 'started', 'pid': os.getpid(), 'url': 'http://127.0.0.1:17877/snapshot'})
    print('Relaying Docker simulation to localhost ROS domain 187. Ctrl-C stops it.', flush=True)
    try:
        while rclpy.ok() and time.monotonic() < deadline:
            with opener.open('http://127.0.0.1:17877/snapshot', timeout=3) as response:
                payload = response.read(MAX_SNAPSHOT_BYTES + 1)
            for topic, sequence, cdr in decode_snapshot(payload, seen, types):
                message = deserialize_message(cdr, types[topic][1])
                publishers[topic].publish(message)
                seen[topic] = sequence
                counts[topic] += 1
            rclpy.spin_once(node, timeout_sec=0.1)
    except KeyboardInterrupt:
        pass
    finally:
        # A terminal/Pixi process group can deliver SIGINT more than once.
        # Finish releasing ROS publishers even if another interrupt arrives.
        previous_handler = signal.signal(signal.SIGINT, signal.SIG_IGN)
        try:
            workflow['append'](log, {'event': 'stopped', 'published_counts': counts})
            print(json.dumps({'published_counts': counts}), flush=True)
            try:
                node.destroy_node()
            finally:
                if rclpy.ok():
                    rclpy.shutdown()
        finally:
            signal.signal(signal.SIGINT, previous_handler)


if __name__ == '__main__':
    main()
