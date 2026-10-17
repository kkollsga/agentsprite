# pxl recipes

PART is the `.pxl` path `pxl next` prints (examples use `parts/player.pxl`), FRAME a frame name
(`02_wait1`). Coordinates are 0-based `x,y` from the top left. `--agent NAME` goes on every editing command.
Modifying commands on one part are serialised automatically (per-part lock); do not run edits on the same part in parallel anyway, it gains nothing.
All examples were run on a copy of a 32x32 project; they work as written.

## Look

```
pxl show PART FRAME --region 10,8,12,10 --ref   # text rows with labels + reference letters
pxl show PART FRAME --region 10,8,6,4 --raw      # labels only, ready to edit and patch back
pxl view PART FRAME --zoom 8 --region 8,6,16,14  # PNG: reference | frame, grid, coordinate labels
pxl compare PART --frames 02_wait1 --zoom        # one frame large; sheets in compare/
pxl compare PART --frames 02_wait1 --on "#406080" # judge at 1x on the level colour
pxl compare PART --frames 'walk*,tag:wait'       # selectors: name, wildcard, 6-11 (1-based), tag:NAME
pxl candidates PART FRAME                        # image: reference | current | vote contour exemplar auto nearest
pxl diff PART 02_wait1 --frames 03_wait2         # pixel differences between two frames
pxl lineup PART:FRAME PART2:all --zoom 4         # frames of several parts side by side, 1x and zoomed, labelled
pxl lint PART --frames FRAME --advice            # hardware errors + optional notes (stray pixels, jaggies...); `fix:` lines are ready commands
```
Read the PNGs with your image viewer. `show` legend: `l=#ff0000>R` means reference colour -> template key.

## Edit (each prints the changed region; each locks the frame; add --preview to get a PNG)

```
pxl set PART FRAME 14,11=W 15,11=outline --agent A1          # single pixels; key char or role
pxl row PART FRAME x=11 y=12 "1222" --agent A1               # or `11 12`; echoes "row y=12 from x=11"; space = unchanged
pxl patch PART FRAME 12 8 "13
22" --agent A1                                               # X Y GRID, rows by newline
pxl line PART FRAME 9,12 14,12 1 --agent A1                  # 1 px line
pxl rect PART FRAME 9,6,3,3 3 --outline --agent A1           # x,y,w,h
pxl fill PART FRAME 12,8 3 --agent A1                        # 4-connected flood
pxl replace PART FRAME 2=1 --region 9,6,6,8 --agent A1       # swap keys
pxl copy PART FROM TO --region 10,8,6,4 --agent A1           # same place; --to-at x,y to move it; --skip-transparent
pxl move PART FRAME --region 12,6,6,4 --by 1,0 --fill body --agent A1
pxl nudge PART FRAME 0,-1 --agent A1                         # whole frame(s); selectors allowed
pxl mirror PART FRAME x --keep low --agent A1                # symmetric; keep low|high half
pxl flip PART FRAME x --region 0,0,16,32 --agent A1
pxl outline PART FRAME --key outline --agent A1              # re-close the outline ring
pxl fix PART FRAME --rules orphan,jaggy --agent A1           # lint fixes (also: eye)
pxl clean PART --frames FRAME --orphans --jaggies --dry-run  # report; drop --dry-run to fix
pxl write_grid PART FRAME - --agent A1                       # replace the whole frame from stdin: exactly 32 rows of 32 chars
pxl undo PART --steps 2 --agent A1
```
Any edit also takes `--dry-run` (show, write nothing) and `--no-lock`.
Every command that takes a PART also takes a key of the nearest `pxl.toml` (`pxl build enemy_07`, `pxl show enemy_07 01_stand`).
Lock without drawing: `pxl lock PART 01_stand,02_walk*` or `pxl lock PART --all` (reload/load then keep those frames);
`pxl unlock PART FRAMES|--all` hands them back. Both are journaled (`pxl undo`) and refuse a part leased by someone else.

### The show -> patch round trip (the fastest manual edit)

```
pxl show PART FRAME --region 10,8,6,4 --raw       # copy these rows, change letters, then:
pxl patch PART FRAME - --agent A1                 # stdin = the edited rows; the row labels place them
```
Labels carry the position. `.` is transparent, space keeps the pixel. You can pipe one frame's region into another frame:
`pxl show PART A --region 10,8,6,4 --raw | pxl patch PART B - --agent A1`.

