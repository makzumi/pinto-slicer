//! MSLA support-point generation, ported from PrusaSlicer's layer-driven
//! approach (`SLA/SupportPointGenerator.cpp`).
//!
//! Slice regions are linked to the regions they grow from. Existing support
//! points propagate through those links with Prusa's height-dependent support
//! radius curve. New points are sampled on overhang outlines and inside new
//! islands. This is deliberately not a random triangle-normal sampler: the
//! question is which printed pixels a previous support can actually carry.

use std::sync::atomic::{AtomicBool, Ordering};

use super::bvh::Bvh;
use super::config::SupportConfig;
use super::CellGrid;
use crate::mesh::{Mesh, Vec3};
use crate::slicer;

/// Analysis resolution. Prusa uses the print layers; the application does not
/// pass its print layer height into support generation, so 0.1 mm is used as a
/// deterministic compromise between contact accuracy and generation time.
pub const ANALYSIS_LAYER_HEIGHT: f32 = 0.1;
const MIN_ANALYSIS_CELL: f32 = 0.25;
const MAX_ANALYSIS_CELL: f32 = 0.5;
const MAX_GRID_SIDE: usize = 512;
const REMOVE_OUTSIDE_PART: f32 = 5.0;

#[derive(Clone, Debug)]
pub struct Contact {
    pub pos: Vec3,
    pub normal: Vec3,
    pub island: bool,
    /// One-based island component id, shared by all contacts in that island.
    pub island_id: usize,
    pub model_id: usize,
}

#[derive(Default)]
pub struct Result {
    pub contacts: Vec<Contact>,
    pub candidate_points: usize,
    pub skipped: usize,
    pub coverage_added: usize,
    pub islands: usize,
    pub islands_supported: usize,
    pub overhang_faces: usize,
    pub regions: usize,
    pub debug_mesh: Mesh,
    pub debug_islands: Vec<Vec3>,
}

#[derive(Clone, Copy)]
struct Grid {
    min_x: f32,
    min_y: f32,
    cell: f32,
    w: usize,
    h: usize,
}

impl Grid {
    fn point(self, x: usize, y: usize) -> (f32, f32) {
        (
            self.min_x + (x as f32 + 0.5) * self.cell,
            self.min_y + (y as f32 + 0.5) * self.cell,
        )
    }

    fn idx(self, x: usize, y: usize) -> usize {
        y * self.w + x
    }
}

struct Component {
    cells: Vec<usize>,
    center: (f32, f32),
    min: (usize, usize),
    max: (usize, usize),
}

/// A current slice component and the support points inherited from the
/// component(s) immediately below it.
struct ActivePart {
    cells: Vec<usize>,
    supports: Vec<usize>,
    /// The overhang coastline this part has accumulated over its layers, in
    /// the spirit of Prusa's propagated coverage: a shallow band only peels
    /// open a few cells per layer, so the ring is walked as one shape once
    /// enough of it has arrived. Each cell carries the height it first
    /// appeared at, so the walk can put a head on the surface it belongs to.
    coast: Vec<(usize, f32)>,
}

