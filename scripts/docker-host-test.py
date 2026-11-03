"""Container/native ROS interoperability; not native Linux desktop validation."""
import argparse
import json
import os
from pathlib import Path
import platform
import socket
import subprocess
import threading
import time
import uuid

ROOT=Path(__file__).resolve().parents[1]
(ROOT/'artifacts/feasibility').mkdir(parents=True, exist_ok=True)
IMAGE='ros-studio-peer:jazzy'
OUT=ROOT/'artifacts/feasibility/evidence/docker-host'
OUT.mkdir(parents=True,exist_ok=True)

def remove(name):
    subprocess.run(['docker','rm','-f',name],capture_output=True,timeout=15)

def probe():
    with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as server:
        server.bind(('127.0.0.1',0))
        server.settimeout(12)
        port=server.getsockname()[1]
        received=[]
        def respond():
            try:
                data,address=server.recvfrom(128)
                received.append(data==b'studio-udp-probe')
                server.sendto(b'studio-udp-response',address)
            except socket.timeout: pass
        thread=threading.Thread(target=respond); thread.start()
        name='studio-udp-'+uuid.uuid4().hex[:8]
        code=f'import socket; s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); s.settimeout(8); s.sendto(b"studio-udp-probe",("127.0.0.1",{port})); assert s.recv(128)==b"studio-udp-response"; print("UDP_ROUNDTRIP_OK")'
        cmd=['docker','run','--rm','--network','host','--name',name,IMAGE,'/opt/fixture-venv/bin/python','-c',code]
        try:
            p=subprocess.run(cmd,capture_output=True,text=True,timeout=20)
            (OUT/'udp.log').write_text(p.stdout+p.stderr)
            ok=p.returncode==0 and 'UDP_ROUNDTRIP_OK' in p.stdout
        except subprocess.TimeoutExpired:
            ok=False
            (OUT/'udp.log').write_text('Docker UDP probe timed out')
        finally: remove(name); thread.join(timeout=13)
    return {'test':'docker-host-udp-roundtrip','outcome':'passed' if ok and received else 'failed','command':cmd,'evidence':str((OUT/'udp.log').relative_to(ROOT))}

def run(mode):
    inspect=json.loads(subprocess.check_output(['docker','image','inspect',IMAGE],text=True))[0]
    meta={'os':platform.platform(),'cpu':platform.machine(),'distro':'jazzy','versions':{'rclrs':'0.8.0','native_lock':'pixi.lock','image_id':inspect['Id'],'base_digest':'sha256:066420e07f60aa18262f2479981def87ebcfcec42eefb0c0c57c4a46098348ca'},'container_os':'Linux','container_cpu':inspect['Architecture'],'network':'host','scope':'Docker Desktop Linux peer and native macOS backend on same computer; not real LAN or native Linux desktop'}
    package=subprocess.run(['docker','run','--rm',IMAGE,'cat','/opt/fixture/packages.txt'],capture_output=True,text=True,timeout=20)
    (OUT/'linux-packages.txt').write_text(package.stdout+package.stderr)
    records=[dict(meta,middleware_pair=[None,None],**probe())]
    print(json.dumps(records[-1]),flush=True)
    if records[0]['outcome']=='passed':
        for local in ['rmw_fastrtps_cpp','rmw_cyclonedds_cpp']:
            for remote in ['rmw_fastrtps_cpp','rmw_cyclonedds_cpp']:
                name='studio-peer-'+uuid.uuid4().hex[:8]
                stem=f'{mode}-{local}-{remote}'
                out=OUT/(stem+'.jsonl'); err=OUT/(stem+'.stderr'); peerlog=OUT/(stem+'-peer.log')
                env=dict(os.environ,ROS_DOMAIN_ID='187',RMW_IMPLEMENTATION=local,ROS_LOG_DIR=str(ROOT/'artifacts/docker-host-logs'),RCUTILS_LOGGING_USE_STDOUT='0')
                env.pop('ROS_LOCALHOST_ONLY',None)
                cmd=['docker','run','--rm','--network','host','--name',name,'-e','ROS_DOMAIN_ID=187','-e',f'RMW_IMPLEMENTATION={remote}']
                if mode=='local-peers':
                    env['ROS_AUTOMATIC_DISCOVERY_RANGE']='LOCALHOST'
                    env['ROS_STATIC_PEERS']='127.0.0.1'
                    cmd += ['-e','ROS_AUTOMATIC_DISCOVERY_RANGE=LOCALHOST','-e','ROS_STATIC_PEERS=127.0.0.1']
                cmd += [IMAGE,'/opt/fixture-venv/bin/python','/opt/fixture/smoke.py','--peer','--echo']
                native=[str(ROOT/'target/debug/studio-backend'),'/studio_fixture','std_msgs/msg/String','--fixture-echo']
                processes=[]; got=False; echo=False
                start=time.monotonic()
                with out.open('w') as o,err.open('w') as e,peerlog.open('w') as pl:
                    try:
                        processes.append(subprocess.Popen(native,env=env,stdout=o,stderr=e))
                        processes.append(subprocess.Popen(cmd,stdout=pl,stderr=pl))
                        deadline=time.monotonic()+30
                        while time.monotonic()<deadline:
                            time.sleep(.25)
                            got='"event":"sample"' in out.read_text() and 'feasibility-proof' in out.read_text()
                            echo='ECHO_VERIFIED' in peerlog.read_text()
                            if got and echo or any(p.poll() is not None for p in processes): break
                    finally:
                        remove(name)
                        for p in processes:
                            if p.poll() is None: p.terminate()
                            try: p.wait(timeout=3)
                            except subprocess.TimeoutExpired: p.kill(); p.wait()
                record=dict(meta,test='docker-native-topic-roundtrip',mode=mode,middleware_pair=[local,remote],command={'native':native,'container':cmd},outcome='passed' if got and echo else 'failed',native_received=got,container_received_echo=echo,seconds=round(time.monotonic()-start,3),evidence=[str(p.relative_to(ROOT)) for p in [out,err,peerlog]])
                records.append(record); print(json.dumps(record),flush=True)
    dest=ROOT/f'artifacts/feasibility/docker-host-{mode}.json'
    dest.write_text(json.dumps(records,indent=2)+'\n')
    return 0 if len(records)==5 and all(r['outcome']=='passed' for r in records) else 1

if __name__=='__main__':
    parser=argparse.ArgumentParser()
    parser.add_argument('--mode',choices=['default','local-peers'],default='default')
    args=parser.parse_args()
    raise SystemExit(run(args.mode))
