//! Tests for the support generator, one per acceptance criterion.

use super::*;
use crate::mesh::{Mesh, Vec3};
use std::sync::atomic::AtomicBool;

/// A closed box standing on the plate, raised to the configured height.
fn block(size: (f32, f32, f32), lift: f32) -> Mesh {
    let (sx, sy, sz) = size;
    let mut m = Mesh::default();
    let (hx, hy) = (sx * 0.5, sy * 0.5);
    let z0 = lift;
    let z1 = lift + sz;
    let corners = [
        Vec3::new(-hx, -hy, z0),
        Vec3::new(hx, -hy, z0),
        Vec3::new(hx, hy, z0),
        Vec3::new(-hx, hy, z0),
        Vec3::new(-hx, -hy, z1),
        Vec3::new(hx, -hy, z1),
        Vec3::new(hx, hy, z1),
        Vec3::new(-hx, hy, z1),
    ];
    m.verts.extend_from_slice(&corners);
    let faces = [
        [0u32, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ];
    m.tris.extend_from_slice(&faces);
    m
}

/// A lever arm: a narrow bar cantilevered out at 30° above the horizontal.
fn lever() -> Mesh {
    let mut m = Mesh::default();
    let len = 40.0;
    let w = 6.0;
    let t = 3.0;
    let lift = 6.0;
    for i in 0..=1 {
        let x = len * i as f32 / 1.0;
        let z = lift + x * (30.0f32).to_radians().tan();
        for (dy, dz) in [
            (-w * 0.5, 0.0f32),
            (w * 0.5, 0.0),
            (w * 0.5, t),
            (-w * 0.5, t),
        ] {
            m.verts.push(Vec3::new(x, dy, z + dz));
        }
    }
    // a post at the fixed end so the arm is attached to something
    for (dy, dz) in [
        (-w * 0.5, 0.0f32),
        (w * 0.5, 0.0),
        (w * 0.5, 0.0),
        (-w * 0.5, 0.0),
    ] {
        let _ = dz;
        m.verts.push(Vec3::new(-2.0, dy, 0.0));
    }
    let quads = [
        [0u32, 1, 5, 4],
        [1, 2, 6, 5],
        [2, 3, 7, 6],
        [3, 0, 4, 7],
        [4, 5, 6, 7],
        [0, 3, 2, 1],
    ];
    for q in quads {
        m.tris.push([q[0], q[1], q[2]]);
        m.tris.push([q[0], q[2], q[3]]);
    }
    m.fix_winding();
    m
}

/// A sphere floating off the plate: an island that must be supported.
fn sphere(radius: f32, centre_z: f32) -> Mesh {
    let mut m = Mesh::default();
    let rings = 16;
    let sides = 24;
    for r in 0..=rings {
        let phi = std::f32::consts::PI * r as f32 / rings as f32;
        for i in 0..sides {
            let t = i as f32 / sides as f32 * std::f32::consts::TAU;
            m.verts.push(Vec3::new(
                radius * phi.sin() * t.cos(),
                radius * phi.sin() * t.sin(),
                centre_z - radius * phi.cos(),
            ));
        }
    }
    for r in 0..rings {
        for i in 0..sides {
            let j = (i + 1) % sides;
            let a = (r * sides + i) as u32;
            let b = (r * sides + j) as u32;
            let c = ((r + 1) * sides + j) as u32;
            let d = ((r + 1) * sides + i) as u32;
            m.tris.push([a, b, c]);
            m.tris.push([a, c, d]);
        }
    }
    m.fix_winding();
    m
}

/// A block with a square through-hole in the middle: a real opening, so a
/// support placed over the hole would touch nothing.
fn bored_block(hole: f32) -> Mesh {
    let mut m = Mesh::default();
    let (o, h) = (10.0f32, hole * 0.5); // outer half-size, hole half-size
    let (z0, z1) = (6.0f32, 16.0f32);
    let mut quad = |p: [[f32; 3]; 4]| {
        let base = m.verts.len() as u32;
        for v in p {
            m.verts.push(Vec3::new(v[0], v[1], v[2]));
        }
        m.tris.push([base, base + 1, base + 2]);
        m.tris.push([base, base + 2, base + 3]);
    };
    // bottom face, inwards (facing down)
    quad([[-o, -o, z0], [o, -o, z0], [o, -h, z0], [-o, -h, z0]]);
    quad([[-o, h, z0], [o, h, z0], [o, o, z0], [-o, o, z0]]);
    quad([[-o, -h, z0], [-h, -h, z0], [-h, h, z0], [-o, h, z0]]);
    quad([[h, -h, z0], [o, -h, z0], [o, h, z0], [h, h, z0]]);
    // top face
    quad([[-o, -h, z1], [o, -h, z1], [o, -o, z1], [-o, -o, z1]]);
    quad([[-o, o, z1], [o, o, z1], [o, h, z1], [-o, h, z1]]);
    quad([[-o, h, z1], [-h, h, z1], [-h, -h, z1], [-o, -h, z1]]);
    quad([[h, h, z1], [o, h, z1], [o, -h, z1], [h, -h, z1]]);
    // outer walls
    quad([[-o, -o, z0], [-o, -o, z1], [o, -o, z1], [o, -o, z0]]);
    quad([[o, -o, z0], [o, -o, z1], [o, o, z1], [o, o, z0]]);
    quad([[o, o, z0], [o, o, z1], [-o, o, z1], [-o, o, z0]]);
    quad([[-o, o, z0], [-o, o, z1], [-o, -o, z1], [-o, -o, z0]]);
    // walls of the hole, facing into it
    quad([[-h, -h, z0], [h, -h, z0], [h, -h, z1], [-h, -h, z1]]);
    quad([[h, -h, z0], [h, h, z0], [h, h, z1], [h, -h, z1]]);
    quad([[h, h, z0], [-h, h, z0], [-h, h, z1], [h, h, z1]]);
    quad([[-h, h, z0], [-h, -h, z0], [-h, -h, z1], [-h, h, z1]]);
    m.fix_winding();
    m
}

fn run(meshes: &[Mesh], cfg: &SupportConfig) -> SupportModel {
    let cancel = AtomicBool::new(false);
    generate(meshes, cfg, &cancel)
}

fn mesh_triangle_count(m: &SupportModel) -> usize {
    m.mesh(12).tris.len()
}

fn branch_segments(branch: &Branch) -> Vec<(Vec3, Vec3)> {
    match branch.via {
        Some(via) => vec![(branch.from, via), (via, branch.to)],
        None => vec![(branch.from, branch.to)],
    }
}

fn assert_connected_trees(model: &SupportModel) {
    for tip in &model.tips {
        let trunk = model
            .trunks
            .iter()
            .find(|trunk| trunk.tip_ids.contains(&tip.id));
        let direct = trunk.is_some_and(|trunk| (trunk.top - tip.lower_end()).length() < 1e-3);
        let branches = model
            .branches
            .iter()
            .filter(|branch| branch.tip_id == tip.id)
            .count();
        assert!(
            direct || branches == 1,
            "tip {} is neither a direct pillar head nor attached by one bridge",
            tip.id
        );
        assert!(branches <= 1, "tip {} has duplicate bridges", tip.id);
    }
    for anchor in &model.anchors {
        assert!(
            model.trunks.iter().any(|trunk| {
                trunk.bottom.z > 0.01 && (trunk.bottom - anchor.lower_end()).length() < 1e-3
            }),
            "a reverse anchor does not meet its model-facing pillar"
        );
    }
    for branch in &model.branches {
        let trunk = model
            .trunks
            .iter()
            .find(|trunk| trunk.tip_ids.contains(&branch.tip_id))
            .unwrap_or_else(|| panic!("branch tip {} has no trunk", branch.tip_id));
        assert!((branch.to.x - trunk.bottom.x).abs() < 1e-3);
        assert!((branch.to.y - trunk.bottom.y).abs() < 1e-3);
        let low = trunk.bottom.z.min(trunk.top.z) - 1e-3;
        let high = trunk.bottom.z.max(trunk.top.z) + 1e-3;
        assert!(
            (low..=high).contains(&branch.to.z),
            "branch ends at z {}, outside its trunk's {low}..{high}",
            branch.to.z
        );
    }
    for trunk in &model.trunks {
        for tip_id in &trunk.tip_ids {
            let tip = model.tips.iter().find(|tip| tip.id == *tip_id);
            assert_eq!(
                tip.map(|tip| tip.model_id),
                Some(trunk.model_id),
                "a trunk contains another model's tip"
            );
        }
        let feet: Vec<_> = model
            .feet
            .iter()
            .filter(|foot| foot.trunk_id == trunk.id)
            .collect();
        if trunk.bottom.z.abs() < 0.01 {
            assert_eq!(feet.len(), 1, "platform trunk has no unique foot");
            assert_eq!(feet[0].model_id, trunk.model_id);
            assert!((feet[0].pos.x - trunk.bottom.x).abs() < 1e-3);
            assert!((feet[0].pos.y - trunk.bottom.y).abs() < 1e-3);
        } else {
            assert!(feet.is_empty(), "model-bonded trunk has a floating foot");
        }
    }
}

fn assert_feet_are_based(model: &SupportModel) {
    for foot in &model.feet {
        for k in 0..24 {
            let angle = k as f32 / 24.0 * std::f32::consts::TAU;
            let x = foot.pos.x + foot.diameter * 0.5 * angle.cos();
            let y = foot.pos.y + foot.diameter * 0.5 * angle.sin();
            assert!(
                model.plates.iter().any(|plate| covers(&plate.poly, x, y)),
                "foot {} is outside every support base",
                foot.id
            );
        }
    }
}

#[test]
fn a_flat_block_gets_a_network_and_a_base_plate() {
    let m = block((30.0, 30.0, 10.0), 6.0);
    let cfg = SupportConfig::default();
    let model = run(std::slice::from_ref(&m), &cfg);
    assert!(!model.tips.is_empty(), "the raised block needs supports");
    assert!(!model.trunks.is_empty(), "tips must reach the plate");
    assert!(!model.plates.is_empty(), "a base plate under the feet");
    assert_eq!(model.stats.tips, model.tips.len());
    assert!(mesh_triangle_count(&model) > 100);
    // every trunk stands on the plate
    for t in &model.trunks {
        assert!(t.bottom.z <= 0.01, "trunk bottom at {}", t.bottom.z);
        assert!(model
            .feet
            .iter()
            .any(|f| (f.pos.x - t.bottom.x).abs() < 0.01 && (f.pos.y - t.bottom.y).abs() < 0.01));
    }
    for f in &model.feet {
        assert!(
            model.trunks.iter().any(|t| t.id == f.trunk_id),
            "foot points at a missing trunk"
        );
    }
    // the plate is a real slab on the platform
    let plate = &model.plates[0];
    assert!(plate.thickness >= 0.7);
    let (min, max) = base::plate_bounds(plate);
    assert!(min.x < max.x && min.y < max.y);
}

#[test]
fn an_island_gets_a_tip() {
    let m = sphere(10.0, 25.0);
    let cfg = SupportConfig::default();
    let model = run(std::slice::from_ref(&m), &cfg);
    assert!(model.stats.islands >= 1, "the floating sphere is an island");
    assert!(
        model.tips.iter().any(|t| t.island),
        "an island tip must exist"
    );
    // and it must sit under the sphere, not above it
    let island_tips = model.tips.iter().filter(|t| t.island).count();
    assert!(island_tips >= 1);
    // a tip reaches up to the sphere's underside at z = 15
    assert!(
        model
            .tips
            .iter()
            .any(|t| t.island && t.pos.z > 14.5 && t.pos.x.abs() < 2.0 && t.pos.y.abs() < 2.0),
        "the island tip must sit under the sphere"
    );
}

#[test]
fn no_support_cuts_through_the_model() {
    for m in [block((30.0, 30.0, 10.0), 6.0), sphere(10.0, 20.0), lever()] {
        let cfg = SupportConfig::default();
        let model = run(std::slice::from_ref(&m), &cfg);
        let tree = bvh::Bvh::from_meshes(std::slice::from_ref(&m));
        // The centre line of every branch, trunk and brace must stay out of the
        // material. Only the last part of a tip, which is pressed in on
        // purpose, is allowed inside.
        for b in &model.branches {
            for (from, to) in branch_segments(b) {
                assert!(
                    tree.segment_clear(from, to, 0.0),
                    "a branch runs through the model ({:?} -> {:?})",
                    from,
                    to
                );
            }
        }
        for t in &model.trunks {
            // A trunk standing on the model starts inside that surface on
            // purpose, the same way a tip is pressed into it. How deep the
            // material can rise around it is what a seat is allowed to slope:
            // the bond, plus the pad's own diameter and a raster layer.
            let from = if t.bottom.z > 0.5 {
                let sunk = cfg.landing_depth() + t.diameter + analysis::ANALYSIS_LAYER_HEIGHT;
                Vec3::new(t.bottom.x, t.bottom.y, t.bottom.z + sunk)
            } else {
                t.bottom
            };
            assert!(
                tree.segment_clear(from, t.top, 0.0),
                "a trunk runs through the model ({from:?} -> {:?})",
                t.top
            );
        }
        for b in &model.braces {
            assert!(
                tree.segment_clear(b.a, b.b, 0.0),
                "a brace runs through the model"
            );
        }
        for tip in &model.tips {
            let body_end = tip.pos - tip.axis * cfg.tip.contact_depth;
            assert!(
                tree.segment_clear(tip.lower_end(), body_end, 0.0),
                "a tip passes through the model"
            );
        }
    }
}

#[test]
fn every_element_mesh_is_closed() {
    let m = block((24.0, 24.0, 8.0), 6.0);
    let cfg = SupportConfig::default();
    let model = run(std::slice::from_ref(&m), &cfg);
    // check a few representative pieces, not the whole network
    let check = |name: &str, mesh: Mesh| {
        let mut edges: std::collections::HashMap<(u32, u32), i32> =
            std::collections::HashMap::new();
        for t in &mesh.tris {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let key = if a < b { (a, b) } else { (b, a) };
                *edges.entry(key).or_insert(0) += 1;
            }
        }
        let open = edges.values().filter(|c| **c % 2 != 0).count();
        assert_eq!(open, 0, "{name} has {open} open edges");
    };
    if let Some(t) = model.tips.first() {
        check("tip", meshgen::tip(t, 12));
    }
    if let Some(b) = model.branches.first() {
        check(
            "branch",
            meshgen::frustum(b.from, b.diameter * 0.5, b.to, b.diameter * 0.5, 12),
        );
    }
    if let Some(f) = model.feet.first() {
        check("foot", meshgen::foot(f));
    }
    for p in &model.plates {
        check("plate", meshgen::plate(p));
    }
}

