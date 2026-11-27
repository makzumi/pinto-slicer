#!/usr/bin/env python3
"""
Extract and decode a specific layer from the real .cxdlpv4 file.
Uses exact pixel_pair_rle decode matching the encoder (binary 3-bit, not gray).
"""
import numpy as np
import struct
import sys

COLS = 15120
ROWS = 6230

def read_cstring_be(data, off):
    ln = struct.unpack('>I', data[off:off+4])[0]
    s = data[off+4:off+4+ln-1].decode('latin1')
    return s, off + 4 + ln

def parse_header(data):
    off = 0
    v = struct.unpack('>I', data[off:off+4])[0]; off += 4
    magic = data[off:off+9]; off += 9
    ver = struct.unpack('>H', data[off:off+2])[0]; off += 2
    name, off = read_cstring_be(data, off)
    res0 = struct.unpack('<H', data[off:off+2])[0]; off += 2
    res1 = struct.unpack('<H', data[off:off+2])[0]; off += 2
    build = struct.unpack('<5f', data[off:off+20]); off += 20
    vals = struct.unpack('<10I', data[off:off+40]); off += 40
    struct.unpack('<2H', data[off:off+4]); off += 4
    vals3 = struct.unpack('<3I', data[off:off+12]); off += 12
    return dict(name=name, res=(res0, res1), layers_off=vals[2], layer_count=vals[3])

def decode_pixel_pair(rle, width, height):
    """Exact decode matching encoder: pair byte = 0x80 | (L>0?0x07:0) | (R>0?0x38:0)"""
    out = np.zeros(width * height, dtype=np.uint8)
    at = 0
    unit_idx = 0
    units_per_row = width // 2
    while at < len(rle):
        code = rle[at]; at += 1
        if (code & 0x80) == 0 or (code & 0x40) != 0:
            raise ValueError(f"bad pixel-pair unit: {code:02x}")
        remaining = units_per_row - (unit_idx % units_per_row)
        repeat = 0
        while at < len(rle) and (rle[at] & 0x80) == 0:
            repeat = (repeat << 7) | rle[at]
            at += 1
            if repeat >= remaining:
                raise ValueError("run crossed a row")
        left = (code & 7) * 36   # 7 -> 252 ≈ 255 (exact: (g<<5)|(g<<2)|(g>>1) with g=7 = 224+28+3=255)
        right = ((code >> 3) & 7) * 36
        # Actually encoder uses 0x87 (0b10000111) for left lit, 0x38 (0b00111000) for right lit
        # Gray mapping: g=7 -> (7<<5)|(7<<2)|(7>>1) = 224+28+3 = 255
        # But we just need binary 0/255 for mask
        l_val = 255 if (code & 7) else 0
        r_val = 255 if (code & 0x38) else 0
        for _ in range(repeat + 1):
            out[unit_idx * 2] = l_val
            out[unit_idx * 2 + 1] = r_val
            unit_idx += 1
    if unit_idx * 2 != width * height:
        raise ValueError(f"incomplete: got {unit_idx*2}, expected {width*height}")
    return out.reshape(height, width)

def get_layer_rle(data, layer_idx, layers_off):
    rec = layers_off + layer_idx * 120
    addr = struct.unpack('<I', data[rec+12:rec+16])[0]
    size = struct.unpack('<I', data[rec+16:rec+20])[0]
    # Pixel data starts at rec+44 (after 44-byte def), length = size
    rle = data[rec+44:rec+44+size]
    return rle, addr, size

if __name__ == '__main__':
    if len(sys.argv) < 2:
        print("Usage: python decode_layer.py <layer_idx> [output.npy]")
        sys.exit(1)
    layer_idx = int(sys.argv[1])
    out_path = sys.argv[2] if len(sys.argv) > 2 else f'/tmp/halot_re/layer_{layer_idx}.npy'

    f = '/run/media/makzumi/storage/Games/halot-box/drive_c/users/steamuser/AppData/Roaming/Creality/HalotBox/4.6.1.4147/17889895071359511442.cxdlpv4'
    data = open(f, 'rb').read()
    hdr = parse_header(data)
    print(f"File: {hdr['name']} {hdr['res']} layers_off={hdr['layers_off']} count={hdr['layer_count']}")

    if layer_idx >= hdr['layer_count']:
        print(f"Layer {layer_idx} out of range (0..{hdr['layer_count']-1})")
        sys.exit(1)

    rle, addr, size = get_layer_rle(data, layer_idx, hdr['layers_off'])
    print(f"Layer {layer_idx}: addr={addr} size={size} rle_len={len(rle)}")

    bmp = decode_pixel_pair(rle, *hdr['res'])
    lit = int((bmp > 0).sum())
    print(f"Decoded: shape={bmp.shape}, lit={lit} ({lit/(COLS*ROWS)*100:.2f}%)")

    np.save(out_path, bmp)
    print(f"Saved to {out_path}")