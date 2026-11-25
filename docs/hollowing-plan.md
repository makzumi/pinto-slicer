# Hollowing and drain holes

Status: **planned, not started** (written 2026-11-24, revised after a review that
corrected the margin arithmetic, the memory estimate, the hole geometry and the
"open meshes just work" assumption). Nothing in this document is built yet.

Hollowing replaces a solid model with a shell of a chosen wall thickness; a drain
hole cuts a cylinder through that wall so the first time the part comes off the
plate, the resin inside can get out.

## What Prusa does

Verified against `/home/makzumi/Downloads/PrusaSlicer-master` (paths below are
relative to that checkout).

1. **Voxelise, then work on the distance field.** `generate_interior`
   (`src/libslic3r/src/libslic3r/SLA/Hollowing.cpp:86`) builds a signed distance
   grid over the mesh, dilates it into a narrow band, optionally runs a
   morphological *closing* of `closing_distance` (compute the interior deeper,
   then grow it back — that is what rounds the inside and kills thin spikes),
   and extracts the inner surface with marching cubes at the iso value
   `min_thickness`. Voxel size comes from `get_voxel_scale`
   (`Hollowing.cpp:642`: `MIN_SAMPLES_IN_WALL = 3.5`, `MAX_OVERSAMPL = 8`,
   `UNIT_VOLUME = 500000`), driven by the model volume and the *Accuracy*
   slider — 0.22 mm for a 100 cm³ part at the default quality, 0.38 mm at a
   litre, 0.86 mm at quality 0.
2. **Stitch it into the model as a negative volume.** `hollow_model`
   (`src/libslic3r/src/libslic3r/SLAPrintSteps.cpp:539`) pushes the inner shell
   onto the CSG stack as a `Difference` part; the boolean itself is deferred to
   slicing. `hollow_mesh` (`Hollowing.cpp:173`) is the non-CSG variant: flip the
   inner shell's normals, merge, and drop outer triangles that are inside the
   cavity.
3. **Holes are cylinders, subtracted, not built.** `DrainHole`
   (`src/slic3r-domain/include/Slic3r/Domain/SLA/DrainHole.hpp`) is
   `{pos, normal, radius, height}` with `stick_out_length = 1 mm` so the
   cylinder pokes out of the surface and the boolean cannot leave a skin over
   the opening. Drilling is either a CGAL mesh `minus`
   (`hollow_mesh_and_drill`, `Hollowing.cpp:704`) or — and this is the part
   worth remembering — **a plain per-layer 2D difference**:
   `cut_drainholes` (`Hollowing.cpp:148`) slices only the hole cylinders at the
   same layer grid and does `diff_ex(object_slices[i], hole_slices[i])`.
4. **Failures are reported, not hidden.** `BadMashForHollowing`,
   `BadHoles`, `BadHolesDrilling`, `BadHolesFailed`
   (`SLAPrintSteps.cpp:466`) end up as object issues.

Settings they expose, with defaults from
`src/slic3r-domain/src/Slic3r/Domain/ConfigDefsSLA.cpp:746-781`: **Wall
thickness** 3 mm (1–10), **Accuracy** 0.5 (0–1), **Closing distance** 2 mm
(0–10); the hole gizmo starts at **4 mm diameter, 6 mm deep**
(`src/slic3r-shared/include/Slic3r/App/Plater/SlaHollowGizmo.hpp:83`), while the
`DrainHole` struct itself defaults to a 5 mm radius and 10 mm height.

## What that means for us

Steps 1–2 exist because Prusa keeps a mesh pipeline: hollow first, then run the
CSG stack, then slice the result into polygons. We do not have a CSG stack and do
not want one — but we already slice any pile of meshes into per-layer loops
(`slicer::slice` → `SliceResult { layers: Vec<Layer { z, segs }> }`) and
rasterise those loops into a mask (`slicer::fill_mask`, non-zero winding).

So the shell can be produced one layer at a time, by erosion, with no mesh
boolean anywhere:

```
solid_i  = fill_mask(layer_i)                // as today, at printer resolution
cavity_i = distance_to_empty_cells(...) ≥ wall   // on a coarse cell grid, in 3D
shell_i  = solid_i AND NOT cavity_i
```

Prusa's own `cut_drainholes` (`Hollowing.cpp:148`) already does holes this way —
a 2D operation on the layer grid — so the layer domain is not a compromise, it
is where their drain holes live too.

What we lose against their approach: there is no inner *mesh*, so the cavity
cannot be previewed as geometry and cannot be a "negative volume" the user
edits. Nothing else: a 3D distance transform is measured in real 3D distance,
so walls are not "measured in the slice plane".

What we gain: no marching cubes, no CGAL, no self-intersection handling, no
interior decimation, and no dependence on the mesh being watertight at all —
with one caveat about open contours, see *Open contours* below.

### The traps

**Ceilings.** Eroding a layer on its own leaves exactly one layer of skin under a
flat ceiling: at the layer under an overhang the cross-section is still wide, so
the cavity reaches to within `layer_height` of the surface. That is a 0.05 mm
ceiling where 2 mm was asked for. The erosion must be three-dimensional.

