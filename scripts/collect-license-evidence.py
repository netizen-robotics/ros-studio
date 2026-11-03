"""Collect cached license/notice texts with provenance; not a legal compliance decision."""
import argparse
from datetime import datetime,timezone
import hashlib,json,os,platform,shutil,subprocess,sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];RAW=ROOT/'artifacts/feasibility';PREFIX=ROOT/'.pixi/envs/default'
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--bundle',type=Path,required=True)
parser.add_argument('--label',default=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ'))
args=parser.parse_args()
if not args.label.replace('-','').isalnum():parser.error('Use alphanumeric label')
bundle=args.bundle.resolve();out=RAW/'evidence'/('licenses-'+args.label);out.mkdir(parents=True,exist_ok=False)
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
manifest=json.loads((bundle/'manifest.json').read_text());distro=manifest.get('distro')
if distro not in ['humble','jazzy','lyrical']:parser.error('Unsupported runtime distro')
workspace=ROOT if distro=='jazzy' else ROOT/'environments'/distro
PREFIX=workspace/'.pixi/envs/default'
if manifest.get('pixi_lock_sha256')!=sha(workspace/'pixi.lock'):
    parser.error('Bundle dependency lock does not match the selected inventory environment')
if platform.system()!='Darwin' or platform.machine()!='arm64':
    parser.error('Current inventory targets macOS ARM64 only')
def copy_files(files,base,destination):
    copied=[]
    for file in sorted(set(files)):
        if not file.is_file():continue
        relative=file.relative_to(base);target=destination/relative;target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(file,target)
        copied.append({'source':str(file),'file':str(target.relative_to(out)),'sha256':sha(target)})
    return copied
# Identify ownership of native libraries AND redistributed interface resources.
owned={};packages={}
for file in (PREFIX/'conda-meta').glob('*.json'):
    d=json.loads(file.read_text());key=f"{d['name']}-{d['version']}-{d['build']}";packages[key]=d
    for relative in d.get('files',[]):owned.setdefault(relative,[]).append(key)
selected={};unowned=[]
for base in [bundle/'lib',bundle/'share']:
    for file in base.rglob('*'):
        if not file.is_file():continue
        relative=str(file.relative_to(bundle));owners=owned.get(relative,[])
        if not owners:unowned.append(relative)
        for owner in owners:selected.setdefault(owner,[]).append(relative)
entries=[]
for key,files in sorted(selected.items()):
    d=packages[key];cache=Path(d.get('link',{}).get('source',''))
    license_dir=cache/'info/licenses'
    texts=list(license_dir.rglob('*')) if license_dir.is_dir() else []
    copied=copy_files(texts,license_dir,out/'native'/key) if texts else []
    entries.append({'kind':'native-package','package':key,'license_expression':d.get('license'),'package_archive_sha256':d.get('sha256'),'shipped_files':files,'texts':copied})
# A target-filtered dependency graph is a conservative superset: it includes
# build dependencies and all workspace binaries, not a precise final link map.
command=['cargo','metadata','--locked','--offline','--format-version','1','--filter-platform','aarch64-apple-darwin']
metadata_env=dict(os.environ,CARGO_HOME=str(ROOT/'.cache/cargo'))
metadata=json.loads(subprocess.check_output(command,cwd=ROOT,text=True,env=metadata_env));(out/'cargo-metadata.json').write_text(json.dumps(metadata)+'\n')
resolved={n['id'] for n in metadata['resolve']['nodes']}
for package in metadata['packages']:
    if package['id'] not in resolved:continue
    base=Path(package['manifest_path']).parent
    if package['source'] is None:continue
    files=[p for p in base.rglob('*') if p.is_file() and p.name.upper().startswith(('LICENSE','LICENCE','COPYING','NOTICE','COPYRIGHT'))]
    if package.get('license_file'):
        path=Path(package['license_file']);files.append(path if path.is_absolute() else base/path)
    key=package['name']+'-'+package['version']
    entries.append({'kind':'rust-crate','package':key,'license_expression':package.get('license'),'cargo_id':package['id'],'texts':copy_files(files,base,out/'rust'/key)})
stdlib=[PREFIX/'share/doc/rust/COPYRIGHT-library.html',PREFIX/'share/doc/rust/COPYRIGHT.html']
entries.append({'kind':'rust-toolchain-notices','package':'rust-1.92.0','license_expression':None,'texts':copy_files(stdlib,PREFIX/'share/doc/rust',out/'rust-toolchain')})
missing=[{'kind':e['kind'],'package':e['package'],'license_expression':e['license_expression']} for e in entries if not e['texts']]
inventory={'scope':'Cached notice collection, not permission/compliance certification. Rust graph is a conservative superset; vendored/embedded obligations and selections still require review.','bundle':str(bundle),'entries':entries,'missing_text_packages':missing,'unowned_bundle_files':unowned,'cargo_metadata_command':command}
(out/'inventory.json').write_text(json.dumps(inventory,indent=2)+'\n')
base=dict(os=platform.platform(),cpu=platform.machine(),distro=distro,middleware_pair=[None,None],versions={'cargo_lock_sha256':sha(ROOT/'Cargo.lock'),'pixi_lock_sha256':sha(workspace/'pixi.lock'),'runtime_manifest_sha256':sha(bundle/'manifest.json')},command=[sys.executable,'scripts/collect-license-evidence.py','--bundle',str(bundle),'--label',args.label],environment={'CARGO_HOME':metadata_env['CARGO_HOME']},evidence=[str((out/'inventory.json').relative_to(RAW))])
results=[dict(base,test='cached-license-text-coverage',outcome='passed' if not missing and not unowned else 'failed',packages=len(entries),missing_text_packages=missing,unowned_bundle_files=unowned,collected_files=sum(len(e['texts']) for e in entries)),dict(base,test='redistribution-license-review',outcome='not tested',reason='Collection does not determine license choices, notice completeness, vendored content, or distribution obligations')]
(RAW/('licenses-'+args.label+'.json')).write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps({'packages':len(entries),'missing_text_packages':missing,'unowned_files':len(unowned),'collected_files':sum(len(e['texts']) for e in entries)},indent=2))
