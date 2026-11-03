"""Run focused Balanced Studio checks and retain versioned, local-only evidence."""
import argparse
import hashlib
import json
import platform
from pathlib import Path
import shlex
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', required=True)
    args = parser.parse_args()
    if not args.label or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_' for c in args.label):
        parser.error('Use a descriptive alphanumeric label with hyphens/underscores')
    report = ROOT / 'artifacts/feasibility' / f'{args.label}.json'
    evidence = ROOT / 'artifacts/feasibility/evidence' / args.label
    if report.exists() or evidence.exists():
        parser.error('Label already exists; choose a new label to preserve evidence')
    evidence.mkdir(parents=True)
    versions = {'rust': subprocess.check_output([str(ROOT / 'scripts/dev'), 'rustc', '--version'], text=True).strip(),
                'egui': '0.31.1', 'eframe': '0.31.1', 'python': platform.python_version()}
    for file in ['Cargo.lock', 'pixi.lock']:
        versions[f'{file}_sha256'] = hashlib.sha256((ROOT / file).read_bytes()).hexdigest()
    commands = [
        ('workspace-build', ['scripts/dev', 'cargo', 'build', '--locked', '--workspace']),
        ('desktop-protocol-tests', ['scripts/dev', 'cargo', 'test', '--locked', '-p', 'studio-desktop', '-p', 'studio-protocol', '--', '--nocapture']),
        ('desktop-clippy', ['scripts/dev', 'cargo', 'clippy', '--locked', '-p', 'studio-desktop', '--all-targets', '--', '-D', 'warnings']),
        ('format-check', ['scripts/dev', 'cargo', 'fmt', '--all', '--', '--check']),
    ]
    rows = []
    for name, command in commands:
        result = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        log = evidence / f'{name}.log'
        log.write_text(result.stdout)
        rows.append({'test': name, 'outcome': 'passed' if result.returncode == 0 else 'failed',
                     'os': platform.platform(), 'cpu': platform.machine(), 'distro': 'jazzy development build; protocol fixtures use no ROS',
                     'middleware_pair': ['build-only; no runtime protocol traffic', None], 'versions': versions.copy(),
                     'command': shlex.join(command), 'exit_code': result.returncode, 'evidence': [str(log.relative_to(ROOT))]})
        print(f'{name}: {rows[-1]["outcome"]}', flush=True)
    binary = ROOT / 'target/debug/studio-desktop'
    if binary.exists():
        for row in rows:
            row['versions']['desktop_sha256'] = hashlib.sha256(binary.read_bytes()).hexdigest()
    report.write_text(json.dumps(rows, indent=2) + '\n')
    for options in [[], ['--check']]:
        subprocess.run([sys.executable, str(ROOT / 'scripts/snapshot-evidence.py'), *options], cwd=ROOT, check=True)
    return int(any(row['outcome'] == 'failed' for row in rows))


if __name__ == '__main__':
    raise SystemExit(main())
