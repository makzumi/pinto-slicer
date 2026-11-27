//! Slicing: cutting meshes into horizontal layers and turning a layer into a
//! mask picture.
//!
//! A layer is filled with the **non-zero winding rule**, not even-odd. That
//! matters because support parts are separate meshes that overlap each other
//! and the model: with even-odd the overlap would punch a hole, with non-zero
//! the overlap stays solid. To make non-zero work, every cross-section segment
//! is stored pointing the same way round the solid (see `cross_triangle`).

use std::sync::Arc;

use crate::mesh::Mesh;

const MAX_LAYERS: usize = 20000;

#[derive(Clone, Copy, Debug, Default)]
pub struct Seg {
    pub a: [f32; 2],
    pub b: [f32; 2],
}

#[derive(Clone, Debug, Default)]
pub struct Layer {
    pub z: f32,
    pub segs: Vec<Seg>,
}

#[derive(Clone, Debug, Default)]
pub struct SliceResult {
    pub layers: Vec<Layer>,
    pub layer_height: f32,
    pub height: f32,
    /// The meshes this result was sliced from.  The HALOT area-part mask is
    /// derived from the model + support geometry above each layer, so the
    /// writer needs the 3D source, not just the 2D segments.
    pub area_meshes: Arc<Vec<Mesh>>,
}

impl SliceResult {
    #[allow(dead_code)]
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }
}

/// Slice every mesh into 0.5 mm-ish layers. Supports are just meshes here, so
/// their outlines and the model's merge naturally.
pub fn slice(meshes: &[Mesh], layer_height: f32) -> SliceResult {
    // UVtools rejects layer heights with more than three decimal places, and
    // f32 slider math can land one ULP below a clean value (0.049999997 for
    // 0.05). Quantize at the slicer boundary so every writer emits the same
    // clean height and the layer positions stay consistent with the header.
    let h = ((layer_height.max(0.005) * 1000.0).round() / 1000.0).max(0.005);
    let mut top = 0.0f32;
    for m in meshes {
        if m.tris.is_empty() {
            continue;
        }
        let (min, max) = m.bounds();
        let _ = min;
        top = top.max(max.z);
    }
    if top <= 0.0 {
        return SliceResult {
            layers: Vec::new(),
            layer_height: h,
            height: 0.0,
            area_meshes: Arc::new(meshes.to_vec()),
        };
    }
    let count = ((top / h).ceil() as usize).clamp(1, MAX_LAYERS);
    let mut layers: Vec<Layer> = (0..count)
        .map(|i| Layer {
            z: (i as f32 + 0.5) * h,
            segs: Vec::new(),
        })
        .collect();

    for m in meshes {
        if !m.tris.is_empty() {
            slice_into(m, &mut layers, h);
        }
    }
    SliceResult {
        layers,
        layer_height: h,
        height: top,
        area_meshes: Arc::new(meshes.to_vec()),
    }
}

fn slice_into(mesh: &Mesh, layers: &mut [Layer], h: f32) {
    let n = layers.len();
    if n == 0 {
        return;
    }
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (ti, t) in mesh.tris.iter().enumerate() {
        let [a, b, c] = mesh.tri_verts(*t);
        let zmin = a.z.min(b.z).min(c.z);
        let zmax = a.z.max(b.z).max(c.z);
        if zmax < layers[0].z - h || zmin > layers[n - 1].z + h {
            continue;
        }
        let i0 = ((zmin / h) as i64 - 1).max(0) as usize;
        let i1 = (((zmax / h) as i64) + 1).clamp(0, n as i64 - 1) as usize;
        for bucket in buckets.iter_mut().take(i1 + 1).skip(i0) {
            bucket.push(ti as u32);
        }
    }
    for (li, bucket) in buckets.iter().enumerate() {
        if bucket.is_empty() {
            continue;
        }
        let z = layers[li].z;
        for &ti in bucket {
            if let Some(seg) = cross_triangle(mesh, mesh.tris[ti as usize], z) {
                layers[li].segs.push(seg);
            }
        }
    }
}

