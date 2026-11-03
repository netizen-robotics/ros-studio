"""Combine cached and upstream notice evidence without declaring legal completeness."""
import argparse,hashlib,json,platform,sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];RAW=ROOT/'artifacts/feasibility'
parser=argparse.ArgumentParser();parser.add_argument('--inventory',type=Path,required=True);parser.add_argument('--retrievals',type=Path,nargs='+',required=True);parser.add_argument('--label',required=True);args=parser.parse_args()
if not args.label.replace('-','').isalnum():parser.error('Use alphanumeric label')
target=RAW/('license-coverage-'+args.label+'.json')
if target.exists():raise SystemExit('Refusing to overwrite evidence')
inventory=json.loads(args.inventory.read_text());manifest=json.loads((Path(inventory['bundle'])/'manifest.json').read_text());distro=manifest['distro']
if distro not in ['humble','jazzy','lyrical']:raise SystemExit('Unsupported inventory distro')
workspace=ROOT if distro=='jazzy' else ROOT/'environments'/distro
if hashlib.sha256((workspace/'pixi.lock').read_bytes()).hexdigest()!=manifest.get('pixi_lock_sha256'):raise SystemExit('Inventory runtime lock mismatch')
missing={p['package'] for p in inventory['missing_text_packages']};covered=set();sources=[]
for path in args.retrievals:
    for record in json.loads(path.read_text()):
        if record['status']!='retrieved':continue
        # Validate the retrieved evidence remains present and has not changed.
        for notice in record['texts']:
            file=path.parent/notice['file']
            if hashlib.sha256(file.read_bytes()).hexdigest()!=notice['sha256']:raise SystemExit(f'Notice changed: {file}')
        covered.update(p['package'] for p in record['packages'])
    sources.append({'file':str(path.resolve().relative_to(RAW)),'sha256':hashlib.sha256(path.read_bytes()).hexdigest()})
row=dict(test='cumulative-notice-candidates',outcome='passed' if missing<=covered else 'failed',os=platform.platform(),cpu=platform.machine(),distro=distro,middleware_pair=[None,None],versions={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in [ROOT/'Cargo.lock',workspace/'pixi.lock']},command=[sys.executable,str(Path(__file__).resolve()),*sys.argv[1:]],evidence=[str(args.inventory.resolve().relative_to(RAW)),*[x['file'] for x in sources]],source_hashes=sources,original_missing_count=len(missing),candidate_count=len(missing&covered),remaining_packages=sorted(missing-covered),limitation='Candidate files only. Package attribution, archive-version provenance, vendored content, notices, license selection and distribution review remain pending.')
target.write_text(json.dumps([row],indent=2)+'\n');print(json.dumps({'candidate_count':row['candidate_count'],'remaining_packages':row['remaining_packages']}))
