"""Inspect Mach-O dependency closure and minimum OS requirements; no clean-host claim."""
import argparse
from datetime import datetime,timezone
import hashlib,json,platform,re,subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];RAW=ROOT/'artifacts/feasibility'
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--bundle',type=Path,required=True)
parser.add_argument('--label',default=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ'))
parser.add_argument('--minimum-macos',default='14.0')
args=parser.parse_args()
if platform.system()!='Darwin':raise SystemExit('Mach-O audit requires macOS')
if not args.label.replace('-','').isalnum():parser.error('Use alphanumeric label')
bundle=args.bundle.resolve();manifest=json.loads((bundle/'manifest.json').read_text());distro=manifest['distro']
if distro not in ['humble','jazzy','lyrical']:raise SystemExit('Unsupported bundle distro')
workspace=ROOT if distro=='jazzy' else ROOT/'environments'/distro
out=RAW/'evidence'/('packaging-audit-'+args.label);out.mkdir(parents=True,exist_ok=False)
files=list((bundle/'lib').glob('*.dylib'))+list((bundle/'bin').iterdir())
if not files:raise SystemExit('No native bundle files')
def version(v):return tuple((list(map(int,v.split('.')))+[0,0,0])[:3])
records=[];missing=[];external=[];too_new=[]
for file in sorted(files):
    cmd=['otool','-L',str(file)];loads=subprocess.check_output(cmd,text=True)
    commands=['otool','-l',str(file)];details=subprocess.check_output(commands,text=True)
    deps=[line.strip().split(' (')[0] for line in loads.splitlines()[1:]]
    paths=re.findall(r'cmd LC_RPATH\s+cmdsize \d+\s+path (.*?) \(offset',details)
    minimums=re.findall(r'cmd LC_BUILD_VERSION\s+cmdsize \d+\s+platform \S+\s+minos ([\d.]+)',details)+re.findall(r'cmd LC_VERSION_MIN_MACOSX\s+cmdsize \d+\s+version ([\d.]+)',details)
    for minimum in minimums:
        if version(minimum)>version(args.minimum_macos):too_new.append({'file':str(file.relative_to(bundle)),'minimum':minimum})
    for path in paths:
        if path.startswith('/') and not path.startswith(('/System/','/usr/lib/')):external.append({'file':str(file.relative_to(bundle)),'rpath':path})
    for dep in deps:
        if file.suffix=='.dylib' and Path(dep).name==file.name:continue
        if dep.startswith(('/System/','/usr/lib/')):continue
        if dep.startswith('@rpath/'):
            resolved=bundle/'lib'/dep.removeprefix('@rpath/')
        elif dep.startswith('@loader_path/'):
            resolved=file.parent/dep.removeprefix('@loader_path/')
        elif dep.startswith('@executable_path/'):
            resolved=bundle/'bin'/dep.removeprefix('@executable_path/')
        else:
            external.append({'file':str(file.relative_to(bundle)),'dependency':dep});continue
        if not resolved.is_file():missing.append({'file':str(file.relative_to(bundle)),'dependency':dep,'expected':str(resolved)})
    records.append({'file':str(file.relative_to(bundle)),'bytes':file.stat().st_size,'sha256':hashlib.sha256(file.read_bytes()).hexdigest(),'dependencies':deps,'rpaths':paths,'minimum_macos':minimums,'commands':[cmd,commands]})
inventory=out/'native-inventory.json';inventory.write_text(json.dumps(records,indent=2)+'\n')
base=dict(os=platform.platform(),cpu=platform.machine(),distro=distro,middleware_pair=[None,None],command=['scripts/dev','.pixi/envs/default/bin/python','scripts/audit-macos-bundle.py','--bundle',str(bundle),'--label',args.label,'--minimum-macos',args.minimum_macos],versions={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in [ROOT/'Cargo.lock',workspace/'pixi.lock']},evidence=[str(inventory.relative_to(RAW))])
results=[dict(base,test='native-dependency-closure',outcome='passed' if not missing and not external else 'failed',missing=missing,external_paths=external,native_files=len(files),total_bundle_bytes=sum(p.stat().st_size for p in bundle.rglob('*') if p.is_file())),dict(base,test='minimum-macos',outcome='passed' if not too_new and all(r['minimum_macos'] for r in records) else 'failed',target=args.minimum_macos,incompatible_files=too_new,missing_metadata=[r['file'] for r in records if not r['minimum_macos']]),dict(base,test='clean-native-host',outcome='not tested',reason='Audit runs on development Mac; sanitized environment is not a clean host'),dict(base,test='redistribution-license-review',outcome='not tested',reason='Package metadata is bundled; full license text/notice completeness not yet reviewed')]
(RAW/('packaging-audit-'+args.label+'.json')).write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps({r['test']:r['outcome'] for r in results}));print(f'{len(files)} native files; {len(missing)} missing dependencies; {len(external)} external paths; {len(too_new)} exceed macOS {args.minimum_macos}')
raise SystemExit(0 if all(r['outcome']!='failed' for r in results) else 1)
