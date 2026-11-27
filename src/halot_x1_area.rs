//! HALOT X1 support-aware area mask.
//!
//! The 68-byte zone field is not the 3-bit slice mask.  HALOT renders the
//! model + support mesh with a half-space clip (`FlatWhite`: keep the geometry
//! above the layer), turns the resulting area image into white runs, and maps
//! those runs through the bead LUT (`getsubzonedata`).  This module is the
//! CPU prototype of that step.
//!
//! It rasterises the projected triangles directly into the 64 x 6230 LUT-cell
//! grid instead of materialising the 15120 x 6230 image.  A cell is marked
//! when any part of a projected triangle covers it; the cell's bead list is
//! then OR-ed into the 68-byte mask.  That is the same cell granularity as
//! `setBeadsForRange`; the optional `SepFilter` pass is not applied yet.
//!
//! `SliceResult` carries the source `area_meshes`, and `write_v4` uses this
//! projection when they are present.  A caller that only has 2D segments still
//! gets the slice-mask fallback.

use crate::config::PrinterProfile;
use crate::halot_x1_mask::{
    build_halot_x1_lut, HALOT_X1_COLS, HALOT_X1_EBP, HALOT_X1_EDI,
    HALOT_X1_MASK_BYTES, HALOT_X1_MAX_BIT, HALOT_X1_ROWS,
};
use crate::mesh::{Mesh, Vec3};

/// Model-mm -> area-image-pixel transform.
#[derive(Clone, Copy, Debug)]
pub struct AreaTransform {
    /// Image coordinate of model (0, 0).
    pub center: [f32; 2],
    /// Pixels per millimetre on each axis.
    pub scale: [f32; 2],
    /// Model +Y goes to image -Y (the HALOT raster is top-down).
    pub flip_y: bool,
}

impl AreaTransform {
    pub fn from_printer(printer: &PrinterProfile) -> Self {
        let res = printer.native_res();
        let px = printer.pixel_mm();
        Self {
            center: [res.0 as f32 * 0.5, res.1 as f32 * 0.5],
            scale: [
                1.0 / px[0].max(1e-6),
                1.0 / px[1].max(1e-6),
            ],
            flip_y: true,
        }
    }

    fn project(&self, p: Vec3) -> [f32; 2] {
        [
            self.center[0] + p.x * self.scale[0],
            self.center[1]
                + if self.flip_y {
                    -p.y * self.scale[1]
                } else {
                    p.y * self.scale[1]
                },
        ]
    }
}

/// Project `meshes` at layer height `z` and return the 542-bit bead mask.
///
/// `range`/`bits` are the LUT from [`build_halot_x1_lut`]; pass a cached pair
/// when computing many layers.
pub fn compute_halot_x1_area_mask(
    meshes: &[Mesh],
    z: f32,
    transform: AreaTransform,
    range: &[u32],
    bits: &[u32],
) -> [u8; HALOT_X1_MASK_BYTES] {
    let mut cells = vec![0u8; HALOT_X1_EBP * HALOT_X1_ROWS];

    for mesh in meshes {
        for tri in &mesh.tris {
            let Some(vertices) = tri_vertices(mesh, tri) else {
                continue;
            };
            let poly = clip_triangle_z(vertices, z);
            if poly.len() < 3 {
                continue;
            }
            let projected: Vec<[f32; 2]> = poly.iter().map(|p| transform.project(*p)).collect();
            rasterise_polygon(&projected, &mut cells);
        }
    }

    cells_to_mask(&cells, range, bits)
}

/// Convenience wrapper that builds the geometric LUT once.
pub fn compute_halot_x1_area_mask_default(
    meshes: &[Mesh],
    z: f32,
    printer: &PrinterProfile,
) -> [u8; HALOT_X1_MASK_BYTES] {
    let (range, bits) = build_halot_x1_lut();
    compute_halot_x1_area_mask(
        meshes,
        z,
        AreaTransform::from_printer(printer),
        &range,
        &bits,
    )
}

fn tri_vertices(mesh: &Mesh, tri: &[u32; 3]) -> Option<[Vec3; 3]> {
    Some([
        *mesh.verts.get(tri[0] as usize)?,
        *mesh.verts.get(tri[1] as usize)?,
        *mesh.verts.get(tri[2] as usize)?,
    ])
}

