"""Build an experimental Ubuntu 24.04 amd64 DEB; never publishes or installs."""
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

ROOT = Path(__file__).resolve().parents[1]
ROLES = ('launcher', 'tools', 'visualizer', 'backend')
SYSTEM = re.compile(r'^(ld-linux.*|lib(c|m|pthread|dl|rt|resolv|util)\.so(?:\..*)?)$')


def run(*args):
    env = {k: v for k, v in os.environ.items() if k not in ('LD_LIBRARY_PATH', 'LD_PRELOAD')}
    try:
        return subprocess.check_output([str(a) for a in args], text=True, stderr=subprocess.STDOUT, env=env)
    except subprocess.CalledProcessError as error:
        print(error.output)
        raise


def elf(path):
    with path.open('rb') as stream:
        return stream.read(4) == b'\x7fELF'


def dependencies(path):
    return run('patchelf', '--print-needed', path).splitlines()


def copy_runtime(prefix, app):
    """Include runtime-loaded ROS plugins, then close their ELF dependency graph."""
    prefix = prefix.resolve()
    lib = app / 'runtime/lib'
    lib.mkdir(parents=True)
    pending = list(app.glob('studio-*'))
    roots = [p for p in (prefix / 'lib').glob('*.so*')
             if '__rosidl_' in p.name or p.name.startswith(('librmw_', 'librcl_logging_'))]
    if not roots:
        raise ValueError('ROS runtime plugin libraries are missing')
    copied = set()

    def add(source, name):
        if '/' in name or name in ('.', '..'):
            raise ValueError('Invalid library name')
        if name in copied or SYSTEM.fullmatch(name):
            return
        resolved = source.resolve()
        if not resolved.is_relative_to(prefix) or not elf(resolved):
            raise ValueError(f'Invalid runtime library: {name}')
        target = lib / name
        shutil.copy2(resolved, target)
        target.chmod(0o755)
        copied.add(name)
        pending.append(target)

    for source in roots:
        add(source, source.name)
    while pending:
        binary = pending.pop()
        for needed in dependencies(binary):
            name = Path(needed).name
            source = prefix / 'lib' / name
            if source.exists() and not SYSTEM.fullmatch(name):
                add(source, name)
            if '/' in needed:
                if not source.exists():
                    raise ValueError(f'Unrelocatable dependency: {needed}')
                run('patchelf', '--replace-needed', needed, name, binary)
        run('patchelf', '--set-rpath', '$ORIGIN' if binary.parent == lib else '$ORIGIN/runtime/lib', binary)
    for binary in [*(app / f'studio-{r}' for r in ROLES), *lib.iterdir()]:
        output = run('ldd', binary)
        if 'not found' in output or str(prefix) in output:
            raise ValueError(f'Unresolved or build-prefix dependency in {binary.name}: {output}')
    return sorted(copied)


