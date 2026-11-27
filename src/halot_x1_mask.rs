// HALOT X1 LED zone mask (per-layer 544-bit backlight control)
// Geometry from cxdlp.dll reverse engineering (initLampBeadCal ord 371 + LUT dump)

pub const HALOT_X1_COLS: usize = 15120;
pub const HALOT_X1_ROWS: usize = 6230;
pub const HALOT_X1_EBP: usize = 64;       // grid column bands
pub const HALOT_X1_EDI: usize = 237;      // pixels per band (ceil(COLS/EBP))
pub const HALOT_X1_MAX_BIT: usize = 542;  // bits 0..541 (542 beads)
pub const HALOT_X1_MASK_BYTES: usize = 68; // (544+7)/8

// Hexagon geometry (pixel space, from getLampMask ord 339 + beaddata ord 355)
// Pointy-top hexagons: apothem W=291px, circumradius H=247px
// Grid: 19 rows alternating 29/28 beads = 542 total
// Row 0 center y = -123.7, row pitch = 371.09px, col pitch = 581.5px
// Odd rows offset by +290.8px
const HEX_W: f32 = 291.0;
const HEX_H: f32 = 247.0;
const ROW_PITCH: f32 = 371.09;
const COL_PITCH: f32 = 581.5;
const ODD_OFFSET: f32 = 290.8;
const ROW0_CENTER_Y: f32 = -123.7;
const N_ROWS: usize = 19;

/// Build the pixel-row × column-band → bead bit-index LUT for HALOT X1.
/// Returns (range, bits) where:
///   range[gi] .. range[gi+1] indexes into bits for cell (row, col_band)
///   gi = row * EBP + col_band
///   bits[] contains bead indices (0..541) whose hexagons overlap the cell
pub fn build_halot_x1_lut() -> (Vec<u32>, Vec<u32>) {
    if let Some(lut) = build_exact_lut() {
        return lut;
    }
    build_geometric_lut()
}

/// Exact bead subzone LUT dumped from HALOT BOX's `ensureBeadLut`.
/// Stored as a 29 KB zlib blob; counts are u8, bead ids are u16.
fn build_exact_lut() -> Option<(Vec<u32>, Vec<u32>)> {
    use std::io::Read;
    const EXACT_LUT_Z: &[u8] = include_bytes!("../assets/halot_x1_lut.z");
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(EXACT_LUT_Z)
        .read_to_end(&mut raw)
        .ok()?;
    if raw.len() < 16 || &raw[0..4] != b"HX1L" {
        return None;
    }
    let u32_at = |o: usize| -> Option<usize> {
        Some(u32::from_le_bytes(raw.get(o..o + 4)?.try_into().ok()?) as usize)
    };
    if u32_at(4)? != 1 {
        return None;
    }
    let n_cells = u32_at(8)?;
    let n_bits = u32_at(12)?;
    if n_cells != HALOT_X1_EBP * HALOT_X1_ROWS || raw.len() != 16 + n_cells + n_bits * 2 {
        return None;
    }
    let counts = &raw[16..16 + n_cells];
    let ids = &raw[16 + n_cells..];
    let mut range = Vec::with_capacity(n_cells + 1);
    range.push(0u32);
    let mut total = 0u32;
    for &c in counts {
        total += c as u32;
        range.push(total);
    }
    if total as usize != n_bits {
        return None;
    }
    let bits = ids
        .chunks_exact(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]) as u32)
        .collect();
    Some((range, bits))
}