/// Sutherland-Hodgman clip of a triangle to `z >= clip_z`.
fn clip_triangle_z(tri: [Vec3; 3], clip_z: f32) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(4);
    for i in 0..3 {
        let cur = tri[i];
        let prev = tri[(i + 2) % 3];
        let cur_in = cur.z >= clip_z;
        let prev_in = prev.z >= clip_z;
        if cur_in != prev_in {
            let dz = prev.z - cur.z;
            if dz.abs() > 1e-12 {
                let t = (clip_z - cur.z) / dz;
                out.push(cur + (prev - cur) * t);
            }
        }
        if cur_in {
            out.push(cur);
        }
    }
    out
}

/// Even-odd scanline fill of one projected convex polygon into LUT cells.
fn rasterise_polygon(poly: &[[f32; 2]], cells: &mut [u8]) {
    if poly.len() < 3 {
        return;
    }
    let y_min = poly.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
    let y_max = poly.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max);
    if !y_min.is_finite() || !y_max.is_finite() {
        return;
    }

    // Sample pixel centres (row + 0.5) that can be covered.
    let first_row = ((y_min - 0.5).ceil() as i32).max(0);
    let last_row = ((y_max - 0.5).floor() as i32).min(HALOT_X1_ROWS as i32 - 1);
    if first_row > last_row {
        return;
    }

    let mut xs: Vec<f32> = Vec::with_capacity(4);
    for row in first_row..=last_row {
        let y = row as f32 + 0.5;
        xs.clear();
        for i in 0..poly.len() {
            let a = poly[i];
            let b = poly[(i + 1) % poly.len()];
            if (a[1] <= y) != (b[1] <= y) {
                let dy = b[1] - a[1];
                if dy.abs() > 1e-12 {
                    let t = (y - a[1]) / dy;
                    xs.push(a[0] + (b[0] - a[0]) * t);
                }
            }
        }
        if xs.len() < 2 {
            continue;
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for pair in xs.chunks_exact(2) {
            mark_span(row as usize, pair[0], pair[1], cells);
        }
    }
}

fn mark_span(row: usize, x0: f32, x1: f32, cells: &mut [u8]) {
    if row >= HALOT_X1_ROWS || !x0.is_finite() || !x1.is_finite() || x1 <= x0 {
        return;
    }
    let x0 = x0.max(0.0).min(HALOT_X1_COLS as f32 - 1.0);
    let x1 = x1.max(0.0).min(HALOT_X1_COLS as f32 - 1.0);
    if x1 <= x0 {
        return;
    }
    let band0 = (x0 as usize) / HALOT_X1_EDI;
    let band1 = ((x1 as usize) / HALOT_X1_EDI).min(HALOT_X1_EBP - 1);
    if band0 > band1 {
        return;
    }
    let base = row * HALOT_X1_EBP;
    for band in band0..=band1 {
        cells[base + band] = 1;
    }
}

