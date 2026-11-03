"""Wrap an existing relocated runtime in a standalone macOS app directory."""
import argparse
import json
import hashlib
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
root=Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser()
parser.add_argument('--runtime',type=Path,default=root/'artifacts/relocated runtime with spaces')
parser.add_argument('--profile',choices=['debug','release'],default='debug')
parser.add_argument('--destination',type=Path,default=root/'artifacts/ROS Studio Feasibility.app')
parser.add_argument('--refresh',action='store_true',help='Update only executables in this generated prototype app')
args=parser.parse_args()
manifest=json.loads((args.runtime/'manifest.json').read_text())
distro=manifest.get('distro')
if distro not in ['humble','jazzy','lyrical']:raise SystemExit('Runtime manifest must declare supported distro')
target=root/'target' if distro=='jazzy' else root/'target'/distro
bundle_id='dev.rosstudio.feasibility'+('' if distro=='jazzy' else '.'+distro)
app=args.destination.resolve()
contents=app/'Contents'
if app.exists():
    if not args.refresh: raise SystemExit(f'Use --refresh for the existing generated prototype: {app}')
    with (contents/'Info.plist').open('rb') as f:
        if plistlib.load(f).get('CFBundleIdentifier') != bundle_id:
            raise SystemExit('Refusing to update an unrelated app')
else:
    # Keep code and data in macOS bundle locations. Never mutate runtime hardlinks
    # while rewriting/signing this app's Mach-O images.
    contents.mkdir(parents=True)
    shutil.copytree(args.runtime/'bin',contents/'MacOS')
    shutil.copytree(args.runtime/'lib',contents/'Frameworks')
    resources=contents/'Resources'
    resources.mkdir()
    # ament resolves type-support libraries relative to its resource prefix.
    (resources/'lib').symlink_to('../Frameworks',target_is_directory=True)
    for source in args.runtime.iterdir():
        if source.name in ['bin','lib']: continue
        if source.is_dir(): shutil.copytree(source,resources/source.name)
        else: shutil.copy2(source,resources/source.name)
if not (contents/'Frameworks').is_dir():
    raise SystemExit('Legacy app layout: create a new destination instead of refreshing')
# Use current prototype executables; unlink hardlinks before replacing them.
for name in ['studio-desktop','studio-backend']:
    final=contents/'MacOS'/name
    staging=Path(tempfile.mkdtemp(prefix='app-executable-',dir=root/'artifacts'))
    dest=staging/name
    shutil.copy2(target/args.profile/name,dest)
    loads=subprocess.check_output(['otool','-l',str(dest)],text=True).splitlines()
    paths=[loads[i+2].strip().split(' ')[1] for i,s in enumerate(loads) if s.strip()=='cmd LC_RPATH']
    for path in paths:
        if path.startswith('/') or path == '@executable_path/../lib':
            subprocess.run(['install_name_tool','-delete_rpath',path,str(dest)],check=True,capture_output=True)
    if '@executable_path/../Frameworks' not in paths:
        subprocess.run(['install_name_tool','-add_rpath','@executable_path/../Frameworks',str(dest)],check=True,capture_output=True)
    subprocess.run(['codesign','--force','--sign','-',str(dest)],check=True,capture_output=True)
    dest.replace(final)
    staging.rmdir()
manifest.update(distro=distro,build_profile=args.profile,source_binary_hashes={name:hashlib.sha256((target/args.profile/name).read_bytes()).hexdigest() for name in ['studio-desktop','studio-backend']},app_binary_hashes={name:hashlib.sha256((contents/'MacOS'/name).read_bytes()).hexdigest() for name in ['studio-desktop','studio-backend']})
(contents/'Resources/manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
with (contents/'Info.plist').open('wb') as f:
    plistlib.dump({'CFBundleExecutable':'studio-desktop','CFBundleIdentifier':bundle_id,'CFBundleName':'ROS Studio Feasibility','CFBundlePackageType':'APPL','CFBundleVersion':'0.1.0','NSHighResolutionCapable':True},f)
subprocess.run(['codesign','--force','--sign','-',str(app)],check=True)
subprocess.run(['codesign','--verify','--deep','--strict',str(app)],check=True)
print(app)