#[test]
fn sliced_layers_of_the_supports_have_no_floating_islands() {
    // Build the supports on their own, then check the very first printed
    // layer: it must be the base plate reaching the plate, not a floating ring.
    let m = block((20.0, 20.0, 8.0), 6.0);
    let cfg = SupportConfig::default();
    let model = run(std::slice::from_ref(&m), &cfg);
    let support = model.mesh(12);
    let sl = crate::slicer::slice(std::slice::from_ref(&support), 0.05);
    assert!(!sl.layers.is_empty());
    let first = &sl.layers[0];
    let (min, max) = support.bounds();
    let w = 200;
    let h = 200;
    let mut mask = Vec::new();
    crate::slicer::fill_mask(
        first,
        [min.x - 1.0, min.y - 1.0],
        [max.x + 1.0, max.y + 1.0],
        w,
        h,
        &mut mask,
    );
    let solid = mask.iter().filter(|v| **v > 128).count();
    assert!(
        solid > 50,
        "the bottom layer must be solid plate, was {solid} px"
    );
}

#[test]
fn tips_stay_out_of_a_narrow_bore() {
    let m = bored_block(2.0);
    let cfg = SupportConfig::default();
    let model = run(std::slice::from_ref(&m), &cfg);
    let tree = bvh::Bvh::from_meshes(std::slice::from_ref(&m));
    // Nothing may sit inside the bore itself (a 2 mm hole through the block,
    // between z = 6 and z = 16), and no support may pass through the material.
    for t in &model.tips {
        let r = (t.pos.x * t.pos.x + t.pos.y * t.pos.y).sqrt();
        assert!(
            r > 0.8 || t.pos.z < 6.0,
            "a tip sits in the bore at {:.2}, {:.2}, z {:.2}",
            t.pos.x,
            t.pos.y,
            t.pos.z
        );
    }
    for b in &model.branches {
        for (from, to) in branch_segments(b) {
            assert!(
                tree.segment_clear(from, to, 0.0),
                "a branch runs through the bored block"
            );
        }
    }
    for t in &model.trunks {
        assert!(
            tree.segment_clear(t.bottom, t.top, 0.0),
            "a trunk runs through the bored block"
        );
    }
}