fn cross_triangle(mesh: &Mesh, t: [u32; 3], z: f32) -> Option<Seg> {
    let p = mesh.tri_verts(t);
    let d = [p[0].z - z, p[1].z - z, p[2].z - z];
    let mut pts: Vec<[f32; 2]> = Vec::with_capacity(3);
    for (i, j) in [(0usize, 1usize), (1, 2), (2, 0)] {
        if (d[i] < 0.0) != (d[j] < 0.0) {
            let denom = d[i] - d[j];
            if denom.abs() < 1e-9 {
                continue;
            }
            let s = d[i] / denom;
            pts.push([
                p[i].x + (p[j].x - p[i].x) * s,
                p[i].y + (p[j].y - p[i].y) * s,
            ]);
        }
    }
    if pts.len() != 2 {
        return None;
    }
    // Point the pair the same way round the solid: for an outward normal n,
    // the outline runs along (-n.y, n.x). Overlapping solids then add up
    // instead of cancelling when the mask is filled.
    let n = (p[1] - p[0]).cross(p[2] - p[0]);
    let along = [-n.y, n.x];
    let delta = [pts[1][0] - pts[0][0], pts[1][1] - pts[0][1]];
    let dot = delta[0] * along[0] + delta[1] * along[1];
    if dot < 0.0 {
        Some(Seg {
            a: pts[1],
            b: pts[0],
        })
    } else {
        Some(Seg {
            a: pts[0],
            b: pts[1],
        })
    }
}

/// Fill a layer into a grayscale mask (255 solid, 0 empty).
/// `min`/`max` are the world-space corners the picture covers.
pub fn fill_mask(
    layer: &Layer,
    min: [f32; 2],
    max: [f32; 2],
    w: usize,
    h: usize,
    out: &mut Vec<u8>,
) {
    out.clear();
    out.resize(w * h, 0);
    if layer.segs.is_empty() || w == 0 || h == 0 {
        return;
    }
    let sx = (max[0] - min[0]).max(1e-6);
    let sy = (max[1] - min[1]).max(1e-6);
    let sub = 2usize;
    let rows = h * sub;

    // Only the scanlines the layer can touch need work. This matters most for
    // the tiny layers at the top of a print, where the plate resolution is
    // still full size.
    let mut y_lo = f32::MAX;
    let mut y_hi = f32::MIN;
    for s in &layer.segs {
        y_lo = y_lo.min(s.a[1]).min(s.b[1]);
        y_hi = y_hi.max(s.a[1]).max(s.b[1]);
    }
    if y_hi <= y_lo {
        return;
    }
    let mut py_lo = (((y_lo - min[1]) / sy * rows as f32).floor().max(0.0) as usize).min(rows);
    let mut py_hi = (((y_hi - min[1]) / sy * rows as f32).ceil().max(0.0) as usize).min(rows);
    // Keep whole sub-scanline pairs together so the two samples of a pixel row
    // are always accumulated and scaled as a unit.
    py_lo &= !(sub - 1);
    py_hi = (py_hi + sub - 1) & !(sub - 1);
    py_hi = py_hi.min(rows);
    if py_hi <= py_lo {
        return;
    }
    let owned_rows = py_hi - py_lo;
    let mut counts = vec![0u8; w * owned_rows];

    // Sweep the segments in y instead of testing every segment on every
    // scanline: each one is added when its lower end is reached and dropped
    // once its upper end is passed.
    let lo = |s: &Seg| s.a[1].min(s.b[1]);
    let hi = |s: &Seg| s.a[1].max(s.b[1]);
    let mut order: Vec<usize> = (0..layer.segs.len()).collect();
    order.sort_by(|&a, &b| lo(&layer.segs[a]).total_cmp(&lo(&layer.segs[b])));
    let mut next = 0usize;
    let mut active: Vec<usize> = Vec::new();
    let mut xs: Vec<(f32, i32)> = Vec::new();
    for py in py_lo..py_hi {
        let wy = min[1] + (py as f32 + 0.5) / rows as f32 * sy;
        while next < order.len() && lo(&layer.segs[order[next]]) <= wy {
            active.push(order[next]);
            next += 1;
        }
        active.retain(|&i| hi(&layer.segs[i]) >= wy);
        xs.clear();
        for &i in &active {
            let s = &layer.segs[i];
            let (ay, by) = (s.a[1], s.b[1]);
            if (ay <= wy) != (by <= wy) {
                let denom = by - ay;
                if denom.abs() < 1e-9 {
                    continue;
                }
                let t = (wy - ay) / denom;
                let x = s.a[0] + (s.b[0] - s.a[0]) * t;
                xs.push((x, if by > ay { 1 } else { -1 }));
            }
        }
        if xs.len() < 2 {
            continue;
        }
        xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let row = py - py_lo;
        // non-zero winding: fill wherever the winding count is anything but 0
        let mut wind = 0i32;
        let mut span: Option<f32> = None;
        for (x, dir) in xs.iter() {
            let was = wind != 0;
            wind += dir;
            let now = wind != 0;
            if !was && now {
                span = Some(*x);
            } else if was && !now {
                if let Some(x0w) = span.take() {
                    fill_span(&mut counts, row, w, min[0], sx, x0w, *x);
                }
            }
        }
        if let Some(x0w) = span {
            fill_span(&mut counts, row, w, min[0], sx, x0w, min[0] + sx);
        }
    }
    let mut out_row = py_lo / sub;
    let mut src = 0usize;
    while src + 2 * w <= counts.len() {
        let dst = out_row * w;
        for px in 0..w {
            let sum = counts[src + px] as u16 + counts[src + w + px] as u16;
            out[dst + px] = (sum * 255 / sub as u16).min(255) as u8;
        }
        src += w * sub;
        out_row += 1;
    }
}

