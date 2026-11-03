"""Native Rust backend QoS/domain/graph checks using isolated synthetic ROS peers.
Run via scripts/dev and project Python. No robot addresses or commands.
"""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
RAW = ROOT / 'artifacts/feasibility'
TOPIC = '/studio_fixture/nested/behavior'
RMWS = ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']


def publisher(args):
    if os.environ.get('ROS_DOMAIN_ID') not in ['187', '188'] or os.environ.get('ROS_LOCALHOST_ONLY') != '1':
        raise SystemExit('Synthetic localhost domains only')
    import rclpy
    from rclpy.qos import QoSProfile, ReliabilityPolicy, DurabilityPolicy
    from std_msgs.msg import String
    rclpy.init()
    node = rclpy.create_node('behavior_fixture', namespace='/studio_fixture/nested')
    qos = QoSProfile(depth=10, reliability=ReliabilityPolicy.BEST_EFFORT if args.best_effort else ReliabilityPolicy.RELIABLE,
                     durability=DurabilityPolicy.TRANSIENT_LOCAL if args.retained else DurabilityPolicy.VOLATILE)
    pub = node.create_publisher(String, TOPIC, qos)
    count = 0
    while rclpy.ok():
        if not args.once or count == 0:
            pub.publish(String(data=f'{args.marker}:{count}'))
            print(json.dumps({'published': f'{args.marker}:{count}'}), flush=True)
            count += 1
        rclpy.spin_once(node, timeout_sec=.05)
        time.sleep(.05)


def stop(p):
    if p.poll() is None:
        p.terminate()
    try:
        p.wait(timeout=2)
    except subprocess.TimeoutExpired:
        p.kill(); p.wait(timeout=2)


def events(path):
    result = []
    for line in path.read_text().splitlines():
        try:
            result.append(json.loads(line))
        except json.JSONDecodeError:
            pass  # A final partial line is possible while the process is writing.
    return result


def wait_for(predicate, seconds=6):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(.05)
    return bool(predicate())


