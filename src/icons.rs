//! The icon set the window draws with.
//!
//! The pictures themselves come from the Phosphor icon font, so this module is
//! a vocabulary rather than a drawing library: each name below says what a
//! picture means, and `paint` puts it on screen. Nothing here knows about
//! buttons, tools or layout, and swapping the icon family later means changing
//! the font once instead of redrawing a dozen shapes.

use eframe::egui::{self, Align2, Color32, FontId, Painter};

use egui_phosphor::regular as phosphor;

/// One picture. Icons are font glyphs, so they scale and tint for free.
pub type Icon = &'static str;

pub const OPEN: Icon = phosphor::FOLDER_OPEN;
pub const PRINTER: Icon = phosphor::PRINTER;
pub const FIT: Icon = phosphor::CORNERS_OUT;
pub const VALUES: Icon = phosphor::RULER;
pub const VIEW: Icon = phosphor::CURSOR;
pub const MOVE: Icon = phosphor::ARROWS_OUT_CARDINAL;
pub const TURN: Icon = phosphor::ARROWS_CLOCKWISE;
pub const SIZE: Icon = phosphor::RESIZE;
pub const CUBE: Icon = phosphor::CUBE;
pub const LAYERS: Icon = phosphor::STACK;
pub const PLAY: Icon = phosphor::PLAY;
pub const PAUSE: Icon = phosphor::PAUSE;
pub const SKIP_BACK: Icon = phosphor::SKIP_BACK;
pub const SKIP_FORWARD: Icon = phosphor::SKIP_FORWARD;
pub const DUPLICATE: Icon = phosphor::COPY;
pub const MIRROR: Icon = phosphor::FLIP_HORIZONTAL;
pub const DELETE: Icon = phosphor::TRASH;

/// How much of a square box the picture fills, in the font's em size.
const FILL: f32 = 0.82;

/// Make the icon glyphs available to the window. Call once, when it opens:
/// this registers the font as a fallback next to the text font, so an icon can
/// also be written straight into a label.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
}

/// Draw `icon` centred in `rect`, sized to fill it.
pub fn paint(painter: &Painter, rect: egui::Rect, icon: Icon, colour: Color32) {
    let side = rect.width().min(rect.height());
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon,
        FontId::proportional(side * FILL),
        colour,
    );
}
