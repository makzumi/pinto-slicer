//! The window and everything in it: a tab per step of the job (prepare,
//! supports, slice, export), the 3D view in the middle and a tool strip.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use eframe::egui;

use crate::branding;
use crate::config::{self, PrintSettings, PrinterProfile};
use crate::dialog;
use crate::export;
use crate::gizmo;
use crate::gpu_render;
use crate::icons;
use crate::input;
use crate::layout;
use crate::mesh::{self, Mesh, Transform, Vec3};
use crate::nav_cube;
use crate::project;
use crate::render::{self, Camera, Scene, ViewMode};
use crate::selection::Selection;
use crate::slicer::{self, SliceResult};
use crate::supports::{self, bvh::Bvh, ContactShape, Preset, SupportModel, TipShape};

// ------------------------------------------------------------------ palette

const BG: egui::Color32 = egui::Color32::from_rgb(23, 25, 30);
const BAR: egui::Color32 = egui::Color32::from_rgb(19, 21, 25);
const CARD: egui::Color32 = egui::Color32::from_rgb(31, 34, 40);
const PILL: egui::Color32 = egui::Color32::from_rgb(42, 46, 54);
const HOVER: egui::Color32 = egui::Color32::from_rgb(46, 51, 60);
const TEXT: egui::Color32 = egui::Color32::from_rgb(232, 235, 241);
const MUTED: egui::Color32 = egui::Color32::from_rgb(143, 151, 165);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(74, 142, 236);
const ACCENT_BG: egui::Color32 = egui::Color32::from_rgb(38, 68, 112);
/// The grey-blue the printed supports are drawn in, used by the panel picture
/// so it matches what appears in the view.
const SUP_COL: egui::Color32 = egui::Color32::from_rgb(158, 164, 176);
const GOOD: egui::Color32 = egui::Color32::from_rgb(120, 200, 160);
const WARN: egui::Color32 = egui::Color32::from_rgb(232, 168, 96);
/// The axis colours the gizmo and the numbers window share, so an X handle and
/// the X value read as the same thing.
const AXIS_COL: [egui::Color32; 3] = [
    egui::Color32::from_rgb(226, 118, 118),
    egui::Color32::from_rgb(126, 200, 132),
    egui::Color32::from_rgb(112, 158, 236),
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Prepare,
    Supports,
    Slice,
    Export,
}

impl Tab {
    /// The steps are numbered because they are meant to be walked in order,
    /// though nothing is gated: step 4 answers with a notice, not a lock.
    pub fn number(self) -> usize {
        match self {
            Self::Prepare => 1,
            Self::Supports => 2,
            Self::Slice => 3,
            Self::Export => 4,
        }
    }

    /// The step after this one, if there is one.
    fn next(self) -> Option<Self> {
        match self {
            Self::Prepare => Some(Self::Supports),
            Self::Supports => Some(Self::Slice),
            Self::Slice => Some(Self::Export),
            Self::Export => None,
        }
    }

    /// The step before this one, if there is one.
    fn back(self) -> Option<Self> {
        match self {
            Self::Prepare => None,
            Self::Supports => Some(Self::Prepare),
            Self::Slice => Some(Self::Supports),
            Self::Export => Some(Self::Slice),
        }
    }

    /// What this step is for, in one line under its name.
    fn purpose(self) -> &'static str {
        match self {
            Self::Prepare => "Put the pieces where you want them",
            Self::Supports => "Grow the supports for this plate",
            Self::Slice => "Cut the layers and look through them",
            Self::Export => "Write the file the printer reads",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Prepare => "Prepare",
            Self::Supports => "Supports",
            Self::Slice => "Slice",
            Self::Export => "Export",
        }
    }

    /// Pick a tab by the name shown in the window, for `--tab`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_lowercase().as_str() {
            "prepare" | "model" | "prepare/model" => Some(Self::Prepare),
            "supports" => Some(Self::Supports),
            "slice" | "slices" => Some(Self::Slice),
            "export" => Some(Self::Export),
            _ => None,
        }
    }
}

/// What the pointer does in the view. Supports are not a tool: they are grown
/// by one run of the generator, so there is nothing to place by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    View,
    Move,
    Turn,
    Size,
}

impl Tool {
    /// Pick a tool by the name the window shows, for `--tool`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_lowercase().as_str() {
            "view" | "look" => Some(Self::View),
            "move" => Some(Self::Move),
            "turn" | "rotate" => Some(Self::Turn),
            "size" | "scale" => Some(Self::Size),
            _ => None,
        }
    }

    /// The strip's tools in the order they are offered.
    pub const ALL: [Self; 4] = [Self::View, Self::Move, Self::Turn, Self::Size];

    /// What the strip calls it, under the glyph.
    pub fn label(self) -> &'static str {
        match self {
            Self::View => "View",
            Self::Move => "Move",
            Self::Turn => "Turn",
            Self::Size => "Scale",
        }
    }

    pub fn icon(self) -> icons::Icon {
        match self {
            Self::View => icons::VIEW,
            Self::Move => icons::MOVE,
            Self::Turn => icons::TURN,
            Self::Size => icons::SIZE,
        }
    }

    /// What the glyph cannot say, for the hover tip.
    pub fn hint(self) -> &'static str {
        match self {
            Self::View => "Look at the plate, change nothing",
            Self::Move => {
                "Drag an arrow to slide the piece, or a square to slide it in two directions"
            }
            Self::Turn => "Drag a ring to turn the piece about its centre",
            Self::Size => "Drag out from the centre to make the piece bigger",
        }
    }

    pub fn shortcut(self) -> Shortcut {
        match self {
            Self::View => LOOK,
            Self::Move => MOVE,
            Self::Turn => TURN,
            Self::Size => SCALE,
        }
    }

    /// The hover text: what it does, then the key that does it too.
    pub fn tip(self) -> String {
        self.shortcut().tip(self.hint())
    }
}

/// One key press that stands for a button, named the way a person would write
/// it so the tooltip and the handler cannot drift apart.
#[derive(Clone, Copy)]
pub struct Shortcut {
    name: &'static str,
    key: egui::Key,
    with_command: bool,
}

impl Shortcut {
    /// Did this key go down in this frame?
    pub fn pressed(self, i: &egui::InputState) -> bool {
        i.key_pressed(self.key) && i.modifiers.command == self.with_command
    }

    /// `what it does  ·  key`.
    pub fn tip(self, what: &str) -> String {
        format!("{what}  ·  {}", self.name())
    }

    pub fn name(self) -> &'static str {
        self.name
    }
}

const LOOK: Shortcut = Shortcut {
    name: "W",
    key: egui::Key::W,
    with_command: false,
};
const MOVE: Shortcut = Shortcut {
    name: "G",
    key: egui::Key::G,
    with_command: false,
};
const TURN: Shortcut = Shortcut {
    name: "R",
    key: egui::Key::R,
    with_command: false,
};
const SCALE: Shortcut = Shortcut {
    name: "S",
    key: egui::Key::S,
    with_command: false,
};
const OPEN: Shortcut = Shortcut {
    name: "Ctrl+O",
    key: egui::Key::O,
    with_command: true,
};
const DUPLICATE: Shortcut = Shortcut {
    name: "Ctrl+D",
    key: egui::Key::D,
    with_command: true,
};
const SELECT_ALL: Shortcut = Shortcut {
    name: "Ctrl+A",
    key: egui::Key::A,
    with_command: true,
};
const FIT: Shortcut = Shortcut {
    name: "F",
    key: egui::Key::F,
    with_command: false,
};
const VALUES: Shortcut = Shortcut {
    name: "V",
    key: egui::Key::V,
    with_command: false,
};
const REMOVE: Shortcut = Shortcut {
    name: "Del",
    key: egui::Key::Delete,
    with_command: false,
};

/// A handle being dragged: which one, and where every picked piece was when it
/// was grabbed, so the whole travel applies to the frozen start.
struct Grab {
    handle: gizmo::Handle,
    /// Frozen world-space centre shown by the gizmo at the start of the drag.
    pivot: Vec3,
    base: Vec<(usize, Transform)>,
}

/// Keys pressed this frame, read once so the handlers stay a plain list.
struct Pressed {
    open: bool,
    open_project: bool,
    save_project: bool,
    save_project_as: bool,
    undo: bool,
    redo: bool,
    fit: bool,
    values: bool,
    duplicate: bool,
    everything: bool,
    nothing: bool,
    remove: bool,
    tool: Option<Tool>,
}

#[derive(Clone)]
pub struct ModelEntry {
    pub mesh: Arc<Mesh>,
    pub transform: Transform,
    /// Where the piece lands once `transform` is applied — the box the last
    /// `refresh_world` put into `world`, and what the panel, the gizmo and the
    /// camera all ask for. Kept because working it out means walking every
    /// vertex: cheap once, ruinous sixty times a second with a full plate.
    placed: std::cell::RefCell<Option<(Transform, (Vec3, Vec3))>>,
    /// The transform and size `world` was last built from for this piece.
    world_for: Option<(Transform, usize, usize)>,
}

impl ModelEntry {
    pub fn new(mesh: Mesh, transform: Transform) -> Self {
        Self::from_shared(Arc::new(mesh), transform)
    }

    pub fn from_shared(mesh: Arc<Mesh>, transform: Transform) -> Self {
        Self {
            mesh,
            transform,
            placed: Default::default(),
            world_for: None,
        }
    }

    /// The box the piece fills on the plate, worked out the same way whether
    /// the caller just moved it or has not touched it in ten frames.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        if let Some((for_transform, box_)) = *self.placed.borrow() {
            if for_transform == self.transform {
                return box_;
            }
        }
        let box_ = self.mesh.transformed(&self.transform).bounds();
        *self.placed.borrow_mut() = Some((self.transform, box_));
        box_
    }
}

/// Walking the layers like a flipbook: what the play button needs to know.
struct Playback {
    playing: bool,
    /// How long one layer stays up, in seconds.
    per_layer: f32,
    /// How much of `per_layer` is left before the next one.
    left: f32,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            playing: false,
            per_layer: 0.12,
            left: 0.0,
        }
    }
}

struct ExportState {
    dir: String,
    stem: String,
    /// `None` uses the printer's native format; alternatives are still limited
    /// to the formats declared compatible by that printer profile.
    format: Option<config::JobFormat>,
    also_masks: bool,
    rx: Option<Receiver<export::Msg>>,
    /// Set to stop the writer; shared with the worker thread.
    cancel: Option<Arc<AtomicBool>>,
    progress: f32,
    note: String,
    result: String,
}

/// What a drawn plate depends on. Two pictures with the same key are the same
/// picture, so the texture is kept instead of being made again.
#[derive(PartialEq)]
struct PictureKey {
    size: (f32, f32),
    pixels: [u32; 2],
    cam: Camera,
    mode: ViewMode,
    layer: usize,
    /// The pieces as they stand in world space.
    world: u64,
    /// The slice, which also stands for the supports in it.
    cut: u64,
    /// Which pieces are picked, since they are shaded differently.
    picked: u64,
    debug: bool,
}

struct Picture {
    key: PictureKey,
    tex: egui::TextureId,
}

