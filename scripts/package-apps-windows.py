"""Assemble per-user Windows x64/Jazzy packages. Development tooling only.

Never publishes. Experimental archives are not accepted by managed installation.
Release mode requires pre-signed binaries, SDK verification, configured release
keys, assembled dependency notices and an explicit GitHub release asset base.
"""
import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import tarfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def assemble(args):
    if platform.system() != 'Windows' or platform.machine().lower() not in ('amd64', 'x86_64'):
        raise ValueError('Build and assemble on Windows x64')
    if args.destination.exists():
        raise ValueError('Choose a new destination; previous evidence is preserved')
    if not args.notices.is_file() or args.notices.stat().st_size == 0:
        raise ValueError('Provide assembled runtime and application dependency notices')
    if not (args.runtime / 'bin').is_dir() or not (args.runtime / 'share/ament_index').is_dir():
        raise ValueError('Provide the Jazzy Windows runtime prefix (Conda Library)')
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
    names = ['studio-bootstrap.exe', 'studio-launcher.exe', 'studio-backend.exe',
             'studio-network-helper.exe', 'studio-tools.exe', 'studio-visualizer.exe']
    for name in names:
        if not (args.binaries / name).is_file():
            raise ValueError(f'Build {name} first')
    if args.release_base_url:
        from urllib.parse import urlparse
        url = urlparse(args.release_base_url)
        parts = url.path.strip('/').split('/')
        if url.scheme != 'https' or url.netloc != 'github.com' or len(parts) != 5 or parts[2:4] != ['releases', 'download'] or url.query or url.fragment:
            raise ValueError('Use a GitHub HTTPS release download base URL')
        if not args.signtool:
            raise ValueError('Release packaging requires SDK signtool verification')
        config = json.loads(subprocess.check_output([str(args.binaries / 'studio-bootstrap.exe'), '--release-configuration'], text=True))
        if not config.get('publisher_sha256') or not config.get('catalog_key'):
            raise ValueError('Build all executables with the pinned publisher and release catalog key')
        for name in names:
            subprocess.run([str(args.signtool), 'verify', '/pa', '/all', str(args.binaries / name)], check=True, capture_output=True)
    args.destination.mkdir(parents=True)
    bootstrap = args.destination / 'bootstrap'
    bootstrap.mkdir()
    for name in names[:2]:
        shutil.copy2(args.binaries / name, bootstrap / name)
    shutil.copy2(args.notices, bootstrap / 'THIRD-PARTY-NOTICES.txt')
    shutil.copy2(ROOT / 'LICENSE', bootstrap / 'LICENSE')
    shutil.copytree(ROOT / 'crates/ui-foundation/assets/fonts', bootstrap / 'font-notices', ignore=shutil.ignore_patterns('*.otf', '*.ttf'))
    # Extracted bootstrap folder is the per-user installer distribution.
    shutil.make_archive(str(args.destination / f'ros-studio-setup-{version}-windows-x86_64'), 'zip', bootstrap)
    releases = []
    records = []
    for app in ('tools', 'visualizer'):
        package = args.destination / app
        package.mkdir()
        for name in (f'studio-{app}.exe', 'studio-backend.exe', 'studio-network-helper.exe'):
            shutil.copy2(args.binaries / name, package / name)
        runtime = package / 'runtime'
        (runtime / 'bin').mkdir(parents=True)
        dlls = list((args.runtime / 'bin').glob('*.dll'))
        if not dlls:
            raise ValueError('Runtime contains no DLLs')
        for source in dlls:
            shutil.copy2(source, runtime / 'bin' / source.name)
        shutil.copytree(args.runtime / 'share', runtime / 'share', symlinks=False)
        metadata = args.runtime.parent / 'conda-meta'
        if metadata.is_dir():
            shutil.copytree(metadata, runtime / 'dependency-metadata')
        (runtime / 'manifest.json').write_text(json.dumps({'schema': 1, 'distro': 'jazzy', 'platform': 'windows', 'architecture': 'x86_64'}) + '\n')
        shutil.copy2(args.notices, package / 'THIRD-PARTY-NOTICES.txt')
        shutil.copy2(ROOT / 'LICENSE', package / 'LICENSE')
        shutil.copytree(bootstrap / 'font-notices', package / 'font-notices')
        if app == 'visualizer':
            shutil.copytree(ROOT / 'crates/visualization/notices', package / 'visualization-notices')
        files = {p.relative_to(package).as_posix(): digest(p) for p in sorted(package.rglob('*')) if p.is_file()}
        (package / 'manifest.json').write_text(json.dumps({'schema': 1, 'application': app, 'version': version, 'distro': 'jazzy', 'files': files}, sort_keys=True) + '\n')
        archive = args.destination / f'studio-{app}-{version}-windows-x86_64.tar.gz'
        with tarfile.open(archive, 'w:gz', dereference=True) as output:
            output.add(package, arcname=app)
        if archive.stat().st_size > 2 * 1024**3:
            raise ValueError('Archive exceeds managed download limit')
        record = {'application': app, 'archive': str(archive), 'sha256': digest(archive),
                  'status': 'signed; qualification pending' if args.release_base_url else 'experimental; not eligible for managed release installation'}
        records.append(record)
        if args.release_base_url:
            releases.append({'application': app, 'version': version, 'platform': 'windows', 'architecture': 'x86_64', 'distro': 'jazzy',
                             'minimum_launcher': version, 'bundle_id': f'dev.rosstudio.{app}',
                             'archive_url': args.release_base_url.rstrip('/') + '/' + archive.name,
                             'archive_bytes': archive.stat().st_size, 'sha256': record['sha256'],
                             'windows': {'minimum_build': 22000, 'manifest_sha256': digest(package / 'manifest.json'),
                                         'application_sha256': files[f'studio-{app}.exe'], 'backend_sha256': files['studio-backend.exe'],
                                         'helper_sha256': files['studio-network-helper.exe']}})
    (args.destination / 'packages.json').write_text(json.dumps(records, indent=2) + '\n')
    if releases:
        (args.destination / 'catalog-unsigned.json').write_text(json.dumps({'schema': 2, 'releases': releases}, indent=2) + '\n')
    print(args.destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime', required=True, type=Path)
    parser.add_argument('--binaries', default=ROOT / 'target/release', type=Path)
    parser.add_argument('--destination', required=True, type=Path)
    parser.add_argument('--notices', required=True, type=Path)
    parser.add_argument('--signtool', type=Path)
    parser.add_argument('--release-base-url')
    args = parser.parse_args()
    try:
        assemble(args)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.error(str(error))


if __name__ == '__main__':
    main()
