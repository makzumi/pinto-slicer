//! PrusaSlicer DEFAULT/MSLA column supports.
//!
//! Ported from `SLA/DefaultSupportTree.cpp` and `SupportTreeUtilsLegacy.hpp`:
//! validate pinheads, classify ground-facing heads, create central pillars,
//! bridge side heads to existing pillars, search avoidance routes, and only
//! then anchor a head to the model. Pillars are always vertical.

use std::collections::{HashMap, HashSet};

use super::bvh::Bvh;
use super::config::SupportConfig;
use super::{Branch, Foot, Stats, Tip, Trunk};
use crate::mesh::Vec3;

const DOWN: Vec3 = Vec3::new(0.0, 0.0, -1.0);
const MAX_BRIDGE_LENGTH: f32 = 10.0;
const MAX_BRIDGES_ON_PILLAR: usize = 3;
const SAFETY_DISTANCE: f32 = 0.5;

pub struct Pieces {
    pub tips: Vec<Tip>,
    pub anchors: Vec<Tip>,
    pub branches: Vec<Branch>,
    pub trunks: Vec<Trunk>,
    pub feet: Vec<Foot>,
    pub braces: Vec<super::Brace>,
}

#[derive(Clone, Copy)]
struct Pillar {
    trunk: usize,
    model_id: usize,
    radius: f32,
    bridges: usize,
    links: usize,
}

#[derive(Clone, Copy)]
struct Hit {
    distance: f32,
    pos: Vec3,
    normal: Vec3,
}

impl Hit {
    fn open() -> Self {
        Self {
            distance: f32::INFINITY,
            pos: Vec3::ZERO,
            normal: Vec3::ZERO,
        }
    }

    fn is_open(self) -> bool {
        !self.distance.is_finite()
    }
}

pub fn route(tips: &[Tip], cfg: &SupportConfig, tree: &Bvh, stats: &mut Stats) -> Pieces {
    let mut out = Pieces {
        tips: Vec::new(),
        anchors: Vec::new(),
        branches: Vec::new(),
        trunks: Vec::new(),
        feet: Vec::new(),
        braces: Vec::new(),
    };
    let mut heads = Vec::new();
    for tip in tips {
        if let Some(head) = prepare_head(tip, cfg, tree) {
            heads.push(head);
        } else {
            stats.tips_skipped += 1;
            stats.dropped += 1;
        }
    }
    if heads.is_empty() {
        return out;
    }

    let mut builder = Builder {
        cfg,
        tree,
        stats,
        out: &mut out,
        pillars: Vec::new(),
        routed: HashSet::new(),
        scans: HashMap::new(),
    };

    let mut ground = Vec::new();
    let mut model = Vec::new();
    for (index, head) in heads.iter().enumerate() {
        let hit = builder.beam_hit(head.lower_end(), DOWN, head_radius(head));
        builder.scans.insert(head.id, hit);
        if hit.is_open() {
            ground.push(index);
        } else {
            model.push(index);
        }
    }

    for cluster in ground_clusters(&heads, &ground, cfg) {
        let central = medoid(&heads, &cluster);
        let central_pillar =
            builder.create_ground_pillar(&heads[central], heads[central].lower_end(), false);
        if central_pillar.is_none() {
            model.push(central);
        }
        for index in cluster {
            if index == central {
                continue;
            }
            let head = &heads[index];
            let mut connected = central_pillar
                .map(|pillar| builder.connect_to_nearpillar(head, pillar))
                .unwrap_or(false)
                || builder.search_pillar_and_connect(head);
            if !connected {
                connected = builder
                    .create_ground_pillar(head, head.lower_end(), false)
                    .is_some();
            }
            if !connected {
                model.push(index);
            }
        }
    }

    for index in model {
        let head = &heads[index];
        if builder.search_pillar_and_connect(head) {
            continue;
        }
        if builder.ground_escape(head) {
            continue;
        }
        if builder.connect_to_model_body(head) {
            continue;
        }
        builder.stats.dropped += 1;
        if std::env::var_os("SUP_DEBUG").is_some() {
            eprintln!("  unrouted Prusa head {} at {:?}", head.id, head.pos);
        }
    }

    out.tips = heads
        .into_iter()
        .filter(|head| builder.routed.contains(&head.id))
        .collect();
    finalize_diameters(&mut out, cfg);
    out
}

