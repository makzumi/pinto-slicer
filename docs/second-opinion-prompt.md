# Second-opinion review prompt

---

You are reviewing a debugging effort — analysis only, no changes to anything.

## Context

I'm building **Pinto Slicer**, a desktop resin slicer (Rust/egui) exporting
Creality's CXDLPv4 job format (`.cxdlpv4`) for a **Creality HALOT-X1**:
15120×6230 mono-LCD printer, 0.05 mm layers. The job file carries a complete
per-layer bitmap (RLE-compressed) plus parameter blocks with exposure and
motion values; the firmware adds nothing but light and timing.

The same 14-piece plate has produced three failed prints: nine small buttons
(~17×14 mm), two ~196 mm flat rails, a few small cases — all lifted 6 mm on
generated supports. The owner prints; each attempt costs ~3 hours and ~100 ml
of resin.

## Failure timeline

1. **v0.1.17:** only the bottom film cured — a couple of support stubs and a
   small clump at the bed. Root cause found by byte-auditing Creality's own
   slicer export of the same plate: we wrote the exposure time into a slot
   the firmware does not read — slicer tail floats `[0, 0, exp, bexp, 0]`
   instead of Creality's `[0, 0, 0, bexp, exp]` — and zeros where Creality
   writes `[5.0, 120.0, 6.0, 1]` in a padding block. Fixed and byte-matched
   since v0.1.18.
2. **v0.1.19** (cure 35 s / 2.5 s, 5 bottom layers): ~15 % of the pieces
   printed well and solid; the rest partial or missing entirely.
3. **v0.1.24 / v0.1.25:** the same partial-failure signature. Pieces that
   succeed are fully cured and solid — so I don't believe light is still the
   problem.

## Verified byte-level against HALOT Box's own export of the same plate

(181.8 MB, 901 layers; that export prints fine on this machine)

- Our per-layer images are where they should be: every piece's support
  columns sit inside its footprint; mirrored/offset variants of the footprint
  test contain fewer columns; grayscale, PWM (255) and antialias (0) are
  identical; our RLE encoding re-encodes both files byte-identically.
- Exposure and motion parameter slots byte-match Creality's.
- Remaining measured differences:
  - **Column count:** theirs 1167 at z=5, ours 666. Their placement dedups
    contacts less aggressively.
  - **Trunk shape:** theirs are cones — about Ø5 mm at the foot, slimming to
    Ø1.2 near the model. Ours (through v0.1.25) were straight Ø1.6–2.4 sticks.
  - **Base:** theirs is one plate-wide solid ~1 mm raft (80.6 % of the panel
    lit through ~20 layers). Ours is 96 separate pads, each capped at
    900 mm², with a full-thickness rim and a recessed middle.
  - **Resin:** theirs 112 ml, ours 86 ml.
- The machine's *proven* cure profile (HALOT Box built-ins the owner has
  printed successfully for years): 30 s bottom, 1.5 s normal, 4 bottom
  layers, rise 8 mm @ 300 mm/min, light-off 4 s. We matched all our defaults
  to this in v0.1.21.

## Shipped since the last failure

- Support-placement overhaul: overhang coastlines walked at 4 mm spacing,
  hollow-ceiling self-supports, per-foot pad caps with drainable seams,
  rim-lipped pads.
- **v0.1.26 (unverified, about to print):** cone trunks Ø5 → Ø1.6, matching
  HALOT Box's measured profile.

## My current hypothesis ranking

1. Thin straight columns snapped or deflected under peel forces before the
   piece bottoms cured in — cones should address this.
2. The 2× column count gap matters more than I think.
3. Pad first-layer bonding.
4. Something outside the file (resin temperature, FEP film, plate leveling) —
   though the same machine prints HALOT Box files fine.

## Questions

1. Does the diagnosis chain hold? Is anything in the failure signature
   *inconsistent* with "columns snapped mid-print"?
2. What failure mechanisms are we missing entirely?
3. Is chasing the 2× column count worth it before spending another
   3 h / 100 ml test print?
4. What is the single most informative next experiment, print or no print?
5. Sanity-check the overall strategy: we've converged every knob toward
   HALOT Box's measured profile ("parity by measurement"). Is there a
   fundamentally better way to find the true root cause than this
   diff-and-match loop?

Rank your hypotheses, and for each give a test that would discriminate it
from the others. Do not propose code changes; do not modify files.

## If you have filesystem/repo access (optional)

- Repo: `/run/media/makzumi/storage/Projects/pinto-slicer` — job writer in
  `src/cxdlp.rs`, support generator in `src/supports/`. Read only.
- Our latest export: `/home/makzumi/Desktop/New Folder/drivemode_v2.cxdlpv4`
  (159.9 MiB, 889 layers, v0.1.26 with cones).
- HALOT Box's export of the same plate:
  `/home/makzumi/Desktop/New Folder/halot-drivemode.cxdlpv4`
  (173.4 MiB, 901 layers).
- Format details worth knowing: 15120×6230 pixels, 0.05 mm layers, pixel-pair
  RLE codec, 120-byte layer records, param blocks at offsets table−68−76 and
  table−76, slicer tail floats `[0, 0, 0, bexp, exp]`, motion 8 mm rise /
  300 mm/min / 4 s light-off.
