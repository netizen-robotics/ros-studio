"""Build one unsigned, per-user Windows preview installer with Inno Setup.

This is a complete offline suite, not the retired per-tool bootstrap. Signing,
Windows 11 graphical execution and clean-machine acceptance remain separate.
"""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]
ROLES = ('launcher', 'tools', 'visualizer', 'backend')


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def stage(runtime, binaries, layout):
    if not (runtime / 'bin').is_dir() or not (runtime / 'share/ament_index').is_dir():
        raise ValueError('Provide the Jazzy Windows Library prefix')
    for role in ROLES:
        if not (binaries / f'studio-{role}.exe').is_file():
            raise ValueError(f'Missing studio-{role}.exe')
    dlls = sorted((runtime / 'bin').glob('*.dll'))
    if not dlls:
        raise ValueError('Runtime DLLs are missing')
    layout.mkdir(parents=True)
    for role in ROLES:
        shutil.copy2(binaries / f'studio-{role}.exe', layout)
    (layout / 'runtime/bin').mkdir(parents=True)
    for dll in dlls:
        shutil.copy2(dll, layout / 'runtime/bin' / dll.name)
    shutil.copytree(runtime / 'share', layout / 'runtime/share')
    metadata = runtime.parent / 'conda-meta'
    if metadata.is_dir():
        shutil.copytree(metadata, layout / 'runtime/dependency-metadata')
    (layout / 'runtime/manifest.json').write_text(json.dumps({
        'schema': 1, 'distro': 'jazzy', 'platform': 'windows', 'architecture': 'x86_64'}) + '\n')
    for name in ('LICENSE', 'THIRD_PARTY_NOTICES.md'):
        shutil.copy2(ROOT / name, layout)
    for source, name in [('crates/ui-foundation/assets/fonts', 'fonts'),
                         ('crates/visualization/notices', 'visualization'),
                         ('crates/backend/notices/ros-interfaces', 'ros-interfaces')]:
        shutil.copytree(ROOT / source, layout / 'notices' / name,
                        ignore=shutil.ignore_patterns('*.ttf', '*.otf'))
    # Preserve cached upstream texts, without claiming redistribution review.
    for record_path in sorted(metadata.glob('*.json')):
        record = json.loads(record_path.read_text())
        source = Path(record.get('link', {}).get('source', '')) / 'info/licenses'
        if source.is_dir():
            name = record_path.stem
            shutil.copytree(source, layout / 'notices/runtime' / name)
    return {p.relative_to(layout).as_posix(): digest(p)
            for p in sorted(layout.rglob('*')) if p.is_file()}


def assemble(args):
    if platform.system() != 'Windows' or platform.machine().lower() not in ('amd64', 'x86_64'):
        raise ValueError('Assemble on Windows x64')
    if args.destination.exists():
        raise ValueError('Choose a new destination; preserve previous evidence')
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
    layout = args.destination / 'layout'
    files = stage(args.runtime.resolve(), args.binaries.resolve(), layout)
    (args.destination / 'payload.json').write_text(json.dumps(files, indent=2) + '\n')
    script = ROOT / 'scripts/windows-suite.iss'
    subprocess.run([str(args.iscc), f'/DSourceDir={layout.resolve()}',
                    f'/DOutputDir={args.destination.resolve()}', f'/DSuiteVersion={version}',
                    f'/DSuiteIcon={ROOT / "crates/ui-foundation/assets/app-icons/launcher.ico"}',
                    str(script)], check=True)
    output = args.destination / f'ros-studio-{version}-windows-x64-setup.exe'
    if not output.is_file():
        raise ValueError('Inno Setup did not produce the installer')
    record = {'product': 'ROS Studio', 'version': version, 'artifact': output.name,
              'sha256': digest(output), 'applications': list(ROLES[:3]),
              'status': 'unsigned preview; clean-machine, native UI, ROS networking and complete license review pending',
              'installer': 'Inno Setup; per-user; no elevation; offline full suite',
              'firewall_helper': 'excluded', 'payload_files': len(files),
              'source_commit': subprocess.check_output(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], text=True).strip()}
    (args.destination / 'suite-package.json').write_text(json.dumps(record, indent=2) + '\n')
    (args.destination / 'SHA256SUMS').write_text(f'{record["sha256"]}  {output.name}\n')
    print(output)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('runtime', 'binaries', 'destination', 'iscc'):
        parser.add_argument('--' + name, required=True, type=Path)
    try:
        assemble(parser.parse_args())
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.error(str(error))
