"""Bound development checkpoints; this is agent workflow tooling, not app runtime.

start --id NAME --outcome TEXT --step TEXT ... (one to five steps)
step --id NAME --number N --result passed|unresolved --note TEXT
repair --id NAME --note TEXT (retain every diagnosis/change/retest cycle)
reopen --id NAME --note TEXT (requires explicit user authorization)
close --id NAME --result passed|unresolved --note TEXT
status --id NAME
"""
import argparse
import json
from pathlib import Path
import re
import sys

MAX_STEPS = 5
ROOT = Path(__file__).resolve().parents[1]
STATE = ROOT / 'artifacts/feasibility/checkpoints'


def execute(args, directory=STATE):
    if not re.fullmatch(r'[a-z0-9][a-z0-9-]{0,79}', args.id):
        raise ValueError('Use a short lowercase checkpoint id')
    directory.mkdir(parents=True, exist_ok=True)
    target = directory / (args.id + '.json')
    if args.command == 'start':
        if target.exists():
            raise ValueError('Checkpoint already exists; its allowance cannot be reset')
        if not args.outcome.strip() or not 1 <= len(args.step) <= MAX_STEPS:
            raise ValueError('Define one outcome and one to five concrete steps')
        steps = [s.strip() for s in args.step]
        if any(not s for s in steps) or len(set(steps)) != len(steps):
            raise ValueError('Steps must be nonempty and distinct')
        outcome = ' '.join(args.outcome.lower().split())
        for previous in directory.glob('*.json'):
            record = json.loads(previous.read_text())
            if record['status'] == 'active':
                raise ValueError('Finish or report the active checkpoint before starting another')
            if record['outcome_key'] == outcome:
                raise ValueError('This outcome is already tracked; renaming cannot reset its allowance')
        record = dict(id=args.id, outcome=args.outcome.strip(), outcome_key=outcome,
                      max_steps=MAX_STEPS, repairs=[], status='active',
                      steps=[dict(description=s, result='pending', note='') for s in steps])
    else:
        if not target.exists():
            raise ValueError('Checkpoint does not exist')
        record = json.loads(target.read_text())
        if args.command == 'status':
            return record
        if args.command == 'reopen':
            if record['status'] != 'unresolved' or not args.note.strip():
                raise ValueError('Only unresolved checkpoints can reopen with an authorization note')
            if any(json.loads(p.read_text())['status'] == 'active' for p in directory.glob('*.json')):
                raise ValueError('Close the active checkpoint first')
            record.setdefault('reopened', []).append(dict(note=args.note, steps=record['steps'], close_note=record.get('close_note')))
            record['steps'] = [dict(description=s['description'], result='pending', note='') if s['result'] == 'unresolved' else s.copy() for s in record['steps']]
            record['status'] = 'active'
            target.write_text(json.dumps(record, indent=2) + '\n')
            return record
        if record['status'] != 'active':
            raise ValueError('Closed checkpoint cannot be restarted or changed')
        if not args.note.strip():
            raise ValueError('Record a concrete result, failure or evidence reference')
        if args.command == 'step':
            if not 1 <= args.number <= len(record['steps']):
                raise ValueError('Step exceeds the frozen checkpoint plan')
            if record['steps'][args.number - 1]['result'] != 'pending':
                raise ValueError('Step already recorded; cannot repeat or reset it')
            if any(s['result'] == 'pending' for s in record['steps'][:args.number - 1]):
                raise ValueError('Record earlier steps before advancing')
            record['steps'][args.number - 1].update(result=args.result, note=args.note)
        elif args.command == 'repair':
            if all(s['result'] != 'pending' for s in record['steps']):
                raise ValueError('Step cap reached; close the checkpoint')
            record['repairs'].append(args.note)
        elif args.command == 'close':
            if args.result == 'passed' and any(s['result'] != 'passed' for s in record['steps']):
                raise ValueError('Cannot pass an incomplete or unresolved checkpoint')
            record['status'] = args.result
            record['close_note'] = args.note
    temporary = target.with_suffix('.tmp')
    temporary.write_text(json.dumps(record, indent=2) + '\n')
    temporary.replace(target)
    return record


def parser():
    result = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    result.add_argument('command', choices=('start', 'step', 'repair', 'close', 'status', 'reopen'))
    result.add_argument('--id', required=True)
    result.add_argument('--outcome', default='')
    result.add_argument('--step', action='append', default=[])
    result.add_argument('--number', type=int, default=0)
    result.add_argument('--result', choices=('passed', 'unresolved'))
    result.add_argument('--note', default='')
    return result


def main():
    args = parser().parse_args()
    if Path(sys.prefix).resolve() != (ROOT / '.pixi/envs/default').resolve():
        raise SystemExit('Use project Pixi Python')
    if args.command in ('step', 'close') and args.result is None:
        raise SystemExit('--result is required')
    try:
        record = execute(args)
    except (ValueError, OSError, KeyError) as error:
        raise SystemExit(str(error)) from error
    finished = sum(s['result'] != 'pending' for s in record['steps'])
    print(f"{record['id']}: {record['status']}; steps {finished}/{len(record['steps'])} "
          f"(max {MAX_STEPS}); repairs {len(record['repairs'])} (no numerical cap)")
    for index, step in enumerate(record['steps'], 1):
        print(f"  {index}. {step['result']}: {step['description']}")


if __name__ == '__main__':
    main()