/// The layer as world-space rectangles, one per solid span per scanline, at
/// `pitch` millimetres per line. A rectangle mesh gives the cut plane's cap
/// its exact outlines — the pixel mask it replaces showed its own grid.
/// Vertically identical spans are welded, so a flat wall costs one quad.
pub fn cap_rects(layer: &Layer, min: [f32; 2], max: [f32; 2], pitch: f32) -> Vec<[f32; 4]> {
    cap_walk(layer, min, max, pitch, true)
}

/// The same walk, but a span whose contour never closes is dropped instead
/// of running to the end of the plate: a preview picture should show what
/// the plate really holds, not a rasteriser's fallback.
pub fn cap_rects_strict(layer: &Layer, min: [f32; 2], max: [f32; 2], pitch: f32) -> Vec<[f32; 4]> {
    cap_walk(layer, min, max, pitch, false)
}

fn cap_walk(
    layer: &Layer,
    min: [f32; 2],
    max: [f32; 2],
    pitch: f32,
    close_open: bool,
) -> Vec<[f32; 4]> {
    let mut rects: Vec<[f32; 4]> = Vec::new();
    if layer.segs.is_empty() {
        return rects;
    }
    let pitch = pitch.max(0.01);
    let sx = (max[0] - min[0]).max(1e-6);
    let lo = |s: &Seg| s.a[1].min(s.b[1]);
    let hi = |s: &Seg| s.a[1].max(s.b[1]);
    let mut y_lo = f32::MAX;
    let mut y_hi = f32::MIN;
    for s in &layer.segs {
        y_lo = y_lo.min(lo(s));
        y_hi = y_hi.max(hi(s));
    }
    if y_hi <= y_lo {
        return rects;
    }
    let mut order: Vec<usize> = (0..layer.segs.len()).collect();
    order.sort_by(|&a, &b| lo(&layer.segs[a]).total_cmp(&lo(&layer.segs[b])));
    let mut next = 0usize;
    let mut active: Vec<usize> = Vec::new();
    let mut xs: Vec<(f32, i32)> = Vec::new();
    // Scanlines sample the middle of each band, never its edge: a vertex
    // that sits exactly on an edge would look like a contour that never
    // closes, and the span would run off to the end of the plate.
    let half = pitch * 0.5;
    let mut row = y_lo + half;
    while row < y_hi {
        while next < order.len() && lo(&layer.segs[order[next]]) <= row {
            active.push(order[next]);
            next += 1;
        }
        active.retain(|&i| hi(&layer.segs[i]) >= row);
        xs.clear();
        for &i in &active {
            let s = &layer.segs[i];
            let (ay, by) = (s.a[1], s.b[1]);
            if (ay <= row) != (by <= row) {
                let denom = by - ay;
                if denom.abs() < 1e-9 {
                    continue;
                }
                let t = (row - ay) / denom;
                let x = s.a[0] + (s.b[0] - s.a[0]) * t;
                xs.push((x, if by > ay { 1 } else { -1 }));
            }
        }
        xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut wind = 0i32;
        let mut span: Option<f32> = None;
        let mut k = 0usize;
        while k < xs.len() {
            let was = wind != 0;
            wind += xs[k].1;
            let now = wind != 0;
            if !was && now {
                span = Some(xs[k].0);
            } else if was && !now {
                if let Some(x0) = span.take() {
                    if let Some((x0, x1)) = clamp_span(min, sx, x0, xs[k].0) {
                        push_span(&mut rects, x0, x1, row - half, row + half);
                    }
                }
            }
            k += 1;
        }
        // A contour left open past the bed edge behaves like the mask it
        // replaces: the span runs to the end of the plate. Strict callers
        // would rather lose the span than invent a bar across the plate.
        if close_open {
            if let Some(x0) = span {
                if let Some((x0, x1)) = clamp_span(min, sx, x0, min[0] + sx) {
                    push_span(&mut rects, x0, x1, row - half, row + pitch - half);
                }
            }
        }
        row += pitch;
    }
    rects
}

