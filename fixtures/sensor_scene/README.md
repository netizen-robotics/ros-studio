# Original combined sensor scene fixture

`cloud-layout.json` is original synthetic data authored for ROS Studio, covered by
the repository license. It describes a noncoplanar tetrahedron; no third-party
capture or physical robot data is included. All coordinates are metres.

XYZ fields are deliberately reordered and mixed FLOAT32/FLOAT64, with unused point
bytes and eight sentinel row-padding bytes. Little- and big-endian versions must
produce the same four source points. Translation `(1,2,3)` and yaw `pi/2` produce
the four expected fixed points recorded in the JSON. Scan A shares that transform;
its two range-1 rays at angles 0 and pi/2 coincide with cloud points 0 and 1.
Scan B has range-2 rays and transform `(-1,0,.5)`, yaw `-pi/2`, yielding
`(-1,-2,.5)` and `(1,0,.5)`. The original [Robot fixture](../robot_visualizer/README.md) is reused by reference.

## Run the publisher

From the repository root, build the backend and follow the two-terminal environment and recorder setup in [testing](../../docs/contributing/testing.md#test-with-synthetic-ros-data). Use a fresh evidence directory for each run:

```sh
scripts/dev .pixi/envs/default/bin/python scripts/sensor-scene-workflow.py \
  --rmw rmw_fastrtps_cpp \
  --evidence artifacts/feasibility/evidence/sensor-readme-local \
  --mode nominal --seconds 600
```

In Visualizer, choose Local computer, domain `187`, matching Fast DDS, fixed frame `map`, and System time. Use `/sensor_scene/tf` and `/sensor_scene/tf_static` for transforms. Explicitly show `/sensor_scene/scan_a`, `/sensor_scene/scan_b` and `/sensor_scene/cloud`; load the articulated Robot model separately if needed. `/sensor_scene/status` is an independent Float64 peer. Stop the owned publisher with Ctrl-C. For a separate Cyclone DDS run, change the environment, `--rmw`, and UI middleware together and choose a new evidence path.

## Modes and limits

`scripts/sensor-scene-workflow.py` publishes only after the existing localhost /
domain187 / explicit matching middleware / unused-evidence guards pass. Normal
clouds have 4 points and 144 bytes. `cloud-pressure` uses exactly 8192 points /
256 KiB raw bytes at a nominal 100 Hz, while scans/TF/peer remain nominal 5 Hz.
This is an optional bounded pressure input, not a performance pass.

`recover` initially omits only cloud TF; write `ready` to the recorded `mode.txt`
to publish its retained static transform. That transition is one-way in a TF cache.
Write the control atomically (write a temporary file, then rename it to `mode.txt`).
`missing` never supplies cloud TF. `cloud-malformed` deliberately publishes an
invalid row_step while Robot/scans/peer remain valid. A publisher cannot prove a
shared actor's native-TF failure or cleanup: inject those faults in backend tests.
No ROS command, motion, goal or joint-state command is published.

Additional modes cover empty clouds, skipped nonfinite points, RGB/RGBA attributes, and unsupported field layouts. Run the helper with `--help` for its complete mode list; `--big-endian` switches byte order. These are intentionally bounded or invalid test inputs, not additional reception guarantees.
