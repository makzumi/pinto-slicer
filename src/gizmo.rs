//! The on-screen handles for the picked pieces.
//!
//! A handle is worked out once per frame as a screen shape, and the drawing and
//! the hit test both read that same shape, so what is drawn is exactly what can
//! be grabbed. The module owns handle geometry, hit testing, drag amounts and
//! drawing; it knows nothing about which pieces are picked or how the camera
//! got where it is.
//!
//! Every handle is laid out from one world-space arm length and then projected,
//! so the whole set foreshortens together when the view is turned, exactly like
//! the model does. Nothing here rescales a single axis to a fixed number of
//! pixels: doing that makes the arrows keep their length while the view swings
//! round them, which reads as the gizmo changing shape on its own.

use eframe::egui::{Color32, Pos2, Stroke, Vec2};

use crate::mesh::Vec3;
use crate::render::View;

/// How far the pointer may be from a handle and still grab it, in pixels.
const GRAB_RADIUS: f32 = 11.0;
/// Screen length of one arm, when that arm lies across the view, in pixels.
const STEM_PIXELS: f32 = 92.0;
/// Every turn ring has the same radius, as a share of an arm, so the three
/// circles read as one set rather than three different sizes.
const RING_RATIO: f32 = 0.82;
/// Points used to draw and hit-test a ring.
const RING_STEPS: usize = 64;
/// Side of a world-space dual-axis move quad before projection, in pixels.
const PLANE_PIXELS: f32 = 20.0;
/// Where the square handles sit along the diagonal, as a share of an arm.
const SQUARE_AT: f32 = 0.52;
/// Under this many pixels across, an axis points at the camera and there is
/// nothing on screen to grab.
const MIN_PIXELS: f32 = 6.0;
/// How far apart, as the sine of the angle between them, two arms have to be on
/// screen before a square between them is worth drawing.
const PLANE_SPREAD: f32 = 0.45;
/// Degrees of turn per degree of pointer travel around a ring.
const TURN_GAIN: f32 = 1.0;

const WIDTH: f32 = 3.2;
const WIDTH_ACTIVE: f32 = 4.4;
const RING_WIDTH: f32 = 3.5;
const RING_WIDTH_ACTIVE: f32 = 4.8;
const OUTLINE: Color32 = Color32::from_rgba_premultiplied(8, 11, 16, 220);

const AXIS_COL: [Color32; 3] = [
    Color32::from_rgb(232, 96, 96),
    Color32::from_rgb(112, 208, 120),
    Color32::from_rgb(96, 156, 240),
];

/// What dragging the handles does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Move,
    Turn,
    Size,
}

impl Mode {
    pub fn of_tool(tool: crate::ui::Tool) -> Option<Self> {
        match tool {
            crate::ui::Tool::Move => Some(Self::Move),
            crate::ui::Tool::Turn => Some(Self::Turn),
            crate::ui::Tool::Size => Some(Self::Size),
            _ => None,
        }
    }
}

/// Which handle of a gizmo is meant.
///
/// An axis handle drives one direction: along it when moving, around it when
/// turning, out from it when scaling. A plane handle only exists for moving,
/// and slides in the two directions it sits between.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    Axis(usize),
    Plane([usize; 2]),
}

impl Handle {
    /// Squares are checked first so that a click near one is not stolen by the
    /// stems that pass close by.
    fn priority(self) -> u8 {
        match self {
            Handle::Plane(_) => 0,
            Handle::Axis(_) => 1,
        }
    }

    /// The axis a turn or a resize turns or resizes about.
    pub fn axis(self) -> usize {
        match self {
            Handle::Axis(axis) => axis,
            Handle::Plane(axes) => axes[0],
        }
    }
}

/// What a handle looks like on screen, and therefore how it is grabbed.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// A straight stem out from the centre: dragged along, or measured against
    /// the centre for scaling.
    Stem(Pos2, Pos2),
    /// A world-space plane quad, projected corner by corner. Unlike the old
    /// screen-aligned square, this foreshortens with its two axes.
    Plane { corners: [Pos2; 4], centre: Pos2 },
    /// A ring around the centre, sampled in screen space.
    Ring(Vec<Pos2>),
}

impl Shape {
    /// Distance from a point to the shape as drawn. Zero means on it.
    fn distance(&self, p: Pos2) -> f32 {
        match self {
            Shape::Stem(a, b) => distance_to_segment(p, *a, *b),
            Shape::Plane { corners, .. } => {
                if point_in_convex(p, corners) {
                    0.0
                } else {
                    (0..4)
                        .map(|i| distance_to_segment(p, corners[i], corners[(i + 1) % 4]))
                        .fold(f32::MAX, f32::min)
                }
            }
            Shape::Ring(pts) => pts
                .windows(2)
                .map(|w| distance_to_segment(p, w[0], w[1]))
                .fold(f32::MAX, f32::min),
        }
    }
}

