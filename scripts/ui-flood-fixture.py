#!/usr/bin/env python3
"""Synthetic stdout backend for manual native-window resilience checks; no ROS."""
import importlib.util
import json
import os
from pathlib import Path
import sys
import time

spec = importlib.util.spec_from_file_location("typed_fixture", Path(__file__).with_name("ui-workspace-fixture.py"))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)

root = Path(os.environ["STUDIO_UI_FIXTURE_EVIDENCE"])
root.mkdir(parents=True, exist_ok=True)
with (root / "launches.jsonl").open("a") as log:
    log.write(json.dumps({"pid": os.getpid(), "started": time.time()}) + "\n")
print(json.dumps({"event": "hello", "version": 2, "distro": "jazzy", "rmw": "synthetic-no-ros"}), flush=True)
print(json.dumps({"event": "graph", "topics": [["/studio_fixture/flood", ["std_msgs/msg/Int32"]]]}), flush=True)
deadline = time.monotonic() + 90
count = 0
try:
    while time.monotonic() < deadline and not (root / "exit-request").exists():
        batch = []
        for _ in range(1000):
            count += 1
            batch.append(json.dumps(fixture.typed_event("/studio_fixture/flood", "std_msgs/msg/Int32", count)))
        sys.stdout.write("\n".join(batch) + "\n")
        sys.stdout.flush()
    with (root / "exits.jsonl").open("a") as log:
        log.write(json.dumps({"pid": os.getpid(), "samples_written": count}) + "\n")
except BrokenPipeError:
    pass