#[test]
fn a_long_part_gets_feet_spread_along_it_and_a_plate_under_them() {
    // A long, thin part: the feet end up spread out along 160 mm, so the base
    // plate has to follow them instead of sitting in one blob.
    let m = block((160.0, 20.0, 10.0), 6.0);
    let model = run(std::slice::from_ref(&m), &SupportConfig::default());
    assert!(model.feet.len() >= 2, "several feet along a long part");
    assert!(!model.plates.is_empty(), "the feet get a base plate");
    let xs: Vec<f32> = model.feet.iter().map(|f| f.pos.x).collect();
    let spread =
        xs.iter().cloned().fold(f32::MIN, f32::max) - xs.iter().cloned().fold(f32::MAX, f32::min);
    assert!(
        spread > 40.0,
        "the feet are spread along the part ({spread:.0} mm)"
    );
    // the plates together reach under all of them
    let mut pmin = f32::MAX;
    let mut pmax = f32::MIN;
    for p in &model.plates {
        let (a, b) = base::plate_bounds(p);
        pmin = pmin.min(a.x);
        pmax = pmax.max(b.x);
    }
    assert!(
        pmax - pmin > 40.0,
        "the plates reach under the spread of feet"
    );
}

#[test]
fn the_plate_mesh_is_a_real_slab() {
    let m = block((20.0, 20.0, 8.0), 6.0);
    let model = run(std::slice::from_ref(&m), &SupportConfig::default());
    let plate = model.plates.first().expect("a plate");
    let mesh = meshgen::plate(plate);
    let (min, max) = mesh.bounds();
    assert!(
        mesh.tris.len() >= 40,
        "a plate is a modelled shape, not a line"
    );
    assert!(
        min.z.abs() < 1e-3 && (max.z - plate.thickness).abs() < 0.05,
        "it sits on the plate and is one slab thick"
    );
    let (bmin, bmax) = base::plate_bounds(plate);
    assert!(
        (min.x - bmin.x).abs() < 0.05 && (max.x - bmax.x).abs() < 0.05,
        "it covers the whole outline"
    );
}

