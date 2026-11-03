"""Build fixture artifacts or export a dedicated native interface install prefix."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
RUNTIME = ROOT / '.pixi/envs/default'


def build(package, destination, distro='jazzy', source_install=None):
    if not re.fullmatch(r'[a-z][a-z0-9_]*', package):
        raise SystemExit('Invalid ROS package name')
    if source_install is None and package not in ['feasibility_interfaces', 'service_extra_interfaces']:
        raise SystemExit('Non-fixture packages require --source-install')
    runtime = RUNTIME if distro == 'jazzy' else ROOT / 'environments' / distro / '.pixi/envs/default'
    if Path(os.environ.get('CONDA_PREFIX', '')).resolve() != runtime.resolve():
        raise SystemExit(f'Run inside the isolated {distro} Pixi environment: {runtime}')
    if os.environ.get('ROS_DISTRO') != distro:
        raise SystemExit(f'ROS_DISTRO must be {distro}')
    for variable in ('AMENT_PREFIX_PATH', 'CMAKE_PREFIX_PATH'):
        prefixes = os.environ.get(variable, '').split(os.pathsep)
        if not prefixes or any(Path(p).resolve() != runtime.resolve() for p in prefixes):
            raise SystemExit(f'{variable} must contain only {runtime}')
    suffix = '' if distro == 'jazzy' else '-' + distro
    build_dir = ROOT / 'artifacts' / (package + '-service-build' + suffix)
    install = ROOT / 'artifacts' / (package + '-service-install' + suffix)
    if destination.exists():
        raise SystemExit(f'Refusing to overwrite evidence bundle: {destination}')
    if platform.system() != 'Darwin':
        raise SystemExit('Fixture bundle authoring currently qualified for macOS only; other platforms not tested')
    if source_install is None:
        subprocess.run(['cmake', '-S', str(ROOT / 'fixtures' / package), '-B', str(build_dir), '-G', 'Ninja',
                        f'-DCMAKE_INSTALL_PREFIX={install}', f'-DPython3_EXECUTABLE={runtime / "bin/python"}',
                        f'-DCMAKE_SHARED_LINKER_FLAGS=-L"{runtime / "lib"}"',
                        f'-DCMAKE_MODULE_LINKER_FLAGS=-L"{runtime / "lib"}"',
                        f'-DCMAKE_INSTALL_RPATH=@loader_path;{runtime / "lib"}'], check=True)
        subprocess.run(['cmake', '--build', str(build_dir)], check=True)
        subprocess.run(['cmake', '--install', str(build_dir)], check=True)
    else:
        install = source_install.resolve()
        index = install / 'share/ament_index/resource_index/packages'
        if not index.is_dir() or {p.name for p in index.iterdir()} != {package}:
            raise SystemExit('Source install must be a dedicated single-package prefix matching --package')
        if not list((install / 'lib').glob(f'lib{package}__*.dylib')):
            raise SystemExit('Source install has no native interface libraries for the selected package')
    if destination.exists():
        raise SystemExit(f'Refusing to overwrite evidence bundle: {destination}')
    destination.mkdir(parents=True)
    # Runtime interface bundle excludes Python modules, headers and CMake files.
    shutil.copytree(install / 'share/ament_index', destination / 'share/ament_index')
    (destination / 'lib').mkdir()
    for lib in (install / 'lib').glob('*.dylib'):
        if 'generator_py' in lib.name:
            continue
        architectures = subprocess.check_output(['lipo', '-archs', str(lib)], text=True).split()
        host_arch = 'arm64' if platform.machine() == 'arm64' else 'x86_64'
        if host_arch not in architectures:
            raise SystemExit(f'Native library architecture mismatch: {lib.name}: {architectures}; expected {host_arch}')
        target = destination / 'lib' / lib.name
        shutil.copy2(lib, target)
        loads=subprocess.check_output(['otool','-L',str(target)],text=True).splitlines()[1:]
        edits=[]
        for line in loads:
            dep=line.strip().split(' (')[0]
            if dep.startswith('@rpath/') and not any((prefix/'lib'/Path(dep).name).exists() for prefix in [runtime,install]):
                raise SystemExit(f'Missing native dependency: {dep}; build dependencies into the matching runtime first')
            if dep.startswith('/') and not dep.startswith(('/System/','/usr/lib/',str(runtime)+'/',str(install)+'/')):
                raise SystemExit(f'Unbundled external native dependency: {dep}')
            if dep.startswith(str(runtime)+'/') or dep.startswith(str(install)+'/'):
                edits += ['-change',dep,'@rpath/'+Path(dep).name]
        commands=subprocess.check_output(['otool','-l',str(target)],text=True)
        rpaths=re.findall(r'cmd LC_RPATH\s+cmdsize \d+\s+path (.*?) \(offset',commands)
        for path in rpaths:
            if path.startswith('/') and not path.startswith(('/System/','/usr/lib/')):
                edits += ['-delete_rpath',path]
        if '@loader_path' not in rpaths:edits += ['-add_rpath','@loader_path']
        if edits:subprocess.run(['install_name_tool',*edits,str(target)],check=True,capture_output=True)
        subprocess.run(['codesign', '--force', '--sign', '-', str(target)], check=True, capture_output=True)
    files = {str(p.relative_to(destination)): hashlib.sha256(p.read_bytes()).hexdigest()
             for p in sorted(destination.rglob('*')) if p.is_file()}
    manifest = dict(schema_version=1, ros_distro=distro, os='macos', arch='aarch64' if platform.machine()=='arm64' else 'x86_64', packages=[package], files=files)
    (destination / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(destination, flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--package', required=True)
    parser.add_argument('--source-install', type=Path, help='Export an existing dedicated single-package native install; no compilation')
    parser.add_argument('--destination', type=Path, required=True)
    parser.add_argument('--distro', choices=['humble', 'jazzy', 'lyrical'], default='jazzy')
    args = parser.parse_args()
    build(args.package, args.destination.resolve(), args.distro, args.source_install)