/// A cheap answer to "are these the same pieces picked?".
fn picked_fingerprint(pieces: &[usize]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &p in pieces {
        h ^= p as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

pub struct App {
    cam: Camera,
    models: Vec<ModelEntry>,
    world: Vec<Mesh>,
    /// Which pieces are picked, and which one the cards and handles follow.
    selection: Selection,
    /// What a press and a drag mean in the view; the pointer's own state.
    input: input::Input,
    /// The handles and the pieces as they stood when one was taken hold of.
    grab: Option<Grab>,
    /// The box the pointer is pulling out, while it is being pulled.
    marquee: Option<egui::Rect>,
    printers: Vec<PrinterProfile>,
    printer_idx: usize,
    settings: PrintSettings,
    /// The structured supports: tips, branches, trunks, feet, braces, plates.
    support: SupportModel,
    /// The drawn shape of those supports.
    support_mesh: Mesh,
    /// Generation running on a worker thread, if any.
    support_job: Option<supports::Job>,
    /// What the last run reported, shown under the panel.
    support_note: String,
    /// Red overhang faces and island markers in the view.
    show_debug: bool,
    /// Triangle index over the models, for clicking on them.
    pick: Option<Bvh>,
    /// One tree per piece, so a click names the piece it hit even when pieces
    /// overlap in the view.
    pick_parts: Option<Vec<Bvh>>,
    slice: Option<SliceResult>,
    slice_gen: u64,
    mode: ViewMode,
    tab: Tab,
    tool: Tool,
    /// Whether the floating numbers window is shown.
    show_values: bool,
    /// On-demand alignment and distribution tools.
    show_align: bool,
    layout_gap: f32,
    /// Where the plate view was last laid out, so a floating card can sit on it
    /// rather than on a guessed offset from the window edge.
    view_rect: Option<egui::Rect>,
    current_layer: usize,
    status: String,
    /// The desktop's file chooser; answers arrive on a background thread.
    picker: dialog::Picker,
    export: ExportState,
    /// Shared after-state history and project file I/O.
    history: Option<project::History>,
    project_job: Option<Receiver<Result<project::IoResult, String>>>,
    project_open_generation: Option<u64>,
    change_serial: u64,
    project_path: Option<PathBuf>,
    /// The picture the plate view was last drawn as, and what it was drawn
    /// from. A heavy plate takes tens of milliseconds to rasterise, and the
    /// camera only moves when the pointer says so: everything else on top of
    /// the picture is drawn per frame and costs nothing.
    picture: Option<Picture>,
    gpu: Option<gpu_render::GpuRenderer>,
    logo: Option<(u32, egui::TextureHandle)>,
    /// Bumped whenever the pieces were rebuilt in world space.
    world_gen: u64,
    layer_tex: Option<egui::TextureHandle>,
    layer_tex_key: Option<(u64, usize, usize)>,
    /// Playback of the layer picture, in the slice step.
    playback: Playback,
    /// Whether a turn dragged with a ring lands on whole steps, and how big.
    snap: bool,
    snap_deg: f32,
    show_help: bool,
    /// Bumped by every change to what is on the plate.
    revision: u64,
    /// The revision the supports on screen were grown for; `None` until a run
    /// has come back. Empty supports belong to every plate, so clearing them
    /// sets this to the current revision.
    supports_for: Option<u64>,
    /// The revision of the plate the slice on screen describes. A slice covers
    /// the supports as well as the models, so it is only as fresh as the
    /// supports it was made with.
    slice_for: Option<u64>,
    /// The revision a worker run was asked for, while it is still working.
    pending: Option<u64>,
    screenshot: Option<PathBuf>,
    frame_no: u64,
    shot_sent: bool,
}

/// What the command line asked for: the window's opening state in one place,
/// so the sequence below is not a row of loose booleans.
#[derive(Default, Clone)]
pub struct Launch {
    /// Draw a few frames, save a PNG and exit.
    pub screenshot: Option<PathBuf>,
    /// Files and folders to open on the plate.
    pub open: Vec<PathBuf>,
    /// Start in the sliced-layer view.
    pub preview: bool,
    /// Start with the sample scene instead of an empty plate.
    pub demo: bool,
}

impl App {
    pub fn new(launch: Launch) -> Self {
        let Launch {
            screenshot,
            open,
            preview,
            demo,
        } = launch;
        // A picture must not be taken of a half-grown plate.
        let wait_for_runs = screenshot.is_some();
        let mut app = Self {
            cam: Camera::default(),
            models: Vec::new(),
            world: Vec::new(),
            selection: Selection::none(),
            input: input::Input::default(),
            grab: None,
            marquee: None,
            printers: config::default_printers(),
            printer_idx: 0,
            settings: PrintSettings::default(),
            support: SupportModel::default(),
            support_mesh: Mesh::default(),
            support_job: None,
            support_note: String::new(),
            show_debug: false,
            pick: None,
            pick_parts: None,
            slice: None,
            slice_gen: 0,
            mode: ViewMode::Solid,
            tab: Tab::Prepare,
            tool: Tool::View,
            show_values: true,
            show_align: false,
            layout_gap: 6.0,
            view_rect: None,
            current_layer: 0,
            status: "Right-drag to orbit · middle-drag to pan · scroll to zoom".into(),
            picker: dialog::Picker::new(),
            export: ExportState {
                dir: default_export_dir(),
                stem: "job".into(),
                format: None,
                also_masks: true,
                rx: None,
                cancel: None,
                progress: 0.0,
                note: String::new(),
                result: String::new(),
            },
            history: None,
            project_job: None,
            project_open_generation: None,
            change_serial: 0,
            project_path: None,
            picture: None,
            gpu: None,
            logo: None,
            world_gen: 0,
            layer_tex: None,
            layer_tex_key: None,
            playback: Playback::default(),
            snap: true,
            snap_deg: 5.0,
            show_help: false,
            revision: 1,
            supports_for: None,
            slice_for: None,
            pending: None,
            screenshot,
            frame_no: 0,
            shot_sent: false,
        };

        // a folder on the command line means "everything printable in here"
        let mut wanted: Vec<PathBuf> = Vec::new();
        for path in &open {
            if path.is_dir() {
                let mut found = folder_models(path);
                found.sort();
                wanted.append(&mut found);
            } else {
                wanted.push(path.clone());
            }
        }
        for path in &wanted {
            app.load_path(path);
        }
        if demo {
            app.add_sample_scene();
        }
        // A window that is only going to be photographed has to show a finished
        // plate, so those two paths take the run up front. Every other start
        // opens on the bare plate: supports are grown by pressing Auto all, and
        // never because a file was opened.
        if wait_for_runs {
            app.start_supports_and_wait();
        } else if preview {
            // Preview wants the same finished plate, but a window that hangs
            // for seconds on a heavy file is worse than one that fills in.
            app.start_supports();
        } else {
            app.slice_now();
        }
        app.fit_view();
        if preview {
            app.mode = ViewMode::Sliced;
            app.tab = Tab::Slice;
            let n = app.slice.as_ref().map(|s| s.layers.len()).unwrap_or(0);
            app.current_layer = (n / 3).min(n.saturating_sub(1));
        }
        app.history = Some(project::History::new(app.project()));
        app
    }

    /// Open on a particular tab (from `--tab`).
    pub fn start_on_tab(&mut self, name: &str) {
        if let Some(tab) = Tab::from_name(name) {
            self.open_step(tab);
        }
    }

    /// Start with a tool already chosen, for `--tool`.
    pub fn start_with_tool(&mut self, name: &str) {
        if let Some(tool) = Tool::from_name(name) {
            self.tool = tool;
        }
    }

    pub fn add_sample_scene(&mut self) {
        // Two sample pieces so a fresh window has something to look at.
        let mut pawn = mesh::sample_pawn();
        pawn.translate(Vec3::new(-30.0, -6.0, 0.0));
        let mut block = mesh::sample_block();
        block.translate(Vec3::new(26.0, 16.0, 0.0));
        self.models.push(ModelEntry::new(
            pawn,
            Transform {
                scale: Vec3::new(2.5, 2.5, 2.5),
                ..Default::default()
            },
        ));
        self.models.push(ModelEntry::new(
            block,
            Transform {
                scale: Vec3::new(2.0, 2.0, 2.0),
                ..Default::default()
            },
        ));
        self.selection = Selection::single(0);
        self.refresh_world();
    }

    pub fn apply_theme(ctx: &egui::Context) {
        let mut style = (*ctx.style()).clone();
        let mut v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = egui::Color32::from_rgb(30, 33, 39);
        v.extreme_bg_color = egui::Color32::from_rgb(17, 19, 23);
        v.faint_bg_color = egui::Color32::from_rgb(36, 39, 46);
        v.window_rounding = egui::Rounding::same(12.0);
        v.menu_rounding = egui::Rounding::same(10.0);
        v.window_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(52, 57, 66));
        v.widgets.noninteractive.bg_fill = PILL;
        v.widgets.noninteractive.rounding = egui::Rounding::same(8.0);
        v.widgets.inactive.bg_fill = PILL;
        v.widgets.inactive.weak_bg_fill = PILL;
        v.widgets.inactive.rounding = egui::Rounding::same(8.0);
        v.widgets.hovered.bg_fill = HOVER;
        v.widgets.hovered.weak_bg_fill = HOVER;
        v.widgets.hovered.rounding = egui::Rounding::same(8.0);
        v.widgets.active.bg_fill = ACCENT_BG;
        v.widgets.active.rounding = egui::Rounding::same(8.0);
        v.selection.bg_fill = ACCENT_BG;
        v.selection.stroke = egui::Stroke::new(1.0_f32, ACCENT);
        style.visuals = v;
        style.spacing.item_spacing = egui::vec2(8.0, 7.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.spacing.slider_width = 132.0;
        style.spacing.interact_size.y = 24.0;
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(14.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(13.5, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(11.5, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(18.0, egui::FontFamily::Proportional),
        );
        ctx.set_style(style);
    }

    // ------------------------------------------------------------ model work

    /// A click in the 3D view picks a piece. Picking works with every tool, so
    /// a piece can be chosen before saying what to do with it. Supports are
    /// never placed by hand: they come from one run of the generator.
    fn click_in_view(&mut self, pos: egui::Pos2, w: f32, h: f32, additive: bool) {
        let view = render::View::new(&self.cam, w, h);
        let (from, dir) = view.ray(pos.x, pos.y);
        if self.pick.is_none() {
            self.pick = Some(Bvh::from_meshes(&self.world));
        }
        let hit = match self.pick.as_ref() {
            Some(tree) => tree.ray_hit(from, dir, 1.0e5),
            None => return,
        };
        let piece = hit.and_then(|(t, _)| self.piece_at(from, dir, t));
        self.selection.click(piece, additive);
    }

    fn refresh_world(&mut self) {
        // Only the pieces that changed are rebuilt. A plate of a dozen pieces
        // is a million triangles, and a drag asks for this sixty times a
        // second: re-transforming all of it is what made moving crawl.
        self.world.resize_with(self.models.len(), Mesh::default);
        for (i, entry) in self.models.iter_mut().enumerate() {
            let key = (
                entry.transform,
                entry.mesh.verts.len(),
                entry.mesh.tris.len(),
            );
            if entry.world_for != Some(key) {
                self.world[i] = entry.mesh.transformed(&entry.transform);
                entry.world_for = Some(key);
            }
        }
        self.world_gen = self.world_gen.wrapping_add(1);
        self.pick = None;
        self.pick_parts = None;
        self.selection.prune(self.models.len());
    }

    /// Which piece owns the world point `p`, if any.
    /// The piece whose surface the ray reached at `t`.
    ///
    /// A click always lands *on* a surface, so asking "is this point inside the
    /// solid" cannot answer it — three rays from a point exactly on a triangle
    /// give whatever parity the tolerance happens to produce, which is why
    /// clicks used to pick nothing. Asking each piece's own tree for its
    /// nearest hit is exact: the piece whose surface is the nearest one is the
    /// piece that was clicked.
    fn piece_at(&mut self, from: Vec3, dir: Vec3, t: f32) -> Option<usize> {
        if self.pick_parts.is_none() {
            self.pick_parts = Some(
                self.world
                    .iter()
                    .map(|m| Bvh::from_meshes(std::slice::from_ref(m)))
                    .collect(),
            );
        }
        self.pick_parts.as_ref()?.iter().position(|tree| {
            tree.ray_hit(from, dir, 1.0e5)
                .is_some_and(|(hit, _)| (hit - t).abs() < 0.05)
        })
    }

    /// The handles for the picked pieces, when the current tool has them.
    /// They sit on the middle of everything that is picked. Turning spins each
    /// piece about its own centre; scaling still expands about this shared anchor.
    fn gizmo_for(&self) -> Option<gizmo::Gizmo> {
        let mode = gizmo::Mode::of_tool(self.tool)?;
        // The handles sit on the average of the picked pieces' centres. This is
        // a convenient place to grab the group, not a pivot for group turning.
        let picked: Vec<Vec3> = self
            .selection
            .pieces()
            .iter()
            .filter_map(|&i| self.models.get(i))
            .map(|entry| {
                entry
                    .transform
                    .apply(layout::bounds_centre(entry.mesh.bounds()))
            })
            .collect();
        let n = picked.len() as f32;
        let anchor = picked.iter().fold(Vec3::ZERO, |sum, p| sum + *p) * (1.0 / n.max(1.0));
        (!picked.is_empty()).then(|| gizmo::Gizmo::new(mode, anchor))
    }

    /// Cancel a gesture before a command changes the selection, model indices,
    /// tool or camera out from under its frozen grab state.
    fn cancel_gesture(&mut self) {
        let changed = self.grab.take().is_some();
        self.input = input::Input::default();
        self.marquee = None;
        if changed {
            self.touched();
        }
    }

    /// Take hold of a handle: remember where everything is now, so the drag can
    /// be applied from here rather than frame by frame.
    fn grab_handles(&mut self, handle: gizmo::Handle) {
        let base: Vec<(usize, Transform)> = self
            .selection
            .pieces()
            .iter()
            .filter_map(|&i| self.models.get(i).map(|m| (i, m.transform)))
            .collect();
        let pivot = base.iter().fold(Vec3::ZERO, |sum, (idx, transform)| {
            let Some(model) = self.models.get(*idx) else {
                return sum;
            };
            sum + transform.apply(layout::bounds_centre(model.mesh.bounds()))
        }) * (1.0 / base.len().max(1) as f32);
        self.grab = Some(Grab {
            handle,
            pivot,
            base,
        });
    }

    /// Apply the whole drag, measured from where the handle was grabbed. The
    /// gizmo has already turned pixels into millimetres, degrees or a factor.
    fn drag_handles(&mut self, change: gizmo::Change) {
        let Some(grab) = &self.grab else { return };
        let axis = grab.handle.axis();
        // A snap makes the ring land on round numbers while it is dragged;
        // typed numbers are exact and never rounded.
        let snap = (self.snap, self.snap_deg.max(0.1));
        for &(idx, base) in &grab.base {
            let Some(entry) = self.models.get_mut(idx) else {
                continue;
            };
            let mut t = base;
            let centre = layout::bounds_centre(entry.mesh.bounds());
            let base_centre = base.apply(centre);
            match change {
                gizmo::Change::Move(d) => {
                    if !(d.x.is_finite() && d.y.is_finite() && d.z.is_finite()) {
                        continue;
                    }
                    t.pos = base.pos + d;
                }
                gizmo::Change::Turn(deg) => {
                    let deg = snap_angle(deg, snap.0, snap.1);
                    t.turn_about(axis, deg, centre);
                }
                gizmo::Change::Size(f) => {
                    t.scale_about(f, centre);
                    let factor = Vec3::new(
                        t.scale.x / base.scale.x,
                        t.scale.y / base.scale.y,
                        t.scale.z / base.scale.z,
                    );
                    let offset = base_centre - grab.pivot;
                    let scaled = grab.pivot
                        + Vec3::new(
                            offset.x * factor.x,
                            offset.y * factor.y,
                            offset.z * factor.z,
                        );
                    t.pos = t.pos + (scaled - base_centre);
                }
            }
            entry.transform = t;
        }
        // the picture has to follow the hand, not wait for the support rebuild
        self.refresh_world();
    }

    /// Add the same rotation to every picked piece, holding each piece's own
    /// centre still. Numeric edits use the primary piece's fields as the delta
    /// reference, just like a gizmo drag.
    fn turn_selected_by(&mut self, delta: Vec3) {
        if ![delta.x, delta.y, delta.z].iter().all(|v| v.is_finite()) {
            return;
        }
        for idx in self.selection.pieces().to_vec() {
            let Some(entry) = self.models.get_mut(idx) else {
                continue;
            };
            let centre = layout::bounds_centre(entry.mesh.bounds());
            for (axis, degrees) in [delta.x, delta.y, delta.z].into_iter().enumerate() {
                if degrees != 0.0 {
                    entry.transform.turn_about(axis, degrees, centre);
                }
            }
        }
    }

    /// Everything whose picture falls inside the box the pointer pulled out.
    fn select_in(&mut self, area: egui::Rect, view: &render::View, additive: bool) {
        let mut hits = Vec::new();
        for (i, m) in self.models.iter().enumerate() {
            let Some(b) = view.screen_bounds(&m.mesh, &m.transform) else {
                continue;
            };
            if area.intersects(b) {
                hits.push(i);
            }
        }
        self.selection.covered(&hits, additive);
        self.status = if hits.is_empty() {
            "Nothing in the box.".into()
        } else {
            format!("{} picked.", self.selection.len())
        };
    }

    /// Put copies of the picked pieces beside them, with room for their supports.
    fn duplicate_selected(&mut self) {
        self.cancel_gesture();
        let picked: Vec<usize> = self
            .selection
            .pieces()
            .iter()
            .copied()
            .filter(|&i| i < self.models.len())
            .collect();
        if picked.is_empty() {
            return;
        }
        let build = self.printers[self.printer_idx].build;
        let mut copies = Vec::new();
        for idx in picked {
            let entry = &self.models[idx];
            let (min, max) = entry.bounds();
            let size = max - min;
            let gap = 6.0;
            let origin = entry.transform.pos;
            let mut pos = origin + Vec3::new(size.x + gap, 0.0, 0.0);
            // step back and down a row when the copy would leave the plate
            if pos.x + size.x * 0.5 > build[0] * 0.5 {
                pos = origin + Vec3::new(0.0, size.y + gap, 0.0);
            }
            let mut copy = entry.clone();
            copy.transform.pos = pos;
            let name = format!("{} copy", copy.mesh.name);
            Arc::make_mut(&mut copy.mesh).name = name;
            copies.push(copy);
        }
        let count = copies.len();
        let first = self.models.len();
        self.models.extend(copies);
        self.selection.only(first..first + count);
        self.refresh_world();
        self.touched();
        self.status = format!("Copied {count} piece(s).");
    }

    /// Flip the picked pieces across one of their own axes, about each piece's
    /// own centre so they do not slide across the plate.
    fn mirror_selected(&mut self, axis: usize) {
        self.cancel_gesture();
        let picked = self.selection.pieces().to_vec();
        if picked.is_empty() {
            return;
        }
        for idx in picked {
            let Some(entry) = self.models.get_mut(idx) else {
                continue;
            };
            let centre = layout::bounds_centre(entry.mesh.bounds());
            entry.transform.mirror_about(axis, centre);
        }
        self.refresh_world();
        self.touched();
        self.status = format!("Mirrored across {}.", ["X", "Y", "Z"][axis]);
    }

    fn delete_selected(&mut self) {
        self.cancel_gesture();
        if self.selection.is_empty() {
            return;
        }
        let doomed: std::collections::BTreeSet<usize> =
            self.selection.pieces().iter().copied().collect();
        let mut map = vec![None; self.models.len()];
        let mut kept = Vec::with_capacity(self.models.len());
        for (i, m) in std::mem::take(&mut self.models).into_iter().enumerate() {
            if doomed.contains(&i) {
                continue;
            }
            map[i] = Some(kept.len());
            kept.push(m);
        }
        self.models = kept;
        self.selection.remap(&map);
        self.refresh_world();
        self.touched();
        self.status = format!("Removed {} piece(s).", doomed.len());
    }

    /// Keep every piece at the height the panel asks for: the lowest point of
    /// the whole plate sits exactly `height from platform` above z = 0.
    /// Moving the pieces here (rather than an invisible copy) is what makes
    /// the slider do something you can see.
    /// Hold every piece at the gap the supports start from. A piece that
    /// already floats higher is left alone: it was put there on purpose.
    fn apply_gap(&mut self) -> bool {
        self.cancel_gesture();
        if !self.settings.supports.enabled || self.models.is_empty() {
            return false;
        }
        let gap = self.settings.supports.height_from_platform;
        let mut lifted = false;
        for m in self.models.iter_mut() {
            let (min, _) = m.bounds();
            let dz = supports::gap_rise(min.z, gap);
            if dz > 1e-4 {
                m.transform.pos.z += dz;
                lifted = true;
            }
        }
        lifted
    }

    /// Capture every user-owned project input. Arc keeps the large mesh arrays
    /// shared between the live plate, history and a background save.
    fn project(&self) -> project::Project {
        project::Project {
            printers: self.printers.clone(),
            printer_idx: self.printer_idx,
            settings: self.settings.clone(),
            models: self
                .models
                .iter()
                .map(|entry| project::ProjectModel {
                    mesh: entry.mesh.clone(),
                    transform: entry.transform,
                })
                .collect(),
            selection: self.selection.pieces().to_vec(),
            export_dir: self.export.dir.clone(),
            export_stem: self.export.stem.clone(),
            export_format: self.export.format,
            export_masks: self.export.also_masks,
            layout_gap: self.layout_gap,
            snap: self.snap,
            snap_deg: self.snap_deg,
        }
    }

    /// Restore project inputs without recording a new edit. Derived support
    /// geometry is preserved for undo and marked stale; a file open starts
    /// from the saved source models and a fresh bare-plate slice.
    fn restore_project(&mut self, saved: project::Project, opening: bool) {
        self.cancel_gesture();
        if let Some(cancel) = self.export.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.export.rx = None;
        let _ = self.support_job.take();
        self.pending = None;

        self.printers = saved.printers;
        self.printer_idx = saved.printer_idx.min(self.printers.len().saturating_sub(1));
        self.settings = saved.settings;
        self.models = saved
            .models
            .into_iter()
            .map(|model| ModelEntry::from_shared(model.mesh, model.transform))
            .collect();
        self.selection = Selection::none();
        self.selection.only(
            saved
                .selection
                .into_iter()
                .filter(|&i| i < self.models.len()),
        );
        self.export.dir = saved.export_dir;
        self.export.stem = saved.export_stem;
        self.export.format = saved.export_format;
        self.export.also_masks = saved.export_masks;
        self.export.progress = 0.0;
        self.export.note.clear();
        self.export.result.clear();
        self.layout_gap = saved.layout_gap;
        self.snap = saved.snap;
        self.snap_deg = saved.snap_deg;
        self.playback.playing = false;
        self.current_layer = 0;

        if opening {
            self.support.clear();
            self.support_mesh = Mesh::default();
            self.supports_for = None;
            self.support_note =
                "Supports are rebuilt from the saved settings with Auto all.".into();
        }
        self.refresh_world();
        self.revision = self.revision.wrapping_add(1);
        self.change_serial = self.change_serial.wrapping_add(1);
        self.slice_now();
        self.picture = None;
        self.layer_tex = None;
        self.layer_tex_key = None;
        if opening {
            self.fit_view();
        }
    }

    fn flush_history(&mut self) {
        let current = self.project();
        if let Some(history) = &mut self.history {
            history.flush(current);
        }
    }

    fn history_tick(&mut self) {
        let current = self.project();
        if let Some(history) = &mut self.history {
            history.tick(&current);
        }
    }

    fn can_undo(&self) -> bool {
        self.grab.is_some()
            || self
                .history
                .as_ref()
                .is_some_and(project::History::can_undo)
    }

    fn can_redo(&self) -> bool {
        self.history
            .as_ref()
            .is_some_and(project::History::can_redo)
    }

    fn undo(&mut self) {
        self.cancel_gesture();
        let current = self.project();
        let restored = self
            .history
            .as_mut()
            .and_then(|history| history.undo(&current));
        if let Some(saved) = restored {
            self.restore_project(saved, false);
            self.status = "Undid the last project change.".into();
        }
    }

    fn redo(&mut self) {
        self.cancel_gesture();
        let current = self.project();
        let restored = self
            .history
            .as_mut()
            .and_then(|history| history.redo(&current));
        if let Some(saved) = restored {
            self.restore_project(saved, false);
            self.status = "Redid the project change.".into();
        }
    }

    fn save_project(&mut self) {
        if let Some(path) = self.project_path.clone() {
            self.start_project_save(path);
        } else {
            self.picker.save_project(None);
        }
    }

    fn save_project_as(&mut self) {
        self.picker.save_project(self.project_path.clone());
    }

    fn open_project(&mut self) {
        self.picker.open_project();
    }

    fn start_project_save(&mut self, mut path: PathBuf) {
        self.cancel_gesture();
        if self.project_job.is_some() {
            self.status = "Wait for the current project operation to finish.".into();
            return;
        }
        let conventional = path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case(dialog::PROJECT_EXTENSION));
        if !conventional {
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("project");
            path.set_file_name(format!("{name}.{}", dialog::PROJECT_EXTENSION));
        }
        self.flush_history();
        let snapshot = self.project();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = snapshot.save(&path).map(|_| project::IoResult::Saved(path));
            let _ = tx.send(result);
        });
        self.project_job = Some(rx);
        self.project_open_generation = None;
        self.status = "Saving project…".into();
    }

    fn start_project_open(&mut self, path: PathBuf) {
        self.cancel_gesture();
        if self.project_job.is_some() {
            self.status = "Wait for the current project operation to finish.".into();
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = project::Project::read(&path)
                .map(|project| project::IoResult::Loaded(path, Box::new(project)));
            let _ = tx.send(result);
        });
        let generation = self.change_serial;
        self.project_job = Some(rx);
        self.project_open_generation = Some(generation);
        self.status = "Opening project…".into();
    }

    fn poll_project(&mut self) {
        let Some(result) = ({
            let rx = self.project_job.as_ref();
            match rx {
                Some(rx) => match rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some(Err("the project worker stopped unexpectedly".into()))
                    }
                },
                None => None,
            }
        }) else {
            return;
        };
        self.project_job = None;
        let changed_while_opening = self.grab.is_some()
            || self
                .project_open_generation
                .is_some_and(|generation| generation != self.change_serial);
        self.project_open_generation = None;
        match result {
            Ok(project::IoResult::Saved(path)) => {
                self.project_path = Some(path.clone());
                self.status = format!("Project saved to {}.", path.display());
            }
            Ok(project::IoResult::Loaded(path, saved)) => {
                if changed_while_opening {
                    self.status = format!(
                        "{} was not opened because the project changed while loading. Your edits were kept; open it again when ready.",
                        path.display()
                    );
                    return;
                }
                self.restore_project(*saved, true);
                self.project_path = Some(path.clone());
                self.history = Some(project::History::new(self.project()));
                self.status = format!("Project opened from {}.", path.display());
            }
            Err(error) => self.status = format!("Project error: {error}"),
        }
    }

    /// Note that the plate changed. Nothing is rebuilt here: what is on screen
    /// was made from an older plate until the person asks for a run, which is
    /// what keeps moving a dozen pieces from re-growing supports and
    /// re-slicing the scene on every frame.
    fn touched(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.record_change();
    }

    fn record_change(&mut self) {
        self.change_serial = self.change_serial.wrapping_add(1);
        if let Some(history) = &mut self.history {
            history.mark_changed();
        }
    }

    /// Are the supports older than the plate? Supports that are not there
    /// cannot be out of date: an empty set fits any plate, so it is current by
    /// definition and only a run can make it wrong.
    fn supports_stale(&self) -> bool {
        !self.support.is_empty() && self.supports_for != Some(self.revision)
    }

    /// Is the slice older than the plate?
    fn slice_stale(&self) -> bool {
        self.slice_for != Some(self.revision)
    }

    /// Is anything on screen older than the plate? This is what the window
    /// uses to stop a print being exported from a plate that has moved on.
    fn out_of_date(&self) -> bool {
        self.supports_stale() || self.slice_stale()
    }

    /// Grow the supports for everything on the plate, then slice the result on
    /// a worker thread. This is the only way supports are ever made: the Auto
    /// all button, and the two starts that open on a finished plate.
    fn start_supports(&mut self) {
        self.run_supports(false);
    }

    /// The same run, but finished before the first frame is drawn — a picture
    /// of a plate that is still growing would be a picture of nothing.
    fn start_supports_and_wait(&mut self) {
        self.run_supports(true);
    }

    fn run_supports(&mut self, wait: bool) {
        if self.apply_gap() {
            self.touched();
        }
        self.refresh_world();
        self.pick = None;
        // A new request supersedes any worker still using older geometry or
        // settings. Dropping the job sets its cancellation flag; otherwise an
        // old result could arrive later and overwrite the new support layout.
        let _ = self.support_job.take();
        let at = self.revision;
        if self.settings.supports.enabled {
            let cfg = self.settings.supports.clone();
            // The pieces are already at the requested height, so the generator
            // sees exactly what the window shows.
            let meshes = self.world.clone();
            if wait {
                let cancel = std::sync::atomic::AtomicBool::new(false);
                self.support = supports::generate(&meshes, &cfg, &cancel);
                self.support_mesh = self.support.mesh(12);
                self.support_note = self.support.report();
                self.supports_for = Some(at);
                self.slice_now();
            } else {
                self.support.clear();
                self.support_mesh = Mesh::default();
                self.support_note = "Growing supports…".into();
                self.support_job = Some(supports::start(meshes, cfg));
                // Remember which plate the run was asked for: if a piece moves
                // while it works, the answer it brings back is already stale.
                self.pending = Some(at);
            }
        } else {
            // Nothing is grown, so nothing can be stale.
            self.support.clear();
            self.support_mesh = Mesh::default();
            self.support_note.clear();
            self.supports_for = Some(at);
            self.slice_now();
        }
    }

    /// Slice what is on the plate now, leaving the supports as they are.
    fn slice_now(&mut self) {
        self.reslice();
        self.slice_for = if self.support.is_empty() {
            Some(self.revision)
        } else {
            self.supports_for
        };
    }

    /// Throw the grown supports away and slice the bare plate.
    fn clear_supports(&mut self) {
        self.support.clear();
        self.support_mesh = Mesh::default();
        self.support_note = "Supports deleted.".into();
        // Empty supports fit every plate, so there is nothing to mark stale.
        self.supports_for = None;
        self.slice_now();
    }

    /// Slice the models together with the supports. They are separate meshes,
    /// so overlaps are joined by the non-zero fill rather than by cutting
    /// anything.
    fn reslice(&mut self) {
        let mut all = self.world.clone();
        if !self.support_mesh.tris.is_empty() {
            all.push(self.support_mesh.clone());
        }
        self.slice = Some(slicer::slice(&all, self.settings.layer_height));
        self.slice_gen += 1;
        let n = self.slice.as_ref().map(|s| s.layers.len()).unwrap_or(0);
        if self.current_layer >= n {
            self.current_layer = n.saturating_sub(1);
        }
        self.layer_tex = None;
        self.layer_tex_key = None;
    }

    /// Pick up a finished generation run.
    fn poll_supports(&mut self, ctx: &egui::Context) {
        let mut finished = false;
        if let Some(job) = &self.support_job {
            while let Ok(msg) = job.rx.try_recv() {
                match msg {
                    supports::JobMsg::Done(model) => {
                        self.support_note = model.report();
                        self.support = *model;
                        self.support_mesh = self.support.mesh(12);
                        finished = true;
                    }
                    supports::JobMsg::Failed(msg) => {
                        self.support_note = format!("support generation stopped: {msg}");
                        finished = true;
                    }
                }
            }
            if !finished {
                ctx.request_repaint_after(std::time::Duration::from_millis(60));
            }
        }
        if finished {
            self.support_job = None;
            // The run answered for a particular plate; if that has changed
            // since, the window says so instead of pretending otherwise.
            self.supports_for = self.pending.take();
            self.slice_now();
        }
    }

    /// Load whatever the system chooser answered.
    fn poll_picker(&mut self) {
        let Some(answer) = self.picker.poll() else {
            return;
        };
        match answer.kind {
            dialog::PickKind::Models => {
                for path in answer.paths {
                    self.load_path(&path);
                }
            }
            dialog::PickKind::OpenProject => {
                if let Some(path) = answer.paths.first() {
                    self.start_project_open(path.clone());
                }
            }
            dialog::PickKind::SaveProject => {
                if let Some(path) = answer.paths.first() {
                    self.start_project_save(path.clone());
                }
            }
        }
    }

    fn load_path(&mut self, path: &Path) {
        self.cancel_gesture();
        if self.project_job.is_some() {
            self.status = "Wait for the current project operation to finish.".into();
            return;
        }
        let project_extension = path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case(dialog::PROJECT_EXTENSION));
        if project_extension || project::is_project(path) {
            match project::Project::read(path) {
                Ok(saved) => {
                    self.restore_project(saved, true);
                    self.project_path = Some(path.to_path_buf());
                    self.history = Some(project::History::new(self.project()));
                    self.status = format!("Project opened from {}.", path.display());
                }
                Err(error) => self.status = error,
            }
            return;
        }
        match mesh::load_model(path) {
            Ok(m) => {
                let name = m.name.clone();
                let tris = m.tris.len();
                let idx = self.models.len();
                self.models.push(ModelEntry::new(m, Transform::default()));
                self.selection = Selection::single(idx);
                self.status = format!(
                    "Loaded {name} ({tris} triangles) — {} on the plate.",
                    self.models.len()
                );
                self.refresh_world();
                self.touched();
                self.arrange_all(false);
                // A new piece arrives floating wherever its own file put it;
                // it lands on the plate like the others before it is seen.
                self.drop_pieces(&[idx]);
                self.fit_view();
            }
            Err(e) => self.status = e,
        }
    }

    fn fit_view(&mut self) {
        self.cancel_gesture();
        let b = self.printers[self.printer_idx].build;
        let mut top = 0.0f32;
        for m in &self.world {
            let (_, max) = m.bounds();
            top = top.max(max.z);
        }
        let plate_small = b[0].min(b[1]);
        let mut model_span = 0.0f32;
        for m in &self.world {
            let (min, max) = m.bounds();
            model_span = model_span.max((max - min).length());
        }
        let size = plate_small.max(model_span * 1.5);
        self.cam.dist = (size * 1.45).max(60.0);
        self.cam.target = Vec3::new(0.0, 0.0, (top * 0.5).max(4.0));
    }

    /// How high a model's lowest point rests: on the plate, or above it when
    /// supports are on and "height from platform" asks for a gap.
    /// Spread every piece out over the plate, in a tidy grid.
    fn arrange_all(&mut self, announce: bool) {
        self.cancel_gesture();
        if self.models.is_empty() {
            return;
        }
        let bounds: Vec<(Vec3, Vec3)> = self.models.iter().map(|m| m.bounds()).collect();
        let build = self.printers[self.printer_idx].build;
        let slots = layout::arrange(&bounds, build, 6.0);
        let mut moved = false;
        for slot in slots {
            if let Some(m) = self.models.get_mut(slot.index) {
                // the slot puts the piece on the plate; the gap is the support
                // run's business, not the layout's
                if slot.delta.length() > 1e-6 {
                    m.transform.pos = m.transform.pos + slot.delta;
                    moved = true;
                }
            }
        }
        if moved {
            self.refresh_world();
            self.touched();
        }
        if announce {
            self.status = if moved {
                format!("{} pieces arranged on the plate.", self.models.len())
            } else {
                "The plate is already arranged.".into()
            };
        }
    }

    fn centre_selected(&mut self) {
        self.cancel_gesture();
        let Some(idx) = self.selection.primary() else {
            return;
        };
        if let Some(m) = self.models.get_mut(idx) {
            let b = m.bounds();
            let delta = layout::plate_offset(b);
            if delta.length() < 1e-6 {
                self.status = "The selected piece is already centred.".into();
                return;
            }
            m.transform.pos = m.transform.pos + delta;
            self.refresh_world();
            self.touched();
        }
    }

    fn apply_layout_deltas(&mut self, picked: &[usize], deltas: &[Vec3], status: String) {
        if deltas.iter().all(|d| d.length() < 1e-6) {
            self.status = "The selected pieces are already in that arrangement.".into();
            return;
        }
        for (&idx, &delta) in picked.iter().zip(deltas) {
            if let Some(model) = self.models.get_mut(idx) {
                model.transform.pos = model.transform.pos + delta;
            }
        }
        self.refresh_world();
        self.touched();
        self.status = status;
    }

    fn align_picked(&mut self, axis: usize, mode: layout::Alignment) {
        self.cancel_gesture();
        let picked: Vec<usize> = self
            .selection
            .pieces()
            .iter()
            .copied()
            .filter(|&idx| idx < self.models.len())
            .collect();
        if picked.len() < 2 {
            return;
        }
        let bounds: Vec<_> = picked
            .iter()
            .map(|&idx| self.models[idx].bounds())
            .collect();
        let deltas = layout::align(&bounds, axis, mode);
        self.apply_layout_deltas(
            &picked,
            &deltas,
            format!(
                "Aligned {} pieces to {} {:?}.",
                picked.len(),
                ["X", "Y", "Z"][axis],
                mode
            ),
        );
    }

    fn distribute_picked(&mut self, axis: usize, equal_gaps: bool) {
        self.cancel_gesture();
        let picked: Vec<usize> = self
            .selection
            .pieces()
            .iter()
            .copied()
            .filter(|&idx| idx < self.models.len())
            .collect();
        if picked.len() < 2 {
            return;
        }
        let bounds: Vec<_> = picked
            .iter()
            .map(|&idx| self.models[idx].bounds())
            .collect();
        let (deltas, description) = if equal_gaps {
            (
                layout::even_gaps(&bounds, axis, self.layout_gap),
                format!("{:.1} mm gaps", self.layout_gap),
            )
        } else {
            (
                layout::distribute_centres(&bounds, axis),
                "even centres".to_string(),
            )
        };
        self.apply_layout_deltas(
            &picked,
            &deltas,
            format!(
                "Spaced {} pieces along {}: {description}.",
                picked.len(),
                ["X", "Y"][axis]
            ),
        );
    }

    /// Which pieces an action speaks for: the picked ones, or the whole plate
    /// when nothing is picked.
    fn targets(&self) -> Vec<usize> {
        if self.selection.is_empty() {
            (0..self.models.len()).collect()
        } else {
            self.selection.pieces().to_vec()
        }
    }

    /// Lower these pieces until each one's lowest point sits on the plate.
    fn drop_pieces(&mut self, which: &[usize]) {
        let mut moved = false;
        for &idx in which {
            let Some(m) = self.models.get_mut(idx) else {
                continue;
            };
            let (min, _) = m.bounds();
            if min.z.abs() > 1e-4 {
                m.transform.pos.z -= min.z;
                moved = true;
            }
        }
        if moved {
            self.refresh_world();
            self.touched();
        }
    }

    fn drop_to_plate(&mut self) {
        self.cancel_gesture();
        let which = self.targets();
        self.drop_pieces(&which);
    }

    /// Put every piece back on the platform (used when supports are switched
    /// off, so nothing is left floating in mid-air).
    fn drop_all_to_plate(&mut self) {
        self.cancel_gesture();
        let all: Vec<usize> = (0..self.models.len()).collect();
        self.drop_pieces(&all);
    }

    fn poll_export(&mut self, ctx: &egui::Context) {
        let mut done = false;
        if let Some(rx) = &self.export.rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    export::Msg::Progress { frac, note } => {
                        self.export.progress = frac;
                        self.export.note = note;
                    }
                    export::Msg::Done(text) => {
                        self.export.result = text.clone();
                        self.export.note = String::new();
                        self.export.progress = 1.0;
                        self.status = text;
                        done = true;
                    }
                    export::Msg::Cancelled => {
                        self.export.result =
                            "Cancelled. The half-written job file was removed again.".into();
                        self.status = "Export cancelled.".into();
                        done = true;
                    }
                    export::Msg::Failed(e) => {
                        self.export.result = format!("Export failed: {e}");
                        self.status = e;
                        done = true;
                    }
                }
                ctx.request_repaint();
            }
        }
        if done {
            self.export.rx = None;
            self.export.cancel = None;
        }
    }

    /// Ask the writer to stop; it removes what it wrote and reports back.
    fn cancel_export(&mut self) {
        if let Some(cancel) = &self.export.cancel {
            cancel.store(true, Ordering::Relaxed);
            self.export.note = "Cancelling…".into();
        }
    }

    fn start_export(&mut self, ctx: &egui::Context) {
        // The layers in memory only describe the plate they were made from.
        if self.out_of_date() {
            self.export.result =
                "The plate has changed since this slice: run Auto all and slice again.".into();
            return;
        }
        let slice = match &self.slice {
            Some(s) if !s.layers.is_empty() => s.clone(),
            _ => {
                self.export.result = "Nothing is sliced yet.".into();
                return;
            }
        };
        let printer = &self.printers[self.printer_idx];
        let format = self
            .export
            .format
            .filter(|format| printer.supports_format(*format))
            .unwrap_or(printer.format);
        let options = export::ExportOptions {
            dir: PathBuf::from(self.export.dir.trim()),
            file_stem: self.export.stem.clone(),
            format,
            also_masks: self.export.also_masks,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.export.cancel = Some(cancel.clone());
        self.export.rx = Some(export::start(
            options,
            self.printers[self.printer_idx].clone(),
            self.settings.clone(),
            slice,
            cancel,
        ));
        self.export.progress = 0.0;
        self.export.result.clear();
        self.export.note = "starting…".into();
        ctx.request_repaint();
    }

    // ------------------------------------------------------------ screenshot

    fn screenshot_tick(&mut self, ctx: &egui::Context) {
        ctx.request_repaint();
        self.frame_no += 1;
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let path = match &self.screenshot {
                Some(p) => p.clone(),
                None => return,
            };
            let mut rgba = Vec::with_capacity(img.pixels.len() * 4);
            for p in &img.pixels {
                rgba.extend_from_slice(&p.to_array());
            }
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let res = image::save_buffer(
                &path,
                &rgba,
                img.size[0] as u32,
                img.size[1] as u32,
                image::ColorType::Rgba8,
            );
            match res {
                Ok(()) => eprintln!("saved screenshot to {}", path.display()),
                Err(e) => eprintln!("could not save the screenshot: {e}"),
            }
            self.screenshot = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if self.frame_no >= 120 && !self.shot_sent {
            self.shot_sent = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        } else if self.frame_no > 900 {
            eprintln!("no picture came back from the window; giving up");
            self.screenshot = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    // ------------------------------------------------------------------- bars

    fn select_printer(&mut self, index: usize) {
        if index == self.printer_idx || index >= self.printers.len() {
            return;
        }
        self.printer_idx = index;
        self.cancel_gesture();
        // Build geometry and raster mapping are not part of PictureKey, so a
        // profile switch must explicitly invalidate both cached pictures.
        self.picture = None;
        self.layer_tex = None;
        self.layer_tex_key = None;
        self.export.result.clear();
        self.export.format = None;
        self.status = format!(
            "{} · .{} · native resolution",
            self.printers[index].name,
            self.printers[index].format.extension()
        );
        self.fit_view();
        self.touched();
    }

    fn printer_picker(&mut self, ui: &mut egui::Ui, id: &str, width: f32) {
        let current = self.printers[self.printer_idx].clone();
        let mut chosen = None;
        egui::ComboBox::from_id_salt(id)
            .selected_text(
                egui::RichText::new(format!("{}  {}", icons::PRINTER, current.name))
                    .size(13.0)
                    .color(TEXT),
            )
            .width(width)
            .show_ui(ui, |ui| {
                ui.set_min_width(width + 54.0);
                for (i, printer) in self.printers.iter().enumerate() {
                    let response = ui.selectable_label(
                        i == self.printer_idx,
                        egui::RichText::new(&printer.name).size(12.5),
                    );
                    if response.clicked() {
                        chosen = Some(i);
                    }
                    response.on_hover_text(printer.note.clone());
                }
            });
        if let Some(index) = chosen {
            self.select_printer(index);
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        let (logo_size, logo_image) = branding::header_image(ctx.pixels_per_point());
        if self.logo.as_ref().map(|(size, _)| *size) != Some(logo_size) {
            self.logo = Some((
                logo_size,
                ctx.load_texture(
                    format!("pinto-slicer-header-{logo_size}"),
                    logo_image,
                    egui::TextureOptions::LINEAR,
                ),
            ));
        }
        let logo = self
            .logo
            .as_ref()
            .expect("the header icon was loaded")
            .1
            .id();
        egui::TopBottomPanel::top("top")
            .exact_height(52.0)
            .frame(
                egui::Frame::none()
                    .fill(BAR)
                    .inner_margin(egui::Margin::symmetric(12.0, 9.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.image((logo, egui::Vec2::splat(branding::HEADER_SIZE)));
                    ui.add_space(3.0);
                    ui.label(
                        egui::RichText::new(branding::APP_NAME)
                            .size(15.0)
                            .color(TEXT)
                            .strong(),
                    );
                    ui.add_space(12.0);
                    egui::menu::menu_button(ui, "File", |ui| {
                        if ui.button("Open models…  Ctrl+O").clicked() {
                            ui.close_menu();
                            self.picker.open_models();
                        }
                        if ui.button("Open project…  Ctrl+Shift+O").clicked() {
                            ui.close_menu();
                            self.open_project();
                        }
                        ui.separator();
                        if ui.button("Save project  Ctrl+S").clicked() {
                            ui.close_menu();
                            self.save_project();
                        }
                        if ui.button("Save project as…  Ctrl+Shift+S").clicked() {
                            ui.close_menu();
                            self.save_project_as();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(self.can_undo(), egui::Button::new("Undo  Ctrl+Z"))
                            .clicked()
                        {
                            ui.close_menu();
                            self.undo();
                        }
                        if ui
                            .add_enabled(
                                self.can_redo(),
                                egui::Button::new("Redo  Ctrl+Shift+Z / Ctrl+Y"),
                            )
                            .clicked()
                        {
                            ui.close_menu();
                            self.redo();
                        }
                    });
                    ui.add_space(12.0);
                    self.printer_picker(ui, "printer_top_left", 230.0);
                    ui.add_space(12.0);
                    for step in [Tab::Prepare, Tab::Supports, Tab::Slice, Tab::Export] {
                        if step_button(ui, self.tab == step, step) {
                            self.open_step(step);
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new("?").size(13.0).color(MUTED))
                                    .fill(PILL)
                                    .rounding(egui::Rounding::same(8.0))
                                    .min_size(egui::vec2(28.0, 28.0)),
                            )
                            .clicked()
                        {
                            self.show_help = true;
                        }
                    });
                });
            });
    }

    fn bottom_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(30.0)
            .frame(
                egui::Frame::none()
                    .fill(BAR)
                    .inner_margin(egui::Margin::symmetric(12.0, 5.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(egui::RichText::new(&self.status).size(12.0).color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(self.printers[self.printer_idx].name.clone())
                                .size(11.5)
                                .color(MUTED),
                        );
                    });
                });
            });
    }

    /// Walking through the slice like a flipbook. The transport lives with the
    /// step that owns the layers, so the plate is never described by two
    /// different sliders.
    fn playback_bar(&mut self, ctx: &egui::Context) {
        if self.tab != Tab::Slice {
            return;
        }
        egui::TopBottomPanel::bottom("playback")
            .exact_height(46.0)
            .frame(
                egui::Frame::none()
                    .fill(BAR)
                    .inner_margin(egui::Margin::symmetric(12.0, 6.0)),
            )
            .show(ctx, |ui| {
                let n = self.slice.as_ref().map(|s| s.layers.len()).unwrap_or(0);
                ui.horizontal_centered(|ui| {
                    if n == 0 {
                        ui.label(
                            egui::RichText::new("Slice the plate to step through the layers.")
                                .size(12.0)
                                .color(MUTED),
                        );
                        self.playback.playing = false;
                        return;
                    }
                    let mut cur = self.current_layer.min(n - 1);
                    let playing = self.playback.playing;
                    if transport_button(ui, if playing { icons::PAUSE } else { icons::PLAY }) {
                        self.playback.playing = !playing;
                        self.playback.left = self.playback.per_layer;
                    }
                    if transport_button(ui, icons::SKIP_BACK) && cur > 0 {
                        cur -= 1;
                    }
                    if transport_button(ui, icons::SKIP_FORWARD) && cur + 1 < n {
                        cur += 1;
                    }
                    if ui
                        .add(
                            egui::Slider::new(&mut cur, 0..=(n - 1))
                                .show_value(false)
                                .trailing_fill(true),
                        )
                        .changed()
                    {
                        self.playback.playing = false;
                    }
                    self.current_layer = cur.min(n - 1);
                    let z = self.slice.as_ref().map(|s| s.layers[cur].z).unwrap_or(0.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} / {}   z {:.2} mm",
                            self.current_layer + 1,
                            n,
                            z
                        ))
                        .size(12.0)
                        .color(TEXT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.playback.per_layer)
                                .speed(0.01)
                                .range(0.02..=1.0)
                                .fixed_decimals(2)
                                .suffix(" s"),
                        );
                        ui.label(egui::RichText::new("per layer").size(11.5).color(MUTED));
                    });
                });
            });
    }

    /// Let playback advance on its own clock while the slice step is open.
    fn playback_tick(&mut self, ctx: &egui::Context) {
        let n = self.slice.as_ref().map(|s| s.layers.len()).unwrap_or(0);
        if !self.playback.playing || self.tab != Tab::Slice || n == 0 {
            return;
        }
        let dt = ctx.input(|i| i.stable_dt).min(0.25);
        self.playback.left -= dt;
        if self.playback.left <= 0.0 {
            self.playback.left = self.playback.per_layer.max(0.02);
            self.current_layer = if self.current_layer + 1 >= n {
                0
            } else {
                self.current_layer + 1
            };
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f32(
            self.playback.left.max(0.0),
        ));
    }

    fn tool_strip(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("tools")
            .exact_width(66.0)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(BAR)
                    .inner_margin(egui::Margin::symmetric(5.0, 10.0)),
            )
            .show(ctx, |ui| {
                let size = egui::vec2(56.0, 46.0);
                if icon_button(
                    ui,
                    icons::OPEN,
                    "Open",
                    &OPEN.tip("Open a model"),
                    false,
                    size,
                ) {
                    self.picker.open_models();
                }
                if icon_button(ui, icons::FIT, "Fit", &FIT.tip("Fit the view"), false, size) {
                    self.fit_view();
                }
                if icon_button(
                    ui,
                    icons::VALUES,
                    "Values",
                    &VALUES.tip("The numbers for this piece"),
                    self.show_values,
                    size,
                ) {
                    self.show_values = !self.show_values;
                }
                ui.add_space(8.0);
                for tool in Tool::ALL {
                    if icon_button(
                        ui,
                        tool.icon(),
                        tool.label(),
                        &tool.tip(),
                        self.tool == tool,
                        size,
                    ) {
                        self.select_tool(tool);
                    }
                }
                // the two view modes sit at the foot of the strip
                let gap = ui.spacing().item_spacing.y;
                let foot = 2.0 * size.y + gap + 4.0;
                if ui.available_height() > foot {
                    ui.add_space(ui.available_height() - foot);
                }
                if icon_button(
                    ui,
                    icons::CUBE,
                    "3D",
                    "The 3D model",
                    self.mode == ViewMode::Solid,
                    size,
                ) {
                    self.mode = ViewMode::Solid;
                }
                if icon_button(
                    ui,
                    icons::LAYERS,
                    "Slices",
                    "The sliced layers",
                    self.mode == ViewMode::Sliced,
                    size,
                ) {
                    self.mode = ViewMode::Sliced;
                }
            });
    }

    /// Pick a tool, bringing the tab that belongs to it into view.
    fn select_tool(&mut self, tool: Tool) {
        self.cancel_gesture();
        self.tool = tool;
        if tool != Tool::View && self.tab != Tab::Prepare {
            self.tab = Tab::Prepare;
        }
    }

    // ---------------------------------------------------------------- panels

    /// Go to a step. Opening a step only sets the view up for it: the slice
    /// step looks at layers, the others at the plate.
    fn open_step(&mut self, step: Tab) {
        self.cancel_gesture();
        self.tab = step;
        if step == Tab::Slice {
            self.mode = ViewMode::Sliced;
        } else {
            self.playback.playing = false;
        }
    }

    fn side_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("side")
            .exact_width(330.0)
            .resizable(true)
            .frame(
                egui::Frame::none()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(10.0, 10.0)),
            )
            .show(ctx, |ui| {
                // The bottom of the panel is where a wizard is walked, so it
                // holds still while the cards above it scroll.
                egui::TopBottomPanel::bottom("step_nav")
                    .exact_height(52.0)
                    .frame(egui::Frame::none().fill(BG))
                    .show_inside(ui, |ui| {
                        ui.add_space(6.0);
                        self.step_nav(ui);
                    });
                self.step_head(ui);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        match self.tab {
                            Tab::Prepare => self.prepare_tab(ui),
                            Tab::Supports => self.support_tab(ui),
                            Tab::Slice => self.slice_tab(ui),
                            Tab::Export => self.export_tab(ui),
                        }
                        ui.add_space(12.0);
                    });
            });
    }

    /// "Step 2 of 4 · Supports" and what the step is for.
    fn step_head(&mut self, ui: &mut egui::Ui) {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("Step {} of 4", self.tab.number()))
                    .size(10.5)
                    .color(ACCENT),
            );
            ui.label(
                egui::RichText::new(self.tab.purpose())
                    .size(10.5)
                    .color(MUTED),
            );
        });
        ui.label(
            egui::RichText::new(self.tab.title())
                .size(19.0)
                .color(TEXT)
                .strong(),
        );
        ui.add_space(6.0);
    }

    /// Back and Next, at the foot of every step.
    fn step_nav(&mut self, ui: &mut egui::Ui) {
        let back = self.tab.back();
        let next = self.tab.next();
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 8.0) * 0.5;
            if nav_button(ui, "Back", PILL, TEXT, width, back.is_some()) {
                if let Some(back) = back {
                    self.open_step(back);
                }
            }
            let walk = ui.available_width();
            match next {
                Some(next) => {
                    let label = format!("Next: {}", next.title());
                    if nav_button(ui, &label, ACCENT_BG, egui::Color32::WHITE, walk, true) {
                        self.open_step(next);
                    }
                }
                None => {
                    nav_button(ui, "Last step", PILL, MUTED, walk, false);
                }
            }
        });
    }

    fn prepare_tab(&mut self, ui: &mut egui::Ui) {
        let mut printer_changed = false;
        card(ui, "PRINTER", |ui| {
            self.printer_picker(ui, "printer_prepare", ui.available_width() - 8.0);
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("+ Duplicate profile").size(12.5))
                        .fill(PILL)
                        .rounding(egui::Rounding::same(8.0)),
                )
                .clicked()
            {
                let mut p = self.printers[self.printer_idx].clone();
                p.name = format!("{} copy", p.name);
                self.printers.push(p);
                let index = self.printers.len() - 1;
                self.select_printer(index);
            }
            let p = &mut self.printers[self.printer_idx];
            small_label(ui, "Name");
            printer_changed |= ui
                .add(egui::TextEdit::singleline(&mut p.name).desired_width(ui.available_width()))
                .changed();
            small_label(ui, "Build area (mm)");
            ui.horizontal(|ui| {
                printer_changed |= ui
                    .add(
                        egui::DragValue::new(&mut p.build[0])
                            .speed(1.0)
                            .suffix(" X"),
                    )
                    .changed();
                printer_changed |= ui
                    .add(
                        egui::DragValue::new(&mut p.build[1])
                            .speed(1.0)
                            .suffix(" Y"),
                    )
                    .changed();
                printer_changed |= ui
                    .add(
                        egui::DragValue::new(&mut p.build[2])
                            .speed(1.0)
                            .suffix(" Z"),
                    )
                    .changed();
            });
            small_label(ui, "Native LCD and job format");
            let (w, h) = p.native_res();
            let pixel = p.pixel_mm();
            ui.label(
                egui::RichText::new(format!(
                    "{w} × {h} px · {:.4} × {:.4} mm/pixel",
                    pixel[0], pixel[1]
                ))
                .size(11.5)
                .color(MUTED),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} · .{} · mirror {}/{}",
                    p.format.label(),
                    p.format.extension(),
                    if p.mirror[0] { "X" } else { "–" },
                    if p.mirror[1] { "Y" } else { "–" },
                ))
                .size(11.5)
                .color(ACCENT),
            );
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(p.note.clone())
                    .size(11.0)
                    .italics()
                    .color(MUTED),
            );
        });
        if printer_changed {
            self.picture = None;
            self.layer_tex = None;
            self.layer_tex_key = None;
            self.touched();
        }

        card(ui, "LAYOUT", |ui| {
            ui.label(
                egui::RichText::new(
                    "Spread the pieces out in a tidy grid, with room to peel them off the plate.",
                )
                .size(12.0)
                .color(MUTED),
            );
            if accent_button(ui, "Arrange all on the plate", ui.available_width()) {
                self.arrange_all(true);
                self.fit_view();
            }
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("Centre the selected piece").size(12.5))
                        .fill(PILL)
                        .rounding(egui::Rounding::same(8.0))
                        .min_size(egui::vec2(ui.available_width(), 28.0)),
                )
                .clicked()
            {
                self.centre_selected();
            }

            if ui
                .add(
                    egui::Button::new(egui::RichText::new("Align & space selected…").size(12.5))
                        .fill(PILL)
                        .rounding(egui::Rounding::same(8.0))
                        .min_size(egui::vec2(ui.available_width(), 28.0)),
                )
                .on_hover_text(
                    "Align two or more picked pieces, distribute their centres, or set equal gaps",
                )
                .clicked()
            {
                self.cancel_gesture();
                self.show_align = true;
            }

            ui.add_space(2.0);
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("Drop onto the plate").size(13.0))
                        .fill(PILL)
                        .rounding(egui::Rounding::same(8.0))
                        .min_size(egui::vec2(ui.available_width(), 30.0)),
                )
                .on_hover_text(
                    "Lower the picked pieces, or everything, until each one touches the plate",
                )
                .clicked()
            {
                self.drop_to_plate();
            }
            ui.label(
                egui::RichText::new("Pieces sit on the plate here; the support run lifts them by the gap when it needs to.")
                    .size(11.0)
                    .color(MUTED),
            );
        });

        card(ui, "PLACEMENT", |ui| {
            if self.models.is_empty() {
                ui.label(
                    egui::RichText::new("Nothing on the plate yet.")
                        .size(12.0)
                        .color(MUTED),
                );
                return;
            }
            let Some(idx) = self.selection.primary() else {
                return;
            };
            let picked = self.selection.len();
            let title = if picked > 1 {
                format!("{} pieces picked", picked)
            } else {
                self.models[idx].mesh.name.clone()
            };
            ui.label(egui::RichText::new(title).size(13.5).color(TEXT).strong());
            // What can be done to the piece as a whole. A mirror is a placement,
            // not a new mesh, so it can be flipped back and forth freely.
            ui.horizontal(|ui| {
                let b = egui::vec2(38.0, 38.0);
                if icon_button(
                    ui,
                    icons::DUPLICATE,
                    "Copy",
                    &DUPLICATE.tip("Duplicate what is picked"),
                    false,
                    b,
                ) {
                    self.duplicate_selected();
                }
                for (axis, name) in ["X", "Y", "Z"].iter().enumerate() {
                    if icon_button_col(
                        ui,
                        icons::MIRROR,
                        name,
                        &format!("Mirror across {name}, keeping it where it is"),
                        AXIS_COL[axis],
                        b,
                    ) {
                        self.mirror_selected(axis);
                    }
                }
                if icon_button(
                    ui,
                    icons::DELETE,
                    "Delete",
                    &REMOVE.tip("Remove what is picked"),
                    false,
                    b,
                ) {
                    self.delete_selected();
                }
            });
            if self.models.get(idx).is_none() {
                return;
            }
            let mut changed = false;
            let mut turn_delta = None;
            let mut drop = false;
            {
                let t = &mut self.models[idx].transform;
                match self.tool {
                    Tool::View => {
                        ui.label(
                            egui::RichText::new("Pick Move, Turn or Size on the left tool strip.")
                                .size(11.5)
                                .color(MUTED),
                        );
                    }
                    Tool::Move => {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("X").size(12.0).color(MUTED));
                            changed |= ui
                                .add(egui::DragValue::new(&mut t.pos.x).speed(0.5).suffix(" mm"))
                                .changed();
                            ui.label(egui::RichText::new("Y").size(12.0).color(MUTED));
                            changed |= ui
                                .add(egui::DragValue::new(&mut t.pos.y).speed(0.5).suffix(" mm"))
                                .changed();
                        });
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Z").size(12.0).color(MUTED));
                            changed |= ui
                                .add(egui::DragValue::new(&mut t.pos.z).speed(0.2).suffix(" mm"))
                                .changed();
                        });
                    }
                    Tool::Turn => {
                        let before = t.rot_deg;
                        let mut x = before.x;
                        let mut y = before.y;
                        let mut z = before.z;
                        let mut turned = false;
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("X").size(12.0).color(MUTED));
                            turned |= ui
                                .add(egui::DragValue::new(&mut x).speed(1.0).suffix("°"))
                                .changed();
                            ui.label(egui::RichText::new("Y").size(12.0).color(MUTED));
                            turned |= ui
                                .add(egui::DragValue::new(&mut y).speed(1.0).suffix("°"))
                                .changed();
                            ui.label(egui::RichText::new("Z").size(12.0).color(MUTED));
                            turned |= ui
                                .add(egui::DragValue::new(&mut z).speed(1.0).suffix("°"))
                                .changed();
                        });
                        if turned {
                            turn_delta = Some(Vec3::new(x, y, z) - before);
                            changed = true;
                        }
                        if picked > 1 {
                            ui.label(
                                egui::RichText::new(
                                    "Every picked piece turns about its own centre.",
                                )
                                .size(11.0)
                                .color(MUTED),
                            );
                        }
                    }
                    Tool::Size => {
                        let same = (t.scale.x - t.scale.y).abs() < 1e-4
                            && (t.scale.y - t.scale.z).abs() < 1e-4;
                        let mut linked = same;
                        let mut s = t.scale.x;
                        ui.checkbox(
                            &mut linked,
                            egui::RichText::new("Same on every side").size(12.0),
                        );
                        if linked {
                            if ui
                                .add(egui::Slider::new(&mut s, 0.05..=5.0).logarithmic(true))
                                .changed()
                            {
                                t.scale = Vec3::new(s, s, s);
                                changed = true;
                            }
                        } else {
                            changed |= ui
                                .add(egui::Slider::new(&mut t.scale.x, 0.05..=5.0).text("X"))
                                .changed();
                            changed |= ui
                                .add(egui::Slider::new(&mut t.scale.y, 0.05..=5.0).text("Y"))
                                .changed();
                            changed |= ui
                                .add(egui::Slider::new(&mut t.scale.z, 0.05..=5.0).text("Z"))
                                .changed();
                        }
                    }
                }
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new("Sit on the plate").size(12.5))
                            .fill(PILL)
                            .rounding(egui::Rounding::same(8.0))
                            .min_size(egui::vec2(ui.available_width(), 26.0)),
                    )
                    .clicked()
                {
                    drop = true;
                }
                let (min, max) = self.models[idx].bounds();
                ui.label(
                    egui::RichText::new(format!(
                        "{:.1} × {:.1} × {:.1} mm",
                        max.x - min.x,
                        max.y - min.y,
                        max.z - min.z
                    ))
                    .size(11.5)
                    .color(MUTED),
                );
                let build = self.printers[self.printer_idx].build;
                let size = max - min;
                let too_big = size.x > build[0] || size.y > build[1] || size.z > build[2];
                if too_big {
                    ui.add_space(3.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "Bigger than the plate ({} × {} × {} mm).",
                            build[0].round(),
                            build[1].round(),
                            build[2].round()
                        ))
                        .size(11.5)
                        .color(WARN),
                    );
                    if ui
                        .add(
                            egui::Button::new(egui::RichText::new("Scale down to fit").size(12.0))
                                .fill(PILL)
                                .rounding(egui::Rounding::same(8.0))
                                .min_size(egui::vec2(ui.available_width(), 26.0)),
                        )
                        .clicked()
                    {
                        let (fx, fy, fz) = (
                            build[0] / size.x.max(0.01),
                            build[1] / size.y.max(0.01),
                            build[2] / size.z.max(0.01),
                        );
                        let f = fx.min(fy).min(fz).min(1.0) * 0.97;
                        let m = &mut self.models[idx];
                        m.transform.scale = m.transform.scale * f;
                        changed = true;
                    }
                }
            }
            if let Some(delta) = turn_delta {
                self.turn_selected_by(delta);
            }
            if changed {
                // the view has to show the new placement right away, not after
                // the supports have been rebuilt
                self.refresh_world();
                self.touched();
            }
            if drop {
                self.drop_to_plate();
            }
        });

        card(ui, "ON THE PLATE", |ui| {
            if self.models.is_empty() {
                ui.label(
                    egui::RichText::new("No models loaded.")
                        .size(12.0)
                        .color(MUTED),
                );
            }
            let mut pick = None;
            for i in 0..self.models.len() {
                let name = self.models[i].mesh.name.clone();
                let selected = self.selection.contains(i);
                let row = egui::Frame::none()
                    .fill(if selected { ACCENT_BG } else { PILL })
                    .rounding(egui::Rounding::same(8.0))
                    .inner_margin(egui::Margin::symmetric(8.0, 5.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(name).size(12.5).color(if selected {
                                egui::Color32::WHITE
                            } else {
                                TEXT
                            }));
                        });
                    });
                if row.response.interact(egui::Sense::click()).clicked() {
                    pick = Some(i);
                }
            }
            if let Some(i) = pick {
                self.cancel_gesture();
                let additive = ui.input(|i| i.modifiers.shift || i.modifiers.command);
                self.selection.click(Some(i), additive);
            }
            let mut remove = false;
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new("Remove").size(12.0))
                            .fill(PILL)
                            .rounding(egui::Rounding::same(8.0)),
                    )
                    .clicked()
                {
                    remove = true;
                }
            });
            if remove {
                self.delete_selected();
            }
        });
    }

    fn support_tab(&mut self, ui: &mut egui::Ui) {
        // `changed` waits for the mouse to be released (so sliders stay
        // smooth); nothing here rebuilds anything by itself — the plate is
        // marked out of date and the run happens when Auto all is pressed.
        let mut changed = false;
        let mut delete_all = false;
        let mut auto_all = false;
        let mut turned_off = false;

        // ------------------------------------------------------------ top
        card(ui, "SUPPORTS", |ui| {
            let was_on = self.settings.supports.enabled;
            ui.checkbox(
                &mut self.settings.supports.enabled,
                egui::RichText::new("Grow supports").size(13.0),
            );
            // The switch is not a settings change to re-run: turning it off puts
            // the pieces back down and throws the grown supports away (below),
            // and turning it on only makes the tab offer Auto all.
            if was_on && !self.settings.supports.enabled {
                turned_off = true;
            } else if !was_on && self.settings.supports.enabled {
                changed = true;
            }

            let s = &self.support.stats;
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{}", s.tips))
                        .size(19.0)
                        .color(TEXT)
                        .strong(),
                );
                ui.label(egui::RichText::new("tips").size(12.0).color(MUTED));
                let mut line = format!("· {} trunks · {} branches", s.trunks, s.branches);
                if s.on_model > 0 {
                    line.push_str(&format!(" · {} on the part", s.on_model));
                }
                ui.label(egui::RichText::new(line).size(11.5).color(MUTED));
            });
            if s.dropped > 0 {
                // A tip only goes unheld when nothing under it is clear all the
                // way up: it is the one number that says the supports are
                // incomplete, so it is said out loud rather than left in the
                // report.
                ui.label(
                    egui::RichText::new(format!(
                        "{} tips found nothing solid to stand on.",
                        s.dropped
                    ))
                    .size(11.0)
                    .color(WARN),
                );
            }
            if self.support_job.is_some() {
                ui.label(
                    egui::RichText::new("Growing supports…")
                        .size(11.5)
                        .color(ACCENT),
                );
            } else if !self.settings.supports.enabled {
                ui.label(
                    egui::RichText::new("Supports are switched off, so nothing is grown.")
                        .size(11.5)
                        .color(WARN),
                );
            } else if self.support.is_empty() {
                ui.label(
                    egui::RichText::new(
                        "Press Auto all to grow the supports for what is on the plate.",
                    )
                    .size(10.5)
                    .color(ACCENT),
                );
            } else if self.supports_stale() {
                ui.label(
                    egui::RichText::new(
                        "The plate has changed since this run — press Auto all again.",
                    )
                    .size(10.5)
                    .color(ACCENT),
                );
            }

            let mut preset = self.settings.supports.preset;
            small_label(ui, "Preset");
            ui.horizontal(|ui| {
                for p in [Preset::Light, Preset::Medium, Preset::Heavy] {
                    let selected = preset == p;
                    let btn = egui::Button::new(egui::RichText::new(p.label()).size(12.5).color(
                        if selected {
                            egui::Color32::WHITE
                        } else {
                            MUTED
                        },
                    ))
                    .fill(if selected { ACCENT_BG } else { PILL })
                    .rounding(egui::Rounding::same(8.0))
                    .min_size(egui::vec2(86.0, 26.0));
                    if ui.add(btn).clicked() {
                        preset = p;
                    }
                }
            });
            if preset != self.settings.supports.preset {
                self.settings.supports.apply_preset(preset);
                changed = true;
            }

            ui.add_space(4.0);
            let can_grow = self.settings.supports.enabled;
            let grown = !self.support.is_empty();
            ui.horizontal(|ui| {
                ui.add_enabled_ui(can_grow, |ui| {
                    if accent_button(ui, "Auto all", 118.0) {
                        auto_all = true;
                    }
                });
                ui.add_enabled_ui(grown, |ui| {
                    if ui
                        .add(
                            egui::Button::new(egui::RichText::new("Delete all").size(12.5))
                                .fill(PILL)
                                .rounding(egui::Rounding::same(8.0))
                                .min_size(egui::vec2(96.0, 26.0)),
                        )
                        .clicked()
                    {
                        delete_all = true;
                    }
                });
            });
        });

        // ------------------------------------------------- height from platform
        card(ui, "HEIGHT FROM PLATFORM", |ui| {
            ui.add_enabled_ui(self.settings.supports.enabled, |ui| {
                let gap = ui
                    .add(
                        egui::Slider::new(
                            &mut self.settings.supports.height_from_platform,
                            0.0..=40.0,
                        )
                        .text("lift")
                        .suffix(" mm")
                        .step_by(0.5),
                    )
                    .changed();
                changed |= gap;
                if gap {
                    // moving the slider moves the pieces straight away, so the
                    // effect is visible even before the supports regrow
                    self.apply_gap();
                    self.refresh_world();
                    self.touched();
                }
                ui.label(
                    egui::RichText::new(format!(
                        "The model's lowest point sits {:.1} mm above the platform. Every \
                         support has to span that gap, so this is also how tall the \
                         support stems are.",
                        self.settings.supports.height_from_platform
                    ))
                    .size(10.5)
                    .color(MUTED),
                );
                ui.horizontal(|ui| {
                    for (label, mm) in [("On plate", 0.0f32), ("5 mm", 5.0), ("10 mm", 10.0)] {
                        if pill_button(
                            ui,
                            label,
                            (self.settings.supports.height_from_platform - mm).abs() < 0.01,
                            egui::vec2(74.0, 24.0),
                        ) {
                            self.settings.supports.height_from_platform = mm;
                            self.apply_gap();
                            self.refresh_world();
                            changed = true;
                        }
                    }
                });
            });
        });

        // ------------------------------------------------------------- base
        card(ui, "BASE", |ui| {
            ui.add_enabled_ui(self.settings.supports.enabled, |ui| {
                let add = ui
                    .checkbox(
                        &mut self.settings.supports.base.enabled,
                        egui::RichText::new("Add a base plate under the feet").size(12.5),
                    )
                    .changed();
                changed |= add;
                ui.add_enabled_ui(self.settings.supports.base.enabled, |ui| {
                    changed |= ui
                        .add(
                            egui::Slider::new(
                                &mut self.settings.supports.base.plate_thickness,
                                0.2..=2.0,
                            )
                            .text("thickness")
                            .suffix(" mm")
                            .step_by(0.05),
                        )
                        .changed();
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut self.settings.supports.base.margin, 0.0..=5.0)
                                .text("margin")
                                .suffix(" mm")
                                .step_by(0.1),
                        )
                        .changed();
                    changed |= ui
                        .add(
                            egui::Slider::new(
                                &mut self.settings.supports.base.chamfer_deg,
                                15.0..=75.0,
                            )
                            .text("chamfer")
                            .suffix("°"),
                        )
                        .changed();
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut self.settings.supports.foot_diameter, 1.5..=8.0)
                                .text("foot Ø")
                                .suffix(" mm")
                                .step_by(0.1),
                        )
                        .changed();
                });
                let plates = self.support.plates.len();
                ui.label(
                    egui::RichText::new(if self.settings.supports.base.enabled {
                        format!(
                            "{plates} plate{} under {} feet. One plate is used where it would \
                             be mostly covered, otherwise one per group of feet.",
                            if plates == 1 { "" } else { "s" },
                            self.support.feet.len()
                        )
                    } else {
                        "No base plate: the feet stand on the platform on their own.".to_string()
                    })
                    .size(10.5)
                    .color(MUTED),
                );
            });
        });

        // --------------------------------------------------------- one tip
        card(ui, "ONE TIP", |ui| {
            support_diagram(ui, &self.settings.supports);
            ui.add_enabled_ui(self.settings.supports.enabled, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Shape").size(11.5).color(MUTED));
                    for (shape, name) in [(TipShape::Cone, "Cone"), (TipShape::Cylinder, "Pin")] {
                        let on = self.settings.supports.tip.shape == shape;
                        let btn =
                            egui::Button::new(egui::RichText::new(name).size(12.0).color(if on {
                                egui::Color32::WHITE
                            } else {
                                MUTED
                            }))
                            .fill(if on { ACCENT_BG } else { PILL })
                            .rounding(egui::Rounding::same(8.0))
                            .min_size(egui::vec2(66.0, 24.0));
                        if ui.add(btn).clicked() && !on {
                            self.settings.supports.tip.shape = shape;
                            changed = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Contact").size(11.5).color(MUTED));
                    for (shape, name) in
                        [(ContactShape::None, "None"), (ContactShape::Ball, "Ball")]
                    {
                        let on = self.settings.supports.tip.contact_shape == shape;
                        let btn =
                            egui::Button::new(egui::RichText::new(name).size(12.0).color(if on {
                                egui::Color32::WHITE
                            } else {
                                MUTED
                            }))
                            .fill(if on { ACCENT_BG } else { PILL })
                            .rounding(egui::Rounding::same(8.0))
                            .min_size(egui::vec2(66.0, 24.0));
                        if ui.add(btn).clicked() && !on {
                            self.settings.supports.tip.contact_shape = shape;
                            changed = true;
                        }
                    }
                });
                changed |= ui
                    .add(
                        egui::Slider::new(
                            &mut self.settings.supports.tip.upper_diameter,
                            0.2..=1.6,
                        )
                        .text("upper Ø")
                        .suffix(" mm")
                        .step_by(0.05),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::Slider::new(
                            &mut self.settings.supports.tip.lower_diameter,
                            0.4..=3.0,
                        )
                        .text("lower Ø")
                        .suffix(" mm")
                        .step_by(0.05),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.settings.supports.tip.length, 1.0..=8.0)
                            .text("length")
                            .suffix(" mm")
                            .step_by(0.1),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.settings.supports.tip.contact_depth, 0.0..=1.0)
                            .text("sinks in")
                            .suffix(" mm")
                            .step_by(0.05),
                    )
                    .changed();
                ui.add_enabled_ui(
                    self.settings.supports.tip.contact_shape == ContactShape::Ball,
                    |ui| {
                        changed |= ui
                            .add(
                                egui::Slider::new(
                                    &mut self.settings.supports.tip.contact_diameter,
                                    0.2..=2.0,
                                )
                                .text("contact Ø")
                                .suffix(" mm")
                                .step_by(0.05),
                            )
                            .changed();
                    },
                );
            });
        });

        // ------------------------------------------------- where they grow
        card(ui, "WHERE THEY GROW", |ui| {
            ui.add_enabled_ui(self.settings.supports.enabled, |ui| {
                changed |= ui
                    .add(
                        egui::Slider::new(
                            &mut self.settings.supports.overhang_angle_deg,
                            5.0..=80.0,
                        )
                        .text("overhang")
                        .suffix("°"),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::Slider::new(
                            &mut self.settings.supports.touch_tip_distance,
                            1.5..=10.0,
                        )
                        .text("tip spacing")
                        .suffix(" mm")
                        .step_by(0.1),
                    )
                    .changed();
                ui.checkbox(
                    &mut self.show_debug,
                    egui::RichText::new("Show overhang faces").size(12.0),
                );
            });
            ui.label(
                egui::RichText::new(
                    "Lower overhang angle and tighter spacing mean more tips. Red faces are the \
                     ones that get a support.",
                )
                .size(11.0)
                .color(MUTED),
            );
        });

        // ------------------------------------------------- branches & trunks
        card(ui, "BRANCHES & TRUNKS", |ui| {
            ui.add_enabled_ui(self.settings.supports.enabled, |ui| {
                changed |= ui
                    .add(
                        egui::Slider::new(
                            &mut self.settings.supports.branch_angle_deg,
                            10.0..=80.0,
                        )
                        .text("branch angle")
                        .suffix("°"),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.settings.supports.branch_diameter, 0.4..=2.5)
                            .text("branch Ø")
                            .suffix(" mm")
                            .step_by(0.05),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.settings.supports.trunk_diameter, 0.8..=4.0)
                            .text("trunk Ø")
                            .suffix(" mm")
                            .step_by(0.05),
                    )
                    .changed();
            });
        });

        card(ui, "LAST RUN", |ui| {
            let note = if self.support_note.is_empty() {
                "Nothing generated yet.".to_string()
            } else {
                self.support_note.clone()
            };
            ui.label(
                egui::RichText::new(note)
                    .size(10.5)
                    .color(MUTED)
                    .monospace(),
            );
            ui.label(
                egui::RichText::new(
                    "Supports are grown by one run of the generator: press Auto all. Moving a \
                     piece leaves them where they were until you run it again.",
                )
                .size(10.5)
                .color(MUTED),
            );
        });

        if delete_all {
            self.clear_supports();
        }
        if turned_off {
            // The pieces came back down, so their placement changed too, and
            // what was grown under the old setting is no longer wanted.
            self.drop_all_to_plate();
            self.refresh_world();
            self.touched();
            self.clear_supports();
        }
        if auto_all {
            if changed {
                self.touched();
            }
            self.start_supports();
            self.mode = ViewMode::Solid;
        } else if changed {
            // Settings changed: what is on screen was made with the old ones.
            self.touched();
        }
    }

    fn slice_tab(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        card(ui, "LAYERS & EXPOSURE", |ui| {
            changed |= ui
                .add(
                    egui::Slider::new(&mut self.settings.layer_height, 0.01..=0.2)
                        .text("layer height")
                        .suffix(" mm")
                        .step_by(0.005),
                )
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut self.settings.exposure, 0.5..=15.0)
                        .text("exposure")
                        .suffix(" s"),
                )
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut self.settings.bottom_layers, 0..=20)
                        .text("bottom layers"),
                )
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut self.settings.bottom_exposure, 5.0..=90.0)
                        .text("bottom exposure")
                        .suffix(" s"),
                )
                .changed();
        });

        card(ui, "SLICE", |ui| {
            if accent_button(ui, "Slice now", ui.available_width()) {
                self.slice_now();
                self.mode = ViewMode::Sliced;
            }
            if self.slice_stale() {
                ui.label(
                    egui::RichText::new("The plate has changed since this slice.")
                        .size(10.5)
                        .color(ACCENT),
                );
            }
            if let Some(sl) = &self.slice {
                let n = sl.layers.len();
                let mins = self.settings.estimated_minutes(n);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{n}"))
                            .size(20.0)
                            .color(TEXT)
                            .strong(),
                    );
                    ui.label(egui::RichText::new("layers").size(12.0).color(MUTED));
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "≈ {:.0} h {:.0} min",
                            (mins / 60.0).floor(),
                            mins % 60.0
                        ))
                        .size(12.5)
                        .color(GOOD),
                    );
                });
            }
        });

        if changed {
            self.touched();
        }
    }

    /// Step 3's other half: the picture the screen would show for the layer
    /// under the transport, beside the plate instead of under it.
    fn layers_panel(&mut self, ctx: &egui::Context) {
        if self.tab != Tab::Slice {
            return;
        }
        egui::SidePanel::right("layers")
            .exact_width(330.0)
            .resizable(true)
            .frame(
                egui::Frame::none()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(10.0, 10.0)),
            )
            .show(ctx, |ui| {
                card(ui, "WHAT THE SCREEN SHOWS", |ui| {
                    if self.slice.is_none() {
                        ui.label(
                            egui::RichText::new("Nothing is sliced yet.")
                                .size(12.0)
                                .color(MUTED),
                        );
                        return;
                    }
                    self.layer_preview(ui);
                    let n = self.slice.as_ref().map(|s| s.layers.len()).unwrap_or(0);
                    let cur = self.current_layer.min(n.saturating_sub(1));
                    let z = self
                        .slice
                        .as_ref()
                        .map(|s| s.layers.get(cur).map(|l| l.z).unwrap_or(0.0))
                        .unwrap_or(0.0);
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("layer {} of {}", cur + 1, n))
                                .size(11.5)
                                .color(TEXT),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!("{z:.2} mm"))
                                    .size(11.5)
                                    .color(MUTED),
                            );
                        });
                    });
                    if self.playback.playing {
                        ui.label(egui::RichText::new("playing").size(10.5).color(ACCENT));
                    }
                });
            });
    }

    /// The mask of the layer on show, one byte per texel, or nothing when the
    /// layer does not exist. The picture on screen is only ever built from a
    /// mask that is real, which is why this is apart from the texture.
    fn layer_mask(&self, w: usize, h: usize) -> Option<Vec<u8>> {
        let build = self.printers[self.printer_idx].build;
        let min = [-build[0] * 0.5, -build[1] * 0.5];
        let max = [build[0] * 0.5, build[1] * 0.5];
        let layer = self.slice.as_ref()?.layers.get(self.current_layer)?;
        let mut buf = Vec::new();
        slicer::fill_mask(layer, min, max, w, h, &mut buf);
        Some(buf)
    }

    fn layer_preview(&mut self, ui: &mut egui::Ui) {
        let build = self.printers[self.printer_idx].build;
        let w = (ui.available_width().max(80.0)) as usize;
        let h = ((w as f32 * build[1] / build[0].max(1.0)).round() as usize).max(20);
        let key = (self.slice_gen, self.current_layer, w);
        let stale = self.layer_tex_key != Some(key);
        if stale {
            match self.layer_mask(w, h) {
                Some(buf) => {
                    // The screen is either lit or it is not, so the picture is
                    // shown the same way a slicer shows it: white where the
                    // resin cures.
                    let mut pixels = Vec::with_capacity(w * h);
                    for v in &buf {
                        let c = *v;
                        pixels.push(egui::Color32::from_rgb(c, c, c));
                    }
                    let img = egui::ColorImage {
                        size: [w, h],
                        pixels,
                    };
                    match &mut self.layer_tex {
                        Some(t) => t.set(img, egui::TextureOptions::NEAREST),
                        None => {
                            self.layer_tex = Some(ui.ctx().load_texture(
                                "layer",
                                img,
                                egui::TextureOptions::NEAREST,
                            ))
                        }
                    }
                }
                // Nothing has been cut for this layer, so there is no picture
                // to keep: a texture whose texel count does not match its size
                // is a panic inside egui.
                None => self.layer_tex = None,
            }
            self.layer_tex_key = Some(key);
        }
        let size = egui::vec2(w as f32, h as f32);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(6.0), egui::Color32::BLACK);
        if let Some(t) = &self.layer_tex {
            ui.painter().image(
                t.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    }

    fn export_tab(&mut self, ui: &mut egui::Ui) {
        let printer = &self.printers[self.printer_idx];
        let output_format = self
            .export
            .format
            .filter(|format| printer.supports_format(*format))
            .unwrap_or(printer.format);
        let available_formats = printer.formats.clone();
        card(ui, "JOB FILE", |ui| {
            small_label(ui, "File name");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.export.stem)
                        .desired_width(ui.available_width()),
                )
                .changed()
            {
                self.record_change();
            }
            ui.label(
                egui::RichText::new(format!(
                    "File name only; .{} is added automatically for {}",
                    output_format.extension(),
                    self.printers[self.printer_idx].name
                ))
                .size(11.0)
                .color(MUTED),
            );
            small_label(ui, "Folder");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.export.dir)
                        .desired_width(ui.available_width()),
                )
                .changed()
            {
                self.record_change();
            }
            ui.label(
                egui::RichText::new("job.json and README.txt go in the same folder.")
                    .size(11.0)
                    .color(MUTED),
            );
        });

        card(ui, "NATIVE OUTPUT", |ui| {
            let printer = &self.printers[self.printer_idx];
            let native = printer.native_res();
            ui.label(
                egui::RichText::new(output_format.label())
                    .size(13.0)
                    .color(TEXT)
                    .strong(),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} × {} px · .{}",
                    native.0,
                    native.1,
                    output_format.extension()
                ))
                .size(12.0)
                .color(ACCENT),
            );
            if available_formats.len() > 1 {
                small_label(ui, "Compatible job format");
                egui::ComboBox::from_id_salt("export_format")
                    .selected_text(output_format.label())
                    .width(ui.available_width() - 8.0)
                    .show_ui(ui, |ui| {
                        for format in available_formats {
                            if ui
                                .selectable_label(
                                    format == output_format,
                                    format!(".{} · {}", format.extension(), format.label()),
                                )
                                .clicked()
                            {
                                self.export.format = Some(format);
                                self.record_change();
                            }
                        }
                    });
            }
            ui.label(
                egui::RichText::new(
                    "The LCD resolution always stays native. Format choices are limited to those accepted by this printer profile.",
                )
                .size(11.0)
                .color(MUTED),
            );
            if ui
                .checkbox(
                    &mut self.export.also_masks,
                    egui::RichText::new("Also write the layer pictures (masks)").size(12.0),
                )
                .changed()
            {
                self.record_change();
            }
        });

        card(ui, "EXPORT", |ui| {
            let busy = self.export.rx.is_some();
            let layers = self.slice.as_ref().map(|s| s.layers.len()).unwrap_or(0);
            let fresh = !self.out_of_date();
            ui.add_enabled_ui(!busy && layers > 0 && fresh, |ui| {
                if accent_button(ui, "Write the job file", ui.available_width()) {
                    self.start_export(ui.ctx());
                }
            });
            if !fresh {
                ui.label(
                    egui::RichText::new(
                        "The plate has moved on since this slice: run Auto all and slice again before writing a job file.",
                    )
                    .size(10.5)
                    .color(ACCENT),
                );
            }
            if busy {
                ui.add(egui::ProgressBar::new(self.export.progress).show_percentage());
                let cancelling = self
                    .export
                    .cancel
                    .as_ref()
                    .is_some_and(|c| c.load(Ordering::Relaxed));
                ui.horizontal(|ui| {
                    let note = if cancelling {
                        "Cancelling…"
                    } else {
                        &self.export.note
                    };
                    ui.label(egui::RichText::new(note).size(11.5).color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if nav_button(ui, "Cancel", PILL, TEXT, 78.0, !cancelling) {
                            self.cancel_export();
                        }
                    });
                });
            }
            if !self.export.result.is_empty() {
                ui.label(
                    egui::RichText::new(&self.export.result)
                        .size(11.5)
                        .color(GOOD),
                );
            }
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Writers follow documented open-source format layouts. Test a short print on your machine before a long job.",
                )
                .size(11.0)
                .italics()
                .color(WARN),
            );
        });
    }

    fn central(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::from_rgb(14, 16, 20)))
            .show(ctx, |ui| {
                let size = ui.available_size();
                let (resp, painter) = ui.allocate_painter(size, egui::Sense::click_and_drag());
                let rect = resp.rect;
                self.view_rect = Some(rect);
                let w = rect.width().max(32.0);
                let h = rect.height().max(32.0);

                // Everything the pointer does is decided in one place; this
                // only carries out what it says.
                let probe = render::View::new(&self.cam, w, h);
                let gizmo = self.gizmo_for();
                let raw_pointer = input::Pointer::read(&resp);
                let pointer = raw_pointer.in_view(rect);
                let navigation = nav_cube::NavCube::new(&probe, rect);
                let over_navigation = navigation.contains(raw_pointer.pos);
                let drag_active = self.input.is_dragging();
                let face_click = if raw_pointer.primary_pressed && !drag_active {
                    navigation.hit(raw_pointer.pos)
                } else {
                    None
                };
                let hover = pointer.pos.filter(|_| resp.hovered() && !over_navigation);
                let hover_handle = match &self.grab {
                    Some(g) => Some(g.handle),
                    None => hover.and_then(|p| gizmo.as_ref().and_then(|g| g.hit(&probe, p))),
                };
                if let Some(face) = face_click {
                    self.cam.align_eye_to(face.direction());
                    self.status = format!("{} view", face.label());
                    self.cancel_gesture();
                }
                let actions = if over_navigation && !drag_active {
                    Vec::new()
                } else {
                    // Existing drags still receive updates and, critically,
                    // their release when the pointer is over the view cube.
                    self.input.update(pointer, gizmo.as_ref(), &probe)
                };
                for action in actions {
                    match action {
                        input::Action::Click { at, additive } => {
                            self.click_in_view(at, w, h, additive);
                        }
                        input::Action::Marquee { area } => self.marquee = Some(area),
                        input::Action::MarqueeEnd { area, additive } => {
                            self.marquee = None;
                            self.select_in(area, &probe, additive);
                        }
                        input::Action::MarqueeCancel => self.marquee = None,
                        input::Action::Orbit(d) => {
                            self.cam.yaw -= d.x * 0.008;
                            self.cam.pitch = (self.cam.pitch + d.y * 0.008).clamp(-1.45, 1.45);
                        }
                        input::Action::Pan(d) => {
                            let scale =
                                2.0 * self.cam.dist * (self.cam.fov_deg.to_radians() * 0.5).tan()
                                    / h;
                            self.cam.target = self.cam.target
                                + probe.right * (-d.x * scale)
                                + probe.up * (d.y * scale);
                        }
                        input::Action::Grab { handle, .. } => self.grab_handles(handle),
                        input::Action::Drag { change } => self.drag_handles(change),
                        input::Action::Release => {
                            // a change to a piece needs its supports rebuilt;
                            // looking around never does
                            if self.grab.take().is_some() {
                                self.touched();
                            }
                        }
                    }
                }
                if over_navigation {
                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                } else {
                    ctx.set_cursor_icon(self.input.cursor(gizmo.as_ref(), &probe, pointer.pos));
                }
                // the wheel zooms while the pointer is over the view
                if resp.hovered() && !over_navigation {
                    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                    if scroll.abs() > 0.1 {
                        self.cam.dist =
                            (self.cam.dist * (1.0 - scroll * 0.0015)).clamp(25.0, 2500.0);
                    }
                }

                let build = self.printers[self.printer_idx].build;
                let pixels_per_point = ctx.pixels_per_point();
                let key = PictureKey {
                    size: (w.max(64.0), h.max(64.0)),
                    pixels: [
                        (w * pixels_per_point).round().max(1.0) as u32,
                        (h * pixels_per_point).round().max(1.0) as u32,
                    ],
                    cam: self.cam,
                    mode: self.mode,
                    layer: self.current_layer,
                    world: self.world_gen,
                    cut: self.slice_gen,
                    picked: picked_fingerprint(self.selection.pieces()),
                    debug: self.show_debug,
                };
                if self.picture.as_ref().is_none_or(|p| p.key != key) {
                    let scene = Scene {
                        models: &self.world,
                        selection: self.selection.pieces(),
                        support_mesh: Some(&self.support_mesh),
                        overhang: if self.show_debug {
                            Some(&self.support.debug_mesh)
                        } else {
                            None
                        },
                        islands: if self.show_debug {
                            &self.support.debug_islands
                        } else {
                            &[]
                        },
                        build,
                        mode: self.mode,
                        slice: self.slice.as_ref(),
                        current_layer: self.current_layer,
                        show_plate: true,
                    };
                    let state = frame.wgpu_render_state().expect("wgpu is enabled");
                    let gpu = self
                        .gpu
                        .get_or_insert_with(|| gpu_render::GpuRenderer::new(&state.device));
                    let view = render::View::new(&self.cam, key.size.0, key.size.1);
                    let id = gpu.render(
                        state,
                        &scene,
                        &view,
                        key.pixels,
                        gpu_render::SceneRevision {
                            models: key.world,
                            derived: key.cut,
                        },
                    );
                    match &mut self.picture {
                        Some(p) => {
                            p.tex = id;
                            p.key = key;
                        }
                        None => {
                            self.picture = Some(Picture { key, tex: id });
                        }
                    }
                }
                if let Some(p) = &self.picture {
                    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                    painter.image(p.tex, rect, uv, egui::Color32::WHITE);
                }
                // Handles and the marquee are worked out in view pixels; the
                // panel's corner offset is added here, once.
                let origin = rect.min.to_vec2();
                let draw_view = render::View::new(&self.cam, w, h);
                if let Some(g) = &gizmo {
                    for mut shape in g.shapes(&draw_view, hover_handle) {
                        shape.translate(origin);
                        painter.add(shape);
                    }
                }
                // and the box the pointer is pulling out, over everything
                if let Some(area) = self.marquee {
                    let area = area.translate(origin);
                    painter.rect_filled(
                        area,
                        egui::Rounding::same(1.0),
                        egui::Color32::from_rgba_unmultiplied(74, 142, 236, 26),
                    );
                    painter.rect_stroke(
                        area,
                        egui::Rounding::same(1.0),
                        egui::Stroke::new(1.2_f32, ACCENT),
                    );
                }

                // The orientation cube is also full-resolution and intercepts
                // clicks before they can select a model behind it.
                navigation.paint(&painter, raw_pointer.pos);

                // a small floating badge instead of a plain caption
                let text = match self.mode {
                    ViewMode::Solid => "3D MODEL",
                    ViewMode::Sliced => "SLICED LAYERS",
                };
                badge(&painter, rect.left_top() + egui::vec2(12.0, 12.0), text);
                if self.models.is_empty() {
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Open an STL or OBJ file to start  (Ctrl+O)",
                        egui::FontId::proportional(16.0),
                        MUTED,
                    );
                }
            });
    }

    // --------------------------------------------------------------- keys

    /// The keys that work anywhere in the window, kept apart from the viewport
    /// gestures so a key never competes with the pointer.
    /// The keys written on the tooltips, all read from the same place as the
    /// tooltips themselves so the two cannot say different things.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let pressed = ctx.input(|i| Pressed {
            open: OPEN.pressed(i),
            open_project: i.key_pressed(egui::Key::O) && i.modifiers.command && i.modifiers.shift,
            save_project: i.key_pressed(egui::Key::S) && i.modifiers.command && !i.modifiers.shift,
            save_project_as: i.key_pressed(egui::Key::S)
                && i.modifiers.command
                && i.modifiers.shift,
            undo: i.key_pressed(egui::Key::Z) && i.modifiers.command && !i.modifiers.shift,
            redo: (i.key_pressed(egui::Key::Z) && i.modifiers.command && i.modifiers.shift)
                || (i.key_pressed(egui::Key::Y) && i.modifiers.command),
            fit: FIT.pressed(i),
            values: VALUES.pressed(i),
            duplicate: DUPLICATE.pressed(i),
            everything: SELECT_ALL.pressed(i),
            nothing: i.key_pressed(egui::Key::Escape),
            remove: REMOVE.pressed(i) || i.key_pressed(egui::Key::Backspace),
            tool: Tool::ALL.into_iter().find(|t| t.shortcut().pressed(i)),
        });
        if pressed.open_project {
            self.open_project();
        } else if pressed.open {
            self.picker.open_models();
        }
        if pressed.save_project_as {
            self.save_project_as();
        } else if pressed.save_project {
            self.save_project();
        }
        if pressed.undo {
            self.undo();
        }
        if pressed.redo {
            self.redo();
        }
        if pressed.fit {
            self.fit_view();
        }
        if pressed.values {
            self.show_values = !self.show_values;
        }
        if pressed.duplicate {
            self.duplicate_selected();
        }
        if pressed.everything {
            self.cancel_gesture();
            self.selection.all(self.models.len());
        }
        if pressed.nothing {
            self.cancel_gesture();
            self.selection.clear();
        }
        if pressed.remove {
            self.delete_selected();
        }
        if let Some(tool) = pressed.tool {
            self.select_tool(tool);
        }
    }

    // ---------------------------------------------------------------- windows

    fn windows(&mut self, ctx: &egui::Context) {
        self.values_window(ctx);
        self.alignment_window(ctx);
        self.help_window(ctx);
    }

    /// The numbers for the selected piece, floating over the view so they can
    /// be read and typed into while a handle is being dragged.
    /// Where the Transform window first opens. After that its position belongs
    /// to the user, and egui remembers the dragged location.
    fn values_default_pos(&self, ctx: &egui::Context) -> egui::Pos2 {
        let screen = ctx.screen_rect();
        let right = self
            .view_rect
            .map_or(screen.right() - 12.0, |view| view.right() - 12.0);
        let top = self
            .view_rect
            .map_or(screen.top() + 96.0, |view| view.top() + 140.0);
        egui::pos2(right - 270.0, top)
    }

    fn values_window(&mut self, ctx: &egui::Context) {
        if !self.show_values || self.models.is_empty() {
            return;
        }
        let Some(idx) = self.selection.primary() else {
            return;
        };
        let mut open = self.show_values;
        let mut changed = false;
        let (mut snap, mut snap_deg) = (self.snap, self.snap_deg);
        egui::Window::new("Transform")
            .open(&mut open)
            .collapsible(true)
            .resizable(false)
            .movable(true)
            // Open beside the plate once, then let the user drag the card out
            // of the way instead of pinning it over the model every frame.
            .default_pos(self.values_default_pos(ctx))
            .default_width(238.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(self.models[idx].mesh.name.clone())
                        .size(12.5)
                        .color(TEXT)
                        .strong(),
                );
                let t = &mut self.models[idx].transform;

                ui.add_space(4.0);
                changed |= values_row(
                    ui,
                    "MOVE",
                    [&mut t.pos.x, &mut t.pos.y, &mut t.pos.z],
                    0.5,
                    " mm",
                );
                ui.add_space(3.0);
                changed |= values_row(
                    ui,
                    "TURN",
                    [&mut t.rot_deg.x, &mut t.rot_deg.y, &mut t.rot_deg.z],
                    1.0,
                    "°",
                );
                ui.horizontal(|ui| {
                    ui.checkbox(
                        &mut snap,
                        egui::RichText::new("Snap the turn rings").size(11.5),
                    )
                    .on_hover_text("Round the angle while a ring is dragged");
                    if snap {
                        ui.add(
                            egui::DragValue::new(&mut snap_deg)
                                .speed(0.5)
                                .range(0.1..=90.0)
                                .fixed_decimals(0)
                                .suffix("°"),
                        );
                    }
                });
                ui.add_space(3.0);
                let (mut sx, mut sy, mut sz) = (t.scale.x, t.scale.y, t.scale.z);
                if values_row(ui, "SIZE", [&mut sx, &mut sy, &mut sz], 0.01, "×") {
                    t.scale = Vec3::new(
                        sx.clamp(0.05, 5.0),
                        sy.clamp(0.05, 5.0),
                        sz.clamp(0.05, 5.0),
                    );
                    changed = true;
                }

                let (min, max) = self.models[idx].bounds();
                ui.add_space(5.0);
                ui.label(
                    egui::RichText::new(format!(
                        "{:.1} × {:.1} × {:.1} mm on the plate",
                        max.x - min.x,
                        max.y - min.y,
                        max.z - min.z
                    ))
                    .size(10.5)
                    .color(MUTED),
                );
                if self.models[idx].transform.mirror.any() {
                    let m = self.models[idx].transform.mirror;
                    let mut on = Vec::new();
                    for (i, axis) in ["X", "Y", "Z"].iter().enumerate() {
                        if m.axis(i) {
                            on.push(*axis);
                        }
                    }
                    ui.label(
                        egui::RichText::new(format!("mirrored across {}", on.join(", ")))
                            .size(10.5)
                            .color(ACCENT),
                    );
                }
            });
        self.show_values = open;
        let snap_changed = self.snap != snap || (self.snap_deg - snap_deg).abs() > 1e-6;
        self.snap = snap;
        self.snap_deg = snap_deg;
        if snap_changed {
            self.record_change();
        }
        if changed {
            self.refresh_world();
            self.touched();
        }
    }

    fn alignment_window(&mut self, ctx: &egui::Context) {
        if !self.show_align {
            return;
        }
        let mut open = self.show_align;
        egui::Window::new("Align & space")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(340.0)
            .default_pos(egui::pos2(
                ctx.screen_rect().center().x - 170.0,
                ctx.screen_rect().center().y - 220.0,
            ))
            .movable(true)
            .show(ctx, |ui| {
                ui.set_width(320.0);
                ui.set_max_width(320.0);
                let count = self.selection.len();
                ui.label(
                    egui::RichText::new(format!(
                        "{count} {} selected",
                        if count == 1 { "piece" } else { "pieces" }
                    ))
                    .size(13.0)
                    .color(TEXT)
                    .strong(),
                );
                if count < 2 {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(
                            "Select at least two pieces with Shift-click or a selection box.",
                        )
                        .size(11.5)
                        .color(MUTED),
                    );
                    return;
                }
                ui.add_space(5.0);
                small_label(ui, "ALIGN TO THE SELECTION BOUNDS");
                for (axis, name, actions) in [
                    (
                        0,
                        "X",
                        [
                            ("Left", layout::Alignment::Min),
                            ("Centre", layout::Alignment::Centre),
                            ("Right", layout::Alignment::Max),
                        ],
                    ),
                    (
                        1,
                        "Y",
                        [
                            ("Front", layout::Alignment::Min),
                            ("Centre", layout::Alignment::Centre),
                            ("Back", layout::Alignment::Max),
                        ],
                    ),
                    (
                        2,
                        "Z",
                        [
                            ("Bottom", layout::Alignment::Min),
                            ("Centre", layout::Alignment::Centre),
                            ("Top", layout::Alignment::Max),
                        ],
                    ),
                ] {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [24.0, 24.0],
                            egui::Label::new(egui::RichText::new(name).color(AXIS_COL[axis])),
                        );
                        let width = ((ui.available_width() - 12.0) / 3.0).max(56.0);
                        for (label, mode) in actions {
                            if nav_button(ui, label, PILL, TEXT, width, true) {
                                self.align_picked(axis, mode);
                            }
                        }
                    });
                }
                ui.add_space(5.0);
                ui.separator();
                small_label(ui, "DISTRIBUTE CENTRES");
                ui.label(
                    egui::RichText::new("Keep the two outer centres fixed on X or Y.")
                        .size(11.0)
                        .color(MUTED),
                );
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - 6.0) * 0.5;
                    if nav_button(ui, "Distribute X", PILL, TEXT, width, true) {
                        self.distribute_picked(0, false);
                    }
                    if nav_button(ui, "Distribute Y", PILL, TEXT, width, true) {
                        self.distribute_picked(1, false);
                    }
                });
                ui.add_space(5.0);
                ui.separator();
                small_label(ui, "EQUAL GAPS");
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Gap").size(11.5).color(MUTED));
                    if ui
                        .add(
                            egui::DragValue::new(&mut self.layout_gap)
                                .speed(0.5)
                                .range(0.0..=100.0)
                                .fixed_decimals(1)
                                .suffix(" mm"),
                        )
                        .changed()
                    {
                        self.record_change();
                    }
                });
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - 6.0) * 0.5;
                    if nav_button(ui, "Space X", PILL, TEXT, width, true) {
                        self.distribute_picked(0, true);
                    }
                    if nav_button(ui, "Space Y", PILL, TEXT, width, true) {
                        self.distribute_picked(1, true);
                    }
                });
                ui.label(
                    egui::RichText::new("Equal gaps preserve the selection group's centre.")
                        .size(11.0)
                        .color(MUTED),
                );
            });
        self.show_align = open;
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        if !self.show_help {
            return;
        }
        let mut open = self.show_help;
        egui::Window::new("How to use this")
            .open(&mut open)
            .default_width(470.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(
                        "The window is walked in four steps; Back and Next at the bottom of \
                         this panel move between them, and any step can be opened directly.\n\
                         1 Prepare — open a model, arrange the pieces, drop them onto the plate \
                         with the button at the bottom of the layout card.\n\
                         2 Supports — choose a preset and the tip, pillar and base sizes, then \
                         press Auto all: that one run is the only thing that grows supports, so \
                         moving a piece afterwards leaves them where they are until you run it \
                         again.\n\
                         3 Slice — layer height and exposure, then Slice now. The picture beside \
                         the plate is what the screen shows; the bar underneath plays through \
                         the layers.\n\
                         4 Export — writes the selected printer's native job file at its native LCD resolution.",
                    )
                    .size(12.5),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "In the view\n                         Right-drag to orbit, middle-drag to pan, scroll to zoom.\n                         Left-click picks a piece, shift-click adds to the pick, and \
                         left-dragging a box picks everything it covers.\n                         The handles move, turn and size what is picked.\n                         Every button in the left strip shows its key on hover; the \
                         same keys work while the view has the keyboard: W G R S F V, \
                         Ctrl+O open models, Ctrl+Shift+O open project, Ctrl+S save, \
                         Ctrl+Shift+S save as, Ctrl+Z undo, Ctrl+Shift+Z or Ctrl+Y redo, \
                         Ctrl+D duplicate, Ctrl+A pick everything, Escape to let go, \
                         Delete or Backspace to remove. A turn ring can be made to land on \
                         round numbers from the Snap box in the numbers window.

                         The Align & space dialog can be dragged by its title bar. Projects \
                         embed their meshes and settings in a single .rslice file; supports \
                         are rebuilt with Auto all after opening.",
                    )
                    .size(12.5),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "Printer jobs follow the corresponding open-source format layouts. Test a short print on your machine before a long job.",
                    )
                    .size(11.0)
                    .italics()
                    .color(WARN),
                );
                ui.add_space(8.0);
                ui.separator();
                ui.label(
                    egui::RichText::new(
                        "Pinto Slicer is free software under the GNU Affero General Public \
                         License v3. It comes with ABSOLUTELY NO WARRANTY.\n\
                         Support-generation code is adapted from PrusaSlicer; upstream \
                         copyrights remain with their respective holders.",
                    )
                    .size(11.0)
                    .color(MUTED),
                );
                ui.hyperlink_to(
                    "Source code, license, and third-party notices",
                    "https://github.com/makzumi/pinto-slicer",
                );
            });
        self.show_help = open;
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.screenshot.is_some() {
            self.screenshot_tick(ctx);
        }
        self.poll_export(ctx);
        self.poll_supports(ctx);
        self.poll_project();
        self.poll_picker();
        self.history_tick();
        if self.project_job.is_some() || self.picker.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.playback_tick(ctx);
        self.top_bar(ctx);
        self.bottom_bar(ctx);
        self.playback_bar(ctx);
        self.tool_strip(ctx);
        self.shortcuts(ctx);
        self.side_panel(ctx);
        self.layers_panel(ctx);
        self.central(ctx, frame);
        self.windows(ctx);
    }
}

