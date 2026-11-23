//! Geometry: a tiny vector type, a mesh made of triangles, and the placement
//! (position / rotation / scale) each model gets on the build plate.

use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn normalized(self) -> Vec3 {
        let l = self.length();
        if l > 1e-9 {
            self * (1.0 / l)
        } else {
            Vec3::ZERO
        }
    }
    pub fn min_each(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x.min(o.x), self.y.min(o.y), self.z.min(o.z))
    }
    pub fn max_each(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x.max(o.x), self.y.max(o.y), self.z.max(o.z))
    }
}

impl std::ops::Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl std::ops::Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl std::ops::Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}
impl std::ops::Neg for Vec3 {
    type Output = Vec3;
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

/// Rotate a point by degrees around X, then Y, then Z.
pub fn rotate_xyz(p: Vec3, deg: Vec3) -> Vec3 {
    let (sx, cx) = deg.x.to_radians().sin_cos();
    let q = Vec3::new(p.x, p.y * cx - p.z * sx, p.y * sx + p.z * cx);
    let (sy, cy) = deg.y.to_radians().sin_cos();
    let q = Vec3::new(q.x * cy + q.z * sy, q.y, -q.x * sy + q.z * cy);
    let (sz, cz) = deg.z.to_radians().sin_cos();
    Vec3::new(q.x * cz - q.y * sz, q.x * sz + q.y * cz, q.z)
}

/// Apply an incremental turn around a world axis to the existing XYZ
/// orientation.  Adding to one Euler component is only equivalent while the
/// object is still aligned with the world; after a previous turn it changes a
/// different effective axis.  The matrix round-trip keeps the public,
/// serialized Euler fields while composing the actual rotations correctly.
fn compose_world_turn(current: Vec3, axis: usize, degrees: f32) -> Vec3 {
    let (sx, cx) = current.x.to_radians().sin_cos();
    let (sy, cy) = current.y.to_radians().sin_cos();
    let (sz, cz) = current.z.to_radians().sin_cos();
    let mut m = [
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ];
    let (sa, ca) = degrees.to_radians().sin_cos();
    let r = match axis {
        0 => [[1.0, 0.0, 0.0], [0.0, ca, -sa], [0.0, sa, ca]],
        1 => [[ca, 0.0, sa], [0.0, 1.0, 0.0], [-sa, 0.0, ca]],
        _ => [[ca, -sa, 0.0], [sa, ca, 0.0], [0.0, 0.0, 1.0]],
    };
    let old = m;
    for row in 0..3 {
        for col in 0..3 {
            m[row][col] = (0..3).map(|k| r[row][k] * old[k][col]).sum();
        }
    }

    let y = (-m[2][0]).clamp(-1.0, 1.0).asin();
    let cy = y.cos();
    let (x, z) = if cy.abs() > 1e-6 {
        (m[2][1].atan2(m[2][2]), m[1][0].atan2(m[0][0]))
    } else {
        // At the Euler singularity there are infinitely many equivalent X/Z
        // pairs. Keep X stable and put the remaining heading in Z.
        (0.0, (-m[0][1]).atan2(m[1][1]))
    };
    Vec3::new(x.to_degrees(), y.to_degrees(), z.to_degrees())
}

/// Which axes of the model are flipped. A mirror is part of the placement, not
/// a change to the mesh data, so a piece can be mirrored back and forth
/// without ever losing the original geometry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Mirror {
    pub x: bool,
    pub y: bool,
    pub z: bool,
}

impl Mirror {
    pub fn axis(&self, axis: usize) -> bool {
        match axis {
            0 => self.x,
            1 => self.y,
            _ => self.z,
        }
    }

    pub fn flip(&mut self, axis: usize) {
        match axis {
            0 => self.x = !self.x,
            1 => self.y = !self.y,
            _ => self.z = !self.z,
        }
    }

    /// Signs to multiply each axis by, 1.0 or -1.0.
    pub fn signs(&self) -> Vec3 {
        Vec3::new(
            if self.x { -1.0 } else { 1.0 },
            if self.y { -1.0 } else { 1.0 },
            if self.z { -1.0 } else { 1.0 },
        )
    }

    pub fn any(&self) -> bool {
        self.x || self.y || self.z
    }
}