**Cell centres are not conservative.** A cell's distance is measured between cell
*centres*, so the real surface sits somewhere between the last solid centre and
the first empty one. A cavity cell at distance ≥ `wall` can therefore end up as
much as a full cell closer to the surface than `wall` — walls come out *thin*,
not fat, if you threshold naively. Threshold at `wall + cell` instead (a full
cell, not half a diagonal: the surface can sit past the neighbouring centre).
Walls then land in `[wall, wall + cell]`. There is a sub-cell worst case — a
sheet of emptiness that passes between centres — which cannot matter at 0.25 mm
cells with a 1 mm minimum wall, but the honesty is worth a test that measures
the minimum wall of a hollowed sphere at printer resolution rather than trusting
the algebra.

**Z is not square.** Layers are 0.05 mm apart and cells are 0.25 mm, so the Z
pass has to run on real positions: `dz = layer_height / cell_size` (0.2 cell
units), or vertical walls come out five times too thick.

**Borders.** Pad the stack by `ceil(wall / cell)` cells on every side, including
below z = 0 and above the top layer. Without padding, a model touching the grid
edge gets no wall on that side and the plate-facing skin has none at all.

**Slivers.** Dropping Prusa's closing has a cost: a part just wider than
2 × wall leaves a 0.3 mm cavity, an unprintable slit with no drain, which the
connectivity check would then report forever. Fix on the cell grid with an
opening — erode by 2 cells, dilate by 2 — plus dropping cavity components below
a minimum volume. That is the cheap equivalent of `closing_distance` and it also
rounds the inside the way they do.

## Plan

### Where the numbers come from

One grid **per piece**, sized to the piece's bounding box plus the padding
above, at a fixed 0.25 mm cell size. Per piece, not per plate:

- a per-piece wall thickness and enable flag become possible at all (a plate-wide
  union cannot hold per-piece settings);
- pieces that merely touch do not fuse into one blob with one cavity;
- a small figurine costs a few thousand cells instead of the whole plate;
- holes belong to a piece anyway, so their numbering and persistence stay local.

Overlapping pieces each get their own shell and their own cavity; where they
intersect, the print has both walls. That is the documented policy, not an
accident.

`slicer::fill_mask` and `slicer::occupancy` are already resolution-agnostic
(`min`, `max`, `w`, `h`), so both the fine mask and the cell grid come from the
same segments with no new rasteriser. For a 60 mm piece at 0.25 mm that is
~240 × 240 cells ≈ 58 k per layer.

### Data flow

```
per piece:  layers ──► cell stack ──► 3D distance ──► cavity bits per layer ─┐
                                                                             │
per layer:  piece mask (printer resolution) AND NOT cavity ──────────────────┤
            support layers ──────────────────────────────────── OR ───────────┴─► writer
            drain-hole footprint subtracted from the piece mask first
```

Hollowing only touches the model: `ui.rs::start_slice` slices `self.world` plus
`self.support_mesh` in one call today, so it has to slice pieces separately (or
hollow them first) and merge afterwards.

### Phase 1 — the shell (~2 days)

- `src/hollow.rs`: `HollowSettings { enabled: bool, wall: f32 }`,
  `struct Cells { w: usize, h: usize, pitch: f32 }`, and
  `fn cavity_cells(layers: &[Layer], cells: &Cells, wall: f32) -> Vec<Vec<u64>>`
  (bit-per-cell rows) doing: `occupancy` at cell pitch → exact 2D distance
  transform per layer → 1D parabola pass along Z with `dz = layer_height / pitch`
  → threshold at `wall + pitch` → opening by 2 cells → drop tiny components.
  ~300 lines, no new dependency. The 2D pass and the Z pass are the same
  routine at different ranks, so the result is a true Euclidean distance (a
  sphere, not a box: a box-kernel gives `wall/√2` on diagonals).
- Memory: 1.6 MB per layer of distances in f32, so process the Z pass in column
  blocks (e.g. 64 × 64 columns × all layers ≈ 15 MB) rather than holding a
  ±60-layer window (190 MB). The cavity result itself is bits: ~7 KB per layer
  for a 60 mm piece, ~6 MB per 900-layer plate.
- `Project` gains `hollow: HollowSettings` per piece; default off. Wall default
  3 mm, range 1–10, following Prusa.
- Supports panel: **Hollow** toggle and **Wall thickness**, next to the base —
  they change the solid the same way.
- Slice tab shows the rings for free; the reader already draws `fill_mask`
  output.
- Warnings: wall < 1 mm ("may tear"); a feature narrower than 2 × wall collapses
  to solid, which is expected (help text, not a warning).

### Phase 2 — drain holes (~1.5 days)

- `DrainHole { pos: Vec3, normal: Vec3, radius: f32, length: f32 }` on the piece,
  persisted in `src/project.rs`.
- Placement: click with the hole tool; `supports::bvh::Bvh` already has
  `ray_hit` / `closest_point` / `inside`, so the surface point and normal come
  from one ray. Start the hole 1 mm outside the surface (`stick_out_length` from
  Prusa) so no skin is left sealing it.
