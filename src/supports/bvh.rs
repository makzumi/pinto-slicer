//! A small bounding-volume tree over the model's triangles, used for
//! "is this point inside the model?" and "does this ray hit the model?".
//!
//! Written in-tree: parry3d is not available offline in this project, and the
//! few queries support generation needs are simple enough to keep here.

use crate::mesh::{Mesh, Vec3};

const LEAF: usize = 8;

#[derive(Clone, Copy)]
struct Node {
    min: Vec3,
    max: Vec3,
    /// Range into `order` for leaves; `left`/`right` are node indices otherwise.
    start: u32,
    count: u32,
    left: u32,
    right: u32,
}

pub struct Bvh {
    tris: Vec<[Vec3; 3]>,
    normals: Vec<Vec3>,
    order: Vec<u32>,
    nodes: Vec<Node>,
}

impl Bvh {
    pub fn from_meshes(meshes: &[Mesh]) -> Self {
        let mut tris = Vec::new();
        let mut normals = Vec::new();
        for m in meshes {
            for t in &m.tris {
                let [a, b, c] = m.tri_verts(*t);
                if !finite(a) || !finite(b) || !finite(c) {
                    continue;
                }
                let n = (b - a).cross(c - a);
                let len = n.length();
                if len < 1e-12 {
                    continue;
                }
                tris.push([a, b, c]);
                normals.push(n * (1.0 / len));
            }
        }
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let mut nodes = Vec::new();
        if !tris.is_empty() {
            let n = tris.len();
            let centroids: Vec<Vec3> = tris
                .iter()
                .map(|t| (t[0] + t[1] + t[2]) * (1.0 / 3.0))
                .collect();
            build(&tris, &centroids, &mut order, &mut nodes, 0, n);
        }
        Self {
            tris,
            normals,
            order,
            nodes,
        }
    }

    pub fn tri_count(&self) -> usize {
        self.tris.len()
    }

    pub fn triangle(&self, i: u32) -> ([Vec3; 3], Vec3) {
        (self.tris[i as usize], self.normals[i as usize])
    }

