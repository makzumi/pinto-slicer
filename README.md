# Pinto Slicer

A resin 3D printer slicer for Linux, in the spirit of the printer maker's own
tool. Built with Rust and egui.

## Run it

```
cargo run                          # opens on an empty plate
cargo run -- --demo                # two sample pieces to try the window on
cargo run -- mypart.stl            # one part, already loaded
cargo run -- project.rslice        # reopen a self-contained saved project
cargo run -- a.stl b.stl           # several parts, arranged on the plate
cargo run -- ~/parts/              # every STL/OBJ in that folder
cargo run -- --preview             # start on the slice step
cargo run -- --tab supports        # open on a step: prepare/supports/slice/export
cargo run -- --tool turn           # start with a tool picked: view/move/turn/size
```

### Desktop and taskbar icon

The app uses the Pinto bean artwork in `assets/` for its header and native
window icon. The embedded source is a 512×512 RGBA PNG with transparent
corners. At startup the header artwork is Lanczos-resampled once to its exact
physical pixel size for the current DPI, avoiding texture-minification
aliasing. Wayland resolves taskbar icons through the desktop entry rather than the window pixels. Install the binary, launcher and
matching icon with:

```
sh packaging/install-user.sh
```

The viewport's Wayland app id is `pinto-slicer`, matching
`pinto-slicer.desktop`. The installer writes an absolute icon path into the
installed launcher, avoiding stale KDE icon-theme caches. No root privileges
are needed.

Without the window:

```
cargo run -- --inspect ~/parts/                 # what is in these files, how fast they slice
cargo run -- --export job.cxdlpv4 ~/parts/      # slice straight to a printer job
cargo run -- --export job.cxdlpv4 part.stl --no-masks
cargo run -- --printer 'ELEGOO Saturn 3 Ultra' --export job.ctb part.stl
cargo run -- --export job.sl1s part.stl         # compatible Prusa profile + format
```

Pieces are moved, turned and scaled with large, high-contrast handles drawn on
the picked pieces: left-click to pick one, shift-click to add another, or drag
a box to pick everything it covers. Drag an arrow (Move), a ring (Turn) or a
square (Size) to change them. The dual-axis Move handles are real XY, YZ and ZX
quads, so they lean and foreshorten with the model instead of facing the
camera. The navigation cube at the top right snaps the view to any of its six
faces. The view itself is right-drag to orbit, middle-drag to pan and scroll to
zoom, so nothing in the view competes with the handles. The values window, titled **Transform**, opens beside the plate and can be
dragged wherever it is convenient. It repeats the same numbers as fields, and
every piece can be duplicated, deleted or mirrored across an axis without
touching its geometry.

Every button in the left strip shows its name under the glyph and its key on
hover: W view, G move, R turn, S scale, F fit, V the values window, Ctrl+O
open models, Ctrl+Shift+O open project, Ctrl+S save project, Ctrl+Shift+S save
project as, Ctrl+Z undo, Ctrl+Shift+Z or Ctrl+Y redo, Ctrl+D duplicate,
Ctrl+A pick everything, Escape let go, Delete or Backspace remove. A ring can
be made to land on round numbers — the Snap box in the values window, 5° unless
told otherwise.

`--screenshot out.png` draws about 120 frames, saves the window as a PNG and
exits without taking focus, which is handy for automated checks.

## The four steps

The window is walked like a wizard: the numbered steps across the top each hold
one job, and **Back** and **Next** at the bottom of the settings column move
between them. Any step can be opened directly, so the order is a guide rather
than a cage — going back to move a piece does not lose the supports, it only
marks the run out of date, and the step says so.

- **Prepare** — printer profile, auto layout ("Arrange all on the plate"),
  placement of the selected piece, and the list of pieces on the plate.
  **Drop onto the plate** lowers the picked pieces (or everything) until each
  one touches it; a piece that arrives from a file does the same on its own. A
  piece that is bigger than the plate says so and can be scaled down to fit.
  **Align & space selected…** opens an on-demand dialog for two or more picked
  pieces: minimum/centre/maximum alignment on X, Y or Z, evenly distributed
  centres on X or Y, and configurable equal gaps that preserve the group's
  centre. Every layout action updates both the transforms and rendered meshes.
