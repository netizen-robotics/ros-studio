"""Native dynamic-service acceptance. Only synthetic localhost/domain-187 endpoints.
Run through scripts/dev; all detailed evidence is ignored under artifacts/feasibility.
"""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
RAW = ROOT / 'artifacts/feasibility'
PROBE = ROOT / 'target/debug/studio-service-probe'
DESKTOP = ROOT / 'target/debug/studio-desktop'
DISTRO = 'jazzy'
WORKSPACE = ROOT
RUNTIME = ROOT / '.pixi/envs/default'
INSTALL_SUFFIX = ''
SERVER_STARTUP_TIMEOUT = 15
PACKAGED_RUNTIME = None
RMWS = ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']
INSPECT = {'sample': {'sequence': 18446744073709551615, 'nested': {'label': 'hello\n机器人', 'value': 3.25}, 'fixed': [1.0, 2.0, 3.0], 'bounded': [-2147483648, 2147483647], 'labels': ['one', 'two'], 'payload': [0, 255]}}
REFLECT = {'enabled': True, 'small': -128, 'large': 18446744073709551615, 'weight': 1.5, 'title': 'runtime', 'values': [-32768, 32767], 'flags': [True, False], 'tags': ['one', 'two']}
REFLECT_RESPONSE = {**{k: v for k, v in REFLECT.items() if k != 'title'}, 'summary': {'label': 'runtime', 'value': 1.5}, 'entries': [{'label': 'one', 'value': 0.0}, {'label': 'two', 'value': 1.0}]}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def stop(process):
    if process.poll() is None:
        process.terminate()
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=2)


