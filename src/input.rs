//! What the pointer means in the viewport.
//!
//! One place decides the button map — right drag orbits, middle drag pans, a
//! left press on a handle grabs it, any other left drag draws a marquee, and a
//! left click picks — and reports the result as an [`Action`] for the window to
//! carry out. Nothing here moves the camera, the pieces or the handles: it only
//! reads the pointer and says what it was doing.

use eframe::egui::{CursorIcon, Pos2, Rect, Response, Vec2};

use crate::gizmo::{self, Gizmo};
use crate::render::View;

/// How far the pointer may travel and still count as a click, in pixels.
const CLICK_SLOP: f32 = 5.0;

/// What the pointer did this frame, in the units it was done in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// A left click that never became a drag.
    Click { at: Pos2, additive: bool },
    /// The marquee as it stands, for drawing.
    Marquee { area: Rect },
    /// The marquee was let go.
    MarqueeEnd { area: Rect, additive: bool },
    /// A box drag ended without a usable final pointer position.
    MarqueeCancel,
    /// Turn the view.
    Orbit(Vec2),
    /// Slide the view.
    Pan(Vec2),
    /// The left button went down on a handle.
    Grab { handle: gizmo::Handle, gizmo: Gizmo },
    /// And it has moved this much since: millimetres, degrees or a factor,
    /// measured from the press, so repeating it never drifts.
    Drag { change: gizmo::Change },
    /// The button came back up.
    Release,
}

/// The pointer as it stands this frame, read off an egui response.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pointer {
    pub pos: Option<Pos2>,
    pub delta: Vec2,
    pub primary_released: bool,
    pub secondary_released: bool,
    pub middle_released: bool,
    pub primary_pressed: bool,
    pub secondary_pressed: bool,
    pub middle_pressed: bool,
    /// Shift, control or command: adds to the selection instead of replacing it.
    pub additive: bool,
    pub hovering: bool,
}

impl Pointer {
    /// Move the pointer into the picture's own pixels.
    ///
    /// Everything downstream — rays, handles, the marquee — is worked out in
    /// the coordinates of the view texture, which start at zero in the corner
    /// of the panel. Only this boundary knows where the panel sits.
    pub fn in_view(mut self, rect: Rect) -> Self {
        self.pos = self.pos.map(|p| p - rect.min.to_vec2());
        self
    }

    pub fn read(resp: &Response) -> Self {
        resp.ctx.input(|i| Self {
            pos: i.pointer.interact_pos(),
            delta: i.pointer.delta(),
            primary_released: i.pointer.button_released(egui::PointerButton::Primary),
            secondary_released: i.pointer.button_released(egui::PointerButton::Secondary),
            middle_released: i.pointer.button_released(egui::PointerButton::Middle),
            primary_pressed: i.pointer.primary_pressed(),
            secondary_pressed: i.pointer.secondary_pressed(),
            middle_pressed: i.pointer.button_pressed(egui::PointerButton::Middle),
            additive: i.modifiers.shift || i.modifiers.command || i.modifiers.ctrl,
            hovering: resp.hovered(),
        })
    }
}

/// What is being dragged right now.
#[derive(Clone, Copy, Debug, Default)]
enum Drag {
    #[default]
    Idle,
    Orbit,
    Pan,
    Box {
        start: Pos2,
        additive: bool,
    },
    Handle {
        handle: gizmo::Handle,
        press: Pos2,
        gizmo: Gizmo,
    },
}

/// The pointer's own state machine, carried from frame to frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    drag: Drag,
}

impl Drag {
    fn released_by(self, p: Pointer) -> bool {
        match self {
            Self::Idle => false,
            Self::Orbit => p.secondary_released,
            Self::Pan => p.middle_released,
            Self::Box { .. } | Self::Handle { .. } => p.primary_released,
        }
    }
}

impl Input {
    pub fn is_dragging(&self) -> bool {
        !matches!(self.drag, Drag::Idle)
    }

