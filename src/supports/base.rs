//! Stage 5 — chamfered support bases on the platform.
//!
//! “Add Base” belongs to the support network: it joins and reinforces the
//! feet, like the reference slicer. It is not a raft projected from the whole
//! model. In particular, tilting a long part must not create a second,
//! model-shaped slab underneath it.

use super::config::BaseConfig;
use super::{Foot, Plate};
use crate::mesh::Vec3;

const SAMPLES_PER_FOOT: usize = 24;
/// Only platform feet receive a base. Model-supported trunks have no foot.
const PLATE_FOOT_Z: f32 = 0.5;
/// Feet join into one pad only when their margined discs already overlap
/// or nearly touch. The gap is measured between the margins around the
/// feet, not between their centre lines — a dense forest keeps its many
/// small pads instead of welding into one model-sized sheet that has to
/// peel off the film as a single suction cup.
const JOIN_GAP: f32 = 0.5;
/// A connected hull with little support material inside it is split along its
/// long axis. This is a convex decomposition of the support contours, like
/// Prusa's concave pad blueprint, rather than one model-sized convex slab.
const MIN_FOOT_COVERAGE: f32 = 0.35;
/// No single pad may grow into a sheet. The largest pad the reference
/// slicer's jobs carry here is about 900 mm²; past that, peel stress
/// concentrates and one lifting corner drags the whole pad off the plate,
/// so the recursion cuts the cluster and leaves a seam between the halves.
const MAX_PAD_AREA: f32 = 900.0;
/// The gap a seam cut leaves between two pads, in millimetres: wide enough
/// to drain, narrow enough that a boundary foot still sits on solid pad —
/// its disc simply gets a flat rim, the thickest part of the profile.
const SEAM: f32 = 0.6;

/// Build connected pads under the support feet, separately for every model.
pub fn plates_for(feet: &[Foot], cfg: &BaseConfig) -> Vec<Plate> {
    if !cfg.enabled || cfg.plate_thickness <= 0.05 {
        return Vec::new();
    }
    let feet: Vec<Foot> = feet
        .iter()
        .filter(|f| f.pos.z.abs() < PLATE_FOOT_Z)
        .cloned()
        .collect();
    if feet.is_empty() {
        return Vec::new();
    }

    let thickness = cfg.plate_thickness;
    let angle = cfg.chamfer_deg.clamp(15.0, 80.0).to_radians();
    // `chamfer` is the vertical rise. The horizontal run is derived from the
    // requested angle instead of using one distance for both axes (which
    // always produced a 45° bevel).
    let chamfer = (thickness * angle.tan()).min(thickness * 0.6).max(0.05);
    let chamfer_run = chamfer / angle.tan().max(1e-4);

    let mut by_model: std::collections::BTreeMap<usize, Vec<Foot>> =
        std::collections::BTreeMap::new();
    for foot in feet {
        by_model.entry(foot.model_id).or_default().push(foot);
    }

    let mut plates = Vec::new();
    for (model_id, subset) in &by_model {
        for connected in connected_feet(subset, cfg.margin.max(0.0)) {
            for poly in pad_polys(connected, cfg.margin.max(0.0), &[]) {
                if poly.len() >= 3 {
                    plates.push(Plate {
                        model_id: *model_id,
                        poly,
                        thickness,
                        chamfer,
                        chamfer_run,
                    });
                }
            }
        }
    }
    plates
}

/// Union feet whose margined discs overlap or nearly touch. Connectivity is
/// transitive, so a row of neighbouring trunks gets one strong base rather
/// than a pile of overlapping discs, while distant support trees stay apart.
fn connected_feet(feet: &[Foot], margin: f32) -> Vec<Vec<Foot>> {
    let mut parent: Vec<usize> = (0..feet.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            let next = parent[i];
            parent[i] = parent[next];
            i = next;
        }
        i
    }

    for i in 0..feet.len() {
        for j in i + 1..feet.len() {
            let a = &feet[i];
            let b = &feet[j];
            let reach =
                a.diameter.max(0.0) * 0.5 + b.diameter.max(0.0) * 0.5 + margin * 2.0 + JOIN_GAP;
            let dx = a.pos.x - b.pos.x;
            let dy = a.pos.y - b.pos.y;
            if dx * dx + dy * dy <= reach * reach {
                let ra = find(&mut parent, i);
                let rb = find(&mut parent, j);
                if ra != rb {
                    parent[ra] = rb;
                }
            }
        }
    }

    let mut groups: Vec<Vec<Foot>> = Vec::new();
    let mut roots: Vec<usize> = Vec::new();
    for (i, foot) in feet.iter().enumerate() {
        let root = find(&mut parent, i);
        let index = roots.iter().position(|known| *known == root);
        let group = match index {
            Some(index) => &mut groups[index],
            None => {
                roots.push(root);
                groups.push(Vec::new());
                groups.last_mut().unwrap()
            }
        };
        group.push(foot.clone());
    }
    groups
}

