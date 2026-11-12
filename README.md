# ROS Studio

Visualize ROS 2 data and inspect a running ROS graph from native desktop applications built in Rust.

ROS Studio brings spatial visualization and ROS inspection into two focused workspaces, included in one ROS Studio installation. Use **Visualizer** to see your robot and sensor data. Use **Tools** to inspect messages, plot values, and work with topics, services, actions, nodes, and parameters.

**Development preview:** build from source or generate the [preview DMG, Windows setup and Ubuntu DEB](docs/contributing/release-ci.md#download-the-first-preview-packages) to try ROS Studio. Signed macOS suite packages and Windows Microsoft Store distribution are not yet qualified for public release. The primary tested development environment is macOS on Apple Silicon with ROS 2 Jazzy. Read the [platform status and known limitations](docs/reference/limitations.md) before use.

## Choose your workspace

| Application | What you can do |
| --- | --- |
| **Visualizer** | View robot models, coordinate frames, laser scans, point clouds, maps, poses, paths, odometry, covariance, and markers in a shared 3D scene, with Image and calibrated Camera companions. |
| **Tools** | Browse the ROS graph, inspect messages and images, plot numeric fields, and explicitly publish, call services, send action goals, or edit parameters. |
| **ROS Studio Launcher** | Open the included Visualizer and Tools. The suite installs and updates as one product. |

Visualizer and Tools run independently, each with its own ROS backend. You can open either application directly.

## Explore your ROS system

Connect to your ROS domain, browse the available data, and choose what to receive. Visualizer combines displays in one scene; Tools keeps inspection and interaction tasks together.

The applications start disconnected. Browsing does not activate subscriptions, and reconnecting does not replay commands. In Tools, a connection and a valid draft enable the named operation buttons; each request requires an explicit action. Saved workspaces restore inactive, so you decide when to receive data or interact with a system.

## Try it from source

On macOS, install [Pixi](https://pixi.sh/) and Xcode Command Line Tools, then open a terminal in this checkout. The locked environment supplies Rust 1.92.0 and ROS 2 Jazzy. Install it with project-local caches, then build the applications and their backend:

```sh
export PIXI_CACHE_DIR="$PWD/.cache/pixi"
export RATTLER_CACHE_DIR="$PWD/.cache/rattler"
pixi install --locked
scripts/dev cargo build --workspace --locked
```

Start ROS Studio through Launcher:

```sh
scripts/dev cargo run --locked -p studio-launcher --bin studio-launcher
```

Launcher is the suite entry point for opening the bundled Visualizer and Tools. Windows distribution offers an unsigned standalone setup preview and targets Microsoft Store MSIX; macOS distribution targets one signed app in a DMG. These release packages still require qualification. Direct app launches remain available for development. The project environment supplies the development dependencies. See [build from source](docs/contributing/building.md) for prerequisites and Windows instructions, then follow [get started](docs/getting-started.md).

## Learn and contribute

- [Documentation](docs/README.md): user guides and technical references
- [Use Visualizer](docs/guides/visualizer.md): add displays and inspect spatial data
- [Use Tools](docs/guides/tools.md): inspect data and perform explicit interactions
- [Test with synthetic data](docs/contributing/testing.md): protocol fixtures and isolated local ROS publishers
- [Troubleshooting](docs/troubleshooting.md): discovery, reception, transforms, and diagnostics
- [Contributing](CONTRIBUTING.md): build, test, and propose changes

ROS Studio is experimental. Review requests and target systems before enabling interactions, and use synthetic data when evaluating the applications.

## License

See the [Code signing policy](docs/contributing/code-signing-policy.md) for
current signature status, and [privacy and network behavior](docs/reference/privacy.md)
for how the bundled applications handle local and ROS data.

ROS Studio's original code and documentation are licensed under [MIT](LICENSE). Third-party components retain their own licenses; see [third-party notices](THIRD_PARTY_NOTICES.md).
