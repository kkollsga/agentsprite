---
name: pxl-sprite
description: Convert high-resolution art (character, enemy, object, effect frames) into hand-finished low-resolution pixel art for a retro console with the pxl CLI. Use when you have a pxl asset or part to work on (pxl next / pxl claim), when a pxl frame looks crude, blocky or lost its eyes, face, hands or key colours, or when asked to redraw, fix, polish or animate sprite frames at a hardware resolution such as 16x16 to 32x32 on any console.
---

# pxl-sprite: you are the pixel artist

If `pxl --version` fails, `pxl` is not installed: run the installer (`curl -fsSL https://raw.githubusercontent.com/kkollsga/agentsprite/main/install.sh | sh`, Windows: `irm https://raw.githubusercontent.com/kkollsga/agentsprite/main/install.ps1 | iex`) or see the `pxl-operator` skill; `pxl skill install` refreshes these skills.

`pxl` gives you fast views, cheap edits and hard hardware limits. It does not know what looks good.
You do. Automatic reductions (vote, contour, exemplar, auto) are rough sketches to start from.
Lint, IoU and the `eyes` column are hints about where to look. They are never the goal, and a frame
can pass all of them and still look bad. The only hard rules are the hardware errors of the project's console (palette
counts, colour format, sprite budget): `pxl lint` errors must be 0 before export.

You own the look. Judge with your eyes, on the PNG, not with the numbers.

## Start here: the console and the design language

