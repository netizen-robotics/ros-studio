# Original mapping alignment fixture

The JSON is original synthetic data created for ROS Studio. No third-party map,
recording, image, or navigation dataset is included.

The row-major 3×2 map uses 0.5 m cells, a translated origin rotated 90 degrees,
and a second translated 90-degree source-to-fixed transform. Cell coordinates
refer to centers; the origin refers to the lower-left corner of cell zero.
The first cell center, pose, first path point, and the existing sensor-scene
first Cloud/scan point coincide at fixed `[1,3,3]`. The two rotations compose to
180 degrees for the pose's local X direction. Occupancy values cover unknown,
free, intermediate, and occupied cells. Unknown is not equivalent to free.

## Run the publisher

From the repository root, build the backend and follow the two-terminal environment and recorder setup in [testing](../../docs/contributing/testing.md#test-with-synthetic-ros-data). Use a fresh evidence directory:

```sh
scripts/dev .pixi/envs/default/bin/python scripts/mapping-scene-workflow.py \
  --rmw rmw_fastrtps_cpp \
  --evidence artifacts/feasibility/evidence/mapping-readme-local \
  --mode nominal --seconds 600
```

In Visualizer, choose Local computer, domain `187`, matching Fast DDS, fixed frame `map`, and System time. Use `/mapping_scene/tf` and `/mapping_scene/tf_static` for transforms. Explicitly show `/mapping_scene/map`, `/mapping_scene/pose`, `/mapping_scene/path`, `/mapping_scene/scan` and `/mapping_scene/cloud`. The [Robot fixture](../robot_visualizer/README.md) can be loaded separately; `/mapping_scene/status` is an independent Float64 peer. Stop the owned publisher with Ctrl-C. For a separate Cyclone DDS run, change the environment, `--rmw`, and UI middleware together and choose a new evidence path.

## Modes and expectations

`scripts/mapping-scene-workflow.py` publishes local synthetic data only after
its existing environment/isolation guards. `--mode map-retained` publishes the
map once with Reliable/TransientLocal QoS for late joining. `recover` omits the
mapping source's retained TF until `mode.txt` contains `ready`. Select other bounded data modes with `--mode`; retained transforms cannot be withdrawn.
No command, goal, service, action, or physical robot operation is performed.
Publisher call logs do not establish DDS delivery or native UI acceptance.

`map-duplicate` retransmits the exact original map header and geometry.
`map-changed-stamp` does likewise until the bounded `mode.txt` control contains
`ready`, then changes the first cell from unknown to occupied while preserving
that original stamp. Write the control atomically after observing the original
map. `map-historical` uses a fresh context, omits the static mapping edge, and
publishes current dynamic transforms while map headers are 20 seconds older.
Pose and Path headers remain current. It must not resolve by using latest TF.

Other modes cover missing transforms, malformed or over-limit Map/Pose/Path inputs, large or empty geometry, and path updates. Run the helper with `--help` for the complete list. The shared [sensor scene](../sensor_scene/README.md) describes cloud layout and scan alignment.