/// One seam cut: keep the half of the pad on one side of the line. Hulls
/// are convex, so clipping is one straight pass per cut.
#[derive(Clone, Copy)]
struct Clip {
    axis_x: bool,
    at: f32,
    keep_ge: bool,
}

/// Cut one connected cluster of feet into convex pads, in two dimensions:
/// the coverage rule keeps dense local clusters joined and splits sparse
/// hulls until each pad follows the feet it carries, and the area rule caps
/// how much sheet one pad may become. A dense cluster past the cap is cut
/// along its long axis, with a seam between the halves, so no peel ever has
/// a whole-model sheet to lift.
fn pad_polys(feet: Vec<Foot>, margin: f32, clips: &[Clip]) -> Vec<Vec<(f32, f32)>> {
    let joined = |hull: Vec<(f32, f32)>| -> Vec<Vec<(f32, f32)>> {
        let poly = clip_poly(hull, clips);
        if poly.len() >= 3 {
            vec![poly]
        } else {
            Vec::new()
        }
    };
    if feet.len() <= 1 {
        return joined(outline(&feet, margin));
    }
    let hull = outline(&feet, margin);
    let area = polygon_area(&hull);
    let support_area: f32 = feet
        .iter()
        .map(|foot| {
            let radius = foot.diameter.max(0.0) * 0.5 + margin;
            std::f32::consts::PI * radius * radius
        })
        .sum();
    if support_area / area.max(1e-4) >= MIN_FOOT_COVERAGE && area <= MAX_PAD_AREA {
        return joined(hull);
    }

    let (min_x, max_x) = feet.iter().fold((f32::MAX, f32::MIN), |(min, max), foot| {
        (min.min(foot.pos.x), max.max(foot.pos.x))
    });
    let (min_y, max_y) = feet.iter().fold((f32::MAX, f32::MIN), |(min, max), foot| {
        (min.min(foot.pos.y), max.max(foot.pos.y))
    });
    let Some(Halves {
        left,
        right,
        axis_x: split_x,
        cut,
        half,
    }) = halve(min_x, max_x, min_y, max_y, feet)
    else {
        // No coordinate gap on either axis: coincident feet, one pad.
        return joined(hull);
    };
    let mut left_clips = clips.to_vec();
    left_clips.push(Clip {
        axis_x: split_x,
        at: cut - half,
        keep_ge: false,
    });
    let mut right_clips = clips.to_vec();
    right_clips.push(Clip {
        axis_x: split_x,
        at: cut + half,
        keep_ge: true,
    });
    let mut out = pad_polys(left, margin, &left_clips);
    out.extend(pad_polys(right, margin, &right_clips));
    out
}

/// Cut a cluster into two halves along its long axis, at the true
/// coordinate gap nearest the middle. Cutting inside a tie group — a row
/// of feet sharing one coordinate — would leave the seam exactly on those
/// feet; cutting between coordinates keeps every foot's centre on solid
/// pad. The seam narrows to whatever room the two neighbouring feet
/// leave, and collapses to nothing when they touch.
/// Where `halve` cut: the two halves, the cut's axis, and the seam centre
/// and half-width in that axis.
struct Halves {
    left: Vec<Foot>,
    right: Vec<Foot>,
    axis_x: bool,
    cut: f32,
    half: f32,
}

