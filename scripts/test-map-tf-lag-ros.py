"""Actual backend regression: bounded map replacement with delayed ROS TF.

Run via scripts/dev and project Python. Only localhost/domain187 is permitted.
Each middleware runs in a separate process. No robot or external peers are used.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import queue
import signal
import subprocess
import sys
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
REMOTE = ('ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT',
          'CYCLONEDDS_URI', 'FASTDDS_DEFAULT_PROFILES_FILE',
          'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE')
RMWS = ('rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp')


def run_case(folder):
    assert os.environ['ROS_DOMAIN_ID'] == '187'
    assert os.environ['ROS_LOCALHOST_ONLY'] == '1'
    assert os.environ['ROS_AUTOMATIC_DISCOVERY_RANGE'] == 'LOCALHOST'
    assert os.environ['RMW_IMPLEMENTATION'] in RMWS
    assert not any(os.environ.get(key) for key in REMOTE)
    import rclpy
    from nav_msgs.msg import OccupancyGrid
    from tf2_msgs.msg import TFMessage
    from geometry_msgs.msg import TransformStamped
    rclpy.init()
    node = rclpy.create_node('studio_map_tf_lag_fixture')
    maps = node.create_publisher(OccupancyGrid, '/studio_fixture/tf_lag_map', 2)
    transforms = node.create_publisher(TFMessage, '/studio_fixture/tf_lag_tf', 10)
    events = queue.Queue(maxsize=512)
    log = (folder / 'backend-events.jsonl').open('w')
    errors = (folder / 'backend-stderr.log').open('w')
    child = subprocess.Popen([str(ROOT / 'target/debug/studio-backend'), '--observe-session'],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=errors, text=True)
    def read():
        for line in child.stdout:
            log.write(line)
            log.flush()
            try:
                events.put_nowait(json.loads(line))
            except queue.Full:
                raise RuntimeError('Bounded test event queue overflow')
    reader = threading.Thread(target=read, daemon=True)
    reader.start()
    def send(value):
        child.stdin.write(json.dumps(value) + '\n')
        child.stdin.flush()
    def wait(predicate, timeout=15):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                event = events.get(timeout=.05)
            except queue.Empty:
                continue
            if predicate(event):
                return event
        raise AssertionError('Backend event timeout')
    def sample(event):
        return event.get('sample', {}) if event.get('event') == 'map_snapshot_observation' else {}
    def stamp(message, ns):
        message.header.stamp.sec, message.header.stamp.nanosec = divmod(ns, 1_000_000_000)
    base = time.time_ns()
    def tf(ns):
        message = TransformStamped()
        message.header.frame_id, message.child_frame_id = 'world', 'source'
        stamp(message, ns)
        message.transform.rotation.w = 1.
        message.transform.translation.x = (ns - base) / 1_000_000_000
        transforms.publish(TFMessage(transforms=[message]))
    def full(ns):
        message = OccupancyGrid()
        message.header.frame_id = 'source'
        stamp(message, ns)
        message.info.width = message.info.height = 2
        message.info.resolution = .5
        message.info.origin.orientation.w = 1.
        message.data = [0, 20, 40, 100]
        maps.publish(message)
    result = {'outcome': 'failed', 'backend_pid': child.pid, 'publisher_pid': os.getpid()}
    try:
        wait(lambda e: e.get('event') == 'ready')
        send(dict(command='start', generation=1, request=1, subscription=1,
                  configuration=dict(topic='/studio_fixture/tf_lag_map',
                    message_type='nav_msgs/msg/OccupancyGrid', best_effort=False,
                    transient_local=False, output='map_snapshot', robot=None,
                    spatial=dict(fixed_frame='world', clock_policy='verified_system',
                        dynamic_tf_topic='/studio_fixture/tf_lag_tf',
                        dynamic_tf_best_effort=False, static_tf_topic='/studio_fixture/tf_lag_static'))))
        wait(lambda e: e.get('event') == 'started')
        deadline = time.monotonic() + 10
        while maps.get_subscription_count() != 1 or transforms.get_subscription_count() != 1:
            assert time.monotonic() < deadline, 'DDS matching timeout'
            rclpy.spin_once(node, timeout_sec=.02)
        tf(base - 300_000_000)
        tf(base)
        full(base - 150_000_000)
        wait(lambda e: sample(e).get('kind') == 'chunk')
        # A 250 ms TF lead must not clear the already complete map.
        replacement = base + 300_000_000
        full(replacement)
        end = time.monotonic() + .25
        while time.monotonic() < end:
            try:
                event = events.get(timeout=.02)
            except queue.Empty:
                continue
            assert sample(event).get('kind') != 'clear', event
        tf(base + 600_000_000)
        event = wait(lambda e: sample(e).get('kind') == 'chunk')
        header = sample(event)['header']
        assert header['metadata']['stamp_ns'] == replacement
        assert abs(header['transform'][0] - .3) < 1e-6, header['transform']
        # Continuous new full maps cannot reset the last-valid-frame deadline.
        end = time.monotonic() + 1.5
        cleared = None
        while time.monotonic() < end:
            full(base + 10_000_000_000)
            until = time.monotonic() + .1
            while time.monotonic() < until:
                try:
                    event = events.get(timeout=.01)
                except queue.Empty:
                    continue
                if sample(event).get('kind') == 'clear':
                    cleared = sample(event)
                    break
            if cleared:
                break
        assert cleared and cleared['reason'] == 'lookup_timeout', cleared
        send(dict(command='stop', generation=1, request=2, subscription=1))
        wait(lambda e: e.get('event') == 'stopped')
        result.update(outcome='passed', checks=['no clear during 250ms TF lag',
            'replacement uses acquisition-time interpolation',
            'continuous input reaches bounded timeout', 'explicit Stop acknowledged'])
    finally:
        child.stdin.close()
        try:
            child.wait(timeout=8)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
        reader.join(timeout=2)
        log.close()
        errors.close()
        node.destroy_node()
        rclpy.shutdown()
        result['backend_exit_code'] = child.returncode
        (folder / 'result.json').write_text(json.dumps(result, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', required=True)
    parser.add_argument('--case', choices=RMWS)
    args = parser.parse_args()
    assert Path(sys.prefix).resolve() == (ROOT / '.pixi/envs/default').resolve()
    assert args.label.replace('-', '').replace('_', '').isalnum()
    folder = ROOT / 'artifacts/feasibility/evidence' / args.label
    if args.case:
        run_case(folder / args.case)
        return
    folder.mkdir(parents=True, exist_ok=False)
    rows = []
    for rmw in RMWS:
        case = folder / rmw
        case.mkdir()
        env = {k: v for k, v in os.environ.items() if k not in REMOTE}
        env.update(ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1',
            ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST', SKIP_DEFAULT_XML='1',
            RMW_IMPLEMENTATION=rmw, DYLD_LIBRARY_PATH=str(ROOT / '.pixi/envs/default/lib'))
        command = [sys.executable, __file__, '--label', args.label, '--case', rmw]
        timed_out = False
        with (case / 'fixture.log').open('w') as log:
            process = subprocess.Popen(command, env=env, stdout=log, stderr=log,
                                       start_new_session=os.name == 'posix')
            try:
                process.wait(timeout=55)
            except subprocess.TimeoutExpired:
                timed_out = True
                log.write('Fixture exceeded its 55 second bound. Stopping its owned process group.\n')
                def terminate(sig):
                    try:
                        if os.name == 'posix':
                            os.killpg(process.pid, sig)
                        else:
                            process.kill()
                    except ProcessLookupError:
                        pass
                terminate(signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    terminate(signal.SIGKILL)
                    process.wait()
        actual = json.loads((case / 'result.json').read_text()) if (case / 'result.json').exists() else None
        remaining = []
        if actual:
            for pid in [actual['backend_pid'], actual['publisher_pid']]:
                try:
                    os.kill(pid, 0)
                    remaining.append(pid)
                except ProcessLookupError:
                    pass
        rows.append(dict(outcome='passed' if not timed_out and not remaining and process.returncode == 0 and actual and actual['outcome'] == 'passed' else 'failed',
            os=platform.platform(), cpu=platform.machine(), distro='jazzy', middleware_pair=[rmw, rmw],
            command=command, actual=actual, remaining_pids=remaining, timed_out=timed_out,
            versions=dict(python=platform.python_version(), rust=subprocess.check_output(['rustc', '--version'], text=True).strip(), rclrs='0.8.0', protocol=21),
            hashes={name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in ['Cargo.lock', 'pixi.lock', 'target/debug/studio-backend']},
            evidence=str(case.relative_to(ROOT)),
            scope='Actual backend and local synthetic ROS publishers; no native window, LAN or physical robot acceptance'))
        print(rmw, rows[-1]['outcome'], flush=True)
    (ROOT / 'artifacts/feasibility' / (args.label + '.json')).write_text(json.dumps(rows, indent=2) + '\n')
    assert all(row['outcome'] == 'passed' for row in rows)


if __name__ == '__main__':
    main()
