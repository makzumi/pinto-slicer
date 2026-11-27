#!/usr/bin/env python3
"""Parity harness for the HALOT X1 68-byte zone field.

The stored field is compared against masks computed from the layer bitmap
with the bead LUT dumped from the real cxdlp.dll (ensureBeadLut, ordinal 310).
This is deliberately independent of the Rust approximation in
src/halot_x1_mask.rs, so it can be used as the reference for porting.

Typical use:

    # one-off: dump the LUT once with the fixed lutdump2.c
    wineg++ -m64 -O1 -o /tmp/halot_re/lutdump2.exe scripts/halot_re/lutdump2.c
    (cd /tmp/halot_re && ./lutdump2.exe)

    python3 scripts/halot_re/parity.py --layers 0,100,400,529
    python3 scripts/halot_re/parity.py --layers all --step 10 --csv /tmp/parity.csv

Models:
  lit   Exact DLL semantics: every LUT cell touched by a lit pixel pair.
  hull  Same LUT, but the bitmap is replaced by the convex hull of the lit
        pixels plus the plate centre (the current best area-part hypothesis).

The harness expects the reference file and LUT paths to be passed in; the
defaults match the development machine.
"""
from __future__ import annotations

import argparse
import csv
import struct
import sys
from pathlib import Path

import numpy as np

COLS = 15120
ROWS = 6230
EBP = 64       # LUT column bands per row
EDI = 237      # pixels per LUT band (ceil(COLS / EBP))
MASK_BYTES = 68
MAX_BIT = 542

DEFAULT_FILE = (
    "/run/media/makzumi/storage/Games/halot-box/drive_c/users/steamuser/"
    "AppData/Roaming/Creality/HalotBox/4.6.1.4147/17889895071359511442.cxdlpv4"
)
DEFAULT_LUT_DIR = "/tmp/halot_re"


# ---------------------------------------------------------------------------
# .cxdlpv4 parsing / decoding
# ---------------------------------------------------------------------------

def read_cstring_be(data: bytes, off: int) -> tuple[str, int]:
    length = struct.unpack(">I", data[off:off + 4])[0]
    text = data[off + 4:off + 4 + length - 1].decode("latin1")
    return text, off + 4 + length


def parse_header(data: bytes) -> dict:
    off = 0
    version = struct.unpack(">I", data[off:off + 4])[0]
    off += 4
    magic = data[off:off + 9]
    off += 9
    file_version = struct.unpack(">H", data[off:off + 2])[0]
    off += 2
    name, off = read_cstring_be(data, off)
    width, height = struct.unpack("<HH", data[off:off + 4])
    off += 4
    off += 20  # build floats
    vals = struct.unpack("<10I", data[off:off + 40])
    off += 40
    off += 4
    off += 12
    return {
        "version": version,
        "magic": magic,
        "file_version": file_version,
        "name": name,
        "width": width,
        "height": height,
        "layers_off": vals[2],
        "layer_count": vals[3],
    }


def load_layer_pairs(data: bytes, hdr: dict, idx: int) -> np.ndarray:
    """Decode one layer to a (ROWS, COLS) uint8 bitmap of 0/1.

    The pixel-pair RLE lives at record.addr + 44 and is record.size - 44 bytes
    long.  Code bytes have the high bit set; the low 3 bits are the left pixel
    and bits 3..5 the right pixel.  Repeat counts are base-127 values stored
    between code bytes, minus one.
    """
    lo = hdr["layers_off"]
    rec = lo + idx * 120
    addr = struct.unpack("<I", data[rec + 12:rec + 16])[0]
    size = struct.unpack("<I", data[rec + 16:rec + 20])[0]
    if not addr or size < 44:
        raise ValueError(f"layer {idx}: invalid addr/size {addr}/{size}")
    rle = np.frombuffer(data[addr + 44:addr + 44 + size - 44], dtype=np.uint8)
    is_code = (rle & 0x80) != 0
    if np.any(is_code & ((rle & 0x40) != 0)):
        raise ValueError(f"layer {idx}: bad pixel-pair code byte")
    cpos = np.flatnonzero(is_code)
    if cpos.size == 0:
        return np.zeros((ROWS, COLS), dtype=np.uint8)

    # Vectorised base-127 repeat values between consecutive code bytes.
    reps = np.zeros(cpos.size, dtype=np.int64)
    for k, pos in enumerate(cpos):
        start = int(pos) + 1
        end = int(cpos[k + 1]) if k + 1 < cpos.size else rle.size
        val = 0
        for b in rle[start:end]:
            val = (val << 7) | int(b)
        reps[k] = val
    units = np.repeat(rle[cpos], reps + 1)
    if units.size * 2 != COLS * ROWS:
        raise ValueError(
            f"layer {idx}: decoded {units.size * 2} px, expected {COLS * ROWS}"
        )
    left = (units & 7) != 0
    right = ((units >> 3) & 7) != 0
    pix = np.empty((units.size, 2), dtype=np.uint8)
    pix[:, 0] = left
    pix[:, 1] = right
    return pix.reshape(ROWS, COLS)


