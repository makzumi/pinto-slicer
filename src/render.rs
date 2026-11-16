//! Camera and scene definitions shared by the GPU viewport, CPU picking and
//! the egui gizmos. `View` is deliberately the only projection source used by
//! all three, so they cannot drift apart.

use crate::mesh::{Mesh, Transform, Vec3};
use crate::slicer::SliceResult;
use eframe::egui;

pub type Rgb = [u8; 3];

/// Anything closer than this to the eye is clipped away.
pub const NEAR: f32 = 0.5;

/// The camera as the window keeps it. It compares by value so a picture can
/// record the exact view it was drawn from.
#[derive(Clone, Copy, PartialEq)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub target: Vec3,
    pub fov_deg: f32,
}

impl Camera {
    /// Look at the current target from a world-space direction. Used by the
    /// navigation cube's six axis-aligned views; distance and FOV are kept.
    pub fn align_eye_to(&mut self, direction: Vec3) {
        if direction.length() < 1e-5 {
            return;
        }
        let d = direction.normalized();
        self.yaw = d.y.atan2(d.x);
        self.pitch = d.z.clamp(-1.0, 1.0).asin();
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw: -0.62,
            pitch: 0.66,
            dist: 290.0,
            target: Vec3::new(0.0, 0.0, 14.0),
            fov_deg: 40.0,
        }
    }
}

pub struct View {
    pub eye: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub fwd: Vec3,
    pub focal: f32,
    pub w: f32,
    pub h: f32,
}

impl View {
    pub fn new(cam: &Camera, w: f32, h: f32) -> Self {
        let (cp, sp) = cam.pitch.cos_sin();
        let (cy, sy) = cam.yaw.cos_sin();
        let dir = Vec3::new(cp * cy, cp * sy, sp);
        let eye = cam.target + dir * cam.dist;
        let fwd = (cam.target - eye).normalized();
        let world_up = Vec3::new(0.0, 0.0, 1.0);
        let mut right = fwd.cross(world_up);
        // Exact top/bottom views have no horizontal cross-product. Preserve
        // the camera's yaw as a stable screen-up reference instead of
        // normalising zero and producing NaNs throughout the viewport.
        if right.length() < 1e-4 {
            right = Vec3::new(cy, sy, 0.0);
        }
        let right = right.normalized();
        let up = right.cross(fwd).normalized();
        let focal = (h * 0.5) / (cam.fov_deg.to_radians() * 0.5).tan();
        Self {
            eye,
            right,
            up,
            fwd,
            focal,
            w,
            h,
        }
    }

    /// Screen position and camera distance of a world point.
    pub fn project(&self, p: Vec3) -> Option<(f32, f32, f32)> {
        let c = self.to_cam(p);
        if c.z <= NEAR {
            return None;
        }
        Some(self.project_cam(c))
    }

    /// Where a piece sits on screen, as a box around its projected corners.
    /// `None` when the piece is entirely behind the camera.
    pub fn screen_bounds(&self, mesh: &Mesh, t: &Transform) -> Option<egui::Rect> {
        let placed = mesh.transformed(t);
        let (min, max) = placed.bounds();
        let mut rect: Option<egui::Rect> = None;
        for corner in 0..8 {
            let p = Vec3::new(
                if corner & 1 == 0 { min.x } else { max.x },
                if corner & 2 == 0 { min.y } else { max.y },
                if corner & 4 == 0 { min.z } else { max.z },
            );
            let Some((x, y, _)) = self.project(p) else {
                continue;
            };
            let r = egui::Rect::from_min_max(egui::pos2(x, y), egui::pos2(x, y));
            rect = Some(match rect {
                Some(acc) => acc.union(r),
                None => r,
            });
        }
        rect
    }

    /// How many pixels one millimetre spans at the depth of `p`. Screen-space
    /// handles are sized with this so they keep a constant size on screen.
    pub fn pixels_per_mm(&self, p: Vec3) -> f32 {
        self.focal / self.to_cam(p).z.max(NEAR)
    }

    /// The ray through a screen point: from, and a unit direction.
    pub fn ray(&self, sx: f32, sy: f32) -> (Vec3, Vec3) {
        let x = (sx - self.w * 0.5) / self.focal;
        let y = -(sy - self.h * 0.5) / self.focal;
        let dir = (self.fwd + self.right * x + self.up * y).normalized();
        (self.eye, dir)
    }

    /// World point -> camera space (right, up, forward).
    pub fn to_cam(&self, p: Vec3) -> Vec3 {
        let d = p - self.eye;
        Vec3::new(d.dot(self.right), d.dot(self.up), d.dot(self.fwd))
    }

    /// Camera-space point -> screen.
    pub fn project_cam(&self, c: Vec3) -> (f32, f32, f32) {
        (
            self.w * 0.5 + c.x / c.z * self.focal,
            self.h * 0.5 - c.y / c.z * self.focal,
            c.z,
        )
    }
}

trait CosSin {
    fn cos_sin(self) -> (f32, f32);
}

impl CosSin for f32 {
    fn cos_sin(self) -> (f32, f32) {
        let (s, c) = self.sin_cos();
        (c, s)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Solid,
    Sliced,
}

/// The colour every piece is drawn in, and the cool one for picked pieces.
pub const PIECE: Rgb = [214, 202, 176];
pub const PIECE_PICKED: Rgb = [130, 176, 244];

pub struct Scene<'a> {
    pub models: &'a [Mesh],
    /// The pieces the person picked, drawn lighter so they read as current.
    pub selection: &'a [usize],
    /// The drawn shape of the supports (cones, branches, trunks and plate).
    pub support_mesh: Option<&'a Mesh>,
    /// Overhanging faces, drawn red when the debug switch is on.
    pub overhang: Option<&'a Mesh>,
    /// Island markers.
    pub islands: &'a [Vec3],
    pub build: [f32; 3],
    pub mode: ViewMode,
    pub slice: Option<&'a SliceResult>,
    pub current_layer: usize,
    pub show_plate: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_behind_the_camera_is_not_drawn() {
        let cam = Camera::default();
        let view = View::new(&cam, 400.0, 300.0);
        assert!(view.project(cam.target).is_some());
        let behind = view.eye + view.fwd * -50.0;
        assert!(view.project(behind).is_none());
    }

    #[test]
    fn exact_axis_views_have_a_finite_stable_basis() {
        let mut cam = Camera::default();
        cam.align_eye_to(Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(cam.pitch, std::f32::consts::FRAC_PI_2);
        let top = View::new(&cam, 400.0, 300.0);
        assert!(top.right.length() > 0.99 && top.up.length() > 0.99);
        assert!(top.project(cam.target).unwrap().0.is_finite());
        cam.align_eye_to(Vec3::new(0.0, 0.0, -1.0));
        let bottom = View::new(&cam, 400.0, 300.0);
        assert!(bottom.right.length() > 0.99 && bottom.up.length() > 0.99);
        assert!(bottom.project(cam.target).unwrap().1.is_finite());
    }
}
