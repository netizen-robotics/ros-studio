"""Runtime-imported custom topic echo and bounded preview, using synthetic peers."""
import argparse
from array import array
import hashlib
import importlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import time
ROOT=Path(__file__).resolve().parents[1];RAW=ROOT/'artifacts/feasibility'

def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def stop(p):
    if p.poll() is None:p.terminate()
    try:p.wait(timeout=2)
    except subprocess.TimeoutExpired:p.kill();p.wait(timeout=2)

def peer(package):
    if os.environ.get('ROS_DOMAIN_ID')!='187' or os.environ.get('ROS_LOCALHOST_ONLY')!='1':raise SystemExit('Local fixture only')
    import rclpy
    typ=getattr(importlib.import_module(package+'.msg'),'Sample' if package=='feasibility_interfaces' else 'Entry')
    value=typ()
    if package=='feasibility_interfaces':
        value.sequence=18446744073709551615
        value.nested.label='λ'*200;value.nested.value=3.25
        value.fixed=array('d',[1.,2.,3.]);value.bounded=array('i',[-2147483648,0,2147483647])
        value.labels=[f'label-{i}' for i in range(12)];value.payload=array('B',bytes(range(256))*4)
    else:value.label='independent package';value.value=-8.5
    rclpy.init();node=rclpy.create_node('studio_custom_topic_fixture');pub=node.create_publisher(typ,'/studio_fixture',10)
    received=False
    def callback(message):
        nonlocal received
        if message==value:received=True;print(json.dumps({'exact_echo':True}),flush=True)
    sub=node.create_subscription(typ,'/studio_echo',callback,10)
    deadline=time.monotonic()+15
    try:
        while not received and time.monotonic()<deadline:
            pub.publish(value);rclpy.spin_once(node,timeout_sec=.1)
    finally:node.destroy_node();rclpy.try_shutdown()
    return 0 if received else 1

def verify_preview(events,package):
    for event in events:
        if event.get('event')!='sample':continue
        fields=dict(event['fields'])
        try:
            if package=='feasibility_interfaces':
                nested=json.loads(fields['nested'])['fields']
                if (fields.get('[preview truncated]')=='true'
                    and json.loads(fields['nested'])['truncated'] is True
                    and json.loads(fields['sequence'])==18446744073709551615
                    and nested['label']=={'bytes':400,'truncated':True,'text':'λ'*128}
                    and nested['value']==3.25
                    and json.loads(fields['fixed'])=={'length':3,'truncated':False,'items':[1.,2.,3.]}
                    and json.loads(fields['bounded'])=={'length':3,'truncated':False,'items':[-2147483648,0,2147483647]}
                    and json.loads(fields['payload'])=={'length':1024,'truncated':True,'items':list(range(8))}
                    and json.loads(fields['labels'])['length']==12
                    and len(json.loads(fields['labels'])['items'])==8
                    and json.loads(fields['labels'])['truncated'] is True):return True
            elif fields.get('[preview truncated]')=='false' and fields['label']=='Simple(String("independent package"))' and json.loads(fields['value'])==-8.5:return True
        except (KeyError,TypeError,ValueError):pass
    return False