- **Supports** — from the top: the "Grow supports" switch with the live counts
  and the Light / Medium / Heavy presets; **Height from platform** (how far the
  model is lifted off the plate, which is also how tall the support stems are,
  with On plate / 5 mm / 10 mm shortcuts); **Base** (add a base plate under the
  feet, its thickness, margin, chamfer and the foot diameter); **One tip** (the
  picture plus cone or pin, upper and lower diameter, length, how far it sinks
  into the model); **Where they grow** (overhang angle, tip spacing, show the
  overhang faces in red); **Branches & trunks** (branch angle and diameters).
  "Auto all" grows them for everything on the plate and "Delete all" throws
  them away again; neither happens on its own, so the settings can be changed
  in peace and the plate only rebuilt when you press one of the two. A plain-
  language report of the last run sits at the bottom, and says when the plate
  has moved on since.
- **Slice** — layer height and exposure, print time estimate, and the layer
  being cured shown black and white beside the plate, the way a slicer shows
  it: white where the screen lights up. The bar under the plate plays through
  the layers, with play/pause, one-layer steps, a layer slider and the time
  each layer is held.
- **Export** — writes the selected printer's job file plus `job.json` and
  `README.txt`, optionally with a PNG mask per layer. The profile supplies the
  exact native LCD resolution and default format; compatible alternatives are
  limited to that printer (for example Photon/PWS). Export is staged in a
  private directory beside the destination and committed only after every
  layer and sidecar succeeds. Cancelling or an encoder failure discards that
  staging directory, leaving any previous valid job and masks untouched. File
  stems are validated as plain names, never paths. If an operating-system
  error prevents rollback itself, the recovery workspace is retained and its
  path is reported instead of deleting the only backups. While export runs, a
  Cancel button sits beside the progress bar.

## What it does

- Saves and opens versioned `.rslice` projects from the File menu or a
  background worker. The extension and versioned format are unchanged by the
  Pinto Slicer rename, so existing projects remain compatible. Projects embed their meshes, transforms, printer and
  print/support settings, selection, export options and layout preferences, so
  they do not depend on the original STL/OBJ paths. Saves use temporary-file
  replacement to preserve a previous project on failure. Derived supports are
  rebuilt with **Auto all** after opening; a fresh bare-plate slice is available
  immediately.
- Provides global undo/redo with Ctrl+Z and Ctrl+Shift+Z (or Ctrl+Y), using the
  same project snapshots as save/open. Rapid edits are coalesced into useful
  steps and mesh arrays are shared between snapshots rather than duplicated.
  Opening a project starts a fresh history; undo/redo marks derived supports
  and slices stale when their inputs change. Background opens never overwrite
  edits made while the file is loading.
- Loads STL (binary and text) and OBJ files; the open window stays open so a
  whole folder of parts can be loaded one after another, or all at once.
- Offers ten printer profiles in the top-left picker. Each owns its build
  volume, exact LCD resolution, optical mirroring and native format. Export
  always uses the full native panel resolution; the old `--detail` downsample
  switch is intentionally no longer used. The GUI shows ten starter profiles;
  batch export also knows the Photon Mono 4K for `.pwma` jobs.
- Places, turns and sizes pieces on the build plate with the Move, Turn and
  Size handles; right-drag orbits, middle-drag pans, scroll zooms, and the left
  button picks a piece or draws a picking box.
- Grows supports with a PrusaSlicer-derived MSLA pipeline. Linked slice
  regions propagate support coverage with a height-dependent radius curve;
  exposed overhang outlines are sampled every 2 mm, new islands and wide
  peninsulas receive interior and rim contacts, and those contacts are
  projected back onto the mesh. The column builder validates each pinhead,
  creates central vertical pillars, attaches nearby side heads with descending
  bridges, searches avoidance routes for obstructed heads, and only then uses
  a reverse pinhead anchor on the model. Nearby pillars get angled zig-zag
  links and contour-derived pads sit under their feet. Nothing is boolean
  merged: overlapping closed primitives are combined by the slicer's non-zero
  fill, as they are in PrusaSlicer.
- Uses a reverse pinhead anchor when an obstructed head cannot join a nearby
  pillar or find a clear avoidance route to the platform. The pillar itself
  remains vertical, and the anchor bonds it into the upward-facing model
  surface it struck on the cached downward probe. Model-anchored pillars get
  no platform foot. The Supports step reports these separately and warns when
  a head has no valid route at all.
