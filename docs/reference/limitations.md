# Platform status and known limitations

ROS Studio is a development preview. Build success and local tests do not establish production installation, every ROS configuration, or physical robot acceptance.

## Platform and distribution status

| Environment | Current evidence |
| --- | --- |
| macOS on Apple Silicon, ROS 2 Jazzy | Primary development environment with bounded native and local ROS checks |
| Windows 11 x64, ROS 2 Jazzy | Native development builds and selected application/ROS checks; full installation and direct-launch LAN qualification remain open |
| Linux, Intel macOS, other ROS distributions | Dependency environments or selected feasibility work exist; full native application support is not qualified |

The one-product release paths (Microsoft Store MSIX on Windows and signed/notarized app/DMG on macOS) still require qualification. Store identity reservation/certification, macOS signing, dependency notice review, clean-machine installation and update/uninstall remain incomplete. Store builds exclude elevated firewall repair; packaged localhost/LAN behavior remains unqualified. Legacy per-tool catalog/install code is retained in source but is not the active delivery path.

## ROS and display restrictions

Current restrictions include:

- Mixed Fast DDS/Cyclone DDS behavior and general LAN interoperability remain unqualified. Use matching middleware when diagnosing a problem.
- Some maximum-size best-effort Map cases pass only with diagnostic localhost profiles. Those results do not establish default or LAN delivery.
- Map updates do not carry a shared base revision. Middleware can lose updates without a reported application drop, so continuity is not guaranteed.
- A late retained DiagnosticArray sample remains unresolved with Cyclone DDS in both applications.
- Automatic endpoint QoS preparation and simulation-clock handling remain incomplete. The scene defaults to System time and fixed frame `map`.
- Calibrated Camera does not include raw-image undistortion, compressed/depth transports, or simulated-time synchronization.
- Full accessibility, sustained maximum-load combinations, and exhaustive native input/lifecycle coverage remain unqualified.

## macOS quitting with a file chooser open

A pending native file chooser can prevent the expected Cmd-Q cleanup path. Close or cancel the chooser before quitting. Native-menu Quit is an alternative in the recorded workaround; report a remaining failure with the chooser state and application version.

## Features outside the current applications

Bag playback/recording, arbitrary plugins, `.rviz` configuration import, and multi-series Plot redesign are not available. ROS Studio does not provide a dedicated navigation-control workflow.

Tools can publish messages, call services, send goals, and write parameters through explicit operation buttons when connected with a valid draft. Those operations can affect a connected system; preview status does not restrict them to synthetic targets automatically.