class Suite:
    def __init__(self, label):
        self.out = RAW / 'evidence' / ('services-' + label)
        self.out.mkdir(parents=True, exist_ok=False)
        self.result = RAW / ('services-' + label + '.json')
        self.records = []
        packages = [json.loads(p.read_text()) for p in sorted((RUNTIME / 'conda-meta').glob('*.json'))]
        (self.out / 'dependencies.json').write_text(json.dumps(packages, indent=2))
        self.base = dict(os.environ, ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1', ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST', RCUTILS_LOGGING_USE_STDOUT='0', ROS_LOG_DIR=str(self.out / 'ros-logs'))
        for key in ['ROS_STATIC_PEERS', 'CYCLONEDDS_URI', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_DEFAULT_PROFILES_FILE', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT']:
            self.base.pop(key, None)
        self.base['AMENT_PREFIX_PATH'] = str(RUNTIME)
        self.base['DYLD_LIBRARY_PATH'] = str(RUNTIME / 'lib')
        self.base['CMAKE_PREFIX_PATH'] = str(RUNTIME)
        self.base['CARGO_HOME'] = str(ROOT / '.cache' / 'cargo')
        self.base['LIBCLANG_PATH'] = str(RUNTIME / 'lib')
        if DISTRO != 'jazzy':
            self.base['CARGO_TARGET_DIR'] = str(ROOT / 'target' / DISTRO)
        self.build_command = ['cargo', 'build', '--locked', '--workspace']
        with (self.out / 'build.log').open('w') as log:
            subprocess.run(self.build_command, cwd=ROOT, env=self.base, stdout=log, stderr=log, check=True)
        self.hash_before = digest(PROBE)
        self.desktop_before = digest(DESKTOP)
        self.counter = 0

    def client_environment(self):
        if PACKAGED_RUNTIME is None:
            return self.base
        home = self.out / 'client-home'
        home.mkdir(exist_ok=True)
        return dict(PATH='/usr/bin:/bin', HOME=str(home), ROS_DISTRO=DISTRO,
                    ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1',
                    ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST', RCUTILS_LOGGING_USE_STDOUT='0',
                    AMENT_PREFIX_PATH=str(PACKAGED_RUNTIME), ROS_LOG_DIR=str(home/'logs'))

    def record(self, name, passed, command, rmw=None, peer=None, **details):
        record = dict(test=name, outcome='passed' if passed else 'failed', os=platform.platform(), cpu=platform.machine(), distro=DISTRO, middleware_pair=[rmw, peer], command=command,
                      versions={'probe_sha256': digest(PROBE), 'desktop_sha256': digest(DESKTOP), 'cargo_lock_sha256': digest(ROOT / 'Cargo.lock'), 'pixi_lock_sha256': digest(WORKSPACE / 'pixi.lock'), 'native_packages': str((self.out / 'dependencies.json').relative_to(RAW))}, **details)
        self.records.append(record)
        if PACKAGED_RUNTIME is not None:
            record['runtime_manifest_sha256'] = digest(PACKAGED_RUNTIME / 'manifest.json')
            record['sanitized_client_environment'] = self.client_environment()
            record['scope'] = 'Packaged native client on development host; project Python fixture server. Not clean-machine evidence.'
        self.result.write_text(json.dumps(self.records, indent=2) + '\n')
        print(name, record['outcome'], flush=True)

    def build_bundle(self, package):
        destination = self.out / package
        command = [sys.executable, 'scripts/build-service-bundle.py', '--distro', DISTRO, '--package', package, '--destination', str(destination)]
        with (self.out / (package + '-build.log')).open('w') as log:
            subprocess.run(command, env=self.base, stdout=log, stderr=log, check=True)
        return destination

    @contextmanager
    def server(self, kind, rmw, mode='normal'):
        self.counter += 1
        stem = self.out / f'server-{self.counter}-{kind}-{mode}-{rmw}'
        request_log = stem.with_suffix('.requests.jsonl')
        console = stem.with_suffix('.log')
        package = 'feasibility_interfaces' if kind == 'inspect' else 'service_extra_interfaces'
        install = ROOT / 'artifacts' / (package + '-service-install' + INSTALL_SUFFIX)
        env = dict(self.base, RMW_IMPLEMENTATION=rmw,
                   PYTHONPATH=str(install / f'lib/python{sys.version_info.major}.{sys.version_info.minor}/site-packages') + os.pathsep + os.environ.get('PYTHONPATH', ''),
                   AMENT_PREFIX_PATH=str(install) + os.pathsep + self.base['AMENT_PREFIX_PATH'],
                   DYLD_LIBRARY_PATH=str(install / 'lib') + os.pathsep + self.base['DYLD_LIBRARY_PATH'])
        command = [sys.executable, 'scripts/service-fixture-server.py', '--kind', kind, '--mode', mode, '--log', str(request_log)]
        with console.open('w') as log:
            process = subprocess.Popen(command, env=env, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + SERVER_STARTUP_TIMEOUT
                while 'READY' not in console.read_text():
                    if process.poll() is not None or time.monotonic() >= deadline:
                        self.record('fixture-server-startup', False, command, rmw, rmw,
                                    startup_timeout_seconds=SERVER_STARTUP_TIMEOUT,
                                    evidence=[str(console.relative_to(RAW))])
                        raise RuntimeError(f'Fixture server did not start: {console}')
                    time.sleep(.05)
                yield request_log, command, console
            finally:
                stop(process)

    def call(self, name, bundle, kind, request, rmw, peer=None, expected=None, error=None, server=None, timeout=10000, typ=None, domain='187', service=None, server_expected=None):
        self.counter += 1
        stem = self.out / f'{self.counter}-{name}-{rmw}'
        input_file = stem.with_suffix('.request.json')
        input_file.write_text(request if isinstance(request, str) else json.dumps(request))
        command = [str(PROBE), '--bundle', str(bundle), '--service', service or '/studio_fixture/' + kind,
                   '--type', typ or ('feasibility_interfaces/srv/Inspect' if kind == 'inspect' else 'service_extra_interfaces/srv/Reflect'), '--request-file', str(input_file), '--timeout-ms', str(timeout)]
        env = dict(self.client_environment(), RMW_IMPLEMENTATION=rmw, ROS_DOMAIN_ID=domain)
        existing = len(self.requests(server[0])) if server else 0
        start = time.monotonic()
        output = subprocess.run(command, env=env, capture_output=True, text=True, timeout=timeout/1000+10)
        stem.with_suffix('.stdout').write_text(output.stdout)
        stem.with_suffix('.stderr').write_text(output.stderr)
        try:
            data = json.loads(output.stdout)
        except json.JSONDecodeError:
            data = {'error': {'code': 'invalid_output'}, 'raw': output.stdout}
        if error:
            good = output.returncode == 1 and data.get('error', {}).get('code') == error
            if error not in ['response_timeout', 'response_decode', 'output_limit']:
                good = good and data.get('request_state') == 'not_sent'
        else:
            good = output.returncode == 0 and data.get('response') == expected and data.get('request_state') == 'response_received'
        requests = self.requests(server[0])[existing:] if server else []
        if server:
            if error and error not in ['response_timeout', 'response_decode', 'output_limit']:
                good = good and not requests
            else:
                good = good and len(requests) == 1
                if not error:
                    good = good and requests[0] == (request if server_expected is None else server_expected)
        self.record(name, good, command, rmw, peer, actual=data, exit_code=output.returncode, seconds=round(time.monotonic()-start,3), received_requests=requests,
                        environment={'ROS_DOMAIN_ID': domain, 'ROS_LOCALHOST_ONLY': '1', 'RMW_IMPLEMENTATION': rmw, 'AMENT_PREFIX_PATH': env['AMENT_PREFIX_PATH'], 'DYLD_LIBRARY_PATH': env.get('DYLD_LIBRARY_PATH')},
                    server_command=server[1] if server else None, evidence=[str(p.relative_to(RAW)) for p in [input_file, stem.with_suffix('.stdout'), stem.with_suffix('.stderr')]] + ([str(server[0].relative_to(RAW)), str(server[2].relative_to(RAW))] if server else []))
        return data

    @staticmethod
    def requests(path):
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def corrupt(self, bundle, label, edit, rehash=False):
        copy = self.out / label
        shutil.copytree(bundle, copy)
        edit(copy)
        if rehash:
            manifest = json.loads((copy / 'manifest.json').read_text())
            manifest['files'] = {str(p.relative_to(copy)): digest(p) for p in sorted(copy.rglob('*')) if p.is_file() and p.name != 'manifest.json'}
            (copy / 'manifest.json').write_text(json.dumps(manifest))
        return copy

    def kill_restart(self, bundle, rmw):
        with self.server('inspect', rmw, 'delay') as server:
            request = self.out / f'kill-{rmw}.json'
            request.write_text(json.dumps(INSPECT))
            command = [str(PROBE), '--bundle', str(bundle), '--service', '/studio_fixture/inspect', '--type', 'feasibility_interfaces/srv/Inspect', '--request-file', str(request)]
            env = dict(self.client_environment(), RMW_IMPLEMENTATION=rmw)
            with (self.out / f'kill-{rmw}.stdout').open('w') as out, (self.out / f'kill-{rmw}.stderr').open('w') as err:
                process = subprocess.Popen(command, env=env, stdout=out, stderr=err)
                try:
                    deadline = time.monotonic() + 8
                    while not self.requests(server[0]) and process.poll() is None and time.monotonic() < deadline:
                        time.sleep(.05)
                    stop(process)
                finally:
                    stop(process)
            # New process has no persisted pending command; no new explicit request is supplied.
            restarted = subprocess.run([str(PROBE)], env=env, capture_output=True, text=True, timeout=5)
            data = json.loads(restarted.stdout)
            (self.out / f'restart-{rmw}.stdout').write_text(restarted.stdout)
            time.sleep(.3)
            requests = self.requests(server[0])
            self.record('kill-restart-no-replay', len(requests)==1 and data.get('request_state')=='not_sent' and restarted.returncode==1,
                        [command, [str(PROBE)]], rmw, rmw, received_requests=requests, actual=data, server_command=server[1],
                        evidence=[str(p.relative_to(RAW)) for p in [request,server[0],server[2],self.out/f'kill-{rmw}.stdout',self.out/f'kill-{rmw}.stderr',self.out/f'restart-{rmw}.stdout']])


def run(label):
    suite = Suite(label)
    inspect = suite.build_bundle('feasibility_interfaces')
    for rmw in RMWS:
        with suite.server('inspect', rmw) as server:
            suite.call('inspect-exact', inspect, 'inspect', INSPECT, rmw, rmw, expected={'accepted': True, 'reason': 'validated'}, server=server)
            suite.call('omitted-defaults', inspect, 'inspect', {}, rmw, rmw, expected={'accepted': True, 'reason': 'validated'}, server=server, server_expected={'sample': {'sequence':0,'nested':{'label':'','value':0.0},'fixed':[0.0,0.0,0.0],'bounded':[],'labels':[],'payload':[]}})
    # The second package is first built/imported only after the executable and first calls.
    extra = suite.build_bundle('service_extra_interfaces')
    suite.record('second-package-no-rebuild', digest(PROBE)==suite.hash_before and digest(DESKTOP)==suite.desktop_before,
                 [suite.build_command, [sys.executable,'scripts/build-service-bundle.py','--distro',DISTRO,'--package','service_extra_interfaces','--destination',str(extra)]],
                 probe_hash_before=suite.hash_before, probe_hash_after=digest(PROBE), desktop_hash_before=suite.desktop_before, desktop_hash_after=digest(DESKTOP),
                 evidence=[str((suite.out/'service_extra_interfaces-build.log').relative_to(RAW))])
    relocated = suite.out / 'relocated interface bundle with spaces'
    shutil.move(str(extra), relocated)
    extra = relocated
    for rmw in RMWS:
        with suite.server('reflect', rmw) as server:
            suite.call('second-service-relocated', extra, 'reflect', REFLECT, rmw, rmw, expected=REFLECT_RESPONSE, server=server)
            for name, changes in [('signed-overflow', {'small':128}),('unsigned-overflow',{'large':18446744073709551616}),('float-overflow',{'weight':1e39}),('string-bound',{'title':'x'*13}),('sequence-bound',{'values':[0]*5}),('fixed-array-bound',{'flags':[True]})]:
                suite.call(name, extra, 'reflect', {**REFLECT,**changes}, rmw, rmw, error='validation',server=server)
        with suite.server('inspect', rmw) as server:
            for name, request, error in [
                ('invalid-json','{','input'),('input-limit',' '*1048577,'input'),
                ('unknown-field',{'unexpected':1},'validation'),
                ('nested-unknown',{'sample':{'nested':{'unknown':1}}},'validation'),
                ('bounded-array',{'sample':{'bounded':[1]*9}},'validation'),
                ('excessive-sequence',{'sample':{'payload':[0]*65537}},'validation'),
            ]:
                suite.call(name,inspect,'inspect',request,rmw,rmw,error=error,server=server)
            suite.call('wrong-domain',inspect,'inspect',INSPECT,rmw,rmw,error='fixture_guard',server=server,domain='57')
            suite.call('wrong-service-namespace',inspect,'inspect',INSPECT,rmw,rmw,error='fixture_guard',server=server,service='/robot/control')
        with suite.server('inspect',rmw,'reject') as server:
            suite.call('application-rejection',inspect,'inspect',INSPECT,rmw,rmw,expected={'accepted':False,'reason':'fixture rejection'},server=server)
        with suite.server('inspect',rmw,'delay') as server:
            suite.call('delayed-response-at-most-once',inspect,'inspect',INSPECT,rmw,rmw,error='response_timeout',server=server,timeout=3000)
            time.sleep(.3)
            suite.record('timeout-no-retry',len(suite.requests(server[0]))==1,server[1],rmw,rmw,evidence=[str(server[0].relative_to(RAW))])
        with suite.server('inspect',rmw,'large') as server:
            suite.call('response-size-limit',inspect,'inspect',INSPECT,rmw,rmw,error='response_decode',server=server)
        suite.call('server-unavailable',inspect,'inspect',INSPECT,rmw,error='server_unavailable',timeout=500,service='/studio_fixture/absent')
        suite.kill_restart(inspect,rmw)
    rmw=RMWS[1]
    for key,value in [('ros_distro','jazzy' if DISTRO != 'jazzy' else 'humble'),('os','windows'),('arch','x86_64')]:
        def change(path,key=key,value=value):
            manifest=json.loads((path/'manifest.json').read_text());manifest[key]=value;(path/'manifest.json').write_text(json.dumps(manifest))
        bad=suite.corrupt(inspect,'wrong-'+key,change)
        suite.call('bundle-'+key,bad,'inspect',INSPECT,rmw,error='bundle')
    lib='lib/libfeasibility_interfaces__rosidl_typesupport_introspection_c.dylib'
    bad=suite.corrupt(inspect,'checksum-mismatch',lambda p:(p/lib).write_bytes(b'corrupt'))
    suite.call('checksum-mismatch',bad,'inspect',INSPECT,rmw,error='bundle')
    bad=suite.corrupt(inspect,'missing-file',lambda p:(p/lib).unlink())
    suite.call('missing-file',bad,'inspect',INSPECT,rmw,error='bundle')
    bad=suite.corrupt(inspect,'missing-type-support',lambda p:(p/lib).unlink(),True)
    suite.call('missing-type-support',bad,'inspect',INSPECT,rmw,error='type_support')
    bad=suite.corrupt(inspect,'unloadable-type-support',lambda p:(p/lib).write_bytes(b'not a native library'),True)
    suite.call('unloadable-type-support',bad,'inspect',INSPECT,rmw,error='type_support')
    suite.call('missing-service-symbol',inspect,'inspect',INSPECT,rmw,error='type_support',typ='feasibility_interfaces/srv/Absent')
    suite.call('unsupported-field-kind',extra,'reflect',{},rmw,error='type_support',typ='service_extra_interfaces/srv/Wide')
    for local,peer in [(RMWS[0],RMWS[1]),(RMWS[1],RMWS[0])]:
        with suite.server('inspect',peer) as server:
            suite.call('mixed-middleware-exploratory',inspect,'inspect',INSPECT,local,peer,expected={'accepted':True,'reason':'validated'},server=server,timeout=5000)
    return 0 if all(r['outcome']=='passed' for r in suite.records if r['test']!='mixed-middleware-exploratory') else 1


if __name__ == '__main__':
    parser=argparse.ArgumentParser()
    parser.add_argument('--distro', choices=['humble','jazzy','lyrical'], default='jazzy')
    parser.add_argument('--server-startup-timeout', type=float, default=15)
    parser.add_argument('--runtime-bundle', type=Path, help='Use packaged probes with a sanitized client environment')
    parser.add_argument('--label',default=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ'))
    args=parser.parse_args()
    if not args.label.replace('-','').isalnum():parser.error('Use an alphanumeric label')
    DISTRO = args.distro
    if not 0 < args.server_startup_timeout <= 120:
        parser.error('Server startup timeout must be between 0 and 120 seconds')
    SERVER_STARTUP_TIMEOUT = args.server_startup_timeout
    WORKSPACE = ROOT if DISTRO == 'jazzy' else ROOT / 'environments' / DISTRO
    RUNTIME = WORKSPACE / '.pixi/envs/default'
    if Path(sys.prefix).resolve() != RUNTIME.resolve() or os.environ.get('ROS_DISTRO') != DISTRO:
        parser.error('Run with the selected distro project Python and ROS_DISTRO')
    if DISTRO != 'jazzy':
        INSTALL_SUFFIX = '-' + DISTRO
        PROBE = ROOT / 'target' / DISTRO / 'debug/studio-service-probe'
        DESKTOP = ROOT / 'target' / DISTRO / 'debug/studio-desktop'
    if args.runtime_bundle:
        PACKAGED_RUNTIME = args.runtime_bundle.resolve()
        manifest = json.loads((PACKAGED_RUNTIME/'manifest.json').read_text())
        if manifest.get('distro') != DISTRO:
            parser.error('Packaged runtime distro must match the selected fixture distro')
        PROBE = PACKAGED_RUNTIME/'bin/studio-service-probe'
        DESKTOP = PACKAGED_RUNTIME/'bin/studio-desktop'
    raise SystemExit(run(args.label))
