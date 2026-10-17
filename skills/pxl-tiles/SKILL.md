---
name: pxl-tiles
description: Hand-finish a pxl tile set (a level's block sheet, an animated tile part, a font, a screen) within the unique-tile, colours-per-tile and palette limits of the project's console. Use when you have a pxl part or asset of mode tile or screen to work on (pxl next --kind tileset / pxl claim), when tiles look blurred, patchy or break at the edges against their neighbours, or when textures (waterfall, checkered soil, bricks, marble, metal plating, water) must repeat cleanly and line up, on any console.
---

# pxl-tiles: you are the tile artist

If `pxl --version` fails, `pxl` is not installed: run the installer (`curl -fsSL https://raw.githubusercontent.com/kkollsga/agentsprite/main/install.sh | sh`, Windows: `irm https://raw.githubusercontent.com/kkollsga/agentsprite/main/install.ps1 | iex`) or see the `pxl-operator` skill; `pxl skill install` refreshes these skills.

Console first: `pxl console show` (the project's console) gives the tile budget, colours per tile, palettes and screen size; `style.md`
(`pxl style show`) and the console's `[style]` give the outline, ramps and light direction every tile follows. The rules below hold on every
console; the numbers in this skill are the Game Boy Color's, as the worked example (`references/gbc-bg.md` has the GBC-only features).
The lessons that hold everywhere: every material keeps light, mid and dark with one light direction (depth, no flat fills); the same material
uses the same ramp in every set and zone (consistency); draw for the 8x8 grid (units of 2, 4 or 8 px) and for the console's attribute
blocks; identity colours of the level (water, lava, a signature grass) survive; and verify every claim at zoom in a lineup, not in one tile.

A tile set is the art a whole level is built from. The tool reduces the native blocks, fits the palettes
and merges near-duplicates automatically. That is a rough sketch of 1000 tiles, not a finished set.
**Every used tile is hand-finished before release**: nothing ships as raw automatic output. The tool
counts it for you: `pxl status --raw` lists the frames and tiles that still equal the last automatic
result, and `pxl check --hand-finished` fails while any remain. Craft first: read
`pxl skill show pxl-sprite --ref craft` sections 8b (textures), 10b (consistency across objects) and
this skill's `references/textures.md`.

Hard limits (errors, never negotiable) come from the console's tile mode (`pxl console show`, the `[tile]` section): colours per tile,
palettes (and which are reserved), the unique-tile budget **including the slots animated tiles reserve** (`unique_tiles_max`; a project may
override it in `consoles/<id>/tile.toml`), and on the NES the 16x16 attribute blocks (lint `attr_block`). GBC example: at most 4 colours per 8x8
tile, 7 usable palettes in first-fit order (BG palette 7 reserved for HUD and text), at most 348 unique tiles. `.` is never allowed in a tile.
A unique tile is an index pattern up to mirroring, whatever its palette: the same shape in two palettes is one tile, so draw
repeating materials with the same shape roles (dark, mid, light) in every palette. Everything else
is your judgement: lint warnings, seam scores and compare numbers say where to look.

## The pipeline (do the steps in this order)

1. **Claim a tile part.** `pxl next --kind tileset --agent NAME` (or `pxl claim PART`). One agent per part.
   Put `--agent NAME` on every edit. Never edit templates or the manifest (ask the coordinator).
2. **Stats.** `pxl tiles stats PART`: colours per tile, unique count, duplicate groups, palette packing.
   Read it as a budget: how many tiles must go (unique minus the console budget), how many are over the colour limit.
3. **Fit.** `pxl tiles fit PART` (dry run), then `--apply`. Recolours the cheapest way so every tile has
   at most the console's colours per tile and the first-fit packing needs at most the console's palettes. Fit output is still raw.
4. **Dedupe.** `pxl tiles dedupe PART --max N` with the console budget (dry run lists every merge with its error), then
   `--apply`. Invisible merges go first. Read the
   list: a visible merge of two tiles that sit next to each other in the level is a defect; undo and
   redraw the pair so they merge by design (same drawing, one difference removed).
5. **Look at the sheet as it will be used.** `pxl compare PART` gives pages of reference block, tile at 2x and the diff, ranked by
   error x use: the first tiles on it are where hand work pays most. Native blocks are in `show PART blocks --tile N --ref`. Put
   neighbouring tiles side by side with `pxl lineup` to judge the edges.
6. **Hand-finish every used tile.** The loop is in "Finishing a tile" below. Textures use `pxl pattern`
   with units aligned to the 8x8 grid (see `references/textures.md`). Tiles that must repeat
   (ground, wall, water, sky) are checked in a lineup of the tile beside itself. Animated tile parts (frames `<name>_00`, `_01`, ...) and palette
   cycles are finished frame by frame and checked as a loop.
7. **Edges.** Lineup the busiest neighbours again and run `pxl lint PART`.
8. **Exit criteria**, all true before `pxl release`:
   - `pxl lint PART`: 0 errors;
   - within the console's unique-tile budget (`pxl tiles stats PART`);
   - `pxl status --raw PART` lists nothing (raw = 0), and `pxl check --hand-finished` passes;
   - you looked at the PNG of each texture family in a lineup (below) and it repeats evenly;
   - `pxl export PART` then the client's checker (if any) says PASS; it is necessary, not sufficient (`pxl check` checks the scene).
   Release with `pxl release PART --state done --note "what was hand-finished, merges you redrew, what is weak"`.

## BG budget, reserved palettes, priority, cycles (GBC features)

On the GBC, `pxl tiles stats` counts by index pattern and shows the animated slots; BG palette 7 is reserved for HUD and text; foreground
priority, palette cycles and underwater palettes are tile-part features. They are in `references/gbc-bg.md`: read it when the part declares
`bg_palette`, `[priority]`, `[[cycle]]` or `[[underwater]]`, or when lint says `tile_palettes`, `priority_color0` or `unique_tiles`.

## Finishing a tile (the loop)

```
pxl show PART blocks --tile 37 --ref          # 8x8 rows + the 16x16 native block (rows 0a/0b...)
pxl patch PART blocks - --tile 37 --agent A1  # stdin = the edited rows; coordinates are 0..7 inside the tile
pxl set PART blocks 3,4=c 4,4=c --tile 37 --agent A1 --preview
pxl show PART blocks --tile 37                # echo: palette, placements, unique or duplicate of another tile
```
Ask of each tile: does it read at 1x next to its neighbours (the level page), does the edge continue into
the next tile, is the lightness step across the edge the same as in the native block, does one value do
one job (no speckle), is the palette still 4 colours. Fix the most used and most visible first: ground
tops, walls, platforms, then background. A tile used once in a corner needs less than a tile used 200 times.
Tiles 100% one colour cannot be hand-finished and do not count as raw.

Reuse instead of redrawing: `snippet save` a clean unit, `pxl pattern PART blocks UNIT --tile N` to fill a
tile, `pxl copy` between tiles of the same texture family, `mirror`/`flip` of a half. Stamp whole tiles with
`pxl patch` from `show --raw` of another tile. After any dedupe or fit step you must re-finish the tiles it
rewrote (they become raw again on purpose).

## Looking at a texture family

```
pxl lineup PART:blocks --region 0,0,64,32 --zoom 6          # a crop of the sheet at 1x and zoomed, labelled
pxl lineup zone_1:blocks zone_2:blocks --region 0,16,32,16   # the same material in two zones side by side
```
Read the PNGs. A texture that is fine on one tile and wrong when it repeats is a defect.

## Rules of the craft that only tiles have

- **The unit is on the grid.** Pattern units are 2, 4 or 8 px so the phase is the same in every tile; a
  pattern that drifts a pixel from one tile to the next shows as a seam in the level.
- **Edges first.** The row and column on a tile's border must continue into every neighbour the level places
  there (look at the neighbours in a lineup). Matching the border of the native block beats
  matching its interior.
- **Air stays air.** Solid cells keep a solid colour at their edge; air cells stay air.
- **Palette discipline.** One tile, 4 colours. Pick a palette per material and reuse it; the same material
  uses the same keys in every part of the set.
- **Animation** (waterfall, flowers, lava, torches): every frame is its own finished drawing;
  the frames are the same shape with the motion advanced by an even step; lineup all frames and check the loop
  closes (frame N to frame 0 is the same step). Palette cycles (`[[cycle]]`) are checked in the exported `palette_cycle/*.json` steps.
- **Fonts and screens**: glyphs are drawn 1:1 on the 8x8 grid, baseline in one row for every glyph;
  Screens are composed from tiles: fit, dedupe, then finish the tiles the
  screen uses most. A screen part is the console's exact screen size (`pxl console show`).

More: `references/tile-workflow.md` (commands, counts, what to read in each output),
`references/textures.md` (waterfalls, soil, brick, marble, metal, water, as 8x8 units).
`references/gbc-bg.md` (GBC BG features).
