"""Fetch version-referenced upstream notice files without executing source contents."""
import argparse
from concurrent.futures import ThreadPoolExecutor,as_completed
from datetime import datetime,timezone
import hashlib,io,json,platform,re,sys,tarfile,urllib.parse,urllib.request
from pathlib import Path,PurePosixPath
import yaml
ROOT=Path(__file__).resolve().parents[1];RAW=ROOT/'artifacts/feasibility';PREFIX=ROOT/'.pixi/envs/default'
parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--inventory',type=Path,required=True);parser.add_argument('--repository',action='append',default=[],help='Verified PACKAGE=URL repository mapping; crate VCS commit still required');parser.add_argument('--package',action='append',default=[],help='Limit this report to exact package identifiers');parser.add_argument('--prior',type=Path,help='Prior retrievals for uncovered packages; use full upstream release-version tags');parser.add_argument('--label',default=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ'));args=parser.parse_args()
overrides=dict(item.split('=',1) for item in args.repository)
if not args.label.replace('-','').isalnum():parser.error('Use alphanumeric label')
out=RAW/'evidence'/('upstream-licenses-'+args.label);out.mkdir(parents=True,exist_ok=False)
inventory=json.loads(args.inventory.read_text());missing={(x['kind'],x['package']) for x in inventory['missing_text_packages'] if not args.package or x['package'] in args.package}
manifest=json.loads((Path(inventory['bundle'])/'manifest.json').read_text());distro=manifest['distro']
if distro not in ['humble','jazzy','lyrical']:parser.error('Unsupported inventory distro')
workspace=ROOT if distro=='jazzy' else ROOT/'environments'/distro
PREFIX=workspace/'.pixi/envs/default'
if hashlib.sha256((workspace/'pixi.lock').read_bytes()).hexdigest()!=manifest.get('pixi_lock_sha256'):parser.error('Inventory runtime lock mismatch')
metadata=json.loads((args.inventory.parent/'cargo-metadata.json').read_text());crates={p['name']+'-'+p['version']:p for p in metadata['packages']}
packages={f"{d['name']}-{d['version']}-{d['build']}":d for p in (PREFIX/'conda-meta').glob('*.json') for d in [json.loads(p.read_text())]}
plans=[];unresolved=[]
prior=json.loads(args.prior.read_text()) if args.prior else []
prior_covered={p['package'] for r in prior if r['status']=='retrieved' for p in r['packages']}
def add(entry,repo,revision,kind,reference):
    repo=(repo or '').removesuffix('.git').rstrip('/').replace('http://github.com/','https://github.com/')
    if repo.startswith('https://github.com/') and '/tree/' in repo:repo=repo.split('/tree/',1)[0]
    if not repo.startswith('https://github.com/') or len(repo.split('/'))!=5 or not revision:
        unresolved.append({'package':entry['package'],'kind':entry['kind'],'reason':'No supported repository and pinned revision','repository':repo});return
    slug=repo.removeprefix('https://github.com/')
    plans.append({'package':entry['package'],'kind':entry['kind'],'repository':repo,'revision':revision,'pin_kind':kind,'reference':str(reference),'reference_sha256':hashlib.sha256(reference.read_bytes()).hexdigest(),'url':'https://codeload.github.com/'+slug+'/tar.gz/'+urllib.parse.quote(revision,safe='')})
for entry in inventory['entries']:
    if (entry['kind'],entry['package']) not in missing or entry['package'] in prior_covered:continue
    if entry['kind']=='native-package':
        d=packages[entry['package']];recipe=Path(d['link']['source'])/'info/recipe/rendered_recipe.yaml'
        if not recipe.is_file():unresolved.append({'package':entry['package'],'reason':'Missing rendered recipe'});continue
        sources=yaml.safe_load(recipe.read_text()).get('recipe',{}).get('source',[])
        if isinstance(sources,dict):sources=[sources]
        found=False
        for source in sources:
            if 'git' in source:
                found=True;add(entry,source['git'],('refs/tags/upstream/'+str(d['version'])) if args.prior else source.get('rev') or source.get('tag'),'release-upstream-version-tag' if args.prior else 'package-recipe-reference',recipe)
        if not found:unresolved.append({'package':entry['package'],'reason':'No git source in recipe'})
    else:
        p=crates[entry['package']];vcs=Path(p['manifest_path']).parent/'.cargo_vcs_info.json'
        if not vcs.is_file():unresolved.append({'package':entry['package'],'reason':'No crate VCS revision'});continue
        repo=overrides.get(entry['package'],p.get('repository'));readme=Path(p['manifest_path']).parent/'README.md'
        readme_repo=False
        if not repo and readme.is_file():
            candidates=set(re.findall(r'https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+',readme.read_text()))
            if len(candidates)==1:repo=candidates.pop().rstrip('.');readme_repo=True
        before=len(plans)
        add(entry,repo,json.loads(vcs.read_text()).get('git',{}).get('sha1'),'crate-vcs-commit',vcs)
        if readme_repo and len(plans)>before:
            plans[-1]['repository_readme']=str(readme)
            plans[-1]['repository_readme_sha256']=hashlib.sha256(readme.read_bytes()).hexdigest()