pub fn generate(meshes: &[Mesh], cfg: &SupportConfig, tree: &Bvh, cancel: &AtomicBool) -> Result {
    let mut out = Result::default();
    if meshes.is_empty() || tree.tri_count() == 0 {
        return out;
    }

    let (min, max) = bounds(meshes);
    let grid = make_grid(min, max);
    let slices = slicer::slice(meshes, ANALYSIS_LAYER_HEIGHT);
    if slices.layers.is_empty() {
        return out;
    }

    let trees: Vec<Bvh> = meshes
        .iter()
        .map(|mesh| Bvh::from_meshes(std::slice::from_ref(mesh)))
        .collect();
    out.debug_mesh = downward_faces(meshes, cfg, &mut out.overhang_faces);

    let masks: Vec<Vec<bool>> = slices
        .layers
        .iter()
        .map(|layer| {
            slicer::occupancy(
                layer,
                [grid.min_x, grid.min_y],
                [
                    grid.min_x + grid.w as f32 * grid.cell,
                    grid.min_y + grid.h as f32 * grid.cell,
                ],
                grid.w,
                grid.h,
            )
        })
        .collect();

    let model_on_plate = min.z <= 0.05;
    let mut saw_material = false;
    let mut previous = vec![false; grid.w * grid.h];
    let mut previous_parts: Vec<ActivePart> = Vec::new();

    for (layer_idx, current) in masks.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if !current.iter().any(|occupied| *occupied) {
            previous.fill(false);
            previous_parts.clear();
            continue;
        }

        let z = slices.layers[layer_idx].z;
        let parts = components(current, grid, cancel);
        let mut previous_owner = vec![usize::MAX; current.len()];
        for (part_id, part) in previous_parts.iter().enumerate() {
            for &cell in &part.cells {
                previous_owner[cell] = part_id;
            }
        }

        let on_plate = model_on_plate && !saw_material;
        let mut active_parts = Vec::with_capacity(parts.len());
        for component in parts {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let mut links: Vec<usize> = component
                .cells
                .iter()
                .filter_map(|cell| {
                    let owner = previous_owner[*cell];
                    (owner != usize::MAX).then_some(owner)
                })
                .collect();
            links.sort_unstable();
            links.dedup();

            let island = links.is_empty() && !on_plate;
            let mut supports: Vec<usize> = links
                .iter()
                .flat_map(|id| previous_parts[*id].supports.iter().copied())
                .collect();
            supports.sort_unstable();
            supports.dedup();

            let mut coast: Vec<(usize, f32)> = links
                .iter()
                .flat_map(|id| previous_parts[*id].coast.iter().copied())
                .collect();
            coast.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
            coast.dedup_by(|a, b| a.0 == b.0);

            // Prusa removes inherited points outside the current region. A
            // later, separate overhang at the same XY can then be supported
            // again instead of being hidden by an unrelated lower point.
            let part_cells = spatial_cells(&component, grid);
            supports.retain(|id| {
                let contact = &out.contacts[*id];
                cells_contain_near(
                    &part_cells,
                    grid,
                    &component,
                    contact.pos,
                    REMOVE_OUTSIDE_PART,
                )
            });

            if island {
                out.islands += 1;
                add_island_contacts(
                    &component,
                    grid,
                    z,
                    cfg,
                    tree,
                    &trees,
                    &mut out,
                    &mut supports,
                );
            } else {
                // Wide partial overhangs receive the same interior/outline
                // treatment as islands (Prusa's peninsula path).
                for peninsula in peninsula_regions(&component, &previous, grid, cancel) {
                    out.islands += 1;
                    add_island_contacts(
                        &peninsula,
                        grid,
                        z,
                        cfg,
                        tree,
                        &trees,
                        &mut out,
                        &mut supports,
                    );
                }
                let overhang: Vec<usize> = unsupported_cells(&component, &previous, grid)
                    .into_iter()
                    .filter(|&cell| {
                        // Only a cell whose surface fails the overhang angle
                        // joins the band. The difference also contains the
                        // one-cell steps a steep wall makes every few
                        // layers; those need nothing, and letting them in
                        // meant the walk spent itself proposing heads that
                        // the angle gate then refused, over and over.
                        let (cx, cy) = grid.point(cell % grid.w, cell / grid.w);
                        let max_delta = (ANALYSIS_LAYER_HEIGHT * 2.0 + grid.cell).max(0.75);
                        tree.downward_surface_near(cx, cy, z, max_delta)
                            .map(|(_, normal)| normal.z <= overhang_limit(cfg) + 1e-4)
                            .unwrap_or(false)
                    })
                    .collect();
                if !overhang.is_empty() {
                    out.regions += 1;
                    // The coastline is where the layer grew. Each layer only
                    // peels a few cells open, so the walk runs over what the
                    // part has accumulated — the whole band by the time it
                    // finishes — and a head goes every tip distance along it,
                    // the same walk the islands take. Per-layer-only sampling
                    // starved thin bands (a rim ring earned one or two heads
                    // a layer, and the coverage gate ate most of those) where
                    // the reference slicer's grid plants a whole ring.
                    coast.extend(overhang.iter().map(|&cell| (cell, z)));
                    coast.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
                    coast.dedup_by(|a, b| a.0 == b.0);
                    let mut band = vec![false; grid.w * grid.h];
                    for &(cell, _) in &coast {
                        band[cell] = true;
                    }
                    let z_of: std::collections::HashMap<usize, f32> =
                        coast.iter().copied().collect();
                    let spacing = cfg.touch_tip_distance.clamp(2.0, 24.0);
                    let mut samples = Vec::new();
                    for piece in components(&band, grid, cancel) {
                        let path = trace_outline(&piece.cells, grid);
                        samples.extend(sample_outline(&path, grid, spacing));
                    }
                    for (x, y) in samples {
                        let (px, py) = grid.point(x, y);
                        let cell = y * grid.w + x;
                        // Each sample is a coast cell, so it knows the height
                        // it appeared at. Asking there finds the surface the
                        // band belongs to; asking at the current layer would
                        // raycast past the part for cells that arrived long
                        // ago.
                        let sample_z = z_of.get(&cell).copied().unwrap_or(z);
                        if covered(&out.contacts, px, py, sample_z, grid, cfg) {
                            continue;
                        }
                        if add_contact(
                            Vec3::new(px, py, sample_z),
                            false,
                            grid,
                            cfg,
                            tree,
                            &trees,
                            &mut out,
                        ) {
                            supports.push(out.contacts.len() - 1);
                            out.coverage_added += 1;
                        }
                    }
                    // A band stacked over an older one buries its cells
                    // inside the accumulated blob, where the outline walk
                    // never visits them: a hollow's ceiling sits right over
                    // the band its own funnel grew. A fresh cell surrounded
                    // by the band on every side is one of those, so it
                    // becomes a candidate at the height it appeared; a fresh
                    // cell on the blob's rim belongs to the outline walk,
                    // which already proposes it. The coverage gate keeps the
                    // spacing honest either way.
                    let interior = |cell: usize| {
                        let (cx, cy) = (cell % grid.w, cell / grid.w);
                        for dy in [usize::MAX, 0, 1] {
                            for dx in [usize::MAX, 0, 1] {
                                if dx == 0 && dy == 0 {
                                    continue;
                                }
                                let nx = cx.wrapping_add(dx);
                                let ny = cy.wrapping_add(dy);
                                if nx >= grid.w || ny >= grid.h {
                                    return false;
                                }
                                if !band[ny * grid.w + nx] {
                                    return false;
                                }
                            }
                        }
                        true
                    };
                    for &cell in &overhang {
                        if !interior(cell) {
                            continue;
                        }
                        let (px, py) = grid.point(cell % grid.w, cell / grid.w);
                        if covered(&out.contacts, px, py, z, grid, cfg) {
                            continue;
                        }
                        if add_contact(
                            Vec3::new(px, py, z),
                            false,
                            grid,
                            cfg,
                            tree,
                            &trees,
                            &mut out,
                        ) {
                            supports.push(out.contacts.len() - 1);
                            out.coverage_added += 1;
                        }
                    }
                }
            }
            active_parts.push(ActivePart {
                cells: component.cells,
                supports,
                coast,
            });
        }

        previous.clone_from(current);
        previous_parts = active_parts;
        saw_material = true;
    }

    out.contacts.sort_by(|a, b| {
        a.pos
            .z
            .total_cmp(&b.pos.z)
            .then_with(|| a.pos.x.total_cmp(&b.pos.x))
            .then_with(|| a.pos.y.total_cmp(&b.pos.y))
    });
    out.debug_islands = out
        .contacts
        .iter()
        .filter(|contact| contact.island)
        .map(|contact| contact.pos)
        .collect();
    out
}

