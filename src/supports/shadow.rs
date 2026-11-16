//! What stands under the plate, seen from below and from above.
//!
//! Support geometry is vertical: a trunk is a line in Z, and a foot rests on a
//! surface. So almost every question routing asks about the model is a
//! two-and-a-half dimensional one, and the layer rasters that the contact
//! analysis already builds answer it exactly:
//!
//! * how low does material hang over this point (may a trunk stand here)?
//! * what surface lies under it, and how far does that surface slope?
//! * is the column free above that surface up to where the trunk must reach?
//!
//! A column of material is a run of consecutive occupied layers, so the whole
//! thing is stored as those runs, one short list per raster cell — no triangle
//! tree descended, no time proportional to the model's triangle count.
//!
//! This module is only the answer. Routing owns the policy of which seat to
//! take; nothing here knows about trunks, feet or tips.

/// Where nothing hangs over a point, and where no surface lies below it.
const OPEN: f32 = f32::INFINITY;

/// One surface under a pad, in world z: its lowest point, where a trunk can
/// start, and its highest, above which the column has to be free.
///
/// The two differ on a slope, which is the normal case on a part that is not
/// standing flat: every face of a part tilted by 45° is itself at 45°. A trunk
/// standing there is bonded in by its lower edge and leaves the material at
/// the higher side, so both heights matter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ground {
    pub low: f32,
    pub high: f32,
}

impl Ground {
    /// A level surface, the common case.
    fn level(at: f32) -> Ground {
        Ground { low: at, high: at }
    }
}

/// The runs of material along every column of the raster grid, and the
/// questions that can be answered from them.
#[derive(Clone, Debug)]
pub struct Shadow {
    min_x: f32,
    min_y: f32,
    cell: f32,
    w: usize,
    h: usize,
    /// Height of each rasterised layer, in ascending order.
    heights: Vec<f32>,
    /// Raster layer spacing: material in a layer spans half a layer either
    /// side of its height.
    layer: f32,
    /// Per cell, the inclusive range of layer indices that are occupied, one
    /// entry per run of consecutive occupied layers.
    runs: Vec<Vec<[u32; 2]>>,
}

impl Default for Shadow {
    fn default() -> Self {
        Shadow {
            min_x: 0.0,
            min_y: 0.0,
            cell: 1.0,
            w: 0,
            h: 0,
            heights: Vec::new(),
            layer: 0.25,
            runs: Vec::new(),
        }
    }
}

impl Shadow {
    pub fn new(min: [f32; 2], cell: f32, w: usize, h: usize) -> Shadow {
        Shadow {
            min_x: min[0],
            min_y: min[1],
            cell: cell.max(1e-3),
            w,
            h,
            runs: vec![Vec::new(); w * h],
            ..Shadow::default()
        }
    }

    /// The raster layer spacing, so callers can express a margin in layers.
    pub fn layer_height(&self) -> f32 {
        self.layer
    }

    /// Fold one rasterised layer in. Layers must arrive in ascending order of
    /// `z`, which is how the slicer produces them.
    pub fn add_layer(&mut self, z: f32, mask: &[bool]) {
        if mask.len() != self.runs.len() {
            return;
        }
        if let Some(dz) = self.heights.last().map(|last| z - *last) {
            if dz > 1e-4 {
                self.layer = dz;
            }
        }
        let index = self.heights.len() as u32;
        self.heights.push(z);
        for (cell, occupied) in mask.iter().enumerate() {
            if !*occupied {
                continue;
            }
            let runs = &mut self.runs[cell];
            match runs.last_mut() {
                Some(run) if run[1] + 1 == index => run[1] = index,
                _ => runs.push([index, index]),
            }
        }
    }

    /// The height at which material starts at or above `from_z` within
    /// `radius` of the point: the ceiling for a trunk standing there.
    ///
    /// `OPEN` when nothing hangs over it, and also for a point outside the
    /// raster — this only promises where the part hangs over the plate, so
    /// callers that care about the plate's edges keep their trunks inside it
    /// for reasons of their own.
    pub fn ceiling(&self, x: f32, y: f32, radius: f32, from_z: f32) -> f32 {
        let mut lowest = OPEN;
        self.each_cell(x, y, radius, |runs| {
            for run in runs {
                let bottom = self.run_bottom(*run);
                if bottom >= from_z && bottom < lowest {
                    lowest = bottom;
                }
            }
        });
        lowest
    }

