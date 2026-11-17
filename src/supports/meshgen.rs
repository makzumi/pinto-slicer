//! Turning each element into a closed, consistently wound mesh.
//!
//! Nothing is merged here: the pieces are just appended, and the slicer's
//! non-zero fill joins them where they overlap.

use super::{ContactShape, Foot, Plate, Tip, TipShape};
use crate::mesh::{Mesh, Vec3};

pub fn append(dst: &mut Mesh, src: &Mesh) {
    let base = dst.verts.len() as u32;
    dst.verts.extend_from_slice(&src.verts);
    for t in &src.tris {
        dst.tris.push([t[0] + base, t[1] + base, t[2] + base]);
    }
}

/// A closed cone frustum from `a` (radius `ra`) to `b` (radius `rb`).
pub fn frustum(a: Vec3, ra: f32, b: Vec3, rb: f32, sides: usize) -> Mesh {
    let sides = sides.clamp(3, 32);
    let mut m = Mesh::default();
    let axis = b - a;
    let len = axis.length();
    if len < 1e-6 {
        return m;
    }
    let axis = axis * (1.0 / len);
    let helper = if axis.z.abs() < 0.9 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::new(1.0, 0.0, 0.0)
    };
    let u = axis.cross(helper).normalized();
    let v = axis.cross(u).normalized();
    let ra = ra.max(0.02);
    let rb = rb.max(0.02);

    for i in 0..sides {
        let t = i as f32 / sides as f32 * std::f32::consts::TAU;
        let (s, c) = (t.sin(), t.cos());
        m.verts.push(a + u * (ra * c) + v * (ra * s));
        m.verts.push(b + u * (rb * c) + v * (rb * s));
    }
    for i in 0..sides {
        let j = (i + 1) % sides;
        let (a_i, b_i) = (i as u32 * 2, i as u32 * 2 + 1);
        let (a_j, b_j) = (j as u32 * 2, j as u32 * 2 + 1);
        m.tris.push([a_i, a_j, b_i]);
        m.tris.push([a_j, b_j, b_i]);
    }
    // caps, wound outwards at both ends
    let ca = m.verts.len() as u32;
    m.verts.push(a);
    let cb = m.verts.len() as u32;
    m.verts.push(b);
    for i in 0..sides {
        let j = (i + 1) % sides;
        let (a_i, a_j) = (i as u32 * 2, j as u32 * 2);
        let (b_i, b_j) = (i as u32 * 2 + 1, j as u32 * 2 + 1);
        m.tris.push([ca, a_j, a_i]);
        m.tris.push([cb, b_i, b_j]);
    }
    m
}

pub fn ball(centre: Vec3, radius: f32, sides: usize) -> Mesh {
    let sides = sides.clamp(6, 32);
    let rings = (sides / 2).max(3);
    let mut m = Mesh::default();
    let r = radius.max(0.05);
    // a single vertex at each pole, so the mesh has no degenerate edges there
    m.verts.push(centre + Vec3::new(0.0, 0.0, r));
    for ring in 1..rings {
        let phi = std::f32::consts::PI * ring as f32 / rings as f32;
        let (s, z) = (phi.sin(), phi.cos());
        for i in 0..sides {
            let t = i as f32 / sides as f32 * std::f32::consts::TAU;
            m.verts
                .push(centre + Vec3::new(s * t.cos() * r, s * t.sin() * r, z * r));
        }
    }
    let south = m.verts.len() as u32;
    m.verts.push(centre - Vec3::new(0.0, 0.0, r));
    let at = |ring: usize, i: usize| -> u32 { 1 + ((ring - 1) * sides + (i % sides)) as u32 };
    for i in 0..sides {
        m.tris.push([0, at(1, i), at(1, i + 1)]);
    }
    for ring in 1..rings - 1 {
        for i in 0..sides {
            m.tris
                .push([at(ring, i), at(ring, i + 1), at(ring + 1, i + 1)]);
            m.tris
                .push([at(ring, i), at(ring + 1, i + 1), at(ring + 1, i)]);
        }
    }
    for i in 0..sides {
        m.tris.push([at(rings - 1, i), at(rings - 1, i + 1), south]);
    }
    m.fix_winding();
    m
}

