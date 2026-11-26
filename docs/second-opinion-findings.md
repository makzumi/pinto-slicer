# Second-opinion findings — HALOT-X1 support-failure debugging

Review of the debugging effort described in `second-opinion-prompt.md`.
Method: independent re-derivation of the CXDLPv4 pixel-pair codec (cross-checked
against UVtools v7.0.3 / PR #1016 and against the project's own test decoder in
`src/cxdlp.rs`), then a full per-layer decode of both exports named in the
prompt. All numbers below come from that decode. No files were modified.

- `halot-drivemode.cxdlpv4` — "theirs" (HALOT Box export, prints fine)
- `drivemode_v2.cxdlpv4` — "ours" (v0.1.26 export, about to print)

---

## TL;DR

1. **The v0.1.26 export does not contain the cone trunks you think it does.**
   You shipped "cones Ø5→Ø1.6". The file has Ø1.53 mm at the pad top tapering to
   Ø0.74 mm at the tips — the Ø5 feet are absent (p90 = Ø1.65 mm at z = 0.85).
   The print you're about to run does not test hypothesis #1 as intended.
2. **The export runs the wrong cure profile.** It carries 35 s × 5 bottom /
   2.5 s normal. The machine's *proven* profile (the one that prints HALOT Box
   files on this exact machine) is 30 s × 4 / 1.5 s. Every print test so far
   has been confounded by +67 % exposure — higher FEP adhesion and more brittle
   supports. ("Matched all defaults in v0.1.21" is not true of this file.)
3. **"Thin columns snap" is contradicted by the control.** Their columns are
   *thinner* than yours almost everywhere (median Ø0.84–0.88 mm from z=2 to
   z=5, vs your Ø1.0–1.5 mm) — and they survive. The real differences are
   elsewhere: 2× the column count, fat feet at the column bases (median
   Ø2.47 mm at z=1.05, max Ø53 mm), and a monolithic raft instead of small
   isolated pads.
4. **Per-column peel-force budget:** your columns carry ≈1.9× the force from
   count alone, ≈2.7× including the exposure delta; tip stress ≈2.1× theirs.
   This is the most likely mechanical root cause (tip tear-out → pieces left on
   the FEP), and it matches the v0.1.19/v0.1.24/v0.1.25 partial-failure
   signature.
5. **Your byte-level measurements are accurate.** Column counts (1167 vs 666),
   the 80.6 % raft, the Ø5 cone feet (as the *maximum*, not the median), the
   exposure/motion slots — all verified against my independent decode. The
   measurement pipeline is not the problem; the conclusions drawn from it are.

---

## 1. What I verified about the file format

