"""Collect embedded source notices from already hash-verified upstream archives.
Candidate evidence only: does not establish package attribution or legal completeness.
"""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import platform
import re
import sys
import tarfile

ROOT = Path(__file__).resolve().parents[1]
RAW = ROOT / 'artifacts/feasibility'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def header(text):
    text = text.lstrip('\ufeff\r\n ')
    if text.startswith('/*'):
        end = text.find('*/')
        return text[:end + 2] if end >= 0 else ''
    lines = []
    for line in text.splitlines():
        if not line.strip() or line.lstrip().startswith(('#', '//')):
            lines.append(line)
        else:
            break
    return '\n'.join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--retrievals', type=Path, required=True)
    parser.add_argument('--package', action='append', required=True)
    parser.add_argument('--label', required=True)
    parser.add_argument('--distro', choices=['humble','jazzy','lyrical'], default='jazzy')
    args = parser.parse_args()
    if not re.fullmatch(r'[a-zA-Z0-9-]+', args.label):
        parser.error('Use an alphanumeric label')
    out = RAW / 'evidence' / ('embedded-notices-' + args.label)
    out.mkdir(parents=True, exist_ok=False)
    results = []
    for record in json.loads(args.retrievals.read_text()):
        packages = [p for p in record['packages'] if p['package'] in args.package]
        if not packages:
            continue
        key = hashlib.sha256(record['url'].encode()).hexdigest()[:20]
        archive_path = args.retrievals.parent / key / 'source.tar.gz'
        if sha(archive_path) != record['archive_sha256']:
            raise ValueError('Source archive checksum mismatch: ' + str(archive_path))
        texts = []
        with tarfile.open(archive_path) as archive:
            for member in archive:
                path = PurePosixPath(member.name)
                if not member.isfile() or member.size > 2 * 1024 * 1024:
                    continue
                if path.is_absolute() or '..' in path.parts:
                    raise ValueError('Unsafe archive path')
                stream = archive.extractfile(member)
                source = stream.read() if stream else b''
                notice = header(source.decode('utf-8', errors='replace'))
                # Require an actual grant/license notice, not a lone copyright line.
                if not re.search(r'Copyright', notice, re.I) or not any(
                    marker in notice for marker in ['Redistribution and use', 'Permission is hereby granted', 'Licensed under the Apache License']
                ):
                    continue
                dest = out / key / Path(*path.parts[1:])
                dest = dest.with_name(dest.name + '.notice.txt')
                dest.parent.mkdir(parents=True, exist_ok=True)
                dest.write_text(notice + '\n')
                texts.append(dict(file=str(dest.relative_to(out)), sha256=sha(dest), archive_path=member.name, source_sha256=hashlib.sha256(source).hexdigest()))
        results.append(dict(url=record['url'], packages=packages, archive_sha256=sha(archive_path), source_archive=str(archive_path.resolve().relative_to(RAW)), status='retrieved' if texts else 'no-notice-files', texts=texts, limitation='Repository-level header candidates; review per-package applicability and completeness'))
    (out / 'retrievals.json').write_text(json.dumps(results, indent=2) + '\n')
    covered = {p['package'] for r in results if r['texts'] for p in r['packages']}
    missing = sorted(set(args.package) - covered)
    workspace=ROOT if args.distro=='jazzy' else ROOT/'environments'/args.distro
    report = dict(test='embedded-notice-candidates', outcome='passed' if not missing else 'failed', os=platform.platform(), cpu=platform.machine(), distro=args.distro, middleware_pair=[None,None], versions={'cargo_lock_sha256':sha(ROOT/'Cargo.lock'),'pixi_lock_sha256':sha(workspace/'pixi.lock'),'source_retrievals_sha256':sha(args.retrievals)}, command=[sys.executable,str(Path(__file__).resolve()),*sys.argv[1:]], evidence=[str((out/'retrievals.json').relative_to(RAW)),str(args.retrievals.resolve().relative_to(RAW))], remaining_packages=missing, collected_files=sum(len(r['texts']) for r in results), limitation='Candidate collection only; attribution and redistribution review remain open')
    (RAW / ('embedded-notices-' + args.label + '.json')).write_text(json.dumps([report], indent=2) + '\n')
    print(json.dumps({'collected_files':report['collected_files'],'remaining_packages':missing}))


if __name__ == '__main__':
    main()