/// A tip: a small cone pressed `contact_depth` into the surface, narrowing
/// towards the model, with an optional little ball at the very end.
pub fn tip(t: &Tip, sides: usize) -> Mesh {
    let mut m = Mesh::default();
    let top = t.upper_end();
    let low = t.lower_end();
    let contact_r = (t.contact_d * 0.5).max(0.05);
    let upper_r = (t.upper_d * 0.5).max(0.02);
    let lower_r = (t.lower_d * 0.5).max(upper_r);
    let body_r = match t.shape {
        TipShape::Cone => upper_r,
        TipShape::Cylinder => lower_r,
    };
    append(&mut m, &frustum(low, lower_r, t.pos, body_r, sides));
    // the part inside the material
    let end_r = match t.contact_shape {
        ContactShape::None => body_r,
        ContactShape::Ball => contact_r,
    };
    append(&mut m, &frustum(t.pos, body_r, top, end_r, sides));
    // The junction is the centre of the back sphere, not just the end of the
    // cone. Unlike two flat caps, the sphere overlaps a post or branch leaving
    // in any direction, including when the head is angled.
    append(&mut m, &ball(low, lower_r, sides.max(8)));
    if t.contact_shape == ContactShape::Ball && t.contact_d > 0.01 {
        append(&mut m, &ball(top, contact_r, sides.max(8)));
    }
    m
}

/// A foot: a short cone standing on the platform, under its trunk.
pub fn foot(f: &Foot) -> Mesh {
    let h = 0.8;
    let r = (f.diameter * 0.5).max(0.3);
    let z = f.pos.z;
    frustum(
        Vec3::new(f.pos.x, f.pos.y, z),
        r,
        Vec3::new(f.pos.x, f.pos.y, z + h),
        r * 0.75,
        16,
    )
}

/// A plate under the feet: straight sides and the configured chamfer along
/// the top edge.
pub fn plate(p: &Plate) -> Mesh {
    let mut m = Mesh::default();
    let n = p.poly.len();
    if n < 3 {
        return m;
    }
    let t = p.thickness.max(0.1);
    let c = p.chamfer.clamp(0.0, t * 0.7);
    let inner = inset(&p.poly, p.chamfer_run.max(0.0));
    let has_inner = inner.len() == n;
    let rings = if has_inner { 3 } else { 2 };
    let zs = if has_inner {
        [0.0, t - c, t]
    } else {
        [0.0, t, t]
    };
    for (ring, z) in zs.iter().enumerate().take(rings) {
        let poly = if ring == 2 { &inner } else { &p.poly };
        for &(x, y) in poly.iter().take(n) {
            m.verts.push(Vec3::new(x, y, *z));
        }
    }
    // walls
    for ring in 0..rings - 1 {
        for k in 0..n {
            let j = (k + 1) % n;
            let a = (ring * n + k) as u32;
            let b = (ring * n + j) as u32;
            let c2 = ((ring + 1) * n + j) as u32;
            let d = ((ring + 1) * n + k) as u32;
            m.tris.push([a, b, c2]);
            m.tris.push([a, c2, d]);
        }
    }
    // bottom cap (facing down) and top cap (facing up). Both fan centres must
    // lie inside their own polygon; using world origin for the bottom cap
    // creates huge triangles when the model is moved away from (0, 0).
    let cb = m.verts.len() as u32;
    let (bx, by) = centroid(&p.poly);
    m.verts.push(Vec3::new(bx, by, 0.0));
    let top_ring = (rings - 1) * n;
    let ct = m.verts.len() as u32;
    let (cx, cy) = centroid(if has_inner { &inner } else { &p.poly });
    m.verts.push(Vec3::new(cx, cy, t));
    for k in 0..n {
        let j = (k + 1) % n;
        m.tris.push([cb, j as u32, k as u32]);
        m.tris
            .push([ct, (top_ring + k) as u32, (top_ring + j) as u32]);
    }
    m.fix_winding();
    m
}

fn centroid(poly: &[(f32, f32)]) -> (f32, f32) {
    let mut x = 0.0;
    let mut y = 0.0;
    for (a, b) in poly {
        x += a;
        y += b;
    }
    let n = poly.len().max(1) as f32;
    (x / n, y / n)
}

/// Shrink (or, with a negative distance, grow) a convex polygon by `d`.
pub fn inset(poly: &[(f32, f32)], d: f32) -> Vec<(f32, f32)> {
    offset(poly, -d)
}