/// The spacing Prusa's coverage curve is calibrated for. Every other spacing
/// scales the curve linearly from this one.
pub const REFERENCE_TIP_SPACING: f32 = 4.0;

/// How far above a head its coverage still reaches: the first leg of
/// Prusa's curve, where one band's columns march together. Above this a
/// candidate belongs to a different overhang and gets its own head.
const PROPAGATION_REACH: f32 = 4.0;

/// In full coverage ("Overhangs Only" unchecked) this is how far past
/// vertical a wall has to lean before it earns a head: steep-enough slopes,
/// flat undersides and hollow ceilings all qualify, near-vertical walls stay
/// clean.
const FULL_COVERAGE_LEAN: f32 = 20.0;

/// The steepest downward normal that still counts as an overhang worth a
/// head. In Overhangs-Only mode that is the panel's angle; in full coverage
/// it is everything leaning more than [`FULL_COVERAGE_LEAN`] past vertical,
/// which is what the hollow ceilings and shallow shelves need.
fn overhang_limit(cfg: &SupportConfig) -> f32 {
    if cfg.overhangs_only {
        -cfg.overhang_angle_deg.clamp(1.0, 89.0).to_radians().sin()
    } else {
        -FULL_COVERAGE_LEAN.to_radians().sin()
    }
}

