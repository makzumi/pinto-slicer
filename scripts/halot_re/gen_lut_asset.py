#!/usr/bin/env python3
"""Pack the dumped HALOT-X1 bead LUT into the small asset used by the app.

Inputs: lut_range.bin (398721 u32 prefix sums) and lut_bits.bin (u32 bead ids)
from scripts/halot_re/lutdump2.c.  Output: assets/halot_x1_lut.z.

Format (little endian): magic "HX1L", version u32=1, n_cells u32,
n_bits u32, n_cells u8 counts, n_bits u16 bead ids.  zlib-compressed.
"""
import array
import pathlib
import struct
import sys
import zlib

root = pathlib.Path(__file__).resolve().parents[2]
src = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/halot_re")
rng = array.array("I")
rng.frombytes((src / "lut_range.bin").read_bytes())
bits = array.array("I")
bits.frombytes((src / "lut_bits.bin").read_bytes())
assert len(rng) == 398721 and len(bits) == rng[-1]
counts = bytes(rng[i + 1] - rng[i] for i in range(len(rng) - 1))
assert max(counts) <= 255 and max(bits) < 65536
packed = (b"HX1L" + struct.pack("<III", 1, len(counts), len(bits))
          + counts + struct.pack(f"<{len(bits)}H", *bits))
out = root / "assets" / "halot_x1_lut.z"
out.write_bytes(zlib.compress(packed, 9))
print(f"wrote {out} ({out.stat().st_size} bytes)")