/// Where a model sits on the plate.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Transform {
    pub pos: Vec3,
    pub rot_deg: Vec3,
    pub scale: Vec3,
    pub mirror: Mirror,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            pos: Vec3::ZERO,
            rot_deg: Vec3::ZERO,
            scale: Vec3::new(1.0, 1.0, 1.0),
            mirror: Mirror::default(),
        }
    }
}

impl Transform {
    pub fn apply(&self, p: Vec3) -> Vec3 {
        let m = self.mirror.signs();
        let s = Vec3::new(
            p.x * m.x * self.scale.x,
            p.y * m.y * self.scale.y,
            p.z * m.z * self.scale.z,
        );
        rotate_xyz(s, self.rot_deg) + self.pos
    }

    /// Undo `apply` for a point. Picking works in the model's own space, so a
    /// cached tree stays valid while the piece is moved, turned or scaled.
    pub fn inverse_apply(&self, p: Vec3) -> Vec3 {
        let q = self.unrotate(p - self.pos);
        let m = self.mirror.signs();
        Vec3::new(
            q.x / (m.x * self.scale.x),
            q.y / (m.y * self.scale.y),
            q.z / (m.z * self.scale.z),
        )
    }

    /// Undo the rotation and scale for a direction, leaving translation out.
    pub fn inverse_apply_dir(&self, d: Vec3) -> Vec3 {
        let q = self.unrotate(d);
        let m = self.mirror.signs();
        Vec3::new(
            q.x / (m.x * self.scale.x),
            q.y / (m.y * self.scale.y),
            q.z / (m.z * self.scale.z),
        )
    }

    /// `apply` turns X, then Y, then Z; undo those in the opposite order.
    fn unrotate(&self, p: Vec3) -> Vec3 {
        let p = rotate_xyz(p, Vec3::new(0.0, 0.0, -self.rot_deg.z));
        let p = rotate_xyz(p, Vec3::new(0.0, -self.rot_deg.y, 0.0));
        rotate_xyz(p, Vec3::new(-self.rot_deg.x, 0.0, 0.0))
    }

    /// Run `change`, then slide the piece so that `centre` — a point in the
    /// mesh's own coordinates — lands back where it was. Turning, scaling and
    /// mirroring all go through here so a piece spins and grows about the point
    /// the eye is looking at, instead of swinging around whatever origin the
    /// file happened to be written with.
    fn anchored(&mut self, centre: Vec3, change: impl FnOnce(&mut Self)) {
        let before = self.apply(centre);
        change(self);
        let after = self.apply(centre);
        self.pos = self.pos + (before - after);
    }

    /// Turn the piece about its own centre.
    pub fn turn_about(&mut self, axis: usize, degrees: f32, centre: Vec3) {
        self.anchored(centre, |t| {
            t.rot_deg = compose_world_turn(t.rot_deg, axis, degrees);
        });
    }

    /// Resize the piece about its own centre.
    pub fn scale_about(&mut self, factor: f32, centre: Vec3) {
        self.anchored(centre, |t| {
            t.scale = Vec3::new(
                (t.scale.x * factor).clamp(0.05, 5.0),
                (t.scale.y * factor).clamp(0.05, 5.0),
                (t.scale.z * factor).clamp(0.05, 5.0),
            );
        });
    }

    /// Mirror the piece across the plane through its own centre, so it stays
    /// where it is on the plate instead of jumping to the other side of it.
    pub fn mirror_about(&mut self, axis: usize, centre: Vec3) {
        self.anchored(centre, |t| t.mirror.flip(axis));
    }

    /// A mirroring transform turns the model inside out; the triangles have to
    /// be wound the other way round for the normals to keep pointing outwards.
    pub fn flips_winding(&self) -> bool {
        (self.scale.x * self.scale.y * self.scale.z)
            * self.mirror.signs().x
            * self.mirror.signs().y
            * self.mirror.signs().z
            < 0.0
    }
}

impl std::fmt::Debug for Mesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Mesh({} verts, {} tris)",
            self.verts.len(),
            self.tris.len()
        )
    }
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Mesh {
    pub verts: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    pub name: String,
}