/// How much wider (or tighter) the spacing is than the reference curve.
/// The clamp only guards against absurd settings; the panel's range is well
/// inside it, so the slider's millimetres are the spacing that is aimed for.
fn density_scale(cfg: &SupportConfig) -> f32 {
    (cfg.touch_tip_distance / REFERENCE_TIP_SPACING).clamp(0.25, 4.0)
}

/// Prusa's default support curve: (radius mm, height above contact mm).
fn support_radius(z_distance: f32, cfg: &SupportConfig) -> f32 {
    if z_distance < 0.0 {
        return 0.0;
    }
    // The whole curve is scaled, first layer included. Leaving the first layer
    // at a fixed 3.2 mm put a floor under every spacing: two tips could never
    // be further apart than the curve allows, whatever the panel said.
    let scale = density_scale(cfg);
    let curve = [(3.2, 0.0), (4.0, 3.9), (5.0, 15.0), (6.0, 40.0)];
    for pair in curve.windows(2) {
        let (r0, z0) = pair[0];
        let (r1, z1) = pair[1];
        if z_distance <= z1 {
            let t = ((z_distance - z0) / (z1 - z0)).clamp(0.0, 1.0);
            return (r0 + (r1 - r0) * t) * scale;
        }
    }
    6.0 * scale
}

fn covered(contacts: &[Contact], x: f32, y: f32, z: f32, grid: Grid, cfg: &SupportConfig) -> bool {
    // The whole model's contacts, not just the ones this part inherited: a
    // band cell can be far from where its part lives by now (a rim's cells
    // stay in the band while the part has narrowed to a stem above them), and
    // the head that already sits on that cell is just as real there.
    contacts.iter().any(|contact| {
        // The raycast snaps a contact onto the surface, which can sit a hair
        // above the layer it was found on; that still counts as level. A
        // contact genuinely higher up supports nothing below it.
        let dz_raw = z - contact.pos.z;
        if dz_raw < -(ANALYSIS_LAYER_HEIGHT * 2.0 + grid.cell) {
            return false;
        }
        // The curve's first leg only: a head claims the slope just above
        // it, where the columns of one band march together, and a leaning
        // wall does not grow a stacked column of heads as it climbs. Farther
        // up is a different overhang — a hollow's ceiling over the band its
        // own funnel grew — and it earns its own heads; counting a far-below
        // head as coverage starved whole rings and hollow ceilings.
        let radius = if dz_raw > PROPAGATION_REACH {
            0.0
        } else {
            support_radius(dz_raw.max(0.0), cfg)
        };
        let dx = contact.pos.x - x;
        let dy = contact.pos.y - y;
        radius > 0.0 && dx * dx + dy * dy < radius * radius
    })
}

fn spatial_cells(component: &Component, grid: Grid) -> CellGrid {
    let mut index = CellGrid::new(REMOVE_OUTSIDE_PART);
    for &cell in &component.cells {
        let (x, y) = grid.point(cell % grid.w, cell / grid.w);
        index.insert(Vec3::new(x, y, 0.0), cell as u32);
    }
    index
}

