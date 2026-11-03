"""Exercise real ROS reader backpressure and explicit recovery on local fixtures."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import statistics
import sys
import threading
import time

ROOT=Path(__file__).resolve().parents[1]
RAW=ROOT/'artifacts/feasibility'
RUNTIME=ROOT/'environments/lyrical/.pixi/envs/default'


def peer(duration):
    if os.environ.get('ROS_DOMAIN_ID')!='187' or os.environ.get('ROS_LOCALHOST_ONLY')!='1':
        raise SystemExit('Fixture requires localhost/domain 187')
    import rclpy
    from std_msgs.msg import String
    rclpy.init();node=rclpy.create_node('studio_overload_fixture')
    pub=node.create_publisher(String,'/studio_fixture/flood',10)
    deadline=time.monotonic()+15
    while pub.get_subscription_count()==0:
        if time.monotonic()>deadline:raise RuntimeError('No matched native reader')
        rclpy.spin_once(node,timeout_sec=.05)
    print('READY',flush=True)
    started=time.monotonic();count=0
    while time.monotonic()-started<duration:
        count+=1;pub.publish(String(data=f'flood:{count:08d}:'+('x'*200)))
        time.sleep(.0005)
    for i in range(100):
        pub.publish(String(data=f'recovered:{i:03d}'))
        rclpy.spin_once(node,timeout_sec=.02)
    print(json.dumps({'flood_published':count,'recovery_published':100}),flush=True)
    node.destroy_node();rclpy.try_shutdown()


def stop(p):
    if p.poll() is None:p.terminate()
    try:p.wait(timeout=3)
    except subprocess.TimeoutExpired:p.kill();p.wait()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--peer',action='store_true');parser.add_argument('--label')
    parser.add_argument('--duration-seconds',type=int,default=8)
    parser.add_argument('--measure-rss',action='store_true')
    args=parser.parse_args()
    if not 8<=args.duration_seconds<=300:parser.error('Duration must be 8..300 seconds')
    if args.measure_rss and (platform.system()!='Darwin' or args.duration_seconds<30):parser.error('RSS measurement requires macOS and at least 30 seconds')
    if args.peer:return peer(args.duration_seconds)
    if not args.label or not args.label.replace('-','').isalnum():parser.error('Unique alphanumeric label required')
    if Path(sys.prefix).resolve()!=RUNTIME.resolve() or os.environ.get('ROS_DISTRO')!='lyrical':parser.error('Use isolated Lyrical Python/runtime')
    out=RAW/'evidence'/('native-reader-overload-'+args.label);out.mkdir(parents=True,exist_ok=False)
    backend=ROOT/'target/lyrical/debug/studio-backend'
    digest=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
    versions={'backend_sha256':digest(backend),'pixi_lock_sha256':digest(ROOT/'environments/lyrical/pixi.lock'),'cargo_lock_sha256':digest(ROOT/'Cargo.lock'),'harness_sha256':digest(Path(__file__))}
    rows=[]
    for rmw in ['rmw_fastrtps_cpp','rmw_cyclonedds_cpp']:
        env=dict(os.environ,ROS_DISTRO='lyrical',ROS_DOMAIN_ID='187',ROS_LOCALHOST_ONLY='1',ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST',RMW_IMPLEMENTATION=rmw,AMENT_PREFIX_PATH=str(RUNTIME),DYLD_LIBRARY_PATH=str(RUNTIME/'lib'),ROS_LOG_DIR=str(out/'ros-logs'),RCUTILS_LOGGING_USE_STDOUT='0',SKIP_DEFAULT_XML='1')
        for key in ['ROS_STATIC_PEERS','ROS_DISCOVERY_SERVER','ROS_SUPER_CLIENT','CYCLONEDDS_URI','FASTRTPS_DEFAULT_PROFILES_FILE','FASTDDS_DEFAULT_PROFILES_FILE','FASTDDS_ENVIRONMENT_FILE']:env.pop(key,None)
        cmd=[str(backend),'/studio_fixture/flood','std_msgs/msg/String']
        peer_cmd=[sys.executable,str(Path(__file__).resolve()),'--peer','--duration-seconds',str(args.duration_seconds)]
        paths=[out/(rmw+suffix) for suffix in ['.backend.stdout','.backend.stderr','.peer.log']]
        stats={'samples':0,'max_drops':0,'drop_counter_monotonic':True,'recovery_samples':0,'parse_errors':0,'errors':[],'hello':False}
        def read(pipe):
            with paths[0].open('w') as log:
                for line in pipe:
                    log.write(line)
                    try:
                        e=json.loads(line)
                        if e.get('event')=='hello':stats['hello']=e.get('version')==1 and e.get('distro')=='lyrical'
                        if e.get('event')=='error':stats['errors'].append(e)
                        if e.get('event')!='sample':continue
                        fields=dict(e['fields']);drops=int(fields['[samples dropped]'])
                        stats['drop_counter_monotonic'] &= drops>=stats['max_drops']
                        stats['max_drops']=max(drops,stats['max_drops']);stats['samples']+=1
                        if 'recovered:' in fields['data']:stats['recovery_samples']+=1
                    except (ValueError,KeyError,TypeError):stats['parse_errors']+=1
        memory=[];memory_errors=[];sampler=None;sampling_done=threading.Event()
        def sample_memory(pid):
            started=time.monotonic()
            while not sampling_done.is_set():
                try:
                    result=subprocess.run(['/bin/ps','-o','rss=','-p',str(pid)],capture_output=True,text=True,timeout=2)
                    if result.returncode:raise RuntimeError(result.stderr.strip() or 'ps returned no process')
                    memory.append({'elapsed_seconds':round(time.monotonic()-started,3),'rss_kib':int(result.stdout.strip())})
                except Exception as exc:
                    memory_errors.append(str(exc));return
                sampling_done.wait(1)
        error=None;reader=None;b=None;p=None
        with paths[1].open('w') as err,paths[2].open('w') as peer_log:
            try:
                b=subprocess.Popen(cmd,env=env,stdout=subprocess.PIPE,stderr=err,text=True)
                p=subprocess.Popen(peer_cmd,env=env,stdout=peer_log,stderr=peer_log)
                deadline=time.monotonic()+20
                while 'READY' not in paths[2].read_text():
                    if b.poll() is not None or p.poll() is not None or time.monotonic()>deadline:raise RuntimeError('Fixture did not become ready')
                    time.sleep(.05)
                if args.measure_rss:
                    sampler=threading.Thread(target=sample_memory,args=(b.pid,),daemon=True);sampler.start()
                # Intentionally fill the OS pipe and bounded application channel.
                time.sleep(3)
                reader=threading.Thread(target=read,args=(b.stdout,),daemon=True);reader.start()
                p.wait(timeout=args.duration_seconds+15)
                deadline=time.monotonic()+3
                while stats['recovery_samples']==0 and time.monotonic()<deadline:time.sleep(.05)
                stats['peer_exit']=p.returncode;stats['backend_alive']=b.poll() is None
            except Exception as exc:error=str(exc)
            finally:
                sampling_done.set()
                if sampler is not None:sampler.join(timeout=3)
                if p is not None:stop(p)
                if b is not None:stop(b)
                if reader is not None:reader.join(timeout=3)
        ok=not error and stats['hello'] and stats.get('peer_exit')==0 and stats.get('backend_alive') and stats['max_drops']>0 and stats['drop_counter_monotonic'] and stats['recovery_samples']==100 and not stats['parse_errors'] and not stats['errors']
        rows.append(dict(test='native-reader-backpressure-recovery',outcome='passed' if ok else 'failed',os=platform.platform(),cpu=platform.machine(),distro='lyrical',middleware_pair=[rmw,rmw],versions=versions,command=[cmd,peer_cmd],harness_command=[sys.executable,str(Path(__file__).resolve()),*sys.argv[1:]],environment={k:env[k] for k in ['ROS_DOMAIN_ID','ROS_LOCALHOST_ONLY','RMW_IMPLEMENTATION']},actual=stats,error=error,output_consumption_paused_seconds=3,evidence=[str(p.relative_to(RAW)) for p in paths],scope='Real native ROS String reader; application-channel loss counter and resumed delivery. Not DDS loss accounting, sensor throughput, UI timing or memory-soak qualification.'))
        if args.measure_rss:
            memory_path=out/(rmw+'.memory.json')
            memory_path.write_text(json.dumps({'samples':memory,'errors':memory_errors},indent=2)+'\n')
            early=[x['rss_kib'] for x in memory if 10<=x['elapsed_seconds']<20]
            late=[x['rss_kib'] for x in memory if x['elapsed_seconds']>=args.duration_seconds-10]
            peak=max((x['rss_kib'] for x in memory),default=0)
            growth=statistics.median(late)-statistics.median(early) if early and late else None
            measured=not memory_errors and len(memory)>=args.duration_seconds*.8 and growth is not None
            memory_ok=measured and peak<=262144 and growth<=16384
            rows.append({**rows[-1],'test':'native-reader-memory-observation','outcome':('passed' if memory_ok else 'failed') if measured else 'not tested','actual':{'sample_count':len(memory),'peak_rss_kib':peak,'late_minus_early_median_kib':growth,'measurement_errors':memory_errors},'criteria':{'duration_seconds':args.duration_seconds,'peak_rss_limit_kib':262144,'late_median_growth_limit_kib':16384},'evidence':[str(memory_path.relative_to(RAW)),*rows[-1]['evidence']],'scope':'Exploratory development-host RSS guardrails for this String fixture only; not production memory, sensor payload or native UI qualification'})
        (RAW/('native-reader-overload-'+args.label+'.json')).write_text(json.dumps(rows,indent=2)+'\n')
        print(rmw,rows[-1]['outcome'],stats,flush=True)
    return 0 if all(r['outcome']=='passed' for r in rows) else 1


if __name__=='__main__':
    raise SystemExit(main())
