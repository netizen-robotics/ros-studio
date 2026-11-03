"""Compare standard rclpy service peers on synthetic localhost/domain 187."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('service_suite', ROOT/'scripts/service-feasibility.py')
suite_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(suite_module)


def peer():
    if os.environ.get('ROS_DOMAIN_ID') != '187' or os.environ.get('ROS_LOCALHOST_ONLY') != '1':
        raise SystemExit('Fixture requires localhost/domain 187')
    import rclpy
    from feasibility_interfaces.srv import Inspect
    from rosidl_runtime_py.set_message import set_message_fields
    from rosidl_runtime_py.convert import message_to_ordereddict
    rclpy.init()
    node = rclpy.create_node('studio_service_python_control')
    client = node.create_client(Inspect, '/studio_fixture/inspect')
    deadline = time.monotonic() + 5
    result = {'request_state': 'not_sent'}
    try:
        while not client.service_is_ready() and time.monotonic() < deadline:
            rclpy.spin_once(node, timeout_sec=min(.05, max(0, deadline-time.monotonic())))
        if not client.service_is_ready():
            result['error'] = 'server_unavailable'
        else:
            request = Inspect.Request()
            set_message_fields(request, suite_module.INSPECT)
            future = client.call_async(request)  # Exactly one send; no retry.
            result['request_state'] = 'sent'
            while not future.done() and time.monotonic() < deadline:
                rclpy.spin_once(node, timeout_sec=min(.05, max(0, deadline-time.monotonic())))
            if future.done():
                result.update(request_state='response_received', response=message_to_ordereddict(future.result()))
            else:
                result['error'] = 'response_timeout'
        print(json.dumps(result), flush=True)
    finally:
        node.destroy_node()
        rclpy.try_shutdown()
    return 0 if result['request_state'] == 'response_received' else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--peer', action='store_true')
    parser.add_argument('--distro', choices=['humble', 'jazzy', 'lyrical'], default='jazzy')
    parser.add_argument('--label', required=True)
    args = parser.parse_args()
    if args.peer:
        return peer()
    if not args.label.replace('-', '').isalnum():
        parser.error('Unique alphanumeric label required')
    m = suite_module
    m.DISTRO = args.distro
    m.WORKSPACE = ROOT if args.distro == 'jazzy' else ROOT/'environments'/args.distro
    m.RUNTIME = m.WORKSPACE/'.pixi/envs/default'
    if Path(sys.prefix).resolve() != m.RUNTIME.resolve() or os.environ.get('ROS_DISTRO') != args.distro:
        parser.error('Use selected distro project environment')
    if args.distro != 'jazzy':
        m.INSTALL_SUFFIX = '-'+args.distro
        m.PROBE = ROOT/'target'/args.distro/'debug/studio-service-probe'
        m.DESKTOP = ROOT/'target'/args.distro/'debug/studio-desktop'
    suite = m.Suite(args.label)
    suite.build_bundle('feasibility_interfaces')
    install = ROOT/'artifacts'/('feasibility_interfaces-service-install'+m.INSTALL_SUFFIX)
    for local in m.RMWS:
        for remote in m.RMWS:
            with suite.server('inspect', remote) as server:
                env = dict(suite.base, RMW_IMPLEMENTATION=local,
                           PYTHONPATH=str(install/f'lib/python{sys.version_info.major}.{sys.version_info.minor}/site-packages'),
                           AMENT_PREFIX_PATH=str(install)+os.pathsep+str(m.RUNTIME),
                           DYLD_LIBRARY_PATH=str(install/'lib')+os.pathsep+str(m.RUNTIME/'lib'))
                command = [sys.executable, str(Path(__file__).resolve()), '--peer', '--label', args.label]
                stem = suite.out/(local+'-'+remote+'-python')
                completed = subprocess.run(command, env=env, capture_output=True, text=True, timeout=20)
                stem.with_suffix('.stdout').write_text(completed.stdout)
                stem.with_suffix('.stderr').write_text(completed.stderr)
                try:
                    result = json.loads(completed.stdout)
                except ValueError:
                    result = {'error': 'invalid_output'}
                requests = suite.requests(server[0])
                good = completed.returncode == 0 and result.get('response') == {'accepted': True, 'reason': 'validated'} and requests == [m.INSPECT]
                suite.record('python-service-control', good, command, local, remote,
                             actual=result, received_requests=requests, exit_code=completed.returncode,
                             server_command=server[1], harness_sha256=m.digest(Path(__file__)),
                             environment={k:env[k] for k in ['ROS_DOMAIN_ID','ROS_LOCALHOST_ONLY','RMW_IMPLEMENTATION','AMENT_PREFIX_PATH','DYLD_LIBRARY_PATH','PYTHONPATH']},
                             evidence=[str(p.relative_to(m.RAW)) for p in [stem.with_suffix('.stdout'),stem.with_suffix('.stderr'),server[0],server[2]]])
    return 0 if all(r['outcome']=='passed' for r in suite.records) else 1


if __name__ == '__main__':
    raise SystemExit(main())