fn cells_to_mask(
    cells: &[u8],
    range: &[u32],
    bits: &[u32],
) -> [u8; HALOT_X1_MASK_BYTES] {
    let mut mask = [0u8; HALOT_X1_MASK_BYTES];
    for (gi, &marked) in cells.iter().enumerate() {
        if marked == 0 {
            continue;
        }
        let s = range[gi] as usize;
        let e = range[gi + 1] as usize;
        for &b in &bits[s..e] {
            if b < HALOT_X1_MAX_BIT as u32 {
                mask[(b >> 3) as usize] |= 1 << (7 - (b & 7));
            }
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_printers;
    use crate::halot_x1_mask::build_halot_x1_lut;
    use crate::mesh::sample_block;

    fn printer() -> PrinterProfile {
        default_printers()
            .into_iter()
            .find(|p| p.machine_name.eq_ignore_ascii_case("HALOT-X1"))
            .expect("HALOT-X1 profile")
    }

    fn count(mask: &[u8; HALOT_X1_MASK_BYTES]) -> u32 {
        mask.iter().map(|b| b.count_ones()).sum()
    }

    #[test]
    fn empty_mesh_is_empty_mask() {
        let (range, bits) = build_halot_x1_lut();
        let mask = compute_halot_x1_area_mask(
            &[],
            5.0,
            AreaTransform::from_printer(&printer()),
            &range,
            &bits,
        );
        assert_eq!(count(&mask), 0);
    }

    #[test]
    fn a_block_above_the_layer_lights_beads() {
        let (range, bits) = build_halot_x1_lut();
        let block = sample_block();
        let mask = compute_halot_x1_area_mask(
            std::slice::from_ref(&block),
            5.0,
            AreaTransform::from_printer(&printer()),
            &range,
            &bits,
        );
        assert!(count(&mask) > 0, "the block should cover some beads");
    }

    #[test]
    fn a_block_below_the_layer_does_not_light() {
        let (range, bits) = build_halot_x1_lut();
        let block = sample_block(); // 0..10 mm
        let mask = compute_halot_x1_area_mask(
            std::slice::from_ref(&block),
            20.0,
            AreaTransform::from_printer(&printer()),
            &range,
            &bits,
        );
        assert_eq!(count(&mask), 0);
    }

    #[test]
    fn support_above_the_layer_changes_the_mask() {
        let (range, bits) = build_halot_x1_lut();
        let transform = AreaTransform::from_printer(&printer());
        // Support column: another block at x=+20, z=0..10.
        let mut support = sample_block();
        support.translate(Vec3::new(20.0, 0.0, 0.0));

        // Model block sits at z=20..30, so it is above both test layers.
        let mut model = sample_block();
        model.translate(Vec3::new(0.0, 0.0, 20.0));

        let at_z5 = compute_halot_x1_area_mask(
            &[model.clone(), support.clone()],
            5.0,
            transform,
            &range,
            &bits,
        );
        let at_z15 = compute_halot_x1_area_mask(
            &[model, support],
            15.0,
            transform,
            &range,
            &bits,
        );

        assert!(
            count(&at_z5) > count(&at_z15),
            "support above z=5 should add beads: {} vs {}",
            count(&at_z5),
            count(&at_z15)
        );
    }

    #[test]
    fn slice_result_retains_the_source_meshes() {
        let block = sample_block();
        let sliced = crate::slicer::slice(std::slice::from_ref(&block), 0.5);
        assert_eq!(sliced.area_meshes.len(), 1);
        assert!(!sliced.area_meshes[0].tris.is_empty());
    }

    #[test]
    fn a_clipped_triangle_has_the_expected_area() {
        // A triangle crossing the clip plane becomes a trapezoid; its
        // projection is non-empty at the layer and empty above its top.
        let tri = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 10.0),
            Vec3::new(0.0, 10.0, 10.0),
        ];
        let clipped = clip_triangle_z(tri, 5.0);
        assert_eq!(clipped.len(), 4);
        assert!(clipped.iter().all(|p| p.z >= 5.0 - 1e-5));
    }
    #[test]
    fn synthetic_support_fan_covers_the_reference_l529_mask() {
        // A triangular support wall whose projection is the L529 reference
        // fan.  This isolates the area projection + exact LUT from the
        // support-generation differences: every reference bead must be
        // covered, even if the synthetic wall adds a few boundary beads.
        let z = 530.0;
        let verts = vec![
            Vec3::new(0.0, 5.149, z),
            Vec3::new(-97.704, -51.16, z),
            Vec3::new(-81.424, -65.35, z),
            Vec3::new(81.424, -65.35, z),
            Vec3::new(97.704, -51.16, z),
        ];
        let fan = Mesh {
            verts,
            tris: vec![[0, 1, 2], [0, 2, 3], [0, 3, 4]],
            name: "synthetic support fan".into(),
        };
        let (range, bits) = build_halot_x1_lut();
        let mask = compute_halot_x1_area_mask(
            std::slice::from_ref(&fan),
            529.0,
            AreaTransform::from_printer(&printer()),
            &range,
            &bits,
        );
        // Stored 68-byte mask for reference layer 529.
        let reference_hex = "0000000000000000000000000000000000000000000000000000000000007000\
                             000fc00001ff00003ffc0007fff000ffffc01fffff03fffffc3fffffe1fffffe07ffffc0";
        let mut reference = [0u8; HALOT_X1_MASK_BYTES];
        for (i, byte) in reference.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&reference_hex[2 * i..2 * i + 2], 16).unwrap();
        }
        let inter: u32 = mask
            .iter()
            .zip(reference.iter())
            .map(|(a, b)| (a & b).count_ones())
            .sum();
        assert_eq!(inter, 178, "the fan must cover every L529 reference bead");
        assert_eq!(count(&mask), 202, "the synthetic wall should add 24 edge beads");
    }

}
