# Changelog

All notable changes to Pinto Slicer are documented here. Versions follow [Semantic Versioning](https://semver.org/), and release tags use the form `vX.Y.Z`.

## [Unreleased]

## [1.0.3] - 2026-11-22

### Added

- **Reset defaults** on the Supports tab restores the default gap, tip, branch, trunk, base, and preset values without changing whether supports are enabled.
- 3D support previews for height, base, tip, branch, and trunk settings, with the model shown as a 30% ghost so it no longer hides the geometry being adjusted.
- Slice controls for initial exposure, exposure time, rising height, motor speed, light-off delay, and bottom exposure layers.

### Changed

- Renamed **Auto all** to **Generate Supports**, matching the only support workflow in the app.
- Transform tools and the left toolbar now appear only on the Prepare tab, so moving and rotating cannot be started from Supports, Slice, or Export.
- The Slice view is labeled **Cross-section view**, and print-time estimates use the configured rising height, motor speed, and light-off delay.
- Export uses one native save dialog for the file name and folder instead of separate path fields, and writes only the printer job file.
- HALOT-X1 `.cxdlpv4` export now uses the pixel-pair RLE layout expected by UVtools 7.1.0 and newer.
- Projects are saved as format v2; older `.rslice` files migrate automatically.

### Fixed

- Support previews no longer show cracks, spears, or incorrect occlusion: the software renderer now uses a depth buffer, back-face culling, and supersampling.
- Fixed the floating Transform window remaining visible after switching away from Prepare.
- Layer-mask generation now buckets scanline spans and visits only filled rows, making export substantially faster.
- Corrected CXDLPv4 header fields, per-layer exposure records, RLE framing, and CRCs for Creality HALOT-X1 output.

## [0.1.3] - 2026-11-16

### Added

- A default-on **Uniform scale** option in the Transform dialog, with optional independent X/Y/Z scaling.

### Fixed

- Support cones now include their back-sphere junctions, creating solid connections to posts and branches, including angled supports.
- Deleting a model removes only its own supports and correctly remaps support ownership for the remaining models, including through undo/redo.
- Moving or rotating a model removes its supports and drops it onto the plate while preserving supports for untouched models.
- Geometry edits cancel stale background support generation, and zero-delta layout operations no longer remove unaffected models' supports.

## [1.0.2] - 2026-11-16

First public release.

### Added

- Interactive STL/OBJ preparation with transforms, alignment, arrangement, duplication, mirroring, and global undo/redo.
- Configurable automatic support generation with presets and explicit rebuilds.
- Layer slicing, exposure-mask preview, and playback.
- Self-contained `.rslice` projects and background export with cancellation and staged replacement.
- Ten GUI printer profiles and twelve native job formats.
- Command-line model inspection, batch export, and application screenshots.
- Linux user installation, AppImage packaging, and source-built Flatpak bundles.
- Universal macOS DMG (Apple Silicon and Intel) and portable Windows x64 ZIP packages.
- Tag-driven GitHub Actions releases with tests, package smoke checks, and SHA-256 checksums.
- AGPL-3.0-only licensing and third-party attribution.

### Fixed

- The AppImage bundles `libxkbcommon`, `libxkbcommon-x11`, and `libxcb-xkb`. winit loads them with `dlopen()`, so systems that do not ship `libxkbcommon-x11` panicked at startup with `Library libxkbcommon-x11.so could not be loaded.`.
- The release workflow installs the X11/xkb libraries needed for packaging and smoke tests instead of relying on whatever the runner image happens to include.

[Unreleased]: https://github.com/makzumi/pinto-slicer/compare/v1.0.3...HEAD
[1.0.3]: https://github.com/makzumi/pinto-slicer/releases/tag/v1.0.3
[0.1.3]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.3
[1.0.2]: https://github.com/makzumi/pinto-slicer/releases/tag/v1.0.2