    /// Distance from `p` to the nearest triangle, if it is within `max_dist`.
    pub fn closest(&self, p: Vec3, max_dist: f32) -> Option<f32> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best = max_dist;
        let mut found = false;
        let mut stack = vec![0u32];
        while let Some(ni) = stack.pop() {
            let node = self.nodes[ni as usize];
            if box_distance(&node, p) >= best {
                continue;
            }
            if node.count > 0 {
                for k in node.start..node.start + node.count {
                    let tri = self.tris[self.order[k as usize] as usize];
                    let d = point_tri_distance(p, tri);
                    if d < best {
                        best = d;
                        found = true;
                    }
                }
            } else {
                stack.push(node.left);
                stack.push(node.right);
            }
        }
        if found {
            Some(best)
        } else {
            None
        }
    }

    /// Nearest hit along the ray: distance from `from`, and the face normal.
    pub fn ray_hit(&self, from: Vec3, dir: Vec3, tmax: f32) -> Option<(f32, Vec3)> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = Vec3::new(
            if dir.x.abs() > 1e-9 {
                1.0 / dir.x
            } else {
                1e30
            },
            if dir.y.abs() > 1e-9 {
                1.0 / dir.y
            } else {
                1e30
            },
            if dir.z.abs() > 1e-9 {
                1.0 / dir.z
            } else {
                1e30
            },
        );
        let mut best = tmax;
        let mut hit: Option<(f32, Vec3)> = None;
        let mut stack = vec![0u32];
        while let Some(ni) = stack.pop() {
            let node = self.nodes[ni as usize];
            if !ray_box(&node, from, inv, best) {
                continue;
            }
            if node.count > 0 {
                for k in node.start..node.start + node.count {
                    let idx = self.order[k as usize];
                    if let Some(t) = ray_tri(from, dir, self.tris[idx as usize]) {
                        if t > 1e-4 && t < best {
                            best = t;
                            hit = Some((t, self.normals[idx as usize]));
                        }
                    }
                }
            } else {
                stack.push(node.left);
                stack.push(node.right);
            }
        }
        hit
    }

    /// Nearest hit distance along the ray.
    pub fn ray(&self, from: Vec3, dir: Vec3, tmax: f32) -> Option<f32> {
        self.ray_hit(from, dir, tmax).map(|(t, _)| t)
    }

    /// How far `dir` travels before it leaves the solid — the local wall
    /// thickness along a direction that points into the model from a point
    /// `pos` on its surface. Starts just inside the surface so the near face
    /// itself is not counted as the exit.
    pub fn wall_thickness(&self, pos: Vec3, dir: Vec3) -> Option<f32> {
        let from = pos + dir * 1e-2;
        self.ray(from, dir, 1.0e4).map(|t| t + 1e-2)
    }

    /// Is this point inside the solid? Three rays in different directions
    /// vote, so a ray slipping exactly along a triangle edge cannot decide.
    pub fn inside(&self, p: Vec3) -> bool {
        let dirs = [
            Vec3::new(1.0, 0.017, 0.011),
            Vec3::new(-0.013, 1.0, 0.021),
            Vec3::new(0.019, -0.011, 1.0),
        ];
        let mut votes = 0;
        for d in dirs {
            let d = d.normalized();
            let mut hits = 0;
            let mut from = p;
            while let Some(t) = self.ray(from, d, 1.0e4) {
                hits += 1;
                from = from + d * (t + 1e-3);
                if hits > 24 {
                    break;
                }
            }
            if hits % 2 == 1 {
                votes += 1;
            }
        }
        votes >= 2
    }

    /// True when a cylinder of `radius` around the segment a..b touches nothing.
    pub fn segment_clear(&self, a: Vec3, b: Vec3, radius: f32) -> bool {
        let len = (b - a).length();
        let steps = ((len / radius.max(0.2)).ceil() as usize).clamp(2, 64);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let p = a + (b - a) * t;
            if let Some(d) = self.closest(p, radius) {
                if d < radius * 0.95 {
                    return false;
                }
            }
            if self.inside(p) {
                return false;
            }
        }
        true
    }

    /// Nearest point and outward normal on the mesh, with an optional maximum
    /// distance. Used when a raster support sample falls between triangles.
    pub fn closest_point(&self, p: Vec3, max_dist: f32) -> Option<(Vec3, Vec3)> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best = max_dist;
        let mut result = None;
        let mut stack = vec![0u32];
        while let Some(ni) = stack.pop() {
            let node = self.nodes[ni as usize];
            if box_distance(&node, p) >= best {
                continue;
            }
            if node.count > 0 {
                for k in node.start..node.start + node.count {
                    let index = self.order[k as usize];
                    let tri = self.tris[index as usize];
                    let distance = point_tri_distance(p, tri);
                    if distance < best {
                        best = distance;
                        result = Some((project_to_triangle(p, tri), self.normals[index as usize]));
                    }
                }
            } else {
                stack.push(node.left);
                stack.push(node.right);
            }
        }
        result
    }

    /// Closest downward-facing surface on a vertical line, restricted to a
    /// real distance from the requesting slice. Unlike `surface_near`, this
    /// never substitutes an upward face or a different overhang far away.
    pub fn downward_surface_near(
        &self,
        x: f32,
        y: f32,
        z: f32,
        max_delta: f32,
    ) -> Option<(Vec3, Vec3)> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut from = Vec3::new(x, y, z - 10_000.0);
        let mut best: Option<(f32, Vec3, Vec3)> = None;
        for _ in 0..128 {
            let Some((distance, normal)) = self.ray_hit(from, Vec3::new(0.0, 0.0, 1.0), 20_000.0)
            else {
                break;
            };
            let at = from + Vec3::new(0.0, 0.0, distance);
            let delta = (at.z - z).abs();
            if normal.z < -0.02
                && delta <= max_delta
                && best
                    .as_ref()
                    .is_none_or(|(best_delta, _, _)| delta < *best_delta)
            {
                best = Some((delta, at, normal));
            }
            from = at + Vec3::new(0.0, 0.0, 1e-3);
        }
        best.map(|(_, point, normal)| (point, normal))
    }

    /// The lowest z on the model surface directly under (x, y), if any.
    pub fn surface_below(&self, x: f32, y: f32, from_z: f32) -> Option<f32> {
        let hit = self.ray(Vec3::new(x, y, from_z), Vec3::new(0.0, 0.0, -1.0), 1.0e4)?;
        Some(from_z - hit)
    }

    /// Highest upward-facing surface below (x,y), for stopping a trunk on the
    /// model when it cannot reach the plate.
    pub fn upward_surface_below(&self, x: f32, y: f32, from_z: f32) -> Option<f32> {
        let mut z = from_z;
        for _ in 0..8 {
            let (t, n) = self.ray_hit(Vec3::new(x, y, z), Vec3::new(0.0, 0.0, -1.0), 1.0e4)?;
            let at = z - t;
            if n.z > 0.7 {
                return Some(at);
            }
            z = at - 0.05;
            if z <= 0.0 {
                return None;
            }
        }
        None
    }

    /// Find the model surface nearest to `(x, y, z)` along the vertical line
    /// through that point. When two hits are at the same height, prefer the
    /// downward-facing one because it is the surface a support can hold.
    ///
    /// Distance is the primary key: a downward face anywhere else in the
    /// column must not pull a contact away from the slice that requested it.
    /// On a tall, tilted model there can be several overhangs above one
    /// another, and preferring downward faces globally used to jump between
    /// them by tens of millimetres.
    ///
    /// Walking a vertical ray through every intersection turns that raster
    /// cell into a real contact point and, importantly, lets us reject holes
    /// instead of putting a support through empty space.
    pub fn surface_near(&self, x: f32, y: f32, z: f32) -> Option<(Vec3, Vec3)> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut from = Vec3::new(x, y, z - 10_000.0);
        let mut best: Option<(f32, Vec3, Vec3, bool)> = None;
        for _ in 0..128 {
            let Some((distance, normal)) = self.ray_hit(from, Vec3::new(0.0, 0.0, 1.0), 20_000.0)
            else {
                break;
            };
            let at = from + Vec3::new(0.0, 0.0, distance);
            let delta = (at.z - z).abs();
            let downward = normal.z < -0.02;
            let replace = match best {
                None => true,
                Some((best_delta, _, _, best_downward)) => {
                    delta < best_delta - 1e-4
                        || ((delta - best_delta).abs() <= 1e-4 && downward && !best_downward)
                }
            };
            if replace {
                best = Some((delta, at, normal, downward));
            }
            from = at + Vec3::new(0.0, 0.0, 1e-3);
        }
        best.map(|(_, point, normal, _)| (point, normal))
    }
}