/// One grabbable handle.
#[derive(Clone, Debug)]
pub struct HandleShape {
    pub handle: Handle,
    pub shape: Shape,
}

impl HandleShape {
    pub fn distance(&self, p: Pos2) -> f32 {
        self.shape.distance(p)
    }
}

/// One short piece of a projected turn ring. Sorting these pieces together,
/// rather than painting one whole axis at a time, gives crossings a front and
/// a back without changing the shapes used for picking.
#[derive(Clone, Copy, Debug)]
struct RingSegment {
    axis: usize,
    a: Pos2,
    b: Pos2,
    depth: f32,
}

/// What a drag asks of a piece.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Change {
    /// A move in world space, already in millimetres.
    Move(Vec3),
    /// A turn about the piece's own centre, in degrees.
    Turn(f32),
    /// A resize about the piece's own centre, as a factor.
    Size(f32),
}

/// The handles of the picked pieces, in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gizmo {
    pub mode: Mode,
    /// The point the handles start from.
    pub origin: Vec3,
}

impl Gizmo {
    pub fn new(mode: Mode, origin: Vec3) -> Self {
        Self { mode, origin }
    }

    /// The three axis directions, always world axes.
    pub fn direction(axis: usize) -> Vec3 {
        match axis {
            0 => Vec3::new(1.0, 0.0, 0.0),
            1 => Vec3::new(0.0, 1.0, 0.0),
            _ => Vec3::new(0.0, 0.0, 1.0),
        }
    }

    /// Where the handles start, in pixels.
    pub fn centre(&self, view: &View) -> Option<Pos2> {
        let (x, y, _) = view.project(self.origin)?;
        Some(Pos2::new(x, y))
    }

    /// The world length of one arm: the length that reads as `STEM_PIXELS`
    /// across the view, and shorter when the view looks down the axis.
    pub fn arm(&self, view: &View) -> f32 {
        STEM_PIXELS / view.pixels_per_mm(self.origin).max(1e-3)
    }

    /// One arm, as the pixel vector it covers on screen.
    fn arm_offset(&self, view: &View, axis: usize) -> Option<Vec2> {
        let centre = self.centre(view)?;
        let tip = self.origin + Self::direction(axis) * self.arm(view);
        let (x, y, _) = view.project(tip)?;
        let v = Vec2::new(x - centre.x, y - centre.y);
        (v.length() >= MIN_PIXELS).then_some(v)
    }

