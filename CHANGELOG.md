# Changelog

All notable changes to Pinto Slicer are documented here. Versions follow [Semantic Versioning](https://semver.org/), and release tags use the form `vX.Y.Z`.

## [Unreleased]

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

[Unreleased]: https://github.com/makzumi/pinto-slicer/compare/v1.0.2...HEAD
[1.0.2]: https://github.com/makzumi/pinto-slicer/releases/tag/v1.0.2
