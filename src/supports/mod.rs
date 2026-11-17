//! Automatic support generation, in stages: detect, place tips, route
//! branches and trunks, add strength, put a base plate under it all.
//!
//! The result is a structured model (tips, branches, trunks, feet, braces,
//! plates), not just a mesh, so a stage can be changed without disturbing the
//! others and a run can be repeated from settings alone.
//!
//! Some helpers here are only reached from the tests or kept as a small API
//! for the next stage, so dead-code notes are switched off for the module.
#![allow(dead_code)]

pub mod analysis;
pub mod base;
pub mod bvh;
pub mod config;
pub mod detect;
pub mod meshgen;
pub mod route;
pub mod shadow;
pub mod strength;
pub mod tips;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Instant;

pub use config::{ContactShape, Preset, SupportConfig, TipShape};

use crate::mesh::{Mesh, Vec3};

#[derive(Clone, Debug)]
pub struct Tip {
    pub id: usize,
    /// The point on the model surface that is touched.
    pub pos: Vec3,
    /// Unit direction from the surface into the material.
    pub axis: Vec3,
    pub length: f32,
    pub upper_d: f32,
    pub lower_d: f32,
    pub shape: TipShape,
    pub contact_d: f32,
    pub contact_shape: ContactShape,
    pub contact_depth: f32,
    pub island: bool,
    /// Which model on the plate this tip belongs to.  Supports are grown
    /// per model, so the trunk, foot and base plate that serve it stay with
    /// the same model instead of merging across the whole build area.
    pub model_id: usize,
}

impl Tip {
    /// Where the tip ends, away from the model.
    pub fn lower_end(&self) -> Vec3 {
        self.pos + self.axis * self.contact_depth - self.axis * self.length
    }
    pub fn upper_end(&self) -> Vec3 {
        self.pos + self.axis * self.contact_depth
    }
}

#[derive(Clone, Debug)]
pub struct Branch {
    pub id: usize,
    /// From the tip's far end.
    pub from: Vec3,
    /// Optional clear-exit point. A dogleg follows the tip axis to this point
    /// before turning towards the trunk, avoiding a wall beside the contact.
    pub via: Option<Vec3>,
    /// To the merge point on the trunk.
    pub to: Vec3,
    pub diameter: f32,
    pub tip_id: usize,
}

#[derive(Clone, Debug)]
pub struct Trunk {
    pub id: usize,
    pub bottom: Vec3,
    pub top: Vec3,
    pub diameter: f32,
    pub tip_ids: Vec<usize>,
    /// The model this trunk serves; its foot and base plate stay local.
    pub model_id: usize,
}

/// A foot on the platform, where a trunk standing there meets the build plate.
///
/// A trunk held from the model itself gets no foot: there is nothing to spread
/// over, and it is bonded straight into the surface instead.
#[derive(Clone, Debug)]
pub struct Foot {
    pub id: usize,
    /// The point on the platform the foot stands on, so `z` is zero.
    pub pos: Vec3,
    pub diameter: f32,
    pub trunk_id: usize,
    /// The model this foot stands under; the base plate is split by this.
    pub model_id: usize,
}

#[derive(Clone, Debug)]
pub struct Brace {
    pub id: usize,
    pub a: Vec3,
    pub b: Vec3,
    pub diameter: f32,
    /// Braces only join trunks belonging to this one model.
    pub model_id: usize,
}

/// A chamfered plate under a group of feet.
#[derive(Clone, Debug)]
pub struct Plate {
    /// The model whose feet this base plate carries.
    pub model_id: usize,
    /// Convex outline, counter-clockwise.
    pub poly: Vec<(f32, f32)>,
    pub thickness: f32,
    /// Vertical rise and horizontal run of the top-edge chamfer.
    pub chamfer: f32,
    pub chamfer_run: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub overhang_faces: usize,
    pub regions: usize,
    pub islands: usize,
    pub islands_supported: usize,
    pub candidate_points: usize,
    pub tips: usize,
    pub tips_skipped: usize,
    pub coverage_added: usize,
    pub trunks: usize,
    pub branches: usize,
    pub braces: usize,
    pub feet: usize,
    pub plates: usize,
    /// How each cluster's trunk was found a clear path.
    pub direct: usize,
    pub shifted: usize,
    pub split: usize,
    pub on_model: usize,
    pub dropped: usize,
    pub ms: f64,
    pub trunk_min: f32,
    pub trunk_max: f32,
}

