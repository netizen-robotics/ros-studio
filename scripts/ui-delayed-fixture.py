"""Synthetic delayed stdout handshake; no ROS, network or commands."""
import json
import os
from pathlib import Path
import time

root = Path(os.environ['STUDIO_UI_FIXTURE_EVIDENCE'])
root.mkdir(parents=True, exist_ok=True)
def record(event):
    with (root/'launches.jsonl').open('a') as stream:
        stream.write(json.dumps({'event':event,'pid':os.getpid(),'time':time.time()})+'\n')
record('started')
time.sleep(25)
record('hello')
print(json.dumps({'event':'hello','version':2,'distro':'jazzy','rmw':'synthetic-delayed-no-ros'}),flush=True)
print(json.dumps({'event':'graph','topics':[['/studio_fixture/delayed',['std_msgs/msg/String']]]}),flush=True)
try:
    for i in range(90):
        print(json.dumps({'event':'sample','topic':'/studio_fixture/delayed','fields':[['data',str(i)]]}),flush=True)
        time.sleep(1)
except BrokenPipeError:
    pass
record('exited')