fn finite(v: Vec3) -> bool {
    v.x.is_finite() && v.y.is_finite() && v.z.is_finite()
}

fn box_distance(node: &Node, p: Vec3) -> f32 {
    let dx = (node.min.x - p.x).max(0.0).max(p.x - node.max.x);
    let dy = (node.min.y - p.y).max(0.0).max(p.y - node.max.y);
    let dz = (node.min.z - p.z).max(0.0).max(p.z - node.max.z);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn ray_box(node: &Node, from: Vec3, inv: Vec3, tmax: f32) -> bool {
    let mut t0 = 0.0f32;
    let mut t1 = tmax;
    for axis in 0..3 {
        let (o, d, lo, hi) = match axis {
            0 => (from.x, inv.x, node.min.x, node.max.x),
            1 => (from.y, inv.y, node.min.y, node.max.y),
            _ => (from.z, inv.z, node.min.z, node.max.z),
        };
        let mut ta = (lo - o) * d;
        let mut tb = (hi - o) * d;
        if ta > tb {
            std::mem::swap(&mut ta, &mut tb);
        }
        t0 = t0.max(ta);
        t1 = t1.min(tb);
        if t0 > t1 {
            return false;
        }
    }
    true
}

fn ray_tri(o: Vec3, d: Vec3, t: [Vec3; 3]) -> Option<f32> {
    let e1 = t[1] - t[0];
    let e2 = t[2] - t[0];
    let pv = d.cross(e2);
    let det = e1.dot(pv);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let tv = o - t[0];
    let u = tv.dot(pv) * inv;
    if !(-1e-4..=1.0001).contains(&u) {
        return None;
    }
    let qv = tv.cross(e1);
    let v = d.dot(qv) * inv;
    if v < -1e-4 || u + v > 1.0001 {
        return None;
    }
    let dist = e2.dot(qv) * inv;
    if dist > 1e-4 {
        Some(dist)
    } else {
        None
    }
}

fn point_tri_distance(p: Vec3, t: [Vec3; 3]) -> f32 {
    let a = t[0];
    let b = t[1];
    let c = t[2];
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return ap.length();
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return bp.length();
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (ap - ab * v).length();
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return cp.length();
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (ap - ac * w).length();
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (p - (b + (c - b) * w)).length();
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (p - (a + ab * v + ac * w)).length()
}

fn project_to_triangle(p: Vec3, t: [Vec3; 3]) -> Vec3 {
    let (a, b, c) = (t[0], t[1], t[2]);
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

fn build(
    tris: &[[Vec3; 3]],
    centroids: &[Vec3],
    order: &mut [u32],
    nodes: &mut Vec<Node>,
    start: usize,
    count: usize,
) -> u32 {
    let mut min = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
    let mut max = Vec3::new(f32::MIN, f32::MIN, f32::MIN);
    for k in start..start + count {
        let t = tris[order[k] as usize];
        for v in t {
            min = v.min_each(min);
            max = v.max_each(max);
        }
    }

    let index = nodes.len() as u32;
    nodes.push(Node {
        min,
        max,
        start: start as u32,
        count: count as u32,
        left: 0,
        right: 0,
    });
    if count <= LEAF {
        return index;
    }
    let extent = max - min;
    let axis = if extent.x >= extent.y && extent.x >= extent.z {
        0
    } else if extent.y >= extent.z {
        1
    } else {
        2
    };
    let mid = start + count / 2;
    order[start..start + count].sort_by(|a, b| {
        let ca = centroids[*a as usize];
        let cb = centroids[*b as usize];
        let (va, vb) = match axis {
            0 => (ca.x, cb.x),
            1 => (ca.y, cb.y),
            _ => (ca.z, cb.z),
        };
        va.partial_cmp(&vb).unwrap_or(std::cmp::Ordering::Equal)
    });
    let left = build(tris, centroids, order, nodes, start, mid - start);
    let right = build(tris, centroids, order, nodes, mid, start + count - mid);
    nodes[index as usize].left = left;
    nodes[index as usize].right = right;
    nodes[index as usize].count = 0;
    index
}
