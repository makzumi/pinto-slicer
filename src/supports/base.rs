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
/// Nearby feet are joined into one pad. The gap is measured between the
/// margins around the feet, not between their centre lines.
const JOIN_GAP: f32 = 4.0;
/// A connected hull with little support material inside it is split along its
/// long axis. This is a convex decomposition of the support contours, like
/// Prusa's concave pad blueprint, rather than one model-sized convex slab.
const MIN_FOOT_COVERAGE: f32 = 0.35;

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
            for group in contour_pieces(connected, cfg.margin.max(0.0)) {
                let poly = outline(&group, cfg.margin);
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

/// Split sparse connected components until each convex pad actually follows
/// the feet it carries. Dense local clusters stay joined.
fn contour_pieces(feet: Vec<Foot>, margin: f32) -> Vec<Vec<Foot>> {
    if feet.len() <= 1 {
        return vec![feet];
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
    if support_area / area.max(1e-4) >= MIN_FOOT_COVERAGE {
        return vec![feet];
    }

    let (min_x, max_x) = feet.iter().fold((f32::MAX, f32::MIN), |(min, max), foot| {
        (min.min(foot.pos.x), max.max(foot.pos.x))
    });
    let (min_y, max_y) = feet.iter().fold((f32::MAX, f32::MIN), |(min, max), foot| {
        (min.min(foot.pos.y), max.max(foot.pos.y))
    });
    let split_x = max_x - min_x >= max_y - min_y;
    let mut sorted = feet;
    sorted.sort_by(|a, b| {
        if split_x {
            a.pos.x.total_cmp(&b.pos.x)
        } else {
            a.pos.y.total_cmp(&b.pos.y)
        }
    });
    let right = sorted.split_off(sorted.len() / 2);
    let mut result = contour_pieces(sorted, margin);
    result.extend(contour_pieces(right, margin));
    result
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