The pixel-pair RLE interpretation is correct in `src/cxdlp.rs`. I re-derived it
independently and matched it against UVtools v7.0.3's
`PixelPairRleCodec.cs` (the HALOT-X1 codec added in PR #1016):

- unit byte = `0x80 | (L>>5) | ((R>>5)<<3)`, bit7 must be set, bit6 clear;
- repeat = big-endian base-128 groups, all groups have bit7 clear; the run ends
  at the next unit byte (bit7 set);
- a run is `repeat + 1` units; runs never cross rows; 7560 units/row, 6230 rows;
- gray expand `(g<<5)|(g<<2)|(g>>1)`.

Your encoder, the UVtools decoder, and my Python decoder agree on both files.
**The RLE layer data is not the problem.** (One caveat: an earlier revision of
this review reported nonsense statistics from a decode bug of my own —
"0.04 % lit everywhere". Disregard anything you heard from me before this
document; the numbers here are the corrected, cross-validated ones.)

Byte-level parameter comparison (both files):

| Slot | theirs | ours | verdict |
|---|---|---|---|
| lift | 8 mm @ 300 mm/min, single-stage | same | identical |
| retract | 300 mm/min, h2=0, spd2=0 | same | identical |
| rest times (pre/post lift, post retract) | 0, 0, 0 | same | identical |
| light-off | 4.0 s all layers | 4.0 s all layers | identical |
| PWM | 255 | 255 | identical |
| bottom layers | 4 | 5 | intentional |
| bottom exposure | 30 s | 35 s | **not matched** |
| normal exposure | 1.5 s | 2.5 s | **not matched** |
| PrintParameters tail floats | `[0,0,0,30,1.5]` | `[0,0,0,35,2.5]` | internally consistent, but ≠ control |
| padding block `[5, 120, 6, 1]` | present | present | matched |
| volume/weight/cost metadata | 112.2 ml / 123.4 g / $3.70 | 91.5 ml / 100.6 g / $0 | cosmetic |

The v0.1.17-class slot bug is gone: exposure appears consistently in the
per-layer table, the PrintParameters block, and the tail. The file says 35/2.5
everywhere — that is what the firmware will do. The problem is that the
**value** is not the proven one, not that it's in the wrong slot.

---

## 2. Ground truth per file

### "Theirs" — HALOT Box export (901 layers, z to 45.05 mm)

| z (mm) | lit % | components | median Ø | max Ø |
|---|---|---|---|---|
| 0.05–1.0 | 80.6 % constant | 6–7 sheets | — | Ø91 mm (33.3 Mpx²) |
| 1.05 | 41.2 % | 88 | Ø2.47 mm | **Ø53.3 mm** |
| 1.5 | 23.5 % | 710 | Ø1.71 mm | Ø19.0 mm |
| 1.8 | 14.6 % | 854 | Ø1.21 mm | Ø14.9 mm |
| 2.0 | 5.93 % | 1089 | **Ø0.88 mm** | Ø3.57 mm |
| 3–4 | 5.2–5.4 % | ~1110 | Ø0.84 mm | Ø3.3 mm |
| 5.0 | 4.67 % | **1168** | Ø0.84 mm | Ø1.74 mm |
| 5.95 | 4.21 % | 1250 | Ø0.65 mm | Ø1.60 mm |
| 6.5 | 8.47 % | model begins | max Ø22.7 mm | |
| 8–14 | 15.1–14.0 % | | max Ø35–38 mm | |
| 44.5 | 1.04 % | 1 | Ø15.6 mm | |

Notes:

- The raft is a **solid 1 mm slab at 80.6 % of the panel, six sheets, zero
  enclosed holes, inset ~1 mm from the plate edges** (rows 21–6100, cols
  121–7551). It is not ventilated by drain holes; its only "ventilation" is
  the 1 mm edge margins and the gaps between the six sheets.
- The "Ø5 mm cone feet" exist — as the *maximum* component (Ø53 mm at z=1.05
  is a plinth, not a column). The **median** column foot is Ø2.47 mm, and the
  flare lasts only ~1 mm (41 % lit at z=1.05 → 5.9 % at z=2.0). After that the
  shafts are nearly constant Ø0.84–0.88 mm cylinders to z≈5, tapering to
  Ø0.65 mm tips.
- Your count of "1167 columns at z=5" is confirmed (I measure 1168).

### "Ours" — v0.1.26 export (889 layers, z to 44.44 mm)

| z (mm) | lit % | components | median Ø | max Ø |
|---|---|---|---|---|
| 0.05–0.40 | 47.6 % constant | 64 | **Ø4.28 mm** | Ø46.6 mm (8.68 Mpx²) |
| 0.45 | 19.1 % | | | |
| 0.5 | 18.1 % | 559 | Ø2.18 mm | Ø17.6 mm |
| 0.85 | 10.6 % | 579 | **Ø1.53 mm** | Ø9.6 mm |
| 1.0 | 8.27 % | 581 | Ø1.45 mm | Ø6.8 mm |
| 2.0 | 5.99 % | 555 | Ø1.33 mm | Ø2.25 mm |
| 3.0 | 4.83 % | 557 | Ø1.16 mm | Ø2.22 mm |
| 5.0 | 4.05 % | **631** | Ø0.84 mm | Ø2.21 mm |
| 6.0 | 3.73 % | 671 | Ø0.74 mm | Ø2.32 mm |
| 6.5 | 7.51 % | model begins | max Ø22.9 mm | |
| 9.4 | 16.21 % (peak) | 393 | | Ø32.4 mm | |
| 44.4 | 0.17 % | 1 | Ø6.3 mm | |

Notes:

- The pads: 96 cluster pads render as **64 components** (adjacent hulls merge).
  Median pad is **Ø4.3 mm** (19.5 mm²); p90 Ø21.1 mm; the largest pad component
  is **2310 mm² — above your stated 900 mm² cap** (merging of capped hulls, or
  the cap not applied to unions — worth a look).
- The pad profile in the file: flat 47.6 % from z=0 to z=0.42, then
  rim-only (19.1 % → 8.3 % at z=1.0). This matches your "full-thickness rim +
  recessed middle" description: rims are ~1.0 mm tall, middles ~0.42 mm.
  At z=1.0 the rims are already fragmented by the drainable seams (581
  components ≈ 6 fragments per pad).
- **The "Ø5→Ø1.6 cones" did not make it into this export.** The trunk is
  Ø1.53 mm at the pad top, tapering smoothly to Ø0.74 mm at z=6. The p90 is
  Ø1.65 mm at z=0.85, so ≥90 % of trunks have no Ø5 foot at all. This file's
  supports are essentially the v0.1.25 straight sticks with a modest taper —
  geometry the machine has already rejected.
- Your count of "666 columns" is confirmed (631–671 depending on z).
- All 14 pieces are present at the right z (peak model+support lit 16.21 % at
  z=9.4 vs their 15.35 % at z=8.25; the cross-sections track each other
  through z=44). **The model content is fine.**

---

## 3. Answers to the five questions

### Q1. Does the diagnosis chain hold? Is anything inconsistent with "columns snapped mid-print"?

The chain (exposure-slot bug → fixed → partial prints → mechanical failure) is
sound, and the byte-level evidence supports "supports are the failing element."
But **"thin straight columns snapped" is contradicted by the control file**:
their columns are thinner than yours (median Ø0.84–0.88 mm for most of their
height, vs your Ø1.0–1.5 mm) and print reliably. Thinness alone cannot be the
differentiator.

What *is* consistent with "columns/tips failed mid-print":

- tip tear-out under ≈2× higher tip stress (see Q3 math) leaves pieces on the
  FEP — exactly the partial-failure signature you saw;
- +67 % exposure makes supports more brittle (higher crosslink density) and
  raises per-layer FEP adhesion, both of which turn peel events into snap
  events;
- the pieces that *did* print were "fully cured and solid" — expected, because
  nothing in this picture hurts the model itself.

Inconsistencies to note: the claim "cones Ø5→Ø1.6, matching HALOT Box's
measured profile" is not what the export contains, and "matched all defaults
in v0.1.21" is not what the export shows (35/2.5/5 vs 30/1.5/4). The chain
holds; two of its load-bearing links are measurements that never made it into
the tested file.

### Q2. What failure mechanisms are we missing?

Ranked by how well they explain the evidence:

1. **Exposure confound (not in your list).** Every test print since v0.1.18
   ran at 2.5 s/35 s while your control prints at 1.5 s/30 s. Higher exposure
   ⇒ higher FEP adhesion per layer ⇒ higher peel force on every support, plus
   brittle supports. This is a *constant offset* applied to every geometry
   hypothesis you have tested — none of those tests were clean.
2. **Force concentration from the 2× column gap.** Same peel area, half the
   columns ⇒ ~2× per-column force. Combined with (1), tip stress is ≈2.1×
   theirs (see Q3). This is the mechanical mechanism that would tear tips off
   and strand pieces on the FEP.
3. **Base anchorage.** Their columns grow from a monolithic 80.6 % raft
   (largest sheet 8850 mm²) with Ø10–53 mm plinths/feet under the pieces. Your
   columns grow from isolated pads — median **Ø4.3 mm (19.5 mm²)** of plate
   contact per cluster, further weakened by the rim-recess (middle only
   0.42 mm thick) and cut into fragments by drainable seams. The recessed
   middle *reduces* plate bonding while the control *maximizes* it — the
   design philosophy is inverted relative to the control.
4. **Stiffness, not thickness.** Your columns are thicker ⇒ ~5× stiffer in
   bending (d⁴) ⇒ they transmit peel shock to the tips instead of absorbing
   it. Their thin (Ø0.85), numerous, flexy rods peel gracefully. This is
   why "add thickness" is the wrong direction — the control wins by being
   *thinner and more numerous*.
5. **Suction/ventilation (can be ruled out).** The control has a solid
   80.6 % slab for 1 mm — worse suction area than your 47.6 % pads — and
   prints. Model cross-sections are nearly identical in both files (16.2 % vs
   16.1 % peak). Suction is not the differentiator on this machine.
6. **External factors.** Still possible as an *interaction* (worn FEP raising
   adhesion hits your supports harder than the control's), but the same
   machine prints HALOT Box files, so it cannot be the primary cause. A
   control print immediately before/after each test print is cheap insurance.

### Q3. Is chasing the 2× column count worth it before another print?

Yes — but **not before un-confoundering the exposure**. The math:

- Peel force on the current layer ≈ (adhesion stress × lit area) / (number of
  attached columns). At the piece-bottom layers the lit areas are nearly equal
  (7.5–8.5 % both files); at the peak cross-sections they're 16.21 % vs
  15.35 %. So per-column force ratio ≈ 1168/631 ≈ **1.9×** from count alone.
- Adhesion stress roughly scales with exposure: ×(2.5/1.5) ⇒ **≈2.7×**
  per-column force.
- Tip areas: yours Ø0.74 mm vs theirs Ø0.65 mm (area ratio 1.3×) ⇒ net **tip
  stress ≈2.1× theirs**. The tips are the weakest joint in both designs, and
  yours are overloaded by a factor of two. Shaft stress, by contrast, is about
  equal (your thicker shafts compensate the count gap — which also shows the
  extra diameter is doing the *shaft's* job, not the *tip's*).

So: the count gap matters, but it compounds with exposure rather than standing
alone. Print order that maximizes information (see Q4) puts the exposure test
first because it's the cheapest and cleanest, and it makes the count test
interpretable either way.

### Q4. Single most informative next experiment?

**Zero-cost, before anything: recover the failure evidence from the last
print.** Where did the breaks happen — at the tips (pieces + tip stubs on the
FEP, supports standing on pads), at the column bases (columns + pieces loose,
pads still on the plate), or at the pad-plate joint (everything gone, bare
plate)? You may already know this from v0.1.17 ("support stubs at the bed")
but the v0.1.19/24/25 signature doesn't state it. Each answer pins a
different hypothesis: tip failure ⇒ overload/brittleness; column-base ⇒
stiffness/fatigue at the foot; pad lift ⇒ anchorage.

**Then, the single most informative print:** re-slice the *current* plate
geometry (no changes) at the proven cure profile — 30 s bottom × 4, 1.5 s
normal. This is the only experiment that isolates the exposure confound from
every geometry hypothesis at the cost of one print. If it prints: exposure was
the dominant cause and all previous geometry conclusions need re-evaluation.
If it still fails identically: exposure is exonerated and the geometry
(columns/pads) becomes the sole suspect — and you now have a clean baseline
for geometry changes.

Only after that: clone the control's support recipe (2× columns, Ø2.5 mm feet
over a raft or full-area pads, Ø0.85 mm shafts, Ø0.65 mm tips) as a mechanical
control print — a "support recipe parity" test, not a fix. And consider the
cheap in-vat torture test: print *only* the support system (no model) with a
known sacrificial weight per column; you can iterate column/pad/tip designs in
~30-minute prints with almost no resin.

### Q5. Is diff-and-match the right strategy?

Parity-by-measurement is a sound debugging method and your measurements are
accurate — but the strategy has two structural flaws exposed by this review:

1. **Converging toward a partial description of the control.** You matched
   the *maximum* cone (Ø5) rather than the distribution (median Ø2.47 feet,
   Ø0.85 shafts), and you never matched the cure profile. Parity has to be
   parity of the *effective* values: what the firmware does with the file.
2. **Testing several changes at once.** Each print changed exposure, pad
   geometry, and support geometry simultaneously. With one machine, one plate,
   and 3 h per data point, every print must differ from the last by exactly
   one variable.

The fundamentally better loop is: (a) establish a clean control — your file
with the proven cure profile; (b) bring the support *system* (not individual
knobs) to the control's measured recipe — count, shaft Ø, foot Ø, base type;
(c) then walk *away* from parity one parameter at a time to find the minimum
viable recipe (that's where the fast support-only prints pay off); (d) use
force-budget math like the Q3 calculation as a pre-filter for every candidate
change — if a change doesn't improve the tip-stress or anchorage budget, it
probably isn't worth a print.

---

## 4. Ranked hypotheses with discriminating tests

1. **Tip/anchorage overload (force concentration: 2× column gap × exposure ×
   small pads).** *Test:* debris forensics (tips vs bases vs pads); the Q4
   exposure print; per-column force-budget calculation before/after any
   change; support-only torture prints.
2. **Exposure confound — brittleness + elevated FEP adhesion.** *Test:* the
   Q4 print (same geometry, 30/1.5/4). Also: if any previous successful pieces
   or supports can be flexed, over-cured supports will snap rather than bend.
3. **Column/support stiffness (your H1, reinterpreted).** *Test:* compare the
   v0.1.26 export as-is (it is *not* the cones you designed) against a true
   clone of their column profile (Ø2.5-foot → Ø0.85 shaft → Ø0.65 tip, 2×
   count). If stiffness were the answer, thinner-not-thicker would fix it —
   the opposite of the current design direction.
4. **Base anchorage (your H3).** *Test:* replace pads with a full raft or
   full-area pad field in one print (mechanical control); measure the largest
   pad (2310 mm² > your 900 mm² cap — inspect the cap logic). Debris forensics
   distinguishes this from (1).
5. **External factors (your H4).** *Test:* HALOT Box control print on the
   same day as each test print.

---

## 5. Fresh-eyes items you may want to check in the code/process

- The Ø5 cone feet not appearing in the export (generator config, radius
  math, or unit mismatch — mm vs px is a classic here; note the plate pitch is
  anisotropic: 14.0 µm/px in x, 19.0 µm/px in y).
- The pad cap: largest measured pad component is 2310 mm² vs the 900 mm² cap
  (hull merging?).
- Exposure values in the tested exports: 35/2.5/5 has been in every post-fix
  print; the proven profile is 30/1.5/4. Reconcile this with "matched defaults
  in v0.1.21".
- Your per-layer measurement pipeline is accurate (counts, 80.6 % raft, Ø5
  max-feet all reproduce) — the risk is not in measurement but in which
  statistic you match (median vs max).
- Consider recording, per failed print: which pieces survived *and where they
  sat on the plate* (center vs edge matters for FEP deflection/peel
  geometry) — the 15 % that survive may be position-correlated, not random.
