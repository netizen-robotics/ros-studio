"""Session11 TF graph backend acceptance with localhost synthetic ROS, both matching RMWs.
Run through scripts/dev .pixi/envs/default/bin/python. No native UI/hardware claim.
"""
import argparse
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import signal
import subprocess
import sys
import time
import traceback

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


SENSOR = load('tf_graph_sensor_acceptance', 'scripts/test-sensor-session.py')
SPLIT = load('tf_graph_split_fixture', 'scripts/test-split-apps-ros.py')


def configuration(fixed='split_world'):
    return dict(topic='/tf', message_type='tf2_msgs/msg/TFMessage', output='tf_graph',
                best_effort=False, transient_local=False, robot=None,
                spatial=dict(fixed_frame=fixed, dynamic_tf_topic='/tf', static_tf_topic='/tf_static',
                             dynamic_tf_best_effort=False, clock_policy='verified_system'))


def graph(backend, sub, status='ready'):
    def match(event):
        if event.get('event') != 'tf_graph_observation' or event.get('subscription') != sub:
            return False
        sample = event['sample']
        assert sample['contract_version'] == 1 and sample['fixed_frame'] == 'split_world'
        frames = sample['frames']
        assert len(frames) <= 256 and sum(f['parent'] is not None for f in frames) <= 128
        assert [f['name'] for f in frames] == sorted({f['name'] for f in frames})
        values = {f['name']: f for f in frames}
        if set(values) != {'split_world', 'split_laser', 'split_base'}:
            return False
        assert values['split_laser']['parent'] == dict(frame='split_world', kind='static', stamp_ns=0)
        assert values['split_base']['parent']['frame'] == 'split_world'
        assert values['split_base']['parent']['kind'] == 'dynamic'
        assert values['split_laser']['pose']['status'] == 'ready'
        assert values['split_laser']['pose']['resolved_stamp_ns'] == 0
        return values['split_base']['pose']['status'] == status
    return backend.wait(match, seconds=12)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', required=True)
    args = parser.parse_args()
    if Path(sys.prefix).resolve() != (ROOT / '.pixi/envs/default').resolve():
        parser.error('Use the project Pixi interpreter')
    if not args.label or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_' for c in args.label):
        parser.error('Use an unused alphanumeric evidence label')
    folder = ROOT / 'artifacts/feasibility/evidence' / args.label
    report = ROOT / 'artifacts/feasibility' / (args.label + '.json')
    if folder.exists() or report.exists():
        parser.error('Preserve earlier evidence: choose unused paths')
    folder.mkdir(parents=True)
    rows = []
    sha = lambda p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest()
    versions = dict(rust=subprocess.check_output(['rustc', '--version'], text=True).strip(),
                    python=platform.python_version(), backend_sha256=sha('target/debug/studio-backend'),
                    cargo_lock_sha256=sha('Cargo.lock'), pixi_lock_sha256=sha('pixi.lock'))
    sources = ['scripts/test-tf-graph-session.py', 'scripts/test-split-apps-ros.py',
               'crates/protocol/src/session.rs', 'crates/protocol/src/tf_graph.rs',
               'crates/backend/src/tf_graph_snapshot.rs', 'crates/backend/src/scene_worker.rs',
               'crates/backend/src/spatial_session.rs', 'crates/backend/src/observation.rs']
    source_hashes = {path: sha(path) for path in sources}
    original = dict(os.environ)
    failed = False
    for rmw in SPLIT.RMWS:
        os.environ.clear()
        os.environ.update({k: v for k, v in original.items() if k not in SPLIT.REMOTE})
        os.environ.update(ROS_DISTRO='jazzy', ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1',
                          ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST', RMW_IMPLEMENTATION=rmw)
        out = folder / rmw
        out.mkdir()
        ready = out / 'publisher.json'
        publisher_command = [sys.executable, str(ROOT / 'scripts/test-split-apps-ros.py'), '--publisher', str(ready)]
        backends = []
        publisher = None
        completed = []
        pids = []
        outcome, details = 'passed', None
        try:
            with (out / 'publisher.log').open('w') as output:
                publisher = subprocess.Popen(publisher_command, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT)
            pids.append(publisher.pid)
            deadline = time.monotonic() + 90
            while not ready.exists():
                assert publisher.poll() is None and time.monotonic() < deadline, 'Publisher startup failed'
                time.sleep(.02)

            def connect(index):
                backend = SENSOR.Backend(out, index, 900 + index, deadline)
                backends.append(backend)
                pids.append(backend.process.pid)
                event = backend.wait(lambda e: e.get('event') == 'ready')
                assert set(event) == {'event', 'distro', 'rmw', *SENSOR.EXPECTED}
                assert all(event[k] == v for k, v in SENSOR.EXPECTED.items())
                assert event['rmw'] == rmw and event['distro'] == 'jazzy'
                backend.wait(lambda e: e.get('event') == 'graph')
                assert not backend.sequence, 'Handshake/discovery activated a subscription'
                return backend

            backend = connect(1)
            bad = configuration()
            bad['spatial']['clock_policy'] = 'unknown'
            request = backend.start_configuration(1, bad, ack=False)
            error = backend.wait(lambda e: e.get('event') == 'request_error' and e.get('request') == request)
            assert error['no_handle'] is True
            completed.append('discovery inactive; unverified clock rejected before handle creation')
            backend.start_configuration(2, configuration())
            graph(backend, 2)
            completed.append('complete sorted static/dynamic graph with native latest poses')
            request = backend.start_configuration(3, configuration(), ack=False)
            error = backend.wait(lambda e: e.get('event') == 'request_error' and e.get('request') == request)
            assert error['no_handle'] is True
            completed.append('second graph owner rejected without disrupting first')
            scan = configuration()
            scan.update(topic='/studio_fixture/split/scan', message_type='sensor_msgs/msg/LaserScan', output='spatial_scan')
            backend.start_configuration(4, scan)
            backend.ready_sample(4, 'spatial_observation')
            backend.stop_robot(2)
            backend.ready_sample(4, 'spatial_observation')
            backend.start_configuration(5, configuration())
            graph(backend, 5)
            completed.append('graph Stop/re-enable while acquisition-time scan peer keeps receiving')
            late = connect(2)
            late.start_configuration(1, configuration())
            graph(late, 1)
            late.close()
            completed.append('retained static late join and active graph EOF cleanup')
            publisher.send_signal(signal.SIGTERM)
            publisher.wait(timeout=10)
            assert publisher.returncode == 0
            graph(backend, 5, 'stale')
            completed.append('dynamic pose ages stale after publisher exit; static axes remain ready')
            backend.stop_robot(5)
            backend.stop_robot(4)
            backend.close()
            reconnect = connect(3)
            reconnect.close()
            completed.append('acknowledged final Drops/Stop and fresh reconnect without replay')
        except Exception:
            failed = True
            outcome, details = 'failed', traceback.format_exc()
        finally:
            for backend in backends:
                try:
                    backend.close()
                except Exception:
                    failed = True
                    outcome, details = 'failed', (details or '') + traceback.format_exc()
            if publisher is not None and publisher.poll() is None:
                publisher.send_signal(signal.SIGTERM)
                try:
                    publisher.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    publisher.kill()
                    publisher.wait(timeout=3)
                    failed = True
                    outcome, details = 'failed', (details or '') + 'Publisher required forced cleanup'
            gone = []
            for pid in pids:
                try:
                    os.kill(pid, 0)
                    absent = False
                except ProcessLookupError:
                    absent = True
                gone.append(dict(pid=pid, absent=absent))
            if not all(p['absent'] for p in gone):
                failed, outcome = True, 'failed'
            cleanup = dict(processes=gone, publisher_exit_code=None if publisher is None else publisher.returncode,
                           backend_exit_codes=[b.process.returncode for b in backends])
            (out / 'cleanup.json').write_text(json.dumps(cleanup, indent=2) + '\n')
            rows.append(dict(test='tf-graph-actual-backend', outcome=outcome, os=platform.platform(),
                             cpu=platform.machine(), distro='jazzy', middleware_pair=[rmw, rmw],
                             versions=versions, source_sha256=source_hashes,
                             command=['scripts/dev', '.pixi/envs/default/bin/python', 'scripts/test-tf-graph-session.py', '--label', args.label],
                             publisher_command=publisher_command, backend_command=['target/debug/studio-backend', '--observe-session'],
                             environment={k: os.environ[k] for k in ['ROS_DOMAIN_ID', 'ROS_LOCALHOST_ONLY', 'ROS_AUTOMATIC_DISCOVERY_RANGE', 'RMW_IMPLEMENTATION']},
                             completed_cases=completed, details=details, evidence=[str(p.relative_to(ROOT)) for p in sorted(out.iterdir())],
                             scope='Actual Rust backend/native tf2/DDS boundary; local synthetic ROS publishers. No native-window or hardware result.'))
            report.write_text(json.dumps(rows, indent=2) + '\n')
            print(rmw, outcome, len(completed), 'cases', flush=True)
    os.environ.clear()
    os.environ.update(original)
    if failed:
        raise SystemExit('TF graph backend acceptance failed; preserve report and logs')


if __name__ == '__main__':
    main()