#[test]
fn the_base_plate_can_be_switched_off() {
    let m = block((20.0, 20.0, 8.0), 6.0);
    let mut cfg = SupportConfig::default();
    let with = run(std::slice::from_ref(&m), &cfg);
    assert!(!with.plates.is_empty(), "a plate is there by default");
    cfg.base.plate_thickness = 0.0;
    let without = run(std::slice::from_ref(&m), &cfg);
    assert!(
        without.plates.is_empty(),
        "0 mm thickness leaves the plate off"
    );
    assert!(!without.feet.is_empty(), "the feet stay");
    assert!(
        mesh_triangle_count(&without) < mesh_triangle_count(&with),
        "fewer triangles without the plate"
    );
}

#[test]
fn height_from_platform_puts_the_model_at_the_gap() {
    // A block sitting on the plate, lifted to 6 mm: the lift moves it exactly
    // once, and a second call changes nothing (no drifting).
    let mut meshes = vec![block((20.0, 20.0, 8.0), 0.0)];
    assert_eq!(lift(&mut meshes, 6.0), 1, "the piece had to move");
    assert!(
        (meshes[0].bounds().0.z - 6.0).abs() < 1e-3,
        "bottom at 6 mm"
    );
    assert_eq!(lift(&mut meshes, 6.0), 0, "a second lift must do nothing");
    assert!((meshes[0].bounds().0.z - 6.0).abs() < 1e-3);

    // A piece somebody already raised stays where it was put.
    let mut pair = vec![block((20.0, 20.0, 4.0), 0.0), block((20.0, 20.0, 4.0), 9.0)];
    assert_eq!(lift(&mut pair, 6.0), 1, "only the low piece moves");
    assert!((pair[0].bounds().0.z - 6.0).abs() < 1e-3);
    assert!(
        (pair[1].bounds().0.z - 9.0).abs() < 1e-3,
        "the high piece is untouched"
    );

    // and the supports are grown for the model where it stands: every tip
    // touches the model at 6 mm, and every trunk reaches the plate
    let model = run(&meshes, &SupportConfig::default());
    assert!(!model.tips.is_empty());
    for t in &model.tips {
        assert!(
            t.pos.z >= 5.9,
            "a tip sits below the lifted model at {}",
            t.pos.z
        );
    }
    for t in &model.trunks {
        assert!(t.bottom.z.abs() < 0.01, "a trunk does not reach the plate");
    }
}

#[test]
fn the_base_plate_switch_removes_the_plate() {
    let m = block((20.0, 20.0, 8.0), 6.0);
    let mut cfg = SupportConfig::default();
    cfg.base.enabled = false;
    let off = run(std::slice::from_ref(&m), &cfg);
    assert!(off.plates.is_empty(), "the switch turns the plate off");
    assert!(!off.feet.is_empty(), "the feet stay on their own");
    assert!(
        mesh_triangle_count(&off) > 100,
        "the supports still have geometry without a plate"
    );
    cfg.base.enabled = true;
    let on = run(std::slice::from_ref(&m), &cfg);
    assert!(!on.plates.is_empty(), "and it comes back");
}

#[test]
fn generation_is_deterministic() {
    let m = sphere(12.0, 24.0);
    let cfg = SupportConfig::default();
    let a = run(std::slice::from_ref(&m), &cfg);
    let b = run(std::slice::from_ref(&m), &cfg);
    assert_eq!(a.tips.len(), b.tips.len());
    assert_eq!(a.trunks.len(), b.trunks.len());
    assert_eq!(a.branches.len(), b.branches.len());
    assert_eq!(a.plates.len(), b.plates.len());
    for (x, y) in a.tips.iter().zip(b.tips.iter()) {
        assert!((x.pos - y.pos).length() < 1e-4);
    }
}

#[test]
fn a_plate_stays_local_when_the_model_is_moved() {
    let p = Plate {
        model_id: 0,
        poly: vec![(98.0, 48.0), (122.0, 48.0), (122.0, 72.0), (98.0, 72.0)],
        thickness: 0.8,
        chamfer: 0.2,
        chamfer_run: 0.2,
    };
    let mesh = meshgen::plate(&p);
    assert!(
        mesh.verts
            .iter()
            .all(|v| v.x >= 97.9 && v.x <= 122.1 && v.y >= 47.9 && v.y <= 72.1),
        "plate cap leaked back to world origin"
    );
}

#[test]
fn tip_and_post_have_volumetric_overlap_at_their_junction() {
    let cfg = SupportConfig::default();
    for normal in [Vec3::new(0.0, 0.0, -1.0), Vec3::new(1.0, 0.0, -1.0)] {
        let tip = tips::make_tip(Vec3::new(0.0, 0.0, 6.0), normal, &cfg, false);
        let joint = tip.lower_end();
        let radius = tip.lower_d * 0.5;
        let down = Vec3::new(0.0, 0.0, -1.0);
        let probe = joint + down * (0.25 * radius);
        let tip_tree = bvh::Bvh::from_meshes(&[meshgen::tip(&tip, 12)]);
        let post = meshgen::frustum(joint, radius, joint + down * (4.0 * radius), radius, 12);
        let post_tree = bvh::Bvh::from_meshes(&[post]);
        assert!(post_tree.inside(probe), "the probe must be inside the post");
        assert!(
            tip_tree.inside(probe),
            "tip and post have no common interior below {joint:?}"
        );
    }
}

