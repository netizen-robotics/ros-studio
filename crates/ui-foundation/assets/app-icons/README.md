# ROS Studio application icons

Original MIT-licensed geometric icons form one family: Launcher uses an application grid with an opening arrow, Visualizer uses a spatial cube, and Tools uses adjustable data controls. Shared petrol, mint and white colors remain recognizable in light and dark desktop environments.

The SVG files are the editable sources. Each application's 256-pixel PNG supplies native window icons and Launcher artwork; ICNS files supply macOS bundle icons; ICO files contain seven PNG representations (16, 24, 32, 48, 64, 128 and 256 pixels) for Windows resources. macOS ICNS contains 128, 256, 512 and 1024-pixel representations. The Launcher SVG also supplies `suite-44.png`, `suite-50.png` and `suite-150.png` for the Windows Store package manifest.

From the repository root, regenerate through project Python with the [icon generator](../../../../scripts/generate-app-icons.py):

```sh
scripts/dev .pixi/envs/default/bin/python scripts/generate-app-icons.py
```

The development renderer is `rsvg-convert`; runtime users need no renderer, Python or compiler. Keep generated derivatives in source so packaging does not depend on local design tools. See [suite packaging](../../../../docs/contributing/packaging.md) for how the product bundles these assets.

Windows build scripts embed icons and version/company metadata using the Windows SDK resource compiler. macOS packaging copies the ICNS asset into each app's Resources and declares it in Info.plist.