### Pitfalls
- `row` takes X then Y (`row PART FRAME 11 12 "..."` writes at x=11, y=12). `set`, `line`, `fill` take `x,y`.
- `rect` and `--region` are `x,y,w,h`; `copy --region` is read from the source frame.
- Negative coordinates are errors; to see the message put `--` before the pixels: `pxl set PART FRAME -- -1,5=W`.
- `write_grid` needs the full 32 rows; for a region use `patch`.
- An edit in a part claimed by another live agent is refused; `show`, `view`, `compare` still work.

## Template changed: old keys in stored frames
`pxl lint`/`export`/`reload` report `key 'W' is not defined in template ...`. Do not script it; run
`pxl template remap PART W=white [OLD=NEW ...] [--frames SEL] [--dry-run]` (NEW = key or role; OLD may be gone from the template;
frames keep their lock; undoable). `pxl reload PART --force` also regenerates frames holding old keys.

## Snippets: reusable features

```
pxl snippet save PART FRAME eye --region 14,11,3,3 --agent A1   # templates/snippets/eye.txt, shared by the template family
pxl snippet list PART
pxl stamp PART OTHER_FRAME eye 14,11 --flip x --preview --agent A1   # top left x,y; --flip x mirrors
```
Save the best eye, shoe and glove from your best frame; stamp them in each frame, then adjust the pupil.

## Pattern fill: textures as repeating units

```
pxl snippet save PART FRAME brick --region 0,0,8,8 --agent A1            # the clean unit (spaces keep the target pixel)
pxl pattern PART FRAME brick --region 0,8,32,16 --agent A1                # repeat it, phase 0,0 (sprites: the part origin)
pxl pattern PART FRAME brick --region 4,4,12,8 --phase 2,0 --mask 2 --agent A1   # only over cells of key 2; keeps the outline
pxl pattern PART FRAME --unit 0,0,2,2 --region 8,8,8,8 --agent A1         # unit cut from the frame itself (read before filling)
pxl pattern TILESET blocks brick --tile 37 --agent A1                     # one 8x8 tile, tile coordinates 0..7
pxl pattern suggest PART FRAME --region 0,8,32,16 --save brick          # find the unit in the REFERENCE: period, confidence, unit rows
pxl pattern suggest TILESET blocks --tiles 37-40 --max-period 8 --save wall   # tiles read as wrapping blocks; --tile N for one
```
`suggest` colour-classifies the native area, picks the smallest period that repeats (noise tolerant), majority-votes the unit over
all repeats, maps it to template keys and saves it as the snippet `pattern_NAME` rotated to the default phase of `pxl pattern`.
`cell(x,y) = unit[(y-py) mod h][(x-px) mod w]`: the same unit and phase tile identically in every region, frame, tile and
neighbouring part. `--mask KEY[,KEY]` limits the fill to cells currently of those keys. An edit like any other (lease, lock,
journal, `--preview`, `--dry-run`, `pxl undo`); apply op `{"op":"pattern","frame":F,"snippet":"brick"|"unit":[x,y,w,h],"region":[..],"phase":[px,py],"mask":"2"}`.

## Lineups

```
pxl lineup wall_a:1,2 wall_b:all --zoom 4 --out look/wall.png          # PART:frames (or PART:all); pages <= 1568 px
pxl lineup PART:FRAME --on "#406080" --region 8,6,16,14                # backdrop colour; crop every item
```
One sheet, each item at 1x and zoomed, labelled `part/frame`, wraps into pages; the same inputs give the same PNG.
Use it for textures, neighbouring pieces, variants and animation frames. Read the PNG.

## Review packets and rework notes (reviewer, coordinator; agents read their notes)

```
pxl review --state done --out rv                  # per part rv/NAME/: compare/ (all frames), zoom/ (worst), lineup.png, review.md
pxl review a.pxl b.pxl --family hero --worst 4    # named parts; only a family; frames zoomed
pxl approve PART --note "style reference: 01_stand" --agent R
pxl rework PART --frames 01_stand,walk* --note "eyes: one white block, draw two ..." --agent R   # --frames default: whole part
pxl status --rework                               # open notes per part and frame (a note closes when its frame is edited afterwards)
pxl status --metrics                              # approved first time / reworked once / twice+, top reasons, rounds per frame
```
The packet lineup holds the part and its siblings (same `family`, same template base, or `[review] siblings`), each item at 1x
and zoomed. `review.md` carries the checklist, the raw (not hand-finished) frames, open notes and a per-frame table to fill in.
A claiming agent sees its notes as `[frames] text` in `pxl next` / `pxl claim`.

## Raw output audit (is it hand-finished?)