/// Idealized hexagon fallback for when the exact dumped LUT is unavailable.
fn build_geometric_lut() -> (Vec<u32>, Vec<u32>) {
    let mut range = Vec::with_capacity(HALOT_X1_EBP * HALOT_X1_ROWS + 1);
    let mut bits = Vec::with_capacity(1_250_000);
    range.push(0); // range[0] = 0

    // Precompute bead centers and vertical spans
    struct BeadInfo {
        idx: u32,
        cx: f32,
        cy: f32,
        y_min: f32,
        y_max: f32,
    }
    let mut beads = Vec::with_capacity(HALOT_X1_MAX_BIT);
    let mut idx = 0u32;
    for r in 0..N_ROWS {
        let cy = ROW0_CENTER_Y + r as f32 * ROW_PITCH;
        let y_min = cy - HEX_H;
        let y_max = cy + HEX_H;
        let n_cols = if r % 2 == 0 { 29 } else { 28 };
        let x_start = if r % 2 == 0 { 0.0 } else { ODD_OFFSET };
        for c in 0..n_cols {
            let cx = x_start + c as f32 * COL_PITCH;
            beads.push(BeadInfo { idx, cx, cy, y_min, y_max });
            idx += 1;
        }
    }
    assert_eq!(idx, HALOT_X1_MAX_BIT as u32);

    // For each pixel row, each column band
    for row in 0..HALOT_X1_ROWS {
        let y = row as f32 + 0.5; // pixel center
        for col_band in 0..HALOT_X1_EBP {
            let x0 = col_band * HALOT_X1_EDI;
            let x1 = ((col_band + 1) * HALOT_X1_EDI).min(HALOT_X1_COLS);
            let x0f = x0 as f32;
            let x1f = x1 as f32;

            let _start = bits.len();
            // Check each bead for overlap with this cell's span [x0f, x1f) at row y
            for b in &beads {
                if y < b.y_min || y >= b.y_max {
                    continue;
                }
                // Horizontal overlap: bead hexagon spans [cx - W, cx + W] at this y
                // For pointy-top hexagon, horizontal half-width at y is W * (1 - |y-cy|/H)
                let dy = (y - b.cy).abs();
                if dy >= HEX_H {
                    continue;
                }
                let half_w = HEX_W * (1.0 - dy / HEX_H);
                let bx0 = b.cx - half_w;
                let bx1 = b.cx + half_w;
                if bx1 <= x0f || bx0 >= x1f {
                    continue;
                }
                bits.push(b.idx);
            }
            range.push(bits.len() as u32);
        }
    }

    // Verify dimensions
    assert_eq!(range.len(), HALOT_X1_EBP * HALOT_X1_ROWS + 1);
    assert!(bits.iter().all(|&b| b < HALOT_X1_MAX_BIT as u32));

    (range, bits)
}

/// Compute per-layer LED zone mask from binary bitmap (0/255).
/// buf: ROWS x COLS, row-major, 0=dark, 255=lit
/// Returns 68-byte mask (MSB-first, bit index == bead index)
pub fn compute_halot_x1_mask(buf: &[u8], range: &[u32], bits: &[u32]) -> [u8; HALOT_X1_MASK_BYTES] {
    let mut mask = [0u8; HALOT_X1_MASK_BYTES];

    for row in 0..HALOT_X1_ROWS {
        let row_start = row * HALOT_X1_COLS;
        let row_end = row_start + HALOT_X1_COLS;
        if row_end > buf.len() {
            break;
        }
        let rowpix = &buf[row_start..row_end];

        // Pairs: lit if either pixel non-zero (matches encoder: cmpb $0,(%r8); cmova)
        let mut pair_lit = [false; HALOT_X1_COLS / 2];
        for (i, pair) in rowpix.chunks_exact(2).enumerate() {
            pair_lit[i] = pair[0] != 0 || pair[1] != 0;
        }

        // Find runs of lit pairs
        let mut i = 0;
        while i < pair_lit.len() {
            if !pair_lit[i] {
                i += 1;
                continue;
            }
            let start_pair = i;
            while i < pair_lit.len() && pair_lit[i] {
                i += 1;
            }
            let end_pair = i; // exclusive

            // LINEAR pixel offsets across whole image (exact encoder semantics)
            let start = row * HALOT_X1_COLS + 2 * start_pair;
            let end = row * HALOT_X1_COLS + 2 * end_pair;

            // subzone(start, end, &mut mask, range, bits)
            subzone_halot_x1(start, end, &mut mask, range, bits);
        }
    }
    mask
}

