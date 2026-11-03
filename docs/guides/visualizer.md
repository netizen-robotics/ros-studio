# Inspect spatial data with Visualizer

Use Visualizer to combine robot geometry, coordinate frames, and sensor data in one scene. Image and calibrated Camera companions let you inspect camera input beside that scene.

## Set up a scene

Connect using the [getting started guide](../getting-started.md). The left browser has **Topics** and **Scene** tabs. The right inspector shows settings for the selected display.

The scene defaults to fixed frame `map` and System time. The frame selector retains a bounded, sorted inventory for the current connection and TF inputs; older observations are marked separately from current availability. Use **Scene** to change the fixed frame, then apply your changes. Active spatial displays resume automatically after their previous readers confirm cleanup; stopped displays stay stopped and raw Image continues receiving. Your publisher must supply the transforms needed to place data in that frame. Missing or outdated transforms show a status message and hide geometry when its bounded retry interval expires.

When a replacement Map arrives just ahead of its transform, Visualizer keeps the last complete, correctly transformed map during the bounded retry interval (up to one second since the last completed delivery). New cells appear only after their acquisition-time transform is available. A timeout, invalid data, or lost update continuity clears the map. **Map details** contains timestamps, dimensions, and counters; appearance controls remain visible separately.

Other acquisition-time displays also retain complete geometry through brief replacement/TF gaps: Scan, PointCloud2, Pose, Path, Odometry, and covariance use a one-second interval; calibrated Camera uses two seconds. Camera retains the complete previous image/calibration/transform pair. Marker and MarkerArray replacements wait up to one second for their transforms; deletion, lifetime expiry, and fault controls still remove affected markers immediately. Repeated unresolved input cannot extend these intervals indefinitely. A new source or continuity reset clears cached geometry, and turning a display off clears it immediately.

Robot and TF displays use latest-available transforms with their existing stale/missing status rules. Raw Image, Messages, and Plot do not require scene transforms. Temporary retention does not establish fresh data or network delivery; new geometry never uses the previous sample's transform.

Simulation-clock handling and automatic endpoint QoS preparation remain incomplete. Configure QoS explicitly when the publisher requires it.

## Choose and inspect topics

Find a compatible topic in **Topics**. Select its name to inspect settings without starting reception. Use its eye to start or stop reception. Each topic has one active display mode; choose the mode in the right inspector. Switching an active mode waits for the previous reader to stop. Saved settings remain available when you switch back. Changing a stopped topic’s mode keeps reception stopped.

Use **Refresh** to request a new topic list without replacing active readers or clearing the current list. Configured topics stay reachable when discovery loses them.

Available displays include:

- Robot models and TF coordinate frames
- LaserScan and PointCloud2
- Maps and map updates
- Poses, paths, odometry, and covariance
- Marker and MarkerArray geometry
- Raw Image and calibrated Camera

Select a compatible robot-description topic to start robot-model setup and review its local asset roots. Camera calibration needs compatible image and CameraInfo sources; CameraInfo is an input to Camera, not a standalone display.

Image and calibrated Camera accept `mono8`, `rgb8`, `bgr8`, `rgba8`, and `bgra8` images up to 2048 pixels per axis. Both the raw payload (including row padding) and decoded RGBA pixels must fit within 16 MiB. This includes ordinary 640×480, 1280×720, and 1920×1080 images without resizing their pixels or changing calibration. Oversized, incomplete, and unsupported images produce an explicit error. Image transport and rendering use bounded queues; sustained high-resolution frame rates and combinations of displays remain subject to drops and performance limits.

For raw depth topics, use **Image**. It also accepts `16UC1` (millimeters) and `32FC1` (meters), respecting byte order and row padding. Depth uses a fixed 0–10 m grayscale range: nearby pixels are lighter; values at or above 10 m are black. Zero, negative, NaN, and infinite depth values are transparent. This is a 2D depth preview; calibrated Camera still requires one of the five color/mono encodings above.

PointCloud2 inputs are limited to 8,192 source points and 256 KiB of raw data per sample. Larger inputs need a smaller or downsampled publisher output. Missing acquisition-time transforms produce waiting/error states; transforms are never guessed.

## Adjust the view

**Views** stays at the top of the right sidebar while display settings scroll below it. Choose **Orbit** or **Top down** from the mode selector. Drag the distance or height slider to zoom, or edit its value in meters. Use **Fit visible**, **Focus selected**, and **Reset** to frame the scene; **Camera…** opens precise camera settings and saved views without moving the display inspector.

In compact windows, Views stays in a single row above the scene, with framing actions inside **Camera…**. Select a display to adjust its source and appearance in the inspector; use **View → Right sidebar** to reopen that inspector. Views remains available even when display details are closed or an older workspace had hidden it.

For TF displays, filter the frame list by name and select a row for its parent, timestamps, and transform status. Hover a truncated row to read its full name and parent.

Changing a draft does not redirect a running source. Review the changes and choose **Apply** when available. Replacement waits for the previous source to stop.

## Manage reception

Use the topic eye to control reception. Turning it off waits for cleanup and clears its live content while retaining settings. Pausing a companion is local inspection; reception can continue in the background.

A workspace can contain up to 32 configured displays, including up to three Maps. Per-display resource limits and a shared reader limit still apply. These configuration limits do not guarantee performance for every combination of sources.

See [known limitations](../reference/limitations.md) for map continuity, camera, timing, and accessibility restrictions.

At short window heights, the Image/Camera companion uses a larger effective share
of the workspace so its controls leave room for the image. The scene remains
above it; resizing does not start or replace reception.