(out/'source-plan.json').write_text(json.dumps({'plans':plans,'unresolved':unresolved},indent=2)+'\n')
# Deduplicate shared repository revisions while preserving per-package provenance.
groups={}
for plan in plans:groups.setdefault(plan['url'],[]).append(plan)
def fetch(item):
    url,plans=item;key=hashlib.sha256(url.encode()).hexdigest()[:20];directory=out/key;directory.mkdir()
    result={'url':url,'packages':plans,'texts':[]}
    try:
        request=urllib.request.Request(url,headers={'User-Agent':'ROS-Studio-feasibility-notice-collector'})
        with urllib.request.urlopen(request,timeout=30) as response:data=response.read(16*1024*1024+1)
        if len(data)>16*1024*1024:raise ValueError('Archive exceeds 16 MiB limit')
        (directory/'source.tar.gz').write_bytes(data);result['archive_sha256']=hashlib.sha256(data).hexdigest()
        with tarfile.open(fileobj=io.BytesIO(data),mode='r:gz') as archive:
            members=archive.getmembers()
            if sum(m.size for m in members)>128*1024*1024:raise ValueError('Uncompressed archive exceeds 128 MiB limit')
            for member in members:
                path=PurePosixPath(member.name)
                if not member.isfile() or path.is_absolute() or '..' in path.parts:continue
                if not path.name.upper().startswith(('LICENSE','LICENCE','COPYING','NOTICE','COPYRIGHT')):continue
                if member.size>2*1024*1024:raise ValueError('Notice exceeds 2 MiB limit')
                file=directory/'texts'/Path(*path.parts[1:]);file.parent.mkdir(parents=True,exist_ok=True)
                stream=archive.extractfile(member)
                if stream is None:continue
                file.write_bytes(stream.read());result['texts'].append({'file':str(file.relative_to(out)),'archive_path':member.name,'sha256':hashlib.sha256(file.read_bytes()).hexdigest()})
        result['status']='retrieved' if result['texts'] else 'no-notice-files'
    except Exception as e:result['status']='failed';result['error']=str(e)
    return result
results=[]
with ThreadPoolExecutor(max_workers=4) as pool:
    for future in as_completed([pool.submit(fetch,item) for item in groups.items()]):
        result=future.result();results.append(result)
        (out/'retrievals.json').write_text(json.dumps(results,indent=2)+'\n')
        print(len(results),'/',len(groups),result['status'],result['url'],flush=True)
covered=prior_covered|{p['package'] for r in results if r['status']=='retrieved' for p in r['packages']};all_missing={package for _,package in missing};remaining=sorted(all_missing-covered)
base=dict(os=platform.platform(),cpu=platform.machine(),distro=distro,middleware_pair=[None,None],versions={'cargo_lock_sha256':hashlib.sha256((ROOT/'Cargo.lock').read_bytes()).hexdigest(),'pixi_lock_sha256':hashlib.sha256((workspace/'pixi.lock').read_bytes()).hexdigest()},command=[sys.executable,'scripts/fetch-license-evidence.py','--inventory',str(args.inventory.resolve()),'--label',args.label],evidence=[str((out/'source-plan.json').relative_to(RAW)),str((out/'retrievals.json').relative_to(RAW))])
for package in args.package:base['command'] += ['--package',package]
for repository in args.repository:base['command'] += ['--repository',repository]
if args.prior:
    base['command'] += ['--prior',str(args.prior.resolve())]
    base['evidence'].append(str(args.prior.resolve().relative_to(RAW)))
rows=[dict(base,test='version-referenced-notice-retrieval',outcome='passed' if not remaining else 'failed',retrieval_groups=len(results),packages_with_candidate_texts=len(covered),remaining_packages=remaining,collected_files=sum(len(r['texts']) for r in results),limitation='Repository notice files are candidates, not confirmed per-package completeness. Recipe tags are version references, not verified original build commits.'),dict(base,test='upstream-notice-review',outcome='not tested',reason='Candidate attribution, embedded/vendored notices and license choices still require review')]
(RAW/('upstream-licenses-'+args.label+'.json')).write_text(json.dumps(rows,indent=2)+'\n')
print(json.dumps({'candidate_packages':len(covered),'remaining_count':len(remaining)}))
