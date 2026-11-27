# HALOT X1 LED-zone mask research tools

These are the Wine callers and simulators used to reverse-engineer how
`cxdlp.dll` (HALOT BOX) computes the per-layer 544-bit backlight mask that
`src/halot_x1_mask.rs` now implements. They are kept for provenance and for
resuming exact-parity work; the slicer itself depends on none of them.

Prerequisites: `wine` + `wineg++` (mingw-w64), the HALOT BOX install, and a
real `.cxdlpv4` exported by HALOT BOX from an X1 (R009) machine.

| Tool | What it does |
| --- | --- |
| `dump2.c` | Loads `cxdlp.dll`, constructs `LampBeadCal` (ordinal 61), calls the X1 `initLampBeadCal` (ordinal 371, 13 args), then `getLampMask` (339) and `getbeaddata` (355); dumps the 542 bead hexagons in pixel and fixed-point space. |
| `lutdump2.c` | Calls `ensureBeadLut` (310) and dumps the internal rasterization LUT: 64 column bands × 6230 pixel rows (398,720 cells, 1,246,524 bead entries), each cell listing the bead bit-indices it covers. The earlier Wine crash was a bad `initLampBeadCal` call (pointer-to-pointer instead of the MSVC vector-by-value structs); fixed and verified. |
| `roundtrip.c` | Calls the real encoder (`encodeCtbImage_16K_withNoAreaPartData_3bit`, ordinal 302) on a raw bitmap to compare its mask output with the file on disk. Builds, but its `initLampBeadCal` arguments still need the same Vec3 fix as `lutdump2.c`; the LUT build is very slow under Wine. |
| `parity.py` | Exact-LUT parity harness: decodes a real `.cxdlpv4`, computes the mask with the dumped `ensureBeadLut` LUT (`--model lit`) or an area-part approximation (`hull`, `cone`, `hybrid`), and reports per-layer Jaccard / fp / fn. `--model all` compares all four in one pass. |
| `simulate.py` | Reimplements the disassembled algorithm (func `0x1800270b0` + `getsubzonedata`) in Python against the dumped LUT, and compares with the mask stored in a real `.cxdlpv4`. Its cached `layer_100.npy` predates the decoder fix and must be regenerated. |
| `area_projection.py` | Prototype of the support-aware area mask: clips a mesh at the layer Z, rasterises the half-space projection (FlatWhite), optionally applies the SepFilter max/min/box pass, extracts white runs, and maps them through the dumped LUT. Accepts model + support STLs and prints the 542-bit mask. |
| `decode_layer.py` | Decodes one layer's pixel-pair RLE from a real `.cxdlpv4` into a bitmap for feeding the tools above. Fixed: the RLE lives at `record.addr + 44` with length `record.size - 44`, not at `record + 44`. |

