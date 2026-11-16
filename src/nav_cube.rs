//! A clickable orientation cube drawn as a lightweight egui overlay.
//! Its projection uses the main camera basis and a fixed perspective lens,
//! so zooming the plate never changes the widget's size or field of view.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};

use crate::mesh::Vec3;
use crate::render::View;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Right,
    Left,
    Back,
    Front,
    Top,
    Bottom,
}

impl Face {
    const ALL: [Self; 6] = [
        Self::Right,
        Self::Left,
        Self::Back,
        Self::Front,
        Self::Top,
        Self::Bottom,
    ];

    pub fn direction(self) -> Vec3 {
        match self {
            Self::Right => Vec3::new(1.0, 0.0, 0.0),
            Self::Left => Vec3::new(-1.0, 0.0, 0.0),
            Self::Back => Vec3::new(0.0, 1.0, 0.0),
            Self::Front => Vec3::new(0.0, -1.0, 0.0),
            Self::Top => Vec3::new(0.0, 0.0, 1.0),
            Self::Bottom => Vec3::new(0.0, 0.0, -1.0),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Right => "RIGHT",
            Self::Left => "LEFT",
            Self::Back => "BACK",
            Self::Front => "FRONT",
            Self::Top => "TOP",
            Self::Bottom => "BOT",
        }
    }

    fn corners(self) -> [Vec3; 4] {
        let p = |x, y, z| Vec3::new(x, y, z);
        match self {
            Self::Right => [
                p(1., -1., -1.),
                p(1., 1., -1.),
                p(1., 1., 1.),
                p(1., -1., 1.),
            ],
            Self::Left => [
                p(-1., -1., -1.),
                p(-1., -1., 1.),
                p(-1., 1., 1.),
                p(-1., 1., -1.),
            ],
            Self::Back => [
                p(-1., 1., -1.),
                p(-1., 1., 1.),
                p(1., 1., 1.),
                p(1., 1., -1.),
            ],
            Self::Front => [
                p(-1., -1., -1.),
                p(1., -1., -1.),
                p(1., -1., 1.),
                p(-1., -1., 1.),
            ],
            Self::Top => [
                p(-1., -1., 1.),
                p(1., -1., 1.),
                p(1., 1., 1.),
                p(-1., 1., 1.),
            ],
            Self::Bottom => [
                p(-1., -1., -1.),
                p(-1., 1., -1.),
                p(1., 1., -1.),
                p(1., -1., -1.),
            ],
        }
    }
}

#[derive(Clone)]
struct ProjectedFace {
    face: Face,
    corners: [Pos2; 4],
    depth: f32,
}

pub struct NavCube {
    centre: Pos2,
    faces: Vec<ProjectedFace>,
    hit_radius: f32,
}

impl NavCube {
    pub fn new(view: &View, viewport: Rect) -> Self {
        let centre = viewport.right_top() + Vec2::new(-78.0, 78.0);
        let scale = 30.0;
        let camera_distance = 5.0_f32;
        let eye = -view.fwd;
        let mut faces = Vec::new();
        for face in Face::ALL {
            let normal = face.direction();
            // A perspective camera sees a face only when its plane faces the
            // camera position, not merely when its normal points forward.
            if normal.dot(eye) <= 1.0 / camera_distance {
                continue;
            }
            let project = |p: Vec3| {
                let depth = (camera_distance - p.dot(eye)).max(0.1);
                let magnification = camera_distance / depth;
                centre
                    + Vec2::new(
                        p.dot(view.right) * scale * magnification,
                        -p.dot(view.up) * scale * magnification,
                    )
            };
            let world = face.corners();
            let corners = world.map(project);
            faces.push(ProjectedFace {
                face,
                corners,
                depth: normal.dot(eye),
            });
        }
        faces.sort_by(|a, b| a.depth.total_cmp(&b.depth));
        let hit_radius = faces
            .iter()
            .flat_map(|face| face.corners)
            .fold(66.0_f32, |radius, p| {
                radius.max((p - centre).length() + 4.0)
            });
        Self {
            centre,
            faces,
            hit_radius,
        }
    }

    pub fn hit(&self, pointer: Option<Pos2>) -> Option<Face> {
        let p = pointer?;
        self.faces
            .iter()
            .rev()
            .find(|face| inside_convex(p, &face.corners))
            .map(|face| face.face)
    }