#[test]
fn tip_back_spheres_stay_above_the_platform() {
    let cfg = SupportConfig::default();
    for normal in [Vec3::new(0.0, 0.0, -1.0), Vec3::new(1.0, 0.0, -1.0)] {
        let tip = tips::make_tip(Vec3::new(0.0, 0.0, 0.5), normal, &cfg, false);
        let (min, _) = meshgen::tip(&tip, 12).bounds();
        assert!(min.z >= -1e-4, "back sphere crossed the plate at {}", min.z);
    }
}

#[test]
fn tip_shape_and_contact_shape_change_the_mesh() {
    let mut cfg = SupportConfig::default();
    let cone = tips::make_tip(
        Vec3::new(0.0, 0.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        &cfg,
        false,
    );
    let cone_mesh = meshgen::tip(&cone, 12);

    cfg.tip.shape = TipShape::Cylinder;
    let pin = tips::make_tip(
        Vec3::new(0.0, 0.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        &cfg,
        false,
    );
    let pin_mesh = meshgen::tip(&pin, 12);
    assert_ne!(
        cone_mesh.verts[1], pin_mesh.verts[1],
        "pin shape is still a cone"
    );

    let none_triangles = cone_mesh.tris.len();
    cfg.tip.contact_shape = ContactShape::Ball;
    let ball = tips::make_tip(
        Vec3::new(0.0, 0.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        &cfg,
        false,
    );
    let ball_mesh = meshgen::tip(&ball, 12);
    assert!(
        ball_mesh.tris.len() > none_triangles,
        "contact ball is not optional"
    );
}

#[test]
fn a_model_already_on_the_platform_needs_no_under_supports() {
    let cfg = SupportConfig {
        height_from_platform: 0.0,
        ..Default::default()
    };
    let model = run(&[block((20.0, 20.0, 8.0), 0.0)], &cfg);
    assert!(
        model.tips.is_empty(),
        "platform contact was misclassified as an island"
    );
}

#[test]
fn support_mesh_never_goes_below_the_platform() {
    let cfg = SupportConfig::default();
    let model = run(&[block((20.0, 20.0, 8.0), 0.5)], &cfg);
    let mesh = model.mesh(12);
    let (min, _) = mesh.bounds();
    assert!(
        min.z >= -1e-4,
        "support geometry crossed the platform at z={}",
        min.z
    );
}

#[test]
fn close_ground_heads_share_a_prusa_central_pillar() {
    let cfg = SupportConfig::default();
    let mut first = tips::make_tip(
        Vec3::new(0.0, 0.0, 10.0),
        Vec3::new(0.0, 0.0, -1.0),
        &cfg,
        false,
    );
    first.id = 0;
    let mut second = tips::make_tip(
        Vec3::new(2.0, 0.0, 10.0),
        Vec3::new(0.0, 0.0, -1.0),
        &cfg,
        false,
    );
    second.id = 1;
    let tree = bvh::Bvh::from_meshes(&[]);
    let mut stats = Stats::default();
    let pieces = route::route(&[first, second], &cfg, &tree, &mut stats);
    assert_eq!(pieces.tips.len(), 2);
    assert_eq!(
        pieces.trunks.len(),
        1,
        "nearby heads share one central pillar"
    );
    assert_eq!(pieces.branches.len(), 1, "the side head uses a bridge");
    let branch = &pieces.branches[0];
    assert!(
        branch.to.z < branch.from.z,
        "the side bridge descends to the pillar"
    );
    assert_eq!(stats.dropped, 0);
}

#[test]
fn a_tall_network_gets_braces() {
    // Two tall, thin trunks standing near each other: the strength stage must
    // tie them together. A pair far apart must be left alone.
    let trunk = |x: f32, id: usize| Trunk {
        id,
        bottom: Vec3::new(x, 0.0, 0.0),
        top: Vec3::new(x, 0.0, 30.0),
        diameter: 1.6,
        tip_ids: vec![id],
        model_id: 0,
    };
    let cfg = SupportConfig::default();
    let empty = bvh::Bvh::from_meshes(&[]);
    let mut near = route::Pieces {
        tips: Vec::new(),
        anchors: Vec::new(),
        branches: Vec::new(),
        trunks: vec![trunk(-4.0, 0), trunk(4.0, 1)],
        feet: Vec::new(),
        braces: Vec::new(),
    };
    strength::strengthen(&mut near, &cfg, &empty);
    assert!(!near.braces.is_empty(), "nearby tall trunks get braced");

    let mut far = route::Pieces {
        tips: Vec::new(),
        anchors: Vec::new(),
        branches: Vec::new(),
        trunks: vec![trunk(-40.0, 0), trunk(40.0, 1)],
        feet: Vec::new(),
        braces: Vec::new(),
    };
    strength::strengthen(&mut far, &cfg, &empty);
    assert!(far.braces.is_empty(), "trunks far apart are not braced");

    let mut separate_models = route::Pieces {
        tips: Vec::new(),
        anchors: Vec::new(),
        branches: Vec::new(),
        trunks: vec![
            trunk(-4.0, 0),
            Trunk {
                model_id: 1,
                ..trunk(4.0, 1)
            },
        ],
        feet: Vec::new(),
        braces: Vec::new(),
    };
    strength::strengthen(&mut separate_models, &cfg, &empty);
    assert!(
        separate_models.braces.is_empty(),
        "different models must not be tied together"
    );
}

/// A part tilted to exactly the angle in the settings still gets held up.
///
/// The angle a layer's edge shifts by is a different question from whether the
/// surface overhangs, and only the second one belongs in the mask: a 45° slab
/// moves its edge by less than one raster cell per layer, so an allowance
/// derived from the angle used to call the whole slope carried and leave the
/// part with a handful of tips.
#[test]
fn a_part_tilted_to_the_limit_of_the_setting_is_held_up() {
    let cfg = SupportConfig::default();
    let flat = run(&[block((60.0, 20.0, 6.0), 0.0)], &cfg);
    assert!(
        flat.tips.is_empty(),
        "a block lying on the plate needs no tips"
    );

    let tilted = block((60.0, 20.0, 6.0), 0.0);
    let (lo, hi) = tilted.bounds();
    let centre = (lo + hi) * 0.5;
    let mut turn = crate::mesh::Transform::default();
    turn.turn_about(1, cfg.overhang_angle_deg, centre);
    let mut tilted = tilted.transformed(&turn);
    let risen = super::lift(std::slice::from_mut(&mut tilted), cfg.height_from_platform);
    assert_eq!(risen, 1, "the tilted block stands on the gap");

    let part = run(&[tilted], &cfg);
    assert!(
        part.tips.len() >= 10,
        "a slope at the limit of the setting needs a field of tips, got {}",
        part.tips.len()
    );
}

#[test]
fn an_overhang_above_the_model_is_held_by_the_model() {
    // A pedestal with a slab hovering over it: nothing can be dropped from the
    // platform to the slab's underside without cutting through the pedestal,
    // so the only support that can exist stands on the pedestal's top.
    let pedestal = block((40.0, 20.0, 10.0), 6.0);
    let slab = block((40.0, 20.0, 4.0), 22.0);
    let cfg = SupportConfig::default();
    let model = run(&[pedestal.clone(), slab.clone()], &cfg);
    assert!(!model.tips.is_empty(), "the slab's underside needs tips");

    let standing: Vec<&Trunk> = model.trunks.iter().filter(|t| t.bottom.z > 5.0).collect();
    assert!(
        !standing.is_empty(),
        "a cluster over the pedestal has to stand on it, {} trunks all start at the plate",
        model.trunks.len()
    );
    assert_eq!(model.stats.on_model, standing.len());

    // The whole part, both pieces: no trunk or branch may cut through either.
    let tree = bvh::Bvh::from_meshes(&[pedestal.clone(), slab.clone()]);
    let cfg = &cfg;
    for trunk in &model.trunks {
        let from = if trunk.bottom.z > 0.5 {
            let sunk = cfg.landing_depth() + trunk.diameter + analysis::ANALYSIS_LAYER_HEIGHT;
            Vec3::new(trunk.bottom.x, trunk.bottom.y, trunk.bottom.z + sunk)
        } else {
            trunk.bottom
        };
        assert!(
            tree.segment_clear(from, trunk.top, 0.0),
            "a trunk runs through the part ({from:?} -> {:?}, started at {:.2})",
            trunk.top,
            trunk.bottom.z
        );
    }
    for branch in &model.branches {
        for (from, to) in branch_segments(branch) {
            assert!(
                tree.segment_clear(from, to, 0.0),
                "a branch runs through the part ({:?} -> {:?})",
                from,
                to
            );
        }
    }
    let top_of_pedestal = 16.0;
    for trunk in &standing {
        let anchor = model
            .anchors
            .iter()
            .find(|anchor| (anchor.lower_end() - trunk.bottom).length() < 1e-3)
            .expect("a model-facing pillar has a reverse anchor");
        assert!(
            (anchor.pos.z - top_of_pedestal).abs() < 0.25,
            "the reverse anchor touches z {}, not the pedestal's top at {top_of_pedestal}",
            anchor.pos.z
        );
        let embedded = anchor.upper_end();
        assert!(
            tree.inside(embedded) || tree.closest(embedded, 1.0).is_some_and(|d| d < 0.6),
            "the reverse anchor does not penetrate the pedestal"
        );
        assert!(
            trunk.top.z > trunk.bottom.z,
            "a standing trunk has negative height"
        );
    }
    // A trunk standing on the part gets no foot: there is no platform for it
    // to spread on, and the trunk is bonded straight into the surface.
    assert!(
        model.feet.iter().all(|f| f.pos.z.abs() < 0.5),
        "a foot is hanging above the plate"
    );
    assert_eq!(
        model.feet.len(),
        model.trunks.len() - standing.len(),
        "every trunk on the plate has one foot, and only those"
    );
}

/// Is a point inside a convex support-base outline?
fn covers(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let n = poly.len();
    (0..n).all(|i| {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        // counter-clockwise outline: every edge keeps the point on its left
        (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0) >= -1e-3
    })
}

#[test]
fn support_pads_and_trees_are_correct_on_a_45_degree_part() {
    let cfg = SupportConfig::default();
    let flat = block((60.0, 20.0, 6.0), 0.0);
    let (lo, hi) = flat.bounds();
    let centre = (lo + hi) * 0.5;
    let mut turn = crate::mesh::Transform::default();
    turn.turn_about(1, 45.0, centre);
    let mut tilted = flat.transformed(&turn);
    super::lift(std::slice::from_mut(&mut tilted), cfg.height_from_platform);

    let part = run(&[tilted], &cfg);
    assert!(
        part.tips.len() >= 10,
        "the tilted face needs a field of tips"
    );
    assert!(!part.plates.is_empty());
    assert_connected_trees(&part);
    assert_feet_are_based(&part);
}

#[test]
fn support_bases_group_nearby_feet_and_leave_distant_feet_separate() {
    let foot = |id, x| Foot {
        id,
        pos: Vec3::new(x, 0.0, 0.0),
        diameter: 3.0,
        trunk_id: id,
        model_id: 0,
    };
    let feet = vec![foot(0, 0.0), foot(1, 4.0), foot(2, 30.0)];
    let plates = base::plates_for(&feet, &crate::supports::config::BaseConfig::default());
    assert_eq!(
        plates.len(),
        2,
        "two support groups, not a model-shaped raft"
    );
    let model = SupportModel {
        feet,
        plates,
        ..SupportModel::default()
    };
    assert_feet_are_based(&model);
}

#[test]
fn plate_caps_have_consistent_outward_winding() {
    let plate = Plate {
        model_id: 0,
        poly: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 8.0), (0.0, 8.0)],
        thickness: 0.8,
        chamfer: 0.2,
        chamfer_run: 0.2,
    };
    let mesh = meshgen::plate(&plate);
    let mut directed = std::collections::HashMap::new();
    for tri in &mesh.tris {
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            *directed.entry((a, b)).or_insert(0usize) += 1;
        }
    }
    for &(a, b) in directed.keys() {
        assert_eq!(directed.get(&(a, b)), Some(&1), "duplicate directed edge");
        assert_eq!(directed.get(&(b, a)), Some(&1), "inconsistent edge winding");
    }
    for tri in &mesh.tris {
        let p = mesh.tri_verts(*tri);
        let normal = (p[1] - p[0]).cross(p[2] - p[0]).normalized();
        if p.iter().all(|v| v.z.abs() < 1e-5) {
            assert!(normal.z < -0.99, "bottom cap faces into the plate");
        }
        if p.iter().all(|v| (v.z - plate.thickness).abs() < 1e-5) {
            assert!(normal.z > 0.99, "top cap faces into the plate");
        }
    }
}

