# Zone-mask & support-parity handoff

Date: 2026-11-26. This documents the investigation into "more supports than
HALOT Box" and the backlight active-region mask, so work can resume cleanly.

## What shipped (committed, ahead of v0.1.26)

| Commit | What it does |
| --- | --- |
| `6a73962` | Light every backlight zone instead of writing a zeroed active-region mask (fixes center-only / dead-edge prints) |
| `0f5808f` | Match HALOT Box support recipe: thin 0.85 mm shafts, 2.5 mm feet, 0.65 mm tips |
| `cbe823a` | Add raft-sheet base mode (weld each connected group into one sheet) |

All in `main`, working tree clean, tests green (243 passed, 0 failed, 4 ignored).

## Why "more supports" than HALOT Box

Not actually more — *bulkier*. Two independent root causes were found and fixed:

1. **Support recipe** — our defaults produced few, thick columns
   (`Ø1.6` trunk, `Ø5.0` max trunk, `Ø3.0` foot, `Ø1.2`/`Ø0.6` tips) versus
   HALOT Box's many thin columns (median `Ø0.84–0.88` shaft, `Ø2.47` foot,
   `Ø0.65` tips, ~2× the column count). Retuned in `0f5808f` + `cbe823a`
   (raft sheets) to match the reference recipe. Measured reference numbers live
   in `docs/second-opinion-findings.md`.
2. **Backlight mask** — see below. This did not affect support *count*, but it
   caused a real print failure (center-only curing).

## The 68-byte active-region mask (shipped in 0.1.28)

HALOT Box writes a per-layer 68-byte (544-bit) "active region" mask into each
layer-table record (words `w10..w26`); we originally wrote zeros there. The
printer's "Honeycomb Matrix" backlight uses it to light only the LED sub-zones
under the current slice (a power/even-exposure optimization, not a
correctness requirement). A zeroed mask made the printer light only the center
→ failed prints. Interim fix: `6a73962` wrote all `0xFF` (light every zone;
the LCD mask still confines the cure geometrically); 0.1.28 computes the
real per-layer mask — see below.

### Known facts

- **Layer record** = 120 bytes (30 × u32): `z, exposure, light_off, address,
  size, 0, island_count, largest_area, flag=1, 0`, then 68-byte mask
  (`w10..w26`), then three zero words. `island_count` + `largest_area` are
  content stats (not printing-critical; `w6=0` on layers that still cure).
- **Mask size**: 544 bits = 68 bytes. 542 unique indices in
  `resources/DefaultHeatMap/DefaultHeatMap.json` (91 partitions, `partitionIndex`
  lists of 6, sizes `{6:87, 5:4}`); the R012 machine uses 450 indices / 76
  partitions in `DefaultHeatMap_R012.json`. Heat map JSONs carry only
  `partitionIndex`/`partitionNumber`/`workingTime` — **no spatial geometry**.
- **Machine**: HALOT-X1 = 92-zone "Honeycomb Matrix" UV LED array, 16K mono LCD
  15120×6230, build 211.68×118.37×200 mm. `R009.printer` config =
  `format_version=4`, `export_format=cxdlpv4`, `mirror_type=mirror_x`.

### Reverse-engineering state (finished in 0.1.28)

The investigation completed after this note was first written. The full
algorithm is understood from disassembling `cxdlp.dll` itself:

- `encodeCtbImage_16K_withNoAreaPartData_3bit` (ordinal 302) scans the layer
  bitmap row by row in 2-pixel units; for each maximal run of lit pairs it
  calls `getsubzonedata(start, end)` with image-linear offsets.
- `getsubzonedata` (ordinal 357) walks the pixel rows a range spans and,
  per row, marks beads through an internal rasterization LUT:
  64 column bands (237 px each) × 6230 rows, each cell a list of bead
  bit-indices. Bits are MSB-first, `bit b → byte b/8, position 7-(b%8)`,
  542 beads. No fill, dilation or thresholding — a bead lights iff a lit
  run touches its rasterized cell.
- `ensureBeadLut` (ordinal 310) builds that LUT from the bead polygons. We
  dumped it once (398,720 cells, 1.25 M indices) and used it to verify the
  algorithm end to end; a fully-lit image reproduces exactly 522 of 542
  beads (the other 20 lie off-plate).
- `BasicKernel.dll` is only the caller: it owns
  `creative_kernel::PrinterHeatMap` and invokes the `LampBeadCal` symbols.
  Debug keys (string VAs, ImageBase `0x180000000`):
  `[HeatMap] initLampBeadCal: beadct=` → `0x1802702a8`,
  `lampBallArray=` → `0x1802700f1`, `lampBallMap=` → `0x1802700e1`,
  `lampBall2LampAreaMap=` → `0x1802703a1`, `beads/inst:` → `0x18027ee88`,
  `enabledBeads=` → `0x180270369`, `[HeatMap] model bbox: min=(` → `0x1802703b8`,
  `[HeatMap] scanline: pixelYrange=[` → `0x1802703f0`.

### What shipped in 0.1.28