    pub fn contains(&self, pointer: Option<Pos2>) -> bool {
        pointer.is_some_and(|p| (p - self.centre).length() <= self.hit_radius)
    }

    pub fn paint(&self, painter: &egui::Painter, pointer: Option<Pos2>) {
        let hovered = self.hit(pointer);
        // One quiet theme colour on every face. The shared edges are drawn
        // separately below, so a seam is never painted twice at different
        // widths and the three polygons meet without light gaps.
        for face in &self.faces {
            let active = hovered == Some(face.face);
            painter.add(egui::Shape::convex_polygon(
                face.corners.to_vec(),
                if active {
                    Color32::from_rgb(48, 79, 124)
                } else {
                    Color32::from_rgb(42, 46, 54)
                },
                Stroke::NONE,
            ));
        }
        let mut edges: Vec<[Pos2; 2]> = Vec::new();
        for face in &self.faces {
            for i in 0..4 {
                let edge = [face.corners[i], face.corners[(i + 1) % 4]];
                let duplicate = edges.iter().any(|old| {
                    (old[0] == edge[0] && old[1] == edge[1])
                        || (old[0] == edge[1] && old[1] == edge[0])
                });
                if !duplicate {
                    edges.push(edge);
                }
            }
        }
        for edge in edges {
            painter.line_segment(edge, Stroke::new(1.0_f32, Color32::from_rgb(76, 85, 99)));
        }
        // A per-face homography keeps the glyph vertices coplanar as cube
        // edges converge, instead of floating screen-upright over the view.
        for face in &self.faces {
            let active = hovered == Some(face.face);
            paint_coplanar_label(
                painter,
                face,
                if active {
                    Color32::WHITE
                } else {
                    Color32::from_rgb(218, 224, 233)
                },
            );
        }
    }
}

/// Width and height of the label's local plane, in text-layout points. Its
/// four corners map exactly to the four corners of the projected cube face.
const LABEL_PLANE: f32 = 44.0;

/// A square-to-quad homography. Unlike the previous affine map, this keeps
/// labels exactly coplanar when neighbouring cube edges converge in perspective.
struct LabelProjection {
    a: f32,
    b: f32,
    d: f32,
    e: f32,
    g: f32,
    h: f32,
    origin: Pos2,
}

impl LabelProjection {
    fn new(face: &ProjectedFace) -> Option<Self> {
        // Local (0,0), (1,0), (0,1), (1,1) map to corners 3, 2, 0, 1.
        let p0 = face.corners[3];
        let p1 = face.corners[2];
        let p2 = face.corners[0];
        let p3 = face.corners[1];
        let dx1 = p1.x - p3.x;
        let dx2 = p2.x - p3.x;
        let dx3 = p0.x - p1.x - p2.x + p3.x;
        let dy1 = p1.y - p3.y;
        let dy2 = p2.y - p3.y;
        let dy3 = p0.y - p1.y - p2.y + p3.y;
        let det = dx1 * dy2 - dx2 * dy1;
        if det.abs() < 1e-5 {
            return None;
        }
        let g = (dx3 * dy2 - dx2 * dy3) / det;
        let h = (dx1 * dy3 - dx3 * dy1) / det;
        Some(Self {
            a: p1.x * (g + 1.0) - p0.x,
            b: p2.x * (h + 1.0) - p0.x,
            d: p1.y * (g + 1.0) - p0.y,
            e: p2.y * (h + 1.0) - p0.y,
            g,
            h,
            origin: p0,
        })
    }

    fn point(&self, local: Vec2) -> Pos2 {
        let u = local.x / LABEL_PLANE + 0.5;
        let v = local.y / LABEL_PLANE + 0.5;
        let denominator = self.g * u + self.h * v + 1.0;
        Pos2::new(
            (self.a * u + self.b * v + self.origin.x) / denominator,
            (self.d * u + self.e * v + self.origin.y) / denominator,
        )
    }
}

fn orient_label(face: Face, local: Vec2) -> Vec2 {
    if matches!(face, Face::Left | Face::Back) {
        // 90° clockwise in the label plane (whose Y axis points down).
        Vec2::new(-local.y, local.x)
    } else {
        local
    }
}