/// A span clipped to the bed, or nothing if it lies entirely outside.
fn clamp_span(min: [f32; 2], sx: f32, x0w: f32, x1w: f32) -> Option<(f32, f32)> {
    let x0 = x0w.max(min[0]);
    let x1 = x1w.min(min[0] + sx);
    (x1 > x0).then_some((x0, x1))
}

/// One span becomes a rectangle, welded onto the one below it when the
/// footprint has not moved.
fn push_span(rects: &mut Vec<[f32; 4]>, x0: f32, x1: f32, y0: f32, y1: f32) {
    if let Some(last) = rects.last_mut() {
        if (last[3] - y0).abs() < 1e-4 && (last[0] - x0).abs() < 1e-4 && (last[2] - x1).abs() < 1e-4
        {
            last[3] = y1;
            return;
        }
    }
    rects.push([x0, y0, x1, y1]);
}

fn fill_span(counts: &mut [u8], row: usize, w: usize, x_min: f32, sx: f32, x0w: f32, x1w: f32) {
    let x0 = ((x0w - x_min) / sx * w as f32).max(0.0);
    let x1 = ((x1w - x_min) / sx * w as f32).min(w as f32);
    if x1 <= x0 {
        return;
    }
    let a = x0.floor() as usize;
    let b = (x1.ceil() as usize).min(w);
    for px in a..b {
        let idx = row * w + px;
        counts[idx] = counts[idx].saturating_add(1);
    }
}