/// Move every edge of a convex polygon outwards by `d` and re-intersect.
pub fn offset(poly: &[(f32, f32)], d: f32) -> Vec<(f32, f32)> {
    let n = poly.len();
    if n < 3 {
        return poly.to_vec();
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let p0 = poly[(i + n - 1) % n];
        let p1 = poly[i];
        let p2 = poly[(i + 1) % n];
        let d1 = (p1.0 - p0.0, p1.1 - p0.1);
        let d2 = (p2.0 - p1.0, p2.1 - p1.1);
        let l1 = (d1.0 * d1.0 + d1.1 * d1.1).sqrt().max(1e-9);
        let l2 = (d2.0 * d2.0 + d2.1 * d2.1).sqrt().max(1e-9);
        // outward normal of a counter-clockwise polygon
        let o1 = (d1.1 / l1, -d1.0 / l1);
        let o2 = (d2.1 / l2, -d2.0 / l2);
        let a1 = (p0.0 + o1.0 * d, p0.1 + o1.1 * d);
        let b1 = (p1.0 + o1.0 * d, p1.1 + o1.1 * d);
        let a2 = (p1.0 + o2.0 * d, p1.1 + o2.1 * d);
        let b2 = (p2.0 + o2.0 * d, p2.1 + o2.1 * d);
        match line_intersect(a1, b1, a2, b2) {
            Some(p) => out.push(p),
            None => out.push(b1),
        }
    }
    out
}

fn line_intersect(
    a1: (f32, f32),
    b1: (f32, f32),
    a2: (f32, f32),
    b2: (f32, f32),
) -> Option<(f32, f32)> {
    let d1 = (b1.0 - a1.0, b1.1 - a1.1);
    let d2 = (b2.0 - a2.0, b2.1 - a2.1);
    let den = d1.0 * d2.1 - d1.1 * d2.0;
    if den.abs() < 1e-9 {
        return None;
    }
    let t = ((a2.0 - a1.0) * d2.1 - (a2.1 - a1.1) * d2.0) / den;
    Some((a1.0 + d1.0 * t, a1.1 + d1.1 * t))
}

/// Drop the points of a closed outline that lie closer than `tolerance` to the
/// line their neighbours already draw, so a shape is described by its corners
/// rather than by every point sampled along it.
///
/// A hull built from a detailed outline may have a vertex for every small
/// bump. Douglas and Peucker again, but on a closed ring: the outline is cut
/// at its leftmost point and at its furthest point from there, and each half
/// is reduced against its own end points. Both ends survive, including the
/// closing side back to the first vertex.
pub fn simplify(points: &[(f32, f32)], tolerance: f32) -> Vec<(f32, f32)> {
    let n = points.len();
    if n < 4 || tolerance <= 0.0 {
        return points.to_vec();
    }
    let left = (0..n)
        .min_by(|a, b| points[*a].0.total_cmp(&points[*b].0))
        .unwrap_or(0);
    let right = (0..n)
        .max_by(|a, b| {
            let da = distance(points[*a], points[left]);
            let db = distance(points[*b], points[left]);
            da.total_cmp(&db)
        })
        .unwrap_or(0);

    let ring: Vec<(f32, f32)> = (0..n).map(|i| points[(left + i) % n]).collect();
    let split = (right + n - left) % n;
    let mut out = reduce(&ring[..=split], tolerance);
    let back_points: Vec<(f32, f32)> = ring[split..]
        .iter()
        .copied()
        .chain(std::iter::once(ring[0]))
        .collect();
    let mut back = reduce(&back_points, tolerance);
    // each half ends where the other begins
    out.pop();
    back.pop();
    out.extend(back);
    if out.len() < 3 {
        return points.to_vec();
    }
    out
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// How far a point lies from the line through `a` and `b`.
fn off_line(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-9 {
        return distance(p, a);
    }
    ((p.0 - a.0) * dy - (p.1 - a.1) * dx).abs() / len
}

fn reduce(points: &[(f32, f32)], tolerance: f32) -> Vec<(f32, f32)> {
    let n = points.len();
    if n < 3 {
        return points.to_vec();
    }
    let (a, b) = (points[0], points[n - 1]);
    let mut worst = 0.0f32;
    let mut at = 0usize;
    for (i, p) in points.iter().enumerate().take(n - 1).skip(1) {
        let d = off_line(*p, a, b);
        if d > worst {
            worst = d;
            at = i;
        }
    }
    if worst <= tolerance {
        return vec![a, b];
    }
    let mut out = reduce(&points[..=at], tolerance);
    out.pop();
    out.extend(reduce(&points[at..], tolerance));
    out
}

/// Convex hull of 2D points, counter-clockwise.
pub fn hull(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut p: Vec<(f32, f32)> = points.to_vec();
    p.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let cross = |o: (f32, f32), a: (f32, f32), b: (f32, f32)| -> f32 {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let mut lower: Vec<(f32, f32)> = Vec::new();
    for &pt in &p {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], pt) <= 0.0 {
            lower.pop();
        }
        lower.push(pt);
    }
    let mut upper: Vec<(f32, f32)> = Vec::new();
    for &pt in p.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], pt) <= 0.0 {
            upper.pop();
        }
        upper.push(pt);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}
