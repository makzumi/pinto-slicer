#!/usr/bin/env python3
"""Project a mesh half-space onto the HALOT X1 bead LUT.

This is the prototype for the support-aware area mask: HALOT's 68-byte field
is the subzone projection of a white *area image*, not of the 3-bit layer
bitmap.  The area image is the model + support mesh clipped at the layer Z
(FlatWhite), optionally filtered (SepFilter), then converted to white runs.

The script is deliberately independent of the Rust writer.  It is a validation
tool for the area-part model; it does not modify `.cxdlpv4` files.

Typical use:

    # cube: 20 mm cube, centred on the plate
    python3 scripts/halot_re/area_projection.py --mesh samples/cube.stl --z 10

    # support column: a 10 mm column from z=0..10; layer 5 should include it
    python3 scripts/halot_re/area_projection.py --mesh /tmp/halot_re/column.stl --z 5
    python3 scripts/halot_re/area_projection.py --mesh /tmp/halot_re/column.stl --z 15

    # compare with a stored 4.6 mask (optional)
    python3 scripts/halot_re/area_projection.py --mesh samples/cube.stl --z 10 \
        --ref /path/to/reference.cxdlpv4 --ref-layer 200

The default image mapping is the plate centre (7560, 3115) with 50 px/mm,
which is close to the 15120x6230 HALOT X1 raster.  Use --scale/--center to
match a different printer profile.
"""
from __future__ import annotations

import argparse
import re
import struct
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import parity  # noqa: E402

COLS = parity.COLS
ROWS = parity.ROWS
MASK_BYTES = parity.MASK_BYTES
MAX_BIT = parity.MAX_BIT


# ---------------------------------------------------------------------------
# STL loading
# ---------------------------------------------------------------------------

def load_stl(path: str | Path) -> np.ndarray:
    """Return an (n, 3, 3) float32 array of triangles."""
    data = Path(path).read_bytes()
    text_head = data[:512].lstrip().lower()
    if text_head.startswith(b"solid") and b"facet" in text_head:
        return _load_ascii_stl(data)
    return _load_binary_stl(data)


def _load_binary_stl(data: bytes) -> np.ndarray:
    if len(data) < 84:
        raise ValueError("binary STL too short")
    count = struct.unpack("<I", data[80:84])[0]
    expected = 84 + 50 * count
    if len(data) < expected:
        raise ValueError(f"binary STL truncated: {len(data)} < {expected}")
    arr = np.frombuffer(data, dtype=np.uint8, count=expected, offset=0)
    rec = arr[84:].reshape(count, 50)
    floats = rec[:, :48].copy().view("<f4").reshape(count, 12)
    return floats[:, 3:12].reshape(count, 3, 3).astype(np.float32)


def _load_ascii_stl(data: bytes) -> np.ndarray:
    text = data.decode("latin1", errors="replace")
    verts = [
        tuple(float(v) for v in m.groups())
        for m in re.finditer(
            r"vertex\s+([-+0-9.eE]+)\s+([-+0-9.eE]+)\s+([-+0-9.eE]+)",
            text,
        )
    ]
    if len(verts) % 3:
        raise ValueError(f"ASCII STL has {len(verts)} vertices, not a multiple of 3")
    return np.asarray(verts, dtype=np.float32).reshape(-1, 3, 3)


# ---------------------------------------------------------------------------
# Half-space clipping / projection
# ---------------------------------------------------------------------------

def _clip_polygon_z(poly: list[tuple[float, float, float]], clip_z: float):
    """Sutherland-Hodgman clip of a polygon to z >= clip_z."""
    if not poly:
        return []
    out: list[tuple[float, float, float]] = []
    for i, cur in enumerate(poly):
        prev = poly[i - 1]
        cur_in = cur[2] >= clip_z
        prev_in = prev[2] >= clip_z
        if cur_in != prev_in:
            dz = prev[2] - cur[2]
            if abs(dz) > 1e-12:
                t = (clip_z - cur[2]) / (prev[2] - cur[2])
                out.append(
                    (
                        cur[0] + t * (prev[0] - cur[0]),
                        cur[1] + t * (prev[1] - cur[1]),
                        clip_z,
                    )
                )
        if cur_in:
            out.append(cur)
    return out


def rasterise_area(
    tris: np.ndarray,
    z: float,
    *,
    center: tuple[float, float],
    scale: float,
    flip_y: bool,
) -> np.ndarray:
    """Rasterise the orthographic projection of all triangles above z."""
    img = Image.new("L", (COLS, ROWS), 0)
    draw = ImageDraw.Draw(img)
    cx, cy = center
    for tri in tris:
        poly = _clip_polygon_z([tuple(map(float, p)) for p in tri], z)
        if len(poly) < 3:
            continue
        pts = []
        for x, y, _ in poly:
            u = cx + x * scale
            v = cy - y * scale if flip_y else cy + y * scale
            pts.append((u, v))
        draw.polygon(pts, fill=1)
    return np.asarray(img, dtype=np.uint8)