- Raises each piece to the "height from platform" you set when a support run
  starts, because that gap is what the supports fill; a piece already floating
  higher is left at its height. The pieces really move, so the picture, the
  supports and the slice always agree.
- Grows supports only when you ask. The Supports step's "Auto all" button runs
  the generator once for everything on the plate; moving, turning or scaling a
  piece leaves the supports where they were and the panel says the run is out
  of date, so nothing is rebuilt behind your back. That matters on a full
  plate: with a dozen heavy models, rebuilding after every nudge made the view
  impossible to turn. There is no click-to-add support tool for the same
  reason.
- Slices into layers at an adjustable layer height, shows each one as a black
  and white picture and plays through them.
- Writes jobs on a background thread, rasterizing one layer at a time, so the
  window stays usable during export.
- Renders the viewport with wgpu into a 4× MSAA offscreen texture with
  reverse-Z depth. Vertex/index buffers are cached by model and slice revision,
  so camera movement and selection do not re-upload unchanged geometry. The
  full native panel is used while orbiting, panning and zooming—there is no
  reduced moving-render scale. `View` remains the shared camera source for the
  GPU, CPU picking and egui gizmos. Colors are converted at the shader boundary
  to the `Rgba8UnormSrgb` format required by egui-wgpu 0.29.

## Printers and job formats

| Profile | Native LCD | Default output |
| --- | ---: | --- |
| Creality HALOT-X1 | 15120 × 6230 | `.cxdlpv4` |
| Creality HALOT-ONE | 1620 × 2560 | `.cxdlp` |
| ELEGOO Saturn 3 Ultra | 11520 × 5120 | `.ctb` |
| ELEGOO Saturn 4 Ultra 12K | 11520 × 5120 | `.goo` |
| Anycubic Photon | 1440 × 2560 | `.photon`, with `.pws` as an alternative |
| Anycubic Photon Mono | 1620 × 2560 | `.pwmo` |
| Anycubic Photon Mono X | 3840 × 2400 | `.pwmx` |
| Anycubic Photon M3 | 4096 × 2560 | `.pm3` |
| Original Prusa SL1 | 1440 × 2560 | `.sl1` |
| Original Prusa SL1S SPEED | 1620 × 2560 | `.sl1s` |

Batch export additionally accepts `--printer 'Anycubic Photon Mono 4K'` for
`.pwma`, or infers that hardware profile from the output extension.

The writers cover all of `.ctb`, `.goo`, `.cxdlp/.cxdlpv4`,
`.photon/.pwmo/.pwmx/.pm3/.pws/.pwma`, and `.sl1/.sl1s`. These extensions are
not aliases for one layout: the implementation includes Creality v3 column
lines, Creality v4 records and HALOT-X1 3-bit pixel-pair RLE, CTB/GOO encoders,
Anycubic v1/v516 and CBDDLP v2, and Prusa ZIP/PNG archives.

## What is not verified

- **Physical printer interoperability.** The binary layouts follow UVtools'
  format implementations and the `msla_format` CTB/GOO crate instead of the
  earlier guessed 64-byte CXDLP header. Tests check signatures, tables,
  addresses, codecs, checksums and readback where a decoder is available.
  UVtools 5fdcc53 independently opened and extracted all 20 layers from a
  sample in each of the twelve requested formats, decoded a full native
  15120 × 6230 HALOT-X1 frame, and read a native 520-layer SL1S CLI export
  with five bottom layers. These are meaningful software checks, but still
  not physical-printer acceptance. Run a short test print before a long job.
- **Machine-specific motion settings.** Exposure values are editable; lift,
  retract and settle defaults are deliberately conservative and should be
  tuned to the resin and printer.

## Checks

```
cargo test        # 174 active tests: file loading, slicing, masks, layout,
                  # every requested printer format, CXDLP codecs/CRC, atomic
                  # export replacement, GPU projection/readback, and supports
cargo clippy --all-targets -- -D warnings
```

