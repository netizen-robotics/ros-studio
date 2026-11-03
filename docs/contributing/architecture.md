# Understand the application and ROS boundaries

ROS Studio separates native UI, a versioned local protocol, and distro-specific ROS backends. This keeps ROS work outside active UI frames and prevents incompatible ROS distributions from sharing a process.

## Applications and shared crates

| Component | Responsibility |
| --- | --- |
| `studio-visualizer` | Spatial scene, displays, Image and calibrated Camera companions |
| `studio-tools` | ROS graph inspection, requests, Messages, Plot, and Image |
| `studio-launcher` | Suite entry point; launches bundled tools without downloading executable code |
| `studio-desktop` | Retained combined compatibility application |
| `studio-client-core`, `studio-ui-foundation` | Shared session/UI behavior and diagnostics |
| `studio-protocol` | Serialized local commands, events, and compatibility contracts |
| `studio-backend` | Standard ROS client-library boundary and native message handling |
| `studio-workspace`, `studio-network` | Saved intent and machine-local network configuration |

Visualizer and Tools each own an independent backend child process. The suite packages all three frontends with one shared runtime. Windows Store owns package updates; macOS initially uses complete-app replacement. Launcher has no ROS dependency, and closing it does not close the other applications. Rust calls standard ROS C/C++ libraries through the backend boundary.

Native presentation uses the [shared UI conventions](ui-design.md) across all
four applications. Theme and font configuration do not own application state.

## Sessions and bounded data

The frontend exchanges versioned commands and events with its backend. Commands are explicit and generation-scoped; stale work must not redirect a replacement session. Shutdown requests cancellation and waits for owned workers and backend cleanup outside active UI frames.

Bounded queues, coalesced snapshots, sample limits, and drop counters limit application-side retention. DDS allocations and network loss are separate from application queue statistics. Report incomplete or stale data rather than presenting it as continuous or current.

Discovery, source drafts, and active subscriptions have separate lifecycles. Tools operations require an explicit operation button and a valid connected draft. Workspace restore and reconnect retain inactive intent and never replay commands.

## Compatibility and persistence

The current paired protocol is `session21`, with Image2 and Camera2 capabilities for larger bounded images. Session20 and its Image1/Camera1 limits remain frozen for compatibility checks; an older backend must be rebuilt with the frontend. Visualizer writes `workspace19`; Tools writes `workspace9`. These are format identifiers, not public application release versions.

Version an intentionally changed contract and test compatibility. Do not reinterpret a frozen format silently. Older compatible workspace formats import through preview, preserving the original and restoring inactive.

Machine-local network preferences live outside workspaces. Managed profiles apply only to the owned backend process. Keep ROS distributions isolated, including their type-support libraries.

## Development fixtures and evidence

Protocol fixtures exercise the UI without ROS. Synthetic publishers and actual backend tests exercise the ROS boundary. Native windows, clean-machine packages, LAN behavior, and physical hardware each need their own evidence.

Store generated reports and captures under ignored `artifacts/`. The [testing guide](testing.md) explains indexing and sharing limits. Internal product plans and historical acceptance records are not dependencies of a fresh public checkout.
