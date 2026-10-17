# GBC background features (worked example)

The numbers and features of the Game Boy Color. Other consoles have their own (`pxl console show ID`); the discipline is the same.

- `pxl tiles stats PART` prints the count by index pattern next to the old colour count and the animated slots; `dedupe --max N`
  and lint `unique_tiles` use the same rule. Name animated parts in the zone part (`animated = ["../x/x.pxl"]`) so their slots count.
- `pxl tiles fit` packs into the palettes left after `bg_reserved`; lint `tile_palettes` fails when a level needs the reserved one.
  HUD, text and card tile sets are parts with `bg_palette = 7`, one frame named like the file (`hud_tiles`, `cards_tiles`), and
  `max_tiles = N` (cards: 96) when the package caps them.
- Foreground tiles (loop fronts, grass tips, pillars): `pxl tiles priority PART --auto` lists tiles whose source blocks have the
  high-priority bit, `--auto --apply` or `--set 3,7,10-12` / `--clear` writes `[priority]`; `priority.png` is exported. The
  see-through part of such a tile must be the air colour, which the export keeps as colour 0 of its palette (lint
  `priority_color0`).
- `[[cycle]]` writes `palette_cycle/<name>.json` (`bg_palette`, `frames_per_step`, `steps`; derive from an MD palette line with
  `file/line/first/count`, or give `targets` and `steps`); four-colour cycles also keep the swatch PNGs. `[[underwater]]` entries
  (`kind = "bg"|"obj"`, `palette`, a palette line or `colors`, `order`) write `palettes_underwater.json` in swap order.