#[derive(Clone, Debug, Default)]
pub struct SupportModel {
    pub tips: Vec<Tip>,
    /// Reverse pinheads bonding model-facing pillars to the part.
    pub anchors: Vec<Tip>,
    pub branches: Vec<Branch>,
    pub trunks: Vec<Trunk>,
    pub feet: Vec<Foot>,
    pub braces: Vec<Brace>,
    pub plates: Vec<Plate>,
    pub stats: Stats,
    /// Overhanging faces of the model, for the red debug view.
    pub debug_mesh: Mesh,
    /// Island positions, for the debug markers.
    pub debug_islands: Vec<Vec3>,
}

impl SupportModel {
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.tips.is_empty() && self.trunks.is_empty()
    }

    /// All element meshes appended into one mesh. No merging is done here: the
    /// slicer's non-zero fill takes care of overlaps.
    pub fn mesh(&self, sides: usize) -> Mesh {
        let mut m = Mesh::default();
        for t in &self.tips {
            meshgen::append(&mut m, &meshgen::tip(t, sides));
        }
        for anchor in &self.anchors {
            meshgen::append(&mut m, &meshgen::tip(anchor, sides));
        }
        for b in &self.branches {
            if let Some(via) = b.via {
                meshgen::append(
                    &mut m,
                    &meshgen::frustum(b.from, b.diameter * 0.5, via, b.diameter * 0.5, sides),
                );
                meshgen::append(
                    &mut m,
                    &meshgen::frustum(via, b.diameter * 0.5, b.to, b.diameter * 0.5, sides),
                );
            } else {
                meshgen::append(
                    &mut m,
                    &meshgen::frustum(b.from, b.diameter * 0.5, b.to, b.diameter * 0.5, sides),
                );
            }
        }
        for t in &self.trunks {
            meshgen::append(
                &mut m,
                &meshgen::frustum(t.bottom, t.diameter * 0.5, t.top, t.diameter * 0.5, sides),
            );
        }
        for f in &self.feet {
            meshgen::append(&mut m, &meshgen::foot(f));
        }
        for b in &self.braces {
            meshgen::append(
                &mut m,
                &meshgen::frustum(b.a, b.diameter * 0.5, b.b, b.diameter * 0.5, sides),
            );
        }
        for p in &self.plates {
            meshgen::append(&mut m, &meshgen::plate(p));
        }
        m
    }

    /// Throw away everything that was grown. Supports are only ever built by
    /// a run of the generator, so there is nothing to keep.
    pub fn clear(&mut self) {
        *self = SupportModel::default();
    }

    /// Remove or renumber whole models' support trees after plate edits. The
    /// map is indexed by the old model id; `None` deletes that model's parts,
    /// while `Some(new_id)` preserves them under their new plate index.
    /// Returns true when support geometry was removed.
    pub fn remap_models(&mut self, map: &[Option<usize>]) -> bool {
        fn mapped(map: &[Option<usize>], old: usize) -> Option<usize> {
            map.get(old).copied().flatten()
        }
        let before = self.tips.len()
            + self.anchors.len()
            + self.branches.len()
            + self.trunks.len()
            + self.feet.len()
            + self.braces.len()
            + self.plates.len();
        // Branch ownership comes from its tip, so it stays consistent without
        // duplicating that relationship in every Branch record.
        let tip_models: HashMap<usize, usize> =
            self.tips.iter().map(|tip| (tip.id, tip.model_id)).collect();
        self.tips.retain_mut(|tip| {
            if let Some(new) = mapped(map, tip.model_id) {
                tip.model_id = new;
                true
            } else {
                false
            }
        });
        self.anchors.retain_mut(|anchor| {
            if let Some(new) = mapped(map, anchor.model_id) {
                anchor.model_id = new;
                true
            } else {
                false
            }
        });
        self.branches.retain(|branch| {
            tip_models
                .get(&branch.tip_id)
                .is_some_and(|old| mapped(map, *old).is_some())
        });
        self.trunks.retain_mut(|trunk| {
            if let Some(new) = mapped(map, trunk.model_id) {
                trunk.model_id = new;
                true
            } else {
                false
            }
        });
        self.feet.retain_mut(|foot| {
            if let Some(new) = mapped(map, foot.model_id) {
                foot.model_id = new;
                true
            } else {
                false
            }
        });
        self.braces.retain_mut(|brace| {
            if let Some(new) = mapped(map, brace.model_id) {
                brace.model_id = new;
                true
            } else {
                false
            }
        });
        self.plates.retain_mut(|plate| {
            if let Some(new) = mapped(map, plate.model_id) {
                plate.model_id = new;
                true
            } else {
                false
            }
        });
        let after = self.tips.len()
            + self.anchors.len()
            + self.branches.len()
            + self.trunks.len()
            + self.feet.len()
            + self.braces.len()
            + self.plates.len();
        let removed = before != after;
        if removed {
            // The red overhang view and island markers are combined geometry,
            // not split by model. Discard them rather than showing old shapes.
            self.debug_mesh = Mesh::default();
            self.debug_islands.clear();
        }
        self.refresh_stats();
        removed
    }

    pub fn refresh_stats(&mut self) {
        self.stats.tips = self.tips.len();
        self.stats.trunks = self.trunks.len();
        self.stats.branches = self.branches.len();
        self.stats.braces = self.braces.len();
        self.stats.feet = self.feet.len();
        self.stats.plates = self.plates.len();
        let mut min = f32::MAX;
        let mut max = 0.0f32;
        for trunk in &self.trunks {
            let len = (trunk.top - trunk.bottom).length();
            min = min.min(len);
            max = max.max(len);
        }
        self.stats.trunk_min = if self.trunks.is_empty() { 0.0 } else { min };
        self.stats.trunk_max = max;
    }

    /// The honest report the person asked for.
    pub fn report(&self) -> String {
        let s = &self.stats;
        let mut out = String::new();
        out.push_str(&format!(
            "tips {}, trunks {}, branches {}, braces {}, feet {}, plates {}\n",
            s.tips, s.trunks, s.branches, s.braces, s.feet, s.plates
        ));
        out.push_str(&format!(
            "overhang faces {}, regions {}, islands {} (supported {})\n",
            s.overhang_faces, s.regions, s.islands, s.islands_supported
        ));
        out.push_str(&format!(
            "tips skipped for clearance {}, added for coverage {}, candidates {}\n",
            s.tips_skipped, s.coverage_added, s.candidate_points
        ));
        out.push_str(&format!(
            "fallbacks: direct {}, shifted {}, split {}, on-model {}, dropped {}\n",
            s.direct, s.shifted, s.split, s.on_model, s.dropped
        ));
        out.push_str(&format!(
            "generation {:.0} ms, trunk length {:.1}..{:.1} mm\n",
            s.ms,
            if s.trunks == 0 { 0.0 } else { s.trunk_min },
            s.trunk_max
        ));
        if self.plates.is_empty() {
            out.push_str("no base plate (switched off)\n");
        } else {
            out.push_str(&format!(
                "base plate {} under the feet\n",
                if self.plates.len() == 1 {
                    "one piece"
                } else {
                    "in pieces"
                }
            ));
        }
        out
    }
}

