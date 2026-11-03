# ROS Studio documentation

Use these guides to build ROS Studio, connect to a ROS 2 system, and inspect its data. For the current distribution and platform status, read [known limitations](reference/limitations.md).

ROS Studio is a development preview containing independent Visualizer, Tools, and Launcher processes in one product. Start with [build from source](contributing/building.md); signed macOS and Microsoft Store release paths still require qualification.

## Start using the applications

| Guide | Task |
| --- | --- |
| [Get started](getting-started.md) | Open an application, connect, and receive your first data |
| [Use Visualizer](guides/visualizer.md) | Configure a scene and inspect spatial displays |
| [Use Tools](guides/tools.md) | Inspect the ROS graph and perform explicit interactions |
| [Configure networking](guides/networking.md) | Choose local, LAN, or custom discovery settings |
| [Save and restore workspaces](guides/workspaces.md) | Reuse configuration without restarting live operations |
| [Troubleshooting](troubleshooting.md) | Diagnose connection, reception, and display problems |
| [Known limitations](reference/limitations.md) | Check platform status and current restrictions |

## Contribute to the project

| Guide | Task |
| --- | --- |
| [Contributing](../CONTRIBUTING.md) | Prepare a focused change |
| [Build from source](contributing/building.md) | Set up the project environment and launch the apps |
| [Style native UI](contributing/ui-design.md) | Use shared colors, typography and components |
| [Test a change](contributing/testing.md) | Run focused checks and synthetic ROS fixtures |
| [Architecture](contributing/architecture.md) | Understand application, protocol, and backend boundaries |
| [Package applications](contributing/packaging.md) | Assemble experimental packages and understand release verification |
| [Configure release signing](contributing/signing.md) | Set up macOS signing/notarization and Windows Store identity |

## Synthetic spatial fixtures

Use the isolation and evidence setup in [test with synthetic ROS data](contributing/testing.md#test-with-synthetic-ros-data) before starting a publisher. Fixture descriptions explain expected geometry and failure modes; they do not establish native rendering or hardware acceptance.

| Fixture | Contents |
| --- | --- |
| [Robot](../fixtures/robot_visualizer/README.md) | Articulated URDF, STL, OBJ, COLLADA, textures, and collision geometry |
| [Sensor scene](../fixtures/sensor_scene/README.md) | Robot, two scans, and bounded PointCloud2 layouts |
| [Mapping scene](../fixtures/mapping_scene/README.md) | Aligned Map, Pose, Path, Robot, scan, and cloud data |
