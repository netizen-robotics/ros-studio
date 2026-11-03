"""Explicit development-only Map diagnostic profiles; never runtime defaults.

Incoming custom profiles/remote discovery remain rejected. The only permitted
profiles are the two reviewed repository fixtures, copied into owned evidence.
"""
import hashlib
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REMOTE = ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI',
          'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']
PROFILES = {
    'rmw_fastrtps_cpp': ('FASTRTPS_DEFAULT_PROFILES_FILE', 'localhost-fast-map.xml'),
    'rmw_cyclonedds_cpp': ('CYCLONEDDS_URI', 'localhost-cyclone-map.xml'),
}


def configure(folder, environment=None):
    env = os.environ if environment is None else environment
    expected = {'STUDIO_MAP_DIAGNOSTIC_PROFILES': '1', 'ROS_DISTRO': 'jazzy', 'ROS_DOMAIN_ID': '187',
                'ROS_LOCALHOST_ONLY': '1', 'ROS_AUTOMATIC_DISCOVERY_RANGE': 'LOCALHOST'}
    if any(env.get(k) != value for k, value in expected.items()):
        raise ValueError('Map profiles require explicit diagnostic opt-in and isolated Jazzy domain187')
    if any(env.get(k) for k in REMOTE):
        raise ValueError('Remove incoming custom profiles and remote discovery settings')
    rmw = env.get('RMW_IMPLEMENTATION')
    if rmw not in PROFILES:
        raise ValueError('Choose a matching supported RMW')
    folder = Path(folder).resolve()
    if not folder.is_relative_to(ROOT / 'artifacts/feasibility/evidence'):
        raise ValueError('Map profile evidence must stay inside ignored artifacts/feasibility/evidence')
    key, name = PROFILES[rmw]
    source = ROOT / 'fixtures/dds' / name
    raw = source.read_bytes()
    target = folder / 'map-dds-profile.xml'
    if target.exists():
        if target.read_bytes() != raw:
            raise ValueError('Existing diagnostic profile differs from the reviewed source')
    else:
        target.write_bytes(raw)
    env[key] = str(target)
    return {'environment_key': key, 'path': str(target), 'source': str(source),
            'sha256': hashlib.sha256(raw).hexdigest(), 'scope': 'Explicit localhost Map diagnostic only'}