/// Run every stage. `meshes` must already be lifted by
/// `cfg.height_from_platform` (see `lift`).
pub fn generate(meshes: &[Mesh], cfg: &SupportConfig, cancel: &AtomicBool) -> SupportModel {
    let started = Instant::now();
    let mut model = SupportModel::default();
    if meshes.is_empty() {
        return model;
    }
    let faces = detect::Faces::new(meshes);
    let tree = bvh::Bvh::from_meshes(meshes);
    let t_detect = started.elapsed();
    let det = detect::detect(&faces, meshes, cfg, cancel);
    let t_tips = started.elapsed();
    model.stats.overhang_faces = det.overhang.len();

    // Prusa-style linked-layer contact generation. The same raster also
    // supplies the collision shadow used by the column router.
    let placed = analysis::generate(meshes, cfg, &tree, cancel);
    let t_placed = started.elapsed();
    model.stats.islands = placed.islands;
    model.stats.regions = placed.regions;
    model.debug_islands = placed.debug_islands.clone();
    model.debug_mesh = det.debug_mesh.clone();
    model.stats.candidate_points = placed.candidate_points;
    model.stats.tips_skipped = placed.skipped;
    model.stats.coverage_added = placed.coverage_added;
    model.stats.islands_supported = 0;

    let mut id = 0usize;
    let mut island_groups = Vec::new();
    let mut tips: Vec<Tip> = Vec::new();
    for p in &placed.contacts {
        let mut tip = tips::make_tip(p.pos, p.normal, cfg, p.island);
        tip.id = id;
        if p.island_id > 0 {
            island_groups.push((id, p.island_id));
        }
        tip.model_id = p.model_id;
        id += 1;
        tips.push(tip);
    }
    if tips.is_empty() || cancel.load(Ordering::Relaxed) {
        model.stats.ms = started.elapsed().as_secs_f64() * 1000.0;
        return model;
    }

    let mut pieces = route::route(&tips, cfg, &tree, &mut model.stats);
    let t_routed = started.elapsed();
    for b in &mut pieces.branches {
        b.id = id;
        id += 1;
    }
    strength::strengthen(&mut pieces, cfg, &tree);

    // Routing uses local trunk indices in Foot::trunk_id while the model is
    // being strengthened. Once the public ids are assigned, remap the feet
    // to those ids as well; otherwise deleting the last tip leaves a dangling
    // foot and base plate behind.
    let trunk_ids: Vec<usize> = pieces
        .trunks
        .iter_mut()
        .map(|t| {
            t.id = id;
            id += 1;
            t.id
        })
        .collect();
    for f in &mut pieces.feet {
        if let Some(&trunk_id) = trunk_ids.get(f.trunk_id) {
            f.trunk_id = trunk_id;
        }
        f.id = id;
        id += 1;
    }
    for b in &mut pieces.braces {
        b.id = id;
        id += 1;
    }
    model.tips = pieces.tips;
    model.anchors = pieces.anchors;
    let routed_tip_ids: std::collections::HashSet<usize> =
        model.tips.iter().map(|tip| tip.id).collect();
    model.stats.islands_supported = island_groups
        .iter()
        .filter(|(tip_id, _)| routed_tip_ids.contains(tip_id))
        .map(|(_, island_id)| *island_id)
        .collect::<std::collections::HashSet<usize>>()
        .len();
    model.branches = pieces.branches;
    model.trunks = pieces.trunks;
    model.feet = pieces.feet;
    model.braces = pieces.braces;
    model.plates = base::plates_for(&model.feet, &cfg.base);
    model.refresh_stats();

    let mut min = f32::MAX;
    let mut max = 0.0f32;
    for t in &model.trunks {
        let len = (t.top - t.bottom).length();
        min = min.min(len);
        max = max.max(len);
    }
    model.stats.trunk_min = if model.trunks.is_empty() { 0.0 } else { min };
    model.stats.trunk_max = max;
    if std::env::var_os("SUP_DEBUG").is_some() {
        let t_all = started.elapsed();
        eprintln!(
            "  stages: faces {:?} detect {:?} tips {:?} route {:?} total {:?}",
            t_detect,
            t_tips - t_detect,
            t_placed - t_tips,
            t_routed - t_placed,
            t_all
        );
    }
    model.stats.ms = started.elapsed().as_secs_f64() * 1000.0;
    if std::env::var_os("SUP_DEBUG").is_some() {
        eprintln!("{}", model.report());
    }
    model
}

