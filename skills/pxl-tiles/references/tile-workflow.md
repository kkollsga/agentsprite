# Tile workflow reference

PART is the tileset `.pxl`; the frame is `blocks` (the default) unless `--frame` says otherwise. Tile N is the
N-th 8x8 tile of the sheet (16 per row, left to right), which is also block N of the source picture.

## Read these outputs

| Command | What to read |
|---|---|
| `pxl tiles stats PART` | `unique tiles ... of ...` (the console's unique-tile budget; GBC 348), `colours per tile` (nothing over the console's limit; GBC 4), `palettes: checker first-fit N (max = the console's palettes; GBC 8)`, duplicate groups |
| `pxl tiles fit PART [--apply\|--write ops.json]` | which tiles get recoloured and the cost; fit never touches the shapes |
| `pxl tiles dedupe PART --max N [--apply]` (N = the console budget) | every merge with its error: invisible, low, visible. Visible merges are reported; undo and redraw them by hand |
| `pxl compare PART` | tile pages: reference block, tile at 2x, diff; ranked worst error x most used. Look at the first pages |
| `pxl lint PART` | `tile_colors`, `tile_palettes`, `unique_tiles` (errors) |
| `pxl status --raw [PART]` | per frame, the tile numbers that are still automatic output; goal: none |

## Editing

All edits are leased, locked, journaled (`pxl undo`) and take `--preview` and `--dry-run`.
`--tile N` makes `set`, `row`, `patch` and `pattern` use tile coordinates 0..7. Without `--tile`, coordinates are
sheet pixels (tile N starts at x = (N mod 16) * 8, y = (N div 16) * 8). Many tiles in one undo step: `pxl apply PART ops.json`
with `patch`, `set`, `copy`, `pattern` ops on the `blocks` frame.

Order of work that avoids redoing things: fit, dedupe, then hand-finish. A fit or dedupe run after hand work re-marks the
tiles it rewrote as raw, and overwrites your drawing there.

## Animated parts and cycles

An animated tile part is a tileset part with one frame per animation frame (`waterfall_00`, `_01`, ...). Finish each frame as a tile sheet; export puts them in `animated/`. Palette cycles are declared
in the part (`[[cycle]]`, explicit `bg_palette`, `targets` and `steps`), exported to `palette_cycle/<name>.json`; check the colour order shifts evenly.

## Counting what remains

`pxl status --raw PART` is the to-do list. `pxl check --hand-finished` is the gate the coordinator runs; it is opt-in so a part
that is mid-work does not fail the normal `pxl check`.