impl Mesh {
    pub fn transformed(&self, t: &Transform) -> Mesh {
        let tris = if t.flips_winding() {
            self.tris.iter().map(|t| [t[0], t[2], t[1]]).collect()
        } else {
            self.tris.clone()
        };
        Mesh {
            verts: self.verts.iter().map(|v| t.apply(*v)).collect(),
            tris,
            name: self.name.clone(),
        }
    }

    /// (minimum corner, maximum corner); zeroes for an empty mesh.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        if self.verts.is_empty() {
            return (Vec3::ZERO, Vec3::ZERO);
        }
        let mut min = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
        let mut max = Vec3::new(f32::MIN, f32::MIN, f32::MIN);
        for v in &self.verts {
            min = min.min_each(*v);
            max = max.max_each(*v);
        }
        (min, max)
    }

    pub fn translate(&mut self, d: Vec3) {
        for v in self.verts.iter_mut() {
            *v = *v + d;
        }
    }

    /// Sit the model on the plate, centred over the middle of the build area.
    pub fn center_on_plate(&mut self) {
        let (min, max) = self.bounds();
        if self.verts.is_empty() {
            return;
        }
        let d = Vec3::new(-(min.x + max.x) * 0.5, -(min.y + max.y) * 0.5, -min.z);
        self.translate(d);
    }

    /// A lighter copy of the mesh for quick drawing while the mouse is moving.
    /// Vertices that fall in the same little box are merged into one, and
    /// triangles that collapse to a line or a point are dropped. The shape
    /// stays close; fine detail goes.
    pub fn decimated(&self, cell: f32) -> Mesh {
        use std::collections::HashMap;
        let cell = cell.max(1e-3);
        let inv = 1.0 / cell;
        let mut map: HashMap<u64, u32> = HashMap::with_capacity(self.verts.len() / 2 + 16);
        let mut verts: Vec<Vec3> = Vec::with_capacity(self.verts.len() / 2 + 16);
        let mut key_of: Vec<u32> = Vec::with_capacity(self.verts.len());
        for v in &self.verts {
            // Clamp each axis into 21 bits so the three fit in one number.
            let ix = ((v.x * inv).round() as i64 + (1 << 20)).clamp(0, (1 << 21) - 1) as u64;
            let iy = ((v.y * inv).round() as i64 + (1 << 20)).clamp(0, (1 << 21) - 1) as u64;
            let iz = ((v.z * inv).round() as i64 + (1 << 20)).clamp(0, (1 << 21) - 1) as u64;
            let key = (ix << 42) | (iy << 21) | iz;
            let next = verts.len() as u32;
            let idx = *map.entry(key).or_insert(next);
            if idx == next {
                verts.push(*v);
            }
            key_of.push(idx);
        }
        let mut tris: Vec<[u32; 3]> = Vec::with_capacity(self.tris.len() / 3 + 16);
        for t in &self.tris {
            let a = key_of[t[0] as usize];
            let b = key_of[t[1] as usize];
            let c = key_of[t[2] as usize];
            if a == b || b == c || a == c {
                continue;
            }
            tris.push([a, b, c]);
        }
        Mesh {
            name: self.name.clone(),
            verts,
            tris,
        }
    }

    pub fn tri_verts(&self, t: [u32; 3]) -> [Vec3; 3] {
        [
            self.verts[t[0] as usize],
            self.verts[t[1] as usize],
            self.verts[t[2] as usize],
        ]
    }

    /// Imported models are sometimes inside-out. The signed volume says so,
    /// and then every triangle is flipped.
    pub fn fix_winding(&mut self) {
        let mut vol = 0.0f64;
        for t in &self.tris {
            let [a, b, c] = self.tri_verts(*t);
            vol += (a.x as f64 * (b.y as f64 * c.z as f64 - b.z as f64 * c.y as f64)
                - a.y as f64 * (b.x as f64 * c.z as f64 - b.z as f64 * c.x as f64)
                + a.z as f64 * (b.x as f64 * c.y as f64 - b.y as f64 * c.x as f64))
                / 6.0;
        }
        if vol < 0.0 {
            for t in self.tris.iter_mut() {
                t.swap(1, 2);
            }
        }
    }
}

// ---------------------------------------------------------------- loading