/// How far a piece has to rise for its lowest point to sit `height` above the
/// platform. A piece already floating higher is left alone: somebody put it
/// there on purpose, and pulling it down would undo that.
///
/// This is the whole of "height from platform": the window, the batch runner
/// and the inspector all ask this one question, so the picture, the supports
/// and the slice agree on where the pieces are.
pub fn gap_rise(lowest_z: f32, height: f32) -> f32 {
    (height.max(0.0) - lowest_z).max(0.0)
}

/// Apply [`gap_rise`] to every piece. Returns how many of them moved.
pub fn lift(meshes: &mut [Mesh], height: f32) -> usize {
    let mut moved = 0;
    for m in meshes.iter_mut() {
        if m.tris.is_empty() {
            continue;
        }
        let dz = gap_rise(m.bounds().0.z, height);
        if dz > 1e-4 {
            m.translate(Vec3::new(0.0, 0.0, dz));
            moved += 1;
        }
    }
    moved
}

// ------------------------------------------------------------------ the job

pub enum JobMsg {
    Done(Box<SupportModel>),
    Failed(String),
}

/// A background generation run. Dropping it cancels the thread.
pub struct Job {
    pub rx: Receiver<JobMsg>,
    pub cancel: Arc<AtomicBool>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Job {
    #[allow(dead_code)]
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Start generating on a worker thread; the window keeps drawing meanwhile.
pub fn start(meshes: Vec<Mesh>, cfg: SupportConfig) -> Job {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    std::thread::spawn(move || {
        let model = generate(&meshes, &cfg, &flag);
        let msg = if flag.load(Ordering::Relaxed) {
            JobMsg::Failed("cancelled".into())
        } else {
            JobMsg::Done(Box::new(model))
        };
        let _ = tx.send(msg);
    });
    Job { rx, cancel }
}

// ------------------------------------------------------------------ a grid

/// A bucket grid over X and Y, used to find near neighbours quickly.
pub struct CellGrid {
    cell: f32,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl CellGrid {
    pub fn new(cell: f32) -> Self {
        Self {
            cell: cell.max(0.05),
            cells: HashMap::new(),
        }
    }

    fn key(&self, p: Vec3) -> (i32, i32) {
        (
            (p.x / self.cell).floor() as i32,
            (p.y / self.cell).floor() as i32,
        )
    }

    pub fn insert(&mut self, p: Vec3, id: u32) {
        self.cells.entry(self.key(p)).or_default().push(id);
    }

    /// Ids in the cells within `rings` cells of `p`.
    pub fn near(&self, p: Vec3, rings: i32, out: &mut Vec<u32>) {
        out.clear();
        let (cx, cy) = self.key(p);
        for dx in -rings..=rings {
            for dy in -rings..=rings {
                if let Some(v) = self.cells.get(&(cx + dx, cy + dy)) {
                    out.extend_from_slice(v);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