class Suite:
    def __init__(self, label, bundle=None, app=False, distro="jazzy"):
        self.distro=distro
        workspace=ROOT if distro=="jazzy" else ROOT/"environments"/distro
        runtime=workspace/".pixi/envs/default"
        if Path(sys.prefix).resolve()!=runtime.resolve() or os.environ.get("ROS_DISTRO")!=distro:
            raise SystemExit("Use the selected distro project Python and ROS_DISTRO")
        if bundle and distro!="jazzy":
            raise SystemExit("Non-Jazzy runtime packaging has not yet been qualified")
        self.out = RAW / 'evidence' / ('ros-behavior-' + label)
        self.out.mkdir(parents=True, exist_ok=False)
        self.report = RAW / ('ros-behavior-' + label + '.json')
        self.rows = []; self.commands = []; self.files = []; self.counter = 0
        self.env = dict(os.environ, ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1', ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST',
                        RCUTILS_LOGGING_USE_STDOUT='0', SKIP_DEFAULT_XML='1', ROS_LOG_DIR=str(self.out / 'ros-logs'))
        for key in ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI', 'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']:
            self.env.pop(key, None)
        self.env['AMENT_PREFIX_PATH']=str(runtime)
        self.env['DYLD_LIBRARY_PATH']=str(runtime/'lib')
        target=ROOT/'target' if distro=='jazzy' else ROOT/'target'/distro
        self.bundle = bundle.resolve() if bundle else None
        self.backend = self.bundle/('MacOS/studio-backend' if app else 'bin/studio-backend') if self.bundle else target/'debug/studio-backend'
        self.prefix = self.bundle/'Resources' if app else self.bundle
        self.home = self.out/'clean-home'
        self.home.mkdir()
        self.versions = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in [self.backend, ROOT/'Cargo.lock', workspace/'pixi.lock', Path(__file__).resolve()]}
        self.versions['native_packages'] = str((self.out/'dependencies.json').relative_to(RAW))
        (self.out/'dependencies.json').write_text(json.dumps([json.loads(p.read_text()) for p in sorted((runtime/'conda-meta').glob('*.json'))],indent=2))

    @contextmanager
    def process(self, rmw, kind, flags=(), domain='187'):
        self.counter += 1
        stem = self.out / f'{self.counter}-{kind}-{rmw}'
        stdout = stem.with_suffix('.stdout'); stderr = stem.with_suffix('.stderr')
        if kind == 'publisher':
            command = [sys.executable, str(Path(__file__).resolve()), '--publisher', *flags]
        else:
            command = [str(self.backend), *flags]
        env=dict(self.env, RMW_IMPLEMENTATION=rmw, ROS_DOMAIN_ID=domain)
        if kind == 'backend' and self.bundle:
            env={'PATH':'/usr/bin:/bin','HOME':str(self.home),'AMENT_PREFIX_PATH':str(self.prefix),'ROS_DISTRO':'jazzy','ROS_DOMAIN_ID':domain,'ROS_LOCALHOST_ONLY':'1','RMW_IMPLEMENTATION':rmw,'RCUTILS_LOGGING_USE_STDOUT':'0','ROS_LOG_DIR':str(self.home/'logs')}
        self.commands.append({'argv': command, 'environment': {k:v for k,v in env.items() if k in ['PATH','HOME','AMENT_PREFIX_PATH','ROS_DISTRO','ROS_DOMAIN_ID','ROS_LOCALHOST_ONLY','RMW_IMPLEMENTATION','ROS_LOG_DIR']},'sanitized_backend_environment':bool(kind=='backend' and self.bundle)})
        self.files += [str(stdout.relative_to(RAW)), str(stderr.relative_to(RAW))]
        with stdout.open('w') as out, stderr.open('w') as err:
            p = subprocess.Popen(command, env=env, stdout=out, stderr=err)
            try:
                if not wait_for(lambda: bool(events(stdout)) or p.poll() is not None):
                    self.record('process-startup-timeout',False,rmw,timeout_seconds=6,kind=kind)
                    raise RuntimeError(f'Process startup timeout: {stderr}')
                if p.poll() is not None:
                    raise RuntimeError(f'Process exited: {stderr}')
                yield p, stdout
            finally:
                stop(p)

    def record(self, name, ok, rmw, **details):
        peer = details.pop('peer', rmw)
        self.rows.append(dict(test=name, outcome='passed' if ok else 'failed', os=platform.platform(), cpu=platform.machine(), distro=self.distro, middleware_pair=[rmw,peer], versions=self.versions, command=self.commands, evidence=self.files, **details))
        self.report.write_text(json.dumps(self.rows, indent=2)+'\n')
        self.commands=[]; self.files=[]
        print(name, rmw, self.rows[-1]['outcome'], flush=True)

    @staticmethod
    def samples(path, marker):
        return [x for x in events(path) if x.get('event')=='sample' and x.get('topic')==TOPIC and marker in str(x.get('fields'))]

    @staticmethod
    def graph(path):
        graphs = [x for x in events(path) if x.get('event')=='graph']
        return graphs[-1]['topics'] if graphs else []

    def qos(self, rmw, name, pubflags, subflags, expect, peer=None):
        peer = peer or rmw
        with self.process(peer, 'publisher', [*pubflags,'--marker',name]) as (_, pub):
            with self.process(rmw, 'backend', [TOPIC,'std_msgs/msg/String',*subflags]) as (_, sub):
                visible=wait_for(lambda: any(t[0]==TOPIC for t in self.graph(sub)))
                received=wait_for(lambda: bool(self.samples(sub,name)), 4)
                samples=self.samples(sub,name)
                expected_fields={'Simple(String('+json.dumps(e['published'])+'))' for e in events(pub) if 'published' in e}
                exact=all(dict(e['fields']).get('data') in expected_fields for e in samples)
                self.record(name,visible and received==expect and exact,rmw,peer=peer,expected_delivery=expect,received_samples=samples,publisher_messages=len(events(pub)),graph_visible=visible,exact_payload_comparison=exact)

    def run(self):
        for rmw in RMWS:
            for name,pub,sub,expected in [
                ('reliable-to-reliable',[],[],True),
                ('best-effort-to-best-effort',['--best-effort'],['--best-effort'],True),
                ('reliable-to-best-effort',[],['--best-effort'],True),
                ('best-effort-to-reliable-incompatible',['--best-effort'],[],False),
                ('transient-late-join',['--retained','--once'],['--transient-local'],True),
                ('volatile-late-join',['--once'],[],False),
            ]:
                self.qos(rmw,name,pub,sub,expected)
            with self.process(rmw,'publisher',['--marker','isolation'],domain='188') as (_,pub):
                with self.process(rmw,'backend',[TOPIC,'std_msgs/msg/String']) as (_,sub):
                    leaked=wait_for(lambda: bool(self.samples(sub,'isolation')),3)
                    published=len(events(pub))
                    isolated=not leaked and published>=3
                with self.process(rmw,'backend',[TOPIC,'std_msgs/msg/String'],domain='188') as (_,control):
                    control_ok=wait_for(lambda: bool(self.samples(control,'isolation')))
                self.record('domain-isolation-with-positive-control',isolated and control_ok,rmw,published=published,isolated=isolated,matching_domain_delivery=control_ok)
            with self.process(rmw,'backend',[TOPIC,'std_msgs/msg/String']) as (backend,sub):
                with self.process(rmw,'publisher',['--marker','before-restart']):
                    first=wait_for(lambda:bool(self.samples(sub,'before-restart')))
                with self.process(rmw,'publisher',['--marker','after-restart']):
                    second=wait_for(lambda:bool(self.samples(sub,'after-restart')))
                self.record('publisher-restart-same-backend',first and second and backend.poll() is None,rmw,first_delivery=first,second_delivery=second,backend_pid=backend.pid)
            # Graph-only backend has no subscription keeping the fixture topic alive.
            with self.process(rmw,'backend') as (_,sub):
                with self.process(rmw,'publisher',['--marker','graph']):
                    appeared=wait_for(lambda:any(t[0]==TOPIC and t[1]==['std_msgs/msg/String'] for t in self.graph(sub)))
                disappeared=wait_for(lambda:all(t[0]!=TOPIC for t in self.graph(sub)),12)
                self.record('namespaced-graph-appearance-removal',appeared and disappeared,rmw,appeared=appeared,disappeared=disappeared)
        return 0 if all(x['outcome']=='passed' for x in self.rows) else 1


if __name__ == '__main__':
    parser=argparse.ArgumentParser()
    parser.add_argument('--distro',choices=['humble','jazzy','lyrical'],default='jazzy')
    parser.add_argument('--publisher',action='store_true');parser.add_argument('--best-effort',action='store_true');parser.add_argument('--retained',action='store_true');parser.add_argument('--once',action='store_true');parser.add_argument('--marker',default='fixture')
    parser.add_argument('--bundle',type=Path,help='Test this native runtime with a sanitized backend environment')
    parser.add_argument('--app',action='store_true',help='--bundle is structured app Contents, with MacOS and Resources directories')
    parser.add_argument('--label',default=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ'))
    args=parser.parse_args()
    if args.publisher:
        publisher(args)
    else:
        if not args.label.replace('-','').isalnum():parser.error('Use alphanumeric label')
        if args.app and not args.bundle: parser.error('--app requires --bundle')
        raise SystemExit(Suite(args.label,args.bundle,args.app,args.distro).run())
