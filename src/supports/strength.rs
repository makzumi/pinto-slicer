//! Prusa-style pillar interconnection. Nearby columns are linked with angled
//! zig-zag sticks (and crosses when their bases are far enough apart), not a
//! stack of horizontal bars.

use super::bvh::Bvh;
use super::config::SupportConfig;
use super::meshgen;
use super::route::Pieces;
use super::{Brace, Trunk};
use crate::mesh::Vec3;

const MAX_BRACES: usize = 4000;
const MAX_LINK_DISTANCE: f32 = 10.0;
const CASCADE_NEIGHBORS: usize = 4;
/// Lowest safe centre for a brace end: clear the base's dropped centre
/// (or the bare foot when the base is off) plus the brace's own radius.
/// Starting at a fixed 3 mm made short columns brace-free; starting at the
/// plate top put the end ball through the pad.
fn brace_floor(cfg: &SupportConfig, radius: f32) -> f32 {
    let surface = cfg.base.surface_height().max(meshgen::FOOT_HEIGHT);
    surface + radius + 0.02
}
/// Double crosses only appear between columns this far apart. Close pairs
/// get a plain zig-zag; giving every pair both diagonals is what made the
/// lattice read as a wall of overlapping sticks.
const CROSS_PAIR_DISTANCE: f32 = 6.0;
/// How much daylight a brace needs from anything it is not attached to.
const BRACE_CLEARANCE: f32 = 0.2;