def assemble(args):
    if platform.system() != 'Linux' or platform.machine() != 'x86_64':
        raise ValueError('Build on Ubuntu 24.04 amd64')
    os_release = Path('/etc/os-release').read_text()
    if 'ID=ubuntu' not in os_release or 'VERSION_ID="24.04"' not in os_release:
        raise ValueError('This candidate targets Ubuntu 24.04 only')
    if args.destination.exists():
        raise ValueError('Choose a new destination; preserve prior runs')
    prefix = args.runtime.resolve()
    if not (prefix / 'share/ament_index').is_dir():
        raise ValueError('Missing Jazzy ament index')
    for role in ROLES:
        if not (args.binaries / f'studio-{role}').is_file() or not elf(args.binaries / f'studio-{role}'):
            raise ValueError(f'Build Linux studio-{role} first')
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
    if not re.fullmatch(r'\d+\.\d+\.\d+', version):
        raise ValueError('Expected numeric suite version')
    version += '~preview1'
    layout = args.destination / 'layout'
    app = layout / 'opt/ros-studio'
    app.mkdir(parents=True)
    for role in ROLES:
        shutil.copy2(args.binaries / f'studio-{role}', app / f'studio-{role}')
        (app / f'studio-{role}').chmod(0o755)
    libraries = copy_runtime(prefix, app)
    share = app / 'runtime/share'
    shutil.copytree(prefix / 'share/ament_index', share / 'ament_index', symlinks=False)
    for package in (prefix / 'share').iterdir():
        if package.is_dir() and (package / 'package.xml').is_file():
            target = share / package.name
            target.mkdir(exist_ok=True)
            shutil.copy2(package / 'package.xml', target / 'package.xml')
            for kind in ('msg', 'srv', 'action'):
                if (package / kind).is_dir():
                    shutil.copytree(package / kind, target / kind, symlinks=False)
    (app / 'runtime/manifest.json').write_text(json.dumps({'schema': 1, 'distro': 'jazzy', 'platform': 'linux', 'architecture': 'x86_64'}) + '\n')
    notices = app / 'notices'
    notices.mkdir()
    shutil.copy2(ROOT / 'LICENSE', notices / 'LICENSE')
    shutil.copy2(ROOT / 'THIRD_PARTY_NOTICES.md', notices / 'THIRD_PARTY_NOTICES.md')
    for source, name in [('crates/ui-foundation/assets/fonts', 'fonts'), ('crates/visualization/notices', 'visualization'), ('crates/backend/notices/ros-interfaces', 'ros-interfaces')]:
        shutil.copytree(ROOT / source, notices / name, ignore=shutil.ignore_patterns('*.ttf', '*.otf'))
    # Preserve dependency metadata as inventory, not as a claim of license review.
    metadata = app / 'dependency-metadata'
    shutil.copytree(prefix / 'conda-meta', metadata / 'conda')
    shutil.copy2(ROOT / 'Cargo.lock', metadata / 'Cargo.lock')
    (notices / 'PREVIEW.txt').write_text('Experimental Ubuntu candidate. Complete dependency license review, native UI and ROS network acceptance remain pending. Not a qualified public release.\n')
    usrbin = layout / 'usr/bin'
    usrbin.mkdir(parents=True)
    for command, role in [('ros-studio', 'launcher'), ('ros-studio-tools', 'tools'), ('ros-studio-visualizer', 'visualizer')]:
        (usrbin / command).symlink_to(f'/opt/ros-studio/studio-{role}')
    desktop = layout / 'usr/share/applications'
    desktop.mkdir(parents=True)
    (desktop / 'ros-studio.desktop').write_text('[Desktop Entry]\nType=Application\nName=ROS Studio\nComment=Visualize and inspect ROS 2 systems (preview)\nExec=/usr/bin/ros-studio\nIcon=ros-studio\nTerminal=false\nCategories=Development;Science;\n')
    icon = layout / 'usr/share/icons/hicolor/150x150/apps'
    icon.mkdir(parents=True)
    shutil.copy2(ROOT / 'crates/ui-foundation/assets/app-icons/suite-150.png', icon / 'ros-studio.png')
    doc = layout / 'usr/share/doc/ros-studio'
    doc.mkdir(parents=True)
    shutil.copy2(ROOT / 'LICENSE', doc / 'copyright')
    shutil.copy2(notices / 'PREVIEW.txt', doc / 'README')
    control = layout / 'DEBIAN'
    control.mkdir()
    depends = 'libc6 (>= 2.39), libgcc-s1, libstdc++6, libx11-6, libxcb1, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libwayland-cursor0, libwayland-egl1, libxcursor1, libxi6, libxrandr2, libgl1, libegl1, libvulkan1, libfontconfig1, libfreetype6, libdbus-1-3, libudev1, libasound2t64, zlib1g'
    size = sum(p.stat().st_size for p in app.rglob('*') if p.is_file()) // 1024 + 1
    (control / 'control').write_text(f'Package: ros-studio\nVersion: {version}\nArchitecture: amd64\nMaintainer: Netizen Robotics\nSection: science\nPriority: optional\nInstalled-Size: {size}\nDepends: {depends}\nHomepage: https://github.com/netizen-robotics/ros-studio\nDescription: Native ROS 2 visualization and inspection suite (experimental preview)\n Includes Launcher, Visualizer, Tools and a private Jazzy runtime.\n Ubuntu 24.04 amd64 candidate; desktop and network qualification pending.\n')
    md5 = []
    for path in sorted(layout.rglob('*')):
        if path.is_file() and not path.is_symlink() and not path.is_relative_to(control):
            md5.append(f'{hashlib.md5(path.read_bytes()).hexdigest()}  {path.relative_to(layout)}')
    (control / 'md5sums').write_text('\n'.join(md5) + '\n')
    output = args.destination / f'ros-studio_{version}_amd64.deb'
    run('dpkg-deb', '--root-owner-group', '--build', layout, output)
    record = {'product': 'ROS Studio', 'version': version, 'architecture': 'amd64', 'os': platform.platform(),
              'artifact': output.name, 'sha256': hashlib.sha256(output.read_bytes()).hexdigest(), 'libraries': libraries,
              'status': 'experimental preview; native UI, ROS networking and complete license review pending',
              'source_commit': run('git', '-C', ROOT, 'rev-parse', 'HEAD').strip(),
              'pixi_lock_sha256': hashlib.sha256((ROOT / 'pixi.lock').read_bytes()).hexdigest(),
              'command': ['package-suite-deb.py', '--runtime', str(args.runtime), '--binaries', str(args.binaries), '--destination', str(args.destination)]}
    (args.destination / 'suite-package.json').write_text(json.dumps(record, indent=2) + '\n')
    print(output)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime', type=Path, default=ROOT / '.pixi/envs/default')
    parser.add_argument('--binaries', type=Path, default=ROOT / 'target/release')
    parser.add_argument('--destination', type=Path, required=True)
    assemble(parser.parse_args())