/// Size the finished tree from the settings, not the tip that happened to
/// create it. HALOT Box prints one shaft thickness everywhere, whether the
/// column stands on the raft or grows down from the model, so every trunk
/// uses `trunk_diameter` and every branch uses `branch_diameter`.
fn finalize_diameters(out: &mut Pieces, cfg: &SupportConfig) {
    let trunk_d = cfg.trunk_diameter.max(0.2);
    for trunk in &mut out.trunks {
        trunk.diameter = trunk_d;
    }
    let branch_d = cfg.branch_diameter.max(0.2);
    for branch in &mut out.branches {
        branch.diameter = branch_d;
    }
}

fn head_radius(head: &Tip) -> f32 {
    (head.lower_d * 0.5).max(0.05)
}

fn directions(head: &Tip, cfg: &SupportConfig) -> Vec<Vec3> {
    let angle = cfg.branch_angle_deg.clamp(15.0, 85.0).to_radians();
    let original = (head.axis * -1.0).normalized();
    let azimuth = original.y.atan2(original.x);
    let mut result = vec![original, DOWN];
    for polar in [
        angle,
        angle.max(60.0f32.to_radians()),
        angle.max(75.0f32.to_radians()),
    ] {
        for step in 0..12 {
            let a = azimuth + step as f32 * std::f32::consts::TAU / 12.0;
            result.push(Vec3::new(
                polar.sin() * a.cos(),
                polar.sin() * a.sin(),
                -polar.cos(),
            ));
        }
    }
    result
}

fn prepare_head(input: &Tip, cfg: &SupportConfig, tree: &Bvh) -> Option<Tip> {
    for fallback in [false, true] {
        let mut local = cfg.clone();
        if fallback {
            local.tip.lower_diameter *= 0.5;
            local.tip.length = local.tip.contact_depth;
        }
        for direction in directions(input, &local) {
            let mut head = super::tips::make_tip(input.pos, direction * -1.0, &local, input.island);
            head.id = input.id;
            head.model_id = input.model_id;
            // A pinhead presses `contact_depth` into the model; on a thin wall
            // it would come out the far side. Cap the depth at half the local
            // thickness so the tip always stops inside the material.
            if let Some(thickness) = tree.wall_thickness(head.pos, head.axis) {
                head.contact_depth = head.contact_depth.min((thickness * 0.5).max(0.05));
            }
            let width = (head.lower_end() - head.pos).length();
            if head.pos.z + width * direction.z < 0.0 {
                continue;
            }
            if pinhead_clearance(&head, tree) > width {
                return Some(head);
            }
        }
    }
    None
}

/// Sixteen-ray pinhead probe used by Prusa. The two rings follow the tangent
/// robe from the front contact to the back sphere; the shortest hit wins.
fn pinhead_clearance(head: &Tip, tree: &Bvh) -> f32 {
    let away = (head.axis * -1.0).normalized();
    let (u, v) = ring_basis(away);
    let front_r = head.upper_d * 0.5;
    let back_r = head_radius(head);
    let safety = SAFETY_DISTANCE.min(back_r);
    let junction = head.lower_end();
    let back_center = junction - away * (front_r - head.contact_depth);
    let mut closest = f32::INFINITY;
    for i in 0usize..16 {
        let angle = (i.saturating_sub(1)) as f32 / 15.0 * std::f32::consts::TAU;
        let radial = if i == 0 {
            Vec3::ZERO
        } else {
            u * angle.cos() + v * angle.sin()
        };
        let a = head.pos + radial * (front_r + safety);
        let b = back_center + radial * (back_r + safety);
        let dir = (b - a).normalized();
        let from = a + dir * safety;
        if let Some((mut distance, normal)) = tree.ray_hit(from, dir, 1.0e4) {
            // An exit hit has its outward normal along the ray, which means
            // the source was inside. Avoid a separate three-ray inside vote.
            if normal.dot(dir) > 0.0 {
                if distance > 2.0 * front_r {
                    return 0.0;
                }
                let recast = from + dir * (distance + 2.0 * safety);
                distance = tree
                    .ray_hit(recast, dir, 1.0e4)
                    .map(|(hit, _)| hit + distance + 2.0 * safety)
                    .unwrap_or(f32::INFINITY);
            }
            closest = closest.min(distance);
        }
    }
    closest
}

