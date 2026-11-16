//! MSLA pinhead construction.

use super::config::SupportConfig;
use super::Tip;
use crate::mesh::Vec3;

/// Build a Prusa-style pinhead. The head points out of the model along the
/// surface normal, clamped into the configured downward bridge cone. Its
/// junction is the back-sphere centre, where a pillar or bridge attaches.
pub fn make_tip(pos: Vec3, normal: Vec3, cfg: &SupportConfig, island: bool) -> Tip {
    let n = if normal.length() < 1e-6 {
        Vec3::new(0.0, 0.0, -1.0)
    } else {
        normal.normalized()
    };
    let outward = if n.z > 0.0 { n * -1.0 } else { n };
    let angle = cfg.branch_angle_deg.clamp(15.0, 85.0).to_radians();
    let min_down = -angle.cos();
    let away = if outward.z <= min_down {
        outward
    } else {
        let horizontal = Vec3::new(outward.x, outward.y, 0.0);
        let horizontal = if horizontal.length() > 1e-6 {
            horizontal.normalized()
        } else {
            Vec3::new(1.0, 0.0, 0.0)
        };
        horizontal * angle.sin() + Vec3::new(0.0, 0.0, min_down)
    };
    let into = away * -1.0;

    // lower_end() subtracts (length - penetration) along `into`. This makes
    // the junction distance 2*front_radius + head_width + back_radius -
    // penetration, matching Prusa's Head::junction_point().
    let configured_length = cfg.tip.length + cfg.tip.upper_diameter + cfg.tip.lower_diameter * 0.5;
    let envelope = (cfg.tip.lower_diameter * 0.5) * (1.0 - into.z * into.z).max(0.0).sqrt();
    let floor_clearance = 0.05 + envelope;
    let max_length = if into.z > 1e-4 {
        ((pos.z + into.z * cfg.tip.contact_depth - floor_clearance) / into.z).max(0.0)
    } else {
        configured_length
    };

    Tip {
        id: 0,
        pos,
        axis: into,
        // Never let the tip body extend through the build-plate plane.
        length: configured_length.min(max_length),
        upper_d: cfg.tip.upper_diameter,
        lower_d: cfg.tip.lower_diameter,
        shape: cfg.tip.shape,
        contact_d: cfg.tip.contact_diameter,
        contact_shape: cfg.tip.contact_shape,
        contact_depth: cfg.tip.contact_depth,
        island,
        model_id: 0,
    }
}