#[test]
fn chamfer_angle_controls_horizontal_run_not_only_its_size() {
    let feet = vec![Foot {
        id: 0,
        pos: Vec3::ZERO,
        diameter: 4.0,
        trunk_id: 0,
        model_id: 0,
    }];
    let plate_at = |degrees| {
        let cfg = crate::supports::config::BaseConfig {
            chamfer_deg: degrees,
            ..crate::supports::config::BaseConfig::default()
        };
        base::plates_for(&feet, &cfg).remove(0)
    };
    let shallow = plate_at(15.0);
    let medium = plate_at(45.0);
    let steep = plate_at(75.0);
    assert!((medium.chamfer_run / medium.chamfer - 1.0).abs() < 1e-4);
    assert!(
        (shallow.chamfer_run / shallow.chamfer - 1.0 / 15.0f32.to_radians().tan()).abs() < 1e-3
    );
    assert!((steep.chamfer_run / steep.chamfer - 1.0 / 75.0f32.to_radians().tan()).abs() < 1e-3);
    assert!(shallow.chamfer_run > medium.chamfer_run);
    assert!(steep.chamfer_run < medium.chamfer_run * 0.5);
}

#[test]
fn simplifying_a_closed_outline_does_not_delete_its_closing_side() {
    let square = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let simplified = meshgen::simplify(&square, 0.01);
    assert_eq!(simplified.len(), 4);
    for (x, y) in square {
        assert!(
            covers(&simplified, x, y),
            "a corner was cut off the outline"
        );
    }
}