1. `pxl console show` (the project's console; `pxl console list` shows all nine): colours and palettes, cell size, sprites per line,
   and its `[style]` hints (white, outline, value spread). Hardware facts are not up for debate; looks are yours.
2. If the project has `style.md` (`pxl style show`), it is your brief: the outline rule, the eye and face rule, the identity list, the
   shared material ramps, light direction, the white and highlight rule, minimum feature sizes, how to fold colours. A project's
   `[style]` overrides the console's. Without one, use the general rules below.
3. `pxl brief ASSET` (in an asset project) prints decisions, tasks, review history, budgets and where the compare images are.

## The general rules (the same on every console)

- **Eyes are white with a pupil.** Both eyes, as many as the source shows, each at least 2x3 target pixels of white plus a 1 px pupil on
  the gaze side, never merged into the outline. A face with one eye, or a white block with a divider, is a failed frame.
- **Identity colours survive.** The 2 or 3 colours that say "this character" (shoes, gloves, a moustache, a glass panel) are in every frame
  where the source has them, with a highlight and a shade. Fold other colours first.
- **Depth through light and shade.** Every material keeps its light, mid and dark (as many tones as the console allows) with one light
  direction across the project. A flat fill where the source has form is a defect. On a console with few shades (the classic Game Boy) use the
  whole value range; white is a main tone there, a highlight on most other consoles (`[style] white`).
- **Draw for the cells.** Consoles place sprites in fixed cells with limited colours per cell: plan colour changes on cell borders and
  reuse cells (`references/cells.md`). Do this from the first version, not as a late fix.
- **No flat fills, no noise.** No large single-colour regions the source does not have; no orphans, checkerboards or pillow shading.
- **Consistency.** The same material, eye, outline and ground line across frames, siblings and the approved style frames.
- **Verify claims at zoom.** Before you say "the source is red here" or "this matches", look at the crop (`pxl show --ref`,
  `pxl compare --zoom`), including a reviewer's claim.
- **Hand-finish everything.** Automatic reductions are sketches.

## The loop

1. **Claim.** `pxl next --agent NAME` prints the PART (or asset) path and the worst frames. Put `--agent NAME`
   on every editing command. Never edit templates or the manifest (ask the coordinator).
2. **Look at the reference large.** `pxl view PART FRAME --zoom 8` and Read the PNG.
   Ask: what makes this character *this character*? List 4 to 6 things that must still read at 1x
   (silhouette, eyes, face/muzzle, a signature colour like shoes or gloves, one or two shape quirks).
   See `references/craft.md`.
3. **Pick the best start per region.** `pxl candidates PART FRAME`, Read the sheet, then
   `pxl pick PART FRAME contour --region x,y,w,h` for each region where a method got it right
   (face from one, body from another). Throw away what is wrong; do not defend a start point.
4. **Draw the key features first**, in this order: eyes, face (muzzle, mouth, face/body separator),
   hands, feet, then the outline and limbs. Draw them by hand with `patch`, even if a candidate
   was close. Save a good one: `pxl snippet save PART FRAME eye --region x,y,w,h`.
5. **Refine.** Clean outlines and limbs, fill clusters, fix colour roles. After each edit
   look (`--preview` or `pxl view`). Edit, look, edit.
6. **Check at 1x and zoomed.** `pxl compare PART --frames FRAME --zoom` and
   `pxl compare PART --frames FRAME --on "#406080"` (or the level colour). Put it next to the
   reference and next to your best approved frame.
7. **Self-review** with the questions in `references/self-review.md`. Fix what a picky art director
   would circle. Stop when the answers are honestly yes, not when the metrics are green.
8. **Next frame.** Reuse: `stamp` the eye and shoe snippets, `copy` unchanged regions from the
   approved frame, change only what moves. Do the hardest frame first and let `pxl reload PART --method auto`
   learn from it for the rest, then hand-finish those too.
9. **Final review of the whole part** (`references/self-review.md`, "Final review before release"):
   every major feature of the original present in every frame, white used as the style says, greys and
   other big colour areas keep their hue. Fix, then:
10. **Lock and release.** Edits lock frames automatically. `pxl lint PART` (0 errors), `pxl export PART`,
   `pxl release PART --state done --note "what you chose and what is still weak"`.

## Working fast with pxl

- **show, edit, patch round trip** is the main tool. `pxl show PART FRAME --region 12,6,10,8 --ref`
  prints rows with labels and the reference letters. Copy the rows, change letters, send them back:
  `pxl patch PART FRAME -` with the rows on stdin. Spaces keep pixels, `.` is transparent.
  Rulers, headers and reference columns are ignored.
- **Preview any edit** with `--preview`: it writes a reference | before | after PNG and prints the
  path. Read it. Use `--dry-run` to see the result without writing.
- **Many small edits** in one atomic undoable step: `pxl apply PART ops.json` (see `references/tool.md`).
- **Reuse**: `snippet save` / `stamp` (with `--flip x`) for eyes, shoes, gloves; `copy --region` between
  frames; `move` and `nudge` to shift parts; `mirror`/`flip` for symmetry.
- **Undo** is cheap: `pxl undo PART --steps N`. Try bold things.
- **Argument order pitfalls**: `row PART FRAME x=X y=Y "keys"` (named form avoids swaps; it echoes the position), `patch PART FRAME X Y GRID`,
  points are `x,y` (column first), regions `x,y,w,h`. Negative numbers need `--` before pixels.

## Palette sets and cells

On consoles with shared palette sets (`pxl console show`: gb, gbc, nes) a part with `palette_set = "..."` draws with plain keys whose
colours come from the shared set, and the palette of each hardware cell is derived. The details (cell views, fitting, dedupe, parts used
in several scenes) and the rules for drawing for the cells are in `references/cells.md`. Read it when `pxl cells PART FRAME --png c.png`
shows invalid cells (red), a lint says a cell needs a second palette, or the scene budget is over. The player (category `player`, set by
`pxl ingest --group player` or `pxl asset set ASSET category player`) may stack a few cells with two slots (`slot2`, 2 sprites each, `pxl console show`);
`cells.md` explains how the cell grid is chosen per frame (not from the canvas corner) and how to draw for it.

## What to keep in your head

- **Stay inside the reference's shape.** Your freedom is in how you draw inside the silhouette
  (±1 px), not in reinventing it. Read the diff panel of `pxl compare --zoom` after every big change:
  large red or green areas mean you drew a different pose. Redraw a head *in the reference's
  angle*; never replace a 3/4 head with a front face.
- **Only copy from another frame when the reference pose there is the same.** Check the diff
  after a `copy` or `stamp`; a transplanted body on a different pose is worse than the rough start.
- **A pupil must read as a pupil**: dark inside or against the white, with white or skin around it,
  not merged into the outline. A white block next to the outline is a wall, not an eye.
- **Spend effort where the frame is weakest.** A frame is not done after an eye fix if hands,
  shoes or limbs are still rough blobs. Compare your frame with the approved one at the same zoom
  and fix every place that looks rougher.
- Hand-finish everything: no frame ships as raw method output. Starts are sketches. `pxl status --raw PART` lists the frames that still equal the automatic output; the coordinator's `pxl check --hand-finished` fails on them. Textures are repeating units (`pxl pattern`, craft 8b), judged in a `pxl lineup` with their neighbours.
- Shrinking is not scaling. Keep the read, simplify the detail, exaggerate what carries identity
  (eyes, hands, feet) by a pixel. A 3 colour sprite is a drawing, not a downsample.
- A frame that matches the reference by IoU but has a blank face is a failed frame.
- Eyes decide whether the sprite feels alive. Spend your effort there before anything else.
- Identity colours (bright shoes, gloves, a signature accent) must survive: one highlight pixel plus a darker shade.
- `pxl lint --advice`, the design gates (`eye_parity`, `identity_colour`, `flat`, `consistency`, `readability`) and `pxl view --features` show notes and detected features: a second opinion, never a requirement.
  If a note flags something you drew on purpose, trust your eye and say so in the release note.

More: `references/craft.md` (how to draw it), `references/tool.md` (recipes, every apply op),
`references/self-review.md` (critique habit and when to stop).

`references/cells.md` (palette sets and cells), `references/craft.md` (how to draw it), `references/tool.md` (recipes, every apply op), `references/self-review.md`.