// ------------------------------------------------------------------ widgets

/// One line of numbers for the values window: an axis letter per field, in the
/// same colour the gizmo uses for that axis.
fn values_row(
    ui: &mut egui::Ui,
    title: &str,
    vals: [&mut f32; 3],
    speed: f64,
    suffix: &str,
) -> bool {
    let mut changed = false;
    ui.label(egui::RichText::new(title).size(10.0).color(MUTED));
    ui.horizontal(|ui| {
        for (i, v) in vals.into_iter().enumerate() {
            ui.label(
                egui::RichText::new(["X", "Y", "Z"][i])
                    .size(10.5)
                    .color(AXIS_COL[i]),
            );
            changed |= ui
                .add(
                    egui::DragValue::new(v)
                        .speed(speed)
                        .suffix(suffix)
                        .fixed_decimals(1),
                )
                .changed();
        }
    });
    changed
}

/// A small square button holding one glyph, for the playback transport.
fn transport_button(ui: &mut egui::Ui, icon: icons::Icon) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
    let hovered = resp.hovered();
    ui.painter().rect_filled(
        rect,
        egui::Rounding::same(7.0),
        if hovered { HOVER } else { PILL },
    );
    icons::paint(ui.painter(), rect, icon, if hovered { TEXT } else { MUTED });
    resp.clicked()
}