fn halve(min_x: f32, max_x: f32, min_y: f32, max_y: f32, mut feet: Vec<Foot>) -> Option<Halves> {
    let mut split_x = max_x - min_x >= max_y - min_y;
    for _ in 0..2 {
        let axis = |f: &Foot| if split_x { f.pos.x } else { f.pos.y };
        feet.sort_by(|a, b| axis(a).total_cmp(&axis(b)));
        let middle = feet.len() as i32 / 2;
        let gap = (1..feet.len())
            .filter(|&i| axis(&feet[i - 1]) != axis(&feet[i]))
            .min_by_key(|&i| (i as i32 - middle).abs());
        if let Some(i) = gap {
            let right = feet.split_off(i);
            let a = axis(feet.last()?);
            let b = axis(right.first()?);
            let cut = (a + b) * 0.5;
            let half = ((b - a) * 0.5).min(SEAM * 0.5);
            return Some(Halves {
                left: feet,
                right,
                axis_x: split_x,
                cut,
                half,
            });
        }
        // The long axis is one solid line of feet; cut across it instead.
        split_x = !split_x;
    }
    None
}

/// Sutherland–Hodgman against the seam cuts. A hull that never crosses a
/// cut comes back unchanged; one that does comes back with a flat rim.
fn clip_poly(mut poly: Vec<(f32, f32)>, clips: &[Clip]) -> Vec<(f32, f32)> {
    for clip in clips {
        let coord = |p: &(f32, f32)| if clip.axis_x { p.0 } else { p.1 };
        let inside = |p: &(f32, f32)| {
            if clip.keep_ge {
                coord(p) >= clip.at
            } else {
                coord(p) <= clip.at
            }
        };
        let mut out: Vec<(f32, f32)> = Vec::with_capacity(poly.len() + 2);
        for i in 0..poly.len() {
            let a = poly[i];
            let b = poly[(i + 1) % poly.len()];
            if inside(&a) {
                out.push(a);
            }
            if inside(&a) != inside(&b) {
                let t = (clip.at - coord(&a)) / (coord(&b) - coord(&a));
                let p = if clip.axis_x {
                    (clip.at, a.1 + t * (b.1 - a.1))
                } else {
                    (a.0 + t * (b.0 - a.0), clip.at)
                };
                out.push(p);
            }
        }
        poly = out;
        if poly.len() < 3 {
            return Vec::new();
        }
    }
    poly.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-5 && (a.1 - b.1).abs() < 1e-5);
    if poly.len() > 3 {
        if let (Some(first), Some(last)) = (poly.first().copied(), poly.last().copied()) {
            if (first.0 - last.0).abs() < 1e-5 && (first.1 - last.1).abs() < 1e-5 {
                poly.pop();
            }
        }
    }
    poly
}

fn polygon_area(poly: &[(f32, f32)]) -> f32 {
    poly.iter()
        .enumerate()
        .map(|(index, a)| {
            let b = poly[(index + 1) % poly.len()];
            a.0 * b.1 - b.0 * a.1
        })
        .sum::<f32>()
        .abs()
        * 0.5
}

/// Convex outline around a group of foot discs, including the configured
/// margin on every side. Unlike a model projection, every part of this pad is
/// there to carry a support foot.
fn outline(feet: &[Foot], margin: f32) -> Vec<(f32, f32)> {
    let margin = margin.max(0.0);
    let mut points = Vec::with_capacity(feet.len() * SAMPLES_PER_FOOT);
    for foot in feet {
        let radius = foot.diameter.max(0.0) * 0.5 + margin;
        for k in 0..SAMPLES_PER_FOOT {
            let angle = k as f32 / SAMPLES_PER_FOOT as f32 * std::f32::consts::TAU;
            points.push((
                foot.pos.x + radius * angle.cos(),
                foot.pos.y + radius * angle.sin(),
            ));
        }
    }
    crate::supports::meshgen::hull(&points)
}

/// Where the plate's own outline sits, for tests and the report.
pub fn plate_bounds(p: &Plate) -> (Vec3, Vec3) {
    let mut min = Vec3::new(f32::MAX, f32::MAX, 0.0);
    let mut max = Vec3::new(f32::MIN, f32::MIN, 0.0);
    for (x, y) in &p.poly {
        min.x = min.x.min(*x);
        min.y = min.y.min(*y);
        max.x = max.x.max(*x);
        max.y = max.y.max(*y);
    }
    (min, max)
}