fn cells_contain_near(
    cells: &CellGrid,
    grid: Grid,
    component: &Component,
    pos: Vec3,
    distance: f32,
) -> bool {
    let mut nearby = Vec::new();
    cells.near(pos, 1, &mut nearby);
    nearby.iter().any(|cell| {
        let index = *cell as usize;
        let (x, y) = grid.point(index % grid.w, index / grid.w);
        (x - pos.x).powi(2) + (y - pos.y).powi(2) <= distance * distance
    }) || component.cells.is_empty()
}

fn unsupported_cells(component: &Component, previous: &[bool], grid: Grid) -> Vec<usize> {
    let membership: std::collections::HashSet<usize> = component.cells.iter().copied().collect();
    component
        .cells
        .iter()
        .copied()
        .filter(|cell| {
            if previous[*cell] {
                return false;
            }
            // Sample the outline of the new area, including its holes. The
            // interior is carried by propagated coverage or island sampling.
            let x = *cell % grid.w;
            let y = *cell / grid.w;
            let current_overhang = |nx: i32, ny: i32| {
                nx >= 0
                    && ny >= 0
                    && nx < grid.w as i32
                    && ny < grid.h as i32
                    && membership.contains(&(ny as usize * grid.w + nx as usize))
                    && !previous[ny as usize * grid.w + nx as usize]
            };
            (-1..=1).any(|dy| {
                (-1..=1).any(|dx| {
                    (dx != 0 || dy != 0) && !current_overhang(x as i32 + dx, y as i32 + dy)
                })
            })
        })
        .collect()
}

/// Prusa's peninsula test: retain an unsupported region only when part of it
/// is more than 2 mm from the previous layer. The region itself starts 1.5 mm
/// away from the previous layer, so narrow overhangs stay on the ordinary
/// coastline-sampling path.
fn peninsula_regions(
    component: &Component,
    previous: &[bool],
    grid: Grid,
    cancel: &AtomicBool,
) -> Vec<Component> {
    let mut mask = vec![false; previous.len()];
    for &cell in &component.cells {
        if !covered_by_previous(cell, previous, grid, 1.5) {
            mask[cell] = true;
        }
    }
    components(&mask, grid, cancel)
        .into_iter()
        .filter(|region| {
            region
                .cells
                .iter()
                .any(|cell| !covered_by_previous(*cell, previous, grid, 2.0))
        })
        .collect()
}