    /// Read the pointer and say what it means.
    pub fn update(&mut self, p: Pointer, gizmo: Option<&Gizmo>, view: &View) -> Vec<Action> {
        let mut out = Vec::new();
        let Some(pos) = p.pos else {
            // the pointer left; a drag that ends without a position still ends
            if self.drag.released_by(p) {
                self.finish(&mut out);
            }
            return out;
        };

        match self.drag {
            Drag::Idle => {
                if !p.hovering {
                    return out;
                }
                if p.primary_pressed {
                    match gizmo.and_then(|g| g.hit(view, pos)) {
                        Some(handle) => {
                            let gizmo = *gizmo.expect("a handle came from a gizmo");
                            self.drag = Drag::Handle {
                                handle,
                                press: pos,
                                gizmo,
                            };
                            out.push(Action::Grab { handle, gizmo });
                        }
                        None => {
                            self.drag = Drag::Box {
                                start: pos,
                                additive: p.additive,
                            };
                        }
                    }
                } else if p.secondary_pressed {
                    self.drag = Drag::Orbit;
                } else if p.middle_pressed {
                    self.drag = Drag::Pan;
                }
            }
            Drag::Orbit => {
                if self.drag.released_by(p) {
                    self.drag = Drag::Idle;
                } else {
                    out.push(Action::Orbit(p.delta));
                }
            }
            Drag::Pan => {
                if self.drag.released_by(p) {
                    self.drag = Drag::Idle;
                } else {
                    out.push(Action::Pan(p.delta));
                }
            }
            Drag::Box { start, additive } => {
                let area = Rect::from_two_pos(start, pos);
                if self.drag.released_by(p) {
                    self.drag = Drag::Idle;
                    if (pos - start).length() < CLICK_SLOP {
                        out.push(Action::Click {
                            at: start,
                            additive,
                        });
                    } else {
                        out.push(Action::MarqueeEnd { area, additive });
                    }
                } else {
                    out.push(Action::Marquee { area });
                }
            }
            Drag::Handle {
                handle,
                press,
                gizmo,
            } => {
                if self.drag.released_by(p) {
                    // the drag is over; the amount of this last frame is the
                    // same one the frame before already carried
                    self.drag = Drag::Idle;
                    out.push(Action::Release);
                } else {
                    out.push(Action::Drag {
                        change: gizmo.change(view, handle, press, pos),
                    });
                }
            }
        }
        out
    }

    /// End whatever was going on, when the pointer goes away mid-drag.
    fn finish(&mut self, out: &mut Vec<Action>) {
        match self.drag {
            Drag::Handle { .. } => out.push(Action::Release),
            Drag::Box { .. } => out.push(Action::MarqueeCancel),
            Drag::Idle | Drag::Orbit | Drag::Pan => {}
        }
        self.drag = Drag::Idle;
    }

