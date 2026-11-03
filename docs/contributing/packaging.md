# Package ROS Studio

ROS Studio is one installed product at version **0.1.0**. Launcher is its entry
point; Visualizer and Tools are bundled tools with independent windows, processes
and backend sessions. Their workspaces remain separate. One package includes one
shared ROS 2 Jazzy runtime. Runtime users need no ROS installation, Pixi, Python
or compiler.

Packaging does not establish Store approval, signing, clean-machine execution,
LAN behavior or public release qualification. Current packages remain previews.
Keep the MIT license and reviewed dependency notices with every distribution.

## macOS Apple Silicon

Build Launcher, Tools, Visualizer and the ROS backend from the same locked
checkout. Assemble the relocated runtime using `scripts/package-macos.py`, then:

```sh
scripts/dev .pixi/envs/default/bin/python scripts/package-suite-macos.py \
  --runtime PATH_TO_RELOCATED_JAZZY_RUNTIME \
  --binaries target/release \
  --destination artifacts/ros-studio-macos-preview
```

The output is one `ROS Studio.app` and a DMG with an Applications shortcut. Launcher lives in `Contents/MacOS`; Visualizer and Tools are nested native app
bundles under `Contents/Applications`. Each tool has a backend executable, while
shared libraries and ROS resources live once in `Contents/Frameworks` and
`Contents/Resources`, reached through contained relative links. Launcher activates
the nested apps through LaunchServices. Both tools remain open when Launcher exits.
The bundle keeps the Launcher identifier `dev.rosstudio.launcher`.

Default signing is ad-hoc and is for development only. Production requires
`--sign-identity`, `--notary-profile` and `--notices`; see [signing](signing.md).
Initial macOS updates replace the complete app. No automatic updater is included.
Close all ROS Studio windows before replacing the app. Existing per-tool managed
installations are not automatically migrated or deleted.

## Windows 11 x64 — Microsoft Store MSIX

Reserve the product in Partner Center and use its exact package identity name,
publisher distinguished name and publisher display name. Do not invent these
values for a real submission. Build all frontends in Store mode:

```powershell
pixi run --locked cargo build --locked --release --target x86_64-pc-windows-msvc -p studio-launcher -p studio-tools -p studio-visualizer -p studio-backend --features studio-launcher/store-distribution,studio-tools/store-distribution,studio-visualizer/store-distribution
```

Then run `scripts/package-suite-windows.py --help` in the project environment.
Required inputs are `--runtime` (the Jazzy Windows runtime prefix), `--binaries`,
`--destination`, `--notices`, `--makeappx` (Windows SDK), `--identity-name`,
`--publisher` and `--publisher-display-name`.

The packager checks the Store build configuration of all three frontends. It
produces one x64 MSIX at version **0.1.0.0**, with one ROS Studio Start entry and a
shared runtime. The generated package declares `runFullTrust`,
`internetClientServer` and `privateNetworkClientServer`. These declarations do
not prove network reception or approval of the restricted capability.

The elevated firewall helper and legacy bootstrap installer are excluded.
Store builds hide automatic firewall repair and reject helper activation. Network
settings remain available; blocked LAN access must be resolved through Windows
Security or the machine administrator. Qualify this workflow before release.

The local MSIX is an **unsigned submission candidate**, not a public installer.
The Store handles package signing after certification and owns installation,
updates and uninstall. Launcher does not download or replace tool executables.
No private Authenticode certificate or catalog key is required for this Store
route. The generated MSIX is not independently trusted for GitHub sideloading.

## Release qualification

Test the actual installed product on clean target machines: first launch, both
tools opening together, Launcher exit without closing tools, complete exit and
backend cleanup, save/reopen, upgrade with data retained, uninstall and network
permissions. Exercise localhost synthetic ROS peers and intended LAN scenarios
separately. Check Store package certification, runtime library relocation,
redistribution licenses and privacy before publication.

The old `package-apps-*` tools, catalog verifier and bootstrap remain legacy
source for compatibility and regression coverage. They are not the active suite
delivery path. `publish-apps-release` is retired; publishing the new product
requires a separately reviewed Store submission or signed macOS release upload.
