"""Build independent app bundles from an existing relocated Jazzy runtime.

Run with scripts/dev .pixi/envs/default/bin/python. Default output is explicitly
experimental/ad-hoc. Supplying release identity/notary credentials performs real
signing/notarization and requires assembled dependency notices. Never publishes.
"""
import argparse
import hashlib
import json
import platform
import plistlib
import shutil
import subprocess
import tarfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
NAMES = {'visualizer': 'ROS Studio Visualizer', 'tools': 'ROS Studio Tools',
         'launcher': 'ROS Studio'}


def run(*command):
    subprocess.run([str(value) for value in command], check=True, capture_output=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime', type=Path, help='Existing relocated Jazzy runtime, needed for visualizer/tools')
    parser.add_argument('--destination', required=True, type=Path)
    parser.add_argument('--profile', choices=['debug', 'release'], default='release')
    parser.add_argument('--applications', nargs='+', choices=NAMES, default=list(NAMES))
    parser.add_argument('--sign-identity')
    parser.add_argument('--notary-profile')
    parser.add_argument('--dependency-notices', type=Path)
    parser.add_argument('--minimum-macos', default='14.0')
    parser.add_argument('--release-base-url', help='https://github.com/OWNER/REPO/releases/download/TAG')
    args = parser.parse_args()
    if platform.system() != 'Darwin' or platform.machine() != 'arm64':
        parser.error('Initial managed packages require macOS ARM64')
    if args.destination.exists():
        parser.error('Destination exists; choose a new path to preserve prior artifacts')
    if args.notary_profile and not args.sign_identity:
        parser.error('Notarization requires a Developer ID signing identity')
    if args.sign_identity and not args.dependency_notices:
        parser.error('Release signing requires assembled dependency notices')
    if args.release_base_url and not (args.sign_identity and args.notary_profile):
        parser.error('Release catalog entries require signed/notarized packages')
    if args.release_base_url:
        from urllib.parse import urlparse
        url = urlparse(args.release_base_url)
        parts = url.path.strip('/').split('/')
        if url.scheme != 'https' or url.netloc != 'github.com' or len(parts) != 5 or parts[2:4] != ['releases', 'download'] or url.query or url.fragment:
            parser.error('Use a GitHub release download base URL')
    needs_runtime = any(app != 'launcher' for app in args.applications)
    runtime_manifest = None
    if needs_runtime:
        if not args.runtime:
            parser.error('--runtime is required for visualization/tools packages')
        runtime_manifest = json.loads((args.runtime / 'manifest.json').read_text())
        if runtime_manifest.get('distro') != 'jazzy':
            parser.error('Managed initial packages require a Jazzy runtime')
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
    for app in args.applications:
        if not (ROOT / 'target' / args.profile / f'studio-{app}').is_file():
            parser.error(f'Build studio-{app} with the requested profile first')
    args.destination.mkdir(parents=True)
    releases = []
    records = []
    for app in args.applications:
        bundle = args.destination / (NAMES[app] + '.app')
        contents = bundle / 'Contents'
        macos = contents / 'MacOS'
        resources = contents / 'Resources'
        macos.mkdir(parents=True)
        resources.mkdir()
        shutil.copy2(ROOT / 'crates/ui-foundation/assets/app-icons' / f'{app}.icns', resources / 'AppIcon.icns')
        shutil.copy2(ROOT / 'LICENSE', resources / 'LICENSE')
        frameworks = contents / 'Frameworks'
        if app != 'launcher':
            shutil.copytree(args.runtime / 'lib', frameworks)
            shutil.copytree(args.runtime / 'share', resources / 'share')
            (resources / 'lib').symlink_to('../Frameworks', target_is_directory=True)
            for name in ['dependency-metadata', 'notices']:
                if (args.runtime / name).exists():
                    shutil.copytree(args.runtime / name, resources / name)
            shutil.copy2(ROOT / 'target' / args.profile / 'studio-backend', macos / 'studio-backend')
            (resources / 'manifest.json').write_text(json.dumps(runtime_manifest, indent=2) + '\n')
        shutil.copy2(ROOT / 'target' / args.profile / f'studio-{app}', macos / f'studio-{app}')
        shutil.copytree(ROOT / 'crates/ui-foundation/assets/fonts', resources / 'font-notices',
                        ignore=shutil.ignore_patterns('*.otf', '*.ttf'))
        if app == 'visualizer':
            shutil.copytree(ROOT / 'crates/visualization/notices', resources / 'visualization-notices')
        if args.dependency_notices:
            shutil.copy2(args.dependency_notices, resources / 'THIRD-PARTY-NOTICES.txt')
        with (contents / 'Info.plist').open('wb') as output:
            plistlib.dump({'CFBundleExecutable': f'studio-{app}',
                          'CFBundleIdentifier': f'dev.rosstudio.{app}',
                          'CFBundleName': NAMES[app], 'CFBundleDisplayName': NAMES[app],
                          'CFBundlePackageType': 'APPL', 'CFBundleIconFile': 'AppIcon.icns',
                          'CFBundleVersion': version,
                          'CFBundleShortVersionString': version,
                          'LSMinimumSystemVersion': args.minimum_macos,
                          'NSHighResolutionCapable': True,
                          'NSLocalNetworkUsageDescription': 'ROS Studio discovers ROS 2 robots and receives the topics you select on your local network.'}, output)
        binaries = list(macos.iterdir()) + (list(frameworks.glob('*.dylib')) if frameworks.exists() else [])
        for binary in binaries:
            loads = subprocess.check_output(['otool', '-l', str(binary)], text=True).splitlines()
            rpaths = [loads[i + 2].strip().split(' ')[1] for i, line in enumerate(loads)
                      if line.strip() == 'cmd LC_RPATH']
            for value in rpaths:
                if value.startswith('/') or value == '@executable_path/../lib':
                    run('install_name_tool', '-delete_rpath', value, binary)
            wanted = '@loader_path' if binary.parent == frameworks else '@executable_path/../Frameworks'
            if wanted not in rpaths:
                run('install_name_tool', '-add_rpath', wanted, binary)
            signing = ['codesign', '--force', '--sign', args.sign_identity or '-']
            if args.sign_identity:
                signing += ['--options', 'runtime', '--timestamp']
            run(*signing, binary)
        signing = ['codesign', '--force', '--sign', args.sign_identity or '-']
        if args.sign_identity:
            signing += ['--options', 'runtime', '--timestamp']
        run(*signing, bundle)
        run('codesign', '--verify', '--deep', '--strict', bundle)
        if args.notary_profile:
            submitted = args.destination / f'{app}-notary.zip'
            run('ditto', '-c', '-k', '--keepParent', bundle, submitted)
            run('xcrun', 'notarytool', 'submit', submitted, '--keychain-profile', args.notary_profile, '--wait')
            run('xcrun', 'stapler', 'staple', bundle)
            run('spctl', '--assess', '--type', 'execute', bundle)
        archive = args.destination / f'studio-{app}-{version}-macos-aarch64.tar.gz'
        with tarfile.open(archive, 'w:gz', dereference=False) as output:
            output.add(bundle, arcname=bundle.name)
        records.append({'application': app, 'version': version,
                        'status': 'signed/notarized; clean-machine qualification pending' if args.notary_profile else 'experimental; not eligible for managed release installation',
                        'archive': str(archive), 'sha256': hashlib.sha256(archive.read_bytes()).hexdigest(),
                        'binary_sha256': hashlib.sha256((macos / f'studio-{app}').read_bytes()).hexdigest(),
                        'license_review': 'provided notices require applicability review' if args.dependency_notices else 'pending'})
        if app != 'launcher' and args.release_base_url:
            releases.append({'application': app, 'version': version, 'platform': 'macos',
                             'architecture': 'aarch64', 'distro': 'jazzy',
                             'minimum_macos': args.minimum_macos, 'minimum_launcher': version,
                             'archive_url': args.release_base_url.rstrip('/') + '/' + archive.name,
                             'archive_bytes': archive.stat().st_size,
                             'sha256': records[-1]['sha256'], 'bundle_id': f'dev.rosstudio.{app}'})
    (args.destination / 'packages.json').write_text(json.dumps(records, indent=2) + '\n')
    if releases:
        (args.destination / 'catalog-unsigned.json').write_text(json.dumps({'schema': 1, 'releases': releases}, indent=2) + '\n')
    print(args.destination)


if __name__ == '__main__':
    main()
