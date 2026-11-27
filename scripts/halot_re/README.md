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
| `parity.py` | Exact-LUT parity harness: decodes a real `.cxdlpv4`, computes the mask with the dumped `ensureBeadLut` LUT (`--model lit`) or with a convex-hull area-part approximation (`--model hull`), and reports per-layer Jaccard / fp / fn. |
| `simulate.py` | Reimplements the disassembled algorithm (func `0x1800270b0` + `getsubzonedata`) in Python against the dumped LUT, and compares with the mask stored in a real `.cxdlpv4`. Its cached `layer_100.npy` predates the decoder fix and must be regenerated. |
| `decode_layer.py` | Decodes one layer's pixel-pair RLE from a real `.cxdlpv4` into a bitmap for feeding the tools above. Fixed: the RLE lives at `record.addr + 44` with length `record.size - 44`, not at `record + 44`. |

Build pattern (in the HALOT BOX drive, adjust the `Z:\` paths inside):

    wineg++ -m64 -O1 -o dump2.exe dump2.c
    ./dump2.exe ; wineserver -k          # run the wrapper directly; exit 3 is success

Exact LUT dump and parity check:

    wineg++ -m64 -O1 -o /tmp/halot_re/lutdump2.exe lutdump2.c
    (cd /tmp/halot_re && ./lutdump2.exe)   # writes lut_range.bin, lut_bits.bin, lut_meta.txt
    python3 scripts/halot_re/parity.py --layers 0,100,400,529
    python3 scripts/halot_re/parity.py --layers all --step 10 --model hull

Current parity numbers (reference `17889895071359511442.cxdlpv4`, MSB):

- exact LUT + lit pixels: mean Jaccard ~0.61 (0.41 on layer 529)
- exact LUT + convex hull + plate centre: mean ~0.83 (0.88 on layer 529)

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