    /// Every handle, as it will be drawn this frame.
    pub fn handles(&self, view: &View) -> Vec<HandleShape> {
        let Some(hub) = self.centre(view) else {
            return Vec::new();
        };
        let arm = self.arm(view);
        let mut out = Vec::new();
        for axis in 0..3 {
            let shape = match self.mode {
                Mode::Move | Mode::Size => match self.arm_offset(view, axis) {
                    Some(v) => Shape::Stem(hub, hub + v),
                    None => continue,
                },
                Mode::Turn => match self.ring(view, axis) {
                    Some(pts) => Shape::Ring(pts),
                    None => continue,
                },
            };
            out.push(HandleShape {
                handle: Handle::Axis(axis),
                shape,
            });
        }
        if self.mode == Mode::Move {
            'planes: for axes in [[0, 1], [1, 2], [2, 0]] {
                // a square between two arms that lie on top of each other on
                // screen cannot be told apart from them, and sliding in that
                // plane would say nothing about either direction
                let (Some(u), Some(v)) = (
                    self.arm_offset(view, axes[0]),
                    self.arm_offset(view, axes[1]),
                ) else {
                    continue;
                };
                let spread = (u.x * v.y - u.y * v.x).abs() / (u.length() * v.length());
                if spread < PLANE_SPREAD {
                    continue;
                }
                let at = self.origin
                    + (Self::direction(axes[0]) + Self::direction(axes[1])) * (arm * SQUARE_AT);
                let Some((x, y, _)) = view.project(at) else {
                    continue;
                };
                let square = Pos2::new(x, y);
                // A plane that covers another arm would take its clicks.
                let covered = (0..3).any(|axis| {
                    if axes.contains(&axis) {
                        return false;
                    }
                    let Some(w) = self.arm_offset(view, axis) else {
                        return false;
                    };
                    distance_to_segment(square, hub, hub + w) < PLANE_PIXELS * 0.5 + 1.0
                });
                if covered {
                    continue;
                }
                // Four actual points in the XY/YZ/ZX plane. This is the key
                // difference from a billboard: edge-on planes really shrink.
                let half = PLANE_PIXELS * 0.5 / view.pixels_per_mm(at).max(1e-3);
                let u = Self::direction(axes[0]) * half;
                let v = Self::direction(axes[1]) * half;
                let world = [at - u - v, at + u - v, at + u + v, at - u + v];
                let mut corners = [square; 4];
                for (i, p) in world.into_iter().enumerate() {
                    let Some((x, y, _)) = view.project(p) else {
                        continue 'planes;
                    };
                    corners[i] = Pos2::new(x, y);
                }
                out.push(HandleShape {
                    handle: Handle::Plane(axes),
                    shape: Shape::Plane {
                        corners,
                        centre: square,
                    },
                });
            }
        }
        out
    }

    /// A turn ring for `axis`, sampled on screen.
    fn ring(&self, view: &View, axis: usize) -> Option<Vec<Pos2>> {
        Some(
            self.projected_ring(view, axis)?
                .into_iter()
                .map(|(point, _)| point)
                .collect(),
        )
    }

    /// Screen points and camera depths around one ring.
    fn projected_ring(&self, view: &View, axis: usize) -> Option<Vec<(Pos2, f32)>> {
        let radius = self.arm(view) * RING_RATIO;
        let (u, v) = turn_plane(axis);
        let mut pts = Vec::with_capacity(RING_STEPS + 1);
        for i in 0..=RING_STEPS {
            let a = std::f32::consts::TAU * i as f32 / RING_STEPS as f32;
            let p = self.origin + (u * a.cos() + v * a.sin()) * radius;
            let (x, y, depth) = view.project(p)?;
            pts.push((Pos2::new(x, y), depth));
        }
        Some(pts)
    }

    /// Every ring segment, from farthest to nearest. Egui is a painter rather
    /// than a depth-buffered scene, so this order is what lets a green or blue
    /// arc pass in front of red when the camera says that it is nearer.
    fn depth_sorted_ring_segments(&self, view: &View) -> Vec<RingSegment> {
        let mut segments = Vec::with_capacity(RING_STEPS * 3);
        for axis in 0..3 {
            let Some(points) = self.projected_ring(view, axis) else {
                continue;
            };
            for pair in points.windows(2) {
                let (a, za) = pair[0];
                let (b, zb) = pair[1];
                segments.push(RingSegment {
                    axis,
                    a,
                    b,
                    depth: (za + zb) * 0.5,
                });
            }
        }
        segments.sort_by(|a, b| b.depth.total_cmp(&a.depth));
        segments
    }

    /// The handle under the pointer, if any.
    pub fn hit(&self, view: &View, pointer: Pos2) -> Option<Handle> {
        let mut best: Option<(f32, u8, Handle)> = None;
        for handle in self.handles(view) {
            let d = handle.distance(pointer);
            if d > GRAB_RADIUS {
                continue;
            }
            let better = match best {
                None => true,
                Some((bd, bp, _)) => {
                    d < bd - 0.5 || ((d - bd).abs() <= 0.5 && handle.handle.priority() < bp)
                }
            };
            if better {
                best = Some((d, handle.handle.priority(), handle.handle));
            }
        }
        best.map(|(_, _, handle)| handle)
    }

    /// What dragging from `press` to `now` asks of the piece. The whole travel
    /// is measured from where the button went down, not a frame's worth, so the
    /// pieces cannot drift.
    pub fn change(&self, view: &View, handle: Handle, press: Pos2, now: Pos2) -> Change {
        let centre = self.centre(view);
        match (self.mode, handle) {
            (Mode::Move, Handle::Axis(axis)) => {
                let Some(offset) = self.arm_offset(view, axis) else {
                    return Change::Move(Vec3::ZERO);
                };
                let along = (now - press).dot(offset.normalized());
                let mm = along * self.arm(view) / offset.length();
                Change::Move(Self::direction(axis) * mm)
            }
            (Mode::Move, Handle::Plane(axes)) => {
                let (Some(u), Some(v)) = (
                    self.arm_offset(view, axes[0]),
                    self.arm_offset(view, axes[1]),
                ) else {
                    return Change::Move(Vec3::ZERO);
                };
                // Solve d = a·u + b·v for the two arms of travel, so the piece
                // follows the pointer across the plane the square lies in.
                let d = now - press;
                let det = u.x * v.y - u.y * v.x;
                if det.abs() < PLANE_SPREAD * u.length() * v.length() {
                    // the plane is edge-on: a drag says nothing about it
                    return Change::Move(Vec3::ZERO);
                }
                let along_u = (d.x * v.y - v.x * d.y) / det;
                let along_v = (u.x * d.y - u.y * d.x) / det;
                Change::Move(
                    (Self::direction(axes[0]) * along_u + Self::direction(axes[1]) * along_v)
                        * self.arm(view),
                )
            }
            (Mode::Turn, _) => {
                let axis = handle.axis();
                let Some(centre) = centre else {
                    return Change::Turn(0.0);
                };
                let a = press - centre;
                let b = now - centre;
                if a.length() < 4.0 || b.length() < 4.0 {
                    return Change::Turn(0.0);
                }
                let sweep = (a.x * b.y - a.y * b.x).atan2(a.dot(b)).to_degrees();
                Change::Turn(sweep * self.winding(view, axis) * TURN_GAIN)
            }
            (Mode::Size, _) => {
                let Some(centre) = centre else {
                    return Change::Size(1.0);
                };
                let was = (press - centre).length();
                let is = (now - centre).length();
                if was < 4.0 {
                    Change::Size(1.0)
                } else {
                    Change::Size(is / was)
                }
            }
        }
    }

    /// Which way round the ring for `axis` appears on screen: `1.0` when
    /// turning in the positive direction goes clockwise as drawn, `-1.0` when
    /// it goes the other way. Without this, half the axes turn the wrong way.
    fn winding(&self, view: &View, axis: usize) -> f32 {
        let (u, v) = turn_plane(axis);
        let radius = self.arm(view) * RING_RATIO;
        let a = self.origin + u * radius;
        let step = 0.05_f32;
        let b = self.origin + (u * step.cos() + v * step.sin()) * radius;
        let (Some((cx, cy, _)), Some((ax, ay, _)), Some((bx, by, _))) =
            (view.project(self.origin), view.project(a), view.project(b))
        else {
            return 1.0;
        };
        let (ax, ay) = (ax - cx, ay - cy);
        let (bx, by) = (bx - cx, by - cy);
        if ax * by - ay * bx >= 0.0 {
            1.0
        } else {
            -1.0
        }
    }

    /// The handles as drawable shapes, in view pixels.
    ///
    /// The gizmo hands back geometry instead of painting it, so the picture's
    /// corner offset stays with whoever owns the panel.
    pub fn shapes(&self, view: &View, hover: Option<Handle>) -> Vec<eframe::egui::Shape> {
        use eframe::egui::{Rect, Rounding, Shape as EShape};
        let mut out = Vec::new();
        let Some(centre) = self.centre(view) else {
            return out;
        };
        // Plane quads sit behind the stems; a stem crossing a translucent
        // plane stays crisp instead of disappearing into its fill.
        for handle in self.handles(view).into_iter().rev() {
            let active = hover == Some(handle.handle);
            let width = if active { WIDTH_ACTIVE } else { WIDTH };
            match &handle.shape {
                Shape::Stem(a, b) => {
                    let axis = handle.handle.axis();
                    let base = AXIS_COL[axis];
                    let col = if active {
                        base.gamma_multiply(1.35)
                    } else {
                        base
                    };
                    out.push(EShape::line_segment(
                        [*a, *b],
                        Stroke::new(width + 2.6, OUTLINE),
                    ));
                    out.push(EShape::line_segment([*a, *b], Stroke::new(width, col)));
                    let dir = (*b - *a).normalized();
                    match self.mode {
                        Mode::Move => {
                            let side = Vec2::new(-dir.y, dir.x) * 7.0;
                            out.push(EShape::convex_polygon(
                                vec![*b + dir * 12.0, *b + side, *b - side],
                                col,
                                Stroke::new(1.3_f32, if active { Color32::WHITE } else { OUTLINE }),
                            ));
                        }
                        _ => {
                            let rect = Rect::from_center_size(*b, Vec2::splat(14.0));
                            out.push(EShape::rect_filled(rect, Rounding::same(2.0), OUTLINE));
                            out.push(EShape::rect_stroke(
                                rect,
                                Rounding::same(2.0),
                                Stroke::new(width, if active { Color32::WHITE } else { col }),
                            ));
                        }
                    }
                }
                Shape::Plane { corners, centre } => {
                    let Handle::Plane(axes) = handle.handle else {
                        continue;
                    };
                    let col = blend(AXIS_COL[axes[0]], AXIS_COL[axes[1]]);
                    out.push(EShape::convex_polygon(
                        corners.to_vec(),
                        if active {
                            col.gamma_multiply(0.95)
                        } else {
                            col.gamma_multiply(0.65)
                        },
                        Stroke::new(3.0_f32, OUTLINE),
                    ));
                    out.push(EShape::closed_line(
                        corners.to_vec(),
                        Stroke::new(
                            if active { 2.4_f32 } else { 1.5_f32 },
                            if active { Color32::WHITE } else { col },
                        ),
                    ));
                    out.push(EShape::circle_filled(*centre, 2.2, col));
                }
                Shape::Ring(_) => {
                    // Turn rings are painted together below in camera-depth
                    // order, never in fixed red/green/blue axis order.
                }
            }
        }
        if self.mode == Mode::Turn {
            for segment in self.depth_sorted_ring_segments(view) {
                let active = hover == Some(Handle::Axis(segment.axis));
                let base = AXIS_COL[segment.axis];
                let (width, col) = if active {
                    (RING_WIDTH_ACTIVE, base.gamma_multiply(1.35))
                } else {
                    (RING_WIDTH, base)
                };
                out.push(EShape::line_segment(
                    [segment.a, segment.b],
                    Stroke::new(width + 2.6, OUTLINE),
                ));
                let delta = segment.b - segment.a;
                // A neighbouring segment's wider under-stroke must not cover
                // the butt cap of this colour stroke and leave a dotted seam.
                // A tiny axial overlap joins the colour without changing the
                // ring's shape or the depth ordering at crossings.
                let join = if delta.length_sq() > 1e-8 {
                    delta.normalized() * 1.6
                } else {
                    Vec2::ZERO
                };
                out.push(EShape::line_segment(
                    [segment.a - join, segment.b + join],
                    Stroke::new(width, col),
                ));
            }
        }
        // the centre marker, tinted with the mode so the set reads at a glance
        let col = match self.mode {
            Mode::Move => Color32::from_rgb(214, 220, 230),
            Mode::Turn => Color32::from_rgb(240, 208, 130),
            Mode::Size => Color32::from_rgb(180, 160, 240),
        };
        out.push(EShape::circle_stroke(
            centre,
            8.5,
            Stroke::new(2.6_f32, OUTLINE),
        ));
        out.push(EShape::circle_stroke(
            centre,
            7.0,
            Stroke::new(1.8_f32, col),
        ));
        out.push(EShape::circle_filled(centre, 2.5, col));
        out
    }
}

