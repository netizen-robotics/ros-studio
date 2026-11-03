"""Developer-only localhost/domain187 Marker publisher for compiled-window checks.

Write an atomic request.json {id, op, ...} inside --evidence. Supported operations:
scenario (rich/missing/finite), connect_frame, counts, rewrite_mesh, exit.
Only the known synthetic ROS publisher runs; no robot commands or remote peers.
The app/backend must select the matching middleware explicitly.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
REMOTE = ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI',
          'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']


def save(path, value):
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


def scenario(folder, name):
    points = [[-.6, -.3, 0.], [0., .4, .1], [.6, -.3, .2]]
    colors = [[.9, .3, .2, 1.], [.2, .8, .5, 1.], [.3, .5, .9, 1.]]
    markers = []
    for kind in range(12):
        spec = dict(id=kind, namespace='native_ros', type=kind,
                    x=(kind % 4 - 1.5) * 3., y=(kind // 4 - 1.) * 2.5, z=.5,
                    scale=[.7, .7, .7], color=[.9, .6, .25, 1.])
        if kind == 0:
            spec['scale'] = [1., .12, .25]
        if kind in (4, 5):
            spec.update(points=points[:2] if kind == 5 else points,
                        colors=colors[:2] if kind == 5 else colors, scale=[.1, .1, .1])
        if kind in (6, 7, 8):
            spec.update(points=points, colors=colors, scale=[.22, .22, .22])
        if kind == 8:
            spec['namespace'] = 'native_ros/long_namespace_' + 'sensor_' * 28
        if kind == 9:
            spec.update(text='ROS Marker · 机器人', scale=[1., 1., .3])
        if kind == 10:
            spec.update(mesh_resource=(folder / 'mesh.obj').as_uri(), embedded=False, color=[.9, .2, .65, 1.])
        if kind == 11:
            spec.update(points=points, colors=colors)
        if kind == 2:
            spec.update(locked=True, frame='missing' if name == 'missing' else 'source')
        if kind == 1 and name == 'finite':
            spec['lifetime_ns'] = 4000000000
        markers.append(spec)
    return markers


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--evidence', required=True)
    parser.add_argument('--rmw', choices=['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp'], required=True)
    parser.add_argument('--seconds', type=int, default=1800)
    args = parser.parse_args()
    if Path(sys.prefix).resolve() != (ROOT / '.pixi/envs/default').resolve():
        parser.error('Use the project Pixi Python')
    folder = Path(args.evidence).resolve()
    if not folder.is_relative_to(ROOT / 'artifacts/feasibility/evidence') or not 1 <= args.seconds <= 3600:
        parser.error('Use an ignored evidence folder and at most3600seconds')
    folder.mkdir(parents=True, exist_ok=True)
    assert not (folder / 'driver-start.json').exists(), 'Use a unique folder'
    triangle = 'v -0.8 -0.5 0\nv 0.8 -0.5 0\nv 0 0.8 0\nf 1 2 3\n'
    (folder / 'mesh.obj').write_text(triangle)
    env = {k: v for k, v in os.environ.items() if k not in REMOTE}
    env.update(ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1', ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST',
               ROS_DISTRO='jazzy', RMW_IMPLEMENTATION=args.rmw,
               DYLD_LIBRARY_PATH=str(ROOT / '.pixi/envs/default/lib'))
    command = [str(ROOT / '.pixi/envs/default/bin/python'), str(ROOT / 'crates/backend/tests/fixtures/marker_publisher.py')]
    events = queue.Queue(maxsize=8)
    error_log = (folder / 'publisher.stderr.log').open('w')
    process = subprocess.Popen(command, cwd=ROOT, env=env, stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=error_log, text=True)

    def read():
        for line in process.stdout:
            events.put(json.loads(line))

    reader = threading.Thread(target=read, daemon=True)
    reader.start()

    def rpc(value):
        process.stdin.write(json.dumps(value) + '\n')
        process.stdin.flush()
        result = events.get(timeout=20)
        with (folder / 'publisher-rpc.jsonl').open('a') as log:
            log.write(json.dumps(dict(time=time.time(), command=value, response=result)) + '\n')
        return result

    status = 'failed'
    try:
        ready = events.get(timeout=20)
        assert ready == dict(ready=True, pid=process.pid)
        save(folder / 'driver-start.json', dict(driver_pid=os.getpid(), publisher_pid=process.pid, command=command,
             environment={k: env[k] for k in ['ROS_DOMAIN_ID', 'ROS_LOCALHOST_ONLY', 'ROS_AUTOMATIC_DISCOVERY_RANGE', 'RMW_IMPLEMENTATION']},
             publisher_sha256=hashlib.sha256(Path(command[1]).read_bytes()).hexdigest(), started_at=time.time()))
        save(folder / 'ready.json', ready)
        print('ready', args.rmw, 'publisher', process.pid, flush=True)
        last_id, due = 0, 0.
        deadline = time.monotonic() + args.seconds
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError('Synthetic ROS publisher exited unexpectedly')
            request_path = folder / 'request.json'
            request = json.loads(request_path.read_text()) if request_path.exists() else None
            if request and request['id'] > last_id:
                last_id = request['id']
                op = request['op']
                if op == 'exit':
                    save(folder / 'response.json', dict(id=last_id, stopped=True))
                    status = 'passed'
                    break
                if op == 'scenario':
                    assert request['name'] in ['rich', 'missing', 'finite']
                    result = rpc(dict(op='publish', target='array', markers=scenario(folder, request['name'])))
                elif op in ['counts', 'connect_frame']:
                    result = rpc(dict(op=op))
                elif op == 'rewrite_mesh':
                    # This owned synthetic asset changes without a new Marker message.
                    (folder / 'mesh.obj').write_text('v -1 -0.5 0\nv 1 -0.5 0\nv 1 0.5 0\nv -1 0.5 0\nf 1 2 3\nf 1 3 4\n')
                    result = dict(rewritten=True, sha256=hashlib.sha256((folder / 'mesh.obj').read_bytes()).hexdigest())
                else:
                    raise ValueError('Unknown development fixture operation')
                save(folder / 'response.json', dict(id=last_id, result=result))
            if time.monotonic() >= due:
                rpc(dict(op='image'))
                due = time.monotonic() + .5
            time.sleep(.02)
        else:
            status = 'timeout'
    finally:
        process.stdin.close()
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
            status = 'failed'
        reader.join(timeout=2)
        error_log.close()
        save(folder / 'driver-finish.json', dict(outcome=status, publisher_exit_code=process.returncode,
                                               publisher_pid=process.pid, driver_pid=os.getpid(), ended_at=time.time()))
    if status != 'passed' or process.returncode:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
