# Build ROS Studio from source

Build the frontends and backend from the same checkout. The project-local Pixi environment supplies Rust and ROS dependencies; you do not need to source a system ROS setup script for this development path.

## Prerequisites

Install [Pixi](https://pixi.sh/) and the platform's native compiler tools. On macOS, use Xcode Command Line Tools. On Windows, use Visual Studio C++ tools and the Windows SDK. The locked environment uses Rust 1.92.0 and ROS 2 Jazzy.

macOS on Apple Silicon is the primary development environment. The root manifest also contains other platform environments; their presence is not a support guarantee. Check [platform status](../reference/limitations.md).

## Build on macOS or Linux

From the checkout root, install the locked environment and build the workspace:

```sh
export PIXI_CACHE_DIR="$PWD/.cache/pixi"
export RATTLER_CACHE_DIR="$PWD/.cache/rattler"
pixi install --locked
scripts/dev cargo build --workspace --locked
```

The `scripts/dev` wrapper sets project-local caches and the Jazzy environment. On Linux, install the desktop development libraries required by the windowing backend; the [native CI workflow](../../.github/workflows/native-feasibility.yml) lists its Ubuntu packages. A prepared CI job is not a passed native support result.

Launch an application:

```sh
scripts/dev cargo run --locked -p studio-visualizer
scripts/dev cargo run --locked -p studio-tools
scripts/dev cargo run --locked -p studio-launcher --bin studio-launcher
```

Run each command separately. Add `--release` to build and run commands when evaluating large scenes. `studio-desktop` is the retained combined compatibility application.

The applications look for `studio-backend` beside their executable. `STUDIO_BACKEND` overrides that path for explicit development fixtures. Keep macOS `DYLD_*` library overrides unset for native frontend launches.

## Build on Windows

Open an x64 Visual Studio C++ Developer Command Prompt in the checkout. Set project-local caches and install the environment:

```bat
set PIXI_CACHE_DIR=%CD%\.cache\pixi
set RATTLER_CACHE_DIR=%CD%\.cache\rattler
set CARGO_HOME=%CD%\.cache\cargo
pixi install --locked
```

Build the applications for the explicit target so static CRT flags do not affect host build tools:

```bat
pixi run --locked cargo build --locked --workspace ^
  --target x86_64-pc-windows-msvc
```

For a development launch, select the environment's ROS prefix and use the matching target directory:

```bat
set ROS_DISTRO=jazzy
set AMENT_PREFIX_PATH=%CD%\.pixi\envs\default\Library
pixi run --locked target/x86_64-pc-windows-msvc/debug/studio-tools.exe
```

Replace `studio-tools.exe` with `studio-visualizer.exe` to open Visualizer. These commands describe development builds, not clean-machine installation.

## Keep ROS distributions separate

The root environment is Jazzy. `environments/humble` and `environments/lyrical` contain separate feasibility environments. Do not combine their prefixes or libraries in one process.
