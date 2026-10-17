---
name: pxl-review
description: Review finished pxl parts or assets as an art director, on any console, and approve them or send them back with concrete rework notes. Use when a coordinator or reviewer has parts in state done (pxl status), needs to run pxl review, read the review packet (compare pages, sibling lineup, review.md), judge a design against style.md and the source, write per-frame rework notes, or approve with pxl approve / pxl rework.
---

# pxl-review: judge the picture, then write notes an artist can act on

If `pxl --version` fails, `pxl` is not installed: run the installer (`curl -fsSL https://raw.githubusercontent.com/kkollsga/agentsprite/main/install.sh | sh`, Windows: `irm https://raw.githubusercontent.com/kkollsga/agentsprite/main/install.ps1 | iex`) or see the `pxl-operator` skill; `pxl skill install` refreshes these skills.

You review with your eyes. Metrics rank where to look first; they do not decide. A part with IoU 100%
and a blank face is a rework. A part with a lint warning on a deliberate detail is fine.

Judge against three things: the **source** (the input image), the **design language** (`style.md` and the console's `[style]`: `pxl style show`,
`pxl review checklist ASSET`) and the **style frames** the user approved. The standing rules are the same on every console: eyes white with a
pupil, identity colours survive, depth through light and shade (no flat fills), drawn for the cells, consistent across frames and siblings, and
every claim verified at zoom (including your own: a note about a colour or direction carries the region and you check the crop before sending it).

## Procedure

1. **Overview.** `pxl status` lists parts and states. `pxl review --state done --out rv` writes one packet per part
   (`rv/NAME/`) and prints a summary: worst frames, raw frames, siblings found. Add `--family F` to review one family.
2. **Open the packet** in this order:
   - `review.md`: the checklist, the raw (not hand-finished) frames, open notes from earlier rounds and the per-frame table you fill in.
   - `compare/`: original | output | diff for every frame. `zoom/`: the worst frames large.
   - `lineup.png`: the part next to its siblings (same family, template base or `[review] siblings`), each at 1x and zoomed.
     Eyes, shoes, textures and ground lines must agree across them.
   - Still unsure: `pxl compare PART --frames F --on "#406080"` for the 1x read on the level colour,
     `pxl compare PART --frames F,APPROVED` next to a frame you approved, `pxl candidates PART F` for a different start.
3. **Judge** each frame with the questions below. Always view the PNG; never approve from numbers.
4. **Verdict** per part: all frames good enough, or send back with notes. A part is only as good as its weakest frame in the animation. A `reference_missing` warning means a gate could not compare with the source: do not approve on it, get the source image back first.
   - `pxl approve PART --note "style reference: 01_stand"`
   - `pxl rework PART --frames 01_stand,02_wait --note "..."` (one call per group of frames with the same problem; without `--frames` the note is about the whole part)
   The agent sees `[frames] note` in `pxl next` / `pxl claim`.
5. **Follow up.** `pxl status --rework` lists what is still open: a note closes for a frame when the agent edits that frame, so an
   open frame was not touched. Check the edited frames again in the next packet; do not close a note by approving without looking.
6. **Marked finished entries.** `review.md` lists "Marked finished without edit" frames (`pxl finish`, with the artist's reason): check each against the source; if it is not really right, `pxl rework PART --frames F --note ...` reopens it as raw.
7. **Close the loop.** Approved/locked frames feed `exemplar` and `auto` for other parts. After approving, tell the coordinator which frames are the style reference.
   `pxl status --metrics` shows how many parts needed rework and why; recurring reasons belong in the briefing of the next batch.

## Questions per frame

The packet's checklist is the project's list (the coordinator keeps it in `references/checklist.md`); these are the standing ones.

- Eyes: as many as the source shows (never one where it shows two), white with a pupil, looking the right way, pupil on the gaze side, not a block with a divider?
- Face: muzzle, mouth, separator between face and body? Or one flat blob?
- Identity colours (shoes, gloves, signature accents, the identity list): present, with highlight and shade?
- Outline: closed, 1 px, smooth? Doubled steps, L corners, stair-blob limbs?
- Limbs and hands: lines and clusters with a shape, or lumps?
- Noise: orphans, checkerboard, pillow shading?
- Texture: one unit repeated at one phase, same as the siblings, or speckle? (`pxl pattern suggest` shows what the original repeats.)
- Consistency: same eye, shoes and ground line as the neighbouring frames, the siblings and the approved ones?
- At 1x on the background: does it read as the character?

Judge against what a careful artist could do in this resolution, not against the reference pixel-for-pixel.

## Writing rework notes

Specific, visual, actionable, per frame. Name the problem, where it is, and a direction. Coordinates help
(x,y,w,h as in `pxl show`). Do not prescribe an exact pixel pattern unless it is the only fix; let the artist draw.
Name the problem with the checklist's own word (eyes, outline, texture ...): `status --metrics` groups notes by the first checklist keyword a note mentions.

Good:
```
pxl rework PART --frames 01_stand --note "eyes are one white block with a vertical divider (x16-19,y11-13): draw two eyes, each 2x3 white
with the pupil on the right side (looking right). No muzzle or mouth below the eyes (y14-16): add a light
patch with a 1 px dark nose and a 1 px mouth line. Shoes lost their bright highlight: one light pixel on top."
pxl rework PART --frames 02_wait1,02_wait2,02_wait3 --note "eyes: copy the fixed eye and muzzle from 01_stand (snippet save + stamp), change only the pupil."
```
Bad: "Looks wrong, fix eyes." / "Raise the score." / "Make it closer to the reference."

Template for a note: frame -> what is wrong (visible) -> where (coordinates) -> direction -> reuse hint
(snippet, copy from frame N). Group repeated problems ("all walk frames: shoes flat red, add highlight").

## Reviewing against the design language (gated projects)

`pxl review checklist ASSET` prints the rules of `style.md` as a checklist, the asset's identity list, the current design-gate warnings and the nearest style frame:
judge the asset against those, and against the source. A note that enforces a rule names it: `pxl review note ASSET FRAME "text" --region x,y,w,h --rule eyes --agent R`.
The lint gates (`eye_parity`: as many eyes with white and pupil as the source shows; `identity_colour`; `flat`; `consistency`; `readability`) block
`pxl review approve` until they are fixed (`pxl lint ASSET` prints ready-made fixes) or you accept them in writing with `--justify "why"`. A justification is for a
deliberate, visible reason ("the cell holds three colours, the shell's light tone is folded on purpose"), never for "the tool is wrong". Never approve a face with one eye.

## When to approve

All frames answer yes to the questions, or the remaining flaws are named in the approve note and
acceptable (hidden in the reference, invisible at 1x). Lint errors must be 0 (hardware). Check `pxl status` shows
no STALE, export fresh and raw 0 (`pxl status --raw`: everything is hand-finished). Do not demand polish on a
one-pixel invisible detail while a limb is a blob. Do not approve because the agent said "done"; they judged their own work.