def apply_filter(area: np.ndarray, mode: str, radius: int) -> np.ndarray:
    """SepFilter approximation: separable max/min/box, radius in pixels."""
    if mode == "none" or radius <= 0:
        return area
    from scipy.ndimage import maximum_filter1d, minimum_filter1d, uniform_filter1d

    size = 2 * radius + 1
    if mode == "dilate":
        out = maximum_filter1d(area, size=size, axis=1, mode="constant", cval=0)
        out = maximum_filter1d(out, size=size, axis=0, mode="constant", cval=0)
        return (out > 0).astype(np.uint8)
    if mode == "erode":
        out = minimum_filter1d(area, size=size, axis=1, mode="constant", cval=0)
        out = minimum_filter1d(out, size=size, axis=0, mode="constant", cval=0)
        return (out > 0).astype(np.uint8)
    if mode == "blur":
        out = uniform_filter1d(area.astype(np.float32), size=size, axis=1, mode="constant", cval=0)
        out = uniform_filter1d(out, size=size, axis=0, mode="constant", cval=0)
        return (out >= 0.5).astype(np.uint8)
    raise ValueError(f"unknown filter mode {mode!r}")


# ---------------------------------------------------------------------------
# Bead mask / display
# ---------------------------------------------------------------------------

def row_start(row: int) -> int:
    """First bead index of a honeycomb row (29 beads on even rows)."""
    return 29 * row - row // 2


def row_width(row: int) -> int:
    return 29 if row % 2 == 0 else 28


def ascii_mask(mask: bytes) -> str:
    bits = parity.bit_set(mask, "msb")
    lines = []
    for row in range(19):
        start = row_start(row)
        width = row_width(row)
        line = "".join("#" if (start + c) in bits else "." for c in range(width))
        lines.append(f"{row:2d} {line}")
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--mesh", action="append", required=True,
                    help="STL file; may be repeated (model + supports)")
    ap.add_argument("--z", type=float, required=True, help="layer clip Z in mm")
    ap.add_argument("--lut-dir", default="/tmp/halot_re")
    ap.add_argument("--center", default="7560,3115",
                    help="image X,Y of model origin (default: plate centre)")
    ap.add_argument("--scale", type=float, default=50.0,
                    help="pixels per mm (default: 50)")
    ap.add_argument("--flip-y", action="store_true", default=True,
                    help="model +Y maps to image -Y (default: true)")
    ap.add_argument("--no-flip-y", dest="flip_y", action="store_false")
    ap.add_argument("--filter", choices=("none", "dilate", "erode", "blur"),
                    default="none")
    ap.add_argument("--radius", type=int, default=0)
    ap.add_argument("--save-png", default=None)
    ap.add_argument("--ref", default=None,
                    help="optional .cxdlpv4 with stored 68-byte masks")
    ap.add_argument("--ref-layer", type=int, default=0)
    args = ap.parse_args()

    center = tuple(float(v) for v in args.center.split(","))
    if len(center) != 2:
        ap.error("--center must be X,Y")

    tris = np.concatenate([load_stl(p) for p in args.mesh], axis=0)
    area = rasterise_area(
        tris, args.z,
        center=center, scale=args.scale, flip_y=args.flip_y,
    )
    area = apply_filter(area, args.filter, args.radius)

    rng, bits = parity.load_lut(args.lut_dir)
    mask = parity.lit_mask(area, rng, bits)
    n = len(parity.bit_set(mask, "msb"))
    print(f"mesh tris={len(tris)} z={args.z:g} mm filter={args.filter} "
          f"radius={args.radius} -> {n} beads")
    print(ascii_mask(mask))

    if args.ref:
        ref_data = Path(args.ref).read_bytes()
        hdr = parity.parse_header(ref_data)
        if args.ref_layer >= hdr["layer_count"]:
            ap.error(f"--ref-layer {args.ref_layer} >= {hdr['layer_count']}")
        ref = parity.stored_mask(ref_data, hdr, args.ref_layer)
        c = parity.compare(mask, ref)
        print(
            f"ref layer {args.ref_layer}: ref={c['ref']} sim={c['sim']} "
            f"inter={c['inter']} fp={c['fp']} fn={c['fn']} "
            f"J={c['jaccard']:.4f}"
        )

    if args.save_png:
        Image.fromarray((area * 255).astype(np.uint8)).save(args.save_png)
        print(f"wrote {args.save_png}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