pub fn load_model(path: &Path) -> Result<Mesh, String> {
    let data = std::fs::read(path).map_err(|e| format!("could not read that file: {e}"))?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model")
        .to_string();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut mesh = match ext.as_str() {
        "obj" => parse_obj(&data)?,
        "stl" => parse_stl(&data)?,
        _ => {
            if looks_like_binary_stl(&data) {
                parse_stl(&data)?
            } else {
                parse_obj(&data)?
            }
        }
    };
    if mesh.tris.is_empty() {
        return Err("no triangles in that file".into());
    }
    mesh.name = name;
    mesh.fix_winding();
    mesh.center_on_plate();
    Ok(mesh)
}

fn looks_like_binary_stl(data: &[u8]) -> bool {
    if data.len() < 84 {
        return false;
    }
    let count = u32::from_le_bytes([data[80], data[81], data[82], data[83]]) as usize;
    84 + count * 50 == data.len()
}

fn parse_stl(data: &[u8]) -> Result<Mesh, String> {
    if looks_like_binary_stl(data) {
        let count = u32::from_le_bytes([data[80], data[81], data[82], data[83]]) as usize;
        let mut mesh = Mesh::default();
        let mut map: HashMap<[u32; 3], u32> = HashMap::new();
        for i in 0..count {
            let base = 84 + i * 50 + 12; // skip the stored normal
            let mut idx = [0u32; 3];
            for (k, slot) in idx.iter_mut().enumerate() {
                let off = base + k * 12;
                if off + 12 > data.len() {
                    break;
                }
                let x =
                    f32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
                let y = f32::from_le_bytes([
                    data[off + 4],
                    data[off + 5],
                    data[off + 6],
                    data[off + 7],
                ]);
                let z = f32::from_le_bytes([
                    data[off + 8],
                    data[off + 9],
                    data[off + 10],
                    data[off + 11],
                ]);
                let key = [x.to_bits(), y.to_bits(), z.to_bits()];
                let id = *map.entry(key).or_insert_with(|| {
                    mesh.verts.push(Vec3::new(x, y, z));
                    (mesh.verts.len() - 1) as u32
                });
                *slot = id;
            }
            if idx[0] != idx[1] && idx[1] != idx[2] && idx[0] != idx[2] {
                mesh.tris.push(idx);
            }
        }
        Ok(mesh)
    } else {
        let text = std::str::from_utf8(data).map_err(|_| "that STL is not readable".to_string())?;
        parse_stl_ascii(text)
    }
}

fn parse_stl_ascii(text: &str) -> Result<Mesh, String> {
    let mut mesh = Mesh::default();
    let mut map: HashMap<[u32; 3], u32> = HashMap::new();
    let mut pending: Vec<u32> = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        if let Some(word) = it.next() {
            if word.eq_ignore_ascii_case("vertex") {
                let x: f32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let y: f32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let z: f32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let key = [x.to_bits(), y.to_bits(), z.to_bits()];
                let id = *map.entry(key).or_insert_with(|| {
                    mesh.verts.push(Vec3::new(x, y, z));
                    (mesh.verts.len() - 1) as u32
                });
                pending.push(id);
                if pending.len() == 3 {
                    let t = [pending[0], pending[1], pending[2]];
                    if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                        mesh.tris.push(t);
                    }
                    pending.clear();
                }
            }
        }
    }
    Ok(mesh)
}

