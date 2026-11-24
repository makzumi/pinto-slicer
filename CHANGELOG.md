# Changelog

All notable changes to Pinto Slicer are documented here. Versions follow [Semantic Versioning](https://semver.org/), and release tags use the form `vX.Y.Z`.

## [Unreleased]

### Changed

- **Presets** set only the strut thicknesses now — tip, branch, trunk and foot diameters — and the **Density** row owns the tip spacing alone. Picking one no longer moves the other's number or drops its highlight, and the row is labelled **Strut sizes** so it cannot be mistaken for a density control.

## [0.1.10] - 2026-11-24

### Changed

- The **Density** row moved into the top Support card, right under **Preset**, where it can be found without scrolling five cards down.
- Presets now set the density too — Light 8 mm, Medium 6 mm, Heavy 4 mm — so the preset and the density row always agree. Moving the spacing away from a preset drops its highlight instead of leaving a lit button that describes other numbers.
- The default spacing is 6 mm (Medium) instead of 4 mm, so a fresh plate grows about half as many supports as before.

## [0.1.9] - 2026-11-24

### Added

- A **Density** row on the Support tab: **Low** (about 8 mm between tips), **Medium** (6 mm) and **High** (4 mm), with the spacing slider next to it as the fine control and a wider 1.5–12 mm range.

### Changed

- Tip spacing now scales the whole coverage curve, first layer included. A wide spacing is no longer held back by a fixed 3.2 mm first-layer radius, so the millimetres on the slider are the distance the generator aims for.

### Fixed

- Tips near each other now share one trunk and one foot out to the spacing's cluster radius instead of only within the foot diameter. A dense run no longer grows a column under every contact.

## [0.1.8] - 2026-11-23

### Added

- A spinner and **Working…** status while supports are generated, the plate is sliced, or a job is exported. Slicing now runs in the background, so the window keeps repainting instead of freezing until the layers are ready.
- The 3D cross-section fills the cut face at the current layer, so supports and models read as solid instead of open shells.

### Changed

- Opening a project no longer generates supports by itself. The plate opens as saved and the support note asks you to press **Generate Supports**.
- Export refuses to write a job until supports have been generated for the current plate, instead of growing them silently.

### Fixed

- Tube and ball supports no longer look hollow in the slice preview: the sphere mesh had mixed triangle winding, which cancelled the interior of its cross-section and left a ring.
- The export progress bar advances without moving the mouse, and the support spinner keeps animating while generation runs.
- Support status buttons are disabled while a run is active, so a second click cannot start a competing job.

## [0.1.7] - 2026-11-22

### Fixed

- Opening a project now rebuilds its derived supports from the saved settings, and export waits for that rebuild before slicing. Jobs exported right after opening a project no longer omit the support base and supports.

## [0.1.6] - 2026-11-22

### Added

- A **View** section on the Slice tab toggles between the 3D model and the sliced-layer view.
- The 3D Slice view now cuts the model at the current layer: layer 0 shows almost nothing, and dragging the slider builds the model up with the active layer's red contour on the cut plane.

### Changed

- Renamed the floating **Transform** dialog to **Values**, matching the sidebar button.

### Fixed

- Model selection highlighting is responsive again, using a cached per-model picking tree so moving, rotating, or scaling a piece does not force a full picking rebuild.

## [0.1.5] - 2026-11-22

### Fixed

- Layer heights are quantized to three decimal places before export, so CXDLPv4 jobs are accepted by UVtools instead of failing on values such as `0.049999997 mm`.

## [0.1.4] - 2026-11-22

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

[Unreleased]: https://github.com/makzumi/pinto-slicer/compare/v0.1.10...HEAD
[0.1.10]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.10
[0.1.9]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.9
[0.1.8]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.8
[0.1.7]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.7
[0.1.6]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.6
[0.1.5]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.5
[0.1.4]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.4
[0.1.3]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.3
[1.0.2]: https://github.com/makzumi/pinto-slicer/releases/tag/v1.0.2