fn paint_coplanar_label(painter: &egui::Painter, face: &ProjectedFace, color: Color32) {
    let Some(projection) = LabelProjection::new(face) else {
        return;
    };
    let label = face.face.label();
    let galley = painter.layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(if label.len() > 3 { 8.5 } else { 9.5 }),
        color,
    );
    let size = galley.size();
    let text = egui::epaint::TextShape::new(Pos2::new(-size.x * 0.5, -size.y * 0.5), galley, color);
    let font_size = painter.fonts(|fonts| fonts.font_image_size());
    let mut tessellator = egui::epaint::Tessellator::new(
        painter.ctx().pixels_per_point(),
        egui::epaint::TessellationOptions::default(),
        font_size,
        Vec::new(),
    );
    let mut mesh = egui::Mesh::default();
    tessellator.tessellate_text(&text, &mut mesh);
    for vertex in &mut mesh.vertices {
        vertex.pos = projection.point(orient_label(face.face, vertex.pos.to_vec2()));
    }
    painter.add(egui::Shape::mesh(mesh));
}

fn inside_convex(p: Pos2, corners: &[Pos2; 4]) -> bool {
    let mut sign = 0.0_f32;
    for i in 0..4 {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        if cross.abs() < 1e-4 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if sign != cross.signum() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Camera;

    #[test]
    fn the_default_view_shows_three_faces_and_top_is_clickable() {
        let camera = Camera::default();
        let view = View::new(&camera, 900.0, 700.0);
        let cube = NavCube::new(
            &view,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0)),
        );
        assert_eq!(cube.faces.len(), 3);
        let top = cube.faces.iter().find(|f| f.face == Face::Top).unwrap();
        let centre = LabelProjection::new(top).unwrap().point(Vec2::ZERO);
        assert_eq!(cube.hit(Some(centre)), Some(Face::Top));
        assert_eq!(Face::Top.direction(), Vec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn neighbouring_faces_share_exact_edges_and_labels_fit_their_quads() {
        let view = View::new(&Camera::default(), 900.0, 700.0);
        let cube = NavCube::new(
            &view,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0)),
        );
        let top = cube.faces.iter().find(|f| f.face == Face::Top).unwrap();
        let right = cube.faces.iter().find(|f| f.face == Face::Right).unwrap();
        let front = cube.faces.iter().find(|f| f.face == Face::Front).unwrap();
        assert_eq!(top.corners[1], right.corners[3]);
        assert_eq!(top.corners[2], right.corners[2]);
        assert_eq!(front.corners[1], right.corners[0]);
        assert_eq!(front.corners[2], right.corners[3]);
        for face in &cube.faces {
            let projection = LabelProjection::new(face).unwrap();
            let centre = projection.point(Vec2::ZERO);
            assert_eq!(cube.hit(Some(centre)), Some(face.face));
            let h = LABEL_PLANE * 0.5;
            for (local, corner) in [
                (Vec2::new(-h, -h), face.corners[3]),
                (Vec2::new(h, -h), face.corners[2]),
                (Vec2::new(-h, h), face.corners[0]),
                (Vec2::new(h, h), face.corners[1]),
            ] {
                assert!((projection.point(local) - corner).length() < 1e-3);
            }
        }
    }

    #[test]
    fn perspective_makes_opposite_edges_converge() {
        let view = View::new(&Camera::default(), 900.0, 700.0);
        let cube = NavCube::new(
            &view,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0)),
        );
        let top = cube.faces.iter().find(|f| f.face == Face::Top).unwrap();
        let a = top.corners[1] - top.corners[0];
        let b = top.corners[2] - top.corners[3];
        let cross = a.x * b.y - a.y * b.x;
        assert!(
            cross.abs() > 0.1,
            "opposite edges are still parallel: {a:?} and {b:?}"
        );
    }

    #[test]
    fn looking_from_the_other_side_changes_the_visible_faces() {
        let mut camera = Camera::default();
        camera.align_eye_to(Vec3::new(-1.0, 1.0, -1.0));
        let view = View::new(&camera, 900.0, 700.0);
        let cube = NavCube::new(
            &view,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0)),
        );
        assert!(cube.faces.iter().any(|f| f.face == Face::Left));
        assert!(cube.faces.iter().any(|f| f.face == Face::Back));
        assert!(cube.faces.iter().any(|f| f.face == Face::Bottom));
        let p = Vec2::new(3.0, 4.0);
        assert_eq!(orient_label(Face::Left, p), Vec2::new(-4.0, 3.0));
        assert_eq!(orient_label(Face::Back, p), Vec2::new(-4.0, 3.0));
        for face in [Face::Right, Face::Front, Face::Top, Face::Bottom] {
            assert_eq!(orient_label(face, p), p);
        }
    }
}
