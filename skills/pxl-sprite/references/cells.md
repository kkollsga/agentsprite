# Cells, palette sets and tile memory

How hardware cells shape the drawing, and the commands that show and fit them. Numbers (cell size, colours per cell, per-line limit,
palette count) come from `pxl console show`. The examples use the Game Boy Color (8x16 cells, one 3-colour palette slot per cell,
10 cells per line); the NES (8x16 or 8x8 cells, 16x16 attribute blocks on the background) and the Game Boy follow the same idea with other numbers.
Flat consoles (sms, gg, md, snes, pce, gba) give each sprite one palette and limit only per asset: skip the shared-set parts.

## Palette-set parts (shared sprite palettes)

A part with `palette_set = "zone_2"` draws with plain keys whose colours come from the shared set; the palette of each 8x16 cell is
derived (one slot holds all its colours; a few cells of the player may use two, see Stacked sprites below). `pxl cells PART FRAME --png c.png` shows the cell layout, slot per cell and sprites per
scanline (invalid cells red); `pxl show PART FRAME --cells` adds the cell grid as text and every edit warns when a cell now needs a
second slot. Draw colour changes on cell borders (8 px columns, 16 px rows) and pack the figure into few cells. To move an existing part
into a set: `pxl palette fit PART --set zone_2 --png fit.png` (dry run, folds listed), then `--apply` (undo works). Tile memory counts
unique 8x16 patterns with flips free: repeat cells, mirror halves, reuse bodies; `pxl budget SCENE` reports the group totals and
`pxl cells dedupe PART --max N` merges near-identical cells (the report lists every merge's pixel difference). Enemies used in several
zones declare `palette_set = ["zone_1", "zone_3"]`: one layout and one slot index per cell serve every zone, `palette fit` minimises
the summed error over the sets and reports each, and `pxl cells --png --set zone_3` (or `compare --set`) shows another zone's colours.

## Where the cells fall (they are not counted from the canvas corner)

The hardware places 8x16 cells freely, so the tool does not tile the canvas from its top-left. For every frame it searches a band offset `oy`
(0 to 15 rows) and, for every 16-row band, a column offset `ox` (0 to 7), and keeps the layout with the fewest invalid cells, then the fewest
sprites on a scanline, then the fewest sprites in all; ties go to the smallest offsets, so the canvas-aligned grid wins when nothing is better.
`pxl cells PART FRAME --png c.png` shows the layout it chose (cell borders, slot per cell, sprites per scanline) and `pxl cells PART FRAME`
prints it as text. Two consequences for drawing:

- Do not assume the cell border is at row 16 or column 8 of the canvas. Look at the chosen layout first, then put a colour change on *its*
  borders. A figure whose colour bands sit 16 rows apart (head, torso, legs and shoes, each one material ramp) gives the search an easy layout;
  a colour change in the middle of a band forces a second slot.
- Moving the whole figure by a row or a column can change the layout. If a frame needs an offset the others do not, the cells of that frame
  differ from its neighbours (unchanged cells between frames are stored once). Keep the figure on the same rows from frame to frame.

A layout you want exactly is written beside the part as `<frame>.cells.json` (`[{"x":-8,"y":-16,"slot":0}]`, relative to the origin, optional
`slot2`); it replaces the search and must cover every opaque pixel.

## Stacked sprites (two-slot cells, the player only)

A cell normally holds one 3-colour slot. For the category `player` a limited number of cells per frame (`multi_slot_max`, 2 on the GBC; see
`pxl console show`) may be **stacked**: the same 8x16 spot is drawn by two sprites, the base with slot A and an overlay with slot B (`slot` and
`slot2` in the cells output). Together they hold up to 6 colours, so a head with outline, hair, skin, white eye, pupil and a cap colour fits one
spot. The price is 2 sprites for that cell, and a scanline that carries one may hold more sprites (`budget.player.overlay_line`) but still counts
against the 10 per line. Every other category must fit each cell in one slot: a cell that needs a second slot there is an error (`cell_palette`).

How to use it:

- Give the asset the category. `pxl ingest --group player` does it (the mapping group to category is `[project.defaults] group_categories`,
  built in: `player = "player"`; change it with `pxl project set defaults.group_categories.enemies enemy`). For an existing asset:
  `pxl asset set hero category player` (or `pxl meta hero set category player`). Then `pxl palette fit hero --apply` folds the cells again with
  the stack allowed.
- `pxl palette plan SCENE` proposes the stacked cells: for a player part it lists the cells that need 4 to 6 colours, the two slots that would
  hold them and the error with one slot and with two, at most `multi_slot_max` per frame. Spend them where the picture gains most: the face band
  (eyes plus hair plus skin) and the shoe band.
- Draw the stacked cell as one picture: use any of the six colours inside it, and keep the colour boundary between the two slots off the eyes
  and the outline so a slot change never reads as a seam. `pxl cells PART FRAME --png c.png` shows which cells are stacked ("two slots: 2 + 4
  (2 sprites)"); `pxl lint` reports a frame with more stacked cells than the allowance.
- The per-frame allowance is small. A frame needing a stacked cell in every band is a frame to redraw, not to stack: put the colour changes on
  the cell borders first.

## Draw for the cells

Plan the drawing around the layout the tool made (`pxl cells PART FRAME --png c.png`):

- **Identity colours in their own band** (GBC example: the lowest 16 px row holds the shoe colours; do not let red climb into the leg cell).
- **Icons in their own cell** (monitor icons, signs): a shared body plus one cell per icon.
- **Shared bodies, flips, repeating segments**: draw a symmetric half once (the mirror is free), repeat columns, chains, spikes and
  girders from one segment; unchanged cells between frames are stored once.
- **The full ramp per cell**: use dark, mid and light of the slot inside each cell; a cell with two colours looks flat.
- **No palette seams**: change the slot on a cell border only where the surface changes (cloth to shoe), and keep the hue family so the
  border does not read as a stripe. Check the cell PNG, and in every zone's colours for multi-set parts (`--set ZONE`).
- **One flat layer of keys.** A palette-set part is drawn with plain keys whose colours all come from the set; the slot of a cell is derived, never
  drawn. Extra template layers (overlays) are an error unless the console profile allows them: the stacked cells above are how the player gets
  a second slot, and they are derived from the same keys.
