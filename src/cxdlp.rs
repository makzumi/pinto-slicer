//! Creality CXDLP v3 and v4 writers.
//!
//! Layouts follow UVtools' CrealityCXDLPFile and CrealityCXDLPv4File. In
//! particular, v4 is not a small "CXDLP" header with byte-pair RLE: it starts
//! with `CXSW3DV2`, carries mixed-endian printer/header records, a 40-byte
//! layer table plus 44-byte per-layer records, and a final big-endian CRC32.
//! HALOT-X1 layers use its row-bounded 3-bit pixel-pair RLE.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::config::{PrintSettings, PrinterProfile};
use crate::formats::{self, Progress};
use crate::slicer::SliceResult;

const MAGIC: &[u8; 9] = b"CXSW3DV2\0";
const PRINT_PARAMETERS_SIZE: usize = 68;
const SLICER_INFO_SIZE: usize = 76;
const LAYER_DEF_EX_SIZE: usize = 44;
/// HALOT Box writes 120 bytes per layer record: the 40 that UVtools knows
/// about, then 80 bytes whose only content is a handful of fixed bytes. The
/// printer walks the table with that stride, so a 40-byte table makes it read
/// every third layer as the next one and refuse the file.
const LAYER_RECORD_SIZE: usize = 120;
const LAYER_RECORD_TAIL: [u8; 80] = {
    let mut tail = [0u8; 80];
    // HALOT Box sets 6 in the word at +66 and 0x180 in the word at +78 of
    // every record, bottom layers and normal ones alike. Nothing documents
    // them; matching the only writer the printer trusts is cheaper than
    // guessing what they mean.
    tail[26] = 0x06;
    tail[30] = 0x70;
    tail[33] = 0x07;
    tail[34] = 0x80;
    tail[37] = 0x38;
    tail[38] = 0x80;
    tail[39] = 0x01;
    tail
};
/// The printer expects the model size as three floats right after the header,
/// before the first thumbnail record.
const MODEL_SIZE_BYTES: u32 = 12;
const PREVIEW_SIZE: usize = 32;
const PREVIEW_SMALL: usize = 120;
const PREVIEW_LARGE: usize = 300;
/// HALOT Box's own file says 1.619 ml weighs 1.781 g, which is 1.1 g/ml —
/// the density every resin quote rounds to anyway.
const RESIN_GRAMS_PER_ML: f32 = 1.1;

/// Round to the three decimals the Creality slots carry, so our numbers read
/// back the way theirs do.
fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

fn be16(w: &mut impl Write, v: u16) -> std::io::Result<()> {
    w.write_all(&v.to_be_bytes())
}
fn be32(w: &mut impl Write, v: u32) -> std::io::Result<()> {
    w.write_all(&v.to_be_bytes())
}
fn le16(w: &mut impl Write, v: u16) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn le32(w: &mut impl Write, v: u32) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn lef(w: &mut impl Write, v: f32) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

fn cstring_be(w: &mut impl Write, value: &str) -> std::io::Result<()> {
    be32(w, value.len() as u32 + 1)?;
    w.write_all(value.as_bytes())?;
    w.write_all(&[0])
}

/// Creality uses the CRC32 polynomial with an initial register of zero and no
/// final XOR (unlike the conventional crc32fast defaults).
fn crc32(bytes: &[u8], mut state: u32) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            let mut value = i as u32;
            for _ in 0..8 {
                value = if value & 1 != 0 {
                    (value >> 1) ^ 0xEDB8_8320
                } else {
                    value >> 1
                };
            }
            *entry = value;
        }
        table
    });
    for byte in bytes {
        let index = ((state as u8) ^ *byte) as usize;
        state = table[index] ^ (state >> 8);
    }
    state
}

fn append_checksum(file: &mut std::fs::File) -> Result<(), String> {
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut state = 0;
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        state = crc32(&buf[..n], state);
    }
    // Do not rely on the position left by the final zero-byte read.  Seeking
    // explicitly also makes this correct for every File implementation.
    file.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    file.write_all(&state.to_be_bytes())
        .map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

fn validate_v4_layers(definitions: &[(u32, u32)], payload_end: u64) -> Result<(), String> {
    for (index, &(address, size)) in definitions.iter().enumerate() {
        let end = u64::from(address)
            .checked_add(u64::from(size))
            .ok_or_else(|| format!("CXDLPv4 layer {index} address overflow"))?;
        if u64::from(address) < u64::from(LAYER_DEF_EX_SIZE as u32) || end > payload_end {
            return Err(format!(
                "CXDLPv4 layer {index} points outside the file: address={address}, size={size}, payload={payload_end}"
            ));
        }
        if size < LAYER_DEF_EX_SIZE as u32 {
            return Err(format!("CXDLPv4 layer {index} has an invalid size {size}"));
        }
    }
    Ok(())
}

/// The seven-bit gray RLE shared by classic CXDLPv4 and CTB.
pub(crate) fn rle7(mask: &[u8]) -> Vec<u8> {
    fn add(out: &mut Vec<u8>, mut count: usize, gray: u8) {
        while count > 0 {
            let run = count.min(0x0FFF_FFFF);
            out.push((gray >> 1) | if run > 1 { 0x80 } else { 0 });
            if run > 1 {
                if run <= 0x7f {
                    out.push(run as u8);
                } else if run <= 0x3fff {
                    out.push(((run >> 8) as u8) | 0x80);
                    out.push(run as u8);
                } else if run <= 0x1f_ffff {
                    out.push(((run >> 16) as u8) | 0xc0);
                    out.push((run >> 8) as u8);
                    out.push(run as u8);
                } else {
                    out.push(((run >> 24) as u8) | 0xe0);
                    out.push((run >> 16) as u8);
                    out.push((run >> 8) as u8);
                    out.push(run as u8);
                }
            }
            count -= run;
        }
    }
    let mut out = Vec::with_capacity(mask.len() / 32 + 16);
    let mut at = 0;
    while at < mask.len() {
        let gray = mask[at] >> 1;
        let mut end = at + 1;
        while end < mask.len() && mask[end] >> 1 == gray {
            end += 1;
        }
        add(&mut out, end - at, gray << 1);
        at = end;
    }
    out
}

