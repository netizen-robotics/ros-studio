"""Exact 2 MiB image/point-cloud echo through the Rust dynamic-message backend."""
import argparse
from array import array
from contextlib import ExitStack
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
RAW = ROOT/'artifacts/feasibility'
RMWS = ['rmw_fastrtps_cpp', 'rmw_cyclonedds_cpp']


def peer(kind, best_effort, payload_bytes):
    if os.environ.get('ROS_DOMAIN_ID') != '187' or os.environ.get('ROS_LOCALHOST_ONLY') != '1':
        raise SystemExit('Synthetic localhost/domain 187 only')
    import rclpy
    from rclpy.qos import QoSProfile, ReliabilityPolicy
    from sensor_msgs.msg import Image, PointCloud2, PointField
    typ = Image if kind == 'image' else PointCloud2
    msg = typ()
    msg.header.frame_id = 'synthetic_payload'
    msg.header.stamp.sec = 123
    msg.header.stamp.nanosec = 456
    msg.data = array('B', (bytes(range(256))*((payload_bytes+255)//256))[:payload_bytes])
    msg.is_bigendian = False
    if kind == 'image':
        msg.height = 1024; msg.width = 2048; msg.encoding = 'mono8'; msg.step = 2048
        if payload_bytes != 2097152: msg.height=1; msg.width=payload_bytes; msg.step=payload_bytes
    else:
        msg.height = 1; msg.width = payload_bytes//16; msg.point_step = 16; msg.row_step = payload_bytes
        msg.is_dense = False
        msg.fields = [PointField(name=n, offset=i*4, datatype=PointField.FLOAT32, count=1)
                      for i,n in enumerate(['x','y','z','intensity'])]
    rclpy.init(); node = rclpy.create_node('sensor_payload_fixture')
    qos = QoSProfile(depth=2, reliability=ReliabilityPolicy.BEST_EFFORT if best_effort else ReliabilityPolicy.RELIABLE)
    pub = node.create_publisher(typ, '/studio_fixture', qos)
    verified = False
    def received(value):
        nonlocal verified
        if value == msg:
            verified = True
            print(json.dumps({'exact_echo': True, 'bytes': len(value.data),
                              'sha256': hashlib.sha256(bytes(value.data)).hexdigest()}), flush=True)
    sub = node.create_subscription(typ, '/studio_echo', received, 10)
    deadline = time.monotonic()+25
    try:
        while not verified and time.monotonic()<deadline:
            pub.publish(msg)
            rclpy.spin_once(node, timeout_sec=.2)
    finally:
        node.destroy_node(); rclpy.try_shutdown()
    return 0 if verified else 1


def stop(p):
    if p.poll() is None: p.terminate()
    try: p.wait(timeout=3)
    except subprocess.TimeoutExpired: p.kill(); p.wait()


def run(args):
    workspace=ROOT if args.distro=="jazzy" else ROOT/"environments"/args.distro
    runtime=workspace/".pixi/envs/default"
    if Path(sys.prefix).resolve()!=runtime.resolve() or os.environ.get("ROS_DISTRO")!=args.distro:
        raise SystemExit("Use the selected distro project Python and ROS_DISTRO")
    out = RAW/'evidence'/('sensor-payload-'+args.label); out.mkdir(parents=True, exist_ok=False)
    fast = out/'fast.xml'; fast.write_bytes((args.profiles_dir/'fast-loopback.xml').read_bytes())
    if args.socket_buffer_bytes or args.max_datagram_bytes:
        tree = ET.parse(fast)
        ns = '{http://www.eprosima.com/XMLSchemas/fastRTPS_Profiles}'
        for transport in tree.getroot().findall('.//'+ns+'transport_descriptor'):
            settings = {}
            if args.socket_buffer_bytes:
                settings.update(sendBufferSize=args.socket_buffer_bytes,receiveBufferSize=args.socket_buffer_bytes)
            if args.max_datagram_bytes: settings['maxMessageSize']=args.max_datagram_bytes
            for name,value in settings.items():
                element = transport.find(ns+name)
                if element is None: element = ET.SubElement(transport,ns+name)
                element.text = str(value)
        ET.register_namespace('', ns[1:-1]); tree.write(fast)
    cyclone = ET.parse(args.profiles_dir/'cyclone-trace.xml')
    for domain in cyclone.getroot().findall('Domain'):
        for tracing in domain.findall('Tracing'): domain.remove(tracing)
        if args.distro == 'humble':
            # Humble's RMW supplies lo0 for ROS_LOCALHOST_ONLY; selecting it
            # again in XML causes Cyclone to reject the duplicate interface.
            for general in domain.findall('General'):
                for interfaces in general.findall('Interfaces'):
                    general.remove(interfaces)
    cycl = out/'cyclone.xml'; cyclone.write(cycl)
    env = dict(os.environ, ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1', ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST',
               FASTRTPS_DEFAULT_PROFILES_FILE=str(fast), CYCLONEDDS_URI=str(cycl), SKIP_DEFAULT_XML='1',
               RCUTILS_LOGGING_USE_STDOUT='0', ROS_LOG_DIR=str(out/'ros-logs'))
    for key in ['ROS_STATIC_PEERS','ROS_DISCOVERY_SERVER','ROS_SUPER_CLIENT','FASTDDS_DEFAULT_PROFILES_FILE','FASTDDS_ENVIRONMENT_FILE']:
        env.pop(key,None)
    env['AMENT_PREFIX_PATH']=str(runtime)
    env['DYLD_LIBRARY_PATH']=str(runtime/'lib')
    if args.default_transports:
        env.pop('FASTRTPS_DEFAULT_PROFILES_FILE',None)
        env.pop('CYCLONEDDS_URI',None)
    target=ROOT/'target' if args.distro=='jazzy' else ROOT/'target'/args.distro
    backend=target/('debug/studio-native-topic-probe' if args.native_topic_probe else 'debug/studio-backend')
    backend_env=env
    bundle_manifest=None
    if args.bundle:
        bundle=args.bundle.resolve()
        bundle_manifest=json.loads((bundle/'manifest.json').read_text())
        if bundle_manifest.get('distro')!=args.distro:raise SystemExit('Bundle distro mismatch')
        backend=bundle/'bin/studio-backend'
        home=out/'backend-home';home.mkdir()
        backend_env={k:env[k] for k in ['ROS_DOMAIN_ID','ROS_LOCALHOST_ONLY','ROS_AUTOMATIC_DISCOVERY_RANGE','FASTRTPS_DEFAULT_PROFILES_FILE','CYCLONEDDS_URI','RMW_FASTRTPS_USE_QOS_FROM_XML','SKIP_DEFAULT_XML','RCUTILS_LOGGING_USE_STDOUT','ROS_LOG_DIR'] if k in env}
        backend_env.update(PATH='/usr/bin:/bin',HOME=str(home),ROS_DISTRO=args.distro,AMENT_PREFIX_PATH=str(bundle))
    (out/'dependencies.json').write_text(json.dumps([json.loads(p.read_text()) for p in sorted((runtime/'conda-meta').glob('*.json'))],indent=2))
    versions = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in [backend,ROOT/'Cargo.lock',workspace/'pixi.lock',Path(__file__)]}
    if args.bundle:versions['bundle_manifest_sha256']=hashlib.sha256((args.bundle.resolve()/'manifest.json').read_bytes()).hexdigest()
    versions['native_packages']=str((out/'dependencies.json').relative_to(RAW))
    records = []
    for local in RMWS:
        for remote in RMWS:
            if args.fast_only and (local != RMWS[0] or remote != RMWS[0]): continue
            for kind in ['image','cloud']:
                for best in [False, True]:
                    if args.best_effort and not best: continue
                    name = f'{kind}-{local}-{remote}-'+('best-effort' if best else 'reliable')
                    typ = 'sensor_msgs/msg/'+('Image' if kind=='image' else 'PointCloud2')
                    cmd = [str(backend),'/studio_fixture',typ,'--fixture-echo']+(['--best-effort'] if best else [])
                    peer_cmd = [sys.executable,str(Path(__file__).resolve()),'--peer',kind,'--payload-bytes',str(args.payload_bytes)]+(['--best-effort'] if best else [])
                    paths = [out/(name+suffix) for suffix in ['.backend.stdout','.backend.stderr','.peer.stdout','.peer.stderr']]
                    started=time.monotonic()
                    with ExitStack() as stack:
                        files=[stack.enter_context(p.open('w')) for p in paths]
                        backend_process=subprocess.Popen(cmd,env=dict(backend_env,RMW_IMPLEMENTATION=local),stdout=files[0],stderr=files[1]);stack.callback(stop,backend_process)
                        fixture=subprocess.Popen(peer_cmd,env=dict(env,RMW_IMPLEMENTATION=remote),stdout=files[2],stderr=files[3]);stack.callback(stop,fixture)
                        deadline=time.monotonic()+30
                        while fixture.poll() is None and backend_process.poll() is None and time.monotonic()<deadline:
                            time.sleep(.05)
                        if fixture.poll() is None: stop(fixture)
                        lines=[json.loads(line) for line in paths[2].read_text().splitlines() if line.startswith('{')]
                        ok=fixture.returncode==0 and any(x.get('exact_echo') and x.get('bytes')==args.payload_bytes for x in lines)
                        if args.native_topic_probe or args.distro == 'lyrical':
                            if args.native_topic_probe:
                                previews=[json.loads(line)['native_preview'] for line in paths[1].read_text().splitlines() if line.startswith('{"native_preview":')]
                            else:
                                previews=[]
                                for line in paths[0].read_text().splitlines():
                                    if not line.startswith('{'): continue
                                    event=json.loads(line)
                                    if event.get('event')!='sample': continue
                                    fields=dict(event['fields'])
                                    if 'data' in fields:
                                        previews.append({'truncated':fields.get('[preview truncated]')=='true','fields':{'data':json.loads(fields['data'])}})
                            valid_preview=any(p.get('truncated') is True and p.get('fields',{}).get('data')=={'length':args.payload_bytes,'truncated':True,'items':list(range(8))} and len(json.dumps(p).encode())<=16384 for p in previews)
                            ok=ok and valid_preview
                            lines.append({'native_preview_verified':valid_preview,'previews':previews})
                    records.append(dict(test=name,outcome='passed' if ok else 'failed',os=platform.platform(),cpu=platform.machine(),distro=args.distro,versions=versions,middleware_pair=[local,remote],command=[cmd,peer_cmd],environment={k:env.get(k) for k in ['ROS_DOMAIN_ID','ROS_LOCALHOST_ONLY','FASTRTPS_DEFAULT_PROFILES_FILE','CYCLONEDDS_URI','RMW_FASTRTPS_USE_QOS_FROM_XML']},backend_environment=({**backend_env,'RMW_IMPLEMENTATION':local} if args.bundle else None),bundle_manifest=bundle_manifest,seconds=round(time.monotonic()-started,3),actual=lines,evidence=[str(p.relative_to(RAW)) for p in [*paths,fast,cycl]],scope=('Exact full-message equality after standalone native rcl echo; observation backend not exercised' if args.native_topic_probe else 'Exact full-message equality after Rust dynamic echo')+('; sanitized relocated bundle backend, development Python peer' if args.bundle else '')+'; no image/cloud rendering, clean-host or LAN qualification'))
                    (RAW/('sensor-payload-'+args.label+'.json')).write_text(json.dumps(records,indent=2)+'\n')
                    print(name,records[-1]['outcome'],flush=True)
    return 0 if all(r['outcome']=='passed' for r in records) else 1


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--peer',choices=['image','cloud']);parser.add_argument('--best-effort',action='store_true')
    parser.add_argument('--distro',choices=['humble','jazzy','lyrical'],default='jazzy')
    parser.add_argument('--bundle',type=Path,help='Use a relocated flat runtime with sanitized backend environment')
    parser.add_argument('--native-topic-probe',action='store_true',help='Use the separate standard-rcl fixture echo, not the observation backend')
    parser.add_argument('--default-transports',action='store_true',help='Use RMW default transports with localhost discovery instead of explicit loopback XML')
    parser.add_argument('--profiles-dir',type=Path);parser.add_argument('--label')
    parser.add_argument('--fast-only',action='store_true',help='Run only the Fast/Fast pair')
    parser.add_argument('--payload-bytes',type=int,default=2097152)
    parser.add_argument('--socket-buffer-bytes',type=int,default=0,help='Process-local Fast UDP send/receive buffer request')
    parser.add_argument('--max-datagram-bytes',type=int,default=0,help='Process-local Fast transport maxMessageSize')
    args=parser.parse_args()
    if not 16 <= args.payload_bytes <= 4194304 or args.payload_bytes % 16: parser.error('Payload must be a multiple of 16 between 16 bytes and 4 MiB')
    if not 0 <= args.socket_buffer_bytes <= 16777216: parser.error('Socket buffer request must be between 0 and 16 MiB')
    if args.max_datagram_bytes and not 1024 <= args.max_datagram_bytes <= 65000: parser.error('Datagram size must be between 1024 and 65000 bytes')
    if args.bundle and args.native_topic_probe:parser.error('Bundle mode targets the observation backend')
    if args.peer: raise SystemExit(peer(args.peer,args.best_effort,args.payload_bytes))
    if not args.profiles_dir or not args.label or not args.label.replace('-','').isalnum(): parser.error('Supply --profiles-dir and unique alphanumeric --label')
    raise SystemExit(run(args))
