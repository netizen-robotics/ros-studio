"""Index ignored local reports without requiring private planning documents.

Run after tests; --check compares the existing index with available local reports.
A fresh checkout with no evidence reports has nothing to validate. No ROS processes
or network activity are started. The index itself must never become an input report.
"""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RAW = ROOT / 'artifacts/feasibility'


def report_paths():
    return sorted(path for path in RAW.glob('*.json') if path.name != 'RESULTS.json')


def summary():
    reports = []
    previous = RAW / 'RESULTS.json'
    if previous.is_file():
        indexed = json.loads(previous.read_text())
        if indexed.get('schema_version') != 1 or not isinstance(indexed.get('reports'), list):
            raise ValueError('Invalid existing results index; refusing to replace history')
        # Preserve records whose raw evidence is unavailable in this checkout.
        # Absence is neither a deletion request nor a new validation of that run.
        reports.extend(row for row in indexed['reports'] if not (RAW / row['report']).is_file())
    for path in report_paths():
        data = json.loads(path.read_text())
        rows = data if isinstance(data, list) else [data]
        rows = [row for row in rows if isinstance(row, dict)]
        outcomes = Counter(row['outcome'] for row in rows if 'outcome' in row)
        if set(outcomes) - {'passed', 'failed', 'not tested'}:
            raise ValueError(f'Invalid outcome in {path}')
        contexts = {
            json.dumps({key: row[key] for key in ('os', 'cpu', 'distro', 'middleware_pair') if key in row}, sort_keys=True)
            for row in rows
        } - {'{}'}
        reports.append({
            'report': path.name,
            'artifact': str(path.relative_to(ROOT)),
            'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'records': len(rows),
            'outcomes': dict(sorted(outcomes.items())),
            'contexts': [json.loads(context) for context in sorted(contexts)],
        })
    return {
        'schema_version': 1,
        'scope': 'Report index only. Historical runs may overlap; do not sum these into an acceptance score.',
        'evidence_policy': 'Full commands, dependency versions and logs are local ignored artifacts or reviewed CI uploads. This index alone is not reproducible evidence. Missing artifacts are unavailable, not passed.',
        'reports': sorted(reports, key=lambda row: row['report']),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    target = RAW / 'RESULTS.json'
    if not report_paths() and not target.exists():
        print('No local evidence available; nothing to index or validate (not tested).')
        return
    rendered = json.dumps(summary(), indent=2) + '\n'
    if args.check:
        if not target.is_file() or target.read_text() != rendered:
            raise SystemExit('Results index is stale; run scripts/snapshot-evidence.py')
        print('Local report index passed; this does not rerun feature tests.')
    else:
        target.write_text(rendered)
        print(f'Updated {target.relative_to(ROOT)}; detailed evidence stays private in artifacts/.')


if __name__ == '__main__':
    main()