/// How much of a button's height the caption under the glyph takes.
const CAPTION: f32 = 12.0;

/// A square button that shows a glyph with a small word under it.
fn icon_button(
    ui: &mut egui::Ui,
    icon: icons::Icon,
    label: &str,
    tip: &str,
    selected: bool,
    size: egui::Vec2,
) -> bool {
    icon_button_inner(ui, icon, label, tip, selected, None, size)
}

/// The same button, with the glyph in a given colour: used for the mirror
/// buttons so each one reads as the axis it flips.
fn icon_button_col(
    ui: &mut egui::Ui,
    icon: icons::Icon,
    label: &str,
    tip: &str,
    colour: egui::Color32,
    size: egui::Vec2,
) -> bool {
    icon_button_inner(ui, icon, label, tip, false, Some(colour), size)
}

fn icon_button_inner(
    ui: &mut egui::Ui,
    icon: icons::Icon,
    label: &str,
    tip: &str,
    selected: bool,
    colour: Option<egui::Color32>,
    size: egui::Vec2,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let hovered = resp.hovered();
    let bg = if selected {
        ACCENT_BG
    } else if hovered {
        HOVER
    } else {
        egui::Color32::TRANSPARENT
    };
    if bg != egui::Color32::TRANSPARENT {
        ui.painter()
            .rect_filled(rect.shrink(1.5), egui::Rounding::same(7.0), bg);
    }
    let ink = colour.unwrap_or(if selected {
        egui::Color32::WHITE
    } else if hovered {
        TEXT
    } else {
        MUTED
    });
    let glyph = egui::Rect::from_min_max(rect.min, rect.max - egui::vec2(0.0, CAPTION));
    icons::paint(ui.painter(), glyph, icon, ink);
    if !label.is_empty() {
        ui.painter().text(
            egui::pos2(rect.center().x, rect.max.y - 2.0),
            egui::Align2::CENTER_BOTTOM,
            label,
            egui::FontId::proportional(9.5),
            if selected {
                egui::Color32::WHITE
            } else {
                MUTED
            },
        );
    }
    resp.on_hover_text(tip).clicked()
}