def stored_mask(data: bytes, hdr: dict, idx: int) -> bytes:
    rec = hdr["layers_off"] + idx * 120
    return data[rec + 40:rec + 40 + MASK_BYTES]


# ---------------------------------------------------------------------------
# Mask models
# ---------------------------------------------------------------------------

def load_lut(lut_dir: str | Path) -> tuple[np.ndarray, np.ndarray]:
    lut_dir = Path(lut_dir)
    rng = np.fromfile(lut_dir / "lut_range.bin", dtype=np.uint32)
    bits = np.fromfile(lut_dir / "lut_bits.bin", dtype=np.uint32)
    expected = EBP * ROWS + 1
    if rng.size != expected:
        raise ValueError(f"lut_range.bin: {rng.size} entries, expected {expected}")
    if bits.size != int(rng[-1]):
        raise ValueError(
            f"lut_bits.bin: {bits.size} entries, range sentinel says {int(rng[-1])}"
        )
    return rng, bits


def lit_mask(pix: np.ndarray, rng: np.ndarray, bits: np.ndarray) -> bytes:
    """Exact translation of getsubzonedata -> setBeadsForRange (MSB-first)."""
    mask = bytearray(MASK_BYTES)
    pairs = (pix[:, 0::2] != 0) | (pix[:, 1::2] != 0)
    for row in range(ROWS):
        lit = pairs[row]
        if not lit.any():
            continue
        d = np.diff(lit.astype(np.int8))
        starts = list(np.flatnonzero(d == 1) + 1)
        ends = list(np.flatnonzero(d == -1) + 1)
        if lit[0]:
            starts.insert(0, 0)
        if lit[-1]:
            ends.append(lit.size)
        base = row * EBP
        for s, e in zip(starts, ends):
            x, x2 = s * 2, e * 2 - 1
            sc = x // EDI
            ec = min(x2 // EDI, EBP - 1)
            for band in range(sc, ec + 1):
                gi = base + band
                for b in bits[int(rng[gi]):int(rng[gi + 1])]:
                    b = int(b)
                    if b < MAX_BIT:
                        mask[b >> 3] |= 1 << (7 - (b & 7))
    return bytes(mask)


def _hull_vertices(pix: np.ndarray) -> np.ndarray:
    """Return the convex-hull vertices of the lit pixels (subsampled)."""
    from scipy.spatial import ConvexHull

    ys, xs = np.nonzero(pix)
    if ys.size == 0:
        return np.empty((0, 2), dtype=np.float64)
    # Many layers have millions of lit pixels; a stride is enough for the hull
    # and keeps the Delaunay step fast.
    stride = max(1, ys.size // 200_000)
    pts = np.column_stack([xs[::stride], ys[::stride]]).astype(np.float64)
    hull = ConvexHull(pts)
    return pts[hull.vertices]


def _raster_polygon(points: np.ndarray) -> np.ndarray:
    from PIL import Image, ImageDraw

    img = Image.new("L", (COLS, ROWS), 0)
    ImageDraw.Draw(img).polygon([tuple(p) for p in points], fill=1)
    return np.asarray(img, dtype=np.uint8)


def hull_bitmap(pix: np.ndarray, centre: tuple[float, float] | None = None) -> np.ndarray:
    """Rasterise the convex hull of the lit pixels (+ plate centre)."""
    verts = _hull_vertices(pix)
    if verts.size == 0:
        return np.zeros((ROWS, COLS), dtype=np.uint8)
    if centre is not None:
        verts = np.vstack([verts, [centre[0], centre[1]]])
    # The points are already a convex polygon; adding the centre keeps the
    # convex hull valid and avoids a second triangulation.
    from scipy.spatial import ConvexHull
    hull = ConvexHull(verts)
    return _raster_polygon(verts[hull.vertices])


def hybrid_bitmap(pix: np.ndarray, centre: tuple[float, float] = (COLS / 2, ROWS / 2)) -> np.ndarray:
    """Hull of centre+layer, plus the cone only below the layer's last row.

    Large layers already reach the bottom of the image, so the cone is not
    allowed to widen their sides; thin high layers get the downward skirt.
    """
    h = hull_bitmap(pix, centre=centre)
    c = cone_bitmap(pix, centre=centre)
    ys = np.nonzero(pix)[0]
    if ys.size == 0:
        return h
    below = np.zeros((ROWS, COLS), dtype=np.uint8)
    below[int(ys.max()) + 1:, :] = 1
    return np.maximum(h, c & below)


def cone_bitmap(pix: np.ndarray, centre: tuple[float, float] = (COLS / 2, ROWS / 2)) -> np.ndarray:
    """Rasterise the central projection (shadow/cone) of the layer hull.

    Each hull vertex is projected from `centre` through the vertex to the
    plate boundary.  The convex hull of the centre, the layer, and those exit
    points is the cone that the layer subtends; it adds the far-side skirt
    that a plain layer hull cannot contain.
    """
    verts = _hull_vertices(pix)
    if verts.size == 0:
        return np.zeros((ROWS, COLS), dtype=np.uint8)
    c = np.asarray(centre, dtype=np.float64)
    exits: list[np.ndarray] = []
    for p in verts:
        d = p - c
        if abs(d[0]) < 1e-9 and abs(d[1]) < 1e-9:
            continue
        ts: list[float] = []
        for axis, bound in ((0, 0.0), (0, COLS - 1.0),
                            (1, 0.0), (1, ROWS - 1.0)):
            if abs(d[axis]) < 1e-9:
                continue
            t = (bound - c[axis]) / d[axis]
            if t <= 1.0:
                continue
            q = c + t * d
            if -0.5 <= q[0] <= COLS - 0.5 and -0.5 <= q[1] <= ROWS - 0.5:
                ts.append(t)
        if ts:
            exits.append(c + min(ts) * d)
    pts = [c, *verts]
    if exits:
        pts.extend(exits)
    allpts = np.asarray(pts, dtype=np.float64)
    from scipy.spatial import ConvexHull
    hull = ConvexHull(allpts)
    return _raster_polygon(allpts[hull.vertices])


# ---------------------------------------------------------------------------
# Comparison
# ---------------------------------------------------------------------------

def bit_set(mask: bytes, order: str = "msb") -> set[int]:
    out: set[int] = set()
    for i in range(len(mask) * 8):
        bit = (7 - (i & 7)) if order == "msb" else (i & 7)
        if mask[i >> 3] & (1 << bit):
            out.add(i)
    return out


def compare(sim: bytes, ref: bytes, order: str = "msb") -> dict:
    s = bit_set(sim, order)
    r = bit_set(ref, order)
    inter = len(s & r)
    union = len(s | r)
    return {
        "sim": len(s),
        "ref": len(r),
        "inter": inter,
        "fp": len(s - r),
        "fn": len(r - s),
        "jaccard": inter / union if union else 1.0,
    }


def parse_layers(spec: str, count: int) -> list[int]:
    if spec == "all":
        return list(range(count))
    layers: list[int] = []
    for part in spec.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            a, b = part.split("-", 1)
            layers.extend(range(int(a), int(b) + 1))
        else:
            layers.append(int(part))
    return [i for i in layers if 0 <= i < count]


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--file", default=DEFAULT_FILE, help="reference .cxdlpv4")
    ap.add_argument("--lut-dir", default=DEFAULT_LUT_DIR,
                    help="directory with lut_range.bin / lut_bits.bin")
    ap.add_argument("--layers", default="0,100,400,529",
                    help="'all', comma list, or ranges like 0-531")
    ap.add_argument("--step", type=int, default=1,
                    help="sample every Nth layer when --layers all")
    ap.add_argument("--model", choices=("lit", "hull", "cone", "hybrid"), default="lit")
    ap.add_argument("--bit-order", choices=("msb", "lsb"), default="msb")
    ap.add_argument("--csv", help="write per-layer results to this CSV")
    args = ap.parse_args(argv)

    data = Path(args.file).read_bytes()
    hdr = parse_header(data)
    if (hdr["width"], hdr["height"]) != (COLS, ROWS):
        print(f"error: unexpected resolution {hdr['width']}x{hdr['height']}",
              file=sys.stderr)
        return 2
    rng, bits = load_lut(args.lut_dir)
    layers = parse_layers(args.layers, hdr["layer_count"])
    if args.step > 1:
        layers = layers[::args.step]
    if not layers:
        print("error: no layers selected", file=sys.stderr)
        return 2

    rows = []
    for idx in layers:
        pix = load_layer_pairs(data, hdr, idx)
        if args.model == "hull":
            pix = hull_bitmap(pix, centre=(COLS / 2, ROWS / 2))
        elif args.model == "cone":
            pix = cone_bitmap(pix, centre=(COLS / 2, ROWS / 2))
        elif args.model == "hybrid":
            pix = hybrid_bitmap(pix, centre=(COLS / 2, ROWS / 2))
        sim = lit_mask(pix, rng, bits)
        ref = stored_mask(data, hdr, idx)
        stats = compare(sim, ref, args.bit_order)
        stats["layer"] = idx
        rows.append(stats)
        print(
            f"L{idx:3d}  ref={stats['ref']:4d}  sim={stats['sim']:4d}  "
            f"inter={stats['inter']:4d}  fp={stats['fp']:3d}  fn={stats['fn']:3d}  "
            f"J={stats['jaccard']:.4f}"
        )

    jac = np.array([r["jaccard"] for r in rows], dtype=float)
    print(
        f"\n{len(rows)} layers  model={args.model}  "
        f"J mean={jac.mean():.4f}  min={jac.min():.4f}  max={jac.max():.4f}"
    )
    if args.csv:
        with open(args.csv, "w", newline="") as fh:
            writer = csv.DictWriter(
                fh, fieldnames=["layer", "ref", "sim", "inter", "fp", "fn", "jaccard"]
            )
            writer.writeheader()
            writer.writerows(rows)
        print(f"wrote {args.csv}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
