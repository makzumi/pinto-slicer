# Changelog

All notable changes to Pinto Slicer are documented here. Versions follow [Semantic Versioning](https://semver.org/), and release tags use the form `vX.Y.Z`.

## [Unreleased]

## [0.1.26] - 2026-11-25

### Changed

- Standing trunks are cones now, not sticks: about 5 mm wide at the
  platform, slim where they meet the model — the shape of the columns the
  printer already prints well. Trunks held from the model stay straight.

## [0.1.25] - 2026-11-25

### Changed

- The printer preview no longer reads half white, half grey. Top faces
  shade like rounded surfaces — brightest in the middle, darker towards
  the rim — and walls darken as they meet the plate, sixteen greys where
  the flat render used four.
- No single support pad may become a sheet. Pads merge only where their
  feet actually touch, none grows past the largest pad the reference
  slicer's own jobs carry, and a pad past the cap is cut along its long
  axis with a drainable seam between the halves — a peeling corner can
  no longer drag a whole model's base off the plate. The pad profile
  also turned its chamfer inside out: the rim keeps the pad's full
  thickness and the middle sits lower, so the first peel grabs a lip,
  never a feathered edge.

## [0.1.24] - 2026-11-25

### Fixed

- Saving or loading a configuration no longer greys out the export. Loading
  a row that already matches the panels keeps the slice fresh, and when the
  values really do change, the Export tab says so and offers a
  Slice again button, so being stale never strands the tab.
- The configuration tooltip no longer shows stray backslashes with lines
  that break mid-sentence: it is one flowing sentence again.

### Changed

- The configuration rows line up like a list: names start at the left edge
  instead of centring, the active row wears a circled check and the rest a
  ring, both drawn from the icon font so no machine shows a tofu box.
- The Layer & exposure values put their names above the sliders, with one
  width for every slider and another for every number field, so nothing is
  cut off in the column.
- The export's Cancel button claims its side of the row first, so the status
  note never crowds it out of the card.

## [0.1.23] - 2026-11-25

### Added

- The Configuration menu by the steps names the supports and export
  settings under rows you can load back in any project. Default is the
  built-in profile and stays read-only; your own rows sit under it, each
  with an × that asks before it removes. Save current configuration is
  only clickable when something has changed, and switching away with
  changes up asks whether to update the row, save as new, or discard —
  listing every value that moved, in the panels' own words.

### Changed

- Open recent is two lists now: the last ten models and the last ten
  projects, each under its own menu. A row from the one combined list
  moves to whichever side it belongs to the first time the app starts.

## [0.1.22] - 2026-11-25

### Changed

- The preview picture the HALOT-X1 shows is a shaded 3-D view of the
  plate now, the way the printer's own previews look: the sliced spans
  stacked as solid boxes, seen from a corner above and lit from the
  upper left, drawn straight from the outlines rather than the 15 K
  masks. A span whose contour never closes is dropped instead of
  streaking across the plate, and scanlines sample the middle of their
  band so a vertex resting exactly on an edge no longer opens one.

## [0.1.21] - 2026-11-25

### Fixed

- A hollow's ceiling earns its own forest of supports again. The heads a
  funnel or a wall grew below it were being counted as covering it, and
  the sparse pickers that ran in between never fired inside the merged
  blob. Coverage now reaches only a short way above a head — the slope it
  belongs to — so a different overhang across the void gets its own
  columns, with the same spacing the ring below has.
- Support generation is deterministic again: the island picker read its
  candidates in hash order, so two runs could disagree by a head.
- The cut plane's cap in the cross-section view is drawn from the layer's
  own outlines instead of a picture of it. The texel grid that showed as
  tiny squares on the cut is gone; a flat wall costs one quad.

### Changed

- **Rings of overhang grow a ring of heads.** A rim or flange that peels open a few cells per layer used to earn one or two heads per layer — about half what the proven slicer plants under the same shape — because each layer only saw its own sliver of the band. The overhang coastline is now carried from layer to layer and walked as one shape: a head every tip distance along the whole ring by the time it closes, each on the surface it belongs to. On the knob that reported the gap: 26 columns became 54, spread around the base rim, the crown and the flange the way the reference plate shows them.

- **Supports grow the way the proven slicer grows them.** HALOT Box's working profile leaves "Overhangs Only" unchecked, so every face that points down gets support — shallow slopes and flat undersides included — and Pinto now starts from that coverage too (a checkbox, "Only steep overhangs", brings the old behaviour back). Strut junctions carry the spheres the column style is known by, and the cross-brace lattice got the profile's own numbers: braces start 3 mm above the plate and no longer run out at 400.
- **Floating shelves grow trees, not a lone column.** Islands — the undersides that hang over nothing — are now sampled the way the proven slicers sample them: a head per 4 mm square of the interior plus a head every 4 mm along the wall (Prusa's own sampler walks the outer wall of a small overhang for exactly this reason, `TreeSupport::sample_overhang_area`). A knob's crown grows a ring of heads that share columns and brace into one tree. On the project that reported it: the crowns went from one head each to six, and the whole plate from 47 columns to 58 carrying 75 heads.
- **The lattice knows when to stop.** Cross braces only earn the double diagonal when their columns stand far apart; close pairs zig-zag on their own. A brace that would run straight through a third column, or stack on one already there, is left out. On the same plate the stick count dropped by half with no two sharing a line.
- **The defaults are the numbers the machine has always printed with.** Every file HALOT Box ever wrote for the X1 carried the same built-in profile, no matter what its settings boxes said: 30 s initial exposure, 1.5 s normal, 4 bottom layers, an 8 mm rise at 300 mm/min and 4 s of settle. Pinto now starts from those numbers — including the tip spacing, 4 mm apart (the High density), the way every well-printed support plate was made.
## [0.1.20] - 2026-11-25

### Changed

- **One type scale and one pill shape for the whole window.** The panels had grown five near-duplicate text sizes (11.0 and 11.5, 12.0 and 12.5, 13.0 and 13.5) and three pill roundings (7, 8, 9). Titles sit at 10.5, notes at 11.5, body text and pills at 12.5, primary buttons at 13.5, and every pill is rounded 8.
- **The help text is no longer a wall.** The How-to window's single run-on paragraph is split into short titled sections (In the view, Keyboard, The rest), and the phrase "the house under the orientation cube" now says the home button.
- **Sentences count properly.** "3 piece(s) mirrored" style statuses now read "3 pieces mirrored", with a real singular for one. The export tab no longer tells you to open HALOT-X1 files with UVtools — it cannot read them yet; the printer can.
- **The plate stays in 3D.** Opening the slice tab used to flip the view to the cut layers on its own; now it leaves the 3D view alone, and the **Slices** buttons are what flip it.
- **The print settings live on the export tab.** Layer height and exposures sit next to the job file they describe, and the slice tab keeps a single column: the cross-section view, right where the settings used to be.
- **Job files start where the project lives.** With a project saved or opened, the export dialog starts in its folder; without one, it starts on the desktop. The old `~/resin-job` default is gone.
- **The version is in the title bar.** "Pinto Slicer (v0.1.19)" sits beside the app name, small and quiet.
- **The supports row is tidy.** **Generate Supports** fills its row; **Delete all** and **Reset defaults** split the one below it, half each.
- **No more shouting.** Card titles and window labels are sentence case: "WHERE THEY GROW" is "Where they grow", MOVE/TURN/SIZE became Move/Turn/Size, and the badge says "3D model", not "3D MODEL".
- **The playback runs exponentially faster.** The pills still say 0.5× to 4×, but every step up the ladder now runs four times the pace of the one before it — the slowest holds a layer for 60 ms and the top pill tears through a thousand layers in about a second. The **Loop** switch moved in beside the 4× pill, where the speed choices live.
- **The view badge is gone.** The little “3D model” tag in the corner of the picture never said anything the picture didn't; the lens pills moved up into its place.

## [0.1.19] - 2026-11-25

### Fixed

- **The printer can say how much resin a print needs.** The file left the volume, weight and cost slots at zero, so the printer announced 0.00 ml and there was no way to tell a working file from an empty one. The volume is now counted off the layer masks themselves, every lit pixel being a voxel of cured resin, and the weight follows at 1.1 g per millilitre, the density Creality's own numbers round to.

## [0.1.18] - 2026-11-25

### Fixed

- **Normal layers get their exposure again.** The first test print cured the bottom film and then printed almost nothing, and the file said why: three slots in the print parameters that HALOT Box fills with 5, 120, 6 and 1 were zero in ours, the slicer block put the exposures where UVtools guessed instead of where Creality's own writer puts them, and every layer record missed two fixed words that HALOT Box always writes. The writer now copies all of them from the files the printer accepts, so the light comes on for every layer, not just the first few.

## [0.1.17] - 2026-11-24

### Fixed

- **Files the HALOT-X1 accepts.** HALOT Box lays every layer out in a 120-byte record, puts two thumbnails and a twelve-byte model size into the file, and repeats the motion settings after the pixel data; ours carried none of that, so the printer turned its nose up at files another slicer wrote. The writer now follows Creality's layout, including the zero anti-alias level this machine writes and both previews, so the plate shows up on the printer's screen before it starts.

## [0.1.16] - 2026-11-24

### Added

- **Copy as many pieces as asked for** from the left strip: **Copy…** asks for a number and puts that many copies of every picked piece beside their originals, wrapping into a second row when the plate runs out. **Ctrl+D** still makes one copy without asking.
- **Mirror beside the original** from the left strip: **Mirror** asks which way to flip — across **X**, **Y** or **Z** — and puts one mirrored copy of each picked piece next to it, so a left-hand part leaves a right-hand one. The flip is a placement, not new geometry, so it can be undone like any other move.

### Fixed

- **Quit without saving** closes the window. Answering the question closed it only to have the app ask again on the next frame and cancel its own close, so the window could not be closed at all once there were unwritten changes.

## [0.1.15] - 2026-11-24

### Added

- **An orthographic lens** on the 3D view, chosen with the **Persp** and **Ortho** chips under the viewport's caption. Perspective is still the default; the orthographic view keeps every piece at the same size whatever its distance, and shows the plate as the flat, parallel projection measuring is easiest on. Both lenses frame the plate the same way, so the chips can be used mid-thought without the piece jumping.
- **A house under the navigation cube** takes the camera back to the three-quarter view the window opens with, keeping the distance, the target and the lens it had.

## [0.1.14] - 2026-11-24

### Added

- **Open recent** in the File menu lists the last ten files that were opened or saved, newest first, each with its full path on hover. A project in the list opens like **Open project**; a model joins the plate the way a dropped file does. A file that has since been moved or deleted says so and leaves the list.
- **Quitting asks before it loses work.** Closing the window — or pressing **Ctrl+Q**, now in the File menu — while the plate differs from the last project file it was written to offers **Save and quit**, **Quit without saving** and **Cancel**. Saving from the question writes the project and closes the window when the write lands.

## [0.1.13] - 2026-11-24

### Fixed

- **Deleting a piece no longer makes the picture and the picks disagree.** The drawn plate was only rebuilt for pieces whose own size or placement changed, so removing one shifted every slot behind it while the picture kept the old geometry: a click then landed on a neighbour, and the piece that disappeared was not the one that was picked. A piece list that changes length now rebuilds the whole plate.

## [0.1.12] - 2026-11-24

### Added

- **Playback speed and loop** on the Slice tab: a second row under the transport offers 0.5×, 1×, 2× and 4× (0.24 s to 0.03 s per layer), and a **Loop** switch chooses whether the last layer starts the plate over or brings playback to rest there. The old seconds-per-layer box is gone, so speed has one control rather than two.

## [0.1.11] - 2026-11-24

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

[Unreleased]: https://github.com/makzumi/pinto-slicer/compare/v0.1.26...HEAD
[0.1.26]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.26
[0.1.25]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.25
[0.1.24]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.24
[0.1.23]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.23
[0.1.22]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.22
[0.1.21]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.21
[0.1.20]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.20
[0.1.19]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.19
[0.1.18]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.18
[0.1.17]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.17
[0.1.16]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.16
[0.1.15]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.15
[0.1.14]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.14
[0.1.13]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.13
[0.1.12]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.12
[0.1.11]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.11
[0.1.10]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.10
[0.1.9]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.9
[0.1.8]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.8
[0.1.7]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.7
[0.1.6]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.6
[0.1.5]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.5
[0.1.4]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.4
[0.1.3]: https://github.com/makzumi/pinto-slicer/releases/tag/v0.1.3
[1.0.2]: https://github.com/makzumi/pinto-slicer/releases/tag/v1.0.2