/// One button of the step footer. Painted here rather than left to `egui` so
/// both halves look like one control, and a step with nowhere to go still
/// reads as a button instead of fading into the panel.
fn nav_button(
    ui: &mut egui::Ui,
    text: &str,
    fill: egui::Color32,
    ink: egui::Color32,
    width: f32,
    enabled: bool,
) -> bool {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 34.0), sense);
    let painter = ui.painter_at(rect);
    let fill = match (enabled, resp.hovered()) {
        (false, _) => fill.linear_multiply(0.6),
        (true, true) => HOVER,
        (true, false) => fill,
    };
    painter.rect_filled(rect, egui::Rounding::same(8.0), fill);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(13.0),
        if enabled { ink } else { MUTED },
    );
    enabled && resp.clicked()
}

/// Round a turned angle to the snap step. The hand may stop anywhere, so the
/// rounding happens once, here, and both the piece and the numbers agree.
fn snap_angle(deg: f32, snapped: bool, step: f32) -> f32 {
    if !snapped {
        return deg;
    }
    let step = step.max(0.1);
    (deg / step).round() * step
}

fn card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(CARD)
        .rounding(egui::Rounding::same(12.0))
        .inner_margin(egui::Margin::symmetric(12.0, 10.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(title).size(10.5).strong().color(MUTED));
            ui.add_space(6.0);
            body(ui);
        });
    ui.add_space(8.0);
}