/// A colour halfway between two axis colours, for the square that shares them.
fn blend(a: Color32, b: Color32) -> Color32 {
    let mix = |x: u8, y: u8| ((x as u16 + y as u16) / 2) as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

fn point_in_convex(p: Pos2, corners: &[Pos2; 4]) -> bool {
    let mut sign = 0.0_f32;
    for i in 0..4 {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        if cross.abs() < 1e-5 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

/// Two unit vectors that span the plane the ring for `axis` turns in.
fn turn_plane(axis: usize) -> (Vec3, Vec3) {
    match axis {
        0 => (Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)),
        1 => (Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 0.0)),
        _ => (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
    }
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 < 1e-6 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Camera;

    fn view() -> View {
        View::new(&Camera::default(), 900.0, 700.0)
    }

    fn gizmo(mode: Mode) -> Gizmo {
        Gizmo::new(mode, Vec3::new(0.0, 0.0, 12.0))
    }

    fn shape_for(g: &Gizmo, view: &View, want: Handle) -> Shape {
        g.handles(view)
            .into_iter()
            .find(|h| h.handle == want)
            .map(|h| h.shape)
            .unwrap_or_else(|| panic!("{want:?} is missing from {:?}", g.mode))
    }

    /// A point that sits exactly on the drawn handle.
    fn point_on(g: &Gizmo, view: &View, want: Handle, at: f32) -> Pos2 {
        match shape_for(g, view, want) {
            Shape::Stem(a, b) => a + (b - a) * at,
            Shape::Plane { centre, .. } => centre,
            Shape::Ring(pts) => pts[((pts.len() - 1) as f32 * at) as usize],
        }
    }

    #[test]
    fn there_is_one_handle_per_axis_and_a_square_between_each_pair() {
        for mode in [Mode::Move, Mode::Turn, Mode::Size] {
            let g = gizmo(mode);
            let axes = g
                .handles(&view())
                .iter()
                .filter(|h| matches!(h.handle, Handle::Axis(_)))
                .count();
            assert_eq!(axes, 3, "{mode:?}");
        }
        let planes = |cam: Camera| {
            let v = View::new(&cam, 900.0, 700.0);
            gizmo(Mode::Move)
                .handles(&v)
                .iter()
                .filter(|h| matches!(h.handle, Handle::Plane(_)))
                .count()
        };
        // a square is only drawn where it can be told apart from the arms, so
        // a well-spread view shows all three and a degenerate one shows fewer
        assert_eq!(
            planes(Camera {
                yaw: 0.8,
                pitch: 0.5,
                ..Default::default()
            }),
            3,
            "a move gizmo moves in three planes"
        );
        assert!(
            planes(Camera::default()) >= 2,
            "even a plain view keeps most of them"
        );
    }

    #[test]
    fn a_press_on_the_drawn_handle_grabs_it() {
        // the regression test for handles that draw one shape and are grabbed
        // as another: the turn rings used to be hit-tested as spokes, so a
        // press on a ring grabbed nothing at all
        let v = view();
        for mode in [Mode::Move, Mode::Turn, Mode::Size] {
            let g = gizmo(mode);
            let handles = g.handles(&v);
            for h in &handles {
                for at in [0.15, 0.45, 0.75, 1.0] {
                    let p = point_on(&g, &v, h.handle, at);
                    let hit = g.hit(&v, p).unwrap_or_else(|| {
                        panic!("{mode:?}: nothing grabs the drawn {:?}", h.handle)
                    });
                    // where two handles cross, either answer is fair
                    let there: Vec<Handle> = handles
                        .iter()
                        .filter(|o| o.distance(p) <= 0.75)
                        .map(|o| o.handle)
                        .collect();
                    assert!(
                        there.contains(&hit),
                        "{mode:?}: a press on {:?} grabbed {hit:?}, but only {there:?} run through that point",
                        h.handle
                    );
                }
            }
        }
    }

    #[test]
    fn a_press_away_from_the_handles_grabs_nothing() {
        let v = view();
        let g = gizmo(Mode::Turn);
        let centre = g.centre(&v).unwrap();
        assert_eq!(g.hit(&v, centre), None, "the middle is not a handle");
        assert_eq!(g.hit(&v, centre + Vec2::new(0.0, -400.0)), None);
    }

    #[test]
    fn the_rings_are_heavy_and_heavier_under_the_hand() {
        use eframe::egui::Shape as EShape;
        let view = view();
        let g = Gizmo::new(Mode::Turn, Vec3::new(0.0, 0.0, 10.0));
        let ring_widths = |hover: Option<Handle>| -> Vec<f32> {
            g.shapes(&view, hover)
                .into_iter()
                .filter_map(|shape| match shape {
                    EShape::Path(path) => Some(path.stroke.width),
                    EShape::LineSegment { stroke, .. } => Some(stroke.width),
                    _ => None,
                })
                .collect()
        };
        let plain = ring_widths(None);
        assert_eq!(
            plain.len(),
            RING_STEPS * 6,
            "every depth-sorted ring segment has a contrast under-stroke"
        );
        for width in plain.iter().step_by(2) {
            assert_eq!(
                *width,
                RING_WIDTH + 2.6,
                "every ring has the same dark contrast outline"
            );
        }
        const {
            assert!(RING_WIDTH > WIDTH);
        }
        for width in plain.iter().skip(1).step_by(2) {
            assert_eq!(*width, RING_WIDTH, "every ring has the same colour weight");
        }
        let grabbed = ring_widths(Some(Handle::Axis(1)));
        assert!(
            grabbed.contains(&RING_WIDTH_ACTIVE),
            "the ring being turned must thicken"
        );
    }

    #[test]
    fn ring_crossings_follow_camera_depth_not_axis_order() {
        fn crosses(a: &RingSegment, b: &RingSegment) -> bool {
            let r = a.b - a.a;
            let s = b.b - b.a;
            let cross = |u: Vec2, v: Vec2| u.x * v.y - u.y * v.x;
            let denominator = cross(r, s);
            if denominator.abs() < 1e-5 {
                return false;
            }
            let between = b.a - a.a;
            let ta = cross(between, s) / denominator;
            let tb = cross(between, r) / denominator;
            (0.0..=1.0).contains(&ta) && (0.0..=1.0).contains(&tb)
        }

        let v = view();
        let segments = gizmo(Mode::Turn).depth_sorted_ring_segments(&v);
        assert!(segments
            .windows(2)
            .all(|pair| pair[0].depth >= pair[1].depth));
        let mut red_in_front = false;
        let mut green_in_front = false;
        for (red_index, red) in segments.iter().enumerate().filter(|(_, s)| s.axis == 0) {
            for (green_index, green) in segments.iter().enumerate().filter(|(_, s)| s.axis == 1) {
                if !crosses(red, green) || (red.depth - green.depth).abs() < 0.25 {
                    continue;
                }
                if red.depth < green.depth {
                    assert!(red_index > green_index);
                    red_in_front = true;
                } else {
                    assert!(green_index > red_index);
                    green_in_front = true;
                }
            }
        }
        assert!(
            red_in_front && green_in_front,
            "the same view should contain crossings with each ring in front"
        );
    }

    #[test]
    fn the_rings_are_all_the_same_size() {
        // a circle's widest on-screen chord is its diameter, whichever way its
        // plane leans, so every ring has to reach the same width for the three
        // to read as one set
        let v = view();
        let g = gizmo(Mode::Turn);
        let want = 2.0 * g.arm(&v) * RING_RATIO * v.pixels_per_mm(g.origin);
        for axis in 0..3 {
            let Shape::Ring(pts) = shape_for(&g, &v, Handle::Axis(axis)) else {
                panic!("a turn gizmo draws rings")
            };
            let mut widest: f32 = 0.0;
            for (i, a) in pts.iter().enumerate() {
                for b in &pts[i + 1..] {
                    widest = widest.max((*b - *a).length());
                }
            }
            assert!(
                (widest - want).abs() < want * 0.03,
                "axis {axis} ring is {widest} px across, want {want}"
            );
        }
    }

    #[test]
    fn dragging_a_stem_moves_the_piece_the_way_the_arrow_points() {
        let v = view();
        let g = gizmo(Mode::Move);
        let press = point_on(&g, &v, Handle::Axis(0), 0.0);
        let Shape::Stem(a, b) = shape_for(&g, &v, Handle::Axis(0)) else {
            panic!()
        };
        let along = (b - a).normalized();
        let now = press + along * 37.0;
        let Change::Move(d) = g.change(&v, Handle::Axis(0), press, now) else {
            panic!("a move gizmo moves")
        };
        assert!(d.x > 1.0, "the piece has to follow X: {d:?}");
        assert!(d.y.abs() < 1e-3 && d.z.abs() < 1e-3, "only X: {d:?}");
        // sideways travel along the stem says nothing
        let side = Vec2::new(-along.y, along.x);
        let Change::Move(sd) = g.change(&v, Handle::Axis(0), press, press + side * 40.0) else {
            panic!()
        };
        assert!(sd.length() < 0.6, "{sd:?}");
    }

    #[test]
    fn dragging_a_square_moves_in_both_directions_at_once() {
        let v = view();
        let g = gizmo(Mode::Move);
        let press = point_on(&g, &v, Handle::Plane([0, 1]), 1.0);
        let Shape::Stem(x0, x1) = shape_for(&g, &v, Handle::Axis(0)) else {
            panic!()
        };
        let Shape::Stem(y0, y1) = shape_for(&g, &v, Handle::Axis(1)) else {
            panic!()
        };
        let u = x1 - x0;
        let w = y1 - y0;
        let now = press + u * 0.4 + w * 0.4;
        let Change::Move(d) = g.change(&v, Handle::Plane([0, 1]), press, now) else {
            panic!("a square moves")
        };
        let want = g.arm(&v) * 0.4;
        assert!((d.x - want).abs() < 0.05, "x {} want {want}", d.x);
        assert!((d.y - want).abs() < 0.05, "y {} want {want}", d.y);
        assert!(d.z.abs() < 1e-3, "a plane handle must not move in Z: {d:?}");
    }

    #[test]
    fn dragging_round_a_ring_gives_degrees() {
        let v = view();
        let g = gizmo(Mode::Turn);
        let centre = g.centre(&v).unwrap();
        for axis in 0..3 {
            let start = Vec2::new(0.0, -60.0);
            let quarter = Vec2::new(60.0 * g.winding(&v, axis), 0.0);
            let Change::Turn(deg) =
                g.change(&v, Handle::Axis(axis), centre + start, centre + quarter)
            else {
                panic!("a turn gizmo turns")
            };
            assert!((deg - 90.0).abs() < 1.0, "axis {axis}: {deg}");
            let Change::Turn(back) =
                g.change(&v, Handle::Axis(axis), centre + start, centre - start)
            else {
                panic!()
            };
            assert!((back.abs() - 180.0).abs() < 1.0, "axis {axis}: {back}");
        }
    }

    #[test]
    fn ring_winding_does_not_depend_on_screen_position() {
        for origin in [
            Vec3::new(0.0, 0.0, 12.0),
            Vec3::new(-75.0, 45.0, 8.0),
            Vec3::new(95.0, -30.0, 22.0),
        ] {
            let v = view();
            let g = Gizmo::new(Mode::Turn, origin);
            for axis in 0..3 {
                let (u, w) = turn_plane(axis);
                let radius = g.arm(&v) * RING_RATIO;
                let step = 0.08_f32;
                let project = |p| {
                    let (x, y, _) = v.project(p).unwrap();
                    Pos2::new(x, y)
                };
                let a = project(origin + u * radius);
                let b = project(origin + (u * step.cos() + w * step.sin()) * radius);
                let Change::Turn(degrees) = g.change(&v, Handle::Axis(axis), a, b) else {
                    panic!("a turn gizmo turns")
                };
                assert!(
                    degrees > 0.0,
                    "positive world rotation reversed at {origin:?}, axis {axis}: {degrees}"
                );
            }
        }
    }

    #[test]
    fn dragging_out_from_the_centre_scales_by_the_distance_ratio() {
        let v = view();
        let g = gizmo(Mode::Size);
        let centre = g.centre(&v).unwrap();
        let press = centre + Vec2::new(40.0, 0.0);
        let Change::Size(twice) =
            g.change(&v, Handle::Axis(0), press, centre + Vec2::new(80.0, 0.0))
        else {
            panic!()
        };
        assert!((twice - 2.0).abs() < 0.02, "{twice}");
        let Change::Size(back) =
            g.change(&v, Handle::Axis(0), press, centre + Vec2::new(20.0, 0.0))
        else {
            panic!()
        };
        assert!((back - 0.5).abs() < 0.02, "{back}");
        let Change::Size(mid) = g.change(&v, Handle::Axis(0), centre, centre + Vec2::new(5.0, 0.0))
        else {
            panic!()
        };
        assert_eq!(mid, 1.0, "grabbing at the centre cannot divide by nothing");
    }

    #[test]
    fn an_arm_reads_the_same_size_at_any_zoom() {
        let g = gizmo(Mode::Move);
        let mut longest = Vec::new();
        for dist in [120.0, 290.0, 600.0] {
            let v = View::new(
                &Camera {
                    dist,
                    ..Default::default()
                },
                900.0,
                700.0,
            );
            let arm = g
                .handles(&v)
                .iter()
                .filter_map(|h| match &h.shape {
                    Shape::Stem(a, b) => Some((*b - *a).length()),
                    _ => None,
                })
                .fold(0.0_f32, f32::max);
            assert!(arm > 20.0, "at dist {dist} no arm is usable: {arm}");
            longest.push(arm);
        }
        let most = longest.iter().cloned().fold(f32::MIN, f32::max);
        let least = longest.iter().cloned().fold(f32::MAX, f32::min);
        assert!(
            most - least < 1.0,
            "the set must not resize while zooming: {longest:?}"
        );
        // and the largest of the three is a leaning arm, so it reads shorter
        // than an arm lying across the view
        assert!(most < STEM_PIXELS, "no arm should be blown up to {most}");
    }

    #[test]
    fn an_arm_foreshortens_like_the_model_instead_of_being_stretched() {
        // the old code forced every stem to exactly STEM_PIXELS on screen,
        // whatever the view did, so the arrows never shortened when an axis
        // leaned towards the camera and the set seemed to change shape as the
        // view turned
        let g = gizmo(Mode::Move);
        for cam in [
            Camera::default(),
            Camera {
                yaw: 0.9,
                pitch: 0.2,
                ..Default::default()
            },
            Camera {
                yaw: -2.1,
                pitch: 1.1,
                ..Default::default()
            },
        ] {
            let v = View::new(&cam, 900.0, 700.0);
            let mut leaning = 0;
            for axis in 0..3 {
                let Shape::Stem(a, b) = shape_for(&g, &v, Handle::Axis(axis)) else {
                    panic!()
                };
                // a stem is exactly the projection of one world arm
                let tip = g.origin + Gizmo::direction(axis) * g.arm(&v);
                let (x, y, _) = v.project(tip).unwrap();
                let want = Pos2::new(x, y);
                assert!(
                    (want - b).length() < 0.01,
                    "axis {axis} ends at {b:?}, projected {want:?}"
                );
                if (b - a).length() < STEM_PIXELS - 5.0 {
                    leaning += 1;
                }
            }
            assert!(
                leaning > 0,
                "at least one arm must lean away from the view at yaw {}",
                cam.yaw
            );
        }
    }

    #[test]
    fn move_planes_are_projected_world_quads_not_screen_billboards() {
        let v = view();
        let Shape::Plane { corners, .. } = shape_for(&gizmo(Mode::Move), &v, Handle::Plane([0, 1]))
        else {
            panic!("a move gizmo has an XY plane")
        };
        // A camera-facing rectangle has two horizontal and two vertical
        // edges. A real projected XY quad at this oblique view has neither.
        for i in 0..4 {
            let edge = corners[(i + 1) % 4] - corners[i];
            assert!(
                edge.x.abs() > 0.1 && edge.y.abs() > 0.1,
                "edge {i} is screen-aligned: {edge:?}"
            );
        }
    }
}
