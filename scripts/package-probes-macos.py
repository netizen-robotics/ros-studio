"""Add distro-matched feasibility probes to a fresh copy of an existing relocated runtime."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

ROOT=Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--runtime',type=Path,required=True)
parser.add_argument('--destination',type=Path,required=True)
parser.add_argument('--profile',choices=['debug','release'],default='release')
parser.add_argument('--refresh-app-binaries',action='store_true',help='Also replace backend/desktop from the selected build profile in the fresh copy')
parser.add_argument('--dependency-runtime',type=Path,help='Already-relocated runtime supplying missing transitive libraries')
args=parser.parse_args()
manifest=json.loads((args.runtime/'manifest.json').read_text())
distro=manifest.get('distro')
if distro not in ['humble','jazzy','lyrical']:raise SystemExit('Unsupported runtime distro')
workspace=ROOT if distro=='jazzy' else ROOT/'environments'/distro
if manifest.get('pixi_lock_sha256')!=hashlib.sha256((workspace/'pixi.lock').read_bytes()).hexdigest():
    raise SystemExit('Runtime dependency lock does not match the selected build environment')
if args.dependency_runtime and json.loads((args.dependency_runtime/'manifest.json').read_text()).get('distro')!=distro:raise SystemExit('Dependency runtime distro mismatch')
target=ROOT/'target' if distro=='jazzy' else ROOT/'target'/distro
dest=args.destination.resolve()
if dest.exists(): raise SystemExit('Destination exists; preserve prior bundles')
shutil.copytree(args.runtime,dest)
hashes={}
added={}
names=['studio-service-probe','studio-action-probe']
if args.refresh_app_binaries:names+=['studio-backend','studio-desktop']
for name in names:
    source=target/args.profile/name
    binary=dest/'bin'/name
    shutil.copy2(source,binary)
    lines=subprocess.check_output(['otool','-l',str(binary)],text=True).splitlines()
    rpaths=[lines[i+2].strip().split(' ')[1] for i,line in enumerate(lines) if line.strip()=='cmd LC_RPATH']
    for path in rpaths:
        if path.startswith('/'):
            subprocess.run(['install_name_tool','-delete_rpath',path,str(binary)],check=True)
    if '@executable_path/../lib' not in rpaths:
        subprocess.run(['install_name_tool','-add_rpath','@executable_path/../lib',str(binary)],check=True)
    subprocess.run(['codesign','--force','--sign','-',str(binary)],check=True)
    subprocess.run(['codesign','--verify','--strict',str(binary)],check=True)
    hashes[name]={'source_sha256':hashlib.sha256(source.read_bytes()).hexdigest(),'packaged_sha256':hashlib.sha256(binary.read_bytes()).hexdigest()}
if args.dependency_runtime:
    pending=list((dest/'bin').iterdir());visited=set()
    while pending:
        binary=pending.pop()
        if binary in visited: continue
        visited.add(binary)
        deps=subprocess.check_output(['otool','-L',str(binary)],text=True).splitlines()[1:]
        for line in deps:
            dep=line.strip().split(' (')[0]
            if dep.startswith(('/System/','/usr/lib/')): continue
            if not dep.startswith('@rpath/'): raise SystemExit(f'Unexpected dependency: {dep}')
            name=dep.removeprefix('@rpath/')
            if Path(name).name != name: raise SystemExit(f'Unsupported dependency path: {dep}')
            target=dest/'lib'/name
            if not target.exists():
                source=args.dependency_runtime/'lib'/name
                shutil.copy2(source,target)
                added[name]={'source':str(source.resolve()),'sha256':hashlib.sha256(source.read_bytes()).hexdigest()}
            pending.append(target)
manifest=json.loads((dest/'manifest.json').read_text())
manifest['feasibility_probes']=hashes
manifest['probe_build_profile']=args.profile
manifest['probe_added_libraries']=added
if args.refresh_app_binaries:
    manifest['build_profile']=args.profile
    manifest['source_binary_hashes']={name:hashes[name]['source_sha256'] for name in ['studio-backend','studio-desktop']}
    manifest['packaged_binary_hashes']={name:hashes[name]['packaged_sha256'] for name in names}
manifest['cargo_lock_sha256']=hashlib.sha256((ROOT/'Cargo.lock').read_bytes()).hexdigest()
manifest['bytes']=sum(p.stat().st_size for p in dest.rglob('*') if p.is_file() and p.name!='manifest.json')
manifest['status']='Experimental probes added; dependency audit and sanitized runtime tests required'
(dest/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
print(dest)