/// HALOT-X1: two 3-bit pixels per unit, high bit set, then an optional
/// big-endian base-128 repeat count. Runs never cross a row.
pub(crate) fn pixel_pair_rle(mask: &[u8], width: usize) -> Result<Vec<u8>, String> {
    if width == 0 || !width.is_multiple_of(2) || !mask.len().is_multiple_of(width) {
        return Err("HALOT-X1 pixel-pair encoding needs an even image width".into());
    }
    let units_per_row = width / 2;
    let unit =
        |row: &[u8], at: usize| -> u8 { 0x80 | (row[at * 2] >> 5) | ((row[at * 2 + 1] >> 5) << 3) };
    let mut out = Vec::with_capacity(mask.len() / 64 + 16);
    for row in mask.chunks_exact(width) {
        let mut at = 0;
        while at < units_per_row {
            let value = unit(row, at);
            let mut end = at + 1;
            while end < units_per_row && unit(row, end) == value {
                end += 1;
            }
            out.push(value);
            let repeat = end - at - 1;
            if repeat > 0 {
                let groups = (usize::BITS - repeat.leading_zeros()).div_ceil(7).max(1);
                for group in (0..groups).rev() {
                    out.push(((repeat >> (group * 7)) & 0x7f) as u8);
                }
            }
            at = end;
        }
    }
    Ok(out)
}

