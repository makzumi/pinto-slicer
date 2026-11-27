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
| `lutdump2.c` | Calls `ensureBeadLut` (310) and dumps the internal rasterization LUT: 64 column bands × 6230 pixel rows, each cell listing the bead bit-indices it covers. Runs on Windows; crashes under current Wine (C++ exception in `ensureBeadLut`). |
| `roundtrip.c` | Calls the real encoder (`encodeCtbImage_16K_withNoAreaPartData_3bit`, ordinal 302) on a raw bitmap to compare its mask output with the file on disk. |
| `simulate.py` | Reimplements the disassembled algorithm (func `0x1800270b0` + `getsubzonedata`) in Python against the dumped LUT, and compares with the mask stored in a real `.cxdlpv4`. |
| `decode_layer.py` | Decodes one layer's pixel-pair RLE from a real `.cxdlpv4` into a bitmap for feeding the tools above. |

Build pattern (in the HALOT BOX drive, adjust the `Z:\` paths inside):

    wineg++ -m64 -O1 -o dump2.exe dump2.c
    wine ./dump2.exe ; wineserver -k     # exit code 3 is success

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
  hexagon grid instead, which is close but not bit-identical.