    /// What the pointer should look like: a grab hand over a handle, otherwise
    /// whatever the tool suggests.
    pub fn cursor(&self, gizmo: Option<&Gizmo>, view: &View, pos: Option<Pos2>) -> CursorIcon {
        match self.drag {
            Drag::Orbit => CursorIcon::Grabbing,
            Drag::Pan => CursorIcon::Move,
            Drag::Handle { .. } => CursorIcon::Grabbing,
            Drag::Box { .. } => CursorIcon::Crosshair,
            Drag::Idle => match (gizmo, pos) {
                (Some(g), Some(p)) => match g.hit(view, p).map(|h| h.axis()) {
                    Some(0) => CursorIcon::ResizeHorizontal,
                    Some(1) => CursorIcon::ResizeVertical,
                    Some(_) => CursorIcon::ResizeNwSe,
                    None => CursorIcon::Default,
                },
                _ => CursorIcon::Default,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gizmo::Mode;
    use crate::gizmo::Shape;
    use crate::mesh::Vec3;
    use crate::render::Camera;

    fn view() -> View {
        View::new(&Camera::default(), 900.0, 700.0)
    }

    fn pointer(pos: Pos2) -> Pointer {
        Pointer {
            pos: Some(pos),
            delta: Vec2::ZERO,
            primary_released: false,
            secondary_released: false,
            middle_released: false,
            primary_pressed: false,
            secondary_pressed: false,
            middle_pressed: false,
            additive: false,
            hovering: true,
        }
    }

    fn move_gizmo() -> Gizmo {
        Gizmo::new(Mode::Move, Vec3::new(0.0, 0.0, 12.0))
    }

    /// Press at `from`, then drag to `to`.
    fn press_and_drag(
        input: &mut Input,
        from: Pos2,
        to: Pos2,
        gizmo: Option<&Gizmo>,
    ) -> Vec<Action> {
        let v = view();
        let mut out = Vec::new();
        let mut p = pointer(from);
        p.primary_pressed = true;
        out.extend(input.update(p, gizmo, &v));
        p.primary_pressed = false;
        p.delta = to - from;
        p.pos = Some(to);
        out.extend(input.update(p, gizmo, &v));
        out
    }

    #[test]
    fn right_drag_orbits_and_middle_drag_pans() {
        let v = view();
        let mut input = Input::default();
        let at = Pos2::new(400.0, 300.0);

        let mut p = pointer(at);
        p.secondary_pressed = true;
        assert!(
            input.update(p, None, &v).is_empty(),
            "a press alone says nothing yet"
        );
        p.secondary_pressed = false;
        p.delta = Vec2::new(12.0, 3.0);
        assert_eq!(
            input.update(p, None, &v),
            vec![Action::Orbit(Vec2::new(12.0, 3.0))]
        );
        p.secondary_released = true;
        assert!(
            input.update(p, None, &v).is_empty(),
            "letting go ends the orbit"
        );

        let mut p = pointer(at);
        p.middle_pressed = true;
        input.update(p, None, &v);
        p.middle_pressed = false;
        p.delta = Vec2::new(-5.0, 7.0);
        assert_eq!(
            input.update(p, None, &v),
            vec![Action::Pan(Vec2::new(-5.0, 7.0))]
        );
    }

    #[test]
    fn a_left_press_that_does_not_move_is_a_click() {
        let v = view();
        let mut input = Input::default();
        let at = Pos2::new(300.0, 260.0);
        let mut p = pointer(at);
        p.primary_pressed = true;
        assert!(input.update(p, None, &v).is_empty());
        p.primary_pressed = false;
        p.primary_released = true;
        assert_eq!(
            input.update(p, None, &v),
            vec![Action::Click {
                at,
                additive: false
            }]
        );
    }

    #[test]
    fn a_left_drag_draws_a_marquee_and_never_orbits() {
        let v = view();
        let mut input = Input::default();
        let from = Pos2::new(200.0, 200.0);
        let to = Pos2::new(360.0, 330.0);
        let actions = press_and_drag(&mut input, from, to, None);
        assert!(
            matches!(actions.first(), Some(Action::Marquee { .. })),
            "a left drag is a marquee: {actions:?}"
        );
        assert!(
            !actions.iter().any(|a| matches!(a, Action::Orbit(_))),
            "and must not spin the camera"
        );
        let mut p = pointer(to);
        p.primary_released = true;
        match input.update(p, None, &v).as_slice() {
            [Action::MarqueeEnd {
                area,
                additive: false,
            }] => {
                assert!((area.width() - 160.0).abs() < 0.1 && (area.height() - 130.0).abs() < 0.1);
            }
            other => panic!("expected the marquee to end, got {other:?}"),
        }
    }

    /// A point half way along the drawn stem for `axis`.
    fn on_a_stem(g: &Gizmo, view: &View, axis: usize) -> Pos2 {
        let h = g
            .handles(view)
            .into_iter()
            .find(|h| h.handle == gizmo::Handle::Axis(axis))
            .expect("an axis is missing");
        match h.shape {
            Shape::Stem(a, b) => a + (b - a) * 0.5,
            _ => panic!("this gizmo draws stems"),
        }
    }

    #[test]
    fn a_press_on_a_handle_grabs_it_instead_of_starting_a_marquee() {
        let v = view();
        let g = move_gizmo();
        let on_arrow = on_a_stem(&g, &v, 0);
        let mut input = Input::default();

        let mut p = pointer(on_arrow);
        p.primary_pressed = true;
        let actions = input.update(p, Some(&g), &v);
        assert!(
            matches!(actions.as_slice(), [Action::Grab { .. }]),
            "{actions:?}"
        );

        p.primary_pressed = false;
        p.pos = Some(on_arrow + Vec2::new(30.0, 2.0));
        let actions = input.update(p, Some(&g), &v);
        assert!(
            matches!(actions.as_slice(), [Action::Drag { .. }]),
            "a grabbed handle drags the piece, it does not draw a box: {actions:?}"
        );

        p.primary_released = true;
        assert_eq!(input.update(p, Some(&g), &v), vec![Action::Release]);
        // and afterwards a drag is a marquee again
        assert_eq!(
            input.cursor(Some(&g), &v, Some(Pos2::new(5.0, 5.0))),
            CursorIcon::Default
        );
    }

    #[test]
    fn shift_clicking_is_passed_through_as_additive() {
        let v = view();
        let mut input = Input::default();
        let at = Pos2::new(120.0, 140.0);
        let mut p = pointer(at);
        p.primary_pressed = true;
        p.additive = true;
        input.update(p, None, &v);
        p.primary_pressed = false;
        p.primary_released = true;
        assert_eq!(
            input.update(p, None, &v),
            vec![Action::Click { at, additive: true }]
        );
    }

    #[test]
    fn releasing_another_button_does_not_release_a_handle() {
        let v = view();
        let g = move_gizmo();
        let mut input = Input::default();
        let mut p = pointer(on_a_stem(&g, &v, 0));
        p.primary_pressed = true;
        input.update(p, Some(&g), &v);
        p.primary_pressed = false;
        p.secondary_released = true;
        assert!(matches!(
            input.update(p, Some(&g), &v).as_slice(),
            [Action::Drag { .. }]
        ));
        assert!(input.is_dragging());
        p.secondary_released = false;
        p.primary_released = true;
        assert_eq!(input.update(p, Some(&g), &v), vec![Action::Release]);
        assert!(!input.is_dragging());
    }

    #[test]
    fn a_marquee_without_a_release_position_is_cancelled() {
        let v = view();
        let mut input = Input::default();
        let mut p = pointer(Pos2::new(100.0, 100.0));
        p.primary_pressed = true;
        input.update(p, None, &v);
        p.primary_pressed = false;
        p.pos = Some(Pos2::new(220.0, 250.0));
        assert!(matches!(
            input.update(p, None, &v).as_slice(),
            [Action::Marquee { .. }]
        ));
        p.pos = None;
        p.primary_released = true;
        assert_eq!(input.update(p, None, &v), vec![Action::MarqueeCancel]);
        assert!(!input.is_dragging());
    }

    #[test]
    fn a_drag_that_leaves_the_window_still_ends_cleanly() {
        let v = view();
        let g = move_gizmo();
        let centre = g.centre(&v).unwrap();
        let mut input = Input::default();
        let mut p = pointer(on_a_stem(&g, &v, 0));
        p.primary_pressed = true;
        input.update(p, Some(&g), &v);
        p.primary_pressed = false;
        p.pos = None;
        p.primary_released = true;
        assert_eq!(input.update(p, Some(&g), &v), vec![Action::Release]);
        // and the next frame is idle again
        p.pos = Some(centre);
        assert!(input.update(p, Some(&g), &v).is_empty());
    }
}