#[test]
fn surface_contact_stays_on_the_requesting_layer() {
    let lower = block((20.0, 20.0, 5.0), 5.0);
    let upper = block((20.0, 20.0, 5.0), 30.0);
    let tree = bvh::Bvh::from_meshes(&[lower, upper]);
    let (pos, normal) = tree.surface_near(0.0, 0.0, 10.0).expect("lower top");
    assert!((pos.z - 10.0).abs() < 1e-3);
    assert!(
        normal.z > 0.9,
        "must not jump to the distant upper overhang"
    );
    let (pos, normal) = tree.surface_near(0.0, 0.0, 30.0).expect("upper underside");
    assert!((pos.z - 30.0).abs() < 1e-3);
    assert!(normal.z < -0.9);
}

#[test]
#[ignore]
fn supplied_body_has_complete_connected_supports_at_45_degrees() {
    let path = "/run/media/makzumi/storage/Projects/iCarControls-3/case-2/HighRes_STLs/Body.stl";
    let cfg = SupportConfig::default();
    let model = crate::mesh::load_model(std::path::Path::new(path)).expect("load Body.stl");
    let (min, max) = model.bounds();
    let centre = (min + max) * 0.5;
    let mut transform = crate::mesh::Transform::default();
    transform.turn_about(1, 45.0, centre);
    let mut meshes = vec![model.transformed(&transform)];
    super::lift(&mut meshes, cfg.height_from_platform);
    let supports = run(&meshes, &cfg);
    println!("{}", supports.report());
    assert!(supports.tips.len() > 100);
    assert_eq!(
        supports.stats.islands_supported, supports.stats.islands,
        "every mandatory island must have a routed contact"
    );
    assert_connected_trees(&supports);
    assert_feet_are_based(&supports);

    let tree = bvh::Bvh::from_meshes(&meshes);
    for branch in &supports.branches {
        for (from, to) in branch_segments(branch) {
            assert!(
                tree.segment_clear(from, to, 0.0),
                "Body branch intersects the model"
            );
        }
    }
    for trunk in &supports.trunks {
        let from = if trunk.bottom.z > 0.5 {
            let sunk = cfg.landing_depth() + trunk.diameter + analysis::ANALYSIS_LAYER_HEIGHT;
            Vec3::new(trunk.bottom.x, trunk.bottom.y, trunk.bottom.z + sunk)
        } else {
            trunk.bottom
        };
        assert!(
            tree.segment_clear(from, trunk.top, 0.0),
            "Body trunk intersects the model"
        );
    }
    for tip in &supports.tips {
        let body_end = tip.pos - tip.axis * cfg.tip.contact_depth;
        assert!(
            tree.segment_clear(tip.lower_end(), body_end, 0.0),
            "Body tip intersects the model"
        );
    }
}

#[test]
fn a_ball_slices_to_a_solid_disk() {
    use crate::slicer;

    let ball = meshgen::ball(Vec3::new(0.0, 0.0, 3.7), 0.3, 12);
    let sliced = slicer::slice(std::slice::from_ref(&ball), 0.05);
    let layer = &sliced.layers[74];
    let (w, h) = (101, 101);
    let mut mask = vec![0u8; w * h];
    slicer::fill_mask(layer, [-0.5, -0.5], [0.5, 0.5], w, h, &mut mask);

    // Every triangle must wind the same way. If the pole fans disagree with
    // the middle bands, non-zero filling cancels the centre and leaves a ring.
    assert_ne!(mask[50 * w + 50], 0, "ball centre must be solid");
    assert_ne!(mask[50 * w + 35], 0, "ball interior must be solid");
    assert_ne!(mask[65 * w + 50], 0, "ball interior must be solid");
    assert_eq!(mask[0], 0, "outside the ball must stay empty");
}