- **No analytic ellipses.** The section of a cylinder near its end caps is a
  truncated ellipse, and a near-horizontal hole's ellipse scales as `1/|dz|`.
  Instead test the capsule directly on the cell grid: for every cell in the
  hole's bounding box, the axial coordinate along the hole's axis must be in
  `[0, length]` and the radial distance ≤ `radius`. Correct for every angle,
  no special cases, still only numbers we can persist. The fine mask reuses the
  same cell footprint, so the hole edge is quantised to 0.25 mm — invisible on a
  4 mm hole.
- **Length is not a constant.** A fixed 6 mm cylinder drills through the far
  wall of a shallow cavity. Walk the axis from the surface and stop at the first
  cavity cell, then add `wall + 2 mm`; store the resulting length.
- Subtract holes from the **model layers only**. Do not cut supports — instead
  make support generation treat each hole cylinder as a keep-out, so a trunk
  neither grows out of a hole nor plugs one. (A hole on the plate-facing side
  would otherwise sit inside the raft.)
- Draw the holes as cylinders in the 3D view.

### Phase 3 — the checks that make it trustworthy (~1 day)

- Connectivity per cavity component: flood fill the cell stack; a component with
  no path to a hole footprint warns "this cavity cannot drain". Offer, one
  click, a hole at the component's lowest and highest cell — that is also the
  only placement that drains *and* vents (a sealed cavity traps air, sucks, and
  keeps uncured resin that can burst during post-cure).
- **Open contours.** `fill_mask` does not close loops: it accumulates winding
  along each scanline and, when the count never returns to zero, fills to the
  right edge of the layer's bounds (`src/slicer.rs`, the `if let Some(x0w) =
  span` tail of the row loop). An open mesh therefore leaks, and hollowing would
  amplify a leak into a wrong cavity. Detect it in `slice()` by pairing segment
  endpoints (hash of quantised points), store `open: bool` on `Layer`, and warn
  once per slice: "this model has holes in its surface; the hollow may be
  wrong". Slices are never persisted, so the field is free.
- README/help text, CHANGELOG.
- Help text note: a large flat cavity ceiling is a bridge and cannot be
  supported; suggest smaller cavities or thicker walls there.

### Tests

- `erosion_keeps_the_wall_where_asked` — a square cross-section keeps 2 mm on
  the axis *and* on the diagonal (fails with a box kernel).
- `the_wall_of_a_hollowed_sphere_is_never_thin` — hollow a sphere, then measure
  the minimum distance from a cavity cell to the surface at printer resolution.
  This is the test that catches the cell-centre margin being wrong.
- `a_flat_ceiling_keeps_its_wall` — a **closed** cube, not an open box: the
  top-most cavity layer sits at least `wall` below the top surface.
- `the_bottom_skin_is_as_thick_as_the_top` — same cube, the other end.
- `thin_fins_come_out_solid` — a 1 mm fin under 3 mm walls is not hollowed.
- `a_horizontal_hole_opens_the_wall` and `an_angled_hole_opens_the_wall` — the
  layer at the hole's depth has an opening through the wall, and a hole aimed at
  the cavity reaches it.
- `a_shallow_drain_hole_stops_inside_the_part` — the length walk does not cut
  through the far wall.
- `an_open_mesh_warns_before_it_hollows` — a cube missing one triangle sets
  `Layer::open` and the warning appears.
- `hollowing_is_off_by_default_and_round_trips_through_a_project`.

## Risks

- **Cell size is the quality knob.** At 0.25 mm a feature narrower than ~0.7 mm
  is decided by rounding. Below 2 × wall that is solid either way, but the
  minimum wall of 1 mm leaves little room; keep the pitch in one constant.
- **Slivers and islands.** The opening pass handles most of them; components
  under the volume threshold are dropped, which can leave a cavity that is
  smaller than the user expected — worth a status line when it happens.
- **Per-layer cost.** The only new per-pixel work is `mask AND NOT cavity` in the
  writer loop, which is a few percent of export time. The cell work is
  seconds-per-plate, on the thread that already slices.
- **Slab edges.** The Z pass needs `wall` of layers either side; the column-block
  scheme must include them, or ceiling walls go thin exactly at block borders
  (a classic off-by-one that the sphere test will not catch — test it directly).
- **Suction.** Two small holes drain better than one big one; the connectivity
  warning covers the worst case but not the physics.

## Non-goals

- An editable inner mesh or negative volumes (that is Prusa's model, and it is
  what their CSG stack is for).
- Adaptive wall thickness, hole bridges, per-region thickness.

## Open questions

- Should holes move with the piece when it is moved or mirrored? They are in
  model space, so yes — but that has to be part of the snapshot `History`
  records (`src/project.rs:408`).
- Do we hollow before or after supports are generated? Supports come from the
  solid geometry either way; the open question is only whether a hole may punch
  through a support contact point (the keep-out above says no).
- Pre-existing internal voids (a model that is already hollow, or has a sealed
  designed cavity) are *not* part of our cavity bits, so the connectivity check
  will not see them. Flagging them means flood filling the enclosed empty cells
  as well — worth doing, but it is a second pass.
