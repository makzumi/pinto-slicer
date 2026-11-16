//! Auto arrange: putting every model on the plate in a tidy grid so nothing
//! overlaps the next piece and everything stays inside the build area.

use crate::mesh::Vec3;

/// Where one model should go.
pub struct Slot {
    /// Which model, in the order it was given.
    pub index: usize,
    /// How far to move it, including dropping it onto the plate.
    pub delta: Vec3,
}

/// Pack the given boxes (already in world space) into rows on the plate.
/// `gap` is the free space kept between pieces.
pub fn arrange(bounds: &[(Vec3, Vec3)], build: [f32; 3], gap: f32) -> Vec<Slot> {
    let gap = gap.max(0.5);
    let half_w = build[0] * 0.5 - gap;
    let mut order: Vec<usize> = (0..bounds.len()).collect();
    // Wider pieces first keeps the rows even.
    order.sort_by(|a, b| {
        let wa = bounds[*b].1.x - bounds[*b].0.x;
        let wb = bounds[*a].1.x - bounds[*a].0.x;
        wa.partial_cmp(&wb).unwrap_or(std::cmp::Ordering::Equal)
    });

    // 1. break the pieces into rows that fit across the plate
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut cursor_x = -half_w;
    for i in order {
        let w = bounds[i].1.x - bounds[i].0.x;
        let start_new = match rows.last() {
            None => true,
            Some(row) => !row.is_empty() && cursor_x + w > half_w,
        };
        if start_new {
            rows.push(Vec::new());
            cursor_x = -half_w;
        }
        rows.last_mut().expect("a row exists").push(i);
        cursor_x += w + gap;
    }

    // 2. centre each row across the plate, and stack the rows up the plate
    let mut placed: Vec<(usize, f32, f32)> = Vec::new();
    let mut y = 0.0f32;
    for row in &rows {
        let depth = row
            .iter()
            .map(|i| bounds[*i].1.y - bounds[*i].0.y)
            .fold(0.0f32, f32::max);
        let total_w: f32 = row
            .iter()
            .map(|i| bounds[*i].1.x - bounds[*i].0.x)
            .sum::<f32>()
            + gap * (row.len().saturating_sub(1)) as f32;
        let mut x = -total_w * 0.5;
        for i in row {
            let (min, max) = bounds[*i];
            let w = max.x - min.x;
            placed.push((*i, x + w * 0.5, y + depth * 0.5));
            x += w + gap;
        }
        y += depth + gap;
    }

    // 3. shift the whole block so it sits in the middle of the plate
    let mut top = f32::MIN;
    let mut bottom = f32::MAX;
    for (i, _, cy) in &placed {
        let (min, max) = bounds[*i];
        let half_d = (max.y - min.y) * 0.5;
        top = top.max(cy + half_d);
        bottom = bottom.min(cy - half_d);
    }
    let shift_y = if placed.is_empty() {
        0.0
    } else {
        -(top + bottom) * 0.5
    };

    let mut out: Vec<Slot> = Vec::with_capacity(placed.len());
    for (i, cx, cy) in placed {
        let (min, max) = bounds[i];
        let centre = Vec3::new((min.x + max.x) * 0.5, (min.y + max.y) * 0.5, 0.0);
        out.push(Slot {
            index: i,
            delta: Vec3::new(cx - centre.x, cy + shift_y - centre.y, -min.z),
        });
    }
    out.sort_by_key(|s| s.index);
    out
}

/// The middle of a bounding box: the point a piece turns or flips about.
pub fn bounds_centre(bounds: (Vec3, Vec3)) -> Vec3 {
    let (min, max) = bounds;
    (min + max) * 0.5
}

/// How far to move a piece to put it back over the middle of the plate. This
/// is an offset, not a point: add it to a position.
pub fn plate_offset(bounds: (Vec3, Vec3)) -> Vec3 {
    let (min, max) = bounds;
    Vec3::new(-(min.x + max.x) * 0.5, -(min.y + max.y) * 0.5, -min.z)
}

/// Which edge or centre of the selection's combined box is the alignment target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alignment {
    Min,
    Centre,
    Max,
}

