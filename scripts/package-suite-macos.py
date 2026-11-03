"""Assemble one ROS Studio.app with three frontends and one shared Jazzy runtime.

Development previews use ad-hoc signing. Production requires Developer ID,
notarization and reviewed notices. This tool never publishes or updates installs.
"""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]
ROLES = ('launcher', 'tools', 'visualizer', 'backend')


def run(*command):
    subprocess.run([str(c) for c in command], check=True, capture_output=True)


def assemble(args):
    if platform.system() != 'Darwin' or platform.machine() != 'arm64':
        raise ValueError('Assemble on macOS Apple Silicon')
    if args.destination.exists():
        raise ValueError('Choose a new destination; preserve previous evidence')
    if args.notary_profile and not args.sign_identity:
        raise ValueError('Notarization requires Developer ID signing')
    if args.sign_identity and not args.notices:
        raise ValueError('Release signing requires reviewed dependency notices')
    if args.notices and (not args.notices.is_file() or not args.notices.stat().st_size):
        raise ValueError('Notices must be a nonempty file')
    manifest = json.loads((args.runtime / 'manifest.json').read_text())
    if manifest.get('distro') != 'jazzy' or not (args.runtime / 'share/ament_index').is_dir():
        raise ValueError('Provide a relocated Jazzy runtime')
    for role in ROLES:
        if not (args.binaries / f'studio-{role}').is_file():
            raise ValueError(f'Build studio-{role} first')
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
    bundle = args.destination / 'ROS Studio.app'
    macos, resources, frameworks = (bundle / 'Contents' / n for n in ('MacOS', 'Resources', 'Frameworks'))
    macos.mkdir(parents=True)
    resources.mkdir()
    shutil.copytree(args.runtime / 'lib', frameworks, symlinks=True)
    shutil.copytree(args.runtime / 'share', resources / 'share', symlinks=True)
    (resources / 'lib').symlink_to('../Frameworks', target_is_directory=True)
    for name in ('dependency-metadata', 'notices'):
        if (args.runtime / name).exists():
            shutil.copytree(args.runtime / name, resources / name, symlinks=True)
    for role in ('launcher', 'backend'):
        shutil.copy2(args.binaries / f'studio-{role}', macos / f'studio-{role}')
    nested = []
    for role in ('tools', 'visualizer'):
        app = bundle / 'Contents/Applications' / f'ROS Studio {role.title()}.app'
        child = app / 'Contents'
        (child / 'MacOS').mkdir(parents=True)
        (child / 'Resources').mkdir()
        for binary in (role, 'backend'):
            shutil.copy2(args.binaries / f'studio-{binary}', child / 'MacOS' / f'studio-{binary}')
        (child / 'Frameworks').symlink_to('../../../Frameworks', target_is_directory=True)
        for name in ('share', 'lib'):
            (child / 'Resources' / name).symlink_to('../../../../Resources/' + name, target_is_directory=True)
        shutil.copy2(args.runtime / 'manifest.json', child / 'Resources/manifest.json')
        shutil.copy2(ROOT / f'crates/ui-foundation/assets/app-icons/{role}.icns', child / 'Resources/AppIcon.icns')
        with (child / 'Info.plist').open('wb') as output:
            plistlib.dump({'CFBundleExecutable': f'studio-{role}', 'CFBundleIdentifier': f'dev.rosstudio.{role}',
                          'CFBundleName': f'ROS Studio {role.title()}', 'CFBundlePackageType': 'APPL',
                          'CFBundleShortVersionString': version, 'CFBundleVersion': version,
                          'CFBundleIconFile': 'AppIcon.icns', 'LSMinimumSystemVersion': '14.0',
                          'NSHighResolutionCapable': True,
                          'NSLocalNetworkUsageDescription': 'ROS Studio receives the ROS 2 data you select on your local network.'}, output)
        nested.append(app)
    shutil.copy2(ROOT / 'LICENSE', resources / 'LICENSE')
    shutil.copytree(ROOT / 'crates/ui-foundation/assets/app-icons', resources / 'app-icons')
    shutil.copy2(ROOT / 'crates/ui-foundation/assets/app-icons/launcher.icns', resources / 'AppIcon.icns')
    shutil.copytree(ROOT / 'crates/ui-foundation/assets/fonts', resources / 'font-notices', ignore=shutil.ignore_patterns('*.ttf', '*.otf'))
    shutil.copytree(ROOT / 'crates/visualization/notices', resources / 'visualization-notices')
    if args.notices:
        shutil.copy2(args.notices, resources / 'THIRD-PARTY-NOTICES.txt')
    (resources / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    with (bundle / 'Contents/Info.plist').open('wb') as output:
        plistlib.dump({'CFBundleExecutable': 'studio-launcher', 'CFBundleIdentifier': 'dev.rosstudio.launcher',
                      'CFBundleName': 'ROS Studio', 'CFBundleDisplayName': 'ROS Studio', 'CFBundlePackageType': 'APPL',
                      'CFBundleIconFile': 'AppIcon.icns', 'CFBundleShortVersionString': version, 'CFBundleVersion': version,
                      'LSMinimumSystemVersion': '14.0', 'NSHighResolutionCapable': True,
                      'NSLocalNetworkUsageDescription': 'ROS Studio discovers ROS 2 systems and receives the data you select on your local network.'}, output)
    binaries = list(macos.iterdir()) + [p for app in nested for p in (app / 'Contents/MacOS').iterdir()] + [p for p in frameworks.rglob('*') if p.is_file() and not p.is_symlink() and p.suffix in ('.dylib', '.so')]
    for binary in binaries:
        loads = subprocess.check_output(['otool', '-l', str(binary)], text=True).splitlines()
        rpaths = [loads[i + 2].strip().split(' ')[1] for i, line in enumerate(loads) if line.strip() == 'cmd LC_RPATH']
        for path in rpaths:
            if path.startswith('/') or path == '@executable_path/../lib':
                run('install_name_tool', '-delete_rpath', path, binary)
        wanted = '@loader_path' if binary.is_relative_to(frameworks) else '@executable_path/../Frameworks'
        if wanted not in rpaths:
            run('install_name_tool', '-add_rpath', wanted, binary)
        signing = ['codesign', '--force', '--sign', args.sign_identity or '-']
        if args.sign_identity:
            signing += ['--options', 'runtime', '--timestamp']
        run(*signing, binary)
    for app in nested:
        signing = ['codesign', '--force', '--sign', args.sign_identity or '-']
        if args.sign_identity:
            signing += ['--options', 'runtime', '--timestamp']
        run(*signing, app)
    signing = ['codesign', '--force', '--sign', args.sign_identity or '-']
    if args.sign_identity:
        signing += ['--options', 'runtime', '--timestamp']
    run(*signing, bundle)
    run('codesign', '--verify', '--deep', '--strict', bundle)
    if args.notary_profile:
        submitted = args.destination / 'ros-studio-notary.zip'
        run('ditto', '-c', '-k', '--keepParent', bundle, submitted)
        run('xcrun', 'notarytool', 'submit', submitted, '--keychain-profile', args.notary_profile, '--wait')
        run('xcrun', 'stapler', 'staple', bundle)
        run('spctl', '--assess', '--type', 'execute', bundle)
    dmg = args.destination / f'ros-studio-{version}-macos-arm64.dmg'
    staging = args.destination / 'dmg-content'
    staging.mkdir()
    # A symlink to the bundle would not give users an installable app on the image.
    run('ditto', bundle, staging / bundle.name)
    (staging / 'Applications').symlink_to('/Applications', target_is_directory=True)
    run('hdiutil', 'create', '-volname', 'ROS Studio', '-srcfolder', staging, '-ov', '-format', 'UDZO', dmg)
    if args.notary_profile:
        run('codesign', '--force', '--sign', args.sign_identity, '--timestamp', dmg)
        run('xcrun', 'notarytool', 'submit', dmg, '--keychain-profile', args.notary_profile, '--wait')
        run('xcrun', 'stapler', 'staple', dmg)
    record = {'product': 'ROS Studio', 'version': version, 'applications': list(ROLES[:3]),
              'status': 'signed/notarized; clean-machine qualification pending' if args.notary_profile else 'experimental/ad-hoc; not qualified for public distribution',
              'artifact': str(dmg), 'sha256': hashlib.file_digest(dmg.open('rb'), 'sha256').hexdigest(),
              'binaries': {role: hashlib.sha256((macos / f'studio-{role}' if role in ('launcher', 'backend') else bundle / f'Contents/Applications/ROS Studio {role.title()}.app/Contents/MacOS/studio-{role}').read_bytes()).hexdigest() for role in ROLES}}
    (args.destination / 'suite-package.json').write_text(json.dumps(record, indent=2) + '\n')
    print(args.destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime', required=True, type=Path)
    parser.add_argument('--binaries', default=ROOT / 'target/release', type=Path)
    parser.add_argument('--destination', required=True, type=Path)
    parser.add_argument('--notices', type=Path)
    parser.add_argument('--sign-identity')
    parser.add_argument('--notary-profile')
    try:
        assemble(parser.parse_args())
    except (ValueError, OSError, subprocess.CalledProcessError) as e:
        parser.error(str(e))


if __name__ == '__main__':
    main()