fn covered_by_previous(cell: usize, previous: &[bool], grid: Grid, distance: f32) -> bool {
    let x = cell % grid.w;
    let y = cell / grid.w;
    let reach = (distance / grid.cell).ceil() as i32;
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx >= 0
                && ny >= 0
                && nx < grid.w as i32
                && ny < grid.h as i32
                && previous[ny as usize * grid.w + nx as usize]
            {
                return true;
            }
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn add_island_contacts(
    component: &Component,
    grid: Grid,
    z: f32,
    cfg: &SupportConfig,
    tree: &Bvh,
    trees: &[Bvh],
    out: &mut Result,
    supports: &mut Vec<usize>,
) {
    // Islands get the same density the column slicers give them: one head
    // per tip-spacing square of the interior, and a head every spacing
    // along the wall. Prusa's sampler walks the outer wall of a small
    // overhang for the same reason ("add the outer wall (of the overhang)
    // to ensure it is correct supported instead",
    // TreeSupport::sample_overhang_area) — a knob's crown is exactly that
    // small ring, and a wall of heads routes into shared columns and
    // braces into one tree instead of a single lonely column.
    let inner = cfg.touch_tip_distance.clamp(2.0, 24.0);

    let interior = grid_cells(&component.cells, grid, inner);
    let rim = sample_outline(&trace_outline(&component.cells, grid), grid, inner);
    let candidates = interior
        .into_iter()
        .map(|p| (p, false))
        .chain(rim.into_iter().map(|p| (p, true)));
    for ((x, y), on_wall) in candidates {
        let (px, py) = grid.point(x, y);
        // The wall walk can step close to the interior squares. Keep the
        // heads apart by half a spacing there, but do not use the larger
        // propagated support radius inside a new island.
        let keep_clear = if on_wall {
            (inner * 0.5).powi(2)
        } else {
            cfg.tip.upper_diameter.max(0.2).powi(2)
        };
        let duplicate = supports.iter().any(|id| {
            let contact = &out.contacts[*id];
            (contact.pos.x - px).powi(2) + (contact.pos.y - py).powi(2) < keep_clear
        });
        if duplicate {
            continue;
        }
        if add_contact(Vec3::new(px, py, z), true, grid, cfg, tree, trees, out) {
            out.contacts.last_mut().unwrap().island_id = out.islands;
            supports.push(out.contacts.len() - 1);
        }
    }
}

fn add_contact(
    approximate: Vec3,
    island: bool,
    grid: Grid,
    cfg: &SupportConfig,
    tree: &Bvh,
    trees: &[Bvh],
    out: &mut Result,
) -> bool {
    out.candidate_points += 1;
    let max_delta = (ANALYSIS_LAYER_HEIGHT * 2.0 + grid.cell).max(0.75);
    let hit = tree
        .downward_surface_near(approximate.x, approximate.y, approximate.z, max_delta)
        .or_else(|| {
            tree.closest_point(approximate, grid.cell * 1.5)
                .filter(|(_, normal)| normal.z < 0.866)
        });
    let Some((pos, normal)) = hit else {
        out.skipped += 1;
        return false;
    };
    // Prusa rejects normals beyond the 150 degree cutoff. Islands must be on
    // a downward face; slope samples may be on a near-vertical side and will
    // have their pinhead tilted to the bridge angle later.
    if normal.z >= 0.866 || (island && normal.z >= -0.02) || pos.z <= 0.02 {
        out.skipped += 1;
        return false;
    }
    // Full coverage honours the panel's switch: with Overhangs Only off,
    // a hollow ceiling leaning less than the panel's angle still earns its
    // forest, while near-vertical walls stay clean.
    if !island && normal.z > overhang_limit(cfg) + 1e-4 {
        out.skipped += 1;
        return false;
    }
    let model_id = owner_of(pos, trees);
    out.contacts.push(Contact {
        pos,
        normal,
        island,
        island_id: 0,
        model_id,
    });
    true
}

fn owner_of(pos: Vec3, trees: &[Bvh]) -> usize {
    let mut owner = 0;
    let mut nearest = f32::MAX;
    for (index, tree) in trees.iter().enumerate() {
        if tree.inside(pos) {
            return index;
        }
        if let Some(distance) = tree.closest(pos, nearest) {
            nearest = distance;
            owner = index;
        }
    }
    owner
}

fn bounds(meshes: &[Mesh]) -> (Vec3, Vec3) {
    let mut min = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
    let mut max = Vec3::new(f32::MIN, f32::MIN, f32::MIN);
    for mesh in meshes {
        if mesh.tris.is_empty() {
            continue;
        }
        let (a, b) = mesh.bounds();
        min = min.min_each(a);
        max = max.max_each(b);
    }
    (min, max)
}

fn make_grid(min: Vec3, max: Vec3) -> Grid {
    let span_x = (max.x - min.x).max(1.0);
    let span_y = (max.y - min.y).max(1.0);
    let mut cell = MAX_ANALYSIS_CELL;
    cell = cell.max(span_x / MAX_GRID_SIDE as f32);
    cell = cell.max(span_y / MAX_GRID_SIDE as f32);
    cell = cell.max(MIN_ANALYSIS_CELL);
    let w = (span_x / cell).ceil().max(1.0) as usize;
    let h = (span_y / cell).ceil().max(1.0) as usize;
    Grid {
        min_x: min.x,
        min_y: min.y,
        cell,
        w,
        h,
    }
}

fn components(mask: &[bool], grid: Grid, cancel: &AtomicBool) -> Vec<Component> {
    let mut seen = vec![false; mask.len()];
    let mut result = Vec::new();
    for y in 0..grid.h {
        for x in 0..grid.w {
            if cancel.load(Ordering::Relaxed) {
                return result;
            }
            let start = grid.idx(x, y);
            if !mask[start] || seen[start] {
                continue;
            }
            let mut cells = Vec::new();
            let mut stack = vec![start];
            seen[start] = true;
            let (mut sum_x, mut sum_y) = (0.0, 0.0);
            let (mut min_x, mut min_y) = (x, y);
            let (mut max_x, mut max_y) = (x, y);
            while let Some(idx) = stack.pop() {
                let cx = idx % grid.w;
                let cy = idx / grid.w;
                cells.push(idx);
                sum_x += cx as f32 + 0.5;
                sum_y += cy as f32 + 0.5;
                min_x = min_x.min(cx);
                min_y = min_y.min(cy);
                max_x = max_x.max(cx);
                max_y = max_y.max(cy);
                for (dx, dy) in [
                    (-1i32, -1i32),
                    (0, -1),
                    (1, -1),
                    (-1, 0),
                    (1, 0),
                    (-1, 1),
                    (0, 1),
                    (1, 1),
                ] {
                    let nx = cx as i32 + dx;
                    let ny = cy as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= grid.w as i32 || ny >= grid.h as i32 {
                        continue;
                    }
                    let next = grid.idx(nx as usize, ny as usize);
                    if mask[next] && !seen[next] {
                        seen[next] = true;
                        stack.push(next);
                    }
                }
            }
            let count = cells.len() as f32;
            result.push(Component {
                cells,
                center: (sum_x / count, sum_y / count),
                min: (min_x, min_y),
                max: (max_x, max_y),
            });
        }
    }
    result
}

/// One head per spacing x spacing square that overlaps the component,
/// sitting at the cell nearest the middle of its square. This is the
/// count the column slicers actually place; the spread-out picker above
/// stays for the gentler region shapes.
fn grid_cells(cells: &[usize], grid: Grid, spacing: f32) -> Vec<(usize, usize)> {
    let spacing = spacing.max(grid.cell);
    let step = ((spacing / grid.cell).round() as usize).max(1);
    let mut best: std::collections::HashMap<(usize, usize), (usize, usize, f32)> =
        std::collections::HashMap::with_capacity(cells.len());
    for &idx in cells {
        let x = idx % grid.w;
        let y = idx / grid.w;
        let key = (x / step, y / step);
        let cx = (key.0 * step) as f32 + (step as f32 - 1.0) * 0.5;
        let cy = (key.1 * step) as f32 + (step as f32 - 1.0) * 0.5;
        let d = (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2);
        use std::collections::hash_map::Entry;
        match best.entry(key) {
            Entry::Vacant(slot) => {
                slot.insert((x, y, d));
            }
            Entry::Occupied(mut slot) => {
                if d < slot.get().2 {
                    *slot.get_mut() = (x, y, d);
                }
            }
        }
    }
    // HashMap iteration order is random per run; the picks are a
    // candidate list, so their order decides which near-misses the
    // keep-clear gate sees first. Sorting keeps every run identical.
    let mut picked: Vec<(usize, usize)> = best.into_values().map(|(x, y, _)| (x, y)).collect();
    picked.sort_unstable();
    picked
}

/// The outline cells in walking order (Moore tracing), so a shelf's edge
/// can carry a head every tip spacing. Prusa's sampler walks the outer
/// wall of a small overhang for the same reason; the reference is
/// TreeSupport::sample_overhang_area, "add the outer wall (of the
/// overhang) to ensure it is correct supported instead".
fn trace_outline(cells: &[usize], grid: Grid) -> Vec<(usize, usize)> {
    if cells.is_empty() {
        return Vec::new();
    }
    let membership: std::collections::HashSet<usize> = cells.iter().copied().collect();
    let at = |x: i32, y: i32| -> Option<usize> {
        if x < 0 || y < 0 || x >= grid.w as i32 || y >= grid.h as i32 {
            return None;
        }
        membership.get(&(y as usize * grid.w + x as usize)).copied()
    };
    // The leftmost-topmost cell's left neighbour is outside by construction.
    let start = cells
        .iter()
        .copied()
        .map(|i| (i % grid.w, i / grid.w))
        .min()
        .unwrap();
    let sx = start.0 as i32;
    let sy = start.1 as i32;
    // Clockwise from East.
    const DIRS: [(i32, i32); 8] = [
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
        (1, -1),
    ];
    let mut path = vec![start];
    let mut cur = (sx, sy);
    let mut from = (sx - 1, sy);
    let limit = cells.len() * 8 + 8;
    for _ in 0..limit {
        let fi = DIRS
            .iter()
            .position(|d| (cur.0 + d.0, cur.1 + d.1) == from)
            .unwrap_or(0);
        let mut next = None;
        for k in 1..=8 {
            let d = DIRS[(fi + k) % 8];
            let n = (cur.0 + d.0, cur.1 + d.1);
            if at(n.0, n.1).is_some() {
                // The cell scanned just before the hit is the new backtrack
                // point, and it is outside the component by construction.
                let pd = DIRS[(fi + k - 1) % 8];
                from = (cur.0 + pd.0, cur.1 + pd.1);
                cur = n;
                next = Some(n);
                break;
            }
        }
        let Some(n) = next else { break };
        if n == (sx, sy) {
            break;
        }
        path.push((n.0 as usize, n.1 as usize));
    }
    path
}

/// A head every spacing along the walked wall. Small loops still earn up
/// to three heads (Prusa's minimum), one-cell loops keep their single.
fn sample_outline(path: &[(usize, usize)], grid: Grid, spacing: f32) -> Vec<(usize, usize)> {
    if path.len() < 2 {
        return path.to_vec();
    }
    let step = ((spacing / grid.cell).round() as usize).max(1);
    let mut sampled = Vec::new();
    let mut i = 0;
    while i < path.len() {
        sampled.push(path[i]);
        i += step;
    }
    let perimeter = path.len() as f32 * grid.cell;
    let want = ((perimeter / spacing).ceil() as usize).clamp(1, 3);
    if sampled.len() < want {
        sampled = (0..want).map(|k| path[k * path.len() / want]).collect();
    }
    sampled
}
fn downward_faces(meshes: &[Mesh], cfg: &SupportConfig, count: &mut usize) -> Mesh {
    let mut debug = Mesh::default();
    let limit = -cfg.overhang_angle_deg.clamp(1.0, 89.0).to_radians().sin();
    for mesh in meshes {
        for tri in &mesh.tris {
            let points = mesh.tri_verts(*tri);
            let normal = (points[1] - points[0])
                .cross(points[2] - points[0])
                .normalized();
            if normal.z > limit + 1e-4 || points.iter().all(|p| p.z <= 0.01) {
                continue;
            }
            let base = debug.verts.len() as u32;
            debug.verts.extend_from_slice(&points);
            debug.tris.push([base, base + 1, base + 2]);
            *count += 1;
        }
    }
    debug
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_radius_follows_prusas_curve_and_density_timing() {
        // The curve as published is the reference spacing's curve.
        let mut cfg = SupportConfig {
            touch_tip_distance: REFERENCE_TIP_SPACING,
            ..Default::default()
        };
        assert!((support_radius(0.0, &cfg) - 3.2).abs() < 1e-4);
        assert!((support_radius(3.9, &cfg) - 4.0).abs() < 1e-4);
        assert!((support_radius(15.0, &cfg) - 5.0).abs() < 1e-4);
        assert!((support_radius(40.0, &cfg) - 6.0).abs() < 1e-4);
        assert!((support_radius(100.0, &cfg) - 6.0).abs() < 1e-4);

        // A wider spacing scales the whole curve, height for height.
        cfg.touch_tip_distance = 8.0;
        assert!(
            (support_radius(0.0, &cfg) - 6.4).abs() < 1e-4,
            "the first layer scales with the spacing too"
        );
        assert!((support_radius(3.9, &cfg) - 8.0).abs() < 1e-4);
        assert!((support_radius(40.0, &cfg) - 12.0).abs() < 1e-4);
    }
}
