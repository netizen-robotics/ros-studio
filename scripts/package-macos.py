"""Experimental relocatable native runtime, not a signed product installer."""
import argparse
import hashlib
import json
import os
import platform
from pathlib import Path
import shutil
import subprocess
import sys
ROOT = Path(__file__).resolve().parents[1]
PREFIX = ROOT / '.pixi/envs/default'
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--distro',choices=['humble','jazzy','lyrical'],default='jazzy')
parser.add_argument('--profile', choices=['debug','release'], default='debug')
parser.add_argument('--destination', type=Path, default=None)
args = parser.parse_args()
WORKSPACE=ROOT if args.distro=='jazzy' else ROOT/'environments'/args.distro
PREFIX=WORKSPACE/'.pixi/envs/default'
TARGET=ROOT/'target' if args.distro=='jazzy' else ROOT/'target'/args.distro
DEST = (args.destination or ROOT/'artifacts'/('macos-'+args.distro)).resolve()
if sys.platform != 'darwin':
    raise SystemExit('This packaging experiment requires macOS')
if DEST.exists():
    raise SystemExit(f'{DEST} already exists; select/archive prior experiment before rebuilding')
(DEST/'bin').mkdir(parents=True)
(DEST/'lib').mkdir()
shutil.copytree(PREFIX/'share/ament_index', DEST/'share/ament_index')
# Package manifests and interface definitions, not development executables.
for package in (PREFIX/'share').iterdir():
    if package.is_dir() and (package/'package.xml').exists():
        target = DEST/'share'/package.name
        target.mkdir(parents=True, exist_ok=True)
        shutil.copy2(package/'package.xml', target/'package.xml')
        for kind in ['msg','srv','action']:
            if (package/kind).exists(): shutil.copytree(package/kind, target/kind)
for name in ['studio-backend','studio-desktop']:
    shutil.copy2(TARGET/args.profile/name, DEST/'bin'/name)
for lib in (PREFIX/'lib').glob('*.dylib'):
    shutil.copy2(lib, DEST/'lib'/lib.name)
shutil.copytree(PREFIX/'conda-meta', DEST/'dependency-metadata')
shutil.copytree(ROOT/'crates/visualization/notices', DEST/'notices/visualization')
shutil.copytree(ROOT/'crates/backend/notices/ros-interfaces', DEST/'notices/ros-interfaces')
# Include available cached native license texts without attesting completeness.
notice_inventory = []
for metadata in sorted((PREFIX/'conda-meta').glob('*.json')):
    record = json.loads(metadata.read_text())
    source = Path(record.get('link', {}).get('source', ''))/'info/licenses'
    present = source.is_dir() and any(p.is_file() for p in source.rglob('*'))
    if present:
        shutil.copytree(source, DEST/'notices/runtime'/metadata.stem)
    notice_inventory.append({'package': metadata.stem, 'cached_license_texts': bool(present)})
(DEST/'notices/runtime-license-inventory.json').write_text(json.dumps({
    'scope': 'Available cached native notices; not a completed redistribution or Rust dependency license review',
    'packages': notice_inventory}, indent=2)+'\n')
files = list((DEST/'lib').glob('*.dylib')) + list((DEST/'bin').iterdir())
for file in files:
    dependencies = subprocess.check_output(['otool','-L',str(file)],text=True).splitlines()[1:]
    changes = []
    for line in dependencies:
        dep = line.strip().split(' (')[0]
        if dep.startswith(str(PREFIX)):
            changes += ['-change',dep,'@rpath/'+Path(dep).name]
    commands = subprocess.check_output(['otool','-l',str(file)],text=True).splitlines()
    rpaths = [commands[i+2].strip().split(' ')[1] for i,s in enumerate(commands) if s.strip() == 'cmd LC_RPATH']
    for path in rpaths:
        if path.startswith('/') and not path.startswith('/System/'):
            changes += ['-delete_rpath',path]
    wanted = '@loader_path' if file.parent.name=='lib' else '@executable_path/../lib'
    if wanted not in rpaths: changes += ['-add_rpath',wanted]
    if file.parent.name=='lib': changes += ['-id','@rpath/'+file.name]
    if changes: subprocess.run(['install_name_tool',*changes,str(file)],check=True,capture_output=True)
    subprocess.run(['codesign','--force','--sign','-',str(file)],check=True,capture_output=True)
manifest = {'status':'experimental, clean-machine validation pending','platform':'macOS '+platform.machine(),'distro':args.distro,'pixi_lock_sha256':hashlib.sha256((WORKSPACE/'pixi.lock').read_bytes()).hexdigest(),'cargo_lock_sha256':hashlib.sha256((ROOT/'Cargo.lock').read_bytes()).hexdigest(),'build_profile':args.profile,'source_binary_hashes':{name:hashlib.sha256((TARGET/args.profile/name).read_bytes()).hexdigest() for name in ['studio-backend','studio-desktop']},'dependencies':'dependency-metadata','license_review':'pending; package metadata is not a complete license distribution','bytes':sum(p.stat().st_size for p in DEST.rglob('*') if p.is_file())}
(DEST/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
print(json.dumps(manifest))
print(DEST)
