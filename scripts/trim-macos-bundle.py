"""Trim an audited runtime without changing its source; preserve native ROS type support."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
ROOT=Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--source',type=Path,required=True)
parser.add_argument('--inventory',type=Path,required=True)
parser.add_argument('--destination',type=Path,required=True)
args=parser.parse_args();source=args.source.resolve();dest=args.destination.resolve()
if dest.exists():raise SystemExit('Destination exists; preserve earlier bundles')
records=json.loads(args.inventory.read_text());byname={Path(r['file']).name:r for r in records}
if len(byname)!=len(records):raise SystemExit('Ambiguous native basenames')
for r in records:
    file=source/r['file']
    if hashlib.sha256(file.read_bytes()).hexdigest()!=r['sha256']:raise SystemExit(f'Stale inventory: {file}')
roots={'studio-backend','studio-desktop','librmw_fastrtps_cpp.dylib','librmw_cyclonedds_cpp.dylib','librcl_logging_spdlog.dylib'}
# The default logging implementation is dynamically loaded on newer distros.
# Generated C/C++ support remains available for standard messages/services/actions.
# Python generators and build tools are not roots. Native dependencies are retained
# even if their name would otherwise look like a development library.
roots.update(n for n in byname if ('__rosidl_' in n and 'generator_py' not in n) or n.startswith('librosidl_dynamic_typesupport'))
missing=roots-byname.keys()
if missing:raise SystemExit(f'Missing runtime roots: {missing}')
selected=set();pending=list(roots)
while pending:
    name=pending.pop()
    if name in selected:continue
    selected.add(name)
    for dep in byname[name]['dependencies']:
        if dep.startswith(('/System/','/usr/lib/')):continue
        dependency=Path(dep).name
        if dependency==name:continue
        if dependency not in byname:raise SystemExit(f'Unknown dependency: {name}: {dep}')
        pending.append(dependency)
# No binary edits are required: audited relocated paths and ad-hoc signatures stay intact.
dest.mkdir(parents=True)
for folder in ['share','dependency-metadata']:
    shutil.copytree(source/folder,dest/folder)
if (source/'notices').is_dir():
    shutil.copytree(source/'notices',dest/'notices')
for name in sorted(selected):
    relative=byname[name]['file'];target=dest/relative;target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(source/relative,target)
manifest=json.loads((source/'manifest.json').read_text())
manifest.update(native_root_policy='Backend/desktop, Fast/Cyclone RMW, dynamically loaded spdlog implementation, standard native ROS C/C++ type support and transitive Mach-O dependencies',native_roots=sorted(roots),native_files=len(selected),removed_native_files=sorted(set(byname)-selected),source=str(source),bytes=sum(p.stat().st_size for p in dest.rglob('*') if p.is_file()),license_review='pending; full original dependency metadata retained, not complete notices')
(dest/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
print(json.dumps({'native_files':len(selected),'removed_native_files':len(byname)-len(selected),'bytes':manifest['bytes']}))