fn ground_clusters(heads: &[Tip], ground: &[usize], cfg: &SupportConfig) -> Vec<Vec<usize>> {
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for &index in ground {
        let head = &heads[index];
        let point = head.lower_end();
        let mut destination = None;
        for (cluster_id, cluster) in clusters.iter().enumerate() {
            if cluster.len() >= MAX_BRIDGES_ON_PILLAR {
                continue;
            }
            let first = heads[cluster[0]].lower_end();
            let dxy = ((point.x - first.x).powi(2) + (point.y - first.y).powi(2)).sqrt();
            // The reach grows with the tip spacing: a sparse network gets the
            // same few columns within reach of each other, instead of one
            // column (and one foot) under every single tip. The clear-path
            // search below still decides whether the bridge can exist.
            let reach = cfg.cluster_radius().max(cfg.foot_diameter);
            if dxy < reach && (point - first).length() < MAX_BRIDGE_LENGTH {
                destination = Some(cluster_id);
                break;
            }
        }
        if let Some(cluster_id) = destination {
            clusters[cluster_id].push(index);
        } else {
            clusters.push(vec![index]);
        }
    }
    clusters
}

fn medoid(heads: &[Tip], cluster: &[usize]) -> usize {
    if cluster.len() <= 2 {
        return cluster[0];
    }
    cluster
        .iter()
        .copied()
        .min_by(|a, b| {
            let cost = |index: usize| {
                cluster
                    .iter()
                    .map(|other| {
                        let p = heads[index].pos;
                        let q = heads[*other].pos;
                        (p.x - q.x).powi(2) + (p.y - q.y).powi(2)
                    })
                    .sum::<f32>()
            };
            cost(*a).total_cmp(&cost(*b))
        })
        .unwrap_or(cluster[0])
}

struct Builder<'a> {
    cfg: &'a SupportConfig,
    tree: &'a Bvh,
    stats: &'a mut Stats,
    out: &'a mut Pieces,
    pillars: Vec<Pillar>,
    routed: HashSet<usize>,
    scans: HashMap<usize, Hit>,
}

impl<'a> Builder<'a> {
    fn create_ground_pillar(&mut self, head: &Tip, top: Vec3, branch: bool) -> Option<usize> {
        let radius = head_radius(head);
        let bottom = Vec3::new(top.x, top.y, 0.0);
        if top.z <= 0.1 || !self.beam_clear(top, bottom, radius) {
            return None;
        }
        if branch && !self.beam_clear(head.lower_end(), top, radius) {
            return None;
        }
        let trunk = self.out.trunks.len();
        self.out.trunks.push(Trunk {
            id: 0,
            bottom,
            top,
            diameter: radius * 2.0,
            tip_ids: vec![head.id],
            model_id: head.model_id,
        });
        self.out.feet.push(Foot {
            id: 0,
            pos: bottom,
            diameter: self.cfg.foot_diameter,
            trunk_id: trunk,
            model_id: head.model_id,
        });
        if branch {
            self.out.branches.push(Branch {
                id: 0,
                from: head.lower_end(),
                via: None,
                to: top,
                diameter: radius * 2.0,
                tip_id: head.id,
            });
            self.stats.shifted += 1;
        } else {
            self.stats.direct += 1;
        }
        self.pillars.push(Pillar {
            trunk,
            model_id: head.model_id,
            radius,
            bridges: 0,
            links: 0,
        });
        self.routed.insert(head.id);
        Some(self.pillars.len() - 1)
    }

