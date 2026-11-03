"""Assemble one Store-submission MSIX for ROS Studio; never signs or publishes.

Build the three frontends with --features studio-launcher/store-distribution,
studio-tools/store-distribution,studio-visualizer/store-distribution first.
Use the exact identity/publisher reserved in Partner Center. Store certification,
package installation, network behavior and clean-machine qualification are separate.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tomllib
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
ROLES = ('launcher', 'tools', 'visualizer', 'backend')
NS = 'http://schemas.microsoft.com/appx/manifest/foundation/windows10'
UAP = 'http://schemas.microsoft.com/appx/manifest/uap/windows10'
RES = 'http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities'
ET.register_namespace('', NS)
ET.register_namespace('uap', UAP)
ET.register_namespace('rescap', RES)


def assemble(args):
    if platform.system() != 'Windows' or platform.machine().lower() not in ('amd64', 'x86_64'):
        raise ValueError('Assemble on Windows x64 with the Windows SDK')
    if args.destination.exists():
        raise ValueError('Choose a new destination; preserve previous evidence')
    if not re.fullmatch(r'[A-Za-z0-9.-]{3,50}', args.identity_name):
        raise ValueError('Use the package identity name reserved in Partner Center')
    if not args.publisher.startswith('CN='):
        raise ValueError('Use the exact package publisher from Partner Center')
    if not args.notices.is_file() or not args.notices.stat().st_size:
        raise ValueError('Provide reviewed, nonempty dependency notices')
    if not (args.runtime / 'bin').is_dir() or not (args.runtime / 'share/ament_index').is_dir():
        raise ValueError('Provide the Jazzy Windows runtime prefix')
    for role in ROLES:
        if not (args.binaries / f'studio-{role}.exe').is_file():
            raise ValueError(f'Build studio-{role}.exe first')
    # Read PE resources without executing unsigned application code.
    # This is a build-mode sanity check, not a signature or trust assertion.
    for role in ROLES[:3]:
        env = dict(os.environ, STUDIO_MSIX_BINARY_METADATA_PATH=str((args.binaries / f'studio-{role}.exe').resolve()))
        query = "$ErrorActionPreference='Stop'; [Diagnostics.FileVersionInfo]::GetVersionInfo($env:STUDIO_MSIX_BINARY_METADATA_PATH) | Select-Object Comments,FileVersion | ConvertTo-Json -Compress"
        config = json.loads(subprocess.check_output(['powershell.exe', '-NoProfile', '-NonInteractive', '-Command', query], env=env, text=True))
        if config.get('Comments') != 'ROS Studio distribution: microsoft-store':
            raise ValueError('Rebuild all frontends with store-distribution before Store packaging')
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
    if not re.fullmatch(r'\d+\.\d+\.\d+', version):
        raise ValueError('MSIX requires a numeric suite version')
    layout = args.destination / 'layout'
    layout.mkdir(parents=True)
    for role in ROLES:
        shutil.copy2(args.binaries / f'studio-{role}.exe', layout / f'studio-{role}.exe')
    runtime = layout / 'runtime'
    (runtime / 'bin').mkdir(parents=True)
    dlls = list((args.runtime / 'bin').glob('*.dll'))
    if not dlls:
        raise ValueError('Runtime contains no DLLs')
    for dll in dlls:
        shutil.copy2(dll, runtime / 'bin' / dll.name)
    shutil.copytree(args.runtime / 'share', runtime / 'share')
    (runtime / 'manifest.json').write_text(json.dumps({'schema': 1, 'distro': 'jazzy', 'platform': 'windows', 'architecture': 'x86_64'}) + '\n')
    shutil.copy2(ROOT / 'LICENSE', layout / 'LICENSE')
    shutil.copy2(args.notices, layout / 'THIRD-PARTY-NOTICES.txt')
    shutil.copytree(ROOT / 'crates/ui-foundation/assets/fonts', layout / 'font-notices', ignore=shutil.ignore_patterns('*.ttf', '*.otf'))
    shutil.copytree(ROOT / 'crates/visualization/notices', layout / 'visualization-notices')
    shutil.copytree(ROOT / 'crates/backend/notices/ros-interfaces', layout / 'ros-interface-notices')
    images = layout / 'Assets'
    images.mkdir()
    for size in (44, 50, 150):
        shutil.copy2(ROOT / f'crates/ui-foundation/assets/app-icons/suite-{size}.png', images / f'suite-{size}.png')
    package = ET.Element(f'{{{NS}}}Package', IgnorableNamespaces='uap rescap')
    ET.SubElement(package, f'{{{NS}}}Identity', Name=args.identity_name, Publisher=args.publisher, Version=version + '.0', ProcessorArchitecture='x64')
    props = ET.SubElement(package, f'{{{NS}}}Properties')
    for name, text in [('DisplayName', 'ROS Studio'), ('PublisherDisplayName', args.publisher_display_name), ('Logo', 'Assets\\suite-50.png')]:
        ET.SubElement(props, f'{{{NS}}}{name}').text = text
    deps = ET.SubElement(package, f'{{{NS}}}Dependencies')
    ET.SubElement(deps, f'{{{NS}}}TargetDeviceFamily', Name='Windows.Desktop', MinVersion='10.0.22000.0', MaxVersionTested='10.0.26100.0')
    ET.SubElement(ET.SubElement(package, f'{{{NS}}}Resources'), f'{{{NS}}}Resource', Language='en-US')
    app = ET.SubElement(ET.SubElement(package, f'{{{NS}}}Applications'), f'{{{NS}}}Application', Id='ROSStudio', Executable='studio-launcher.exe', EntryPoint='Windows.FullTrustApplication')
    ET.SubElement(app, f'{{{UAP}}}VisualElements', DisplayName='ROS Studio', Description='Visualize and inspect ROS 2 systems', BackgroundColor='transparent', Square150x150Logo='Assets\\suite-150.png', Square44x44Logo='Assets\\suite-44.png')
    caps = ET.SubElement(package, f'{{{NS}}}Capabilities')
    for cap in ('internetClientServer', 'privateNetworkClientServer'):
        ET.SubElement(caps, f'{{{NS}}}Capability', Name=cap)
    ET.SubElement(caps, f'{{{RES}}}Capability', Name='runFullTrust')
    ET.ElementTree(package).write(layout / 'AppxManifest.xml', encoding='utf-8', xml_declaration=True)
    output = args.destination / f'ros-studio-{version}-windows-x64.msix'
    subprocess.run([str(args.makeappx), 'pack', '/d', str(layout), '/p', str(output)], check=True, capture_output=True)
    with output.open('rb') as source:
        digest = hashlib.file_digest(source, 'sha256').hexdigest()
    (args.destination / 'suite-package.json').write_text(json.dumps({'product': 'ROS Studio', 'version': version, 'artifact': str(output), 'sha256': digest, 'status': 'unsigned Store submission candidate; not installable by public users until certified/signed', 'applications': list(ROLES[:3]), 'firewall_helper': 'excluded; no elevation capability'}, indent=2) + '\n')
    print(args.destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('runtime', 'binaries', 'destination', 'notices', 'makeappx'):
        parser.add_argument('--' + name, required=True, type=Path)
    parser.add_argument('--identity-name', required=True)
    parser.add_argument('--publisher', required=True)
    parser.add_argument('--publisher-display-name', required=True)
    try:
        assemble(parser.parse_args())
    except (ValueError, OSError, subprocess.CalledProcessError) as e:
        parser.error(str(e))


if __name__ == '__main__':
    main()