The integration checks also exercise every requested output extension. Seven
headless Vulkan tests cover projection, model/support shading, plate/shadows,
lines, revision caching and close-up support geometry; they return early with
a message if no Vulkan adapter is available. Four heavier checks are ignored
by default: a native 94-megapixel HALOT-X1 export, an interactive heavy-plate
GPU performance check, a high-resolution project-file round-trip, and the
supplied `Body.stl` (306,000 triangles) support-pipeline check at 45°. Run the
native panel check and keep its output for inspection with:

```
KEEP_FORMAT_SAMPLES=1 cargo test cxdlp::tests::halot_x1_writes_its_exact_native_panel -- --ignored --exact
```

```
KEEP_FORMAT_SAMPLES=1 cargo test project::tests::supplied_body_project_roundtrip -- --ignored --exact
```

The release GPU benchmark renders 288,000 triangles in 10 pieces at roughly
4.2 ms per full 880×700 frame on the development machine (NVIDIA RTX 4070):

```
cargo test --release ui::cost::interactive_cost_with_a_heavy_plate -- --ignored --nocapture
```

The Body.stl check produces 210 heads on 159 pillars with 69 side bridges, 30
reverse model anchors and 129 platform feet. All 4 island groups are
supported, every emitted tree is connected, and the branch/trunk/tip collision
audit passes. One non-island head has no safe route and is reported as dropped
instead of being drawn as broken geometry. Generation takes about 2.7 seconds.
Run it with:

```
cargo test supports::tests::supplied_body_has_complete_connected_supports_at_45_degrees -- --ignored --nocapture
```

### Where the support algorithm comes from

The support pipeline is based on PrusaSlicer's MSLA implementation in the
supplied `PrusaSlicer-master` tree, principally:

- `src/libslic3r/src/libslic3r/SLA/SupportPointGenerator.cpp` — linked slice
  regions, support-radius propagation, overhang outlines and island sampling.
- `src/libslic3r/src/libslic3r/SLA/DefaultSupportTree.cpp` — pinhead filtering,
  pillar clustering, side bridges, model-facing fallbacks and pillar links.
- `src/libslic3r/src/libslic3r/SLA/SupportTreeUtilsLegacy.hpp` — beam probes,
  avoidance routes and ground-pillar policy.
- `src/libslic3r/src/libslic3r/SLA/Pad.cpp` — support-contour-derived pads.

The Rust port keeps those algorithms and their defaults where the existing UI
has an equivalent setting. Raster island sampling uses deterministic
farthest-point placement rather than Prusa's CGAL/Voronoi implementation, and
sparse support contours are split into convex pads because this project does
not depend on Clipper. The UI's 4 mm tip spacing, 45° overhang, 0.6/1.2 mm
cone and 1.6 mm trunk defaults remain configurable application settings.

### Printer format references

The job writers were cross-checked against these implementations:

- UVtools' [CXDLP v4 writer](https://github.com/sn4k3/UVtools/blob/5fdcc53043dfb7ac4d3876586f79edfb4c5c048b/UVtools.Core/FileFormats/CrealityCXDLPv4File.cs),
  [HALOT-X1 pixel-pair codec](https://github.com/sn4k3/UVtools/blob/5fdcc53043dfb7ac4d3876586f79edfb4c5c048b/UVtools.Core/FileFormats/PixelPairRleCodec.cs)
  and [legacy CXDLP writer](https://github.com/sn4k3/UVtools/blob/5fdcc53043dfb7ac4d3876586f79edfb4c5c048b/UVtools.Core/FileFormats/CrealityCXDLPFile.cs).
- UVtools' [Anycubic v1/v516](https://github.com/sn4k3/UVtools/blob/5fdcc53043dfb7ac4d3876586f79edfb4c5c048b/UVtools.Core/FileFormats/AnycubicFile.cs),
  [CBDDLP/Photon](https://github.com/sn4k3/UVtools/blob/5fdcc53043dfb7ac4d3876586f79edfb4c5c048b/UVtools.Core/FileFormats/ChituboxFile.cs)
  and [SL1/SL1S](https://github.com/sn4k3/UVtools/blob/5fdcc53043dfb7ac4d3876586f79edfb4c5c048b/UVtools.Core/FileFormats/SL1File.cs)
  implementations, plus its machine registry for panel data.
- The [`msla_format` 0.2](https://docs.rs/msla_format/0.2.0/msla_format/)
  crate supplies the maintained CTB and GOO codecs.
