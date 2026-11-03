"""Native fixture smoke test. Run with scripts/dev .pixi/envs/default/bin/python.
No physical robot endpoints: uses dedicated domain 187 and synthetic topics.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]

def peer(custom, echo, large=False):
    import rclpy
    from rclpy.executors import ExternalShutdownException
    from rclpy.qos import QoSProfile, ReliabilityPolicy
    if custom:
        from feasibility_interfaces.msg import Sample as Message
    else:
        from std_msgs.msg import String as Message
    rclpy.init()
    node = rclpy.create_node('studio_synthetic_peer')
    pub = node.create_publisher(Message, '/studio_fixture', QoSProfile(depth=10, reliability=ReliabilityPolicy.RELIABLE))
    msg = Message()
    if custom:
        msg.sequence = 42
        msg.nested.label = 'feasibility-proof'
        msg.nested.value = 3.25
        msg.fixed = [1.0, 2.0, 3.0]
        msg.bounded = [1, 2, 3]
        msg.labels = ['nested', 'runtime']
        msg.payload = list(range(256)) * 8192 if large else list(range(16))
    else:
        msg.data = 'feasibility-proof'
    if echo:
        def received(value):
            if value == msg:
                print('ECHO_VERIFIED', flush=True)
        subscription = node.create_subscription(Message, '/studio_echo', received, 10)
    try:
        while True:
            pub.publish(msg)
            rclpy.spin_once(node, timeout_sec=0.1)
    except (ExternalShutdownException, KeyboardInterrupt):
        pass
    finally:
        node.destroy_node()
        rclpy.try_shutdown()

def control():
    import rclpy
    from std_msgs.msg import String
    from rclpy.executors import ExternalShutdownException
    rclpy.init()
    node = rclpy.create_node('python_control_subscriber')
    def receive(msg):
        print(json.dumps({'event':'sample','topic':'/studio_fixture','fields':[['data',msg.data]]}), flush=True)
    sub = node.create_subscription(String, '/studio_fixture', receive, 10)
    node.create_timer(.5, lambda: print(json.dumps({'event':'graph','topics':node.get_topic_names_and_types()}), flush=True))
    try: rclpy.spin(node)
    except (ExternalShutdownException, KeyboardInterrupt): pass
    finally: node.destroy_node(); rclpy.try_shutdown()

def run(custom, backend, echo=False, subnet=False, python_control=False, large=False, same_rmw=False):
    records = []
    artifact = ROOT / 'artifacts/feasibility/evidence/smoke'
    artifact.mkdir(parents=True, exist_ok=True)
    for local in ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']:
        for remote in ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']:
            if same_rmw and local != remote: continue
            if python_control and (local != 'rmw_fastrtps_cpp' or remote != 'rmw_cyclonedds_cpp'):
                continue
            variant = ('-echo' if echo else '') + ('-subnet' if subnet else '')
            if python_control: variant += '-python-control'
            if large: variant += '-large'
            name = f'{"custom" if custom else "standard"}{variant}-{local}-{remote}'
            env = dict(os.environ, ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1', ROS_LOG_DIR=str(artifact / 'ros-logs'), RCUTILS_LOGGING_USE_STDOUT='0')
            if subnet:
                env.pop('ROS_LOCALHOST_ONLY', None)
                env['ROS_AUTOMATIC_DISCOVERY_RANGE'] = 'SUBNET'
            if custom:
                prefix = ROOT / 'artifacts/fixture-install'
                env['AMENT_PREFIX_PATH'] = str(prefix) + os.pathsep + env['AMENT_PREFIX_PATH']
                env['PYTHONPATH'] = str(prefix / 'lib/python3.12/site-packages') + os.pathsep + env.get('PYTHONPATH', '')
                env['DYLD_LIBRARY_PATH'] = str(prefix / 'lib') + os.pathsep + str(ROOT / '.pixi/envs/default/lib')
            output = artifact / f'{name}.jsonl'
            errors = artifact / f'{name}.stderr'
            typ = 'feasibility_interfaces/msg/Sample' if custom else 'std_msgs/msg/String'
            cmd = [str(backend), '/studio_fixture', typ]
            if python_control: cmd = [sys.executable, __file__, '--control']
            if echo: cmd.append('--fixture-echo')
            started = time.monotonic()
            received = False
            graph_seen = False
            procs = []
            with output.open('w') as out, errors.open('w') as err:
                try:
                    procs.append(subprocess.Popen([sys.executable, __file__, '--peer'] + (['--custom'] if custom else []) + (['--echo'] if echo else []) + (['--large'] if large else []), env=dict(env, RMW_IMPLEMENTATION=remote), stdout=err, stderr=err))
                    procs.append(subprocess.Popen(cmd, env=dict(env, RMW_IMPLEMENTATION=local), stdout=out, stderr=err))
                    deadline = time.monotonic() + 25
                    while time.monotonic() < deadline:
                        time.sleep(.25)
                        for line in output.read_text().splitlines():
                            try:
                                event = json.loads(line)
                            except json.JSONDecodeError:
                                continue
                            if event.get('event') == 'sample' and 'feasibility-proof' in line:
                                received = True
                            if event.get('event') == 'graph' and '/studio_fixture' in line:
                                graph_seen = True
                        if (received and graph_seen and (not echo or 'ECHO_VERIFIED' in errors.read_text())) or any(p.poll() is not None for p in procs):
                            break
                finally:
                    for p in procs:
                        if p.poll() is None:
                            p.terminate()
                    for p in procs:
                        try: p.wait(timeout=3)
                        except subprocess.TimeoutExpired:
                            p.kill(); p.wait()
            success = received and graph_seen and (not echo or 'ECHO_VERIFIED' in errors.read_text())
            records.append(dict(test=name, os=platform.platform(), cpu=platform.machine(), distro=os.environ.get('ROS_DISTRO'), versions={'rclrs':'0.8.0','environment_lock':'pixi.lock','rust_lock':'Cargo.lock'}, middleware_pair=[local,remote], command=cmd, outcome='passed' if success else 'failed', seconds=round(time.monotonic()-started,3), evidence=[str(output.relative_to(ROOT)),str(errors.relative_to(ROOT))], scope='same-machine synthetic dynamic messages and graph; echo checked' if echo else 'same-machine synthetic dynamic subscription and graph only'))
            print(json.dumps(records[-1]), flush=True)
    dest = ROOT / 'artifacts/feasibility' / (('custom' if custom else 'standard') + variant + '-smoke.json')
    dest.write_text(json.dumps(records, indent=2)+'\n')
    return 0 if all(r['outcome']=='passed' for r in records) else 1

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--peer', action='store_true')
    parser.add_argument('--custom', action='store_true')
    parser.add_argument('--echo', action='store_true')
    parser.add_argument('--subnet', action='store_true')
    parser.add_argument('--control', action='store_true')
    parser.add_argument('--python-control', action='store_true')
    parser.add_argument('--large', action='store_true')
    parser.add_argument('--same-rmw', action='store_true')
    parser.add_argument('--backend', type=Path, default=ROOT/'target/debug/studio-backend')
    args = parser.parse_args()
    if args.large and not args.custom: parser.error('--large requires --custom')
    if args.python_control and (args.custom or args.echo or args.same_rmw): parser.error('--python-control is a standard-message asymmetric baseline')
    if args.control: control()
    elif args.peer: peer(args.custom, args.echo, args.large)
    else: sys.exit(run(args.custom, args.backend.resolve(), args.echo, args.subnet, args.python_control, args.large, args.same_rmw))