pub fn strengthen(pieces: &mut Pieces, cfg: &SupportConfig, tree: &Bvh) {
    // Feet must remain at least as wide as the validated pillar above them.
    let diameters: Vec<f32> = pieces.trunks.iter().map(|trunk| trunk.diameter).collect();
    for foot in pieces.feet.iter_mut() {
        let trunk_d = diameters
            .get(foot.trunk_id)
            .copied()
            .unwrap_or(cfg.trunk_diameter);
        foot.diameter = foot.diameter.max(trunk_d * 1.4);
    }

    let mut pairs = Vec::new();
    for i in 0..pieces.trunks.len() {
        for j in i + 1..pieces.trunks.len() {
            let a = &pieces.trunks[i];
            let b = &pieces.trunks[j];
            if a.model_id != b.model_id || a.bottom.z > 0.01 || b.bottom.z > 0.01 {
                continue;
            }
            let span = xy_distance(a.bottom, b.bottom);
            if span >= a.diameter.max(b.diameter) && span <= MAX_LINK_DISTANCE {
                pairs.push((span, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut links = vec![0usize; pieces.trunks.len()];
    let mut braces = Vec::new();
    for (_, i, j) in pairs {
        if links[i] >= CASCADE_NEIGHBORS || links[j] >= CASCADE_NEIGHBORS {
            continue;
        }
        let added = interconnect(
            &pieces.trunks[i],
            &pieces.trunks[j],
            &pieces.trunks,
            cfg,
            tree,
            &mut braces,
        );
        if added && links[i] < CASCADE_NEIGHBORS && links[j] < CASCADE_NEIGHBORS {
            links[i] += 1;
            links[j] += 1;
        }
        if braces.len() >= MAX_BRACES {
            break;
        }
    }
    pieces.braces = braces;
}

fn interconnect(
    a: &Trunk,
    b: &Trunk,
    trunks: &[Trunk],
    cfg: &SupportConfig,
    tree: &Bvh,
    braces: &mut Vec<Brace>,
) -> bool {
    let radius = a.diameter.min(b.diameter) * 0.5;
    let distance = xy_distance(a.bottom, b.bottom);
    if distance < 1e-4 || radius <= 0.0 {
        return false;
    }
    let top = a.top.z.min(b.top.z);
    let bottom = a.bottom.z.max(b.bottom.z).max(brace_floor(cfg, radius));
    if top - bottom < 0.25 {
        return false;
    }
    let step = (distance * cfg.branch_angle_deg.to_radians().tan()).min(top - bottom);
    if step < 0.25 {
        return false;
    }

    let mut added = false;
    let mut z = top;
    let mut swap = false;
    while z + 1e-4 - step >= bottom && braces.len() < MAX_BRACES {
        let (from_trunk, to_trunk) = if swap { (b, a) } else { (a, b) };
        let from = Vec3::new(from_trunk.bottom.x, from_trunk.bottom.y, z);
        let to = Vec3::new(to_trunk.bottom.x, to_trunk.bottom.y, z - step);
        if tree.segment_clear(from, to, radius)
            && clear_of_columns(from, to, radius, a, b, trunks)
            && clear_of_braces(from, to, radius, braces, None)
        {
            braces.push(Brace {
                id: 0,
                a: from,
                b: to,
                diameter: radius * 2.0,
                model_id: from_trunk.model_id,
            });
            added = true;
        }

        // Prusa's dynamic mode adds the opposite diagonal for columns whose
        // bases are farther apart than one base diameter. Only the wide gaps
        // get the double cross; a close pair zig-zags on its own.
        if distance > CROSS_PAIR_DISTANCE {
            let reverse_from = Vec3::new(to_trunk.bottom.x, to_trunk.bottom.y, z);
            let reverse_to = Vec3::new(from_trunk.bottom.x, from_trunk.bottom.y, z - step);
            // The two legs of a cross meet by design, so the forward leg just
            // pushed is excused; anything else still needs its daylight.
            let excuse = if added { Some(braces.len() - 1) } else { None };
            if tree.segment_clear(reverse_from, reverse_to, radius)
                && clear_of_columns(reverse_from, reverse_to, radius, a, b, trunks)
                && clear_of_braces(reverse_from, reverse_to, radius, braces, excuse)
            {
                braces.push(Brace {
                    id: 0,
                    a: reverse_from,
                    b: reverse_to,
                    diameter: radius * 2.0,
                    model_id: from_trunk.model_id,
                });
                added = true;
            }
        }
        swap = !swap;
        z -= step;
    }
    added
}

/// A brace may touch the two columns it joins and nothing else. Anything
/// closer than the combined radii plus a hair of daylight would read as a
/// stick rammed through a column — HALOT Box's lattice never does that.
fn clear_of_columns(
    from: Vec3,
    to: Vec3,
    radius: f32,
    a: &Trunk,
    b: &Trunk,
    trunks: &[Trunk],
) -> bool {
    for trunk in trunks {
        if trunk.model_id != a.model_id || trunk.bottom == a.bottom || trunk.bottom == b.bottom {
            continue;
        }
        let needed = radius + trunk.diameter * 0.5 + BRACE_CLEARANCE;
        if segment_distance(from, to, trunk.bottom, trunk.top) < needed {
            return false;
        }
    }
    true
}

/// Nor may a brace stack on one that is already there. `excused` is the
/// index of the other leg of the same cross, which is allowed to meet.
fn clear_of_braces(
    from: Vec3,
    to: Vec3,
    radius: f32,
    braces: &[Brace],
    excused: Option<usize>,
) -> bool {
    for (index, brace) in braces.iter().enumerate() {
        if Some(index) == excused {
            continue;
        }
        let needed = radius + brace.diameter * 0.5 + BRACE_CLEARANCE;
        if segment_distance(from, to, brace.a, brace.b) < needed {
            return false;
        }
    }
    true
}

/// Closest distance between two line segments, both ends clamped
/// (Ericson, Real-Time Collision Detection, §5.1.9).
fn segment_distance(a0: Vec3, a1: Vec3, b0: Vec3, b1: Vec3) -> f32 {
    let d1 = a1 - a0;
    let d2 = b1 - b0;
    let r = a0 - b0;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    if a <= 1e-9 && e <= 1e-9 {
        return r.length();
    }
    if a <= 1e-9 {
        let t = (f / e).clamp(0.0, 1.0);
        return (a0 - (b0 + d2 * t)).length();
    }
    let c = d1.dot(r);
    if e <= 1e-9 {
        let s = (-c / a).clamp(0.0, 1.0);
        return ((a0 + d1 * s) - b0).length();
    }
    let b = d1.dot(d2);
    let denom = a * e - b * b;
    let s = if denom.abs() > 1e-9 {
        ((b * f - c * e) / denom).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let t = ((b * s + f) / e).clamp(0.0, 1.0);
    let pa = a0 + d1 * s;
    let pb = b0 + d2 * t;
    (pa - pb).length()
}

fn xy_distance(a: Vec3, b: Vec3) -> f32 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}
