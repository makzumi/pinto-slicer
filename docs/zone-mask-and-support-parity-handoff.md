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

## The 68-byte active-region mask (remaining work)

HALOT Box writes a per-layer 68-byte (544-bit) "active region" mask into each
layer-table record (words `w10..w26`); we originally wrote zeros there. The
printer's "Honeycomb Matrix" backlight uses it to light only the LED sub-zones
under the current slice (a power/even-exposure optimization, not a
correctness requirement). A zeroed mask made the printer light only the center
→ failed prints. Interim fix: `6a73962` writes all `0xFF` (light every zone;
the LCD mask still confines the cure geometrically).

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

### Reverse-engineering state (where we stopped)

- `cxdlp.dll` (`LampBeadCal` class) **computes** the mask, and its bead
  geometry is **angular/arc**, not a plain rectangle: `preinit(f1,f2,c1,c2,…)`
  iterates beads in angular space ÷ `180.0f`, applies cos/sin, scales by LCD
  resolution; 6 beads per zone; `findBeadByPoint(x,y)` maps pixel → bead/zone
  via a stored 3×2 affine matrix + integer divide.
- **Pitch + count are runtime params passed by the caller**, not hardcoded:
  a float scan of every DLL/exe found no `7.3`/`6.23`/`7.29931`/`6.22947`
  constants. Build plate `211.68`/`118.37` IS hardcoded in `cxdlp.dll` `.rdata`
  (at `0x18004dd08`/`0x18004dd00`).
- **Caller = `BasicKernel.dll`** — it imports the `LampBeadCal` symbols
  (`initLampBeadCal`, `getbeaddata`, `getsubzonedata`, `ensureBeadLut`,
  `setBeadsForRange`, `whereami`, `reset`, `resetForReinit`) and owns the
  `creative_kernel::PrinterHeatMap` class (`changeHeatMap(QJsonObject)` etc.).
  Its debug keys: `[HeatMap] initLampBeadCal: beadct=`,
  `[HeatMap] model bbox: min=(`, `[HeatMap] scanline: pixelYrange=[`,
  `lampBallArray=`, `lampBallMap=`, `lampBall2LampAreaMap=`, `beads/inst:`,
  `enabledBeads=`.
- String VAs in `BasicKernel.dll` (ImageBase `0x180000000`), for disassembly:
  - `[HeatMap] initLampBeadCal: beadct=` → `0x1802702a8` (RVA `0x2702a8`)
  - `lampBallArray=` → `0x1802700f1`; `lampBallMap=` → `0x1802700e1`
  - `lampBall2LampAreaMap=` → `0x1802703a1`; `beads/inst:` → `0x18027ee88`
  - `enabledBeads=` → `0x180270369`; `[HeatMap] model bbox: min=(` → `0x1802703b8`
  - `[HeatMap] scanline: pixelYrange=[` → `0x1802703f0`

### Next steps to finish the dynamic mask

1. **Find the caller's pitch/count params.** Disassemble `BasicKernel.dll`
   around the `[HeatMap] initLampBeadCal: beadct=` call site (string xref at
   `0x1802702a8`) to see what floats/ushorts it passes. Expected X1 values:
   pitchX `7.30` mm, pitchY `6.23` mm (211.68/29, 118.37/19), 29 cols × 19
   rows → 542 beads → 544-bit mask.
2. **Or run `getLampMask` under Wine** — `cxdlp.dll` exports
   `?getLampMask@LampBeadCal@@…` returning `vector<vector<pair<int64,int64>>>`
   (bead pixel ranges). Needs a Windows-hosted caller (no mingw cross-compiler
   and no Windows python in the Wine prefix).
3. **Then implement** per-zone masks in `src/cxdlp.rs` write path: for each
   slice, compute the 544-bit mask (which beads the slice touches) using the
   recovered bead geometry, and write it into `w10..w26`. Keep the all-`0xFF`
   fallback behind a toggle.

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

1. Dynamic per-zone backlight mask (see above) — the only remaining item.
2. (Optional, declined) per-model support density.