    fn connect_to_nearpillar(&mut self, head: &Tip, pillar_id: usize) -> bool {
        let pillar = self.pillars[pillar_id];
        if pillar.model_id != head.model_id
            || pillar.bridges >= MAX_BRIDGES_ON_PILLAR
            || pillar.radius + 1e-4 < head_radius(head)
        {
            return false;
        }
        let trunk = &self.out.trunks[pillar.trunk];
        let source = head.lower_end();
        let top = trunk.top;
        let bottom = trunk.bottom;
        let radius = head_radius(head);
        let normal_radius = (self.cfg.trunk_diameter * 0.5).max(0.05);
        let max_length = (MAX_BRIDGE_LENGTH * radius / normal_radius).max(2.0);
        let dxy = ((source.x - top.x).powi(2) + (source.y - top.y).powi(2)).sqrt();
        let distance = (source - top).length();
        let slope = (top.z - source.z).atan2(dxy);
        let bridge_slope = self.cfg.branch_angle_deg.to_radians();
        let mut target = top;
        let mut via = None;

        if distance > max_length || slope > -bridge_slope {
            let mut z = source.z - dxy * bridge_slope.tan();
            if z > top.z {
                let zdiff = z - top.z;
                let lowered = source - Vec3::new(0.0, 0.0, zdiff);
                if !self.beam_clear(source, lowered, radius) {
                    return false;
                }
                via = Some(lowered);
                z -= zdiff;
            }
            // A flat overhang puts the ideal 45-degree landing below the
            // plate. Clamp it to the lowest usable point on the column
            // instead of dropping the whole bridge: two heads over a flat
            // ceiling should still share one pillar.
            let floor = (bottom.z + 0.3)
                .max(4.0 * radius + 0.05)
                .min(top.z);
            if z < floor {
                z = floor;
            }
            let branch_start = via.unwrap_or(source);
            if z > top.z
                || (branch_start - Vec3::new(top.x, top.y, z)).length() >= max_length
            {
                return false;
            }
            target.z = z;
        }
        if target.z < 4.0 * radius {
            return false;
        }
        let start = via.unwrap_or(source);
        if !self.beam_clear(start, target, radius) {
            return false;
        }

        self.out.branches.push(Branch {
            id: 0,
            from: source,
            via,
            to: target,
            diameter: radius * 2.0,
            tip_id: head.id,
        });
        let trunk = &mut self.out.trunks[pillar.trunk];
        if !trunk.tip_ids.contains(&head.id) {
            trunk.tip_ids.push(head.id);
        }
        self.pillars[pillar_id].bridges += 1;
        self.routed.insert(head.id);
        true
    }

    fn search_pillar_and_connect(&mut self, head: &Tip) -> bool {
        let source = head.lower_end();
        let mut candidates: Vec<usize> = (0..self.pillars.len()).collect();
        candidates.sort_by(|a, b| {
            let pa = self.out.trunks[self.pillars[*a].trunk].bottom;
            let pb = self.out.trunks[self.pillars[*b].trunk].bottom;
            let da = (pa.x - source.x).powi(2) + (pa.y - source.y).powi(2);
            let db = (pb.x - source.x).powi(2) + (pb.y - source.y).powi(2);
            da.total_cmp(&db)
        });
        for pillar in candidates {
            if self.connect_to_nearpillar(head, pillar) {
                return true;
            }
        }
        false
    }

