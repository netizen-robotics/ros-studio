"""Native /clock and tf2 integration on synthetic localhost/domain187 peers."""
import argparse
from datetime import datetime,timezone
import hashlib,json,os,platform,subprocess,sys,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];RAW=ROOT/'artifacts/feasibility'


def serve():
    if os.environ.get('ROS_DOMAIN_ID')!='187' or os.environ.get('ROS_LOCALHOST_ONLY')!='1':raise SystemExit('Local fixture only')
    import rclpy
    from rclpy.qos import QoSProfile,DurabilityPolicy
    from rclpy.executors import ExternalShutdownException
    from rosgraph_msgs.msg import Clock
    from tf2_msgs.msg import TFMessage
    from geometry_msgs.msg import TransformStamped
    rclpy.init();node=rclpy.create_node('studio_time_tf_server')
    clock=node.create_publisher(Clock,'/clock',10)
    dynamic=node.create_publisher(TFMessage,'/tf',10)
    static=node.create_publisher(TFMessage,'/tf_static',QoSProfile(depth=1,durability=DurabilityPolicy.TRANSIENT_LOCAL))
    def transform(parent,child,stamp,x):
        t=TransformStamped();t.header.frame_id=parent;t.child_frame_id=child;t.header.stamp.sec=stamp;t.transform.translation.x=float(x);t.transform.rotation.w=1.;return t
    static.publish(TFMessage(transforms=[transform('map','odom',0,10),transform('base','sensor',0,1)]))
    print('STATIC_PUBLISHED_BEFORE_PROBE',flush=True)
    try:
        deadline=time.monotonic()+15
        while not all(p.get_subscription_count()>0 for p in [clock,dynamic,static]):
            if time.monotonic()>deadline:raise RuntimeError('Probe subscription discovery timed out')
            rclpy.spin_once(node,timeout_sec=.05)
        print('ALL_SUBSCRIPTIONS_MATCHED',flush=True)
        start=time.monotonic();previous=None
        while rclpy.ok():
            elapsed=time.monotonic()-start
            if elapsed<1:stage='initial';stamp=10;publish_tf=True
            elif elapsed<1.5:stage='pause';stamp=None;publish_tf=False
            elif elapsed<2.3:stage='reset';stamp=2;publish_tf=False
            elif elapsed<3.3:stage='new-epoch';stamp=2;publish_tf=True
            elif elapsed<4.3:stage='forward';stamp=20;publish_tf=False
            else:stage='paused-final';stamp=None;publish_tf=False
            if stage!=previous:print(json.dumps({'stage':stage,'elapsed':elapsed}),flush=True);previous=stage
            if stamp is not None:
                msg=Clock();msg.clock.sec=stamp;clock.publish(msg)
            if publish_tf:dynamic.publish(TFMessage(transforms=[transform('odom','base',stamp,2 if stamp==10 else 8)]))
            rclpy.spin_once(node,timeout_sec=.05);time.sleep(.05)
    except (ExternalShutdownException,KeyboardInterrupt):pass
    except Exception:
        if rclpy.ok():raise
    finally:node.destroy_node();rclpy.try_shutdown()


def run(label, distro):
    workspace=ROOT if distro=="jazzy" else ROOT/"environments"/distro
    runtime=workspace/".pixi/envs/default"
    if Path(sys.prefix).resolve()!=runtime.resolve() or os.environ.get("ROS_DISTRO")!=distro:
        raise SystemExit("Use the selected distro project Python and ROS_DISTRO")
    out=RAW/'evidence'/('time-tf-'+label);out.mkdir(parents=True,exist_ok=False)
    base=dict(os.environ,ROS_DOMAIN_ID='187',ROS_LOCALHOST_ONLY='1',ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST',SKIP_DEFAULT_XML='1',RCUTILS_LOGGING_USE_STDOUT='0',ROS_LOG_DIR=str(out/'ros-logs'))
    for key in ['ROS_STATIC_PEERS','CYCLONEDDS_URI','FASTRTPS_DEFAULT_PROFILES_FILE','FASTDDS_DEFAULT_PROFILES_FILE','ROS_DISCOVERY_SERVER','ROS_SUPER_CLIENT','FASTDDS_ENVIRONMENT_FILE']:base.pop(key,None)
    base['AMENT_PREFIX_PATH']=str(runtime)
    base['DYLD_LIBRARY_PATH']=str(runtime/'lib')
    target=ROOT/'target' if distro=='jazzy' else ROOT/'target'/distro
    probe=target/'debug/studio-time-tf-probe'
    (out/'dependencies.json').write_text(json.dumps([json.loads(p.read_text()) for p in sorted((runtime/'conda-meta').glob('*.json'))],indent=2))
    versions={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [probe,ROOT/'Cargo.lock',workspace/'pixi.lock',Path(__file__).resolve()]};versions['native_packages']=str((out/'dependencies.json').relative_to(RAW))
    rows=[]
    for rmw in ['rmw_fastrtps_cpp','rmw_cyclonedds_cpp']:
        env=dict(base,RMW_IMPLEMENTATION=rmw);log_path=out/(rmw+'-server.log')
        server_cmd=[sys.executable,str(Path(__file__).resolve()),'--server']
        with log_path.open('w') as log:
            server=subprocess.Popen(server_cmd,env=env,stdout=log,stderr=log)
            try:
                deadline=time.monotonic()+10
                while 'STATIC_PUBLISHED_BEFORE_PROBE' not in log_path.read_text():
                    if server.poll() is not None or time.monotonic()>deadline:raise RuntimeError(f'Fixture startup: {log_path}')
                    time.sleep(.05)
                p=subprocess.run([str(probe)],env=env,capture_output=True,text=True,timeout=14)
                stdout=out/(rmw+'.stdout');stderr=out/(rmw+'.stderr');stdout.write_text(p.stdout);stderr.write_text(p.stderr)
                try:result=json.loads(p.stdout)
                except json.JSONDecodeError:result={'passed':False,'raw':p.stdout}
                ok=p.returncode==0 and result.get('passed') is True and 'ALL_SUBSCRIPTIONS_MATCHED' in log_path.read_text()
                rows.append(dict(test='native-clock-tf-integration',outcome='passed' if ok else 'failed',os=platform.platform(),cpu=platform.machine(),distro=distro,middleware_pair=[rmw,rmw],command=[str(probe)],server_command=server_cmd,versions=versions,environment={'ROS_DOMAIN_ID':'187','ROS_LOCALHOST_ONLY':'1','RMW_IMPLEMENTATION':rmw},actual=result,exit_code=p.returncode,evidence=[str(p.relative_to(RAW)) for p in [log_path,stdout,stderr]]))
                (RAW/('time-tf-'+label+'.json')).write_text(json.dumps(rows,indent=2)+'\n');print(rmw,rows[-1]['outcome'],flush=True)
            finally:
                server.terminate()
                try:server.wait(timeout=2)
                except subprocess.TimeoutExpired:server.kill();server.wait(timeout=2)
    return 0 if all(r['outcome']=='passed' for r in rows) else 1


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--distro',choices=['humble','jazzy','lyrical'],default='jazzy');parser.add_argument('--server',action='store_true');parser.add_argument('--label',default=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ'));args=parser.parse_args()
    if args.server:serve()
    else:
        if not args.label.replace('-','').isalnum():parser.error('Use alphanumeric label')
        raise SystemExit(run(args.label,args.distro))