/// Fill a layer into a grid of cells: true where the layer is solid.
/// Used by island detection, where a full grayscale picture is overkill.
pub fn occupancy(layer: &Layer, min: [f32; 2], max: [f32; 2], w: usize, h: usize) -> Vec<bool> {
    let mut mask = Vec::new();
    fill_mask(layer, min, max, w, h, &mut mask);
    mask.into_iter().map(|v| v >= 128).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Vec3;

    fn cube(cx: f32, cy: f32, cz: f32, size: f32) -> Mesh {
        let h = size * 0.5;
        let mut m = Mesh::default();
        let corners = [
            Vec3::new(cx - h, cy - h, cz - h),
            Vec3::new(cx + h, cy - h, cz - h),
            Vec3::new(cx + h, cy + h, cz - h),
            Vec3::new(cx - h, cy + h, cz - h),
            Vec3::new(cx - h, cy - h, cz + h),
            Vec3::new(cx + h, cy - h, cz + h),
            Vec3::new(cx + h, cy + h, cz + h),
            Vec3::new(cx - h, cy + h, cz + h),
        ];
        for c in corners {
            m.verts.push(c);
        }
        let faces = [
            [0u32, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        m.tris.extend_from_slice(&faces);
        m
    }

    #[test]
    fn a_cube_slices_into_layers_that_cover_its_footprint() {
        let m = cube(0.0, 0.0, 5.0, 10.0);
        let sl = slice(std::slice::from_ref(&m), 0.05);
        assert!((sl.height - 10.0).abs() < 1e-3);
        assert_eq!(sl.layer_count(), 200);
        let mut mask = Vec::new();
        fill_mask(
            &sl.layers[100],
            [-10.0, -10.0],
            [10.0, 10.0],
            40,
            40,
            &mut mask,
        );
        let solid = mask.iter().filter(|v| **v > 200).count();
        // the middle of a 20 x 20 mm window, at 0.5 mm a pixel, is ~400 px
        assert!((380..=420).contains(&solid), "solid pixels: {solid}");
        // the corner of the window is empty
        assert_eq!(mask[0], 0);
    }

    #[test]
    fn overlapping_meshes_stay_solid_where_they_overlap() {
        // Two cubes shifted by 2 mm: with even-odd filling the overlap would
        // come out as a hole; with non-zero it stays solid.
        let a = cube(0.0, 0.0, 5.0, 10.0);
        let b = cube(2.0, 0.0, 5.0, 10.0);
        let sl = slice(&[a, b], 0.05);
        let mut mask = Vec::new();
        fill_mask(
            &sl.layers[100],
            [-10.0, -10.0],
            [10.0, 10.0],
            80,
            80,
            &mut mask,
        );
        // x = 0.5 mm is inside both cubes; it must be solid
        let px = ((0.5 + 10.0) / 20.0 * 80.0) as usize;
        let py = 40;
        assert_eq!(mask[py * 80 + px], 255, "the overlap must not be a hole");
        let lone = ((6.5 + 10.0) / 20.0 * 80.0) as usize;
        assert_eq!(
            mask[py * 80 + lone],
            255,
            "the second cube must be solid too"
        );
    }

    #[test]
    fn a_hole_in_a_mesh_is_still_a_hole() {
        // A small box with a square hole (two cubes' worth of rings): the
        // winding fill must still leave the middle empty.
        let mut m = cube(0.0, 0.0, 5.0, 10.0);
        m.fix_winding();
        let sl = slice(std::slice::from_ref(&m), 0.05);
        assert!(!sl.layers.is_empty());
        let mut mask = Vec::new();
        fill_mask(
            &sl.layers[50],
            [-10.0, -10.0],
            [10.0, 10.0],
            40,
            40,
            &mut mask,
        );
        let solid = mask.iter().filter(|v| **v > 200).count();
        assert!(solid > 300, "the cube should be mostly solid, was {solid}");
    }

    #[test]
    fn a_thin_cone_frustum_slices_to_a_ring_of_the_right_size() {
        // Mirrors what a support tip does: a section of a tapered column.
        let mut m = Mesh::default();
        let sides = 16;
        let (r0, r1, z0, z1) = (0.6f32, 0.3f32, 0.0f32, 2.0f32);
        for i in 0..sides {
            let a = i as f32 / sides as f32 * std::f32::consts::TAU;
            m.verts.push(Vec3::new(r0 * a.cos(), r0 * a.sin(), z0));
            m.verts.push(Vec3::new(r1 * a.cos(), r1 * a.sin(), z1));
        }
        for i in 0..sides {
            let j = (i + 1) % sides;
            let (b0, t0, b1, t1) = (
                i as u32 * 2,
                i as u32 * 2 + 1,
                j as u32 * 2,
                j as u32 * 2 + 1,
            );
            m.tris.push([b0, b1, t1]);
            m.tris.push([b0, t1, t0]);
        }
        // caps
        for i in 1..sides - 1 {
            m.tris.push([0, (i + 1) as u32 * 2, i as u32 * 2]);
            m.tris.push([1, i as u32 * 2 + 1, (i + 1) as u32 * 2 + 1]);
        }
        m.fix_winding();
        let sl = slice(std::slice::from_ref(&m), 0.05);
        let mut mask = Vec::new();
        fill_mask(&sl.layers[20], [-2.0, -2.0], [2.0, 2.0], 80, 80, &mut mask);
        let solid = mask.iter().filter(|v| **v > 200).count();
        // At z = 1.0 the cone's radius is 0.45 mm, so its area is about
        // 0.64 mm². One pixel is 0.05 x 0.05 = 0.0025 mm², so a full disc is
        // roughly 250 pixels and a ring (what a hollow shape would give) would
        // be far fewer.
        assert!((200..=320).contains(&solid), "solid pixels: {solid}");
    }
    #[test]
    fn layer_height_is_quantized_to_three_decimals() {
        let sl = slice(&[cube(0.0, 0.0, 5.0, 10.0)], 0.049999997);
        assert_eq!(sl.layer_height, 0.05);
    }
    fn square_layer(x0: f32, y0: f32, x1: f32, y1: f32, reversed: bool) -> Layer {
        let pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
        let mut segs = Vec::new();
        for k in 0..4 {
            let (a, b) = if reversed {
                (pts[(k + 1) % 4], pts[k])
            } else {
                (pts[k], pts[(k + 1) % 4])
            };
            segs.push(Seg {
                a: [a.0, a.1],
                b: [b.0, b.1],
            });
        }
        Layer { z: 1.0, segs }
    }

    #[test]
    fn cap_rects_weld_a_flat_wall_into_one_rectangle() {
        let layer = square_layer(0.0, 0.0, 4.0, 4.0, false);
        let rects = cap_rects(&layer, [0.0, 0.0], [10.0, 10.0], 1.0);
        assert_eq!(rects.len(), 1, "a flat wall is one quad, was {rects:?}");
        assert_eq!(rects[0], [0.0, 0.0, 4.0, 4.0]);
    }

    #[test]
    fn cap_rects_keep_a_hole_open() {
        let mut layer = square_layer(0.0, 0.0, 6.0, 6.0, false);
        // A hole is the same loop walked the other way.
        layer
            .segs
            .extend(square_layer(2.0, 2.0, 4.0, 4.0, true).segs);
        let rects = cap_rects(&layer, [0.0, 0.0], [10.0, 10.0], 1.0);
        let area: f32 = rects.iter().map(|r| (r[2] - r[0]) * (r[3] - r[1])).sum();
        let expect = 36.0 - 4.0;
        assert!(
            (area - expect).abs() < 1e-4,
            "the hole must not be filled: area {area} over {rects:?}"
        );
        for r in &rects {
            // On the rows through the hole, a rect either stops before it
            // or starts past it; none may bridge it.
            if r[1] > 1.9 && r[3] < 4.1 {
                assert!(
                    r[2] < 2.0 + 1e-4 || r[0] > 4.0 - 1e-4,
                    "hole row was filled: {r:?}"
                );
            }
        }
    }

    #[test]
    fn cap_rects_survive_a_vertex_on_the_scanline() {
        // A diamond whose lowest point sits exactly on a band edge: the
        // crossing at the tip used to leave a span that never closed and
        // ran off to the end of the plate.
        let pts = [(2.0, 0.0), (4.0, 2.0), (2.0, 4.0), (0.0, 2.0)];
        let mut segs = Vec::new();
        for k in 0..4 {
            let (a, b) = (pts[k], pts[(k + 1) % 4]);
            segs.push(Seg {
                a: [a.0, a.1],
                b: [b.0, b.1],
            });
        }
        let layer = Layer { z: 1.0, segs };
        let rects = cap_rects(&layer, [0.0, 0.0], [10.0, 10.0], 1.0);
        let area: f32 = rects.iter().map(|r| (r[2] - r[0]) * (r[3] - r[1])).sum();
        assert!(
            (area - 8.0).abs() < 1e-4,
            "the diamond holds its 8 mm², was {area} over {rects:?}"
        );
        for r in &rects {
            assert!(
                r[2] < 10.0 - 1e-5,
                "a span ran off to the plate edge: {r:?}"
            );
        }
    }

    #[test]
    fn cap_rects_strict_drops_a_span_that_never_closes() {
        // One edge on its own is not a contour: the plain walk paints to the
        // end of the plate, the strict one would rather show nothing.
        let layer = Layer {
            z: 1.0,
            segs: vec![Seg {
                a: [1.0, 0.0],
                b: [3.0, 4.0],
            }],
        };
        let rects = cap_rects(&layer, [0.0, 0.0], [10.0, 10.0], 1.0);
        assert!(
            rects.iter().any(|r| (r[2] - 10.0).abs() < 1e-5),
            "the plain walk mirrors the mask, was {rects:?}"
        );
        let strict = cap_rects_strict(&layer, [0.0, 0.0], [10.0, 10.0], 1.0);
        assert!(strict.is_empty(), "strict invents nothing, was {strict:?}");
    }

    #[test]
    fn cap_rects_clip_to_the_bed() {
        let layer = square_layer(8.0, 0.0, 12.0, 2.0, false);
        let rects = cap_rects(&layer, [0.0, 0.0], [10.0, 10.0], 1.0);
        assert!(!rects.is_empty(), "the part inside the bed still shows");
        for r in &rects {
            assert!(r[1] <= 10.0 + 1e-5, "rect runs past the bed edge: {r:?}");
        }
    }
}
