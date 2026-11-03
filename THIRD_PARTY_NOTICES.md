# Third-party licenses and notices

The [MIT license](LICENSE) covers ROS Studio's original code, documentation, and synthetic fixtures. Third-party code, assets, and dependencies retain their own copyright notices and license terms.

## Bundled font assets

Noto Emoji and Noto Sans SC are distributed under the SIL Open Font License 1.1. Their original notices remain in the source tree and are embedded in the application:

- [Noto Emoji license](crates/ui-foundation/assets/fonts/OFL.txt)
- [Noto Sans SC license](crates/ui-foundation/assets/fonts/OFL-NotoSansSC.txt)
- [Font sources and attribution](crates/ui-foundation/assets/fonts/README.md)

The `studio-ui-foundation` package includes these assets, so its Cargo license expression includes both MIT and OFL-1.1.

## Adapted RViz map palettes

The map palette adaptation retains the upstream Bosch copyright and BSD redistribution terms. Preserve the [RViz map palette notice](crates/visualization/notices/rviz-map-palettes.txt) when redistributing the adaptation, including in binary distributions.

The `studio-visualization` package's Cargo license expression includes both MIT and BSD-3-Clause.

## Dependencies and packaged runtimes

Rust dependencies and bundled ROS/native runtimes have their own licenses. This page identifies the checked-in assets and adaptation above; it is not a complete license inventory for an application package. Assemble and review dependency notices for the exact shipped versions when [packaging applications](docs/contributing/packaging.md).
