# Bundled font fallbacks

Both bundled fonts are unmodified. The shared loader prefers available platform UI and monospace fonts, then egui's defaults, then Noto Sans SC and Noto Emoji in that order. A semibold family uses an available system semibold face with bundled fallbacks. If a system face is absent or cannot be parsed, the bundled fonts remain available. See [the shared loader](../../src/fonts.rs).

The complete upstream notices are embedded and accessible in the Visualizer and retained desktop About dialogs under **Font licenses**, and in Tools under **Font license**. Neither bundled font requires a system install or runtime download.

## Noto Sans SC

Unmodified static Noto Sans SC Regular OpenType font (CFF outlines). Copyright 2014–2021 Adobe (http://www.adobe.com/). Noto is a trademark of Google Inc. Distributed under the SIL Open Font License 1.1; the full upstream license is retained in [OFL-NotoSansSC.txt](OFL-NotoSansSC.txt), with the font's copyright notice also embedded in About. This fallback supplies Simplified Chinese glyphs; it does not promise comprehensive Japanese, Korean, Traditional Chinese or universal Unicode coverage.

Pinned source: `notofonts/noto-cjk` commit `f8d157532fbfaeda587e826d4cd5b21a49186f7c`.

- [Original font](https://raw.githubusercontent.com/notofonts/noto-cjk/f8d157532fbfaeda587e826d4cd5b21a49186f7c/Sans/SubsetOTF/SC/NotoSansSC-Regular.otf), stored as `NotoSansSC-Regular.otf`: 8,331,336 bytes, SHA-256 `faa6c9df652116dde789d351359f3d7e5d2285a2b2a1f04a2d7244df706d5ea9`.
- [Original license](https://raw.githubusercontent.com/notofonts/noto-cjk/f8d157532fbfaeda587e826d4cd5b21a49186f7c/Sans/LICENSE): SHA-256 `6a73f9541c2de74158c0e7cf6b0a58ef774f5a780bf191f2d7ec9cc53efe2bf2`.

The static Regular file avoids relying on a variation-axis override unavailable through current egui FontData. The existing ab_glyph/ttf-parser stack supports CFF outlines; focused raster tests verify the actual embedded file.

## Noto Emoji

Unmodified monochrome Noto Emoji variable TrueType font, used only after egui's existing UI and monospace fonts. Copyright 2013 Google LLC. Distributed under the SIL Open Font License 1.1; the full upstream notice is retained in [OFL.txt](OFL.txt) and accessible in the app under About → Font license.

Pinned source: `google/fonts` commit `8b0a1d0f5983c89bc2b93f1b5fb55f9e252744b5`.

- [Original font](https://raw.githubusercontent.com/google/fonts/8b0a1d0f5983c89bc2b93f1b5fb55f9e252744b5/ofl/notoemoji/NotoEmoji%5Bwght%5D.ttf), stored as `NotoEmoji.ttf`: 1,982,596 bytes, SHA-256 `de6c18832938afc99caf132b39d6a30a19bac7f2e812e28db2535b4608d27551`.
- [Original license](https://raw.githubusercontent.com/google/fonts/8b0a1d0f5983c89bc2b93f1b5fb55f9e252744b5/ofl/notoemoji/OFL.txt): SHA-256 `500bb1ccf43df7bbb522112f9133a52b16e1c35e809632f5d8609b179152de5b`.

The bytes are embedded in the desktop executable. No system font install, font download or developer tool is required at runtime. This does not promise universal Unicode coverage, color emoji or complex emoji-sequence shaping. Update the pinned source and hashes deliberately when replacing the asset.