```
pxl status                  # column raw: frames (tile sets: tiles) still equal to the last automatic output
pxl status --raw PART       # which frames / tile numbers
pxl check --hand-finished   # fails while any remain (opt-in)
```
`load`, `reload`, a whole-frame `pick` and `tiles fit` / `tiles dedupe` record their output; any later edit makes the frame (tile) yours.
Frames with no pixels and tiles with one colour never count.

### Finished without an edit: `pxl finish`

When the automatic output of a frame already equals the source within the palette (or is otherwise right), say so instead of touching a pixel:

```
pxl finish PART 01_stand,02_wait --reason "equals the source within the palette"
pxl finish PART blocks --tiles 37-40 --reason "flat sky tiles are right as reduced"   # tile parts: only these tiles
```

Pixels stay unchanged; the frames leave `pxl status --raw`. It is an edit-like step (lease, journaled, `pxl undo` removes it), and the reviewer sees every mark and reason in `review.md` under "Marked finished without edit"; `pxl rework PART --frames F --note ...` reopens a frame as raw. Do not use it to silence the audit on frames you have not looked at. An approved part refuses all edits (also `finish`, `apply`, `pick`, `load`, `reload`): reopen with `pxl rework PART --note ...` first (`--force-approved` overrides).

## Candidates and pick

```
pxl candidates PART FRAME                          # Read compare/<part>_cand_FRAME.png; lower score = closer; * = best
pxl pick PART FRAME contour --region 9,6,14,10 --preview --agent A1   # one method for one region
pxl reload PART --try vote,contour,exemplar,auto   # compare methods per frame; writes nothing
pxl reload PART --method auto                      # re-reduce unlocked frames from your locked ones (locked never overwritten; `reload --force` or `load --force` regenerates locked frames too and loses their hand edits)
```
Take the face from one candidate and the body from another; then hand-draw the key features.

## Batches: pxl apply PART ops.json (atomic, one undo step; `-` reads stdin; add --dry-run to test)

Fields per op (all take `"frame"` except copy). Points `[x,y]`, regions `[x,y,w,h]`, keys characters or roles.
```
{"op":"set","frame":F,"pixels":["14,11=W","15,11=white"]}
{"op":"row","frame":F,"x":11,"y":12,"pixels":"1222"}
{"op":"patch","frame":F,"x":12,"y":8,"grid":["13","22"]}
{"op":"line","frame":F,"from":[9,12],"to":[14,12],"key":"1"}
{"op":"rect","frame":F,"region":[9,6,3,3],"key":"3","outline":true}
{"op":"fill","frame":F,"at":[12,8],"key":"3"}
{"op":"replace","frame":F,"maps":["2=1"],"region":[9,6,6,8]}
{"op":"copy","from":F1,"to":F2,"region":[9,6,6,8],"to_at":[0,0],"skip_transparent":false}
{"op":"mirror","frame":F,"axis":"x","keep":"low","region":[0,0,16,32]}
{"op":"flip","frame":F,"axis":"x","region":[0,0,16,32]}
{"op":"write_grid","frame":F,"grid":["32 rows of 32 chars"]}
{"op":"pick","frame":F,"method":"contour","region":[9,6,14,10]}
{"op":"stamp","frame":F,"name":"eye","at":[14,11],"flip":"x"}
{"op":"move","frame":F,"region":[12,6,6,4],"by":[1,0],"fill":"body"}
{"op":"nudge","frame":F,"by":[0,-1]}
{"op":"outline","frame":F,"key":"outline"}
{"op":"fix","frame":F,"rules":["orphan","jaggy"]}
{"op":"pattern","frame":F,"snippet":"brick","region":[0,0,16,16],"phase":[0,0],"mask":"2"}
```
Typical use: stamp the eye and shoe snippets into six walk frames in one call. Unknown fields are errors
that list the allowed fields. Wrap a list in `[...]`.

## Deliver

```
pxl lint PART            # 0 errors required (hardware limits); --advice adds notes, never required
pxl export PART          # indexed PNG per layer + .aseprite
pxl release PART --state done --note "kept X, exaggerated Y, weak: Z" --agent A1
pxl release PART --state blocked --note "why" --agent A1
pxl renew PART --agent A1    # extend the lease (every edit does it); lease expires after 15 minutes idle
pxl status                   # state, owner, stale, export freshness
```
Hardware limit `hw_limit` (error): at most 10 occupied 8x16 cells per 16 px band and 40 per frame (profile `hw_cells_per_row_max`/`hw_cells_max`, the cell grid is searched per frame, not counted from the canvas corner: see `cells.md`); over it, split or narrow the sprite. The 12/5 `hw_cells` budget stays a warning.
Never edit templates or the manifest; ask the coordinator.