/// Mark beads for a linear pixel range [start, end).
/// Exact translation of getsubzonedata (ord 357) -> func(0x1800270b0)
fn subzone_halot_x1(
    start: usize,
    end: usize,
    mask: &mut [u8; HALOT_X1_MASK_BYTES],
    range: &[u32],
    bits: &[u32],
) {
    if end <= start {
        return;
    }
    let mut row = start / HALOT_X1_COLS;
    let mut x = start % HALOT_X1_COLS;
    let end_row = (end - 1) / HALOT_X1_COLS;
    let end_x = (end - 1) % HALOT_X1_COLS;

    while row < HALOT_X1_ROWS {
        let x2 = if row == end_row { end_x } else { HALOT_X1_COLS - 1 };
        func_mask_halot_x1(row, x, x2, mask, range, bits);
        let nxt = (row + 1) * HALOT_X1_COLS;
        if nxt <= start || nxt >= end {
            break;
        }
        row += 1;
        x = 0;
    }
}

/// Mark beads for one row's pixel span [x, x2].
/// Exact translation of func(0x1800270b0)
fn func_mask_halot_x1(
    row: usize,
    x: usize,
    x2: usize,
    mask: &mut [u8; HALOT_X1_MASK_BYTES],
    range: &[u32],
    bits: &[u32],
) {
    let sc = x / HALOT_X1_EDI;
    let ec = (x2 / HALOT_X1_EDI).min(HALOT_X1_EBP - 1);
    if sc > ec {
        return;
    }
    let base = row * HALOT_X1_EBP;
    for col in sc..=ec {
        let gi = base + col;
        let s = range[gi] as usize;
        let e = range[gi + 1] as usize;
        for &b in &bits[s..e] {
            if b >= HALOT_X1_MAX_BIT as u32 {
                continue;
            }
            // MSB-first: bit b -> byte b/8, bit position 7-(b%8)
            mask[(b >> 3) as usize] |= 1 << (7 - (b & 7));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lut_builds_correct_dimensions() {
        let (range, bits) = build_halot_x1_lut();
        assert_eq!(range.len(), HALOT_X1_EBP * HALOT_X1_ROWS + 1);
        assert!(range[0] == 0);
        // Last entry should equal total bits
        assert_eq!(range[range.len() - 1] as usize, bits.len());
        // All bits should be valid bead indices
        for &b in &bits {
            assert!(b < HALOT_X1_MAX_BIT as u32);
        }
    }

    #[test]
    fn fully_lit_image_produces_expected_beads() {
        let (range, bits) = build_halot_x1_lut();
        // Fully lit image: all pixels = 255
        let buf = vec![255u8; HALOT_X1_COLS * HALOT_X1_ROWS];
        let mask = compute_halot_x1_mask(&buf, &range, &bits);
        let count: u32 = mask.iter().map(|b| b.count_ones()).sum();
        // The exact dumped HALOT LUT activates 522 of the 542 bead indices.
        assert_eq!(count, 522, "fully lit should hit 522 beads with the exact LUT, got {}", count);
    }

    #[test]
    fn exact_lut_matches_a_reference_cell() {
        let (range, bits) = build_halot_x1_lut();
        let gi = 1000 * HALOT_X1_EBP + 30;
        assert_eq!(
            &bits[range[gi] as usize..range[gi + 1] as usize],
            &[70, 71, 98, 99, 100, 127, 128]
        );
    }

    #[test]
    fn empty_image_produces_zero_mask() {
        let (range, bits) = build_halot_x1_lut();
        let buf = vec![0u8; HALOT_X1_COLS * HALOT_X1_ROWS];
        let mask = compute_halot_x1_mask(&buf, &range, &bits);
        let count: u32 = mask.iter().map(|b| b.count_ones()).sum();
        assert_eq!(count, 0, "empty image should produce zero mask");
    }

    #[test]
    fn single_pixel_lights_correct_beads() {
        let (range, bits) = build_halot_x1_lut();
        // Single pixel at center of bed
        let mut buf = vec![0u8; HALOT_X1_COLS * HALOT_X1_ROWS];
        let center_y = HALOT_X1_ROWS / 2;
        let center_x = HALOT_X1_COLS / 2;
        buf[center_y * HALOT_X1_COLS + center_x] = 255;
        let mask = compute_halot_x1_mask(&buf, &range, &bits);
        let count: u32 = mask.iter().map(|b| b.count_ones()).sum();
        // Should light a small number of beads (the ones covering that pixel)
        assert!(count > 0 && count < 10, "single pixel should light few beads, got {}", count);
    }
}