    /// The surface under the point that is below `below_z`: the ground a trunk
    /// or a foot can stand on.
    ///
    /// Every cell of the pad must have material under it — a pad that reaches
    /// out over an edge would be printed half in the air — and the ground may
    /// slope, but no faster than the pad itself is wide: a rise of more than
    /// its own diameter across the pad is a step or a cliff, not ground.
    ///
    /// A run's top is a surface only when nothing is printed directly above
    /// it, which the run itself already says: a downward-facing face is the
    /// *bottom* of a run, never its top.
    pub fn surface(&self, x: f32, y: f32, radius: f32, below_z: f32) -> Option<Ground> {
        let reach = radius.max(0.0);
        // A landing clipped by the raster edge is not fully supported. The
        // ceiling query may treat outside as open space, but a surface query
        // must prove material under the whole pad.
        if self.runs.is_empty()
            || x - reach < self.min_x
            || y - reach < self.min_y
            || x + reach > self.min_x + self.w as f32 * self.cell
            || y + reach > self.min_y + self.h as f32 * self.cell
        {
            return None;
        }
        let mut low = OPEN;
        let mut high = f32::MIN;
        let mut cells = 0;
        let mut covered = true;
        self.each_cell(x, y, radius, |runs| {
            cells += 1;
            let top = runs
                .iter()
                .filter(|run| self.run_top(**run) < below_z)
                .map(|run| self.run_top(*run))
                .fold(f32::MIN, f32::max);
            if top == f32::MIN {
                covered = false;
                return;
            }
            low = low.min(top);
            high = high.max(top);
        });
        if cells == 0 || !covered || high - low > 2.0 * radius.max(0.0) + self.layer {
            return None;
        }
        Some(Ground { low, high })
    }

    /// True when a column of `radius` at `(x, y)` can stand up to `top`.
    pub fn holds(&self, x: f32, y: f32, radius: f32, top: f32) -> bool {
        top <= self.ceiling(x, y, radius, 0.0)
    }

    /// The print area the shadow covers, for tests and callers that want to
    /// know whether a point is inside it at all.
    #[cfg(test)]
    pub fn area(&self) -> ([f32; 2], [f32; 2]) {
        (
            [self.min_x, self.min_y],
            [
                self.min_x + self.w as f32 * self.cell,
                self.min_y + self.h as f32 * self.cell,
            ],
        )
    }

    /// Material of a run starts half a layer below the first occupied layer's
    /// height, and ends half a layer above the last one's.
    fn run_bottom(&self, run: [u32; 2]) -> f32 {
        self.heights[run[0] as usize] - self.layer * 0.5
    }

    fn run_top(&self, run: [u32; 2]) -> f32 {
        self.heights[run[1] as usize] + self.layer * 0.5
    }

