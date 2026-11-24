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
const MAX_POINTS_PER_REGION: usize = 2048;
const OVERHANG_SAMPLE_STEP: f32 = 2.0;
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
        let components = components(current, grid, cancel);
        let mut previous_owner = vec![usize::MAX; current.len()];
        for (part_id, part) in previous_parts.iter().enumerate() {
            for &cell in &part.cells {
                previous_owner[cell] = part_id;
            }
        }

        let on_plate = model_on_plate && !saw_material;
        let mut active_parts = Vec::with_capacity(components.len());
        for component in components {
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
                let overhang = unsupported_cells(&component, &previous, grid);
                if !overhang.is_empty() {
                    out.regions += 1;
                    let samples = select_cells(
                        &Component {
                            center: component.center,
                            min: component.min,
                            max: component.max,
                            cells: overhang,
                        },
                        grid,
                        OVERHANG_SAMPLE_STEP * density_scale(cfg),
                        true,
                    );
                    for (x, y) in samples {
                        let (px, py) = grid.point(x, y);
                        if covered(&supports, &out.contacts, px, py, z, cfg) {
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

fn covered(
    supports: &[usize],
    contacts: &[Contact],
    x: f32,
    y: f32,
    z: f32,
    cfg: &SupportConfig,
) -> bool {
    supports.iter().any(|id| {
        let contact = &contacts[*id];
        let radius = support_radius(z - contact.pos.z, cfg);
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
    // Prusa's SampleConfigFactory derives island spacing from the contact
    // head area. Keep those relationships, scaled by the UI's tip distance.
    let head_area = std::f32::consts::PI * (cfg.tip.upper_diameter * 0.5).powi(2);
    let one_support_length = head_area * 2.9 + 1.3;
    let inner = (one_support_length * 3.9 * density_scale(cfg)).clamp(2.0, 24.0);
    let outline = one_support_length * 3.9 * 0.75 * density_scale(cfg);

    let interior = select_cells(component, grid, inner, false);
    let rim = select_cells(component, grid, outline, true);
    for (x, y) in interior.into_iter().chain(rim) {
        let (px, py) = grid.point(x, y);
        // The two selections can meet at the rim. Avoid overlapping heads but
        // do not use the larger propagated support radius inside a new island.
        let duplicate = supports.iter().any(|id| {
            let contact = &out.contacts[*id];
            (contact.pos.x - px).powi(2) + (contact.pos.y - py).powi(2)
                < cfg.tip.upper_diameter.max(0.2).powi(2)
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
    let overhang_limit = -cfg.overhang_angle_deg.clamp(1.0, 89.0).to_radians().sin();
    if !island && normal.z > overhang_limit + 1e-4 {
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

/// Deterministic farthest-point sampling. No fixed 64-point cap: a large
/// island is sampled until its selected points cover the requested radius or
/// the explicit safety budget is exhausted.
fn select_cells(
    component: &Component,
    grid: Grid,
    spacing: f32,
    outline_only: bool,
) -> Vec<(usize, usize)> {
    let cells: Vec<usize> = if outline_only {
        let membership: std::collections::HashSet<usize> =
            component.cells.iter().copied().collect();
        component
            .cells
            .iter()
            .copied()
            .filter(|idx| {
                let x = idx % grid.w;
                let y = idx / grid.w;
                (-1..=1).any(|dy| {
                    (-1..=1).any(|dx| {
                        if dx == 0 && dy == 0 {
                            return false;
                        }
                        let nx = x as i32 + dx;
                        let ny = y as i32 + dy;
                        nx < 0
                            || ny < 0
                            || nx >= grid.w as i32
                            || ny >= grid.h as i32
                            || !membership.contains(&(ny as usize * grid.w + nx as usize))
                    })
                })
            })
            .collect()
    } else {
        component.cells.clone()
    };
    if cells.is_empty() {
        return Vec::new();
    }
    let area = cells.len() as f32 * grid.cell * grid.cell;
    let spacing = spacing.max(grid.cell);
    if area <= (spacing * 0.75).powi(2) {
        return vec![nearest_cell(&cells, grid, component.center)];
    }

    let mut selected = vec![nearest_cell(&cells, grid, component.center)];
    let max_points =
        ((area / (spacing * spacing * 0.75)).ceil() as usize).clamp(2, MAX_POINTS_PER_REGION);
    let reach = spacing.max(grid.cell * 1.5);
    while selected.len() < max_points {
        let mut best = None;
        let mut best_distance = 0.0;
        for &idx in &cells {
            let x = idx % grid.w;
            let y = idx / grid.w;
            if selected.contains(&(x, y)) {
                continue;
            }
            let distance = selected
                .iter()
                .map(|&(sx, sy)| {
                    let dx = (sx as f32 - x as f32) * grid.cell;
                    let dy = (sy as f32 - y as f32) * grid.cell;
                    dx * dx + dy * dy
                })
                .fold(f32::MAX, f32::min);
            if distance > best_distance {
                best_distance = distance;
                best = Some((x, y));
            }
        }
        let Some(next) = best else { break };
        if best_distance.sqrt() <= reach {
            break;
        }
        selected.push(next);
    }
    selected
}

fn nearest_cell(cells: &[usize], grid: Grid, center: (f32, f32)) -> (usize, usize) {
    cells
        .iter()
        .map(|idx| (*idx % grid.w, *idx / grid.w))
        .min_by(|(ax, ay), (bx, by)| {
            let da = (*ax as f32 - center.0).powi(2) + (*ay as f32 - center.1).powi(2);
            let db = (*bx as f32 - center.0).powi(2) + (*by as f32 - center.1).powi(2);
            da.total_cmp(&db)
        })
        .unwrap_or((0, 0))
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
        let mut cfg = SupportConfig::default();
        assert_eq!(support_radius(0.0, &cfg), 3.2);
        assert!((support_radius(3.9, &cfg) - 4.0).abs() < 1e-4);
        assert!((support_radius(15.0, &cfg) - 5.0).abs() < 1e-4);
        assert!((support_radius(40.0, &cfg) - 6.0).abs() < 1e-4);
        assert!((support_radius(100.0, &cfg) - 6.0).abs() < 1e-4);

        cfg.touch_tip_distance = 8.0;
        assert!(
            (support_radius(0.0, &cfg) - 6.4).abs() < 1e-4,
            "the first layer scales with the spacing too"
        );
        assert!((support_radius(40.0, &cfg) - 12.0).abs() < 1e-4);
    }
}