def run(a):
    workspace=ROOT if a.distro=='jazzy' else ROOT/'environments'/a.distro;runtime=workspace/'.pixi/envs/default'
    if Path(sys.prefix).resolve()!=runtime.resolve() or os.environ.get('ROS_DISTRO')!=a.distro:raise SystemExit('Use the selected project environment')
    out=RAW/'evidence'/('custom-topics-'+a.label);out.mkdir(parents=True,exist_ok=False)
    backend=(ROOT/'target' if a.distro=='jazzy' else ROOT/'target'/a.distro)/'debug/studio-backend';desktop=backend.with_name('studio-desktop')
    packaged=None
    if a.runtime_bundle:
        packaged=a.runtime_bundle.resolve()
        if json.loads((packaged/'manifest.json').read_text()).get('distro')!=a.distro:raise SystemExit('Packaged runtime distro mismatch')
        backend=packaged/'bin/studio-backend';desktop=backend.with_name('studio-desktop')
    hashes={p.name:digest(p) for p in [backend,desktop]}
    env=dict(os.environ,ROS_DOMAIN_ID='187',ROS_LOCALHOST_ONLY='1',ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST',AMENT_PREFIX_PATH=str(runtime),CMAKE_PREFIX_PATH=str(runtime),DYLD_LIBRARY_PATH=str(runtime/'lib'),RCUTILS_LOGGING_USE_STDOUT='0',ROS_LOG_DIR=str(out/'ros-logs'))
    for key in ['ROS_STATIC_PEERS','ROS_DISCOVERY_SERVER','ROS_SUPER_CLIENT','CYCLONEDDS_URI','FASTRTPS_DEFAULT_PROFILES_FILE','FASTDDS_DEFAULT_PROFILES_FILE','FASTDDS_ENVIRONMENT_FILE']:env.pop(key,None)
    rows=[]
    for package in ['feasibility_interfaces','service_extra_interfaces']:
        bundle=out/(package+' relocated bundle')
        build=[sys.executable,'scripts/build-service-bundle.py','--distro',a.distro,'--package',package,'--destination',str(bundle)]
        build_log=out/(package+'-build.log')
        with build_log.open('w') as log:subprocess.run(build,env=env,stdout=log,stderr=log,check=True)
        manifest=json.loads((bundle/'manifest.json').read_text());assert manifest['ros_distro']==a.distro
        assert all(digest(bundle/f)==h for f,h in manifest['files'].items())
        native_audit=[]
        for lib in (bundle/'lib').glob('*.dylib'):
            loads=subprocess.check_output(['otool','-L',str(lib)],text=True)
            commands=subprocess.check_output(['otool','-l',str(lib)],text=True)
            rpaths=re.findall(r'cmd LC_RPATH\s+cmdsize \d+\s+path (.*?) \(offset',commands)
            assert not any(path.startswith('/') and not path.startswith(('/System/','/usr/lib/')) for path in rpaths),'Development RPATH remains'
            deps=[line.strip().split(' (')[0] for line in loads.splitlines()[1:]]
            assert all(dep.startswith(('@rpath/','@loader_path/','/System/','/usr/lib/')) for dep in deps),'Absolute development dependency remains'
            native_audit.append({'file':str(lib),'rpaths':rpaths,'dependencies':deps,'sha256':digest(lib)})
        audit_path=out/(package+'-native-audit.json');audit_path.write_text(json.dumps(native_audit,indent=2)+'\n')
        install=ROOT/'artifacts'/(package+'-service-install'+('' if a.distro=='jazzy' else '-'+a.distro))
        typ=package+'/msg/'+('Sample' if package=='feasibility_interfaces' else 'Entry')
        for rmw in ['rmw_fastrtps_cpp','rmw_cyclonedds_cpp']:
            stem=out/(package+'-'+rmw);paths=[stem.with_suffix(s) for s in ['.backend.stdout','.backend.stderr','.peer.stdout','.peer.stderr']]
            backend_cmd=[str(backend),'/studio_fixture',typ,'--fixture-echo','--interface-bundle',str(bundle)];peer_cmd=[sys.executable,str(Path(__file__).resolve()),'--peer',package]
            backend_env=dict(env,RMW_IMPLEMENTATION=rmw,AMENT_PREFIX_PATH=str(runtime))
            if packaged:
                home=out/'backend-home';home.mkdir(exist_ok=True)
                backend_env={key:env[key] for key in ['ROS_DOMAIN_ID','ROS_LOCALHOST_ONLY','ROS_AUTOMATIC_DISCOVERY_RANGE','RCUTILS_LOGGING_USE_STDOUT','ROS_LOG_DIR']}
                backend_env.update(PATH='/usr/bin:/bin',HOME=str(home),ROS_DISTRO=a.distro,RMW_IMPLEMENTATION=rmw,AMENT_PREFIX_PATH=str(packaged))
            peer_env=dict(env,RMW_IMPLEMENTATION=rmw,AMENT_PREFIX_PATH=str(install)+os.pathsep+str(runtime),DYLD_LIBRARY_PATH=str(install/'lib')+os.pathsep+str(runtime/'lib'),PYTHONPATH=str(install/f'lib/python{sys.version_info.major}.{sys.version_info.minor}/site-packages'))
            with paths[0].open('w') as bo,paths[1].open('w') as be,paths[2].open('w') as po,paths[3].open('w') as pe:
                b=subprocess.Popen(backend_cmd,env=backend_env,stdout=bo,stderr=be)
                p=subprocess.Popen(peer_cmd,env=peer_env,stdout=po,stderr=pe)
                try:
                    try:p.wait(timeout=25)
                    except subprocess.TimeoutExpired:stop(p)
                    time.sleep(.2)
                    events=[json.loads(line) for line in paths[0].read_text().splitlines() if line.startswith('{')]
                    exact=p.returncode==0 and '"exact_echo": true' in paths[2].read_text()
                    preview=verify_preview(events,package)
                    unchanged=all(digest(path)==hashes[path.name] for path in [backend,desktop])
                    passed=exact and preview and unchanged and b.poll() is None
                finally:stop(p);stop(b)
            rows.append(dict(test='runtime-custom-topic-'+package,outcome='passed' if passed else 'failed',os=platform.platform(),cpu=platform.machine(),distro=a.distro,middleware_pair=[rmw,rmw],versions={**hashes,'pixi_lock_sha256':digest(workspace/'pixi.lock'),'cargo_lock_sha256':digest(ROOT/'Cargo.lock')},command=[build,backend_cmd,peer_cmd],backend_environment=(backend_env if packaged else None),runtime_bundle_manifest_sha256=(digest(packaged/"manifest.json") if packaged else None),exact_echo=exact,preview_verified=preview,preview_format='native structured explicit import',binaries_unchanged=unchanged,evidence=[str(p.relative_to(RAW)) for p in [*paths,build_log,bundle/'manifest.json',audit_path]],scope='Development-host imported native topic support and bounded previews; Python peer uses development install; no clean-runtime claim'))
            (RAW/('custom-topics-'+a.label+'.json')).write_text(json.dumps(rows,indent=2)+'\n');print(package,rmw,rows[-1]['outcome'],flush=True)
    # Corrupt copies only; reject before even emitting ROS Hello/graph events.
    for case,expected in [('distro','Incompatible bundle'),('platform','Incompatible bundle'),('checksum','Checksum mismatch'),('missing','Missing bundle file'),('unlisted','Unlisted bundle file')]:
        bad=out/('invalid-'+case);shutil.copytree(bundle,bad)
        manifest=json.loads((bad/'manifest.json').read_text())
        lib=next((bad/'lib').glob('*.dylib'))
        if case=='distro':manifest['ros_distro']='jazzy' if a.distro=='humble' else 'humble'
        elif case=='platform':manifest['os']='windows'
        elif case=='checksum':lib.write_bytes(b'corrupt')
        elif case=='missing':lib.unlink()
        else:(bad/'unexpected.bin').write_bytes(b'unlisted')
        (bad/'manifest.json').write_text(json.dumps(manifest))
        cmd=[str(backend),'/studio_fixture',typ,'--interface-bundle',str(bad)]
        result=subprocess.run(cmd,env=backend_env,capture_output=True,text=True,timeout=10)
        stdout=out/(case+'.stdout');stderr=out/(case+'.stderr')
        stdout.write_text(result.stdout);stderr.write_text(result.stderr)
        try:events=[json.loads(line) for line in result.stdout.splitlines()]
        except ValueError:events=[]
        passed=result.returncode==1 and len(events)==1 and events[0].get('event')=='error' and expected in events[0].get('message','')
        rows.append(dict(test='backend-import-'+case,outcome='passed' if passed else 'failed',os=platform.platform(),cpu=platform.machine(),distro=a.distro,middleware_pair=[backend_env['RMW_IMPLEMENTATION'],None],versions=hashes,command=cmd,actual=events,exit_code=result.returncode,evidence=[str(p.relative_to(RAW)) for p in [stdout,stderr,bad/'manifest.json']],scope='Backend rejects explicit invalid bundle before ROS context creation; no peer or publication'))
        (RAW/('custom-topics-'+a.label+'.json')).write_text(json.dumps(rows,indent=2)+'\n');print(case,rows[-1]['outcome'],flush=True)
    return 0 if all(row['outcome']=='passed' for row in rows) else 1

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--peer',choices=['feasibility_interfaces','service_extra_interfaces']);p.add_argument('--distro',choices=['humble','jazzy','lyrical'],default='lyrical');p.add_argument('--label');p.add_argument('--runtime-bundle',type=Path);a=p.parse_args()
    if a.peer:raise SystemExit(peer(a.peer))
    if not a.label or not a.label.replace('-','').isalnum():p.error('Use a unique alphanumeric label')
    raise SystemExit(run(a))