fn small_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(text).size(11.0).color(MUTED));
}

fn pill_button(ui: &mut egui::Ui, label: &str, selected: bool, size: egui::Vec2) -> bool {
    let btn = egui::Button::new(egui::RichText::new(label).size(12.5).color(if selected {
        egui::Color32::WHITE
    } else {
        MUTED
    }))
    .fill(if selected { ACCENT_BG } else { PILL })
    .rounding(egui::Rounding::same(9.0))
    .min_size(size);
    ui.add(btn).clicked()
}

fn accent_button(ui: &mut egui::Ui, label: &str, width: f32) -> bool {
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(13.5)
                .color(egui::Color32::WHITE),
        )
        .fill(ACCENT)
        .rounding(egui::Rounding::same(9.0))
        .min_size(egui::vec2(width.max(60.0), 32.0)),
    )
    .clicked()
}

/// One step of the wizard: its number in a small badge, then its name.
fn step_button(ui: &mut egui::Ui, selected: bool, step: Tab) -> bool {
    let galley = ui.painter().layout_no_wrap(
        step.title().to_string(),
        egui::FontId::proportional(14.0),
        egui::Color32::WHITE,
    );
    let badge = 18.0;
    let size = egui::vec2(galley.size().x + badge + 30.0, 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    {
        let p = ui.painter();
        if selected {
            p.rect_filled(rect, egui::Rounding::same(9.0), ACCENT_BG);
        } else if resp.hovered() {
            p.rect_filled(rect, egui::Rounding::same(9.0), HOVER);
        }
        let colour = if selected {
            egui::Color32::WHITE
        } else {
            MUTED
        };
        let circle = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 10.0 + badge * 0.5, rect.center().y),
            egui::vec2(badge, badge),
        );
        // the number reads as a badge once you are on the step, and as a quiet
        // disc otherwise
        p.circle_filled(
            circle.center(),
            badge * 0.5,
            if selected {
                egui::Color32::from_white_alpha(46)
            } else {
                PILL
            },
        );
        let num = ui.painter().layout_no_wrap(
            step.number().to_string(),
            egui::FontId::proportional(11.0),
            egui::Color32::WHITE,
        );
        p.galley(circle.center() - num.size() * 0.5, num, colour);
        let pos = egui::pos2(
            circle.right() + 7.0,
            rect.center().y - galley.size().y * 0.5,
        );
        p.galley(pos, galley, colour);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

fn badge(painter: &egui::Painter, at: egui::Pos2, text: &str) {
    let galley = painter.layout_no_wrap(text.to_string(), egui::FontId::proportional(10.5), MUTED);
    let pad = egui::vec2(8.0, 4.0);
    let rect = egui::Rect::from_min_size(at, galley.size() + pad * 2.0);
    painter.rect_filled(
        rect,
        egui::Rounding::same(7.0),
        egui::Color32::from_black_alpha(120),
    );
    painter.galley(rect.min + pad, galley, MUTED);
}

/// A small picture of one support: the cone tip at the top, thin branches
/// feeding into a trunk, and the chamfered plate it stands on. The widths and
/// heights follow the settings, so moving a slider changes the drawing.
fn support_diagram(ui: &mut egui::Ui, s: &crate::supports::SupportConfig) {
    let height = 132.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let p = ui.painter_at(rect);
    p.rect_filled(
        rect,
        egui::Rounding::same(8.0),
        egui::Color32::from_rgb(26, 29, 35),
    );

    let mid = rect.center().x;
    let top = rect.top() + 10.0;
    let bottom = rect.bottom() - 10.0;
    let span = bottom - top;

    let plate_h = (s.base.plate_thickness * 8.0).clamp(8.0, 18.0);
    let plate_top = bottom - plate_h;
    // Drawn schematically: the plate is always the widest part so the picture
    // reads at a glance, and the other widths keep their real proportions to it.
    let plate_r = (rect.width() * 0.22).min(84.0);
    let ratio = |mm: f32| (mm / s.foot_diameter.max(0.5)).clamp(0.06, 0.55) * plate_r;
    let trunk_r = ratio(s.trunk_diameter).max(3.0);
    let tip_low_r = ratio(s.tip.lower_diameter).max(2.4);
    let tip_up_r =
        (tip_low_r * (s.tip.upper_diameter / s.tip.lower_diameter.max(0.1))).clamp(1.2, tip_low_r);
    let tip_h = (s.tip.length * 8.0).clamp(18.0, 46.0);

    // the trunk, from the plate up to where the tips begin
    let merge_z = plate_top - span * 0.42;
    p.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(mid - trunk_r, merge_z - 6.0),
            egui::pos2(mid + trunk_r, plate_top),
        ),
        1.0,
        SUP_COL,
    );

    // two thin branches fanning out of the trunk top
    let tip_x = [mid - tip_low_r * 2.6, mid + tip_low_r * 2.6];
    for (i, x) in tip_x.iter().enumerate() {
        let from = egui::pos2(*x, top + tip_h);
        let to = egui::pos2(mid, merge_z);
        let stroke_w = (ratio(s.branch_diameter) * 1.6).max(1.6);
        p.line_segment([from, to], egui::Stroke::new(stroke_w, SUP_COL));
        // one cone at the top of each branch
        let lift = i as f32 * 0.0;
        p.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(*x - tip_low_r, top + tip_h + lift),
                egui::pos2(*x + tip_low_r, top + tip_h + lift),
                egui::pos2(*x + tip_up_r, top + lift),
                egui::pos2(*x - tip_up_r, top + lift),
            ],
            SUP_COL,
            egui::Stroke::NONE,
        ));
    }

    // the plate: a slab with a chamfer on both sides
    let chamfer = (s.base.chamfer_deg.to_radians().tan() * plate_h).clamp(1.0, plate_h);
    p.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(mid - plate_r, plate_top),
            egui::pos2(mid + plate_r, plate_top),
            egui::pos2(mid + plate_r - chamfer, bottom),
            egui::pos2(mid - plate_r + chamfer, bottom),
        ],
        SUP_COL,
        egui::Stroke::NONE,
    ));

    // a couple of labels so the picture explains itself
    p.text(
        egui::pos2(rect.left() + 8.0, top - 2.0),
        egui::Align2::LEFT_TOP,
        format!(
            "cone Ø{:.1}/{:.1}",
            s.tip.upper_diameter, s.tip.lower_diameter
        ),
        egui::FontId::proportional(10.0),
        MUTED,
    );
    p.text(
        egui::pos2(rect.right() - 8.0, bottom - 14.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("plate {:.1} mm", s.base.plate_thickness),
        egui::FontId::proportional(10.0),
        MUTED,
    );
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Every STL or OBJ file directly inside `dir`.
fn folder_models(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if ext == "stl" || ext == "obj" {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn default_export_dir() -> String {
    match home_dir() {
        Some(h) => h.join("resin-job").display().to_string(),
        None => "resin-job".to_string(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_tool_carries_its_own_name_glyph_and_key() {
        let mut keys: Vec<&str> = Vec::new();
        for tool in super::Tool::ALL {
            assert!(
                !tool.label().is_empty(),
                "{tool:?} has no label for the strip"
            );
            assert!(!tool.hint().is_empty(), "{tool:?} has no hint for the tip");
            let key = tool.shortcut().name();
            assert!(
                tool.tip().contains(key),
                "{tool:?} does not name its key {key} in {:?}",
                tool.tip()
            );
            assert!(!keys.contains(&key), "two tools answer to {key}");
            keys.push(key);
        }
    }

    #[test]
    fn a_command_key_is_not_its_bare_letter() {
        let mut plain = egui::InputState::default();
        plain.events.push(egui::Event::Key {
            key: egui::Key::O,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        assert!(
            !super::OPEN.pressed(&plain),
            "a bare O must not open a file"
        );
        let mut with_command = plain.clone();
        with_command.modifiers.command = true;
        assert!(super::OPEN.pressed(&with_command));
    }

    use super::*;

    /// A window with the sample pieces on the plate.
    fn app_with_a_piece() -> App {
        let mut app = App::new(Launch::default());
        app.add_sample_scene();
        app.tool = Tool::Move;
        app
    }

    fn assert_world_matches_models(app: &App) {
        for (entry, world) in app.models.iter().zip(&app.world) {
            let (want_min, want_max) = entry.mesh.transformed(&entry.transform).bounds();
            let (min, max) = world.bounds();
            assert!((min - want_min).length() < 1e-3, "stale rendered minimum");
            assert!((max - want_max).length() < 1e-3, "stale rendered maximum");
        }
    }

    #[test]
    fn a_ring_lands_on_the_snap_step() {
        assert_eq!(
            snap_angle(37.4, true, 5.0),
            35.0,
            "37.4 rounds to the 5° step"
        );
        assert_eq!(snap_angle(37.4, false, 5.0), 37.4, "no snap, no rounding");
        assert_eq!(snap_angle(-37.4, true, 5.0), -35.0, "the sign is kept");
        let guarded = snap_angle(37.4, true, 0.0);
        assert!(
            guarded.is_finite(),
            "a zero step must not divide by nothing"
        );
    }

    #[test]
    fn dropping_only_lowers_what_is_picked() {
        let mut app = app_with_a_piece();
        app.models[0].transform.pos.z += 12.0;
        app.models[1].transform.pos.z += 12.0;
        app.refresh_world();
        app.selection = Selection::single(0);
        app.drop_to_plate();
        assert!(
            app.models[0].bounds().0.z.abs() < 1e-3,
            "the picked piece lands"
        );
        assert!(
            (app.models[1].bounds().0.z - 12.0).abs() < 1e-3,
            "a piece nobody picked keeps its height"
        );
    }

    #[test]
    fn auto_layout_updates_the_rendered_models_not_only_the_transforms() {
        let mut app = app_with_a_piece();
        for model in &mut app.models {
            model.transform.pos = Vec3::new(20.0, 15.0, 9.0);
        }
        app.refresh_world();
        app.arrange_all(false);
        assert_world_matches_models(&app);
        let a = layout::bounds_centre(app.world[0].bounds());
        let b = layout::bounds_centre(app.world[1].bounds());
        assert!((a - b).length() > 1.0, "the pieces were not arranged");
        assert!(app.world.iter().all(|m| m.bounds().0.z.abs() < 1e-3));
    }

    #[test]
    fn centring_updates_the_rendered_model_not_only_the_gizmo() {
        let mut app = app_with_a_piece();
        app.selection = Selection::single(0);
        app.models[0].transform.pos = Vec3::new(40.0, -20.0, 12.0);
        app.refresh_world();
        app.centre_selected();
        assert_world_matches_models(&app);
        let (min, max) = app.world[0].bounds();
        let centre = layout::bounds_centre((min, max));
        assert!(centre.x.abs() < 1e-3, "rendered X is {centre:?}");
        assert!(centre.y.abs() < 1e-3, "rendered Y is {centre:?}");
        assert!(min.z.abs() < 1e-3, "rendered piece is not on the plate");
    }

    #[test]
    fn alignment_and_spacing_update_all_selected_rendered_models() {
        let mut app = app_with_a_piece();
        app.selection.only(0..app.models.len());
        app.align_picked(0, layout::Alignment::Centre);
        assert_world_matches_models(&app);
        let a = layout::bounds_centre(app.world[0].bounds());
        let b = layout::bounds_centre(app.world[1].bounds());
        assert!((a.x - b.x).abs() < 1e-3);

        app.distribute_picked(0, true);
        assert_world_matches_models(&app);
        let mut boxes = [app.world[0].bounds(), app.world[1].bounds()];
        boxes.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
        assert!((boxes[0].1.x + app.layout_gap - boxes[1].0.x).abs() < 1e-3);
    }

    #[test]
    fn undo_and_redo_restore_models_settings_and_rendered_geometry() {
        let mut app = app_with_a_piece();
        app.history = Some(project::History::new(app.project()));
        let original = app.models[0].transform;
        let exposure = app.settings.exposure;
        app.models[0].transform.pos.x += 25.0;
        app.settings.exposure = 7.0;
        app.touched();
        app.undo();
        assert_eq!(app.models[0].transform, original);
        assert_eq!(app.settings.exposure, exposure);
        assert_world_matches_models(&app);
        assert!(app.can_redo());
        app.redo();
        assert!((app.models[0].transform.pos.x - original.pos.x - 25.0).abs() < 1e-4);
        assert_eq!(app.settings.exposure, 7.0);
        assert_world_matches_models(&app);
    }

    #[test]
    fn opening_a_self_contained_project_restores_meshes_and_resets_history() {
        let mut app = app_with_a_piece();
        app.settings.exposure = 4.25;
        app.export.stem = "case-project".into();
        app.selection.only(0..app.models.len());
        let path = std::env::temp_dir().join("resin_slicer_ui_project.rslice");
        app.project().save(&path).unwrap();
        app.models.clear();
        app.refresh_world();
        app.load_path(&path);
        assert_eq!(app.models.len(), 2);
        assert_eq!(app.settings.exposure, 4.25);
        assert_eq!(app.export.stem, "case-project");
        assert_eq!(app.selection.len(), 2);
        assert_eq!(app.project_path.as_deref(), Some(path.as_path()));
        assert!(!app.can_undo());
        assert_world_matches_models(&app);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn background_project_save_and_open_round_trip() {
        let mut app = app_with_a_piece();
        app.export.stem = "background-project".into();
        let path = std::env::temp_dir().join("resin_slicer_background_project.rslice");
        app.start_project_save(path.clone());
        for _ in 0..200 {
            app.poll_project();
            if app.project_job.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(app.project_job.is_none(), "save did not finish");
        assert!(path.exists());
        app.models.clear();
        app.refresh_world();
        app.start_project_open(path.clone());
        for _ in 0..200 {
            app.poll_project();
            if app.project_job.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(app.project_job.is_none(), "open did not finish");
        assert_eq!(app.models.len(), 2);
        assert_eq!(app.export.stem, "background-project");
        assert!(!app.can_undo());
        assert_world_matches_models(&app);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cancelling_or_undoing_a_live_drag_finalizes_its_geometry_change() {
        let mut app = app_with_a_piece();
        app.history = Some(project::History::new(app.project()));
        let original = app.models[0].transform;
        app.grab_handles(gizmo::Handle::Axis(0));
        app.drag_handles(gizmo::Change::Move(Vec3::new(18.0, 0.0, 0.0)));
        let revision = app.revision;
        app.cancel_gesture();
        assert!(app.revision > revision);
        assert!(app.can_undo());
        app.undo();
        assert_eq!(app.models[0].transform, original);
        assert_world_matches_models(&app);
    }

    #[test]
    fn a_background_open_never_discards_edits_made_while_loading() {
        let mut app = app_with_a_piece();
        let path = std::env::temp_dir().join("resin_slicer_open_race.rslice");
        app.project().save(&path).unwrap();
        app.start_project_open(path.clone());
        app.models.clear();
        app.touched();
        for _ in 0..200 {
            app.poll_project();
            if app.project_job.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(app.models.is_empty(), "the live edit was overwritten");
        assert!(app.status.contains("was not opened"));
        assert!(app.can_undo());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_noop_layout_action_preserves_redo() {
        let mut app = app_with_a_piece();
        app.history = Some(project::History::new(app.project()));
        app.models[0].transform.pos.x += 12.0;
        app.touched();
        app.undo();
        assert!(app.can_redo());
        app.selection.only(0..app.models.len());
        // Two outer centres are already evenly distributed by definition.
        app.distribute_picked(0, false);
        assert!(app.can_redo());
        app.redo();
        assert!(app.models[0].transform.pos.x >= 12.0);
    }

    #[test]
    fn the_gap_is_a_floor_not_a_ceiling() {
        assert_eq!(
            supports::gap_rise(0.0, 6.0),
            6.0,
            "a piece on the plate rises"
        );
        assert_eq!(
            supports::gap_rise(12.0, 6.0),
            0.0,
            "a raised piece is left alone"
        );
        assert_eq!(
            supports::gap_rise(-2.0, 6.0),
            8.0,
            "a sunken piece rises further"
        );
        assert_eq!(
            supports::gap_rise(0.0, -4.0),
            0.0,
            "a negative gap is no gap"
        );
    }

    #[test]
    fn the_gizmo_sits_on_the_middle_of_the_piece() {
        let app = app_with_a_piece();
        let g = app
            .gizmo_for()
            .expect("a piece is picked, so there is a gizmo");
        let idx = app.selection.primary().unwrap();
        let m = &app.models[idx];
        let want = layout::bounds_centre(m.bounds());
        assert!(
            (g.origin - want).length() < 1e-3,
            "handles at {:?}, piece centre {want:?}",
            g.origin
        );
    }

    #[test]
    fn the_gizmo_follows_the_piece_when_it_is_moved() {
        let mut app = app_with_a_piece();
        app.models[0].transform.pos = Vec3::new(-60.0, 25.0, 0.0);
        app.refresh_world();
        let g = app.gizmo_for().unwrap();
        let (min, max) = app.models[0]
            .mesh
            .transformed(&app.models[0].transform)
            .bounds();
        assert!(
            (g.origin - layout::bounds_centre((min, max))).length() < 1e-3,
            "the handles have to travel with the piece"
        );
        // and once both pieces are apart, the gizmo is on the picked one
        app.selection = Selection::single(1);
        let other = app.gizmo_for().unwrap();
        assert!((other.origin - g.origin).length() > 20.0);
    }

    #[test]
    fn the_gizmo_sits_on_the_middle_of_what_is_picked() {
        let mut app = app_with_a_piece();
        app.selection.only(0..app.models.len());
        let g = app.gizmo_for().unwrap();
        // the handles sit on the average of the pieces' own centres: the very
        // points a turn or a resize holds still, so the ring you grab is the
        // middle the piece turns about
        let mut sum = Vec3::ZERO;
        for m in &app.models {
            sum = sum + m.transform.apply(layout::bounds_centre(m.mesh.bounds()));
        }
        let want = sum * (1.0 / app.models.len() as f32);
        assert!(
            (g.origin - want).length() < 1e-3,
            "handles at {:?}, middle of what is picked {want:?}",
            g.origin
        );
    }

    /// Put the other pieces out of the way so a click can only mean one thing.
    fn make_room(app: &mut App) {
        for (i, m) in app.models.iter_mut().enumerate().skip(1) {
            m.transform.pos = Vec3::new(300.0 * i as f32, 0.0, 0.0);
        }
        app.refresh_world();
        app.selection = Selection::none();
    }

    fn click_at(app: &mut App, at: Vec3) {
        let (w, h) = (900.0_f32, 700.0_f32);
        let view = render::View::new(&app.cam, w, h);
        let (x, y, _) = view
            .project(at)
            .expect("that point is in front of the camera");
        app.click_in_view(egui::pos2(x, y), w, h, false);
    }

    #[test]
    fn clicking_the_face_of_a_piece_picks_it() {
        let mut app = app_with_a_piece();
        make_room(&mut app);
        let centre = app.models[0]
            .transform
            .apply(layout::bounds_centre(app.models[0].mesh.bounds()));
        click_at(&mut app, centre);
        assert_eq!(
            app.selection.primary(),
            Some(0),
            "clicking a face has to pick the piece that is there"
        );
    }

    #[test]
    fn clicking_the_empty_plate_clears_the_pick() {
        let mut app = app_with_a_piece();
        make_room(&mut app);
        app.selection = Selection::single(0);
        click_at(&mut app, Vec3::new(-400.0, -380.0, 0.0));
        assert!(app.selection.is_empty(), "a click on nothing picks nothing");
    }

    #[test]
    fn turning_a_handle_holds_the_piece_in_place() {
        let mut app = app_with_a_piece();
        make_room(&mut app);
        app.selection = Selection::single(0);
        app.tool = Tool::Turn;
        let centre = layout::bounds_centre(app.models[0].mesh.bounds());
        let before = app.models[0].transform.apply(centre);
        app.grab_handles(gizmo::Handle::Axis(2));
        app.drag_handles(gizmo::Change::Turn(35.0));
        let after = app.models[0].transform.apply(centre);
        assert!(
            (after - before).length() < 1e-2,
            "the middle must not wander: {before:?} -> {after:?}"
        );
        assert!(
            app.models[0].transform.rot_deg.z.abs() > 1.0,
            "the piece still has to turn"
        );
    }

    #[test]
    fn turning_a_group_spins_each_piece_about_its_own_pivot() {
        let mut app = app_with_a_piece();
        app.models[0].transform.rot_deg = Vec3::new(10.0, 20.0, 30.0);
        app.models[1].transform.rot_deg = Vec3::new(-5.0, 15.0, 45.0);
        app.selection.only(0..app.models.len());
        app.tool = Tool::Turn;
        let centres = |app: &App| {
            app.models
                .iter()
                .map(|m| m.transform.apply(layout::bounds_centre(m.mesh.bounds())))
                .collect::<Vec<_>>()
        };
        let before = centres(&app);
        let rotations = app
            .models
            .iter()
            .map(|m| m.transform.rot_deg)
            .collect::<Vec<_>>();
        assert!((before[0] - before[1]).length() > 1.0);
        app.grab_handles(gizmo::Handle::Axis(2));
        app.drag_handles(gizmo::Change::Turn(25.0));
        let after = centres(&app);
        for (i, (&old, &new)) in before.iter().zip(after.iter()).enumerate() {
            assert!(
                (new - old).length() < 0.02,
                "piece {i} moved off its own pivot: {old:?} -> {new:?}"
            );
            let turned = app.models[i].transform.rot_deg;
            assert!(
                (turned.z - rotations[i].z - 25.0).abs() < 1e-3,
                "piece {i} did not receive the same 25 degree turn"
            );
            assert!((turned.x - rotations[i].x).abs() < 1e-3);
            assert!((turned.y - rotations[i].y).abs() < 1e-3);
        }
    }

    #[test]
    fn numeric_turning_applies_the_same_delta_to_every_piece() {
        let mut app = app_with_a_piece();
        app.selection.only(0..app.models.len());
        let before = app
            .models
            .iter()
            .map(|m| {
                (
                    m.transform.apply(layout::bounds_centre(m.mesh.bounds())),
                    m.transform.rot_deg,
                )
            })
            .collect::<Vec<_>>();
        app.turn_selected_by(Vec3::new(25.0, 0.0, 0.0));
        for (i, m) in app.models.iter().enumerate() {
            let centre = m.transform.apply(layout::bounds_centre(m.mesh.bounds()));
            assert!((centre - before[i].0).length() < 0.02);
            assert!((m.transform.rot_deg.x - before[i].1.x - 25.0).abs() < 1e-3);
            assert!((m.transform.rot_deg.y - before[i].1.y).abs() < 1e-3);
            assert!((m.transform.rot_deg.z - before[i].1.z).abs() < 1e-3);
        }
    }

    #[test]
    fn dragging_a_handle_moves_the_piece_the_way_the_arrow_points() {
        let mut app = app_with_a_piece();
        app.tool = Tool::Move;
        let before = app.models[0].transform.pos;
        app.grab_handles(gizmo::Handle::Axis(0));
        app.drag_handles(gizmo::Change::Move(Vec3::new(12.0, 0.0, 0.0)));
        let after = app.models[0].transform.pos;
        assert!(
            after.x > before.x + 1.0,
            "X went {} -> {}",
            before.x,
            after.x
        );
        assert!((after.y - before.y).abs() < 1e-3, "only X should change");
        assert!((after.z - before.z).abs() < 1e-3, "only X should change");
    }

    #[test]
    fn a_drag_is_measured_from_where_it_started_not_frame_by_frame() {
        let mut app = app_with_a_piece();
        app.tool = Tool::Move;
        let x0 = app.models[0].transform.pos.x;
        app.grab_handles(gizmo::Handle::Axis(0));
        // the pointer reports the whole travel each frame, as `gizmo::change`
        // does, so the piece must land on 20, not on 10 + 20
        app.drag_handles(gizmo::Change::Move(Vec3::new(10.0, 0.0, 0.0)));
        app.drag_handles(gizmo::Change::Move(Vec3::new(20.0, 0.0, 0.0)));
        assert!((app.models[0].transform.pos.x - x0 - 20.0).abs() < 1e-3);
        // letting go and starting again starts from the new place
        app.grab = None;
        app.grab_handles(gizmo::Handle::Axis(0));
        app.drag_handles(gizmo::Change::Move(Vec3::new(5.0, 0.0, 0.0)));
        assert!((app.models[0].transform.pos.x - x0 - 25.0).abs() < 1e-3);
    }

    #[test]
    fn turning_a_handle_spins_the_piece_and_scaling_resizes_it() {
        let mut app = app_with_a_piece();

        app.tool = Tool::Turn;
        app.grab_handles(gizmo::Handle::Axis(1));
        app.drag_handles(gizmo::Change::Turn(30.0));
        assert!((app.models[0].transform.rot_deg.y - 30.0).abs() < 1e-3);

        app.tool = Tool::Size;
        let before = app.models[0].transform.scale.x;
        app.grab_handles(gizmo::Handle::Axis(0));
        app.drag_handles(gizmo::Change::Size(1.5));
        let after = app.models[0].transform.scale.x;
        assert!((after - before * 1.5).abs() < 1e-3, "{before} -> {after}");
        assert!(after <= 5.0, "and stay within the slider's range");
    }

    #[test]
    fn scaling_a_group_expands_about_the_common_pivot() {
        let mut app = app_with_a_piece();
        app.selection.only(0..app.models.len());
        app.tool = Tool::Size;
        let centres = |app: &App| {
            app.models
                .iter()
                .map(|m| m.transform.apply(layout::bounds_centre(m.mesh.bounds())))
                .collect::<Vec<_>>()
        };
        let before = centres(&app);
        let pivot = before.iter().fold(Vec3::ZERO, |sum, p| sum + *p) * (1.0 / before.len() as f32);
        app.grab_handles(gizmo::Handle::Axis(0));
        app.drag_handles(gizmo::Change::Size(1.25));
        for (&old, &new) in before.iter().zip(centres(&app).iter()) {
            let want = pivot + (old - pivot) * 1.25;
            assert!(
                (new - want).length() < 0.02,
                "{old:?} -> {new:?}, want {want:?}"
            );
        }
    }

    #[test]
    fn a_marquee_picks_everything_it_covers_and_a_click_takes_one() {
        let mut app = app_with_a_piece();
        app.refresh_world();
        let view = render::View::new(&app.cam, 900.0, 700.0);
        let all = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(900.0, 700.0));
        app.select_in(all, &view, false);
        assert_eq!(app.selection.len(), 2, "the whole view holds both pieces");
        // click straight at the middle of the first piece
        let m = &app.models[0];
        let (min, max) = m.bounds();
        let (sx, sy, _) = view.project(layout::bounds_centre((min, max))).unwrap();
        app.click_in_view(egui::pos2(sx, sy), 900.0, 700.0, false);
        assert_eq!(app.selection.len(), 1, "a click takes one piece");
        // an empty box clears it
        let nothing = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        app.select_in(nothing, &view, false);
        assert!(app.selection.is_empty());
    }

    /// The window opens on an empty plate; the sample scene is opt-in, so a
    /// launch never has pieces nobody asked for.
    #[test]
    fn a_fresh_window_holds_nothing_until_it_is_asked_to() {
        assert!(App::new(Launch::default()).models.is_empty());
        assert_eq!(
            App::new(Launch {
                demo: true,
                ..Default::default()
            })
            .models
            .len(),
            2
        );
    }

    /// A picture of a layer is only built from a mask that fills every texel.
    /// Building one from an empty mask panicked the window the first time the
    /// Slice step was opened on a plate that had not been cut yet.
    #[test]
    fn every_texel_of_the_layer_picture_has_a_value() {
        let empty = App::new(Launch::default());
        assert!(empty.layer_mask(40, 20).is_none());

        let mut app = app_with_a_piece();
        app.reslice();
        let mask = app
            .layer_mask(40, 20)
            .expect("a cut plate has a layer to show");
        assert_eq!(mask.len(), 40 * 20);
    }

    /// Press, drag and release on a drawn handle, through the same input layer
    /// the window uses, so the three parts of the gesture are checked together.
    #[test]
    fn a_whole_gesture_moves_the_piece_through_the_input_layer() {
        let mut app = app_with_a_piece();
        app.tool = Tool::Move;
        app.selection = Selection::single(0);
        let view = render::View::new(&app.cam, 900.0, 700.0);
        let g = app.gizmo_for().expect("a picked piece has handles");
        let before = app.models[0].transform.pos;
        let stamp = app.revision;

        let mut input = input::Input::default();
        let mut pointer = input::Pointer {
            hovering: true,
            ..Default::default()
        };
        // half way along the X stem, as it is drawn
        let (a, b) = g
            .handles(&view)
            .into_iter()
            .find(|h| h.handle == gizmo::Handle::Axis(0))
            .map(|h| match h.shape {
                gizmo::Shape::Stem(a, b) => (a, b),
                _ => panic!("a move gizmo draws stems"),
            })
            .expect("an X handle");
        let press = a + (b - a) * 0.5;
        let along = (b - a).normalized();

        pointer.pos = Some(press);
        pointer.primary_pressed = true;
        for action in input.update(pointer, Some(&g), &view) {
            match action {
                input::Action::Grab { handle, .. } => app.grab_handles(handle),
                other => panic!("the press should only grab, got {other:?}"),
            }
        }
        assert!(app.grab.is_some());

        pointer.primary_pressed = false;
        for step in 1..=5 {
            pointer.pos = Some(press + along * (12.0 * step as f32));
            for action in input.update(pointer, Some(&g), &view) {
                match action {
                    input::Action::Drag { change } => app.drag_handles(change),
                    other => panic!("a held handle only drags, got {other:?}"),
                }
            }
        }
        let moved = app.models[0].transform.pos - before;
        assert!(moved.x > 1.0, "the piece follows the X arrow: {moved:?}");
        assert!(moved.y.abs() < 1e-3 && moved.z.abs() < 1e-3, "{moved:?}");
        assert_eq!(
            app.revision, stamp,
            "a held handle does not reach the supports"
        );

        pointer.primary_released = true;
        for action in input.update(pointer, Some(&g), &view) {
            match action {
                input::Action::Release => {
                    if app.grab.take().is_some() {
                        app.touched();
                    }
                }
                other => panic!("letting go only releases, got {other:?}"),
            }
        }
        assert!(app.grab.is_none());
        assert_eq!(app.revision, stamp + 1, "letting go is one plate change");
    }

    /// Supports belong to the plate they were grown for. Moving a piece leaves
    /// them where they were standing (and keeps them, so nothing has to be
    /// rebuilt while the hand is still moving); one run brings them back up to
    /// date.
    #[test]
    fn a_plate_change_leaves_the_supports_behind_until_the_next_run() {
        let mut app = app_with_a_piece();
        // The waiting form of the run is what a photographed start uses, and it
        // is also what lets the test do without a worker thread.
        app.start_supports_and_wait();
        assert!(!app.supports_stale(), "a run catches up with the plate");
        assert!(!app.slice_stale(), "and slices what it grew");
        let grown = app.support.tips.len();
        assert!(
            grown > 0,
            "the sample scene overhangs, so a run has work to do"
        );

        app.selection = Selection::single(0);
        app.models[0].transform.pos.x += 20.0;
        app.refresh_world();
        app.touched();
        assert!(
            app.supports_stale(),
            "the supports did not follow the piece"
        );
        assert!(app.slice_stale());
        assert!(app.out_of_date());
        assert_eq!(app.support.tips.len(), grown, "nothing was thrown away");

        app.start_supports_and_wait();
        assert!(!app.out_of_date(), "the second run is up to date again");
    }

    #[test]
    fn duplicating_and_mirroring_keep_the_pieces_apart() {
        let mut app = app_with_a_piece();
        app.selection = Selection::single(0);
        app.duplicate_selected();
        assert_eq!(
            app.models.len(),
            3,
            "the copy is added next to the original"
        );
        assert_eq!(
            app.selection.primary(),
            Some(2),
            "and it becomes the picked piece"
        );
        let a = app.models[0].transform;
        let b = app.models[2].transform;
        assert!(
            (a.pos - b.pos).length() > 1.0,
            "the copy must not sit exactly on top of the original"
        );
        assert!(
            app.models[2].mesh.name.ends_with("copy"),
            "the copy is named so it can be told apart, got {}",
            app.models[2].mesh.name
        );

        app.mirror_selected(0);
        assert!(app.models[2].transform.mirror.x);
        app.mirror_selected(0);
        assert!(
            !app.models[2].transform.mirror.x,
            "mirroring twice undoes it"
        );
    }

    #[test]
    fn duplicate_mirror_and_remove_act_on_every_picked_piece() {
        let mut app = app_with_a_piece();
        app.selection.only(0..app.models.len());
        app.duplicate_selected();
        assert_eq!(app.models.len(), 4, "one copy for each picked piece");
        assert_eq!(app.selection.len(), 2, "the copies are what is picked now");

        app.selection.only(0..4);
        app.mirror_selected(1);
        assert!(app.models.iter().all(|m| m.transform.mirror.y));

        app.selection.only(1..3);
        app.delete_selected();
        assert_eq!(app.models.len(), 2);
        assert_eq!(app.selection.len(), 0, "nothing picked points at nothing");
        app.delete_selected();
        assert_eq!(
            app.models.len(),
            2,
            "and an empty selection removes nothing"
        );
    }

    #[test]
    fn deleting_during_a_drag_cancels_its_frozen_model_indices() {
        let mut app = app_with_a_piece();
        app.selection = Selection::single(0);
        app.tool = Tool::Move;
        app.grab_handles(gizmo::Handle::Axis(0));
        let pointer = input::Pointer {
            pos: Some(egui::pos2(20.0, 20.0)),
            primary_pressed: true,
            hovering: true,
            ..Default::default()
        };
        let view = render::View::new(&app.cam, 900.0, 700.0);
        app.input.update(pointer, None, &view);
        assert!(app.input.is_dragging());
        app.delete_selected();
        assert!(!app.input.is_dragging());
        assert!(app.grab.is_none());
        let survivor = app.models[0].transform;
        app.drag_handles(gizmo::Change::Move(Vec3::new(50.0, 0.0, 0.0)));
        assert_eq!(app.models[0].transform, survivor);
    }

    #[test]
    fn removing_the_last_piece_leaves_a_usable_window() {
        let mut app = app_with_a_piece();
        app.tool = Tool::Move;
        app.selection = Selection::single(1);
        app.delete_selected();
        assert_eq!(app.models.len(), 1);
        assert!(app.gizmo_for().is_none(), "nothing is picked any more");
        app.selection = Selection::single(0);
        assert!(app.gizmo_for().is_some());
        app.delete_selected();
        assert!(app.models.is_empty());
        assert!(app.gizmo_for().is_none(), "nothing left to grab");
        assert!(app.selection.is_empty());
    }
}

#[cfg(test)]
mod cost {
    use super::*;
    use std::time::Instant;

    /// A sphere with `detail * detail * 2` triangles, for weight.
    fn heavy_ball(detail: usize, r: f32) -> Mesh {
        let mut m = Mesh::default();
        let at = |i: usize, j: usize, m: &mut Mesh| {
            let th = std::f32::consts::PI * i as f32 / detail as f32;
            let ph = 2.0 * std::f32::consts::PI * j as f32 / detail as f32;
            m.verts.push(Vec3::new(
                r * th.sin() * ph.cos(),
                r * th.sin() * ph.sin(),
                r * th.cos(),
            ));
            (m.verts.len() - 1) as u32
        };
        for i in 0..detail {
            for j in 0..detail {
                let a = at(i, j, &mut m);
                let b = at(i + 1, j, &mut m);
                let c = at(i + 1, j + 1, &mut m);
                let d = at(i, j + 1, &mut m);
                m.tris.push([a, b, c]);
                m.tris.push([a, c, d]);
            }
        }
        m
    }

    /// One solid frame of the plate at `w` x `h`, in milliseconds.
    fn frame(
        app: &App,
        w: f32,
        h: f32,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        gpu: &mut gpu_render::GpuRenderer,
    ) -> f64 {
        let build = [192.0, 120.0, 200.0];
        let t = Instant::now();
        for _ in 0..10 {
            let view = render::View::new(&app.cam, w, h);
            let scene = render::Scene {
                models: &app.world,
                selection: &[],
                support_mesh: None,
                overhang: None,
                islands: &[],
                build,
                mode: ViewMode::Solid,
                slice: None,
                current_layer: 0,
                show_plate: w > 64.0,
            };
            gpu.draw(
                device,
                queue,
                &scene,
                &view,
                [w as u32, h as u32],
                gpu_render::SceneRevision::default(),
            );
        }
        let _ = device.poll(wgpu::Maintain::Wait);
        t.elapsed().as_secs_f64() * 100.0
    }

    /// A rough, printed measurement of what an interactive frame costs.
    /// `cargo test --release interactive_cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn interactive_cost_with_a_heavy_plate() {
        let mut app = App::new(Launch::default());
        app.models.clear();
        let detail: usize = std::env::var("DETAIL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(120);
        for i in 0..10 {
            let mut ball = heavy_ball(detail, 20.0);
            let step = (i % 5) as f32 * 34.0 - 68.0;
            let row = (i / 5) as f32 * 46.0 - 23.0;
            ball.translate(Vec3::new(step, row, 0.0));
            app.models.push(ModelEntry::new(ball, Transform::default()));
        }
        let tris: usize = app.models.iter().map(|m| m.mesh.tris.len()).sum();
        println!("\n{tris} triangles in {} pieces", app.models.len());

        let t = Instant::now();
        for _ in 0..10 {
            app.refresh_world();
        }
        println!(
            "refresh_world      {:>8.2} ms",
            t.elapsed().as_secs_f64() * 100.0
        );

        let t = Instant::now();
        for i in 0..10 {
            app.models[0].transform.pos.x += 0.5;
            app.refresh_world();
            let _ = app.models[i].bounds();
        }
        println!(
            "drag one piece     {:>8.2} ms",
            t.elapsed().as_secs_f64() * 100.0
        );

        let Some((device, queue)) = gpu_render::headless_device() else {
            println!("no Vulkan adapter for the GPU benchmark");
            return;
        };
        let mut gpu = gpu_render::GpuRenderer::new(&device);
        println!(
            "GPU full frame     {:>8.2} ms",
            frame(&app, 880.0, 700.0, &device, &queue, &mut gpu)
        );
        println!(
            "GPU small frame    {:>8.2} ms",
            frame(&app, 16.0, 16.0, &device, &queue, &mut gpu)
        );

        let cfg = app.settings.supports.clone();
        let t = Instant::now();
        let job = supports::start(app.world.clone(), cfg);
        let done = loop {
            if let Ok(v) = job.rx.try_recv() {
                break v;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        let done = match done {
            supports::JobMsg::Done(m) => m,
            supports::JobMsg::Failed(why) => panic!("{:?}", why),
        };
        println!(
            "supports::start    {:>8.2} ms ({} tips)",
            t.elapsed().as_secs_f64() * 1000.0,
            done.tips.len()
        );
    }
}