    /// Walk the cells whose square the pad at `(x, y)` touches. Cells are
    /// material over their whole square, so the pad is treated as the square
    /// that encloses it: generous by the corners, which is the safe direction
    /// for a trunk (it can only stand further away, never inside the part).
    fn each_cell(&self, x: f32, y: f32, radius: f32, mut visit: impl FnMut(&[[u32; 2]])) {
        if self.runs.is_empty() {
            return;
        }
        let reach = radius.max(0.0);
        let (w, h) = (self.w as i64, self.h as i64);
        let x0 = (((x - reach - self.min_x) / self.cell).floor() as i64).max(0);
        let x1 = (((x + reach - self.min_x) / self.cell).ceil() as i64 - 1).min(w - 1);
        let y0 = (((y - reach - self.min_y) / self.cell).floor() as i64).max(0);
        let y1 = (((y + reach - self.min_y) / self.cell).ceil() as i64 - 1).min(h - 1);
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                visit(&self.runs[(cy * w + cx) as usize]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shadow over a 10 × 10 mm patch at 1 mm cells, with `spans` of
    /// material given as inclusive layer ranges over the cells listed.
    /// One block of material: its first and last raster layer, and the cells
    /// it covers.
    type Span<'a> = (u32, u32, &'a [(usize, usize)]);

    fn shadow_with(spans: &[Span<'_>]) -> Shadow {
        let mut shadow = Shadow::new([0.0, 0.0], 1.0, 10, 10);
        let layers = spans.iter().map(|(_, last, _)| *last).max().unwrap_or(0) as usize + 1;
        for layer in 0..layers {
            let mut mask = vec![false; 100];
            for (first, last, cells) in spans {
                if (layer as u32) < *first || (layer as u32) > *last {
                    continue;
                }
                for (x, y) in *cells {
                    mask[y * 10 + x] = true;
                }
            }
            shadow.add_layer(layer as f32 * 0.25 + 0.125, &mask);
        }
        shadow
    }

    #[test]
    fn an_empty_column_is_clear_to_the_top() {
        let shadow = shadow_with(&[(4, 6, &[(2, 2)])]);
        assert_eq!(shadow.ceiling(7.5, 7.5, 0.4, 0.0), OPEN);
        assert!(shadow.holds(7.5, 7.5, 0.4, 50.0));
        assert_eq!(shadow.surface(7.5, 7.5, 0.4, 50.0), None);
    }

    #[test]
    fn a_column_under_material_stops_at_the_underside() {
        let shadow = shadow_with(&[(4, 6, &[(2, 2)])]);
        // the material spans layers 4..6, whose height starts at 1.0
        assert!((shadow.ceiling(2.5, 2.5, 0.2, 0.0) - 1.0).abs() < 1e-4);
        assert!(
            shadow.holds(2.5, 2.5, 0.2, 1.0),
            "up to the material is fine"
        );
        assert!(
            !shadow.holds(2.5, 2.5, 0.2, 1.01),
            "the trunk would run into it"
        );
    }

    #[test]
    fn a_thick_column_counts_the_material_beside_it() {
        let shadow = shadow_with(&[(4, 6, &[(2, 2)])]);
        // 1.4 mm to the side of the cell (which ends at x = 3)
        assert!((shadow.ceiling(3.9, 2.5, 1.4, 0.0) - 1.0).abs() < 1e-4);
        assert_eq!(shadow.ceiling(3.9, 2.5, 0.2, 0.0), OPEN);
    }

    #[test]
    fn a_ceiling_is_asked_from_a_height() {
        let shadow = shadow_with(&[(0, 2, &[(2, 2)]), (10, 12, &[(2, 2)])]);
        assert!((shadow.ceiling(2.5, 2.5, 0.2, 0.0) - 0.0).abs() < 1e-4);
        // above the first block, the ceiling is the second one
        assert!((shadow.ceiling(2.5, 2.5, 0.2, 1.0) - 2.5).abs() < 1e-4);
        assert_eq!(shadow.ceiling(2.5, 2.5, 0.2, 5.0), OPEN);
    }

    #[test]
    fn a_surface_is_the_top_of_the_material_below() {
        let shadow = shadow_with(&[(0, 2, &[(2, 2)]), (10, 12, &[(2, 2)])]);
        // the first block spans 0.0 to 0.75 (layers 0..2, half a layer each side)
        assert_eq!(
            shadow.surface(2.5, 2.5, 0.2, 2.0),
            Some(Ground::level(0.75))
        );
        // asked from above both blocks, the highest surface wins
        assert_eq!(
            shadow.surface(2.5, 2.5, 0.2, 50.0),
            Some(Ground::level(3.25))
        );
        // nothing lies under the plate
        assert_eq!(shadow.surface(2.5, 2.5, 0.2, 0.0), None);
    }

    #[test]
    fn the_whole_pad_must_have_something_under_it() {
        // one cell of material, and a pad wide enough to reach past its edge
        let shadow = shadow_with(&[(0, 2, &[(2, 2)])]);
        assert!(shadow.surface(2.5, 2.5, 0.4, 50.0).is_some());
        assert_eq!(
            shadow.surface(2.5, 2.5, 1.2, 50.0),
            None,
            "a pad overhanging the edge of the material touches nothing at one side"
        );
    }

    #[test]
    fn a_sloping_surface_is_a_place_to_stand() {
        // a slope rising one layer per millimetre, three cells wide and deep
        // enough that the whole pad has ground under it
        let shadow = shadow_with(&[
            (0, 2, &[(2, 2), (2, 3), (2, 4)]),
            (0, 3, &[(3, 2), (3, 3), (3, 4)]),
            (0, 4, &[(4, 2), (4, 3), (4, 4)]),
        ]);
        let ground = shadow
            .surface(3.5, 3.5, 0.7, 50.0)
            .expect("a slope is ground");
        assert!((ground.low - 0.75).abs() < 1e-4, "low edge {:?}", ground);
        assert!((ground.high - 1.25).abs() < 1e-4, "high edge {:?}", ground);
    }

    #[test]
    fn a_step_is_not_a_place_to_stand() {
        // a column ten layers taller than the cell beside it: a wall, not ground
        let shadow = shadow_with(&[
            (0, 2, &[(3, 2), (3, 3), (3, 4)]),
            (0, 12, &[(2, 2), (2, 3), (2, 4)]),
        ]);
        assert_eq!(
            shadow.surface(3.0, 3.5, 0.7, 50.0),
            None,
            "a pad straddling a wall is not standing on anything"
        );
    }

    #[test]
    fn a_landing_cannot_be_clipped_by_the_raster_edge() {
        let shadow = shadow_with(&[(0, 2, &[(0, 0)])]);
        assert_eq!(
            shadow.surface(0.5, 0.5, 1.0, 50.0),
            None,
            "the pad extends outside the raster and cannot be fully supported"
        );
    }

    #[test]
    fn an_empty_shadow_is_free_everywhere() {
        let shadow = Shadow::default();
        assert_eq!(shadow.ceiling(0.0, 0.0, 1.0, 0.0), OPEN);
        assert_eq!(shadow.surface(0.0, 0.0, 1.0, 10.0), None);
        assert!(shadow.area().0[0] >= -1e6);
    }
}