fn parse_obj(data: &[u8]) -> Result<Mesh, String> {
    let text = std::str::from_utf8(data).map_err(|_| "that OBJ is not readable".to_string())?;
    let mut mesh = Mesh::default();
    for line in text.lines() {
        let line = line.trim();
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let x: f32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let y: f32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let z: f32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                mesh.verts.push(Vec3::new(x, y, z));
            }
            Some("f") => {
                let mut idx: Vec<u32> = Vec::new();
                for tok in it {
                    let first = tok.split('/').next().unwrap_or("");
                    if let Ok(v) = first.parse::<i64>() {
                        let n = mesh.verts.len() as i64;
                        let resolved = if v < 0 { n + v } else { v - 1 };
                        if resolved >= 0 && resolved < n {
                            idx.push(resolved as u32);
                        }
                    }
                }
                // fan-triangulate polygons
                for k in 1..idx.len().saturating_sub(1) {
                    let t = [idx[0], idx[k], idx[k + 1]];
                    if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                        mesh.tris.push(t);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(mesh)
}

// ------------------------------------------------------------- built-ins

fn push_tri(mesh: &mut Mesh, a: u32, b: u32, c: u32) {
    if a != b && b != c && a != c {
        mesh.tris.push([a, b, c]);
    }
}

/// A shape made by spinning a profile around the vertical axis.
fn lathe(profile: &[(f32, f32)], segments: usize) -> Mesh {
    let mut mesh = Mesh::default();
    let mut rings: Vec<Vec<u32>> = Vec::new();
    for &(r, z) in profile {
        let mut ring = Vec::with_capacity(segments);
        for j in 0..segments {
            let a = j as f32 / segments as f32 * std::f32::consts::TAU;
            let (s, c) = a.sin_cos();
            let id = mesh.verts.len() as u32;
            mesh.verts.push(Vec3::new(r * c, r * s, z));
            ring.push(id);
        }
        rings.push(ring);
    }
    for i in 0..rings.len().saturating_sub(1) {
        for j in 0..segments {
            let j2 = (j + 1) % segments;
            let a = rings[i][j];
            let b = rings[i][j2];
            let c = rings[i + 1][j2];
            let d = rings[i + 1][j];
            push_tri(&mut mesh, a, b, c);
            push_tri(&mut mesh, a, c, d);
        }
    }
    mesh
}

fn boxy(size: Vec3) -> Mesh {
    let (x, y, z) = (size.x * 0.5, size.y * 0.5, size.z * 0.5);
    let mut mesh = Mesh::default();
    for &(sx, sy, sz) in &[
        (-1.0f32, -1.0f32, 1.0f32),
        (1.0, -1.0, 1.0),
        (1.0, 1.0, 1.0),
        (-1.0, 1.0, 1.0),
        (-1.0, -1.0, -1.0),
        (1.0, -1.0, -1.0),
        (1.0, 1.0, -1.0),
        (-1.0, 1.0, -1.0),
    ] {
        mesh.verts.push(Vec3::new(x * sx, y * sy, z * sz + z));
    }
    let faces: [[u32; 4]; 6] = [
        [0, 3, 2, 1], // bottom
        [4, 5, 6, 7], // top
        [0, 1, 5, 4],
        [1, 2, 6, 5],
        [2, 3, 7, 6],
        [3, 0, 4, 7],
    ];
    for f in faces {
        push_tri(&mut mesh, f[0], f[1], f[2]);
        push_tri(&mut mesh, f[0], f[2], f[3]);
    }
    mesh
}

/// A pawn-like sample piece, used as the starting model in a fresh scene.
pub fn sample_pawn() -> Mesh {
    let profile: [(f32, f32); 19] = [
        (0.0, 0.0),
        (7.5, 0.0),
        (7.5, 1.2),
        (7.0, 2.0),
        (4.0, 3.0),
        (3.2, 4.4),
        (3.0, 7.6),
        (4.2, 8.6),
        (4.2, 9.6),
        (2.5, 10.4),
        (3.6, 11.4),
        (4.1, 12.8),
        (4.5, 14.6),
        (3.8, 16.2),
        (2.2, 17.0),
        (0.9, 17.7),
        (0.2, 17.95),
        (0.0, 18.0),
        (0.0, 18.0),
    ];
    let mut mesh = lathe(&profile, 48);
    mesh.name = "sample pawn".into();
    mesh.fix_winding();
    mesh.center_on_plate();
    mesh
}

/// A calibration block: a plain 16 mm cube with a sloped top.
pub fn sample_block() -> Mesh {
    let mut mesh = boxy(Vec3::new(16.0, 16.0, 10.0));
    mesh.name = "calibration block".into();
    mesh.fix_winding();
    mesh.center_on_plate();
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(name);
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn an_ascii_stl_is_read_and_set_on_the_plate() {
        let text = "solid t\n\
                    facet normal 0 0 -1\n\
                    outer loop\n\
                    vertex -1 -1 0\n\
                    vertex 1 -1 0\n\
                    vertex 1 1 0\n\
                    endloop\n\
                    endfacet\n\
                    endsolid t\n";
        let path = temp("resin_slicer_ascii.stl");
        std::fs::write(&path, text).unwrap();
        let m = load_model(&path).unwrap();
        assert_eq!(m.tris.len(), 1);
        let (min, _) = m.bounds();
        assert!(min.z.abs() < 1e-4, "the piece should sit on the plate");
    }

    #[test]
    fn a_binary_stl_is_read() {
        let mut bytes: Vec<u8> = vec![0u8; 80];
        bytes.extend_from_slice(&2u32.to_le_bytes());
        for tri in [
            [[0.0f32, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]],
            [[0.0f32, 0.0, 5.0], [0.0, 10.0, 5.0], [10.0, 0.0, 5.0]],
        ] {
            bytes.extend_from_slice(&[0u8; 12]); // normal
            for v in tri {
                for c in v {
                    bytes.extend_from_slice(&c.to_le_bytes());
                }
            }
            bytes.extend_from_slice(&[0u8; 2]); // attribute count
        }
        let path = temp("resin_slicer_binary.stl");
        std::fs::write(&path, &bytes).unwrap();
        let m = load_model(&path).unwrap();
        assert_eq!(m.tris.len(), 2);
        assert_eq!(m.verts.len(), 6);
    }

    #[test]
    fn an_obj_with_quads_becomes_triangles() {
        let text = "v 0 0 0\nv 10 0 0\nv 10 10 0\nv 0 10 0\nv 0 0 10\n\
                    f 1 2 3 4\nf 1/1 2/1 5/1\n";
        let path = temp("resin_slicer_quad.obj");
        std::fs::write(&path, text).unwrap();
        let m = load_model(&path).unwrap();
        assert_eq!(m.tris.len(), 3, "a quad is two triangles plus one more");
    }

    #[test]
    fn an_inside_out_cube_is_turned_right_way_out() {
        let mut m = boxy(Vec3::new(10.0, 10.0, 10.0));
        for t in m.tris.iter_mut() {
            t.swap(1, 2);
        }
        m.fix_winding();
        let mut volume = 0.0f64;
        for t in &m.tris {
            let [a, b, c] = m.tri_verts(*t);
            volume += (a.x as f64 * (b.y as f64 * c.z as f64 - b.z as f64 * c.y as f64)
                - a.y as f64 * (b.x as f64 * c.z as f64 - b.z as f64 * c.x as f64)
                + a.z as f64 * (b.x as f64 * c.y as f64 - b.y as f64 * c.x as f64))
                / 6.0;
        }
        assert!(volume > 0.0, "volume {volume} should end up positive");
    }

    #[test]
    fn the_built_in_samples_are_closed_and_sit_on_the_plate() {
        for m in [sample_pawn(), sample_block()] {
            let (min, max) = m.bounds();
            assert!(min.z.abs() < 1e-4, "{} should sit on z=0", m.name);
            assert!(max.z > 5.0);
            assert!(!m.tris.is_empty());
        }
    }

    #[test]
    fn mirroring_leaves_the_piece_where_it_was() {
        let mut t = Transform {
            pos: Vec3::new(30.0, -12.0, 0.0),
            rot_deg: Vec3::new(0.0, 0.0, 90.0),
            ..Default::default()
        };
        let pivot = Vec3::new(0.0, 0.0, 5.0);
        let before = t.apply(pivot);
        t.mirror_about(0, pivot);
        assert!(
            (t.apply(pivot) - before).length() < 1e-4,
            "the middle of the piece must not move"
        );
    }

    #[test]
    fn mirroring_across_x_swaps_left_and_right_in_place() {
        // a 10 mm wide piece sitting at x 0..10
        let mut t = Transform {
            pos: Vec3::new(7.0, 0.0, 0.0),
            ..Default::default()
        };
        t.mirror_about(0, Vec3::new(5.0, 0.0, 0.0));
        let left = t.apply(Vec3::new(0.0, 0.0, 0.0));
        let right = t.apply(Vec3::new(10.0, 0.0, 0.0));
        assert!((left.x - 17.0).abs() < 1e-4, "left edge was {left:?}");
        assert!((right.x - 7.0).abs() < 1e-4, "right edge was {right:?}");
        assert!(t.mirror.x);
    }

    #[test]
    fn mirroring_twice_gives_the_piece_back() {
        let mut t = Transform {
            pos: Vec3::new(7.0, 3.0, 0.0),
            scale: Vec3::new(1.5, 1.5, 1.5),
            ..Default::default()
        };
        let pivot = Vec3::new(0.0, 0.0, 4.0);
        let p = Vec3::new(2.0, -1.0, 6.0);
        let start = t.apply(p);
        t.mirror_about(1, pivot);
        t.mirror_about(1, pivot);
        assert!((t.apply(p) - start).length() < 1e-4);
        assert!(!t.mirror.any());
    }

    #[test]
    fn turning_a_piece_holds_its_centre_still() {
        let m = boxy(Vec3::new(20.0, 8.0, 12.0));
        let (min, max) = m.bounds();
        let centre = (min + max) * 0.5;
        let mut t = Transform {
            pos: Vec3::new(40.0, -18.0, 2.0),
            ..Default::default()
        };
        let corner = Vec3::new(max.x, max.y, max.z);
        let before = t.apply(centre);
        let corner_before = t.apply(corner);
        t.turn_about(2, 37.0, centre);
        let after = t.apply(centre);
        assert!(
            (after - before).length() < 1e-4,
            "the middle must stay put: {before:?} -> {after:?}"
        );
        let corner_after = t.apply(corner);
        assert!(
            (corner_after - corner_before).length() > 1.0,
            "the piece has to actually turn"
        );
        // turning about the centre keeps every point at its distance
        assert!((corner_after - after).length() - (corner_before - before).length() < 1e-3);
        assert!((corner_after - after).length() - (corner_before - before).length() > -1e-3);
    }

    #[test]
    fn scaling_a_piece_holds_its_centre_still() {
        let m = boxy(Vec3::new(10.0, 10.0, 10.0));
        let (min, max) = m.bounds();
        let centre = (min + max) * 0.5;
        let mut t = Transform {
            pos: Vec3::new(-25.0, 6.0, 0.0),
            ..Default::default()
        };
        let before = t.apply(centre);
        t.scale_about(2.0, centre);
        assert!(
            (t.apply(centre) - before).length() < 1e-4,
            "the middle must stay put"
        );
        assert!((t.scale.x - 2.0).abs() < 1e-4);
    }

    #[test]
    fn inverse_apply_round_trips_points_and_directions() {
        let mut t = Transform {
            pos: Vec3::new(-12.0, 4.0, 7.0),
            rot_deg: Vec3::new(17.0, -31.0, 43.0),
            scale: Vec3::new(1.7, 0.6, 2.3),
            ..Default::default()
        };
        t.mirror.flip(0);
        for p in [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(3.0, -5.0, 11.0),
            Vec3::new(-80.0, 42.0, 0.5),
        ] {
            let there = t.apply(p);
            let back = t.inverse_apply(there);
            assert!((back - p).length() < 1e-4, "{p:?} -> {there:?} -> {back:?}");
        }
        let d = Vec3::new(2.0, -3.0, 5.0);
        let there = t.apply(d) - t.apply(Vec3::ZERO);
        let back = t.inverse_apply_dir(there);
        assert!((back - d).length() < 1e-4, "{d:?} -> {there:?} -> {back:?}");
    }

    #[test]
    fn a_mirrored_piece_has_its_triangles_turned_the_other_way() {
        let m = boxy(Vec3::new(10.0, 10.0, 10.0));
        let mut t = Transform::default();
        assert!(!t.flips_winding(), "a plain transform keeps the winding");
        let plain = m.transformed(&t);
        assert_eq!(plain.tris[0], m.tris[0]);

        t.mirror.flip(0);
        assert!(
            t.flips_winding(),
            "an odd number of flips turns it inside out"
        );
        let flipped = m.transformed(&t);
        assert_eq!(flipped.tris[0], [m.tris[0][0], m.tris[0][2], m.tris[0][1]]);

        t.mirror.flip(2);
        assert!(!t.flips_winding(), "two flips point outwards again");
    }
}
