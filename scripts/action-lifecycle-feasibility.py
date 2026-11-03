"""Concurrent action identity/cancellation and server restart; synthetic domain 187."""
import argparse
from contextlib import ExitStack
import importlib.util
import json
import os
import sys
from pathlib import Path
import subprocess
import time

spec = importlib.util.spec_from_file_location('action_suite', Path(__file__).with_name('action-feasibility.py'))
support = importlib.util.module_from_spec(spec)
spec.loader.exec_module(support)


def launch(suite, stack, bundle, rmw, name, target, timeout=10000, cancel=None):
    stem = suite.out / (name + '-' + rmw)
    request = stem.with_suffix('.json')
    request.write_text(json.dumps({'target': target}))
    cmd = [str(support.PROBE), '--bundle', str(bundle), '--action', '/studio_fixture/count',
           '--type', 'feasibility_interfaces/action/Count', '--goal-file', str(request),
           '--timeout-ms', str(timeout)]
    if cancel: cmd += ['--cancel-after-feedback', str(cancel)]
    stdout = stem.with_suffix('.stdout'); stderr = stem.with_suffix('.stderr')
    out = stack.enter_context(stdout.open('w')); err = stack.enter_context(stderr.open('w'))
    process = subprocess.Popen(cmd, env=dict(suite.client_environment(), RMW_IMPLEMENTATION=rmw), stdout=out, stderr=err)
    stack.callback(support.stop, process)
    return process, cmd, stdout, [request, stdout, stderr]


def finish(client):
    client[0].wait(timeout=20)
    return json.loads(client[2].read_text())


def wait_executing(suite, server):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        if any(e['event'] == 'execute' for e in suite.events(server)): return
        time.sleep(.05)
    raise RuntimeError('Fixture did not receive goal before restart')


def run(label):
    suite = support.Suite(label)
    bundle = suite.build('feasibility_interfaces')
    for rmw in support.RMWS:
        with suite.server('count', rmw, 'normal') as server, ExitStack() as stack:
            a = launch(suite, stack, bundle, rmw, 'cancel-client', 80, cancel=10)
            b = launch(suite, stack, bundle, rmw, 'complete-client', 30)
            ra, rb = finish(a), finish(b)
            events = suite.events(server)
            executions = [e for e in events if e['event'] == 'execute']
            results = [e for e in events if e['event'] == 'result']
            cancellations = [e for e in events if e['event'] == 'cancel']
            ids = [r.get('goal_id', {}).get('uuid') for r in [ra, rb]]
            overlap = len(executions) == len(results) == 2 and max(e['monotonic'] for e in executions) < min(e['monotonic'] for e in results)
            matched = all(any(e['uuid'] == result['goal_id']['uuid'] and e['result'] == result['result_response']['result'] for e in results) for result in [ra, rb])
            requests_match = all(any(e['uuid'] == goal_id and e['goal'] == {'target': target} for e in executions)
                                 for goal_id, target in zip(ids, [80, 30]))
            ok = (a[0].returncode == b[0].returncode == 0 and ids[0] != ids[1] and overlap and matched and requests_match
                  and ra.get('goal_state') == 'canceled' and ra.get('cancel_accepted') is True
                  and rb.get('goal_state') == 'succeeded' and rb['result_response']['result'] == {'completed': 30}
                  and len(cancellations) == 1 and cancellations[0]['uuid'] == ids[0]
                  and all(1 <= f['current'] <= 30 for f in rb['feedback']))
            suite.record('concurrent-goals-cancel-isolation', ok, [a[1], b[1], server[1]], rmw, rmw,
                         actual=[ra, rb], server_events=events, overlapping_execution=overlap,
                         evidence=[str(p.relative_to(support.RAW)) for p in a[3]+b[3]+[server[0],server[2]]])
        with ExitStack() as stack:
            old = stack.enter_context(suite.server('count', rmw, 'hang'))
            client = launch(suite, stack, bundle, rmw, 'restart-pending', 80, timeout=5000)
            wait_executing(suite, old)
            support.stop(old[3])
            with suite.server('count', rmw, 'normal') as new:
                result = finish(client)
                old_events = suite.events(old); new_events = suite.events(new)
                ok = (client[0].returncode == 1 and result.get('error', {}).get('code') == 'action_timeout'
                      and len([e for e in old_events if e['event'] == 'goal']) == 1
                      and not any(e['event'] in ['goal', 'cancel'] for e in new_events))
                suite.record('server-restart-no-goal-replay', ok, [client[1], old[1], new[1]], rmw, rmw,
                             actual=result, old_server_events=old_events, new_server_events=new_events,
                             evidence=[str(p.relative_to(support.RAW)) for p in client[3]+[old[0],old[2],new[2]]])
                suite.call('explicit-goal-after-server-restart', bundle, 'count', {'target': 3}, rmw, rmw,
                           server=new, state='succeeded', check=lambda r,e:r['result_response']=={'status':4,'result':{'completed':3}})
    return 0 if all(r['outcome']=='passed' for r in suite.records) else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', required=True)
    parser.add_argument('--distro', choices=['humble','jazzy','lyrical'], default='jazzy')
    parser.add_argument('--runtime-bundle', type=Path)
    args = parser.parse_args()
    if not args.label.replace('-', '').isalnum(): parser.error('Use an alphanumeric label')
    support.DISTRO=args.distro
    support.WORKSPACE=support.ROOT if args.distro=='jazzy' else support.ROOT/'environments'/args.distro
    support.RUNTIME=support.WORKSPACE/'.pixi/envs/default'
    if Path(sys.prefix).resolve()!=support.RUNTIME.resolve() or os.environ.get('ROS_DISTRO')!=args.distro:
        parser.error('Use matching isolated project Python and ROS_DISTRO')
    support.INSTALL_SUFFIX='' if args.distro=='jazzy' else '-'+args.distro
    support.BIN=(support.ROOT/'target' if args.distro=='jazzy' else support.ROOT/'target'/args.distro)/'debug'
    if args.runtime_bundle:
        support.PACKAGED_RUNTIME=args.runtime_bundle.resolve()
        if json.loads((support.PACKAGED_RUNTIME/'manifest.json').read_text()).get('distro')!=args.distro:
            parser.error('Runtime/fixture distro mismatch')
        support.BIN=support.PACKAGED_RUNTIME/'bin'
    support.PROBE=support.BIN/'studio-action-probe'
    raise SystemExit(run(args.label))