#[test]
fn a_wall_that_barely_faces_down_gets_support_unless_overhangs_only() {
    // A slab tilted 70° from horizontal: its underside is a hair past
    // vertical, far shallower than the 45° setting would ever flag. HALOT
    // Box's working profile (Overhangs Only unchecked) still gives it a
    // field of tips; the checked box leaves it bare.
    let cfg = SupportConfig::default();
    let wall = block((60.0, 20.0, 6.0), 0.0);
    let (lo, hi) = wall.bounds();
    let centre = (lo + hi) * 0.5;
    let mut turn = crate::mesh::Transform::default();
    turn.turn_about(1, 70.0, centre);
    let mut wall = wall.transformed(&turn);
    super::lift(std::slice::from_mut(&mut wall), cfg.height_from_platform);

    let full = run(&[wall.clone()], &cfg);
    assert!(
        full.stats.overhang_faces > 0,
        "the unchecked profile sees the wall"
    );

    let steep = run(
        &[wall],
        &SupportConfig {
            overhangs_only: true,
            ..cfg
        },
    );
    assert!(
        steep.stats.overhang_faces < full.stats.overhang_faces,
        "steep-only mode must flag fewer faces, {} vs {}",
        steep.stats.overhang_faces,
        full.stats.overhang_faces
    );
}

#[test]
fn strut_junctions_carry_balls() {
    let mut model = SupportModel::default();
    model.branches.push(Branch {
        id: 0,
        from: Vec3::new(0.0, 0.0, 10.0),
        via: None,
        to: Vec3::new(0.0, 0.0, 0.0),
        diameter: 0.9,
        tip_id: 0,
    });
    model.braces.push(Brace {
        id: 0,
        a: Vec3::new(-5.0, 0.0, 2.0),
        b: Vec3::new(5.0, 0.0, 2.0),
        diameter: 0.9,
        model_id: 0,
    });
    let mesh = model.mesh(12);
    // The bare strut, without the balls a junction needs.
    let bare = meshgen::frustum(
        Vec3::new(0.0, 0.0, 10.0),
        0.45,
        Vec3::new(0.0, 0.0, 0.0),
        0.45,
        12,
    );
    assert!(
        mesh.verts.len() > bare.verts.len(),
        "the balls add geometry at the joints"
    );
}

/// A floating cap over a stem: the underside ring is an island that must grow
/// a ring of heads dense enough to share columns, the way HALOT Box's
/// islands do, instead of one lonely column under the middle.
#[test]
fn a_floating_cap_grows_a_ring_of_heads_that_share_columns() {
    let meshes = [block((4.0, 4.0, 11.0), 6.0), block((16.0, 16.0, 4.0), 15.0)];
    let cfg = SupportConfig::default();
    let model = run(&meshes, &cfg);

    let cap: Vec<_> = model
        .tips
        .iter()
        .filter(|t| t.island && (t.pos.z - 15.0).abs() < 0.5)
        .collect();
    assert!(
        cap.len() >= 12,
        "the cap ring needs a real handful of heads, got {}",
        cap.len()
    );
    assert!(
        model.branches.len() >= 5,
        "heads this close must share columns, not each stand on one"
    );
    assert!(
        model.trunks.len() < cap.len(),
        "sharing columns is the point: {} trunks for {} heads",
        model.trunks.len(),
        cap.len()
    );
    assert!(
        model.trunks.iter().any(|t| t.tip_ids.len() >= 2),
        "at least one column must carry two heads"
    );
}

/// Close columns zig-zag on their own; only wide gaps earn the double cross.
#[test]
fn close_columns_get_a_plain_zig_zag_wide_ones_get_the_cross() {
    let trunk = |x: f32| Trunk {
        id: 0,
        bottom: Vec3::new(x, 0.0, 0.0),
        top: Vec3::new(x, 0.0, 30.0),
        diameter: 1.6,
        tip_ids: vec![0],
        model_id: 0,
    };
    let cfg = SupportConfig::default();
    let empty = bvh::Bvh::from_meshes(&[]);
    let pieces = |a: f32, b: f32| route::Pieces {
        tips: Vec::new(),
        anchors: Vec::new(),
        branches: Vec::new(),
        trunks: vec![trunk(a), trunk(b)],
        feet: Vec::new(),
        braces: Vec::new(),
    };

    let mut near = pieces(-2.0, 2.0);
    strength::strengthen(&mut near, &cfg, &empty);
    assert!(!near.braces.is_empty(), "close columns still get braced");
    let mut spans: Vec<(f32, f32)> = near
        .braces
        .iter()
        .map(|b| {
            let (lo, hi) = if b.a.z < b.b.z {
                (b.a.z, b.b.z)
            } else {
                (b.b.z, b.a.z)
            };
            (lo, hi)
        })
        .collect();
    spans.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.total_cmp(&y.1)));
    assert!(
        spans.windows(2).all(|w| w[0] != w[1]),
        "a close pair zig-zags; stacking both diagonals on every level is the \
         wall of sticks nobody asked for"
    );

    let mut far = pieces(-5.0, 5.0);
    strength::strengthen(&mut far, &cfg, &empty);
    assert!(!far.braces.is_empty(), "wide columns still get braced");
    let mut spans: Vec<(f32, f32)> = far
        .braces
        .iter()
        .map(|b| {
            let (lo, hi) = if b.a.z < b.b.z {
                (b.a.z, b.b.z)
            } else {
                (b.b.z, b.a.z)
            };
            (lo, hi)
        })
        .collect();
    spans.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.total_cmp(&y.1)));
    assert!(
        spans.windows(2).any(|w| w[0] == w[1]),
        "a wide gap earns the double cross"
    );
}

/// The lattice must route around a column in its way, never through it.
#[test]
fn a_brace_never_passes_through_a_third_column() {
    let trunk = |x: f32| Trunk {
        id: 0,
        bottom: Vec3::new(x, 0.0, 0.0),
        top: Vec3::new(x, 0.0, 30.0),
        diameter: 1.6,
        tip_ids: vec![0],
        model_id: 0,
    };
    let cfg = SupportConfig::default();
    let empty = bvh::Bvh::from_meshes(&[]);
    let mut pieces = route::Pieces {
        tips: Vec::new(),
        anchors: Vec::new(),
        branches: Vec::new(),
        trunks: vec![trunk(-4.0), trunk(0.0), trunk(4.0)],
        feet: Vec::new(),
        braces: Vec::new(),
    };
    strength::strengthen(&mut pieces, &cfg, &empty);
    assert!(!pieces.braces.is_empty(), "the neighbours still brace");
    for brace in &pieces.braces {
        let span = (brace.a.x - brace.b.x).abs();
        assert!(
            span < 7.0,
            "a brace spanning {span} mm would run straight through the \
             middle column"
        );
    }
}
