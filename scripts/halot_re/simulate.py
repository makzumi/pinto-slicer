#!/usr/bin/env python3
"""
Simulate HALOT X1 LED zone mask from bitmap using exact disassembly algorithm.
Loads LUT dumped from cxdlp.dll via Wine, reproduces func(0x1800270b0) + getsubzonedata.
"""
import numpy as np
import struct
import sys

# ---- LUT (dumped from dll via lutdump2.exe) ----
rng = np.fromfile('/tmp/halot_re/lut_range.bin', dtype=np.uint32)
bits = np.fromfile('/tmp/halot_re/lut_bits.bin', dtype=np.uint32)

# Constants from initLampBeadCal + LUT dump
EBP = 64       # column bands (grid cells per row)
EDI = 237      # pixels per band (COLS / EBP ≈ 236.25 → 237)
COLS = 15120
ROWS = 6230
MAXBIT = 542   # bits 0..541 (542 beads)

def func_mask(row, x, x2, mask):
    """Exact translation of 0x1800270b0"""
    sc = x // EDI
    ec = min(x2 // EDI, EBP - 1)
    if sc > ec:
        return
    base = row * EBP
    for col in range(sc, ec + 1):
        gi = base + col
        s, e = int(rng[gi]), int(rng[gi + 1])
        for b in bits[s:e]:
            b = int(b)
            if b >= MAXBIT:
                continue
            mask[b >> 3] |= 1 << (7 - (b & 7))  # MSB-first, bit index == bead index

def subzone(start, end, mask):
    """Exact translation of getsubzonedata (ord 357) -> func per row"""
    if end <= start:
        return
    row = start // COLS
    x = start % COLS
    end_row = (end - 1) // COLS
    end_x = (end - 1) % COLS
    while row < ROWS:
        x2 = (COLS - 1) if row != end_row else end_x
        func_mask(row, x, x2, mask)
        nxt = (row + 1) * COLS
        if nxt <= start or nxt >= end:
            break
        start = nxt
        row += 1
        x = 0

def compute_mask(bmp):
    """Compute mask from binary bitmap (uint8, ROWS x COLS, 0 or 255)"""
    mask = bytearray(68)
    for y in range(ROWS):
        rowpix = bmp[y]
        # pairs: lit if either pixel non-zero (exact encoder logic: cmpb $0,(%r8); cmova)
        pairs = rowpix[0::2] | rowpix[1::2]
        lit = pairs != 0
        if not lit.any():
            continue
        # find runs of True
        d = np.diff(lit.astype(np.int8))
        starts = np.where(d == 1)[0] + 1
        ends = np.where(d == -1)[0] + 1
        if lit[0]:
            starts = np.concatenate(([0], starts))
        if lit[-1]:
            ends = np.concatenate((ends, [len(lit)]))
        for s, e in zip(starts, ends):
            # LINEAR pixel offsets across whole image (exact encoder: rbx = cumulative pair index)
            subzone(y * COLS + 2 * int(s), y * COLS + 2 * int(e), mask)
    return mask

# ---- Real file parsing ----
def read_cstring_be(data, off):
    ln = struct.unpack('>I', data[off:off+4])[0]  # len+1
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
    return dict(v=v, magic=magic, ver=ver, name=name, res=(res0, res1),
                build=build, bottom=vals[0], small_preview=vals[1],
                layers_off=vals[2], layer_count=vals[3], large_preview=vals[4])

def real_mask(data, layer_idx, layers_off):
    rec = layers_off + layer_idx * 120
    addr = struct.unpack('<I', data[rec+12:rec+16])[0]
    size = struct.unpack('<I', data[rec+16:rec+20])[0]
    mask = data[rec+40:rec+108]  # first 68 bytes of 80-byte tail
    return mask, addr, size

def bitset(m):
    return {i for i in range(544) if m[i >> 3] & (1 << (7 - (i & 7)))}

if __name__ == '__main__':
    # Load real file
    f = '/run/media/makzumi/storage/Games/halot-box/drive_c/users/steamuser/AppData/Roaming/Creality/HalotBox/4.6.1.4147/17889895071359511442.cxdlpv4'
    data = open(f, 'rb').read()
    hdr = parse_header(data)
    print('header:', hdr['name'], hdr['res'], 'layers_off', hdr['layers_off'], 'count', hdr['layer_count'])

    # Load decoded layer 100 bitmap
    bmp = np.load('/tmp/halot_re/layer_100.npy')
    real, addr, size = real_mask(data, 100, hdr['layers_off'])
    print('layer100 real mask popcount:', sum(bin(x).count('1') for x in real), 'addr', addr, 'size', size)

    sim = compute_mask(bmp)
    print('sim mask popcount:', sum(bin(x).count('1') for x in sim))

    rb = bitset(real)
    sb = bitset(sim)
    print('real bits:', len(rb), 'sim bits:', len(sb))
    print('sim-only (false pos):', len(sb - rb))
    print('real-only (false neg):', len(rb - sb))
    if sb - rb:
        print('false pos bits:', sorted(sb - rb))
    if rb - sb:
        print('false neg bits:', sorted(rb - sb))
    print('intersection:', len(rb & sb))