fn component(v: Vec3, axis: usize) -> f32 {
    match axis {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

fn axis_delta(axis: usize, value: f32) -> Vec3 {
    match axis {
        0 => Vec3::new(value, 0.0, 0.0),
        1 => Vec3::new(0.0, value, 0.0),
        _ => Vec3::new(0.0, 0.0, value),
    }
}

/// Move every box to the same minimum, centre or maximum on `axis`.
/// The returned deltas correspond to the input boxes in their original order.
pub fn align(bounds: &[(Vec3, Vec3)], axis: usize, mode: Alignment) -> Vec<Vec3> {
    assert!(axis < 3, "an alignment axis is X, Y or Z");
    if bounds.is_empty() {
        return Vec::new();
    }
    let low = bounds
        .iter()
        .map(|b| component(b.0, axis))
        .fold(f32::INFINITY, f32::min);
    let high = bounds
        .iter()
        .map(|b| component(b.1, axis))
        .fold(f32::NEG_INFINITY, f32::max);
    let target = match mode {
        Alignment::Min => low,
        Alignment::Centre => (low + high) * 0.5,
        Alignment::Max => high,
    };
    bounds
        .iter()
        .map(|(min, max)| {
            let current = match mode {
                Alignment::Min => component(*min, axis),
                Alignment::Centre => component(*min + *max, axis) * 0.5,
                Alignment::Max => component(*max, axis),
            };
            axis_delta(axis, target - current)
        })
        .collect()
}

/// Evenly distribute box centres between the two outer boxes on `axis`.
/// The first and last centres stay fixed; two boxes therefore need no movement.
pub fn distribute_centres(bounds: &[(Vec3, Vec3)], axis: usize) -> Vec<Vec3> {
    assert!(axis < 3, "a distribution axis is X, Y or Z");
    if bounds.len() < 3 {
        return vec![Vec3::ZERO; bounds.len()];
    }
    let centre = |b: &(Vec3, Vec3)| component(b.0 + b.1, axis) * 0.5;
    let mut order: Vec<usize> = (0..bounds.len()).collect();
    order.sort_by(|&a, &b| centre(&bounds[a]).total_cmp(&centre(&bounds[b])));
    let first = centre(&bounds[order[0]]);
    let last = centre(&bounds[order[order.len() - 1]]);
    let mut out = vec![Vec3::ZERO; bounds.len()];
    for (rank, idx) in order.into_iter().enumerate() {
        let t = rank as f32 / (bounds.len() - 1) as f32;
        let target = first + (last - first) * t;
        out[idx] = axis_delta(axis, target - centre(&bounds[idx]));
    }
    out
}

/// Put an equal empty gap between neighbouring boxes on `axis`, then move the
/// whole group so its combined centre stays where it was. Boxes are sorted by
/// their centres, and the returned deltas retain the original input order.
pub fn even_gaps(bounds: &[(Vec3, Vec3)], axis: usize, gap: f32) -> Vec<Vec3> {
    assert!(axis < 3, "a spacing axis is X, Y or Z");
    if bounds.is_empty() {
        return Vec::new();
    }
    let centre = |b: &(Vec3, Vec3)| component(b.0 + b.1, axis) * 0.5;
    let group_low = bounds
        .iter()
        .map(|b| component(b.0, axis))
        .fold(f32::INFINITY, f32::min);
    let group_high = bounds
        .iter()
        .map(|b| component(b.1, axis))
        .fold(f32::NEG_INFINITY, f32::max);
    let mut order: Vec<usize> = (0..bounds.len()).collect();
    order.sort_by(|&a, &b| centre(&bounds[a]).total_cmp(&centre(&bounds[b])));
    let gap = gap.max(0.0);
    let mut cursor = component(bounds[order[0]].0, axis);
    let mut out = vec![Vec3::ZERO; bounds.len()];
    for idx in order.iter().copied() {
        let (min, max) = bounds[idx];
        let size = component(max - min, axis);
        let desired = cursor;
        out[idx] = axis_delta(axis, desired - component(min, axis));
        cursor = desired + size + gap;
    }
    let final_centre =
        (component(bounds[order[0]].0, axis) + component(out[order[0]], axis) + cursor - gap) * 0.5;
    let shift = (group_low + group_high) * 0.5 - final_centre;
    for delta in &mut out {
        *delta = *delta + axis_delta(axis, shift);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(x: f32, y: f32, w: f32, d: f32, h: f32) -> (Vec3, Vec3) {
        (Vec3::new(x, y, 0.0), Vec3::new(x + w, y + d, h))
    }

    fn apply(b: (Vec3, Vec3), d: Vec3) -> (Vec3, Vec3) {
        (b.0 + d, b.1 + d)
    }

    #[test]
    fn pieces_end_up_inside_the_plate_and_apart() {
        let build = [218.88, 122.88, 200.0];
        let bounds = vec![
            boxed(-8.0, -8.0, 16.0, 16.0, 10.0),
            boxed(30.0, 30.0, 16.0, 16.0, 10.0),
            boxed(0.0, 0.0, 60.0, 40.0, 25.0),
        ];
        let slots = arrange(&bounds, build, 4.0);
        assert_eq!(slots.len(), 3);
        let placed: Vec<(Vec3, Vec3)> = slots
            .iter()
            .map(|s| apply(bounds[s.index], s.delta))
            .collect();
        for (min, max) in &placed {
            assert!(min.x >= -build[0] * 0.5 - 0.01 && max.x <= build[0] * 0.5 + 0.01);
            assert!(min.y >= -build[1] * 0.5 - 0.01 && max.y <= build[1] * 0.5 + 0.01);
            assert!(min.z.abs() < 0.01, "each piece should sit on the plate");
        }
        for i in 0..placed.len() {
            for j in i + 1..placed.len() {
                let a = placed[i];
                let b = placed[j];
                let apart = a.1.x <= b.0.x + 0.01
                    || b.1.x <= a.0.x + 0.01
                    || a.1.y <= b.0.y + 0.01
                    || b.1.y <= a.0.y + 0.01;
                assert!(apart, "pieces {i} and {j} overlap");
            }
        }
    }

    #[test]
    fn a_single_piece_lands_in_the_middle() {
        let bounds = vec![boxed(40.0, 40.0, 20.0, 20.0, 5.0)];
        let slots = arrange(&bounds, [200.0, 120.0, 100.0], 4.0);
        let (min, max) = apply(bounds[0], slots[0].delta);
        assert!(((min.x + max.x) * 0.5).abs() < 0.01);
        assert!(((min.y + max.y) * 0.5).abs() < 0.01);
    }

    #[test]
    fn the_middle_of_a_box_is_its_centre() {
        let c = bounds_centre(boxed(10.0, 20.0, 4.0, 6.0, 8.0));
        assert!((c - Vec3::new(12.0, 23.0, 4.0)).length() < 1e-4, "{c:?}");
    }

    #[test]
    fn centring_moves_a_piece_to_the_middle() {
        let d = plate_offset(boxed(70.0, -30.0, 10.0, 10.0, 4.0));
        let (min, max) = apply(boxed(70.0, -30.0, 10.0, 10.0, 4.0), d);
        assert!(((min.x + max.x) * 0.5).abs() < 0.01);
        assert!(min.z.abs() < 0.01);
    }

    #[test]
    fn alignment_uses_the_combined_selection_box() {
        let boxes = vec![
            boxed(0.0, 0.0, 4.0, 3.0, 5.0),
            boxed(12.0, 8.0, 8.0, 2.0, 7.0),
            boxed(-6.0, 3.0, 2.0, 6.0, 4.0),
        ];
        for (mode, target) in [
            (Alignment::Min, -6.0),
            (Alignment::Centre, 7.0),
            (Alignment::Max, 20.0),
        ] {
            let deltas = align(&boxes, 0, mode);
            for (i, b) in boxes.iter().enumerate() {
                let moved = apply(*b, deltas[i]);
                let actual = match mode {
                    Alignment::Min => moved.0.x,
                    Alignment::Centre => (moved.0.x + moved.1.x) * 0.5,
                    Alignment::Max => moved.1.x,
                };
                assert!(
                    (actual - target).abs() < 1e-4,
                    "{mode:?}: {actual} != {target}"
                );
                assert_eq!(deltas[i].y, 0.0);
                assert_eq!(deltas[i].z, 0.0);
            }
        }
    }

    #[test]
    fn distribution_keeps_outer_centres_and_spaces_the_middle() {
        let boxes = vec![
            boxed(0.0, 0.0, 2.0, 2.0, 2.0),
            boxed(10.0, 0.0, 6.0, 2.0, 2.0),
            boxed(30.0, 0.0, 4.0, 2.0, 2.0),
        ];
        let deltas = distribute_centres(&boxes, 0);
        let moved: Vec<_> = boxes
            .iter()
            .zip(deltas)
            .map(|(b, d)| apply(*b, d))
            .collect();
        let centres: Vec<f32> = moved.iter().map(|b| (b.0.x + b.1.x) * 0.5).collect();
        assert!((centres[0] - 1.0).abs() < 1e-4);
        assert!((centres[1] - 16.5).abs() < 1e-4);
        assert!((centres[2] - 32.0).abs() < 1e-4);
        assert!((centres[1] - centres[0] - (centres[2] - centres[1])).abs() < 1e-4);
    }

    #[test]
    fn equal_gaps_preserve_the_group_centre() {
        let boxes = vec![
            boxed(0.0, 0.0, 2.0, 2.0, 2.0),
            boxed(10.0, 0.0, 6.0, 2.0, 2.0),
            boxed(30.0, 0.0, 4.0, 2.0, 2.0),
        ];
        let before_centre = (boxes.iter().map(|b| b.0.x).fold(f32::INFINITY, f32::min)
            + boxes
                .iter()
                .map(|b| b.1.x)
                .fold(f32::NEG_INFINITY, f32::max))
            * 0.5;
        let deltas = even_gaps(&boxes, 0, 3.0);
        let mut moved: Vec<_> = boxes
            .iter()
            .zip(deltas)
            .map(|(b, d)| apply(*b, d))
            .collect();
        moved.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
        for pair in moved.windows(2) {
            assert!((pair[0].1.x + 3.0 - pair[1].0.x).abs() < 1e-4);
        }
        let after_centre = (moved[0].0.x + moved[moved.len() - 1].1.x) * 0.5;
        assert!((after_centre - before_centre).abs() < 1e-4);
    }
}