`src/halot_x1_mask.rs` implements the same pipeline in Rust:
`build_halot_x1_lut()` rasterizes the ideal honeycomb (19 rows alternating
29/28 beads, 581.5 × 371.09 px pitch, 291-px apothem hexagons) into the same
64 × 6230 grid, and `compute_halot_x1_mask()` replays the run → row → band →
bit marking over each slice bitmap. `write_v4` stores one 68-byte mask per
layer and writes it into the record tail in place of the all-`0xFF`
workaround (non-X1 printers keep the old tail).

This is a close geometric approximation, not bit-exact parity: our fully-lit
image marks 486 beads versus HALOT's 522, because HALOT's LUT is rasterized
from clipped bead polygons, not ideal centre-based hexagons. Functionally it
lights the correct region per layer; only the fringe zones differ.

### Remaining work for exact parity

1. Dump HALOT's own LUT: `scripts/halot_re/lutdump2.c` already does this
   from a Windows host, but `ensureBeadLut` throws a C++ exception under
   current Wine (the older dumps predate a `/tmp` cleanup and are gone).
   Either fix the Wine crash or run it on real Windows, then bake the
   398,720-cell table in as data instead of computing it.
2. Or disassemble `ensureBeadLut` at `0x180021c30` fully: it computes
   `edi = (COLS-1+64)/64 = 237` px bands, then fills the per-cell bead lists
   via `collectBeadsForBand` (ordinal 276 at `0x180020c20`), which clips
   each bead polygon to the band — the exact rasterization rule we
   approximated.
3. Byte 67's extra bits: HALOT sets bits 542 and 543 together on 148 of 532
   layers of the reference file; purpose unknown (printer-side flag, not a
   zone). We write zero there.

Research tools and their README are kept under `scripts/halot_re/`.

### Useful files

- HALOT Box install (reference binaries + resources):
  `/run/media/makzumi/storage/Games/halot-box/drive_c/Program Files/HALOT BOX/`
  - `cxdlp.dll` (427 KB) — the CXDLPv4 writer + mask computation
  - `BasicKernel.dll` (6.7 MB) — `PrinterHeatMap` caller
  - `resources/PrinterConfig/R009.printer` — the X1 machine config
  - `resources/DefaultHeatMap/*.json` — zone/partition index maps
  - `resources/ProSupport/*.json` — reference support presets
- Researcher report: `/tmp/lampbead-geometry-report.md` (146 lines, may be gone
  after reboot — key facts duplicated above).

## CXDLPv4 format reference (for the writer)

- Header: `magic_size` BE32, magic `"CXSW3DV2\0"`, version BE16=4, machine name
  (`R009` vs our `HALOT-X1`), res u16×2 `15120×6230`, bed f32×3
  `211.68/118.37/200`, print_height, layer_height, bottom_layers, preview
  offsets, table/count, print_time, mirror, params_at/size, antialias,
  light/bottom_pwm u16, encryption, slicer_at/size.
- Params block (`@params_at`, 68 B): 10 motion floats, volume@5, weight@6,
  cost@7 (=volume×0.033), light_off@8/9, bcount@10, exposure@11, bexp@12, pads
  `5.0/120.0/6.0/1`.
- Slicer block (`@slicer_at`, 76 B): 7 floats, perlayer/mins/aa/sw u32s, tail
  floats `[0,0,0,bexp,exp]` (sw=0 for X1).
- Layer table: 120 B/record. Data = 44 B motion + pixel-pair RLE.
- **Pixel-pair codec** (`src/cxdlp.rs:183` `pixel_pair_rle`, `:214`
  `decode_pixel_pair`): unit byte FIRST (`0x80 | (left>>5) | ((right>>5)<<3)`,
  bits 0–2 = left, 3–5 = right, bit7 flag, bit6 must be 0), then 0+ repeat
  bytes (bit7=0, base-127 MSB-group-first, run = count−1). Gray expand
  `(g<<5)|(g<<2)|(g>>1)`. Runs never cross a row. (Reference: UVtools v7.0.3
  PR #1016.)

## Support recipe (committed state)

- `src/supports/config.rs` defaults: tip `0.65`/`0.85`/length `3.0`, branch
  `0.85`, trunk `0.85`, trunk_max `2.5`, foot `2.5`, touch_tip_distance `4.0`
  (= Density::High), base `{raft: true, plate_thickness 0.8, margin 1.0,
  chamfer 45°}`.
- Presets: Light (tip `0.7`/`0.55`, branch `0.7`, trunk `0.7`, max `1.8`, foot
  `1.8`); Medium (defaults); Heavy (tip `1.1`/`0.8`, branch `1.0`, trunk
  `1.1`, max `3.4`, foot `3.4`).
- Density: Low `8.0` / Medium `6.0` / High `4.0` (tip spacing). Per-model
  density was **declined** by the user for now.
- `BaseConfig.raft` is `#[serde(default = "yes")]` so old project JSONs load as
  raft=true.

## Open items

1. Bit-exact LUT parity (see "Remaining work for exact parity" above) —
   the shipped mask is a geometric approximation and prints correctly;
   exact parity is polish.
2. (Optional, declined) per-model support density.