#[cfg(test)]
fn decode_pixel_pair(rle: &[u8], width: usize, height: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(width * height);
    let mut at = 0;
    let mut unit_index = 0usize;
    let units_per_row = width / 2;
    while at < rle.len() {
        let code = rle[at];
        at += 1;
        if code & 0x80 == 0 || code & 0x40 != 0 {
            return Err("bad pixel-pair unit".into());
        }
        let remaining = units_per_row - unit_index % units_per_row;
        let mut repeat = 0usize;
        while at < rle.len() && rle[at] & 0x80 == 0 {
            repeat = (repeat << 7) | rle[at] as usize;
            at += 1;
            if repeat >= remaining {
                return Err("run crossed a row".into());
            }
        }
        let left = code & 7;
        let right = (code >> 3) & 7;
        let expand = |g: u8| (g << 5) | (g << 2) | (g >> 1);
        for _ in 0..repeat + 1 {
            out.push(expand(left));
            out.push(expand(right));
        }
        unit_index += repeat + 1;
    }
    if out.len() != width * height {
        return Err("incomplete pixel-pair image".into());
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn rgb15_rle(pixels: &[u16]) -> Vec<u8> {
    const REPEAT_MASK: u16 = 0x0020;
    const LIMIT: usize = 0x0FFF;
    let mut out = Vec::new();
    let mut i = 0;
    while i < pixels.len() {
        let colour = pixels[i] & !REPEAT_MASK;
        let mut run = 1;
        while i + run < pixels.len() && (pixels[i + run] & !REPEAT_MASK) == colour && run < LIMIT {
            run += 1;
        }
        if run <= 2 {
            for _ in 0..run {
                out.extend_from_slice(&colour.to_le_bytes());
            }
        } else {
            out.extend_from_slice(&(colour | REPEAT_MASK).to_le_bytes());
            out.extend_from_slice(&(((run - 1) as u16) | 0x3000).to_le_bytes());
        }
        i += run;
    }
    out
}

/// A shaded 3-D view of everything the file prints, the way the printer's
/// own previews look: the sliced spans stacked as solid boxes, seen from a
/// corner above, lit from the upper left. Flat greys on black, drawn from
/// the spans rather than the 15 K masks, so it costs a fraction of the
/// export itself.
fn thumbnail(
    slice: &SliceResult,
    printer: &PrinterProfile,
    small: usize,
    large: usize,
) -> (Vec<u16>, Vec<u16>, [f32; 3]) {
    let bed = [
        [-printer.build[0] * 0.5, -printer.build[1] * 0.5],
        [printer.build[0] * 0.5, printer.build[1] * 0.5],
    ];
    // One scanline pass per layer turns its outlines into world-space
    // rectangles; tall files keep every other layer, which the eye cannot
    // tell from the full stack. The pass is shared by both sizes so the
    // plate is walked once, and a span whose contour never closes is
    // dropped rather than streaking across the plate.
    let step = if slice.layers.len() > 1200 { 2 } else { 1 };
    let pitch = (slice.layer_height.max(0.01)) * step as f32;
    let mut boxes: Vec<(f32, [f32; 4])> = Vec::new();
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    let mut z_lo = f32::MAX;
    let mut z_hi = f32::MIN;
    for layer in slice.layers.iter().step_by(step) {
        for r in crate::slicer::cap_rects_strict(layer, bed[0], bed[1], pitch) {
            lo[0] = lo[0].min(r[0]);
            lo[1] = lo[1].min(r[1]);
            hi[0] = hi[0].max(r[2]);
            hi[1] = hi[1].max(r[3]);
            z_lo = z_lo.min(layer.z);
            z_hi = z_hi.max(layer.z);
            boxes.push((layer.z, r));
        }
    }
    let bounds = if boxes.is_empty() {
        [0.0, 0.0, slice.height.max(0.0)]
    } else {
        [
            (hi[0] - lo[0]).max(0.0),
            (hi[1] - lo[1]).max(0.0),
            slice.height.max(0.0),
        ]
    };
    let small_pixels = if boxes.is_empty() {
        vec![0u16; small * small]
    } else {
        render_plate(&boxes, [lo, hi], [z_lo, z_hi], small)
    };
    let large_pixels = if boxes.is_empty() {
        vec![0u16; large * large]
    } else {
        render_plate(&boxes, [lo, hi], [z_lo, z_hi], large)
    };
    (small_pixels, large_pixels, bounds)
}

/// Where the eye sits: a corner three quarters around the plate, looking
/// down. The yaw shows two walls, the pitch shows the plate's floor
/// receding instead of a flat map.
const VIEW_YAW_DEG: f32 = 32.0;
const VIEW_PITCH_DEG: f32 = 28.0;
/// Towards the light, in plate millimetres: up, to the left, in front.
const LIGHT: [f32; 3] = [-0.34, -0.44, 0.83];
/// How far a top face's normal tips away from the face's centre at its
/// rim, so a box shades like a rounded surface instead of one flat card
/// that reads half white, half grey.
const DOME: f32 = 0.55;

/// Draw the stacked boxes with a z-buffer: each rectangle paints its top
/// face and the four walls that hang below it, whichever wall the camera
/// can see is decided by depth, exactly the way a solid hides its own
/// far side.
fn render_plate(
    boxes: &[(f32, [f32; 4])],
    xy: [[f32; 2]; 2],
    zr: [f32; 2],
    size: usize,
) -> Vec<u16> {
    let yaw = VIEW_YAW_DEG.to_radians();
    let pitch = VIEW_PITCH_DEG.to_radians();
    let (cy, sy) = (yaw.cos(), yaw.sin());
    let (cp, sp) = (pitch.cos(), pitch.sin());
    // screen: x right, y up, depth towards the eye (bigger is nearer).
    let project = |p: [f32; 3]| -> [f32; 3] {
        let u = p[0] * cy - p[1] * sy;
        let v = p[0] * sy + p[1] * cy;
        [u, p[2] * cp - v * sp, p[2] * sp + v * cp]
    };
    // Fit the projected box of the whole plate into the frame with a quiet
    // margin, the way a camera auto-frames a part.
    let mut corner = [[f32::MAX, f32::MIN]; 2];
    for dx in [xy[0][0], xy[1][0]] {
        for dy in [xy[0][1], xy[1][1]] {
            for dz in [zr[0], zr[1]] {
                let q = project([dx, dy, dz]);
                corner[0][0] = corner[0][0].min(q[0]);
                corner[0][1] = corner[0][1].max(q[0]);
                corner[1][0] = corner[1][0].min(q[1]);
                corner[1][1] = corner[1][1].max(q[1]);
            }
        }
    }
    let margin = (size as f32 * 0.08).max(3.0);
    let room = (size as f32 - 2.0 * margin).max(1.0);
    let span = [
        (corner[0][1] - corner[0][0]).max(1e-6),
        (corner[1][1] - corner[1][0]).max(1e-6),
    ];
    let scale = (room / span[0]).min(room / span[1]);
    let mid = [
        (corner[0][0] + corner[0][1]) * 0.5,
        (corner[1][0] + corner[1][1]) * 0.5,
    ];
    let ox = size as f32 * 0.5 - mid[0] * scale;
    let oy = size as f32 * 0.5 + mid[1] * scale;
    let light_len = (LIGHT[0] * LIGHT[0] + LIGHT[1] * LIGHT[1] + LIGHT[2] * LIGHT[2]).sqrt();
    let light = [
        LIGHT[0] / light_len,
        LIGHT[1] / light_len,
        LIGHT[2] / light_len,
    ];
    let grey = |lit: f32| -> u16 {
        let g = ((0.30 + 0.70 * lit.clamp(0.0, 1.0)) * 31.0).round() as u16;
        (g << 11) | ((g * 2) << 5) | g
    };
    let cam: [f32; 4] = [cy, sy, cp, sp];

    let mut zbuf = vec![f32::MIN; size * size];
    let mut out = vec![0u16; size * size];
    let drop = 0.6_f32.max(0.0); // walls hang a little past their floor
    for (z, r) in boxes {
        let zf = *z;
        // Corners of the box's top face, in plate millimetres.
        let c = [
            [r[0], r[1], zf],
            [r[2], r[1], zf],
            [r[2], r[3], zf],
            [r[0], r[3], zf],
        ];
        let corners: Vec<[f32; 3]> = c.iter().map(|p| project(*p)).collect();
        // Top face: the normal tips away from the face centre, brightest in
        // the middle and darker towards the rim, the way a rounded surface
        // would catch the same light.
        let centre = [(r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5];
        let radii = [
            ((r[2] - r[0]) * 0.5).max(0.5),
            ((r[3] - r[1]) * 0.5).max(0.5),
        ];
        let dome = |p: [f32; 3]| -> u16 {
            let nx = (p[0] - centre[0]) / radii[0] * DOME;
            let ny = (p[1] - centre[1]) / radii[1] * DOME;
            let len = (nx * nx + ny * ny + 1.0).sqrt();
            grey((nx * light[0] + ny * light[1] + light[2]) / len)
        };
        tri(
            &mut zbuf, &mut out, size, scale, ox, oy, dome, cam, corners[0], corners[1], corners[2],
        );
        tri(
            &mut zbuf, &mut out, size, scale, ox, oy, dome, cam, corners[0], corners[2], corners[3],
        );
        // Four walls: each edge of the top face, dropped to the floor. The
        // wall darkens as it nears the plate, the way surfaces meet in
        // shadow.
        let below: Vec<[f32; 3]> = corners
            .iter()
            .map(|p| [p[0], p[1] - drop * cp, p[2] - drop * sp])
            .collect();
        for k in 0..4 {
            let k2 = (k + 1) % 4;
            // Outward normal: edge k runs corner[k] -> corner[k2].
            let n = match k {
                0 => [0.0, -1.0, 0.0], // y0 edge
                1 => [1.0, 0.0, 0.0],  // x1 edge
                2 => [0.0, 1.0, 0.0],  // y1 edge
                _ => [-1.0, 0.0, 0.0], // x0 edge
            };
            let flat = (n[0] * light[0] + n[1] * light[1]).max(0.0);
            let wall = move |p: [f32; 3]| -> u16 {
                let frac = ((zf - p[2]) / drop).clamp(0.0, 1.0);
                grey(flat * (1.0 - 0.35 * frac))
            };
            tri(
                &mut zbuf,
                &mut out,
                size,
                scale,
                ox,
                oy,
                wall,
                cam,
                corners[k],
                corners[k2],
                below[k2],
            );
            tri(
                &mut zbuf, &mut out, size, scale, ox, oy, wall, cam, corners[k], below[k2],
                below[k],
            );
        }
    }
    out
}

/// Fill one projected triangle into the z-buffered image: screen-space
/// edges, depth interpolated per pixel, first-come wins ties. A pixel
/// that survives the depth test asks `colour` where in the plate it sits,
/// so a face can shade differently across itself.
#[allow(clippy::too_many_arguments)]
fn tri(
    zbuf: &mut [f32],
    out: &mut [u16],
    size: usize,
    scale: f32,
    ox: f32,
    oy: f32,
    colour: impl Fn([f32; 3]) -> u16,
    cam: [f32; 4],
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
) {
    let to_px = |p: [f32; 3]| -> (f32, f32, f32) { (ox + p[0] * scale, oy - p[1] * scale, p[2]) };
    let (ax, ay, az) = to_px(a);
    let (bx, by, bz) = to_px(b);
    let (cx, cy, cz) = to_px(c);
    let minx = ax.min(bx).min(cx).floor().max(0.0) as i32;
    let maxx = ax.max(bx).max(cx).ceil().min(size as f32) as i32;
    let miny = ay.min(by).min(cy).floor().max(0.0) as i32;
    let maxy = ay.max(by).max(cy).ceil().min(size as f32) as i32;
    if minx >= maxx || miny >= maxy {
        return;
    }
    let area = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
    if area.abs() < 1e-9 {
        return;
    }
    let inv = 1.0 / area;
    for py in miny..maxy {
        for px in minx..maxx {
            let fx = px as f32 + 0.5;
            let fy = py as f32 + 0.5;
            let w0 = (bx - ax) * (fy - ay) - (by - ay) * (fx - ax);
            let w1 = (cx - bx) * (fy - by) - (cy - by) * (fx - bx);
            let w2 = (ax - cx) * (fy - cy) - (ay - cy) * (fx - cx);
            let (e0, e1, e2) = if area > 0.0 {
                (w0, w1, w2)
            } else {
                (-w0, -w1, -w2)
            };
            if e0 < 0.0 || e1 < 0.0 || e2 < 0.0 {
                continue;
            }
            // w0 pairs with C's depth, w1 with A's, w2 with B's; the
            // three weights always add up to twice the area.
            let t = (e1 * az + e2 * bz + e0 * cz) * inv * 0.5;
            let idx = py as usize * size + px as usize;
            if t > zbuf[idx] + 1e-6 {
                zbuf[idx] = t;
                // Unrotate the camera: screen x, screen y and depth back
                // to plate millimetres, so `colour` knows its address.
                let u = (fx - ox) / scale;
                let sy = (oy - fy) / scale;
                let v = t * cam[2] - sy * cam[3];
                let p = [
                    u * cam[0] + v * cam[1],
                    v * cam[0] - u * cam[1],
                    sy * cam[2] + t * cam[3],
                ];
                out[idx] = colour(p);
            }
        }
    }
}

fn write_v4_preview(file: &mut std::fs::File, size: usize, encoded: &[u8]) -> std::io::Result<()> {
    let offset = file.stream_position()? as u32 + PREVIEW_SIZE as u32;
    le32(file, size as u32)?;
    le32(file, size as u32)?;
    le32(file, offset)?;
    le32(file, encoded.len() as u32)?;
    for _ in 0..4 {
        le32(file, 0)?;
    }
    file.write_all(encoded)
}

/// Where each block landed, so the header can be filled in last.
struct V4Layout {
    small_preview: u32,
    large_preview: u32,
    layers: u32,
    params: u32,
    slicer: u32,
}

fn write_v4_header(
    file: &mut std::fs::File,
    printer: &PrinterProfile,
    settings: &PrintSettings,
    slice: &SliceResult,
    res: (usize, usize),
    layout: &V4Layout,
    x1: bool,
) -> std::io::Result<()> {
    be32(file, 9)?;
    file.write_all(MAGIC)?;
    be16(file, 4)?;
    cstring_be(file, &printer.machine_name)?;
    le16(file, res.0 as u16)?;
    le16(file, res.1 as u16)?;
    for v in [
        printer.build[0],
        printer.build[1],
        printer.build[2],
        slice.height,
        slice.layer_height,
    ] {
        lef(file, v)?;
    }
    le32(file, settings.bottom_layers)?;
    le32(file, layout.small_preview)?;
    le32(file, layout.layers)?;
    le32(file, slice.layers.len() as u32)?;
    le32(file, layout.large_preview)?;
    le32(
        file,
        (settings.estimated_minutes(slice.layers.len()) * 60.0).round() as u32,
    )?;
    le32(file, printer.mirror[0] as u32)?;
    le32(file, layout.params)?;
    le32(file, PRINT_PARAMETERS_SIZE as u32)?;
    // HALOT Box writes zero here for this machine, and its files are the ones
    // the printer is known to accept.
    le32(file, u32::from(!x1))?; // antialias level
    le16(file, 255)?;
    le16(file, 255)?;
    le32(file, 0)?; // encryption disabled
    le32(file, layout.slicer)?;
    le32(file, SLICER_INFO_SIZE as u32)
}

fn write_v4_params(
    file: &mut std::fs::File,
    settings: &PrintSettings,
    x1: bool,
) -> std::io::Result<()> {
    let lift = settings.rising_height.max(0.0);
    let speed = settings.motor_speed.max(1.0);
    for v in [
        lift,  // bottom lift height
        speed, // bottom lift speed
        lift,  // normal lift height
        speed, // normal lift speed
        speed, // retract speed
        0.0,   // volume (ml): filled in after the pixels are counted
        0.0,   // weight (g): filled in with the volume
        0.0,   // cost: no resin price is set anywhere, so it stays empty
        settings.light_off_delay,
        settings.light_off_delay,
    ] {
        lef(file, v)?;
    }
    le32(file, settings.bottom_layers)?;
    lef(file, settings.exposure)?;
    lef(file, settings.bottom_exposure)?;
    if x1 {
        // The four undocumented slots. HALOT Box writes 5, 120, 6 and 1 for
        // this machine and its files are the ones the printer accepts; one
        // of those numbers could plausibly gate the light or the motion on
        // normal layers, and zeros there are a real difference between our
        // file and a print that works. Copy them.
        lef(file, 5.0)?;
        lef(file, 120.0)?;
        lef(file, 6.0)?;
        le32(file, 1)?;
    } else {
        for _ in 0..4 {
            le32(file, 0)?;
        }
    }
    Ok(())
}

fn write_v4_slicer(
    file: &mut std::fs::File,
    settings: &PrintSettings,
    x1: bool,
) -> std::io::Result<()> {
    // Two-stage lift is not exposed by the UI; keep the secondary stage at zero
    // so the machine uses the primary motion from the print parameters and the
    // per-layer records.
    for v in [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] {
        lef(file, v)?;
    }
    // The machine keeps its motion in the print parameters, exactly like the
    // files HALOT Box writes for it; other machines get per-layer records.
    le32(file, u32::from(!x1))?;
    let minutes = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 60)
        .unwrap_or(0);
    le32(file, minutes as u32)?;
    le32(file, u32::from(!x1))?;
    // Creality's own slicer leaves the version at zero; UVtools guesses
    // 0x01060300 and never verified a write against the machine.
    le32(file, if x1 { 0 } else { 0x0106_0300 })?;
    if x1 {
        // HALOT Box puts zero in the slot UVtools calls ExposureTime, the
        // real bottom exposure in the slot UVtools calls BottomExposureTime,
        // and its real normal exposure in the last slot. The layer records
        // and the print parameters carry the exposure too, but those agree
        // with a working print, so if the firmware reads it from anywhere
        // else it has to be here. Mirror the placement.
        for v in [
            0.0, // rest time after retract (light-off is stored per layer)
            0.0, // rest time before lift
            0.0, // the slot UVtools calls ExposureTime: HALOT Box zeroes it
            settings.bottom_exposure,
            settings.exposure,
        ] {
            lef(file, v)?;
        }
    } else {
        for v in [
            0.0, // rest time after retract (light-off is stored per layer)
            0.0, // rest time before lift
            settings.exposure,
            settings.bottom_exposure,
            0.0, // rest time after lift
        ] {
            lef(file, v)?;
        }
    }
    le32(file, 0)?; // transition layers
    le32(file, 0)?;
    le32(file, 0)
}

pub fn write_v4(
    path: &Path,
    printer: &PrinterProfile,
    settings: &PrintSettings,
    slice: &SliceResult,
    res: (usize, usize),
    progress: Progress<'_>,
) -> Result<Option<usize>, String> {
    let total = slice.layers.len();
    let x1 = printer.machine_name.eq_ignore_ascii_case("HALOT-X1")
        || printer.machine_name.eq_ignore_ascii_case("R009")
        || res == (15120, 6230);
    let header_size = 4 + MAGIC.len() + 2 + 4 + printer.machine_name.len() + 1 + 80;
    let (small_preview, large_preview, model_size) =
        thumbnail(slice, printer, PREVIEW_SMALL, PREVIEW_LARGE);
    let small_encoded = rgb15_rle(&small_preview);
    let large_encoded = rgb15_rle(&large_preview);
    let small_offset = header_size as u32 + MODEL_SIZE_BYTES;
    let large_offset = small_offset + PREVIEW_SIZE as u32 + small_encoded.len() as u32;
    let params_offset = large_offset + PREVIEW_SIZE as u32 + large_encoded.len() as u32;
    let slicer_offset = params_offset + PRINT_PARAMETERS_SIZE as u32;
    let layers_offset = slicer_offset + SLICER_INFO_SIZE as u32;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| formats::io(path, e))?;
    file.write_all(&vec![0; header_size])
        .map_err(|e| formats::io(path, e))?;
    for v in model_size {
        lef(&mut file, v).map_err(|e| formats::io(path, e))?;
    }
    write_v4_preview(&mut file, PREVIEW_SMALL, &small_encoded).map_err(|e| formats::io(path, e))?;
    write_v4_preview(&mut file, PREVIEW_LARGE, &large_encoded).map_err(|e| formats::io(path, e))?;
    write_v4_params(&mut file, settings, x1).map_err(|e| formats::io(path, e))?;
    write_v4_slicer(&mut file, settings, x1).map_err(|e| formats::io(path, e))?;
    file.write_all(&vec![0; total * LAYER_RECORD_SIZE])
        .map_err(|e| formats::io(path, e))?;

    let mut definitions = Vec::with_capacity(total);
    let mut buf = Vec::new();
    // Every lit pixel is a voxel of cured resin, so the volume the printer
    // should announce comes straight off the masks. Counting while the layers
    // stream keeps the estimate in step with what the file actually cures.
    let mut lit_pixels: u64 = 0;
    for (i, layer) in slice.layers.iter().enumerate() {
        formats::mask(layer, printer, res, &mut buf);
        lit_pixels += buf.iter().filter(|v| **v != 0).count() as u64;
        let encoded = if x1 {
            pixel_pair_rle(&buf, res.0)?
        } else {
            rle7(&buf)
        };
        let data_address = file.stream_position().map_err(|e| formats::io(path, e))? as u32;
        let bottom = (i as u32) < settings.bottom_layers;
        write_layer_motion(&mut file, settings).map_err(|e| formats::io(path, e))?;
        file.write_all(&encoded).map_err(|e| formats::io(path, e))?;
        definitions.push((
            (i + 1) as f32 * slice.layer_height,
            if bottom {
                settings.bottom_exposure
            } else {
                settings.exposure
            },
            settings.light_off_delay,
            data_address,
            (LAYER_DEF_EX_SIZE + encoded.len()) as u32,
        ));
        if !progress(i + 1, total) {
            return Ok(None);
        }
    }

    // The parameters block was written before the pixels existed, so the
    // resin estimate goes in now, through the same seek-and-rewrite the
    // header and the layer table already use. HALOT Box puts the volume, the
    // weight and a price there; with no price configured the cost stays zero.
    let data_end = file.stream_position().map_err(|e| formats::io(path, e))?;
    let pixel_mm2 = (printer.build[0].max(0.0) / res.0.max(1) as f32)
        * (printer.build[1].max(0.0) / res.1.max(1) as f32);
    let volume_ml = round3(lit_pixels as f32 * pixel_mm2 * slice.layer_height / 1000.0);
    file.seek(SeekFrom::Start(params_offset as u64 + 20))
        .map_err(|e| formats::io(path, e))?;
    lef(&mut file, volume_ml).map_err(|e| formats::io(path, e))?;
    lef(&mut file, round3(volume_ml * RESIN_GRAMS_PER_ML)).map_err(|e| formats::io(path, e))?;
    file.seek(SeekFrom::Start(data_end))
        .map_err(|e| formats::io(path, e))?;

    // HALOT Box repeats every layer's motion block once more, as a table after
    // the pixel data. Nothing documents it, but the printer reads its own
    // files, so the shape has to match.
    for _ in 0..total {
        write_layer_motion(&mut file, settings).map_err(|e| formats::io(path, e))?;
    }

    file.seek(SeekFrom::Start(0))
        .map_err(|e| formats::io(path, e))?;
    write_v4_header(
        &mut file,
        printer,
        settings,
        slice,
        res,
        &V4Layout {
            small_preview: small_offset,
            large_preview: large_offset,
            layers: layers_offset,
            params: params_offset,
            slicer: slicer_offset,
        },
        x1,
    )
    .map_err(|e| formats::io(path, e))?;
    file.seek(SeekFrom::Start(layers_offset as u64))
        .map_err(|e| formats::io(path, e))?;
    let layer_ranges: Vec<(u32, u32)> = definitions
        .iter()
        .map(|(_, _, _, address, size)| (*address, *size))
        .collect();
    validate_v4_layers(&layer_ranges, data_end).map_err(|e| formats::io(path, e))?;
    for (z, exposure, off, address, size) in definitions {
        lef(&mut file, z).map_err(|e| formats::io(path, e))?;
        lef(&mut file, exposure).map_err(|e| formats::io(path, e))?;
        lef(&mut file, off).map_err(|e| formats::io(path, e))?;
        le32(&mut file, address).map_err(|e| formats::io(path, e))?;
        le32(&mut file, size).map_err(|e| formats::io(path, e))?;
        le32(&mut file, 0).map_err(|e| formats::io(path, e))?; // pixel data
        le32(&mut file, 0).map_err(|e| formats::io(path, e))?; // centroid distance
        le32(&mut file, 0).map_err(|e| formats::io(path, e))?; // largest area
        le32(&mut file, 1).map_err(|e| formats::io(path, e))?; // has an image
        le32(&mut file, 0).map_err(|e| formats::io(path, e))?;
        file.write_all(&LAYER_RECORD_TAIL)
            .map_err(|e| formats::io(path, e))?;
    }
    file.seek(SeekFrom::End(0))
        .map_err(|e| formats::io(path, e))?;
    append_checksum(&mut file)?;
    Ok(Some(total))
}

fn write_layer_motion(file: &mut std::fs::File, settings: &PrintSettings) -> std::io::Result<()> {
    let lift = settings.rising_height.max(0.0);
    let speed = settings.motor_speed.max(1.0);
    for v in [
        lift,  // lift height
        speed, // lift speed
        0.0,   // secondary lift height
        0.0,   // secondary lift speed
        speed, // retract speed
        0.0,   // secondary retract height
        0.0,   // secondary retract speed
        0.0,   // rest before lift
        0.0,   // rest after lift
        0.0,   // rest after retract (light-off lives in the layer def)
        255.0, // light PWM
    ] {
        lef(file, v)?;
    }
    Ok(())
}

fn utf16be_string(w: &mut impl Write, value: &str) -> std::io::Result<()> {
    be32(w, value.encode_utf16().count() as u32 * 2)?;
    for unit in value.encode_utf16() {
        be16(w, unit)?;
    }
    Ok(())
}

/// Legacy CXDLP v3 stores vertical non-black runs, not the v4 bitmap RLE.
pub fn write_v3(
    path: &Path,
    printer: &PrinterProfile,
    settings: &PrintSettings,
    slice: &SliceResult,
    res: (usize, usize),
    progress: Progress<'_>,
) -> Result<Option<usize>, String> {
    let total = slice.layers.len();
    if total > u16::MAX as usize || res.0 >= 16384 || res.1 >= 8192 {
        return Err("legacy CXDLP exceeds its coordinate or layer-count limits".into());
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| formats::io(path, e))?;
    be32(&mut file, 9).map_err(|e| formats::io(path, e))?;
    file.write_all(MAGIC).map_err(|e| formats::io(path, e))?;
    be16(&mut file, 3).map_err(|e| formats::io(path, e))?;
    cstring_be(&mut file, &printer.machine_name).map_err(|e| formats::io(path, e))?;
    be16(&mut file, total as u16).map_err(|e| formats::io(path, e))?;
    be16(&mut file, res.0 as u16).map_err(|e| formats::io(path, e))?;
    be16(&mut file, res.1 as u16).map_err(|e| formats::io(path, e))?;
    file.write_all(&[0; 64]).map_err(|e| formats::io(path, e))?;
    // Three fixed-size RGB565 preview slots. Black previews are valid images;
    // omitting the slots would shift every subsequent record.
    for size in [116 * 116 * 2, 290 * 290 * 2, 290 * 290 * 2] {
        file.write_all(&vec![0; size])
            .map_err(|e| formats::io(path, e))?;
        file.write_all(b"\r\n").map_err(|e| formats::io(path, e))?;
    }
    utf16be_string(&mut file, &format!("{}", printer.build[0]))
        .map_err(|e| formats::io(path, e))?;
    utf16be_string(&mut file, &format!("{}", printer.build[1]))
        .map_err(|e| formats::io(path, e))?;
    utf16be_string(&mut file, &format!("{}", slice.layer_height))
        .map_err(|e| formats::io(path, e))?;
    for value in [
        // Exposure is in deciseconds. CL-60 speeds are integer mm/s;
        // UVtools rounds the reference 80/80/150 mm/min to 1/1/3 here.
        (settings.exposure.max(0.0) * 10.0).round() as u16,
        (settings.motor_speed / 60.0).round().max(1.0) as u16,
        settings.bottom_exposure.round().max(0.0) as u16,
        settings.bottom_layers.min(u16::MAX as u32) as u16,
        7,
        1,
        6,
        1,
        3,
        255,
        255,
    ] {
        be16(&mut file, value).map_err(|e| formats::io(path, e))?;
    }
    let area_table = file.stream_position().map_err(|e| formats::io(path, e))?;
    file.write_all(&vec![0; total * 4])
        .map_err(|e| formats::io(path, e))?;
    file.write_all(b"\r\n").map_err(|e| formats::io(path, e))?;
    cstring_be(&mut file, "Pinto Slicer 0.1").map_err(|e| formats::io(path, e))?;
    cstring_be(&mut file, "").map_err(|e| formats::io(path, e))?;
    file.write_all(&[0]).map_err(|e| formats::io(path, e))?;
    be32(&mut file, 600).map_err(|e| formats::io(path, e))?;
    be32(&mut file, 300000).map_err(|e| formats::io(path, e))?;
    file.write_all(&[1]).map_err(|e| formats::io(path, e))?;
    be16(&mut file, 0).map_err(|e| formats::io(path, e))?;
    file.write_all(&[0]).map_err(|e| formats::io(path, e))?;
    be16(&mut file, 1000).map_err(|e| formats::io(path, e))?;
    file.write_all(&[1, 1, 255, 0, 2])
        .map_err(|e| formats::io(path, e))?;
    file.write_all(b"\r\n").map_err(|e| formats::io(path, e))?;

    let mut areas = Vec::with_capacity(total);
    let mut buf = Vec::new();
    let pixel_area = printer.build[0] / res.0 as f32 * printer.build[1] / res.1 as f32;
    for (i, layer) in slice.layers.iter().enumerate() {
        formats::mask(layer, printer, res, &mut buf);
        let lit = buf.iter().filter(|v| **v != 0).count();
        areas.push((lit as f32 * pixel_area * 1000.0).round() as u32);
        let mut lines = Vec::new();
        for x in 0..res.0 {
            let mut y = 0;
            while y < res.1 {
                let gray = buf[y * res.0 + x];
                let start = y;
                y += 1;
                while y < res.1 && buf[y * res.0 + x] == gray {
                    y += 1;
                }
                if gray == 0 {
                    continue;
                }
                let start = start as u16;
                let end = (y - 1) as u16;
                let x = x as u16;
                lines.extend_from_slice(&[
                    ((start >> 5) & 0xff) as u8,
                    (((start << 3) + (end >> 10)) & 0xff) as u8,
                    ((end >> 2) & 0xff) as u8,
                    (((end << 6) + (x >> 8)) & 0xff) as u8,
                    x as u8,
                    gray,
                ]);
            }
        }
        be32(&mut file, areas[i]).map_err(|e| formats::io(path, e))?;
        be32(&mut file, (lines.len() / 6) as u32).map_err(|e| formats::io(path, e))?;
        file.write_all(&lines).map_err(|e| formats::io(path, e))?;
        file.write_all(b"\r\n").map_err(|e| formats::io(path, e))?;
        if !progress(i + 1, total) {
            return Ok(None);
        }
    }
    file.seek(SeekFrom::Start(area_table))
        .map_err(|e| formats::io(path, e))?;
    for area in areas {
        be32(&mut file, area).map_err(|e| formats::io(path, e))?;
    }
    file.seek(SeekFrom::End(0))
        .map_err(|e| formats::io(path, e))?;
    be32(&mut file, 9).map_err(|e| formats::io(path, e))?;
    file.write_all(MAGIC).map_err(|e| formats::io(path, e))?;
    append_checksum(&mut file)?;
    Ok(Some(total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_printers;
    use crate::mesh;
    use crate::slicer;

    #[test]
    fn pixel_pairs_round_trip_and_never_cross_rows() {
        let pixels = [0, 0, 0, 0, 255, 255, 0, 0, 224, 224, 224, 224];
        let rle = pixel_pair_rle(&pixels, 4).unwrap();
        let expected = [0, 0, 0, 0, 255, 255, 0, 0, 255, 255, 255, 255];
        assert_eq!(decode_pixel_pair(&rle, 4, 3).unwrap(), expected);
        assert!(pixel_pair_rle(&pixels, 3).unwrap_err().contains("even"));
    }

    #[test]
    fn seven_bit_rle_handles_long_runs() {
        let input = vec![255u8; 400];
        let rle = rle7(&input);
        assert_eq!(rle[0], 0xff);
        assert!(rle.len() < 10);
    }

    #[test]
    fn v4_has_the_real_header_layer_records_and_crc() {
        let sl = slicer::slice(&[mesh::sample_block()], 0.5);
        let path = std::env::temp_dir().join("resin_slicer_real.cxdlpv4");
        let n = write_v4(
            &path,
            &default_printers()[0],
            &PrintSettings::default(),
            &sl,
            (96, 64),
            &mut |_, _| true,
        )
        .unwrap()
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(u32::from_be_bytes(bytes[0..4].try_into().unwrap()), 9);
        assert_eq!(&bytes[4..13], MAGIC);
        assert_eq!(u16::from_be_bytes(bytes[13..15].try_into().unwrap()), 4);
        assert_eq!(u32::from_be_bytes(bytes[15..19].try_into().unwrap()), 9);
        assert_eq!(&bytes[19..28], b"HALOT-X1\0");
        assert_eq!(u16::from_le_bytes(bytes[28..30].try_into().unwrap()), 96);
        let layer_table = u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
        let data = u32::from_le_bytes(
            bytes[layer_table + 12..layer_table + 16]
                .try_into()
                .unwrap(),
        ) as usize;
        let size = u32::from_le_bytes(
            bytes[layer_table + 16..layer_table + 20]
                .try_into()
                .unwrap(),
        ) as usize;
        let decoded =
            decode_pixel_pair(&bytes[data + LAYER_DEF_EX_SIZE..data + size], 96, 64).unwrap();
        assert_eq!(decoded.len(), 96 * 64);
        let stored_crc = u32::from_be_bytes(bytes[bytes.len() - 4..].try_into().unwrap());
        assert_eq!(crc32(&bytes[..bytes.len() - 4], 0), stored_crc);
        assert_eq!(n, sl.layers.len());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn v4_header_layer_height_has_three_decimals() {
        let sl = slicer::slice(&[mesh::sample_block()], 0.049999997);
        let path = std::env::temp_dir().join("resin_slicer_layer_height.cxdlpv4");
        write_v4(
            &path,
            &default_printers()[0],
            &PrintSettings::default(),
            &sl,
            (96, 64),
            &mut |_, _| true,
        )
        .unwrap()
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let height = f32::from_le_bytes(bytes[48..52].try_into().unwrap());
        assert_eq!(height, 0.05);
        if std::env::var_os("KEEP_FORMAT_SAMPLES").is_none() {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn previews_round_trip_through_the_printer_codec() {
        let pixels: Vec<u16> = (0..120 * 120)
            .map(|i| if (i / 120) % 2 == 0 { 0xffff } else { 0x0000 })
            .collect();
        let encoded = rgb15_rle(&pixels);
        // Every row is one run, and a run costs four bytes.
        assert_eq!(encoded.len(), 120 * 4);
        let mut index = 0;
        let mut decoded = Vec::new();
        while index < encoded.len() {
            let word = u16::from_le_bytes([encoded[index], encoded[index + 1]]);
            index += 2;
            let colour = word & !0x0020;
            let mut repeat = 1usize;
            if word & 0x0020 != 0 {
                let count = u16::from_le_bytes([encoded[index], encoded[index + 1]]);
                repeat = (count & 0x0fff) as usize + 1;
                index += 2;
            }
            decoded.extend(std::iter::repeat_n(colour, repeat));
        }
        assert_eq!(decoded.len(), pixels.len());
        for (a, b) in decoded.iter().zip(pixels.iter()) {
            assert_eq!(a & !0x0020, b & !0x0020);
        }
    }

    #[test]
    fn v4_matches_the_shape_halot_box_writes() {
        let sl = slicer::slice(&[mesh::sample_block()], 0.5);
        let path = std::env::temp_dir().join("resin_slicer_shape.cxdlpv4");
        write_v4(
            &path,
            &default_printers()[0],
            &PrintSettings::default(),
            &sl,
            (96, 64),
            &mut |_, _| true,
        )
        .unwrap()
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let count = u32::from_le_bytes(bytes[64..68].try_into().unwrap()) as usize;
        let small = u32::from_le_bytes(bytes[56..60].try_into().unwrap()) as usize;
        let large = u32::from_le_bytes(bytes[68..72].try_into().unwrap()) as usize;
        let table = u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
        assert!(small > 0 && large > small, "both previews are written");
        // Right after the header: three floats with the model size, then the
        // small thumbnail record, whose pixels start 32 bytes later.
        let header_end =
            4 + MAGIC.len() + 2 + 4 + default_printers()[0].machine_name.len() + 1 + 80;
        assert_eq!(small, header_end + MODEL_SIZE_BYTES as usize);
        let image_offset = u32::from_le_bytes(bytes[small + 8..small + 12].try_into().unwrap());
        let image_length = u32::from_le_bytes(bytes[small + 12..small + 16].try_into().unwrap());
        assert_eq!(image_offset as usize, small + PREVIEW_SIZE);
        assert_eq!(large, image_offset as usize + image_length as usize);
        // The layer table is walked with a 120 byte stride and the pixel data
        // starts right after it, with HALOT Box's extra motion table and the
        // checksum at the end.
        assert_eq!(count, sl.layers.len());
        let mut address = 0;
        for i in 0..count {
            let record = table + i * LAYER_RECORD_SIZE;
            let layer = u32::from_le_bytes(bytes[record + 12..record + 16].try_into().unwrap());
            if i == 0 {
                assert_eq!(layer as usize, table + count * LAYER_RECORD_SIZE);
            }
            address = layer as usize;
            assert_eq!(
                &bytes[record + 40..record + LAYER_RECORD_SIZE],
                &LAYER_RECORD_TAIL
            );
        }
        let data_end = address
            + u32::from_le_bytes(
                bytes[table + (count - 1) * LAYER_RECORD_SIZE + 16
                    ..table + (count - 1) * LAYER_RECORD_SIZE + 20]
                    .try_into()
                    .unwrap(),
            ) as usize;
        assert_eq!(bytes.len() - 4 - data_end, count * LAYER_DEF_EX_SIZE);
        // The print parameters carry HALOT Box's four undocumented slots and
        // the slicer block mirrors where HALOT Box puts the exposures.
        let params_at = table - PRINT_PARAMETERS_SIZE - SLICER_INFO_SIZE;
        let paddings: Vec<u8> = [5.0f32, 120.0, 6.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .chain(1u32.to_le_bytes())
            .collect();
        assert_eq!(
            &bytes[params_at + 52..params_at + 68],
            &paddings[..],
            "HALOT Box's parameter slots are copied"
        );
        let slicer_at = table - SLICER_INFO_SIZE;
        assert_eq!(
            u32::from_le_bytes(bytes[slicer_at + 40..slicer_at + 44].try_into().unwrap()),
            0,
            "the software version stays at zero"
        );
        assert_eq!(
            f32::from_le_bytes(bytes[slicer_at + 52..slicer_at + 56].try_into().unwrap()),
            0.0,
            "the slot UVtools calls ExposureTime stays zero"
        );
        assert_eq!(
            f32::from_le_bytes(bytes[slicer_at + 56..slicer_at + 60].try_into().unwrap()),
            PrintSettings::default().bottom_exposure,
            "the bottom exposure sits where HALOT Box puts it"
        );
        assert_eq!(
            f32::from_le_bytes(bytes[slicer_at + 60..slicer_at + 64].try_into().unwrap()),
            PrintSettings::default().exposure,
            "the normal exposure sits in the last slot, like HALOT Box"
        );
        // The resin estimate counts what the file actually cures: decode every
        // layer back out of its own records and the volume has to match.
        let mut lit: u64 = 0;
        for i in 0..count {
            let record = table + i * LAYER_RECORD_SIZE;
            let data =
                u32::from_le_bytes(bytes[record + 12..record + 16].try_into().unwrap()) as usize;
            let size =
                u32::from_le_bytes(bytes[record + 16..record + 20].try_into().unwrap()) as usize;
            let pixels =
                decode_pixel_pair(&bytes[data + LAYER_DEF_EX_SIZE..data + size], 96, 64).unwrap();
            lit += pixels.iter().filter(|v| **v != 0).count() as u64;
        }
        let printer = &default_printers()[0];
        let pixel_mm2 = printer.build[0] / 96.0 * printer.build[1] / 64.0;
        let expected = round3(lit as f32 * pixel_mm2 * sl.layer_height / 1000.0);
        let volume = f32::from_le_bytes(bytes[params_at + 20..params_at + 24].try_into().unwrap());
        let weight = f32::from_le_bytes(bytes[params_at + 24..params_at + 28].try_into().unwrap());
        assert!(expected > 0.0, "the sample cures something");
        assert_eq!(volume, expected, "the volume matches the cured pixels");
        assert_eq!(weight, round3(expected * RESIN_GRAMS_PER_ML));
        if std::env::var_os("KEEP_FORMAT_SAMPLES").is_none() {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn v3_has_big_endian_magic_and_a_valid_crc() {
        let sl = slicer::slice(&[mesh::sample_block()], 0.5);
        let path = std::env::temp_dir().join("resin_slicer_real.cxdlp");
        write_v3(
            &path,
            &default_printers()[1],
            &PrintSettings::default(),
            &sl,
            (96, 64),
            &mut |_, _| true,
        )
        .unwrap()
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[4..13], MAGIC);
        assert_eq!(u16::from_be_bytes(bytes[13..15].try_into().unwrap()), 3);
        // Legacy motion uses integer mm/s and exposure uses deciseconds.
        let motion: Vec<u8> = [15_u16, 5, 30, 4, 7, 1, 6, 1, 3, 255, 255]
            .into_iter()
            .flat_map(u16::to_be_bytes)
            .collect();
        assert!(bytes.windows(motion.len()).any(|w| w == motion));
        let stored_crc = u32::from_be_bytes(bytes[bytes.len() - 4..].try_into().unwrap());
        assert_eq!(crc32(&bytes[..bytes.len() - 4], 0), stored_crc);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    #[ignore = "allocates and encodes the 94-megapixel HALOT-X1 panel"]
    fn halot_x1_writes_its_exact_native_panel() {
        let source = slicer::slice(&[mesh::sample_block()], 0.5);
        let sl = crate::slicer::SliceResult {
            layers: vec![source.layers[0].clone()],
            layer_height: 0.5,
            height: 0.5,
        };
        let printer = &default_printers()[0];
        let path = std::env::temp_dir().join("resin_slicer_native_x1.cxdlpv4");
        write_v4(
            &path,
            printer,
            &PrintSettings::default(),
            &sl,
            printer.native_res(),
            &mut |_, _| true,
        )
        .unwrap()
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(u16::from_le_bytes(bytes[28..30].try_into().unwrap()), 15120);
        assert_eq!(u16::from_le_bytes(bytes[30..32].try_into().unwrap()), 6230);
        if std::env::var_os("KEEP_FORMAT_SAMPLES").is_none() {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn the_preview_is_a_shaded_corner_view() {
        let printer = &default_printers()[0];
        let sl = slicer::slice(&[mesh::sample_block()], 0.5);
        let (small, large, bounds) = thumbnail(&sl, printer, PREVIEW_SMALL, PREVIEW_LARGE);
        assert_eq!(small.len(), PREVIEW_SMALL * PREVIEW_SMALL);
        assert_eq!(large.len(), PREVIEW_LARGE * PREVIEW_LARGE);
        // A corner view of a block is a rounded, shaded block on a black
        // field, not a flat silhouette: the dome alone spans several greys,
        // and the walls add their own.
        let lit = large.iter().filter(|p| **p != 0).count();
        assert!(
            lit > 3000,
            "the block should light much of the frame, was {lit}"
        );
        let mut shades: Vec<u16> = large.iter().copied().filter(|p| *p != 0).collect();
        shades.sort_unstable();
        shades.dedup();
        assert!(shades.len() >= 6, "want shaded faces, got {shades:?}");
        for p in &large {
            let (r, g, b) = (*p >> 11, (*p >> 5) & 0x3F, *p & 0x1F);
            assert_eq!(r, b, "preview pixels stay grey");
            assert_eq!(g, r * 2, "preview pixels stay grey");
        }
        // The same plate twice renders the same picture, both sizes.
        let again = thumbnail(&sl, printer, PREVIEW_SMALL, PREVIEW_LARGE);
        assert_eq!(small, again.0);
        assert_eq!(large, again.1);
        // The size the file reports is the size of what was sliced.
        assert!((bounds[0] - 16.0).abs() < 0.3, "width {bounds:?}");
        assert!((bounds[1] - 16.0).abs() < 0.3, "depth {bounds:?}");
        assert!((bounds[2] - sl.height).abs() < 1e-4, "height {bounds:?}");
        // An empty plate renders quiet and black.
        let empty = slicer::SliceResult::default();
        let (s, l, b) = thumbnail(&empty, printer, PREVIEW_SMALL, PREVIEW_LARGE);
        assert!(s.iter().all(|p| *p == 0));
        assert!(l.iter().all(|p| *p == 0));
        assert_eq!(b, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn cancellation_stops_before_a_complete_job() {
        let sl = slicer::slice(&[mesh::sample_block()], 0.5);
        let path = std::env::temp_dir().join("resin_slicer_stop.cxdlpv4");
        let result = write_v4(
            &path,
            &default_printers()[0],
            &PrintSettings::default(),
            &sl,
            (96, 64),
            &mut |i, _| i < 2,
        )
        .unwrap();
        assert_eq!(result, None);
        let _ = std::fs::remove_file(path);
    }
}
