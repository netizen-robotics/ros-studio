# Get started with ROS Studio

Start **ROS Studio** (Launcher), open the included Visualizer or Tools, then connect to a ROS 2 domain and explicitly select the data you want to receive. Start with a synthetic publisher or a system you are authorized to inspect.

## Open your workspace

ROS Studio 0.1.0 targets one installed suite on macOS Apple Silicon and Windows x64; for the current development preview, [build from source](contributing/building.md).
Choose **Open** under Visualizer or Tools. Both tools are included and work offline;
closing Launcher leaves their windows running. The appearance icon cycles System → Light → Dark. The information button opens the temporary documentation destination at [Netizen Robotics](https://www.netizenrobotics.com). Visualizer and Tools also have an appearance button outside their View menus.

Windows releases target Microsoft Store installation and updates. macOS releases
use one signed app in a DMG; updating replaces the complete app after closing its
windows. These release paths still require [qualification](reference/limitations.md).
If application files are missing, reinstall the complete suite and choose **Retry**.
For source builds, compile all frontends into the same target directory before
starting Launcher.

## Before you connect

Build the applications and backend using [build from source](contributing/building.md). You need a running ROS 2 publisher to see live data. For a local test publisher, follow [test with synthetic ROS data](contributing/testing.md#test-with-synthetic-ros-data).

Check the publisher's domain, middleware, topic type, and quality of service (QoS). ROS Studio uses domain **0** by default. The synthetic test environment uses domain **187** and localhost-only discovery.

## Connect to a ROS domain

1. Open **Visualizer** or **Tools**.
2. Open **Network settings** and choose **Local computer** or your LAN adapter.
3. Set the domain to match your publisher.
4. Select the matching Fast DDS or Cyclone DDS middleware.
5. Choose **Connect** and wait for discovery.

A connection discovers interfaces; it does not start receiving every topic. If nothing appears, follow [configure networking](guides/networking.md).

## Receive data

In Visualizer, find a displayable topic and use its **Show** eye action. Inspect the display's source, QoS, status, and appearance. Spatial data also needs a valid transform into the scene's fixed frame; read [use Visualizer](guides/visualizer.md).

In Tools, open a topic task and explicitly enable reception in **Messages**, **Plot**, or **Image**. Select a supported numeric field for a plot. Opening a task or changing a selection does not replace an active subscription; read [use Tools](guides/tools.md).

## Stop and return later

Stop reception or disconnect before changing the domain or middleware. A reconnect leaves readers inactive and does not replay publishing, service requests, action goals, or parameter writes.

Save a [workspace](guides/workspaces.md) to retain your configuration. It does not save live sensor data.

Use **File → Exit** in Visualizer or Tools, or the native window close action, to close that application and clean up its workers and owned backend processes. On macOS, Cmd-Q also exits. Launcher has no File menu; closing or quitting it leaves the other application windows running.
