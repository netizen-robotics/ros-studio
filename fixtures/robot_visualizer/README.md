# Original articulated robot fixture

These URDF, mesh, material and texture assets were authored for ROS Studio as synthetic test data.
They contain no third-party geometry. They inherit the repository license.
No robot hardware, xacro execution, ROS package lookup or network fetch is needed.

## Fixture assets

| URDF | Purpose |
| --- | --- |
| [articulated.urdf](articulated.urdf) | Five links with primitives, ASCII/binary STL, package mapping, and an invisible visual |
| [topic.urdf](topic.urdf) | Two-link description used by topic-based robot setup fixtures |
| [obj.urdf](obj.urdf), [collada.urdf](collada.urdf) | OBJ/MTL and COLLADA material panels |
| [textured.urdf](textured.urdf), [collada-textured.urdf](collada-textured.urdf) | Local checker texture on OBJ and COLLADA meshes |
| [rich.urdf](rich.urdf) | Textured meshes, translucent and opaque primitives, and a textured cylinder |
| [collision.urdf](collision.urdf) | Separate visual/collision geometry, including a collision-only link |

The following geometry expectations describe `articulated.urdf`. Load it and explicitly map package `fixture_robot` to this directory.
The ASCII mesh resolves relative to the URDF; the binary mesh uses that package map.
The binary STL intentionally begins with `solid` to test binary/ASCII detection.
Both STL files encode the original nondegenerate triangle `(0,0,0),(1,0,0),(0,1,1)`.

Description order is `base`, `arm`, `tip`, `ascii_mesh`, `binary_mesh`. The five
links exercise box, cylinder, sphere, ASCII STL and binary STL respectively.
Base also contains a completely hidden alpha-zero visual. Opaque colors are red,
green, blue, magenta and yellow; fractional alpha is not part of this fixture.

## Run the articulated publisher

From the repository root, build the backend and follow the two-terminal environment and recorder setup in [testing](../../docs/contributing/testing.md#test-with-synthetic-ros-data). In the publisher terminal, choose a fresh evidence directory and run:

```sh
scripts/dev .pixi/envs/default/bin/python scripts/robot-ros-workflow.py \
  --rmw rmw_fastrtps_cpp \
  --evidence artifacts/feasibility/evidence/robot-readme-local \
  --mode recover --seconds 600
```

In Visualizer, connect with matching Fast DDS, Local computer, and domain `187`. Load the articulated model with the package map above, use fixed frame `map` and System time, and select `/robot_demo/tf` and `/robot_demo/tf_static`. Enable reception explicitly. Status peers are `/robot_demo/status` and `/robot_demo/status_two`.

`nominal` includes the tip transform from startup; `missing` never supplies it. In `recover`, atomically write `ready` to the evidence directory's `mode.txt` to release the tip transform once. Writing `waiting` afterward cannot remove retained TF; use a fresh publisher/backend session to repeat the missing-transform case. Stop the owned publisher with Ctrl-C. To test Cyclone DDS separately, change both the environment and `--rmw` to `rmw_cyclonedds_cpp`, select Cyclone DDS in the UI, and use a fresh evidence path.

## Analytic expectations

The ASCII visual applies scale `(2,1,.5)`, visual yaw `pi/2`, then visual translation
`(.2,0,0)`. Its vertex `(1,0,0)` becomes `(.2,2,0)` in link coordinates.
External link yaw `pi/2` and translation `(1,2,.2)` must produce `(-1,2.2,.2)`.
Its vertex `(0,1,1)` becomes `(-.8,0,.5)` in link coordinates. Apply visual origin
once, then only the external fixed-frame/link transform; joint origins are metadata.

The guarded `scripts/robot-ros-workflow.py` publisher uses fixed-frame `map` poses:
base `(0,0,0)` identity; arm `(0,0,.2)` rotating about Y; tip at the arm-transformed
`(0,0,.6)` with the same rotation; ASCII and binary links identity. At arm angle zero
the tip sphere center is `(0,0,.85)`. At `pi/2` it is `(.65,0,.2)`, including its
visual origin. Omit only the leaf tip TF to test partial hiding and recovery while
other links and an independent status peer continue. These are analytic synthetic test poses,
not evidence of physical motion or a new acceptance run.

Independent tests generate malformed XML, invalid trees, missing/escaping paths,
oversized files and invalid materials in temporary directories. Runtime inputs and
asset loading remain inactive until explicit user actions in the application.
