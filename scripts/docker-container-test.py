"""Linux ROS controls sharing Docker's host network; not native Mac validation."""
import argparse
import json
from pathlib import Path
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
(ROOT / 'artifacts/feasibility').mkdir(parents=True, exist_ok=True)
OUT = ROOT / 'artifacts/feasibility/evidence/docker-containers'
IMAGE = 'ros-studio-peer:jazzy'


def main():
    global OUT
    parser = argparse.ArgumentParser()
    parser.add_argument('--shared-ipc', action='store_true')
    args = parser.parse_args()
    suffix = '-shared-ipc' if args.shared_ipc else ''
    OUT = OUT.with_name(OUT.name + suffix)
    OUT.mkdir(parents=True, exist_ok=True)
    metadata = json.loads(subprocess.check_output(['docker', 'image', 'inspect', IMAGE]))[0]
    packages = subprocess.check_output(['docker', 'run', '--rm', IMAGE, 'cat', '/opt/fixture/packages.txt'], text=True)
    (OUT / 'packages.txt').write_text(packages)
    records = []
    for subscriber in ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']:
        for publisher in ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']:
            key = f'{subscriber}-{publisher}'
            names, processes, handles, commands = [], [], [], []
            started = time.monotonic()
            passed = False
            try:
                for role, rmw, flag in [('subscriber', subscriber, '--control'), ('publisher', publisher, '--peer')]:
                    name = 'studio-control-' + uuid.uuid4().hex[:12]
                    names.append(name)
                    command = ['docker', 'run', '--rm', '--network', 'host', '--name', name,
                               '-e', 'ROS_DOMAIN_ID=187', '-e', 'ROS_LOCALHOST_ONLY=1',
                               '-e', f'RMW_IMPLEMENTATION={rmw}', IMAGE,
                               '/opt/fixture-venv/bin/python', '/opt/fixture/smoke.py', flag]
                    if args.shared_ipc:
                        command[2:2] = ['--ipc', 'host']
                    commands.append(command)
                    log = (OUT / f'{key}-{role}.log').open('w')
                    handles.append(log)
                    processes.append(subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT))
                while time.monotonic() - started < 25:
                    text = (OUT / f'{key}-subscriber.log').read_text()
                    if '"event": "sample"' in text and 'feasibility-proof' in text:
                        passed = True
                        break
                    if any(p.poll() is not None for p in processes):
                        break
                    time.sleep(.25)
            finally:
                for name in names:
                    subprocess.run(['docker', 'rm', '-f', name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
                for process in processes:
                    process.wait(timeout=10)
                for handle in handles:
                    handle.close()
            records.append(dict(outcome='passed' if passed else 'failed', os='Linux container / Ubuntu 24.04',
                                cpu=metadata['Architecture'], distro='jazzy', image_id=metadata['Id'],
                                versions=f'evidence/{OUT.name}/packages.txt', subscriber=subscriber,
                                publisher=publisher, commands=commands, duration_seconds=round(time.monotonic()-started, 2),
                                scope='Python ROS control, shared Docker host network; no Rust backend or Mac network boundary',
                                evidence=[f'evidence/{OUT.name}/{key}-{role}.log' for role in ['subscriber', 'publisher']]))
            print(key, records[-1]['outcome'], flush=True)
    (OUT.parent.parent / ('docker-containers' + suffix + '.json')).write_text(json.dumps(records, indent=2) + '\n')
    return 0 if all(r['outcome'] == 'passed' for r in records) else 1


if __name__ == '__main__':
    raise SystemExit(main())
