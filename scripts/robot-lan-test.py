"""Read-only native LAN subscriptions. Never publishes or invokes robot services/actions.
Run through scripts/dev after verifying the peer's distro, domain and middleware.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
(ROOT / 'artifacts/feasibility').mkdir(parents=True, exist_ok=True)
OUT = ROOT / 'artifacts/feasibility/evidence/robot-lan'
CASES = [
    ('scan-reliable', '/scan', 'sensor_msgs/msg/LaserScan', [], 'header'),
    ('scan-best-effort', '/scan', 'sensor_msgs/msg/LaserScan', ['--best-effort'], 'header'),
    ('odometry', '/odometry/filtered', 'nav_msgs/msg/Odometry', [], 'header'),
    ('tf', '/tf', 'tf2_msgs/msg/TFMessage', [], 'transforms'),
    ('tf-static', '/tf_static', 'tf2_msgs/msg/TFMessage', ['--transient-local'], None),
    ('compressed-image', '/color/image_raw/compressed', 'sensor_msgs/msg/CompressedImage', [], 'header'),
    ('depth-image', '/depth/image_raw', 'sensor_msgs/msg/Image', ['--best-effort'], 'header'),
    ('point-cloud', '/camera/depth/points', 'sensor_msgs/msg/PointCloud2', ['--best-effort'], 'header'),
]


def run(args):
    global OUT
    suffix = '-' + args.label if args.label else ''
    OUT = OUT.with_name(OUT.name + suffix)
    OUT.mkdir(parents=True, exist_ok=True)
    records = []
    backend = ROOT / 'target/debug/studio-backend'
    digest = hashlib.sha256(backend.read_bytes()).hexdigest()
    for rmw in ['rmw_cyclonedds_cpp', 'rmw_fastrtps_cpp']:
        if args.rmw and rmw != args.rmw:
            continue
        for name, topic, message_type, flags, freshness_field in CASES:
            if args.case and name not in args.case:
                continue
            key = f'{rmw}-{name}'
            stdout = OUT / (key + '.jsonl')
            stderr = OUT / (key + '.stderr')
            env = dict(os.environ, ROS_DOMAIN_ID=str(args.domain), RMW_IMPLEMENTATION=rmw,
                       ROS_AUTOMATIC_DISCOVERY_RANGE='SUBNET')
            for variable in ['ROS_LOCALHOST_ONLY', 'ROS_STATIC_PEERS', 'CYCLONEDDS_URI', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_DEFAULT_PROFILES_FILE']:
                env.pop(variable, None)
            command = [str(backend), topic, message_type] + flags
            start = time.monotonic()
            samples, unique, topics = [], set(), set()
            with stdout.open('w') as out, stderr.open('w') as err:
                p = subprocess.Popen(command, env=env, stdout=out, stderr=err)
                try:
                    while time.monotonic() - start < args.timeout:
                        time.sleep(.25)
                        events = []
                        for line in stdout.read_text().splitlines():
                            try: events.append(json.loads(line))
                            except json.JSONDecodeError: pass
                        samples = [e for e in events if e.get('event') == 'sample' and e.get('topic') == topic and e.get('fields')]
                        unique = {dict(e['fields']).get(freshness_field) for e in samples} - {None}
                        topics = {t[0] for e in events if e.get('event') == 'graph' for t in e['topics']}
                        if (len(samples) >= 3 and len(unique) >= 2) or (freshness_field is None and samples):
                            break
                        if p.poll() is not None: break
                finally:
                    if p.poll() is None: p.terminate()
                    try: p.wait(timeout=5)
                    except subprocess.TimeoutExpired: p.kill(); p.wait()
            passed = bool(samples) if freshness_field is None else len(samples) >= 3 and len(unique) >= 2
            records.append(dict(test=name, outcome='passed' if passed else 'failed',
                                os=platform.platform(), cpu=platform.machine(), distro='jazzy',
                                peer=args.peer, peer_os='Ubuntu 24.04.4 Linux aarch64 (ROS container)',
                                domain=args.domain, middleware_pair=[rmw, 'rmw_cyclonedds_cpp'],
                                versions={'native':'native-dependencies.json','peer':'evidence/robot-lan/robot-runtime.txt','backend_sha256':digest},
                                command=command, environment={'ROS_DOMAIN_ID':str(args.domain),'RMW_IMPLEMENTATION':rmw,'ROS_AUTOMATIC_DISCOVERY_RANGE':'SUBNET'},
                                samples=len(samples), distinct_previews=len(unique), freshness_field=freshness_field,
                                topic_seen=topic in topics, seconds=round(time.monotonic()-start,3),
                                evidence=[str(f.relative_to(ROOT / 'artifacts/feasibility')) for f in [stdout,stderr]],
                                scope='Passive receipt of existing robot publications; preview decoding only, not rendering, full-payload validation or performance qualification'))
            print(key, records[-1]['outcome'], f'samples={len(samples)} distinct={len(unique)}', flush=True)
            (OUT.parent.parent / ('robot-lan' + suffix + '.json')).write_text(json.dumps(records,indent=2)+'\n')
    return 0 if all(r['outcome'] == 'passed' for r in records) else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--peer', required=True, help='Verified publishing host, recorded for attribution; discovery remains domain-wide')
    parser.add_argument('--domain', type=int, required=True)
    parser.add_argument('--timeout', type=float, default=15)
    parser.add_argument('--case', action='append', choices=[case[0] for case in CASES])
    parser.add_argument('--rmw', choices=['rmw_cyclonedds_cpp', 'rmw_fastrtps_cpp'])
    parser.add_argument('--label', default='', help='Separate evidence suffix for a targeted repeat')
    args = parser.parse_args()
    if not 0 <= args.domain <= 232 or not 0 < args.timeout <= 60:
        parser.error('domain must be 0..232 and timeout >0..60 seconds')
    if args.label and not args.label.replace('-', '').isalnum():
        parser.error('label must contain only letters, numbers and hyphens')
    raise SystemExit(run(args))
