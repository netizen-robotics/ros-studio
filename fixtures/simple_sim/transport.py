"""Versioned, bounded fixture endpoint contract; no ROS or network imports."""
import base64
import json

FIXTURE_VERSION = 2
MAX_CDR_BYTES = 16384
MAX_SNAPSHOT_BYTES = 131072
LEGACY_TOPICS = {
    '/studio_sim/odometry': 'nav_msgs/msg/Odometry',
    '/studio_sim/joint_states': 'sensor_msgs/msg/JointState',
    '/studio_sim/scan': 'sensor_msgs/msg/LaserScan',
    '/studio_sim/status': 'std_msgs/msg/String',
    '/studio_sim/robot_description': 'std_msgs/msg/String',
    '/tf': 'tf2_msgs/msg/TFMessage',
    '/tf_static': 'tf2_msgs/msg/TFMessage',
}
TOPICS = {**LEGACY_TOPICS, '/studio_sim/camera': 'sensor_msgs/msg/Image'}


def topics_for_version(version):
    if type(version) is not int or version not in (1, FIXTURE_VERSION):
        raise ValueError('Unsupported fixture transport version')
    return LEGACY_TOPICS if version == 1 else TOPICS


def encode_snapshot(samples):
    """Startup may have a subset, but unknown endpoints never enter a snapshot."""
    if not isinstance(samples, dict) or len(samples) > len(TOPICS):
        raise ValueError('Unexpected fixture endpoint set')
    for topic, sample in samples.items():
        if topic not in TOPICS or not isinstance(sample, dict) or sample.get('type') != TOPICS[topic]:
            raise ValueError('Unexpected fixture topic/type')
        if set(sample) != {'type', 'sequence', 'cdr'} or type(sample['sequence']) is not int or sample['sequence'] < 0:
            raise ValueError('Invalid fixture sample metadata')
        cdr = sample['cdr']
        if not isinstance(cdr, str) or len(cdr) > ((MAX_CDR_BYTES + 2) // 3) * 4:
            raise ValueError('Oversized fixture sample')
        raw = base64.b64decode(cdr, validate=True)
        if len(raw) > MAX_CDR_BYTES or base64.b64encode(raw).decode('ascii') != cdr:
            raise ValueError('Invalid or oversized fixture sample')
    payload = json.dumps({'fixture_version': FIXTURE_VERSION, 'samples': samples}).encode()
    if len(payload) > MAX_SNAPSHOT_BYTES:
        raise ValueError('Oversized fixture snapshot')
    return payload