Build pattern (in the HALOT BOX drive, adjust the `Z:\` paths inside):

    wineg++ -m64 -O1 -o dump2.exe dump2.c
    ./dump2.exe ; wineserver -k          # run the wrapper directly; exit 3 is success

Exact LUT dump and parity check:

    wineg++ -m64 -O1 -o /tmp/halot_re/lutdump2.exe lutdump2.c
    (cd /tmp/halot_re && ./lutdump2.exe)   # writes lut_range.bin, lut_bits.bin, lut_meta.txt
    python3 scripts/halot_re/parity.py --layers 0,100,400,529 --model all
    python3 scripts/halot_re/parity.py --layers all --step 10 --model all --csv /tmp/parity.csv

Current parity numbers (reference `17889895071359511442.cxdlpv4`, MSB, 54
sampled layers):

- exact LUT + lit pixels: mean Jaccard ~0.61 (0.41 on layer 529)
- exact LUT + convex hull + plate centre: mean ~0.83 (0.88 on layer 529)
- cone (project the hull to the plate boundary): mean ~0.79, but 0.92 on 529
- hybrid (hull + cone only below the layer): mean ~0.85, 0.92 on 529

Flat-pad discriminator: export a 10x10x1 mm plate near the front edge with
supports off (and again with supports on), then run

    python3 scripts/halot_re/parity.py --file <export>.cxdlpv4 \
        --layers 0,10,20 --model all

If the stored field for a flat plate is a fan rather than the plate footprint,
the field is an area-part projection; if supports change it, the support area
is included.

The stored 68-byte field is a strict superset of the lit-pixel mask, so the
LUT alone cannot reproduce it. `encodeCtbImage_16K_withPartData` and
`..._withNoAreaPartData_3bit` are instruction-for-instruction equivalent in
the encoder; the extra bits come from the caller pre-filling the output vector
with an "area part" (or from a different input image). Exact parity therefore
requires reproducing HALOT BOX's area-part generation, not just the bead LUT.

What the disassembly established (see also
`docs/zone-mask-and-support-parity-handoff.md`):

- The mask's bit index equals the bead index, MSB-first within each byte
  (`bit b → byte b/8, position 7-(b%8)`), 542 beads over 68 bytes.
- The encoder scans the bitmap row by row in 2-pixel units; each maximal run
  of lit pairs becomes a linear `[start, end)` pixel range fed to
  `getsubzonedata`, which walks the pixel rows it spans and marks every bead
  whose LUT cell the segment touches.
- The exact LUT contents come from HALOT's own `ensureBeadLut` rasterization
  of the clipped bead polygons; our Rust implementation rasterizes the ideal
  hexagon grid instead, with the wrong x origin (0/290.8 instead of
  -581.0/-290.5) and a diamond half-width, so it is not bit-identical.
- The LUT is necessary but not sufficient: exact-LUT lit-pixel parity is only
  ~0.61 Jaccard, and the stored field contains an extra area-part component.


Area-part projection (support-aware)

The stored mask is not a function of the 3-bit slice.  Projecting the decoded
4.6 slices onto the LUT gives, for layer 529, only 73 bits (the thin strip),
while the stored mask is a 178-bit fan from row 8 to row 18.  The same fan is
present at 440..530 even though the part there is only the strip.  The GPU
pipeline explains it: `FlatWhite` renders the mesh half-space above the layer
clip as solid white, `SepFilter` optionally dilates/erodes/blurs it, and
`getsubzonedata` turns its white runs into the 542-bit bead mask.  The missing
geometry is the support/area mesh, not the slice.

`area_projection.py` is the executable prototype:

    python3 scripts/halot_re/area_projection.py --mesh samples/cube.stl --z 10

A 20 mm cube at z=10 gives 22 beads.  Adding a 6x6x10 mm column at x=40 gives
29 beads at z=5 (the column is above the clip) and 22 beads at z=15 (the
column is below the clip), which is the expected half-space behaviour.  The
script uses the exact dumped LUT; the remaining unknowns are HALOT's support
mesh and the exact `SepFilter` radius/mode (and the model-to-image matrix).

Exact parity with a HALOT job therefore requires the same support geometry
that HALOT Box used.  For a Pinto Slicer job the mask must be computed from
Pinto's own model + support mesh; that is the intended writer path.

The Rust port lives in `src/halot_x1_area.rs`: `compute_halot_x1_area_mask`
clips each triangle to the layer, rasterises the projection into the 64 x 6230
LUT-cell grid, and ORs the cells' beads.  It has unit tests for empty meshes,
blocks above/below the layer, and a support above the layer.  It is not wired
into `write_v4`; `SliceResult.area_meshes` now carries the source meshes, and
the writer uses the area projection when they are present.  The CPU projection
has not been benchmarked on production meshes yet.  The exact dumped LUT is
embedded as the 29 KB `assets/halot_x1_lut.z`; regenerate it with
`scripts/halot_re/gen_lut_asset.py` when a new dump is available.
