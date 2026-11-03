"""Compare fresh-path/repeat packaged startup, optionally including ROS initialization."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess
import threading
import time

ROOT=Path(__file__).resolve().parents[1]
RAW=ROOT/'artifacts/feasibility'
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--runtime',type=Path,required=True)
parser.add_argument('--label',required=True)
parser.add_argument('--sample-startup',action='store_true',help='Sample only fresh-path service processes with macOS sample; may perturb timings')
parser.add_argument('--trace',action='store_true',help='Collect opt-in service startup phase timestamps')
parser.add_argument('--interface-bundle',type=Path,help='Also initialize ROS and wait 500ms for an absent fixture endpoint')
args=parser.parse_args()
if not args.label.replace('-','').isalnum():parser.error('Use alphanumeric label')
if args.sample_startup and platform.system()!='Darwin':parser.error('Startup sampling requires macOS')
manifest=json.loads((args.runtime/'manifest.json').read_text());distro=manifest.get('distro')
if distro not in ['humble','jazzy','lyrical']:parser.error('Unsupported runtime distro')
workspace=ROOT if distro=='jazzy' else ROOT/'environments'/distro
if args.interface_bundle and json.loads((args.interface_bundle/'manifest.json').read_text()).get('ros_distro')!=distro:parser.error('Interface/runtime distro mismatch')
out=RAW/'evidence'/('probe-startup-'+args.label);out.mkdir(parents=True,exist_ok=False)
rows=[]
for copy_index in range(2):
    runtime=out/f'fresh runtime {copy_index}'
    shutil.copytree(args.runtime,runtime)
    home=out/f'home-{copy_index}';home.mkdir()
    env={'PATH':'/usr/bin:/bin','HOME':str(home),'DYLD_PRINT_STATISTICS':'1'}
    if args.trace:env['STUDIO_STARTUP_TRACE']='1'
    if args.interface_bundle:
        interface=out/f'fresh interface {copy_index}';shutil.copytree(args.interface_bundle,interface)
        env.update(ROS_DISTRO=distro,ROS_DOMAIN_ID='187',ROS_LOCALHOST_ONLY='1',AMENT_PREFIX_PATH=str(runtime),ROS_LOG_DIR=str(home/'logs'))
    for name in ['studio-service-probe','studio-action-probe']:
        binary=runtime/'bin'/name
        pairs=[(rmw,attempt) for rmw in (['rmw_fastrtps_cpp','rmw_cyclonedds_cpp'] if args.interface_bundle else [None]) for attempt in ['first','repeat']]
        for rmw,attempt in pairs:
            stem=out/f'{copy_index}-{name}-{rmw}-{attempt}'
            cmd=[str(binary)]
            expected='arguments';run_env=dict(env)
            if args.interface_bundle:
                service=name=='studio-service-probe'
                request=stem.with_suffix('.request.json');request.write_text('{}' if service else '{"target":3}')
                cmd += ['--bundle',str(interface),'--service' if service else '--action','/studio_fixture/absent_startup_probe',
                        '--type','feasibility_interfaces/srv/Inspect' if service else 'feasibility_interfaces/action/Count',
                        '--request-file' if service else '--goal-file',str(request),'--timeout-ms','500']
                run_env['RMW_IMPLEMENTATION']=rmw;expected='server_unavailable'
            start_wall_ms=time.time()*1000;start=time.monotonic();timed_out=False
            sample_record=None
            with subprocess.Popen(cmd,env=run_env,stdout=subprocess.PIPE,stderr=subprocess.PIPE) as process:
                sampler=None
                if args.sample_startup and name=='studio-service-probe' and attempt=='first':
                    sample_path=stem.with_suffix('.sample.txt')
                    sample_cmd=['/usr/bin/sample',str(process.pid),'1','-mayDie','-file',str(sample_path)]
                    sample_record={'command':sample_cmd,'status':'pending'}
                    def sample_startup():
                        time.sleep(.25)
                        if process.poll() is not None:
                            sample_record['status']='process_finished_before_sample'
                            return
                        try:
                            sampled=subprocess.run(sample_cmd,capture_output=True,timeout=10)
                            sample_record.update(status='captured' if sampled.returncode==0 and sample_path.exists() else 'failed',exit_code=sampled.returncode)
                            stem.with_suffix('.sample-command.log').write_bytes(sampled.stdout+sampled.stderr)
                        except subprocess.TimeoutExpired:
                            sample_record['status']='sample_timeout'
                    sampler=threading.Thread(target=sample_startup)
                    sampler.start()
                try:
                    stdout,stderr=process.communicate(timeout=40)
                    code=process.returncode
                except subprocess.TimeoutExpired:
                    process.kill();stdout,stderr=process.communicate();code=None;timed_out=True
                seconds=time.monotonic()-start
                if sampler:sampler.join()
            stem.with_suffix('.stdout').write_bytes(stdout);stem.with_suffix('.stderr').write_bytes(stderr)
            try: data=json.loads(stdout)
            except (ValueError,UnicodeDecodeError):data={}
            traces=[]
            for line in stderr.decode(errors='replace').splitlines():
                try:
                    item=json.loads(line)
                    if 'startup_trace' in item:traces.append(item['startup_trace'])
                except (ValueError,TypeError):pass
            phases=[{'from':a['stage'],'to':b['stage'],'milliseconds':round(b['elapsed_ms']-a['elapsed_ms'],3)} for a,b in zip(traces,traces[1:])]
            required_stages=(['main','bundle_validation_start','bundle_validation_end','type_support_start','type_support_end','connect_start','connect_end','run_finished'] if args.interface_bundle else ['main','run_finished'])
            trace_valid=(not args.trace or name!='studio-service-probe' or
                         ([t['stage'] for t in traces]==required_stages and all(p['milliseconds']>=0 for p in phases)))
            valid=code==1 and data.get('error',{}).get('code')==expected and (data.get('request_state')=='not_sent' or data.get('goal_state')=='not_sent')
            rows.append(dict(test=f'{copy_index}-{name}-{rmw}-{attempt}',outcome='passed' if valid and trace_valid else 'failed',
                             os=platform.platform(),cpu=platform.machine(),distro=distro,middleware_pair=[rmw,None],
                             versions={'executable_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'Cargo.lock':hashlib.sha256((ROOT/'Cargo.lock').read_bytes()).hexdigest(),'pixi.lock':hashlib.sha256((workspace/'pixi.lock').read_bytes()).hexdigest()},
                             command=cmd,environment=run_env,seconds=round(seconds,3),slow_start=seconds>20,comparison_threshold_seconds=20,expected_error=expected,expected_error_observed=valid,
                             startup_sample=sample_record,harness_timeout=timed_out,exit_code=code,trace_valid=trace_valid,probe_build_profile=manifest.get('probe_build_profile'),startup_trace=traces,phase_intervals=phases,spawn_to_main_upper_bound_ms=(round(traces[0]['unix_ms']-start_wall_ms,3) if traces else None),evidence=[str(stem.with_suffix(s).relative_to(RAW)) for s in ['.stdout','.stderr','.sample.txt','.sample-command.log'] if stem.with_suffix(s).exists()],
                             scope=('Sampling enabled; timings may be perturbed. ' if args.sample_startup else '')+'Fresh path copy on development host; OS caches not flushed. Optional ROS initialization uses absent localhost/domain187 fixture endpoint, no request sent. Not clean-host evidence. Twenty-second threshold is diagnostic only: user accepts slow usable startup. Forty-second process watchdog remains bounded; operational requests keep their own deadlines.'))
            (RAW/('probe-startup-'+args.label+'.json')).write_text(json.dumps(rows,indent=2)+'\n')
            print(rows[-1]['test'],rows[-1]['outcome'],rows[-1]['seconds'],flush=True)
raise SystemExit(0 if all(r['outcome']=='passed' for r in rows) else 1)
