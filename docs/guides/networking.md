# Connect to a local or LAN ROS system

Choose a discovery mode, domain, and middleware before connecting Visualizer or Tools. Network settings belong to each application and machine, separately from saved workspaces.

Open **Network settings** beside Connect. Its compact editor overlays the workspace without reducing the work area. Choose a discovery mode and, for LAN, an adapter; **Adapter options** includes VPN and virtual adapters. Choose **Save settings**, then Connect explicitly. While connected or starting, disconnect before editing. Troubleshooting notes are under **Connection help**.

Development fixtures can replace the ROS backend. Visualizer labels the built-in fixture **Demo · synthetic**; its topics are generated locally and cannot show a robot. The standard backend is labelled **ROS 2 · Observe**. ROS topics can come from a physical robot or a simulator, so verify the publisher and matching connection settings before interpreting the data as hardware observations.

## Choose a discovery mode

| Mode | Use it when |
| --- | --- |
| **Local computer** | Publishers run on the same computer; use this for synthetic tests |
| **LAN** | Publishers run on your physical local network; select the intended adapter |
| **Custom configuration** | You need inherited discovery settings or an external middleware XML profile |

LAN selection prefers an eligible physical adapter. If several adapters are available, choose deliberately. A VPN, WSL, or container adapter can advertise addresses that other ROS participants cannot reach.

The application prepares managed profiles for its backend process. It does not replace your global DDS configuration. If inherited settings conflict, review the available choices rather than combining unrelated profiles.

## Match the publisher

Set the same ROS domain on both sides. Domain `0` is the default; synthetic fixtures use `187` explicitly. Select the publisher's middleware when testing: Fast DDS with Fast DDS, or Cyclone DDS with Cyclone DDS. Mixed-middleware behavior remains unqualified.

ROS discovery uses Data Distribution Service (DDS). Discovery and sample delivery can fail separately because of routing, multicast, firewall, or QoS settings. A reachable host or a successful ping does not prove ROS data delivery.

## Recover from a network change

Disconnect before changing network settings. If an adapter or address disappears, correct the selection and reconnect explicitly. Readers stay inactive until you restart them.

On Windows, a backend-scoped inbound UDP allowance may be necessary. Microsoft Store builds exclude the elevated helper and automatic firewall repair; use Windows Security or ask the machine administrator to resolve blocked LAN access. Legacy non-Store managed firewall installation requires a properly signed build and remains experimental. Check the backend path, selected adapter, network profile, and intended peers. Packaged localhost/LAN behavior still requires qualification; see [packaging](../contributing/packaging.md).

On macOS, review **System Settings → Privacy & Security → Local Network** if access is denied. A custom profile inherited by the frontend may require closing and relaunching the application.

For troubleshooting steps, see [connection and reception problems](../troubleshooting.md).