    fn ground_escape(&mut self, head: &Tip) -> bool {
        let radius = head_radius(head);
        let normal_radius = (self.cfg.trunk_diameter * 0.5).max(0.05);
        let max_length = (MAX_BRIDGE_LENGTH * radius / normal_radius).max(2.0);
        let source = head.lower_end();
        for direction in directions(head, self.cfg) {
            let mut travel = 0.0;
            while travel <= max_length {
                let top = source + direction * travel;
                if top.z < 0.1 || !self.beam_clear(source, top, radius) {
                    break;
                }
                if self.beam_clear(top, Vec3::new(top.x, top.y, 0.0), radius) {
                    return self.create_ground_pillar(head, top, true).is_some();
                }
                travel += radius.max(0.2);
            }
        }
        false
    }

    fn connect_to_model_body(&mut self, head: &Tip) -> bool {
        let Some(hit) = self.scans.get(&head.id).copied() else {
            return false;
        };
        if hit.is_open() {
            return false;
        }
        let radius = head_radius(head);
        let source = head.lower_end();
        let fullwidth = (source - head.pos).length() + radius;
        let height = (hit.distance - radius).min(fullwidth);
        if height <= 0.0 || hit.distance <= radius {
            return false;
        }
        let endpoint = Vec3::new(source.x, source.y, source.z - hit.distance + height);
        let center_hit = self.tree.ray_hit(source, DOWN, 1.0e4);
        let hit_pos = if let Some((distance, _)) = center_hit {
            if (distance - hit.distance).abs() < 2.0 * radius {
                source + DOWN * distance
            } else {
                hit.pos
            }
        } else {
            hit.pos
        };
        let tail = endpoint - hit_pos;
        let distance = tail.length();
        if distance < 1e-4 {
            return false;
        }
        let direction = tail * (1.0 / distance);
        let trunk = self.out.trunks.len();
        self.out.trunks.push(Trunk {
            id: 0,
            bottom: endpoint,
            top: source,
            diameter: radius * 2.0,
            tip_ids: vec![head.id],
            model_id: head.model_id,
        });
        let penetration = head.contact_depth;
        let anchor = Tip {
            id: 0,
            pos: hit_pos,
            axis: direction * -1.0,
            length: distance + penetration,
            upper_d: head.upper_d,
            lower_d: head.lower_d,
            shape: head.shape,
            contact_d: head.contact_d,
            contact_shape: head.contact_shape,
            contact_depth: penetration,
            island: false,
            model_id: head.model_id,
        };
        self.out.anchors.push(anchor);
        self.pillars.push(Pillar {
            trunk,
            model_id: head.model_id,
            radius,
            bridges: 0,
            links: 0,
        });
        self.routed.insert(head.id);
        self.stats.on_model += 1;
        true
    }

    fn beam_clear(&self, a: Vec3, b: Vec3, radius: f32) -> bool {
        let delta = b - a;
        let length = delta.length();
        if length < 1e-5 {
            return true;
        }
        self.beam_hit(a, delta * (1.0 / length), radius).distance >= length - 1e-3
    }

    /// Eight rays (centre plus seven perimeter samples) against the model,
    /// matching Prusa's beam probe. No support/support collision is rejected.
    fn beam_hit(&self, source: Vec3, direction: Vec3, radius: f32) -> Hit {
        let direction = direction.normalized();
        let (u, v) = ring_basis(direction);
        let safety = SAFETY_DISTANCE.min(radius);
        let mut closest = Hit::open();
        for i in 0usize..8 {
            let angle = (i.saturating_sub(1)) as f32 / 7.0 * std::f32::consts::TAU;
            let radial = if i == 0 {
                Vec3::ZERO
            } else {
                u * angle.cos() + v * angle.sin()
            };
            let from = source + radial * (radius + safety);
            if let Some((distance, normal)) = self.tree.ray_hit(from, direction, 1.0e4) {
                if distance < closest.distance {
                    closest = Hit {
                        distance,
                        pos: from + direction * distance,
                        normal,
                    };
                }
            }
        }
        closest
    }
}

fn ring_basis(axis: Vec3) -> (Vec3, Vec3) {
    let helper = if axis.z.abs() < 0.9 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::new(1.0, 0.0, 0.0)
    };
    let u = axis.cross(helper).normalized();
    let v = axis.cross(u).normalized();
    (u, v)
}
