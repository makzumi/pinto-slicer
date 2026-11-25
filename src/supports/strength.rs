//! Prusa-style pillar interconnection. Nearby columns are linked with angled
//! zig-zag sticks (and crosses when their bases are far enough apart), not a
//! stack of horizontal bars.

use super::bvh::Bvh;
use super::config::SupportConfig;
use super::route::Pieces;
use super::{Brace, Trunk};
use crate::mesh::Vec3;

const MAX_BRACES: usize = 4000;
const MAX_LINK_DISTANCE: f32 = 10.0;
const CASCADE_NEIGHBORS: usize = 4;
/// Cross braces start this far above the plate — HALOT Box's Cross Start
/// Height, which keeps the lattice out of the base's chamfer and the film's
/// first peel.
const CROSS_START_HEIGHT: f32 = 3.0;

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
        let added = interconnect(&pieces.trunks[i], &pieces.trunks[j], cfg, tree, &mut braces);
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
    cfg: &SupportConfig,
    tree: &Bvh,
    braces: &mut Vec<Brace>,
) -> bool {
    let radius = a.diameter.min(b.diameter) * 0.5;
    let distance = xy_distance(a.bottom, b.bottom);
    if distance < 1e-4 || radius <= 0.0 {
        return false;
    }
    let step = distance * cfg.branch_angle_deg.to_radians().tan();
    if step < 0.25 {
        return false;
    }
    let bottom = (a.bottom.z.max(b.bottom.z) + cfg.base.plate_thickness).max(CROSS_START_HEIGHT);
    let top = a.top.z.min(b.top.z);
    if top - bottom < step {
        return false;
    }

    let mut added = false;
    let mut z = top;
    let mut swap = false;
    while z - step >= bottom && braces.len() < MAX_BRACES {
        let (from_trunk, to_trunk) = if swap { (b, a) } else { (a, b) };
        let from = Vec3::new(from_trunk.bottom.x, from_trunk.bottom.y, z);
        let to = Vec3::new(to_trunk.bottom.x, to_trunk.bottom.y, z - step);
        if tree.segment_clear(from, to, radius) {
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
        // bases are farther apart than one base diameter.
        if distance > cfg.foot_diameter {
            let reverse_from = Vec3::new(to_trunk.bottom.x, to_trunk.bottom.y, z);
            let reverse_to = Vec3::new(from_trunk.bottom.x, from_trunk.bottom.y, z - step);
            if tree.segment_clear(reverse_from, reverse_to, radius) {
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

fn xy_distance(a: Vec3, b: Vec3) -> f32 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}
