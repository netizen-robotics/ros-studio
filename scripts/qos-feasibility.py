"""Native rcl history/deadline/liveliness fixture proof; localhost domain187 only."""
import argparse
from datetime import datetime,timezone
import hashlib,json,os,platform,subprocess,sys,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];RAW=ROOT/'artifacts/feasibility'


def serve(mode):
    if os.environ.get('ROS_DOMAIN_ID')!='187' or os.environ.get('ROS_LOCALHOST_ONLY')!='1':raise SystemExit('Fixture requires localhost/domain187')
    import rclpy
    from rclpy.duration import Duration
    from rclpy.qos import QoSProfile,LivelinessPolicy
    from rclpy.executors import ExternalShutdownException
    from std_msgs.msg import Int32
    rclpy.init();node=rclpy.create_node('studio_qos_server')
    qos=QoSProfile(depth=10)
    if mode=='events':
        qos.deadline=Duration(nanoseconds=200000000);qos.liveliness=LivelinessPolicy.MANUAL_BY_TOPIC;qos.liveliness_lease_duration=Duration(nanoseconds=300000000)
    pub=node.create_publisher(Int32,'/studio_fixture/qos',qos)
    print('READY',flush=True)
    try:
        deadline=time.monotonic()+10
        while pub.get_subscription_count()==0:
            if time.monotonic()>deadline:raise RuntimeError('No native subscriber matched')
            rclpy.spin_once(node,timeout_sec=.02)
        print('MATCHED',flush=True)
        if mode=='history':
            for i in range(8):pub.publish(Int32(data=i));time.sleep(.03)
            print(json.dumps({'burst':list(range(8))}),flush=True)
        else:
            pub.publish(Int32(data=1));pub.assert_liveliness();print('INITIAL_SAMPLE',flush=True)
        start=time.monotonic();recovered=False
        while rclpy.ok():
            if mode=='events' and time.monotonic()-start>1.2:
                pub.publish(Int32(data=2));pub.assert_liveliness()
                if not recovered:print('RECOVERY_SAMPLE',flush=True);recovered=True
            rclpy.spin_once(node,timeout_sec=.04);time.sleep(.04)
    except (ExternalShutdownException,KeyboardInterrupt):pass
    except Exception:
        if rclpy.ok():raise
    finally:node.destroy_node();rclpy.try_shutdown()


def run(label, distro):
    workspace=ROOT if distro=="jazzy" else ROOT/"environments"/distro
    runtime=workspace/".pixi/envs/default"
    if Path(sys.prefix).resolve()!=runtime.resolve() or os.environ.get("ROS_DISTRO")!=distro:
        raise SystemExit("Use the selected distro project Python and ROS_DISTRO")
    out=RAW/'evidence'/('qos-'+label);out.mkdir(parents=True,exist_ok=False)
    envbase=dict(os.environ,ROS_DOMAIN_ID='187',ROS_LOCALHOST_ONLY='1',ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST',SKIP_DEFAULT_XML='1',RCUTILS_LOGGING_USE_STDOUT='0',ROS_LOG_DIR=str(out/'ros-logs'))
    for key in ['ROS_STATIC_PEERS','ROS_DISCOVERY_SERVER','ROS_SUPER_CLIENT','CYCLONEDDS_URI','FASTDDS_DEFAULT_PROFILES_FILE','FASTRTPS_DEFAULT_PROFILES_FILE','FASTDDS_ENVIRONMENT_FILE']:envbase.pop(key,None)
    envbase['AMENT_PREFIX_PATH']=str(runtime)
    envbase['DYLD_LIBRARY_PATH']=str(runtime/'lib')
    target=ROOT/'target' if distro=='jazzy' else ROOT/'target'/distro
    (out/'dependencies.json').write_text(json.dumps([json.loads(p.read_text()) for p in sorted((runtime/'conda-meta').glob('*.json'))],indent=2))
    probe=target/'debug/studio-qos-probe';versions={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [probe,ROOT/'Cargo.lock',workspace/'pixi.lock',Path(__file__).resolve()]};versions['native_packages']=str((out/'dependencies.json').relative_to(RAW));rows=[]
    for rmw in ['rmw_fastrtps_cpp','rmw_cyclonedds_cpp']:
        for mode in ['history','events']:
            env=dict(envbase,RMW_IMPLEMENTATION=rmw);stem=out/(rmw+'-'+mode);server_log=stem.with_suffix('.server.log');server_cmd=[sys.executable,str(Path(__file__).resolve()),'--server',mode]
            with server_log.open('w') as log:
                server=subprocess.Popen(server_cmd,env=env,stdout=log,stderr=log)
                try:
                    deadline=time.monotonic()+10
                    while 'READY' not in server_log.read_text():
                        if server.poll() is not None or time.monotonic()>deadline:raise RuntimeError(f'Fixture start: {server_log}')
                        time.sleep(.05)
                    cmd=[str(probe),mode];p=subprocess.run(cmd,env=env,capture_output=True,text=True,timeout=10)
                    stdout=stem.with_suffix('.stdout');stderr=stem.with_suffix('.stderr');stdout.write_text(p.stdout);stderr.write_text(p.stderr)
                    try:actual=json.loads(p.stdout)
                    except json.JSONDecodeError:actual={'passed':False,'raw':p.stdout}
                    ok=p.returncode==0 and actual.get('passed') is True and 'MATCHED' in server_log.read_text()
                    rows.append(dict(test=mode,outcome='passed' if ok else 'failed',os=platform.platform(),cpu=platform.machine(),distro=distro,middleware_pair=[rmw,rmw],versions=versions,command=cmd,server_command=server_cmd,environment={'ROS_DOMAIN_ID':'187','ROS_LOCALHOST_ONLY':'1','RMW_IMPLEMENTATION':rmw},actual=actual,exit_code=p.returncode,evidence=[str(p.relative_to(RAW)) for p in [stdout,stderr,server_log]]))
                    (RAW/('qos-'+label+'.json')).write_text(json.dumps(rows,indent=2)+'\n');print(rmw,mode,rows[-1]['outcome'],flush=True)
                finally:
                    server.terminate()
                    try:server.wait(timeout=2)
                    except subprocess.TimeoutExpired:server.kill();server.wait(timeout=2)
    return 0 if all(r['outcome']=='passed' for r in rows) else 1


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--distro',choices=['humble','jazzy','lyrical'],default='jazzy');parser.add_argument('--server',choices=['history','events']);parser.add_argument('--label',default=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ'));args=parser.parse_args()
    if args.server:serve(args.server)
    else:
        if not args.label.replace('-','').isalnum():parser.error('Use alphanumeric label')
        raise SystemExit(run(args.label,args.distro))
